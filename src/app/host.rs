use crate::app::util::initialize_pane_size;
use anyhow::Result;
use crossterm::event::Event;
use crossterm::event::KeyEvent;
use crossterm::terminal::size;
use ratatui::Frame;
use std::time::Duration;

pub trait Host {
    /// Input from the outside world: a key, a resize, or a disconnect.
    fn poll_event(&mut self, timeout: Duration) -> Result<Option<HostEvent>>;

    /// Draw one frame. The closure receives a ratatui `Frame`.
    fn draw(&mut self, f: &mut dyn FnMut(&mut Frame)) -> Result<()>;
    /// Size the layout should target.
    fn size(&self) -> (u16, u16);
}

pub enum HostEvent {
    Key(KeyEvent),
    Resize(u16, u16),
    Detach, // graceful disconnect; server keeps running
}
pub struct LocalHost {
    terminal: DefaultTerminal,
}

impl LocalHost {
    pub fn new() -> Result<Self> {
        // raw mode + alternate screen move here from main.rs:55–56
        let terminal = ratatui::init();
        Ok(Self { terminal })
    }

    pub fn size(&self) -> (u16, u16) {
        let s = self.terminal.size().unwrap_or_default();
        (s.width, s.height)
    }
}

impl Host for LocalHost {
    fn poll_event(&mut self, timeout: Duration) -> Result<Option<HostEvent>> {
        if !crossterm::event::poll(timeout)? {
            return Ok(None);
        }
        Ok(match crossterm::event::read()? {
            Event::Key(key) => Some(HostEvent::Key(key)),
            Event::Resize(w, h) => Some(HostEvent::Resize(w, h)),
            _ => None, // matches today's `_ => {}` in pump
        })
    }

    fn draw(&mut self, f: &mut dyn FnMut(&mut Frame)) -> Result<()> {
        self.terminal.draw(f)?;
        Ok(())
    }

    fn size(&self) -> (u16, u16) {
        let (term_cols, term_rows) = size().unwrap();
        let term_rows = term_rows.max(1);
        let term_cols = term_cols.max(1);
        initialize_pane_size(term_rows, term_cols)
    }
}

impl Drop for LocalHost {
    fn drop(&mut self) {
        ratatui::restore(); // raw mode off + leave alternate screen
    }
}
