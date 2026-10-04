use std::io::{ErrorKind, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

use crate::app::events::{PaneEvent, PaneId};
use crate::app::util::lock_or_recover;

const SCROLLBACK_SIZE: usize = 1200;

struct MuxCallbacks {
    id: PaneId,
    /// Channel back to the UI loop.
    tx: Sender<PaneEvent>,
    /// Replies to terminal queries. Buffered here rather than written,
    /// because callbacks run while the parser mutex is held; `read_loop`
    /// flushes them after `process()` returns and the parser lock is released.
    reply: Vec<u8>,
    /// Last title reported. OSC 0 fires both the icon-name and title callbacks
    /// with the same string; only a real change should reach the UI.
    last_title: Option<String>,
}

impl MuxCallbacks {
    fn set_title(&mut self, title: &[u8]) {
        if let Ok(title) = std::str::from_utf8(title)
            && self.last_title.as_deref() != Some(title)
        {
            self.last_title = Some(title.to_string());
            let _ = self.tx.send(PaneEvent::Title(self.id, title.to_string()));
        }
    }

    fn take_reply(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.reply)
    }
}

impl vt100::Callbacks for MuxCallbacks {
    fn set_window_title(&mut self, _screen: &mut vt100::Screen, title: &[u8]) {
        self.set_title(title);
    }

    fn set_window_icon_name(&mut self, _screen: &mut vt100::Screen, icon_name: &[u8]) {
        // treat OSC 1 the same as OSC 2 if you want icon-name-only tools to count
        self.set_title(icon_name);
    }
    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        i1: Option<u8>,
        _i2: Option<u8>,
        params: &[&[u16]],
        c: char,
    ) {
        let first = params.first().and_then(|p| p.first()).copied();
        let reply: Option<Vec<u8>> = match (i1, c) {
            (None, 'c') => Some(b"\x1b[?62;22c".to_vec()),
            (Some(b'>'), 'c') => Some(b"\x1b[>0;1;0c".to_vec()),
            (None, 'n') => match first {
                Some(5) => Some(b"\x1b[0n".to_vec()),
                Some(6) => {
                    let (row, col) = screen.cursor_position();
                    Some(format!("\x1b[{};{}R", row + 1, col + 1).into_bytes())
                }
                _ => None,
            },
            (Some(b'?'), 'n') => {
                if first == Some(6) {
                    let (row, col) = screen.cursor_position();
                    Some(format!("\x1b[{};{}R", row + 1, col + 1).into_bytes())
                } else {
                    None
                }
            }
            _ => None,
        };
        if let Some(bytes) = reply {
            self.reply.extend_from_slice(&bytes);
        }
    }
}

/// The write half of a pane's PTY.
///
/// A newtype so the hot paths never juggle an `Option` — nothing ever takes
/// the writer away — while tests can still inject a recording writer behind
/// `dyn Write`.
struct PtyWriter(Mutex<Box<dyn Write + Send>>);

impl PtyWriter {
    fn new(writer: Box<dyn Write + Send>) -> Self {
        Self(Mutex::new(writer))
    }

    /// Write `bytes` to the PTY. A failure means the child is gone; callers
    /// decide whether that is worth surfacing.
    fn write_all(&self, bytes: &[u8]) -> std::io::Result<()> {
        lock_or_recover(&self.0, "pty writer").write_all(bytes)
    }
}

fn read_loop(
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
            Ok(0) => {
                let _ = tx.send(PaneEvent::Exited(id));
                break;
            }
            Ok(n) => {
                // Lock order: parser, then writer — and never the reverse.
                // `process` runs under the parser lock, so any replies it
                // queues are taken while that lock is still held, then
                // flushed once it is released.
                let reply = {
                    let mut parser = lock_or_recover(vpty, "vt100 parser");
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
            Err(_) => {
                let _ = tx.send(PaneEvent::Exited(id));
                break;
            }
        }
    }
}

/// Encode clipboard text as the bytes to write to a PTY.
///
/// Newlines are normalized to `\r` (the byte `Enter` sends), collapsing `\r\n`
/// first so Windows line endings do not become `\r\r`. Control characters,
/// `ESC` included, are stripped so a paste cannot inject terminal sequences
/// or close the bracketed-paste guard early. When `bracketed`, the payload is
/// wrapped in `ESC[200~` … `ESC[201~`, which tells a bracketed-paste-aware
/// application (zsh's zle, bash's readline, vim, …) to insert it as editable
/// text instead of executing each line.
fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    let normalized = text.replace("\r\n", "\n");
    let mut payload = String::with_capacity(normalized.len());
    for c in normalized.chars() {
        match c {
            '\n' => payload.push('\r'),
            '\t' | '\r' => payload.push(c),
            c if c.is_control() => {}
            c => payload.push(c),
        }
    }

    if bracketed {
        let mut bytes = Vec::with_capacity(payload.len() + 12);
        bytes.extend_from_slice(b"\x1b[200~");
        bytes.extend_from_slice(payload.as_bytes());
        bytes.extend_from_slice(b"\x1b[201~");
        bytes
    } else {
        payload.into_bytes()
    }
}

pub(super) struct PtySession {
    vpty: Arc<Mutex<vt100::Parser<MuxCallbacks>>>,
    writer: Arc<PtyWriter>,
    master: Box<dyn MasterPty>,
    child: Box<dyn Child + Send + Sync>,
    screen_changed: Arc<AtomicBool>,
    rows: u16,
    cols: u16,
}

impl PtySession {
    pub(super) fn in_alternate_screen(&self) -> bool {
        lock_or_recover(&self.vpty, "vt100 parser")
            .screen()
            .alternate_screen()
    }

    pub(super) fn spawn(rows: u16, cols: u16, id: PaneId, tx: Sender<PaneEvent>) -> Result<Self> {
        let pair = native_pty_system().openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into());
        let cmd = CommandBuilder::new(shell);
        let child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);

        let writer = Arc::new(PtyWriter::new(pair.master.take_writer()?));
        let vpty = Arc::new(Mutex::new(vt100::Parser::new_with_callbacks(
            rows,
            cols,
            SCROLLBACK_SIZE,
            MuxCallbacks {
                id,
                tx: tx.clone(),
                reply: Vec::new(),
                last_title: None,
            },
        )));
        // Starts clear: the app draws its first frame without waiting for
        // output, and `read_loop` wakes it only on the next transition.
        let screen_changed = Arc::new(AtomicBool::new(false));
        let vpt_clone = Arc::clone(&vpty);
        let sc_clone = Arc::clone(&screen_changed);
        let writer_clone = Arc::clone(&writer);

        let reader = pair.master.try_clone_reader()?;
        let _reader_thread = std::thread::spawn(move || {
            read_loop(&vpt_clone, &writer_clone, &sc_clone, &tx, id, reader)
        });

        Ok(PtySession {
            vpty,
            writer,
            master: pair.master,
            child,
            screen_changed,
            rows,
            cols,
        })
    }

    pub(super) fn write_bytes(&self, bytes: &[u8]) -> Result<()> {
        self.writer.write_all(bytes)?;
        Ok(())
    }

    /// Whether the foreground application asked for bracketed paste
    /// (`ESC[?2004h`), which the parser tracks on the current screen.
    pub(super) fn bracketed_paste(&self) -> bool {
        lock_or_recover(&self.vpty, "vt100 parser")
            .screen()
            .bracketed_paste()
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
        lock_or_recover(&self.vpty, "vt100 parser")
            .screen_mut()
            .set_size(rows, cols);
        true
    }

    pub(super) fn scroll_offset(&self) -> usize {
        lock_or_recover(&self.vpty, "vt100 parser")
            .screen()
            .scrollback()
    }

    pub(super) fn set_scroll_offset(&self, offset: usize) {
        lock_or_recover(&self.vpty, "vt100 parser")
            .screen_mut()
            .set_scrollback(offset);
        log::debug!("offset {offset}");
    }

    pub(super) fn screen(&self) -> vt100::Screen {
        #[cfg(test)]
        SCREEN_CLONES.fetch_add(1, Ordering::Relaxed);
        lock_or_recover(&self.vpty, "vt100 parser").screen().clone()
    }
}

/// Test-only count of full screen clones, so a test can assert that idle
/// frames reuse the pane's cached snapshot.
#[cfg(test)]
pub(super) static SCREEN_CLONES: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

impl Drop for PtySession {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{self, Receiver, Sender};

    struct TestWriter(Arc<Mutex<Vec<u8>>>);

    impl Write for TestWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn test_parser() -> vt100::Parser<MuxCallbacks> {
        test_parser_with_events().0
    }

    fn test_parser_with_events() -> (vt100::Parser<MuxCallbacks>, Receiver<PaneEvent>) {
        let (tx, rx) = mpsc::channel();
        let parser = vt100::Parser::new_with_callbacks(
            24,
            80,
            SCROLLBACK_SIZE,
            MuxCallbacks {
                id: PaneId(1),
                tx,
                reply: Vec::new(),
                last_title: None,
            },
        );
        (parser, rx)
    }

    #[allow(clippy::type_complexity)]
    fn session_wiring() -> (
        Arc<Mutex<vt100::Parser<MuxCallbacks>>>,
        Arc<PtyWriter>,
        Arc<Mutex<Vec<u8>>>,
        Arc<AtomicBool>,
        Sender<PaneEvent>,
        Receiver<PaneEvent>,
    ) {
        let (tx, rx) = mpsc::channel();
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let writer = Arc::new(PtyWriter::new(Box::new(TestWriter(Arc::clone(&bytes)))));
        let vpty = Arc::new(Mutex::new(vt100::Parser::new_with_callbacks(
            24,
            80,
            SCROLLBACK_SIZE,
            MuxCallbacks {
                id: PaneId(1),
                tx: tx.clone(),
                reply: Vec::new(),
                last_title: None,
            },
        )));
        let screen_changed = Arc::new(AtomicBool::new(false));
        (vpty, writer, bytes, screen_changed, tx, rx)
    }

    #[test]
    fn da1_replies_vt220() {
        let mut parser = test_parser();
        parser.process(b"\x1b[c");
        assert_eq!(
            parser.callbacks_mut().take_reply(),
            b"\x1b[?62;22c".to_vec()
        );
    }

    #[test]
    fn dsr5_replies_ok() {
        let mut parser = test_parser();
        parser.process(b"\x1b[5n");
        assert_eq!(parser.callbacks_mut().take_reply(), b"\x1b[0n".to_vec());
    }

    #[test]
    fn cpr_reports_one_based_position() {
        let mut parser = test_parser();
        parser.process(b"\x1b[3;5H\x1b[6n");
        assert_eq!(parser.callbacks_mut().take_reply(), b"\x1b[3;5R".to_vec());
    }

    #[test]
    fn cpr_reports_home_as_1_1() {
        let mut parser = test_parser();
        parser.process(b"\x1b[H\x1b[6n");
        assert_eq!(parser.callbacks_mut().take_reply(), b"\x1b[1;1R".to_vec());
    }

    #[test]
    fn private_cpr_is_answered() {
        let mut parser = test_parser();
        parser.process(b"\x1b[2;2H\x1b[?6n");
        assert_eq!(parser.callbacks_mut().take_reply(), b"\x1b[2;2R".to_vec());
    }

    #[test]
    fn da2_replies_generic_terminal() {
        let mut parser = test_parser();
        parser.process(b"\x1b[>0c");
        assert_eq!(
            parser.callbacks_mut().take_reply(),
            b"\x1b[>0;1;0c".to_vec()
        );
    }

    #[test]
    fn printer_status_is_ignored() {
        let mut parser = test_parser();
        parser.process(b"\x1b[?5n");
        assert!(parser.callbacks_mut().take_reply().is_empty());
    }

    #[test]
    fn osc0_title_surfaces_as_single_change() {
        let (mut parser, events) = test_parser_with_events();
        // OSC 0 fires both the icon-name and title callbacks with the same
        // string; the channel must still report exactly one change.
        parser.process(b"\x1b]0;my title\x07");
        assert_eq!(
            events.try_recv().ok(),
            Some(PaneEvent::Title(PaneId(1), "my title".into()))
        );
        assert_eq!(events.try_recv().ok(), None);
    }

    #[test]
    fn osc2_title_updates() {
        let (mut parser, events) = test_parser_with_events();
        parser.process(b"\x1b]2;other title\x07");
        assert_eq!(
            events.try_recv().ok(),
            Some(PaneEvent::Title(PaneId(1), "other title".into()))
        );
    }

    #[test]
    fn osc1_icon_name_counts_as_title() {
        let (mut parser, events) = test_parser_with_events();
        parser.process(b"\x1b]1;icon name\x07");
        assert_eq!(
            events.try_recv().ok(),
            Some(PaneEvent::Title(PaneId(1), "icon name".into()))
        );
    }

    #[test]
    fn non_utf8_title_is_ignored() {
        let (mut parser, events) = test_parser_with_events();
        parser.process(b"\x1b]2;\xff\xfe\x07");
        assert_eq!(events.try_recv().ok(), None);
    }

    #[test]
    fn every_title_change_is_reported_in_order() {
        let (mut parser, events) = test_parser_with_events();
        parser.process(b"\x1b]2;first\x07");
        parser.process(b"\x1b]2;second\x07");
        assert_eq!(
            events.try_recv().ok(),
            Some(PaneEvent::Title(PaneId(1), "first".into()))
        );
        assert_eq!(
            events.try_recv().ok(),
            Some(PaneEvent::Title(PaneId(1), "second".into()))
        );
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
            vpty.lock().unwrap().screen().rows(0, 80).next(),
            Some("hello".to_string())
        );
        assert!(screen_changed.swap(false, Ordering::Relaxed));
        assert_eq!(events.try_recv().ok(), Some(PaneEvent::Output(PaneId(1))));
        assert_eq!(events.try_recv().ok(), Some(PaneEvent::Exited(PaneId(1))));
    }

    #[test]
    fn read_loop_eof_emits_exited() {
        let (vpty, writer, _bytes, screen_changed, tx, events) = session_wiring();
        read_loop(
            &vpty,
            &writer,
            &screen_changed,
            &tx,
            PaneId(1),
            std::io::Cursor::new(Vec::new()),
        );
        assert_eq!(events.try_recv().ok(), Some(PaneEvent::Exited(PaneId(1))));
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
        assert_eq!(events.try_recv().ok(), Some(PaneEvent::Exited(PaneId(1))));

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
        assert_eq!(events.try_recv().ok(), Some(PaneEvent::Exited(PaneId(1))));
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
        assert_eq!(*bytes.lock().unwrap(), b"\x1b[0n".to_vec());
        // The reply is buffered by the callbacks and only reaches the writer
        // through the post-`process` flush.
        assert!(vpty.lock().unwrap().callbacks_mut().take_reply().is_empty());
    }

    #[test]
    fn bracketed_paste_mode_tracks_enable_and_disable() {
        let mut parser = test_parser();
        assert!(!parser.screen().bracketed_paste());
        parser.process(b"\x1b[?2004h");
        assert!(parser.screen().bracketed_paste());
        parser.process(b"\x1b[?2004l");
        assert!(!parser.screen().bracketed_paste());
    }

    #[test]
    fn paste_wraps_only_when_bracketed() {
        assert_eq!(
            paste_bytes("ls -l\ncargo test", true),
            b"\x1b[200~ls -l\rcargo test\x1b[201~".to_vec()
        );
        assert_eq!(
            paste_bytes("ls -l\ncargo test", false),
            b"ls -l\rcargo test".to_vec()
        );
    }

    #[test]
    fn paste_normalizes_newlines_to_carriage_return() {
        assert_eq!(paste_bytes("a\nb", false), b"a\rb".to_vec());
        assert_eq!(paste_bytes("a\r\nb\nc", false), b"a\rb\rc".to_vec());
        // A lone CR is already what Enter sends; it must not be doubled.
        assert_eq!(paste_bytes("a\rb\tc", false), b"a\rb\tc".to_vec());
    }

    #[test]
    fn paste_strips_control_characters() {
        // ESC could inject terminal sequences or close the bracket early;
        // Ctrl+letter controls would signal or edit the child.
        assert_eq!(paste_bytes("a\x1b[201~b\x03", false), b"a[201~b".to_vec());
    }

    #[test]
    fn paste_wrapper_is_the_only_escape_left() {
        let bytes = paste_bytes("evil\x1b[201~", true);
        assert_eq!(bytes, b"\x1b[200~evil[201~\x1b[201~".to_vec());
        assert_eq!(bytes.iter().filter(|&&b| b == 0x1b).count(), 2);
    }

    #[test]
    fn paste_decision_follows_the_parsed_mode() {
        let mut parser = test_parser();
        parser.process(b"\x1b[?2004h");
        assert_eq!(
            paste_bytes("hi\nthere", parser.screen().bracketed_paste()),
            b"\x1b[200~hi\rthere\x1b[201~".to_vec()
        );
    }
}
