use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use ratatui::prelude::Position;
use ratatui::style::Stylize;
use ratatui::{
    Frame,
    layout::{Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Paragraph},
};

const SCROLLBACK_SIZE: usize = 1200;

pub struct MuxCallbacks {
    writer: Arc<Mutex<Option<Box<dyn Write + Send>>>>,
    title: Arc<Mutex<Option<String>>>,
    title_changed: Arc<AtomicBool>,
}

impl vt100::Callbacks for MuxCallbacks {
    fn set_window_title(&mut self, _screen: &mut vt100::Screen, title: &[u8]) {
        if let Ok(s) = std::str::from_utf8(title) {
            *self.title.lock().unwrap() = Some(s.to_string());
            self.title_changed.store(true, Ordering::Relaxed);
        }
    }

    fn set_window_icon_name(&mut self, _screen: &mut vt100::Screen, icon_name: &[u8]) {
        // treat OSC 1 the same as OSC 2 if you want icon-name-only tools to count
        if let Ok(s) = std::str::from_utf8(icon_name) {
            *self.title.lock().unwrap() = Some(s.to_string());
            self.title_changed.store(true, Ordering::Relaxed);
        }
    }
    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        i1: Option<u8>,
        _i2: Option<u8>,
        params: &[&[u16]],
        c: char,
    ) {
        let first = params.first().and_then(|p| p.first()).copied();
        let reply: Option<Vec<u8>> = match (i1, c) {
            (None, 'c') => Some(b"\x1b[?62;22c".to_vec()),
            (Some(b'>'), 'c') => Some(b"\x1b[>0;1;0c".to_vec()),
            (None, 'n') => match first {
                Some(5) => Some(b"\x1b[0n".to_vec()),
                Some(6) => {
                    let (row, col) = screen.cursor_position();
                    Some(format!("\x1b[{};{}R", row + 1, col + 1).into_bytes())
                }
                _ => None,
            },
            (Some(b'?'), 'n') => {
                if first == Some(6) {
                    let (row, col) = screen.cursor_position();
                    Some(format!("\x1b[{};{}R", row + 1, col + 1).into_bytes())
                } else {
                    None
                }
            }
            _ => None,
        };
        if let Some(bytes) = reply
            && let Some(w) = self.writer.lock().unwrap().as_mut()
        {
            let _ = w.write_all(&bytes);
        }
    }
}

pub struct Pane {
    pub vpty: Arc<Mutex<vt100::Parser<MuxCallbacks>>>,
    pty_writer: Arc<Mutex<Option<Box<dyn Write + Send>>>>,
    pub pty_master: Box<dyn MasterPty>,
    pub screen_changed: Arc<AtomicBool>,
    // Scroll position tracking
    // Last size this pane's virtual terminal was set to
    rows: u16,
    cols: u16,
    pub title: String,
    title_shared: Arc<Mutex<Option<String>>>,
    title_changed: Arc<AtomicBool>,
}

impl Pane {
    pub fn new(row: u16, coll: u16) -> Result<Self> {
        let pty_system = native_pty_system();

        let pair = pty_system.openpty(PtySize {
            rows: row,
            cols: coll,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into());
        let cmd = CommandBuilder::new(shell);
        let _child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);
        let pty_writer = Arc::new(Mutex::new(Some(pair.master.take_writer()?)));
        let title_shared = Arc::new(Mutex::new(None));
        let title_changed = Arc::new(AtomicBool::new(false));

        let callbacks = MuxCallbacks {
            writer: pty_writer.clone(),
            title: title_shared.clone(),
            title_changed: title_changed.clone(),
        };
        let vpty = Arc::new(Mutex::new(vt100::Parser::new_with_callbacks(
            row,
            coll,
            SCROLLBACK_SIZE,
            callbacks,
        )));
        let vpt_clone = Arc::clone(&vpty);

        let screen_changed = Arc::new(AtomicBool::new(true));
        let sc_clone = Arc::clone(&screen_changed);

        let mut reader = pair.master.try_clone_reader()?;
        let _reader_thread = std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        vpt_clone.lock().unwrap().process(&buf[..n]);
                        sc_clone.store(true, Ordering::Relaxed);
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(Pane {
            vpty,
            pty_writer,
            pty_master: pair.master,
            screen_changed,
            rows: row,
            cols: coll,
            title: "~".to_string(),
            title_shared,
            title_changed,
        })
    }
    /// Forward a key event to the pane's PTY as the bytes a real terminal
    /// would send for it. No-op if the writer is gone (child exited).
    pub fn write_key(&mut self, key: KeyEvent) -> Result<()> {
        if let Some(w) = self.pty_writer.lock().unwrap().as_mut() {
            w.write_all(&key_to_bytes(&key))?;
        }
        Ok(())
    }

    pub fn sync_title(&mut self) -> bool {
        if self.title_changed.swap(false, Ordering::Relaxed)
            && let Some(t) = self.title_shared.lock().unwrap().clone()
        {
            self.title = t;
            return true; // title actually changed, redraw tab bar etc.
        }

        false
    }
    // Set scroll position using vt100 parser
    pub fn set_scroll_offset(&mut self, offset: usize) {
        let mut parser = self.vpty.lock().unwrap();
        parser.screen_mut().set_scrollback(offset);
        log::debug!("offset {offset}");
    }

    // Get current scroll offset from vt100 parser
    pub fn get_scroll_offset(&self) -> usize {
        let parser = self.vpty.lock().unwrap();
        parser.screen().scrollback()
    }

    // Scroll up by lines (increase scrollback offset to show older content)
    pub fn scroll_up(&mut self, lines: usize) {
        let current = self.get_scroll_offset();
        let new_offset = current.saturating_add(lines);
        log::debug!("{new_offset} new offset");
        self.set_scroll_offset(new_offset);
    }

    // Scroll down by lines (decrease scrollback offset to show newer content)
    pub fn scroll_down(&mut self, lines: usize) {
        let current = self.get_scroll_offset();
        let new_offset = current.saturating_sub(lines);
        self.set_scroll_offset(new_offset);
    }
    // Scroll to top (offset = 0)
    pub fn scroll_to_top(&mut self) {
        self.set_scroll_offset(usize::MAX);
    }

    // Scroll to bottom (offset = max scrollback)
    pub fn scroll_to_bottom(&mut self) {
        self.set_scroll_offset(0);
    }

    // Get number of visible lines (this pane's current virtual terminal height)
    pub fn visible_lines(&self) -> usize {
        self.rows as usize
    }

    /// Resize the backing PTY and the vt100 screen so both stay in sync.
    /// Skips the work (and avoids a spurious SIGWINCH on the shell) when the
    /// requested size is unchanged.
    pub fn resize(&mut self, rows: u16, cols: u16) {
        let rows = rows.max(1);
        let cols = cols.max(1);
        if rows == self.rows && cols == self.cols {
            return;
        }
        self.rows = rows;
        self.cols = cols;
        self.pty_master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        self.vpty.lock().unwrap().screen_mut().set_size(rows, cols);
    }

    pub fn render_pane(&self, frame: &mut Frame, area: Rect, is_active: bool) {
        let screen = {
            let parser = self.vpty.lock().unwrap_or_else(|e| e.into_inner());
            parser.screen().clone()
        };
        let (screen_rows, _cols) = screen.size();
        let inner = area.inner(Margin {
            horizontal: 1,
            vertical: 1,
        });
        let rows = (screen_rows as usize).min(inner.height as usize);
        let text = vterm_to_ratatui(&screen, rows);
        frame.render_widget(
            Paragraph::new(text)
                .block(Block::bordered().title(self.title.clone().bold().fg(Color::Cyan))),
            area,
        );

        if is_active {
            let (row, col) = screen.cursor_position();
            let view_row = row as usize + screen.scrollback();
            if view_row < inner.height as usize {
                let x = (inner.x + col).min(inner.right().saturating_sub(1));
                frame.set_cursor_position(Position::new(x, inner.y + view_row as u16));
            }
        }
    }
}
/// Encode a key event as the bytes a terminal program expects.
///
/// Alt-modified keys use the standard xterm "meta sends escape" encoding: the
/// key's unmodified bytes prefixed with ESC.
fn key_to_bytes(key: &KeyEvent) -> Vec<u8> {
    let mut bytes = unmodified_key_to_bytes(key);
    if key.modifiers.contains(KeyModifiers::ALT) && !bytes.is_empty() {
        bytes.insert(0, 0x1b);
    }
    bytes
}

fn unmodified_key_to_bytes(key: &KeyEvent) -> Vec<u8> {
    match key.code {
        KeyCode::Enter => b"\r".to_vec(),
        KeyCode::Tab => b"\t".to_vec(),
        KeyCode::Backspace => b"\x7f".to_vec(),
        KeyCode::Esc => b"\x1b".to_vec(),
        KeyCode::Up => b"\x1b[A".to_vec(),
        KeyCode::Down => b"\x1b[B".to_vec(),
        KeyCode::Right => b"\x1b[C".to_vec(),
        KeyCode::Left => b"\x1b[D".to_vec(),
        KeyCode::Delete => b"\x1b[3~".to_vec(),
        KeyCode::Char(c) if key.modifiers.contains(KeyModifiers::CONTROL) => {
            match control_char_byte(c) {
                Some(b) => vec![b],
                None => c.to_string().into_bytes(),
            }
        }
        KeyCode::Char(c) => c.to_string().into_bytes(),
        _ => Vec::new(),
    }
}

fn control_char_byte(c: char) -> Option<u8> {
    let c = c.to_ascii_lowercase();
    match c {
        'a'..='z' => Some(c as u8 - b'a' + 1),
        '@' | ' ' => Some(0x00),
        '[' => Some(0x1b),
        '\\' => Some(0x1c),
        ']' => Some(0x1d),
        '^' => Some(0x1e),
        '_' | '/' => Some(0x1f),
        '?' => Some(0x7f),
        _ => None,
    }
}

fn build_style(cell: &vt100::Cell) -> Style {
    let mut style = Style::default();

    // fg color
    style = style.fg(match cell.fgcolor() {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(n) => Color::Indexed(n),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    });

    // bg color
    style = style.bg(match cell.bgcolor() {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(n) => Color::Indexed(n),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    });

    if cell.bold() {
        style = style.add_modifier(Modifier::BOLD);
    }
    if cell.italic() {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if cell.underline() {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    if cell.dim() {
        style = style.add_modifier(Modifier::DIM);
    }
    if cell.inverse() {
        // swap fg and bg
        let fg = style.fg.unwrap_or(Color::Reset);
        style = style.fg(style.bg.unwrap_or(Color::Reset));
        style = style.bg(fg);
    }

    style
}

fn vterm_to_ratatui(screen: &vt100::Screen, visible_rows: usize) -> Text<'static> {
    let size = screen.size();
    let (_rows, cols) = size;
    let mut lines = Vec::with_capacity(visible_rows);
    // Build an empty fill row (spaces with default style) for the non-occupied area
    // Then iterate each row, then each column within that row
    for row in 0..visible_rows as u16 {
        let mut spans = vec![];
        let mut col: u16 = 0;
        while col < cols {
            // Adjust row index based on scroll position
            match screen.cell(row, col) {
                Some(cell) if !cell.is_wide_continuation() => {
                    let style = build_style(cell);
                    let content = if cell.has_contents() {
                        cell.contents().to_string()
                    } else {
                        " ".to_string()
                    };
                    spans.push(Span::styled(content, style));
                    if cell.is_wide() {
                        col += 1;
                    }
                }
                _ => spans.push(Span::raw(" ")),
            }
            col += 1;
        }
        lines.push(Line::from(spans));
    }
    Text::from(lines)
}
#[cfg(test)]
mod tests {
    use super::*;

    struct TestWriter(Arc<Mutex<Vec<u8>>>);

    impl Write for TestWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn test_parser() -> (vt100::Parser<MuxCallbacks>, Arc<Mutex<Vec<u8>>>) {
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let writer: Box<dyn Write + Send> = Box::new(TestWriter(Arc::clone(&bytes)));
        let writer = Arc::new(Mutex::new(Some(writer)));
        let title_shared = Arc::new(Mutex::new(None));
        let title_changed = Arc::new(AtomicBool::new(false));

        let parser = vt100::Parser::new_with_callbacks(
            24,
            80,
            SCROLLBACK_SIZE,
            MuxCallbacks {
                writer: Arc::clone(&writer),
                title_changed,
                title: title_shared,
            },
        );
        (parser, bytes)
    }

    #[test]
    fn da1_replies_vt220() {
        let (mut parser, bytes) = test_parser();
        parser.process(b"\x1b[c");
        assert_eq!(*bytes.lock().unwrap(), b"\x1b[?62;22c".to_vec());
    }

    #[test]
    fn dsr5_replies_ok() {
        let (mut parser, bytes) = test_parser();
        parser.process(b"\x1b[5n");
        assert_eq!(*bytes.lock().unwrap(), b"\x1b[0n".to_vec());
    }

    #[test]
    fn cpr_reports_one_based_position() {
        let (mut parser, bytes) = test_parser();
        parser.process(b"\x1b[3;5H\x1b[6n");
        assert_eq!(*bytes.lock().unwrap(), b"\x1b[3;5R".to_vec());
    }

    #[test]
    fn cpr_reports_home_as_1_1() {
        let (mut parser, bytes) = test_parser();
        parser.process(b"\x1b[H\x1b[6n");
        assert_eq!(*bytes.lock().unwrap(), b"\x1b[1;1R".to_vec());
    }

    #[test]
    fn private_cpr_is_answered() {
        let (mut parser, bytes) = test_parser();
        parser.process(b"\x1b[2;2H\x1b[?6n");
        assert_eq!(*bytes.lock().unwrap(), b"\x1b[2;2R".to_vec());
    }

    #[test]
    fn da2_replies_generic_terminal() {
        let (mut parser, bytes) = test_parser();
        parser.process(b"\x1b[>0c");
        assert_eq!(*bytes.lock().unwrap(), b"\x1b[>0;1;0c".to_vec());
    }

    #[test]
    fn printer_status_is_ignored() {
        let (mut parser, bytes) = test_parser();
        parser.process(b"\x1b[?5n");
        assert!(bytes.lock().unwrap().is_empty());
    }

    fn pressed(code: KeyCode, modifiers: KeyModifiers) -> Vec<u8> {
        key_to_bytes(&KeyEvent::new(code, modifiers))
    }

    #[test]
    fn control_letters_encode_to_c0() {
        for (i, c) in ('a'..='z').enumerate() {
            assert_eq!(
                pressed(KeyCode::Char(c), KeyModifiers::CONTROL),
                vec![(i + 1) as u8],
                "Ctrl+{c}"
            );
        }
    }

    #[test]
    fn control_uppercase_and_shift_are_normalized() {
        assert_eq!(
            pressed(KeyCode::Char('C'), KeyModifiers::CONTROL),
            vec![0x03],
            "Ctrl+C reported as uppercase"
        );
        assert_eq!(
            pressed(
                KeyCode::Char('C'),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT
            ),
            vec![0x03],
            "Ctrl+Shift+C"
        );
    }

    #[test]
    fn control_punctuation_covers_rest_of_c0() {
        let cases = [
            ('@', 0x00),
            (' ', 0x00),
            ('[', 0x1b),
            ('\\', 0x1c),
            (']', 0x1d),
            ('^', 0x1e),
            ('_', 0x1f),
            ('/', 0x1f),
            ('?', 0x7f),
        ];
        for (c, expected) in cases {
            assert_eq!(
                pressed(KeyCode::Char(c), KeyModifiers::CONTROL),
                vec![expected],
                "Ctrl+{c}"
            );
        }
    }

    #[test]
    fn control_on_unmappable_char_is_sent_literally() {
        assert_eq!(
            pressed(KeyCode::Char('1'), KeyModifiers::CONTROL),
            b"1".to_vec()
        );
        assert_eq!(
            pressed(KeyCode::Char('é'), KeyModifiers::CONTROL),
            "é".as_bytes().to_vec()
        );
    }

    #[test]
    fn navigation_and_edit_keys_get_terminal_sequences() {
        let cases = [
            (KeyCode::Enter, &b"\r"[..]),
            (KeyCode::Tab, b"\t"),
            (KeyCode::Backspace, b"\x7f"),
            (KeyCode::Esc, b"\x1b"),
            (KeyCode::Up, b"\x1b[A"),
            (KeyCode::Down, b"\x1b[B"),
            (KeyCode::Right, b"\x1b[C"),
            (KeyCode::Left, b"\x1b[D"),
            (KeyCode::Delete, b"\x1b[3~"),
            (KeyCode::Null, b""),
        ];
        for (code, expected) in cases {
            assert_eq!(pressed(code, KeyModifiers::NONE), expected, "{code:?}");
        }
    }

    #[test]
    fn plain_chars_are_sent_as_utf8() {
        assert_eq!(
            pressed(KeyCode::Char('a'), KeyModifiers::NONE),
            b"a".to_vec()
        );
        assert_eq!(
            pressed(KeyCode::Char('é'), KeyModifiers::NONE),
            "é".as_bytes().to_vec()
        );
    }

    #[test]
    fn alt_keys_are_esc_prefixed() {
        // xterm "meta sends escape": Alt+key = ESC + unmodified bytes.
        assert_eq!(
            pressed(KeyCode::Char('x'), KeyModifiers::ALT),
            b"\x1bx".to_vec()
        );
        assert_eq!(
            pressed(KeyCode::Enter, KeyModifiers::ALT),
            b"\x1b\r".to_vec()
        );
        assert_eq!(
            pressed(KeyCode::Backspace, KeyModifiers::ALT),
            b"\x1b\x7f".to_vec()
        );
        assert_eq!(
            pressed(KeyCode::Up, KeyModifiers::ALT),
            b"\x1b\x1b[A".to_vec()
        );
        // Alt composes with Ctrl: ESC + the control byte.
        assert_eq!(
            pressed(
                KeyCode::Char('c'),
                KeyModifiers::ALT | KeyModifiers::CONTROL
            ),
            b"\x1b\x03".to_vec()
        );
    }
}
