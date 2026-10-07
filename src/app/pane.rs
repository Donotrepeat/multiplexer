use std::sync::mpsc::Sender;

use crate::app::config::TerminalConfig;
use anyhow::Result;
use crossterm::event::KeyEvent;
use ratatui::layout::{Margin, Rect};
use ratatui::prelude::Position;
use ratatui::style::{Color, Stylize};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

use crate::app::events::{PaneEvent, PaneId};

mod keys;
mod render;
mod session;

use keys::key_to_bytes;
use render::vterm_to_ratatui;
use session::PtySession;

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
    dirty: bool,
    snapshot: Option<vt100::Screen>,
}

impl Pane {
    pub fn new(
        rows: u16,
        cols: u16,
        id: PaneId,
        tx: Sender<PaneEvent>,
        terminal: &TerminalConfig,
    ) -> Result<Self> {
        Ok(Pane {
            id,
            session: PtySession::spawn(rows, cols, id, tx, terminal)?,
            title: "~".to_string(),
            exited: false,
            dirty: true,
            snapshot: None,
        })
    }

    pub fn id(&self) -> PaneId {
        self.id
    }

    pub fn write_key(&mut self, key: KeyEvent) -> Result<()> {
        if self.exited {
            return Ok(());
        }
        if let Err(err) = self.session.write_bytes(&key_to_bytes(&key)) {
            log::warn!("pane {} write failed: {err}", self.id.0);
        }
        Ok(())
    }

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

    pub fn set_title(&mut self, title: String) {
        self.title = if self.exited {
            exited_label(&title)
        } else {
            title
        };
    }

    pub fn mark_exited(&mut self) {
        self.exited = true;
        self.title = exited_label(&self.title);
    }

    pub fn ack_output(&mut self) {
        self.session.take_screen_changed();
        self.dirty = true;
    }

    pub fn in_alternated_state(&self) -> bool {
        self.session.in_alternate_screen()
    }

    pub fn size(&self) -> (u16, u16) {
        self.session.size()
    }

    pub fn set_scroll_offset(&mut self, offset: usize) {
        self.session.set_scroll_offset(offset);
        self.dirty = true;
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
        if self.session.resize(rows, cols) {
            // The snapshot holds a screen of the old size.
            self.dirty = true;
        }
    }

    pub fn render_pane(&mut self, frame: &mut Frame, area: Rect, is_active: bool) {
        if self.dirty || self.snapshot.is_none() {
            self.session.take_screen_changed();
            self.snapshot = Some(self.session.screen());
            self.dirty = false;
        }
        let screen = self.snapshot.as_ref().expect("snapshot refreshed above");
        let (screen_rows, _cols) = screen.size();
        let inner = area.inner(Margin {
            horizontal: 1,
            vertical: 1,
        });
        let rows = (screen_rows as usize).min(inner.height as usize);
        let text = vterm_to_ratatui(screen, rows);
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
    use super::session::SCREEN_CLONES;
    use super::*;
    use crossterm::event::{KeyCode, KeyModifiers};
    use std::sync::atomic::Ordering;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    #[test]
    fn exited_label_is_added_exactly_once() {
        assert_eq!(exited_label("vim"), "[exited] vim");
        assert_eq!(exited_label("[exited] vim"), "[exited] vim");
    }

    #[test]
    fn pane_exit_is_reported_by_the_reaper() -> Result<()> {
        let (tx, rx) = mpsc::channel();
        let mut pane = Pane::new(4, 20, PaneId(7), tx, &TerminalConfig::default())?;

        for key in "exit\r".chars() {
            let code = if key == '\r' {
                KeyCode::Enter
            } else {
                KeyCode::Char(key)
            };
            pane.write_key(KeyEvent::new(code, KeyModifiers::NONE))?;
        }

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(remaining) {
                Ok(PaneEvent::Exited(id)) => {
                    assert_eq!(id, PaneId(7));
                    break;
                }
                Ok(_) => continue,
                Err(err) => panic!("no Exited event from the reaper: {err}"),
            }
        }
        Ok(())
    }

    #[test]
    fn idle_render_reuses_the_cached_screen() -> Result<()> {
        let (tx, _rx) = mpsc::channel();
        let mut pane = Pane::new(4, 20, PaneId(0), tx, &TerminalConfig::default())?;
        SCREEN_CLONES.store(0, Ordering::Relaxed);

        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(20, 10))?;
        let area = Rect::new(0, 0, 20, 10);

        terminal.draw(|frame| pane.render_pane(frame, area, true))?;
        assert_eq!(SCREEN_CLONES.load(Ordering::Relaxed), 1);

        // Nothing changed: the cached snapshot is reused, no clone.
        terminal.draw(|frame| pane.render_pane(frame, area, true))?;
        assert_eq!(SCREEN_CLONES.load(Ordering::Relaxed), 1);

        // Output schedules a refresh, so the next render clones again.
        pane.ack_output();
        terminal.draw(|frame| pane.render_pane(frame, area, true))?;
        assert_eq!(SCREEN_CLONES.load(Ordering::Relaxed), 2);
        Ok(())
    }
}
