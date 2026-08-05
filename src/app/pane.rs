use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::usize;

use anyhow::Result;
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};

pub struct Pane {
    pub vpty: Arc<Mutex<vt100::Parser>>,
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
        let vpty = Arc::new(Mutex::new(vt100::Parser::new(row, coll, 1200)));
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
        self.set_scroll_offset(new_offset);
    }

    // Scroll down by lines (decrease scrollback offset to show newer content)
    pub fn scroll_down(&mut self, lines: usize) {
        let current = self.get_scroll_offset();
        let new_offset = current.saturating_sub(lines);
        self.set_scroll_offset(new_offset);
    }
    pub fn scroll_to_input(&mut self, num_panes: usize) {
        let parser = self.vpty.lock().unwrap();
        let screen = parser.screen();
        let row = screen.cursor_position().0 as usize;
        let a = screen.size().0 as usize - 4;
        self.scroll_offset = row.saturating_sub(a);
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
        size.0 as usize
    }

    // Check if at top (offset = 0)
    pub fn at_top(&self) -> bool {
        self.get_scroll_offset() == 0
    }

    // Check if at bottom
    pub fn at_bottom(&self) -> bool {
        self.get_scroll_offset() >= 1200
    }
}
