use anyhow::Result;
use crossterm::event::{Event, KeyEvent};
use ratatui::{DefaultTerminal, Frame};
use std::time::Duration;

/// The I/O seam `App` runs through: local terminal today, network clients
/// later. `App` owns all session logic; a `Host` only supplies input events,
/// draws frames, and reports the screen size.
pub trait Host {
    /// Input from the outside world: a key, a resize, or a disconnect.
    /// Returns `Ok(None)` when nothing arrives within `timeout`.
    fn poll_event(&mut self, timeout: Duration) -> Result<Option<HostEvent>>;

    /// Draw one frame. The closure receives a ratatui `Frame`. Takes a
    /// `dyn` closure so the trait stays object-safe.
    fn draw(&mut self, f: &mut dyn FnMut(&mut Frame)) -> Result<()>;

    /// Size the layout should target, as `(cols, rows)`.
    fn size(&self) -> (u16, u16);
}

pub enum HostEvent {
    Key(KeyEvent),
    Resize(u16, u16),
    // Graceful disconnect; only the future network host produces this.
    // A local session never detaches.
    #[allow(dead_code)]
    Detach,
}

/// Local terminal host: wraps crossterm input and the ratatui terminal.
/// Creating it enters raw mode and the alternate screen; dropping it
/// restores the terminal, including on early returns and unwinds.
pub struct LocalHost {
    terminal: DefaultTerminal,
}

impl LocalHost {
    pub fn new() -> Result<Self> {
        let terminal = ratatui::try_init()?;
        Ok(Self { terminal })
    }
}

impl Host for LocalHost {
    fn poll_event(&mut self, timeout: Duration) -> Result<Option<HostEvent>> {
        loop {
            if !crossterm::event::poll(timeout)? {
                return Ok(None);
            }
            match crossterm::event::read()? {
                Event::Key(key) => return Ok(Some(HostEvent::Key(key))),
                Event::Resize(cols, rows) => return Ok(Some(HostEvent::Resize(cols, rows))),
                // Ignore everything else (mouse, focus, ...), as before.
                _ => {}
            }
        }
    }

    fn draw(&mut self, f: &mut dyn FnMut(&mut Frame)) -> Result<()> {
        self.terminal.draw(f)?;
        Ok(())
    }

    fn size(&self) -> (u16, u16) {
        let size = self.terminal.size().unwrap_or_default();
        (size.width.max(1), size.height.max(1))
    }
}

impl Drop for LocalHost {
    fn drop(&mut self) {
        ratatui::restore(); // raw mode off + leave alternate screen
    }
}

pub enum ClientEvent {
    Key(KeyEvent),
    Resize(u16, u16),
    // Graceful disconnect; only the future network host produces this.
    // A local session never detaches.
    #[allow(dead_code)]
    Detach,
}
