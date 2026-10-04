use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

const SCROLLBACK_SIZE: usize = 1200;

#[derive(Clone, Default)]
struct SharedTitle {
    title: Arc<Mutex<Option<String>>>,
    changed: Arc<AtomicBool>,
}

impl SharedTitle {
    fn set(&self, title: String) {
        *self.title.lock().unwrap_or_else(|e| e.into_inner()) = Some(title);
        self.changed.store(true, Ordering::Relaxed);
    }

    fn set_from_bytes(&self, title: &[u8]) {
        if let Ok(s) = std::str::from_utf8(title) {
            self.set(s.to_string());
        }
    }

    fn take_if_changed(&self) -> Option<String> {
        self.changed
            .swap(false, Ordering::Relaxed)
            .then(|| self.title.lock().unwrap_or_else(|e| e.into_inner()).clone())
            .flatten()
    }
}

struct MuxCallbacks {
    title: SharedTitle,
    /// Replies to terminal queries. Buffered here rather than written,
    /// because callbacks run while the parser mutex is held; `read_loop`
    /// flushes them after `process()` returns and the parser lock is released.
    reply: Vec<u8>,
}

impl MuxCallbacks {
    fn take_reply(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.reply)
    }
}

impl vt100::Callbacks for MuxCallbacks {
    fn set_window_title(&mut self, _screen: &mut vt100::Screen, title: &[u8]) {
        self.title.set_from_bytes(title);
    }

    fn set_window_icon_name(&mut self, _screen: &mut vt100::Screen, icon_name: &[u8]) {
        // treat OSC 1 the same as OSC 2 if you want icon-name-only tools to count
        self.title.set_from_bytes(icon_name);
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

fn read_loop(
    vpty: &Mutex<vt100::Parser<MuxCallbacks>>,
    writer: &Mutex<Option<Box<dyn Write + Send>>>,
    screen_changed: &AtomicBool,
    exited: &AtomicBool,
    mut reader: impl Read,
) {
    let mut buf = [0u8; 4096];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => {
                exited.store(true, Ordering::Relaxed);
                break;
            }
            Ok(n) => {
                // Lock order: parser, then writer — and never the reverse.
                // `process` runs under the parser lock, so any replies it
                // queues are taken while that lock is still held, then
                // flushed once it is released.
                let reply = {
                    let mut parser = vpty.lock().unwrap_or_else(|e| e.into_inner());
                    parser.process(&buf[..n]);
                    parser.callbacks_mut().take_reply()
                };
                if !reply.is_empty()
                    && let Some(w) = writer.lock().unwrap_or_else(|e| e.into_inner()).as_mut()
                {
                    let _ = w.write_all(&reply);
                }
                screen_changed.store(true, Ordering::Relaxed);
            }
            Err(_) => {
                exited.store(true, Ordering::Relaxed);
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
    writer: Arc<Mutex<Option<Box<dyn Write + Send>>>>,
    master: Box<dyn MasterPty>,
    child: Box<dyn Child + Send + Sync>,
    screen_changed: Arc<AtomicBool>,
    exited: Arc<AtomicBool>,
    title: SharedTitle,
    rows: u16,
    cols: u16,
}

impl PtySession {
    pub(super) fn in_alternate_screen(&self) -> bool {
        self.vpty
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .screen()
            .alternate_screen()
    }

    pub(super) fn spawn(rows: u16, cols: u16) -> Result<Self> {
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

        let writer = Arc::new(Mutex::new(Some(pair.master.take_writer()?)));
        let title = SharedTitle::default();
        let vpty = Arc::new(Mutex::new(vt100::Parser::new_with_callbacks(
            rows,
            cols,
            SCROLLBACK_SIZE,
            MuxCallbacks {
                title: title.clone(),
                reply: Vec::new(),
            },
        )));
        let screen_changed = Arc::new(AtomicBool::new(true));
        let vpt_clone = Arc::clone(&vpty);
        let sc_clone = Arc::clone(&screen_changed);
        let writer_clone = Arc::clone(&writer);

        let reader = pair.master.try_clone_reader()?;
        let exited = Arc::new(AtomicBool::new(false));
        let ex_clone = Arc::clone(&exited);
        let _reader_thread = std::thread::spawn(move || {
            read_loop(&vpt_clone, &writer_clone, &sc_clone, &ex_clone, reader)
        });

        Ok(PtySession {
            vpty,
            writer,
            master: pair.master,
            child,
            screen_changed,
            exited,
            title,
            rows,
            cols,
        })
    }

    pub(super) fn write_bytes(&self, bytes: &[u8]) -> Result<()> {
        if self.is_not_alive() {
            return Ok(());
        }
        if let Some(w) = self
            .writer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_mut()
        {
            w.write_all(bytes)?;
        }
        Ok(())
    }

    /// Whether the foreground application asked for bracketed paste
    /// (`ESC[?2004h`), which the parser tracks on the current screen.
    pub(super) fn bracketed_paste(&self) -> bool {
        self.vpty
            .lock()
            .unwrap_or_else(|e| e.into_inner())
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

    pub(super) fn is_not_alive(&self) -> bool {
        self.exited.load(Ordering::Relaxed)
    }

    pub(super) fn take_title(&self) -> Option<String> {
        self.title.take_if_changed()
    }

    pub(super) fn size(&self) -> (u16, u16) {
        (self.rows, self.cols)
    }

    /// Resize the backing PTY and the vt100 screen so both stay in sync.
    /// Skips the work (and avoids a spurious SIGWINCH on the shell) when the
    /// requested size is unchanged.
    pub(super) fn resize(&mut self, rows: u16, cols: u16) {
        let rows = rows.max(1);
        let cols = cols.max(1);
        if rows == self.rows && cols == self.cols {
            return;
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
        self.vpty
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .screen_mut()
            .set_size(rows, cols);
    }

    pub(super) fn scroll_offset(&self) -> usize {
        self.vpty
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .screen()
            .scrollback()
    }

    pub(super) fn set_scroll_offset(&self, offset: usize) {
        self.vpty
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .screen_mut()
            .set_scrollback(offset);
        log::debug!("offset {offset}");
    }

    pub(super) fn screen(&self) -> vt100::Screen {
        self.vpty
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .screen()
            .clone()
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        test_parser_with_title().0
    }

    fn test_parser_with_title() -> (vt100::Parser<MuxCallbacks>, SharedTitle) {
        let title = SharedTitle::default();
        let parser = vt100::Parser::new_with_callbacks(
            24,
            80,
            SCROLLBACK_SIZE,
            MuxCallbacks {
                title: title.clone(),
                reply: Vec::new(),
            },
        );
        (parser, title)
    }

    #[allow(clippy::type_complexity)]
    fn session_wiring() -> (
        Arc<Mutex<vt100::Parser<MuxCallbacks>>>,
        Arc<Mutex<Option<Box<dyn Write + Send>>>>,
        Arc<Mutex<Vec<u8>>>,
        Arc<AtomicBool>,
        Arc<AtomicBool>,
        SharedTitle,
    ) {
        let title = SharedTitle::default();
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let writer: Arc<Mutex<Option<Box<dyn Write + Send>>>> =
            Arc::new(Mutex::new(Some(Box::new(TestWriter(Arc::clone(&bytes))))));
        let vpty = Arc::new(Mutex::new(vt100::Parser::new_with_callbacks(
            24,
            80,
            SCROLLBACK_SIZE,
            MuxCallbacks {
                title: title.clone(),
                reply: Vec::new(),
            },
        )));
        let screen_changed = Arc::new(AtomicBool::new(false));
        let exited = Arc::new(AtomicBool::new(false));
        (vpty, writer, bytes, screen_changed, exited, title)
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
        let (mut parser, title) = test_parser_with_title();
        // OSC 0 fires both the icon-name and title callbacks with the same
        // string; the channel must still report exactly one change.
        parser.process(b"\x1b]0;my title\x07");
        assert_eq!(title.take_if_changed().as_deref(), Some("my title"));
        assert_eq!(title.take_if_changed(), None);
    }

    #[test]
    fn osc2_title_updates() {
        let (mut parser, title) = test_parser_with_title();
        parser.process(b"\x1b]2;other title\x07");
        assert_eq!(title.take_if_changed().as_deref(), Some("other title"));
    }

    #[test]
    fn osc1_icon_name_counts_as_title() {
        let (mut parser, title) = test_parser_with_title();
        parser.process(b"\x1b]1;icon name\x07");
        assert_eq!(title.take_if_changed().as_deref(), Some("icon name"));
    }

    #[test]
    fn non_utf8_title_is_ignored() {
        let (mut parser, title) = test_parser_with_title();
        parser.process(b"\x1b]2;\xff\xfe\x07");
        assert_eq!(title.take_if_changed(), None);
    }

    #[test]
    fn latest_title_wins_before_sync() {
        let (mut parser, title) = test_parser_with_title();
        parser.process(b"\x1b]2;first\x07");
        parser.process(b"\x1b]2;second\x07");
        assert_eq!(title.take_if_changed().as_deref(), Some("second"));
    }

    #[test]
    fn read_loop_publishes_title_and_flags_screen() {
        let (vpty, writer, _bytes, screen_changed, exited, title) = session_wiring();
        read_loop(
            &vpty,
            &writer,
            &screen_changed,
            &exited,
            std::io::Cursor::new(b"\x1b]2;hi\x07".to_vec()),
        );
        assert_eq!(title.take_if_changed().as_deref(), Some("hi"));
        assert!(screen_changed.swap(false, Ordering::Relaxed));
    }

    #[test]
    fn read_loop_output_lands_in_screen_and_sets_flag() {
        let (vpty, writer, _bytes, screen_changed, exited, _title) = session_wiring();
        read_loop(
            &vpty,
            &writer,
            &screen_changed,
            &exited,
            std::io::Cursor::new(b"hello".to_vec()),
        );
        assert_eq!(
            vpty.lock().unwrap().screen().rows(0, 80).next(),
            Some("hello".to_string())
        );
        assert!(screen_changed.swap(false, Ordering::Relaxed));
    }

    #[test]
    fn read_loop_eof_leaves_flag_clear() {
        let (vpty, writer, _bytes, screen_changed, exited, _title) = session_wiring();
        read_loop(
            &vpty,
            &writer,
            &screen_changed,
            &exited,
            std::io::Cursor::new(Vec::new()),
        );
        assert!(!screen_changed.swap(false, Ordering::Relaxed));
    }

    #[test]
    fn read_loop_flushes_buffered_csi_reply_after_processing() {
        let (vpty, writer, bytes, screen_changed, exited, _title) = session_wiring();
        read_loop(
            &vpty,
            &writer,
            &screen_changed,
            &exited,
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
