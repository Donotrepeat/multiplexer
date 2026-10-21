use crate::app::events::{PaneEvent, PaneId};
use crate::app::pane::session::callbacks::MuxCallbacks;
use parking_lot::Mutex;
use portable_pty::ChildKiller;
use std::io::{ErrorKind, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// The write half of a pane's PTY.
///
/// A newtype so the hot paths never juggle an `Option` — nothing ever takes
/// the writer away — while tests can still inject a recording writer behind
/// `dyn Write`.
pub(super) struct PtyWriter(Mutex<Box<dyn Write + Send>>);

impl PtyWriter {
    pub(super) fn new(writer: Box<dyn Write + Send>) -> Self {
        Self(Mutex::new(writer))
    }

    /// Write `bytes` to the PTY. A failure means the child is gone; callers
    /// decide whether that is worth surfacing.
    pub(super) fn write_all(&self, bytes: &[u8]) -> std::io::Result<()> {
        self.0.lock().write_all(bytes)
    }
}

pub(super) fn read_loop(
    vpty: &Mutex<vt100::Parser<MuxCallbacks>>,
    writer: &PtyWriter,
    screen_changed: &AtomicBool,
    tx: &Sender<PaneEvent>,
    id: PaneId,
    mut reader: impl Read,
) {
    let mut buf = [0u8; 4096];
    loop {
        match reader.read(&mut buf) {
            // EOF just ends the reader thread; the reaper reports `Exited`
            // once the child has actually been waited on.
            Ok(0) => break,
            Ok(n) => {
                // Lock order: parser, then writer — and never the reverse.
                // `process` runs under the parser lock, so any replies it
                // queues are taken while that lock is still held, then
                // flushed once it is released.
                let reply = {
                    let mut parser = vpty.lock();
                    parser.process(&buf[..n]);
                    parser.callbacks_mut().take_reply()
                };
                if !reply.is_empty() {
                    let _ = writer.write_all(&reply);
                }
                if !screen_changed.swap(true, Ordering::Relaxed) {
                    // Coalesced: the UI loop clears the flag when it handles
                    // this event, so the next chunk of output wakes it again.
                    let _ = tx.send(PaneEvent::Output(id));
                }
            }
            Err(err) if err.kind() == ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
}

/// Join a background thread without letting a stuck child block teardown:
/// wait up to a short grace period, then log and detach it.
pub(super) fn join_with_timeout(handle: Option<JoinHandle<()>>, what: &str) {
    let Some(handle) = handle else {
        return;
    };
    let deadline = Instant::now() + Duration::from_millis(200);
    while !handle.is_finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(1));
    }
    if handle.is_finished() {
        let _ = handle.join();
    } else {
        log::warn!("{what} thread did not stop within the grace period; detaching");
    }
}

/// Signal the child to stop, unless the reaper has already waited on it.
/// After `wait()` returns the child is gone and its PID is free for reuse,
/// so a late `kill(pid)` could land on an unrelated process that inherited
/// the ID. Errors are ignored: the child dying on its own is not a failure.
pub(super) fn kill_unless_reaped(killer: &mut dyn ChildKiller, reaped: bool) {
    if reaped {
        return;
    }
    let _ = killer.kill();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::mpsc;

    struct TestWriter(Arc<Mutex<Vec<u8>>>);

    impl Write for TestWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Records `kill()` calls so teardown can be asserted without a real
    /// child. `ChildKiller` requires `Debug`; `Downcast` and `Send` are
    /// satisfied by any `'static` type via blanket impls.
    #[derive(Debug, Default)]
    struct KillRecorder {
        kills: usize,
    }

    impl ChildKiller for KillRecorder {
        fn kill(&mut self) -> std::io::Result<()> {
            self.kills += 1;
            Ok(())
        }

        fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
            Box::new(KillRecorder { kills: self.kills })
        }
    }

    #[allow(clippy::type_complexity)]
    fn session_wiring() -> (
        Arc<Mutex<vt100::Parser<MuxCallbacks>>>,
        Arc<PtyWriter>,
        Arc<Mutex<Vec<u8>>>,
        Arc<AtomicBool>,
        Sender<PaneEvent>,
        mpsc::Receiver<PaneEvent>,
    ) {
        let (tx, rx) = mpsc::channel();
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let writer = Arc::new(PtyWriter::new(Box::new(TestWriter(Arc::clone(&bytes)))));
        let vpty = Arc::new(Mutex::new(vt100::Parser::new_with_callbacks(
            24,
            80,
            crate::app::pane::session::SCROLLBACK_SIZE,
            MuxCallbacks::new(PaneId(1), tx.clone()),
        )));
        let screen_changed = Arc::new(AtomicBool::new(false));
        (vpty, writer, bytes, screen_changed, tx, rx)
    }

    #[test]
    fn read_loop_publishes_title_and_flags_screen() {
        let (vpty, writer, _bytes, screen_changed, tx, events) = session_wiring();
        read_loop(
            &vpty,
            &writer,
            &screen_changed,
            &tx,
            PaneId(1),
            std::io::Cursor::new(b"\x1b]2;hi\x07".to_vec()),
        );
        assert_eq!(
            events.try_recv().ok(),
            Some(PaneEvent::Title(PaneId(1), "hi".into()))
        );
        assert!(screen_changed.swap(false, Ordering::Relaxed));
    }

    #[test]
    fn read_loop_output_lands_in_screen_and_emits_output() {
        let (vpty, writer, _bytes, screen_changed, tx, events) = session_wiring();
        read_loop(
            &vpty,
            &writer,
            &screen_changed,
            &tx,
            PaneId(1),
            std::io::Cursor::new(b"hello".to_vec()),
        );
        assert_eq!(
            vpty.lock().screen().rows(0, 80).next(),
            Some("hello".to_string())
        );
        assert!(screen_changed.swap(false, Ordering::Relaxed));
        assert_eq!(events.try_recv().ok(), Some(PaneEvent::Output(PaneId(1))));
        assert_eq!(events.try_recv().ok(), None);
    }

    #[test]
    fn read_loop_eof_emits_nothing() {
        // The reaper owns exit reporting; EOF alone is not an exit event.
        let (vpty, writer, _bytes, screen_changed, tx, events) = session_wiring();
        read_loop(
            &vpty,
            &writer,
            &screen_changed,
            &tx,
            PaneId(1),
            std::io::Cursor::new(Vec::new()),
        );
        assert_eq!(events.try_recv().ok(), None);
        assert!(!screen_changed.swap(false, Ordering::Relaxed));
    }

    #[test]
    fn read_loop_coalesces_output_until_the_flag_is_cleared() {
        let (vpty, writer, _bytes, screen_changed, tx, events) = session_wiring();
        screen_changed.store(true, Ordering::Relaxed);
        read_loop(
            &vpty,
            &writer,
            &screen_changed,
            &tx,
            PaneId(1),
            std::io::Cursor::new(b"a".to_vec()),
        );
        assert_eq!(events.try_recv().ok(), None);

        screen_changed.store(false, Ordering::Relaxed);
        read_loop(
            &vpty,
            &writer,
            &screen_changed,
            &tx,
            PaneId(1),
            std::io::Cursor::new(b"b".to_vec()),
        );
        assert_eq!(events.try_recv().ok(), Some(PaneEvent::Output(PaneId(1))));
        assert_eq!(events.try_recv().ok(), None);
    }

    #[test]
    fn read_loop_flushes_buffered_csi_reply_after_processing() {
        let (vpty, writer, bytes, screen_changed, tx, _events) = session_wiring();
        read_loop(
            &vpty,
            &writer,
            &screen_changed,
            &tx,
            PaneId(1),
            std::io::Cursor::new(b"\x1b[5n".to_vec()),
        );
        assert_eq!(*bytes.lock(), b"\x1b[0n".to_vec());
        // The reply is buffered by the callbacks and only reaches the writer
        // through the post-`process` flush.
        assert!(vpty.lock().callbacks_mut().take_reply().is_empty());
    }

    #[test]
    fn teardown_does_not_signal_an_already_reaped_child() {
        let mut killer = KillRecorder::default();
        kill_unless_reaped(&mut killer, false);
        kill_unless_reaped(&mut killer, false);
        assert_eq!(killer.kills, 2);

        // Once the reaper has waited, the PID may have been reused; no signal.
        kill_unless_reaped(&mut killer, true);
        assert_eq!(killer.kills, 2);
    }
}
