use anyhow::Result;
use crossterm::event::KeyEvent;
use ratatui::Frame;
use ratatui::layout::{Margin, Rect};
use ratatui::prelude::Position;
use ratatui::style::{Color, Stylize};
use ratatui::widgets::{Block, Paragraph};

mod keys;
mod render;
mod session;

use keys::key_to_bytes;
use render::vterm_to_ratatui;
use session::PtySession;

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

    /// Paste clipboard text into the pane's PTY. Framing follows the
    /// foreground application's bracketed-paste mode; see
    /// [`PtySession::paste`](session::PtySession::paste).
    pub fn paste(&mut self, text: &str) -> Result<()> {
        self.session.paste(text)
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
