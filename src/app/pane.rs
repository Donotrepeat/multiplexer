use std::sync::mpsc::Sender;

use anyhow::Result;
use crossterm::event::KeyEvent;
use ratatui::Frame;
use ratatui::layout::{Margin, Rect};
use ratatui::prelude::Position;
use ratatui::style::{Color, Stylize};
use ratatui::widgets::{Block, Paragraph};

use crate::app::events::{PaneEvent, PaneId};

mod keys;
mod render;
mod session;

use keys::key_to_bytes;
use render::vterm_to_ratatui;
use session::PtySession;

/// Prefix `[exited]` unless the title already carries it.
fn exited_label(title: &str) -> String {
    if title.starts_with("[exited]") {
        title.to_string()
    } else {
        format!("[exited] {title}")
    }
}

pub struct Pane {
    id: PaneId,
    session: PtySession,
    pub title: String,
    exited: bool,
}

impl Pane {
    pub fn new(rows: u16, cols: u16, id: PaneId, tx: Sender<PaneEvent>) -> Result<Self> {
        Ok(Pane {
            id,
            session: PtySession::spawn(rows, cols, id, tx)?,
            title: "~".to_string(),
            exited: false,
        })
    }

    pub fn id(&self) -> PaneId {
        self.id
    }

    /// Forward a key event to the pane's PTY as the bytes a real terminal
    /// would send for it. No-op once the child has exited; a write failure
    /// (the child died between the exit event and this write) is logged
    /// rather than brought down as an app error.
    pub fn write_key(&mut self, key: KeyEvent) -> Result<()> {
        if self.exited {
            return Ok(());
        }
        if let Err(err) = self.session.write_bytes(&key_to_bytes(&key)) {
            log::warn!("pane {} write failed: {err}", self.id.0);
        }
        Ok(())
    }

    /// Paste clipboard text into the pane's PTY. Framing follows the
    /// foreground application's bracketed-paste mode; see
    /// [`PtySession::paste`](session::PtySession::paste).
    pub fn paste(&mut self, text: &str) -> Result<()> {
        if self.exited {
            return Ok(());
        }
        if let Err(err) = self.session.paste(text) {
            log::warn!("pane {} paste failed: {err}", self.id.0);
        }
        Ok(())
    }

    /// Apply a title reported by the foreground program. A title that arrives
    /// after the child exited keeps the `[exited]` marker.
    pub fn set_title(&mut self, title: String) {
        self.title = if self.exited {
            exited_label(&title)
        } else {
            title
        };
    }

    /// Mark the child as exited and tag the title, whichever order the
    /// `Title` and `Exited` events happen to arrive in.
    pub fn mark_exited(&mut self) {
        self.exited = true;
        self.title = exited_label(&self.title);
    }

    /// Acknowledge an `Output` event: clears the session's coalescing flag so
    /// the next chunk of output wakes the UI loop again.
    pub fn ack_output(&mut self) {
        self.session.take_screen_changed();
    }

    pub fn in_alternated_state(&self) -> bool {
        self.session.in_alternate_screen()
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

#[cfg(test)]
mod tests {
    use super::exited_label;

    #[test]
    fn exited_label_is_added_exactly_once() {
        assert_eq!(exited_label("vim"), "[exited] vim");
        assert_eq!(exited_label("[exited] vim"), "[exited] vim");
    }
}
