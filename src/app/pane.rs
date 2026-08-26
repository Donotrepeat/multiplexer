use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use ratatui::prelude::Position;
use ratatui::{
    layout::{Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Paragraph},
    Frame,
};
pub struct MuxCallbacks {
    writer: Arc<Mutex<Option<Box<dyn Write + Send>>>>,
}

impl vt100::Callbacks for MuxCallbacks {
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
        if let Some(bytes) = reply {
            if let Some(w) = self.writer.lock().unwrap().as_mut() {
                let _ = w.write_all(&bytes);
            }
        }
    }
}

pub struct Pane {
    pub vpty: Arc<Mutex<vt100::Parser<MuxCallbacks>>>,
    pub pty_writer: Arc<Mutex<Option<Box<dyn Write + Send>>>>,
    pub pty_master: Box<dyn MasterPty>,
    pub screen_changed: Arc<AtomicBool>,
    // Scroll position tracking
    pub scroll_offset: usize,
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
        let vpty = Arc::new(Mutex::new(vt100::Parser::new_with_callbacks(
            row,
            coll,
            1200,
            MuxCallbacks {
                writer: Arc::clone(&pty_writer),
            },
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
            vpty: vpty,
            pty_writer,
            pty_master: pair.master,
            screen_changed,
            scroll_offset: 0,
        })
    }

    // Set scroll position using vt100 parser
    pub fn set_scroll_offset(&mut self, offset: usize) {
        let mut parser = self.vpty.lock().unwrap();
        parser.screen_mut().set_scrollback(offset);
        self.scroll_offset = offset;
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
    pub fn scroll_to_input(&mut self, _num_panes: usize, _active_id: usize) {
        let parser = self.vpty.lock().unwrap();
        let screen = parser.screen();
        let row = screen.cursor_position().0 as usize;
        let visible = self.visible_lines();

        self.scroll_offset = row.saturating_sub(visible);
    }
    // Scroll to top (offset = 0)
    pub fn scroll_to_top(&mut self) {
        self.set_scroll_offset(0);
    }

    // Scroll to bottom (offset = max scrollback)
    pub fn scroll_to_bottom(&mut self) {
        self.set_scroll_offset(1200);
    }

    // Get number of visible lines (terminal height)
    pub fn visible_lines(&self) -> usize {
        let parser = self.vpty.lock().unwrap();
        let size = parser.screen().size();
        size.0 as usize - 4
    }

    // Check if at top (offset = 0)
    pub fn at_top(&self) -> bool {
        self.get_scroll_offset() == 0
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        self.pty_master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
    }

    // Check if at bottom
    pub fn at_bottom(&self) -> bool {
        self.get_scroll_offset() >= 1200
    }
    pub fn render_pane(&self, frame: &mut Frame, area: Rect, is_active: bool) {
        let parser = self.vpty.lock().unwrap();
        let screen = parser.screen();
        let (visible_rows, _cols) = screen.size();
        let text = vterm_to_ratatui(screen, self.scroll_offset, visible_rows as usize);
        frame.render_widget(Paragraph::new(text).block(Block::bordered()), area);

        if is_active {
            let (row, col) = screen.cursor_position();
            let inner = area.inner(Margin {
                horizontal: 1,
                vertical: 1,
            });
            if row as usize >= self.scroll_offset {
                let x = (inner.x + col).min(inner.right() - 1);
                let y = (inner.y + row - self.scroll_offset as u16).min(inner.bottom() - 1);
                frame.set_cursor_position(Position::new(x, y));
            }
        }
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

fn vterm_to_ratatui(
    screen: &vt100::Screen,
    scroll_offset: usize,
    visible_rows: usize,
) -> Text<'static> {
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
        let parser = vt100::Parser::new_with_callbacks(
            24,
            80,
            1200,
            MuxCallbacks {
                writer: Arc::clone(&writer),
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
}
