use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
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

#[derive(Clone, Default)]
struct SharedTitle {
    title: Arc<Mutex<Option<String>>>,
    changed: Arc<AtomicBool>,
}

impl SharedTitle {
    fn set(&self, title: String) {
        *self.title.lock().unwrap_or_else(|e| e.into_inner()) = Some(title);
        self.changed.store(true, Ordering::Relaxed);
    }

    fn set_from_bytes(&self, title: &[u8]) {
        if let Ok(s) = std::str::from_utf8(title) {
            self.set(s.to_string());
        }
    }

    fn take_if_changed(&self) -> Option<String> {
        self.changed
            .swap(false, Ordering::Relaxed)
            .then(|| self.title.lock().unwrap_or_else(|e| e.into_inner()).clone())
            .flatten()
    }
}

struct MuxCallbacks {
    writer: Arc<Mutex<Option<Box<dyn Write + Send>>>>,
    title: SharedTitle,
}

impl vt100::Callbacks for MuxCallbacks {
    fn set_window_title(&mut self, _screen: &mut vt100::Screen, title: &[u8]) {
        self.title.set_from_bytes(title);
    }

    fn set_window_icon_name(&mut self, _screen: &mut vt100::Screen, icon_name: &[u8]) {
        // treat OSC 1 the same as OSC 2 if you want icon-name-only tools to count
        self.title.set_from_bytes(icon_name);
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
            && let Some(w) = self
                .writer
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_mut()
        {
            let _ = w.write_all(&bytes);
        }
    }
}

fn read_loop(
    vpty: &Mutex<vt100::Parser<MuxCallbacks>>,
    screen_changed: &AtomicBool,
    exited: &AtomicBool,
    mut reader: impl Read,
) {
    let mut buf = [0u8; 4096];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => {
                exited.store(false, Ordering::Relaxed);
                break;
            }
            Ok(n) => {
                vpty.lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .process(&buf[..n]);
                screen_changed.store(true, Ordering::Relaxed);
            }
            Err(_) => {
                exited.store(false, Ordering::Relaxed);
                break;
            }
        }
    }
}

struct PtySession {
    vpty: Arc<Mutex<vt100::Parser<MuxCallbacks>>>,
    writer: Arc<Mutex<Option<Box<dyn Write + Send>>>>,
    master: Box<dyn MasterPty>,
    child: Box<dyn Child + Send + Sync>,
    screen_changed: Arc<AtomicBool>,
    exited: Arc<AtomicBool>,
    title: SharedTitle,
    rows: u16,
    cols: u16,
}

impl PtySession {
    fn in_alternate_screen(&self) -> bool {
        self.vpty.lock().unwrap().screen().alternate_screen()
    }

    fn spawn(rows: u16, cols: u16) -> Result<Self> {
        let pair = native_pty_system().openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into());
        let cmd = CommandBuilder::new(shell);
        let child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);

        let writer = Arc::new(Mutex::new(Some(pair.master.take_writer()?)));
        let title = SharedTitle::default();
        let vpty = Arc::new(Mutex::new(vt100::Parser::new_with_callbacks(
            rows,
            cols,
            SCROLLBACK_SIZE,
            MuxCallbacks {
                writer: Arc::clone(&writer),
                title: title.clone(),
            },
        )));
        let screen_changed = Arc::new(AtomicBool::new(true));
        let vpt_clone = Arc::clone(&vpty);
        let sc_clone = Arc::clone(&screen_changed);

        let reader = pair.master.try_clone_reader()?;
        let exited = Arc::new(AtomicBool::new(false));
        let ex_clone = Arc::clone(&exited);
        let _reader_thread =
            std::thread::spawn(move || read_loop(&vpt_clone, &sc_clone, &ex_clone, reader));

        Ok(PtySession {
            vpty,
            writer,
            master: pair.master,
            child,
            screen_changed,
            exited,
            title,
            rows,
            cols,
        })
    }

    fn write_bytes(&self, bytes: &[u8]) -> Result<()> {
        if self.is_not_alive() {
            return Ok(());
        }
        if let Some(w) = self
            .writer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_mut()
        {
            w.write_all(bytes)?;
        }
        Ok(())
    }

    fn take_screen_changed(&self) -> bool {
        self.screen_changed.swap(false, Ordering::Relaxed)
    }

    fn is_not_alive(&self) -> bool {
        self.exited.load(Ordering::Relaxed)
    }

    fn take_title(&self) -> Option<String> {
        self.title.take_if_changed()
    }

    fn size(&self) -> (u16, u16) {
        (self.rows, self.cols)
    }

    /// Resize the backing PTY and the vt100 screen so both stay in sync.
    /// Skips the work (and avoids a spurious SIGWINCH on the shell) when the
    /// requested size is unchanged.
    fn resize(&mut self, rows: u16, cols: u16) {
        let rows = rows.max(1);
        let cols = cols.max(1);
        if rows == self.rows && cols == self.cols {
            return;
        }
        self.rows = rows;
        self.cols = cols;
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        self.vpty
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .screen_mut()
            .set_size(rows, cols);
    }

    fn scroll_offset(&self) -> usize {
        self.vpty
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .screen()
            .scrollback()
    }

    fn set_scroll_offset(&self, offset: usize) {
        self.vpty
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .screen_mut()
            .set_scrollback(offset);
        log::debug!("offset {offset}");
    }

    fn screen(&self) -> vt100::Screen {
        self.vpty
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .screen()
            .clone()
    }
}

pub struct Pane {
    session: PtySession,
    pub title: String,
}

impl Pane {
    pub fn new(rows: u16, cols: u16) -> Result<Self> {
        Ok(Pane {
            session: PtySession::spawn(rows, cols)?,
            title: "~".to_string(),
        })
    }
    /// Forward a key event to the pane's PTY as the bytes a real terminal
    /// would send for it. No-op if the writer is gone (child exited).
    pub fn write_key(&mut self, key: KeyEvent) -> Result<()> {
        self.session.write_bytes(&key_to_bytes(&key))
    }

    pub fn sync_title(&mut self) -> bool {
        if let Some(t) = self.session.take_title() {
            if self.is_not_alive() && !self.title.starts_with("[exited]") {
                self.title = format!("[exited] {}", self.title);
            } else {
                self.title = t;
            }
            return true; // title actually changed, redraw tab bar etc.
        }

        false
    }

    pub fn in_alternated_state(&self) -> bool {
        self.session.in_alternate_screen()
    }

    pub fn take_screen_changed(&self) -> bool {
        self.session.take_screen_changed()
    }
    pub fn is_not_alive(&self) -> bool {
        self.session.is_not_alive()
    }

    pub fn size(&self) -> (u16, u16) {
        self.session.size()
    }

    pub fn set_scroll_offset(&mut self, offset: usize) {
        self.session.set_scroll_offset(offset);
    }

    pub fn get_scroll_offset(&self) -> usize {
        self.session.scroll_offset()
    }

    // Scroll up by lines (increase scrollback offset to show older content)
    pub fn scroll_up(&mut self, lines: usize) {
        let current = self.get_scroll_offset();
        let new_offset = current.saturating_add(lines);
        self.set_scroll_offset(new_offset);
    }

    // Scroll down by lines (decrease scrollback offset to show newer content)
    pub fn scroll_down(&mut self, lines: usize) {
        let current = self.get_scroll_offset();
        let new_offset = current.saturating_sub(lines);
        self.set_scroll_offset(new_offset);
    }
    // Scroll to top of scrollback (maximum offset)
    pub fn scroll_to_top(&mut self) {
        self.set_scroll_offset(usize::MAX);
    }

    // Scroll to bottom (offset = 0, most recent output)
    pub fn scroll_to_bottom(&mut self) {
        self.set_scroll_offset(0);
    }

    // Get number of visible lines (this pane's current virtual terminal height)
    pub fn visible_lines(&self) -> usize {
        self.session.size().0 as usize
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        self.session.resize(rows, cols);
    }

    pub fn render_pane(&self, frame: &mut Frame, area: Rect, is_active: bool) {
        let screen = self.session.screen();
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

impl Drop for PtySession {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn key_to_bytes(key: &KeyEvent) -> Vec<u8> {
    if let Some(seq) = csi_key_to_bytes(key) {
        return seq;
    }

    let mut bytes = match key.code {
        KeyCode::Enter => b"\r".to_vec(),
        KeyCode::Tab => b"\t".to_vec(),
        KeyCode::Backspace => b"\x7f".to_vec(),
        KeyCode::Esc => b"\x1b".to_vec(),
        KeyCode::Char(c) if key.modifiers.contains(KeyModifiers::CONTROL) => {
            match control_char_byte(c) {
                Some(b) => vec![b],
                None => c.to_string().into_bytes(),
            }
        }
        KeyCode::Char(c) => c.to_string().into_bytes(),
        _ => Vec::new(),
    };

    if key.modifiers.contains(KeyModifiers::ALT) && !bytes.is_empty() {
        bytes.insert(0, 0x1b);
    }
    bytes
}

fn csi_key_to_bytes(key: &KeyEvent) -> Option<Vec<u8>> {
    let modifiers =
        key.modifiers & (KeyModifiers::SHIFT | KeyModifiers::ALT | KeyModifiers::CONTROL);
    let m = modifier_param(modifiers);
    let modified = !modifiers.is_empty();

    match key.code {
        KeyCode::Up => Some(ansi_csi("A", modified, m)),
        KeyCode::Down => Some(ansi_csi("B", modified, m)),
        KeyCode::Right => Some(ansi_csi("C", modified, m)),
        KeyCode::Left => Some(ansi_csi("D", modified, m)),
        KeyCode::Home => Some(ansi_csi("H", modified, m)),
        KeyCode::End => Some(ansi_csi("F", modified, m)),
        KeyCode::Insert => Some(ansi_tilde(2, modified, m)),
        KeyCode::Delete => Some(ansi_tilde(3, modified, m)),
        KeyCode::PageUp => Some(ansi_tilde(5, modified, m)),
        KeyCode::PageDown => Some(ansi_tilde(6, modified, m)),
        KeyCode::F(n) if (1..=12).contains(&n) => Some(f_key_to_bytes(n, modified, m)),
        KeyCode::Tab if modifiers.contains(KeyModifiers::SHIFT) => Some(b"\x1b[Z".to_vec()),
        _ => None,
    }
}

fn ansi_csi(final_byte: &str, modified: bool, m: u8) -> Vec<u8> {
    if modified {
        format!("\x1b[1;{}{}", m, final_byte).into_bytes()
    } else {
        format!("\x1b[{}", final_byte).into_bytes()
    }
}

fn ansi_tilde(n: u8, modified: bool, m: u8) -> Vec<u8> {
    if modified {
        format!("\x1b[{};{}~", n, m).into_bytes()
    } else {
        format!("\x1b[{}~", n).into_bytes()
    }
}

fn f_key_to_bytes(n: u8, modified: bool, m: u8) -> Vec<u8> {
    match n {
        1..=4 if modified => format!("\x1b[1;{}{}", m, f_final(n)).into_bytes(),
        1..=4 => format!("\x1bO{}", f_final(n)).into_bytes(),
        _ => {
            let code = match n {
                5 => 15,
                6 => 17,
                7 => 18,
                8 => 19,
                9 => 20,
                10 => 21,
                11 => 23,
                12 => 24,
                _ => unreachable!(),
            };
            ansi_tilde(code, modified, m)
        }
    }
}

fn f_final(n: u8) -> char {
    match n {
        1 => 'P',
        2 => 'Q',
        3 => 'R',
        4 => 'S',
        _ => unreachable!(),
    }
}

fn modifier_param(modifiers: KeyModifiers) -> u8 {
    let shift = modifiers.contains(KeyModifiers::SHIFT) as u8;
    let alt = modifiers.contains(KeyModifiers::ALT) as u8;
    let ctrl = modifiers.contains(KeyModifiers::CONTROL) as u8;
    1 + shift + 2 * alt + 4 * ctrl
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
        let (parser, bytes, _title) = test_parser_with_title();
        (parser, bytes)
    }

    fn test_parser_with_title() -> (
        vt100::Parser<MuxCallbacks>,
        Arc<Mutex<Vec<u8>>>,
        SharedTitle,
    ) {
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let writer: Box<dyn Write + Send> = Box::new(TestWriter(Arc::clone(&bytes)));
        let writer = Arc::new(Mutex::new(Some(writer)));
        let title = SharedTitle::default();

        let parser = vt100::Parser::new_with_callbacks(
            24,
            80,
            SCROLLBACK_SIZE,
            MuxCallbacks {
                writer: Arc::clone(&writer),
                title: title.clone(),
            },
        );
        (parser, bytes, title)
    }

    #[allow(clippy::type_complexity)]
    fn session_wiring() -> (
        Arc<Mutex<vt100::Parser<MuxCallbacks>>>,
        Arc<AtomicBool>,
        Arc<AtomicBool>,
        SharedTitle,
    ) {
        let title = SharedTitle::default();
        let writer: Box<dyn Write + Send> = Box::new(TestWriter(Arc::new(Mutex::new(Vec::new()))));
        let vpty = Arc::new(Mutex::new(vt100::Parser::new_with_callbacks(
            24,
            80,
            SCROLLBACK_SIZE,
            MuxCallbacks {
                writer: Arc::new(Mutex::new(Some(writer))),
                title: title.clone(),
            },
        )));
        let screen_changed = Arc::new(AtomicBool::new(false));
        let exited = Arc::new(AtomicBool::new(false));
        (vpty, screen_changed, exited, title)
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

    #[test]
    fn osc0_title_surfaces_as_single_change() {
        let (mut parser, _bytes, title) = test_parser_with_title();
        // OSC 0 fires both the icon-name and title callbacks with the same
        // string; the channel must still report exactly one change.
        parser.process(b"\x1b]0;my title\x07");
        assert_eq!(title.take_if_changed().as_deref(), Some("my title"));
        assert_eq!(title.take_if_changed(), None);
    }

    #[test]
    fn osc2_title_updates() {
        let (mut parser, _bytes, title) = test_parser_with_title();
        parser.process(b"\x1b]2;other title\x07");
        assert_eq!(title.take_if_changed().as_deref(), Some("other title"));
    }

    #[test]
    fn osc1_icon_name_counts_as_title() {
        let (mut parser, _bytes, title) = test_parser_with_title();
        parser.process(b"\x1b]1;icon name\x07");
        assert_eq!(title.take_if_changed().as_deref(), Some("icon name"));
    }

    #[test]
    fn non_utf8_title_is_ignored() {
        let (mut parser, _bytes, title) = test_parser_with_title();
        parser.process(b"\x1b]2;\xff\xfe\x07");
        assert_eq!(title.take_if_changed(), None);
    }

    #[test]
    fn latest_title_wins_before_sync() {
        let (mut parser, _bytes, title) = test_parser_with_title();
        parser.process(b"\x1b]2;first\x07");
        parser.process(b"\x1b]2;second\x07");
        assert_eq!(title.take_if_changed().as_deref(), Some("second"));
    }

    #[test]
    fn read_loop_publishes_title_and_flags_screen() {
        let (vpty, screen_changed, exited, title) = session_wiring();
        read_loop(
            &vpty,
            &screen_changed,
            &exited,
            std::io::Cursor::new(b"\x1b]2;hi\x07".to_vec()),
        );
        assert_eq!(title.take_if_changed().as_deref(), Some("hi"));
        assert!(screen_changed.swap(false, Ordering::Relaxed));
    }

    #[test]
    fn read_loop_output_lands_in_screen_and_sets_flag() {
        let (vpty, screen_changed, exited, _title) = session_wiring();
        read_loop(
            &vpty,
            &screen_changed,
            &exited,
            std::io::Cursor::new(b"hello".to_vec()),
        );
        assert_eq!(
            vpty.lock().unwrap().screen().rows(0, 80).next(),
            Some("hello".to_string())
        );
        assert!(screen_changed.swap(false, Ordering::Relaxed));
    }

    #[test]
    fn read_loop_eof_leaves_flag_clear() {
        let (vpty, screen_changed, exited, _title) = session_wiring();
        read_loop(
            &vpty,
            &screen_changed,
            &exited,
            std::io::Cursor::new(Vec::new()),
        );
        assert!(!screen_changed.swap(false, Ordering::Relaxed));
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
        // Alt on CSI keys uses the modifier-parameter form, not ESC-prefix.
        assert_eq!(
            pressed(KeyCode::Up, KeyModifiers::ALT),
            b"\x1b[1;3A".to_vec()
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

    #[test]
    fn csi_keys_encode_with_modifier_parameter() {
        let cases: &[(KeyCode, KeyModifiers, &[u8])] = &[
            (KeyCode::Home, KeyModifiers::NONE, b"\x1b[H"),
            (KeyCode::Home, KeyModifiers::SHIFT, b"\x1b[1;2H"),
            (KeyCode::Home, KeyModifiers::CONTROL, b"\x1b[1;5H"),
            (KeyCode::Home, KeyModifiers::ALT, b"\x1b[1;3H"),
            (
                KeyCode::Home,
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
                b"\x1b[1;6H",
            ),
            (KeyCode::End, KeyModifiers::NONE, b"\x1b[F"),
            (KeyCode::End, KeyModifiers::SHIFT, b"\x1b[1;2F"),
            (KeyCode::End, KeyModifiers::CONTROL, b"\x1b[1;5F"),
            (KeyCode::End, KeyModifiers::ALT, b"\x1b[1;3F"),
            (
                KeyCode::End,
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
                b"\x1b[1;6F",
            ),
            (KeyCode::Insert, KeyModifiers::NONE, b"\x1b[2~"),
            (KeyCode::Insert, KeyModifiers::SHIFT, b"\x1b[2;2~"),
            (KeyCode::Insert, KeyModifiers::CONTROL, b"\x1b[2;5~"),
            (KeyCode::Insert, KeyModifiers::ALT, b"\x1b[2;3~"),
            (
                KeyCode::Insert,
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
                b"\x1b[2;6~",
            ),
            (KeyCode::PageUp, KeyModifiers::NONE, b"\x1b[5~"),
            (KeyCode::PageUp, KeyModifiers::SHIFT, b"\x1b[5;2~"),
            (KeyCode::PageUp, KeyModifiers::CONTROL, b"\x1b[5;5~"),
            (KeyCode::PageUp, KeyModifiers::ALT, b"\x1b[5;3~"),
            (
                KeyCode::PageUp,
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
                b"\x1b[5;6~",
            ),
            (KeyCode::PageDown, KeyModifiers::NONE, b"\x1b[6~"),
            (KeyCode::PageDown, KeyModifiers::SHIFT, b"\x1b[6;2~"),
            (KeyCode::PageDown, KeyModifiers::CONTROL, b"\x1b[6;5~"),
            (KeyCode::PageDown, KeyModifiers::ALT, b"\x1b[6;3~"),
            (
                KeyCode::PageDown,
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
                b"\x1b[6;6~",
            ),
            (KeyCode::F(1), KeyModifiers::NONE, b"\x1bOP"),
            (KeyCode::F(1), KeyModifiers::SHIFT, b"\x1b[1;2P"),
            (KeyCode::F(1), KeyModifiers::CONTROL, b"\x1b[1;5P"),
            (KeyCode::F(1), KeyModifiers::ALT, b"\x1b[1;3P"),
            (
                KeyCode::F(1),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
                b"\x1b[1;6P",
            ),
            (KeyCode::F(4), KeyModifiers::NONE, b"\x1bOS"),
            (KeyCode::F(4), KeyModifiers::SHIFT, b"\x1b[1;2S"),
            (KeyCode::F(4), KeyModifiers::CONTROL, b"\x1b[1;5S"),
            (KeyCode::F(4), KeyModifiers::ALT, b"\x1b[1;3S"),
            (
                KeyCode::F(4),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
                b"\x1b[1;6S",
            ),
            (KeyCode::F(5), KeyModifiers::NONE, b"\x1b[15~"),
            (KeyCode::F(5), KeyModifiers::SHIFT, b"\x1b[15;2~"),
            (KeyCode::F(5), KeyModifiers::CONTROL, b"\x1b[15;5~"),
            (KeyCode::F(5), KeyModifiers::ALT, b"\x1b[15;3~"),
            (
                KeyCode::F(5),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
                b"\x1b[15;6~",
            ),
            (KeyCode::F(12), KeyModifiers::NONE, b"\x1b[24~"),
            (KeyCode::F(12), KeyModifiers::SHIFT, b"\x1b[24;2~"),
            (KeyCode::F(12), KeyModifiers::CONTROL, b"\x1b[24;5~"),
            (KeyCode::F(12), KeyModifiers::ALT, b"\x1b[24;3~"),
            (
                KeyCode::F(12),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
                b"\x1b[24;6~",
            ),
        ];

        for (code, modifiers, expected) in cases {
            assert_eq!(
                pressed(*code, *modifiers),
                expected.to_vec(),
                "{code:?} with {modifiers:?}"
            );
        }

        assert_eq!(
            pressed(KeyCode::Tab, KeyModifiers::SHIFT),
            b"\x1b[Z".to_vec()
        );
    }
}
