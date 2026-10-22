use anyhow::Result;
use crossterm::event::KeyEvent;
use ratatui::Frame;
use std::time::Duration;

pub trait Host {
    /// Input from the outside world: a key, a resize, or a disconnect.
    fn poll_event(&mut self, timeout: Duration) -> Result<Option<HostEvent>>;

    /// Draw one frame. The closure receives a ratatui `Frame`.
    fn draw<F: FnOnce(&mut Frame)>(&mut self, f: F) -> Result<()>;

    /// Size the layout should target.
    fn size(&self) -> (u16, u16);
}

pub enum HostEvent {
    Key(KeyEvent),
    Resize(u16, u16),
    Detach, // graceful disconnect; server keeps running
}
