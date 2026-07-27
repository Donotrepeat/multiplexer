use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use std::thread;

pub struct Pane {
    pub vpty: Arc<Mutex<vt100::Parser>>,
    pub pty_writer: Arc<Mutex<Option<Box<dyn Write + Send>>>>,
    pub pty_master: Box<dyn MasterPty>,
    pub screen_changed: Arc<AtomicBool>,
}

impl Pane {
    pub fn new(row: u16, coll: u16) -> Self {
        let pty_system = native_pty_system();

        // Create a new pty
        let pair = pty_system.openpty(PtySize {
            rows: row,
            cols: coll,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        // Spawn a shell into the pty
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into());
        let cmd = CommandBuilder::new(shell);
        let mut child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);
        let pty_writer = Arc::new(Mutex::new(Some(pair.master.take_writer()?)));
        let vpty = Arc::new(Mutex::new(vt100::Parser::new(row, coll, 12)));
        let vpt_clone = Arc::clone(&vpty);

        let screen_changed = Arc::new(AtomicBool::new(true));
        let sc_clone = Arc::clone(&screen_changed);

        let mut reader = pair.master.try_clone_reader()?;
        let reader_thread = thread::spawn(move || {
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
        Pane {
            vpty: vpty,
            pty_writer,
            pty_master: pair.master,
            screen_changed,
        }
    }
}
