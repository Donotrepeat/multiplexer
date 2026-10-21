use crate::app::config::TerminalConfig;
use crate::app::events::{PaneEvent, PaneId};
use crate::app::pane::session::callbacks::MuxCallbacks;
use crate::app::pane::session::paste::paste_bytes;
use crate::app::pane::session::threads::{
    PtyWriter, join_with_timeout, kill_unless_reaped, read_loop,
};
use anyhow::Result;
use parking_lot::Mutex;
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::thread::JoinHandle;

mod callbacks;
mod paste;
mod probe;
mod threads;

pub(super) const SCROLLBACK_SIZE: usize = 1200;

pub(super) struct PtySession {
    vpty: Arc<Mutex<vt100::Parser<MuxCallbacks>>>,
    writer: Arc<PtyWriter>,
    master: Box<dyn MasterPty>,
    /// Signals the child to stop on teardown. The child itself lives in the
    /// reaper thread, which owns `wait()`.
    killer: Box<dyn ChildKiller + Send + Sync>,
    /// Set by the reaper once `wait()` has returned: the child is gone and
    /// its PID is free for reuse, so teardown must not signal it anymore.
    reaped: Arc<AtomicBool>,
    reader_thread: Option<JoinHandle<()>>,
    reaper_thread: Option<JoinHandle<()>>,
    screen_changed: Arc<AtomicBool>,
    rows: u16,
    cols: u16,
    pub shell: PathBuf, // resolved at spawn, before the &TerminalConfig borrow ends
    pub spawn_dir: PathBuf, // the cwd the child was actually started in
    pid: u32,           // child.pid() taken *before* moving it into the reaper
}

impl PtySession {
    pub(super) fn in_alternate_screen(&self) -> bool {
        self.vpty.lock().screen().alternate_screen()
    }

    pub(super) fn spawn(
        rows: u16,
        cols: u16,
        id: PaneId,
        tx: Sender<PaneEvent>,
        terminal: &TerminalConfig,
    ) -> Result<Self> {
        let pair = native_pty_system().openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let shell = terminal.shell();
        let mut cmd = CommandBuilder::new(&shell);
        cmd.cwd(terminal.working_dir());
        for (k, v) in terminal.env() {
            cmd.env(k, v);
        }
        let child = pair.slave.spawn_command(cmd)?;
        let pid = child.process_id().unwrap_or_default();
        drop(pair.slave);
        let killer = child.clone_killer();

        let writer = Arc::new(PtyWriter::new(pair.master.take_writer()?));
        let vpty = Arc::new(Mutex::new(vt100::Parser::new_with_callbacks(
            rows,
            cols,
            SCROLLBACK_SIZE,
            MuxCallbacks::new(id, tx.clone()),
        )));
        // Starts clear: the app draws its first frame without waiting for
        // output, and `read_loop` wakes it only on the next transition.
        let screen_changed = Arc::new(AtomicBool::new(false));
        let vpt_clone = Arc::clone(&vpty);
        let sc_clone = Arc::clone(&screen_changed);
        let writer_clone = Arc::clone(&writer);

        let reader = pair.master.try_clone_reader()?;
        let reader_tx = tx.clone();
        let reader_thread = std::thread::spawn(move || {
            read_loop(&vpt_clone, &writer_clone, &sc_clone, &reader_tx, id, reader)
        });
        let reaped = Arc::new(AtomicBool::new(false));
        let reaped_clone = Arc::clone(&reaped);
        let reaper_thread = std::thread::spawn(move || {
            let mut child = child;
            let _ = child.wait();
            reaped_clone.store(true, Ordering::Release);
            let _ = tx.send(PaneEvent::Exited(id));
        });

        Ok(PtySession {
            vpty,
            writer,
            master: pair.master,
            killer,
            reaped,
            reader_thread: Some(reader_thread),
            reaper_thread: Some(reaper_thread),
            screen_changed,
            rows,
            cols,
            spawn_dir: terminal.working_dir(),
            shell,
            pid,
        })
    }

    pub(super) fn write_bytes(&self, bytes: &[u8]) -> Result<()> {
        self.writer.write_all(bytes)?;
        Ok(())
    }

    /// Whether the foreground application asked for bracketed paste
    /// (`ESC[?2004h`), which the parser tracks on the current screen.
    pub(super) fn bracketed_paste(&self) -> bool {
        self.vpty.lock().screen().bracketed_paste()
    }

    /// Paste `text` into the PTY, framed according to the application's
    /// current bracketed-paste mode. No-op for empty text and dead children.
    pub(super) fn paste(&self, text: &str) -> Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        let bracketed = self.bracketed_paste();
        self.write_bytes(&paste_bytes(text, bracketed))
    }

    pub(super) fn take_screen_changed(&self) -> bool {
        self.screen_changed.swap(false, Ordering::Relaxed)
    }

    pub(super) fn size(&self) -> (u16, u16) {
        (self.rows, self.cols)
    }

    /// Resize the backing PTY and the vt100 screen so both stay in sync.
    /// Skips the work (and avoids a spurious SIGWINCH on the shell) when the
    /// requested size is unchanged. Returns whether anything changed.
    pub(super) fn resize(&mut self, rows: u16, cols: u16) -> bool {
        let rows = rows.max(1);
        let cols = cols.max(1);
        if rows == self.rows && cols == self.cols {
            return false;
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
        self.vpty.lock().screen_mut().set_size(rows, cols);
        true
    }

    pub(super) fn scroll_offset(&self) -> usize {
        self.vpty.lock().screen().scrollback()
    }

    pub(super) fn set_scroll_offset(&self, offset: usize) {
        self.vpty.lock().screen_mut().set_scrollback(offset);
        log::debug!("offset {offset}");
    }

    pub(super) fn screen(&self) -> vt100::Screen {
        #[cfg(test)]
        SCREEN_CLONES.fetch_add(1, Ordering::Relaxed);
        self.vpty.lock().screen().clone()
    }
}

/// Test-only count of full screen clones, so a test can assert that idle
/// frames reuse the pane's cached snapshot.
#[cfg(test)]
pub(super) static SCREEN_CLONES: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

impl Drop for PtySession {
    fn drop(&mut self) {
        // The reaper owns the child and does the only `wait()`, off the UI
        // thread. Killing first — unless the child was already reaped, see
        // `kill_unless_reaped` — makes both background threads finish
        // promptly; the bounded join keeps a child that ignores SIGHUP (or a
        // grandchild holding the pty slave open) from hanging the app.
        kill_unless_reaped(self.killer.as_mut(), self.reaped.load(Ordering::Acquire));
        join_with_timeout(self.reader_thread.take(), "pty reader");
        join_with_timeout(self.reaper_thread.take(), "pty reaper");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::config::TerminalConfig;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    #[test]
    fn live_cwd_falls_back_to_spawn_dir_when_the_child_is_gone() -> Result<()> {
        let (tx, rx) = mpsc::channel();
        let spawn_dir = PathBuf::from("/tmp");
        let terminal = TerminalConfig {
            working_dir: Some(spawn_dir.clone()),
            ..TerminalConfig::default()
        };
        let session = PtySession::spawn(4, 20, PaneId(1), tx, &terminal)?;
        session.write_bytes(b"exit\r")?;

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(remaining) {
                Ok(PaneEvent::Exited(_)) => break,
                Ok(_) => continue,
                Err(err) => panic!("no Exited event from the reaper: {err}"),
            }
        }

        // The child is reaped, its pid no longer names it: the probe must
        // not return some other process's cwd.
        assert_eq!(session.live_cwd(), spawn_dir);
        Ok(())
    }
}
