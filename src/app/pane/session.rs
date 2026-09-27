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
    writer: Arc<Mutex<Option<Box<dyn Write + Send>>>>,
    title: SharedTitle,
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
        if let Some(bytes) = reply
            && let Some(w) = self
                .writer
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_mut()
        {
            let _ = w.write_all(&bytes);
        }
    }
}

fn read_loop(
    vpty: &Mutex<vt100::Parser<MuxCallbacks>>,
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
                vpty.lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .process(&buf[..n]);
                screen_changed.store(true, Ordering::Relaxed);
            }
            Err(_) => {
                exited.store(true, Ordering::Relaxed);
                break;
            }
        }
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
                writer: Arc::clone(&writer),
                title: title.clone(),
            },
        )));
        let screen_changed = Arc::new(AtomicBool::new(true));
        let vpt_clone = Arc::clone(&vpty);
        let sc_clone = Arc::clone(&screen_changed);

        let reader = pair.master.try_clone_reader()?;
        let exited = Arc::new(AtomicBool::new(false));
        let ex_clone = Arc::clone(&exited);
        let _reader_thread =
            std::thread::spawn(move || read_loop(&vpt_clone, &sc_clone, &ex_clone, reader));

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

    fn test_parser() -> (vt100::Parser<MuxCallbacks>, Arc<Mutex<Vec<u8>>>) {
        let (parser, bytes, _title) = test_parser_with_title();
        (parser, bytes)
    }

    fn test_parser_with_title() -> (
        vt100::Parser<MuxCallbacks>,
        Arc<Mutex<Vec<u8>>>,
        SharedTitle,
    ) {
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let writer: Box<dyn Write + Send> = Box::new(TestWriter(Arc::clone(&bytes)));
        let writer = Arc::new(Mutex::new(Some(writer)));
        let title = SharedTitle::default();

        let parser = vt100::Parser::new_with_callbacks(
            24,
            80,
            SCROLLBACK_SIZE,
            MuxCallbacks {
                writer: Arc::clone(&writer),
                title: title.clone(),
            },
        );
        (parser, bytes, title)
    }

    #[allow(clippy::type_complexity)]
    fn session_wiring() -> (
        Arc<Mutex<vt100::Parser<MuxCallbacks>>>,
        Arc<AtomicBool>,
        Arc<AtomicBool>,
        SharedTitle,
    ) {
        let title = SharedTitle::default();
        let writer: Box<dyn Write + Send> = Box::new(TestWriter(Arc::new(Mutex::new(Vec::new()))));
        let vpty = Arc::new(Mutex::new(vt100::Parser::new_with_callbacks(
            24,
            80,
            SCROLLBACK_SIZE,
            MuxCallbacks {
                writer: Arc::new(Mutex::new(Some(writer))),
                title: title.clone(),
            },
        )));
        let screen_changed = Arc::new(AtomicBool::new(false));
        let exited = Arc::new(AtomicBool::new(false));
        (vpty, screen_changed, exited, title)
    }

    #[test]
    fn da1_replies_vt220() {
        let (mut parser, bytes) = test_parser();
        parser.process(b"\x1b[c");
        assert_eq!(*bytes.lock().unwrap(), b"\x1b[?62;22c".to_vec());
    }

    #[test]
    fn dsr5_replies_ok() {
        let (mut parser, bytes) = test_parser();
        parser.process(b"\x1b[5n");
        assert_eq!(*bytes.lock().unwrap(), b"\x1b[0n".to_vec());
    }

    #[test]
    fn cpr_reports_one_based_position() {
        let (mut parser, bytes) = test_parser();
        parser.process(b"\x1b[3;5H\x1b[6n");
        assert_eq!(*bytes.lock().unwrap(), b"\x1b[3;5R".to_vec());
    }

    #[test]
    fn cpr_reports_home_as_1_1() {
        let (mut parser, bytes) = test_parser();
        parser.process(b"\x1b[H\x1b[6n");
        assert_eq!(*bytes.lock().unwrap(), b"\x1b[1;1R".to_vec());
    }

    #[test]
    fn private_cpr_is_answered() {
        let (mut parser, bytes) = test_parser();
        parser.process(b"\x1b[2;2H\x1b[?6n");
        assert_eq!(*bytes.lock().unwrap(), b"\x1b[2;2R".to_vec());
    }

    #[test]
    fn da2_replies_generic_terminal() {
        let (mut parser, bytes) = test_parser();
        parser.process(b"\x1b[>0c");
        assert_eq!(*bytes.lock().unwrap(), b"\x1b[>0;1;0c".to_vec());
    }

    #[test]
    fn printer_status_is_ignored() {
        let (mut parser, bytes) = test_parser();
        parser.process(b"\x1b[?5n");
        assert!(bytes.lock().unwrap().is_empty());
    }

    #[test]
    fn osc0_title_surfaces_as_single_change() {
        let (mut parser, _bytes, title) = test_parser_with_title();
        // OSC 0 fires both the icon-name and title callbacks with the same
        // string; the channel must still report exactly one change.
        parser.process(b"\x1b]0;my title\x07");
        assert_eq!(title.take_if_changed().as_deref(), Some("my title"));
        assert_eq!(title.take_if_changed(), None);
    }

    #[test]
    fn osc2_title_updates() {
        let (mut parser, _bytes, title) = test_parser_with_title();
        parser.process(b"\x1b]2;other title\x07");
        assert_eq!(title.take_if_changed().as_deref(), Some("other title"));
    }

    #[test]
    fn osc1_icon_name_counts_as_title() {
        let (mut parser, _bytes, title) = test_parser_with_title();
        parser.process(b"\x1b]1;icon name\x07");
        assert_eq!(title.take_if_changed().as_deref(), Some("icon name"));
    }

    #[test]
    fn non_utf8_title_is_ignored() {
        let (mut parser, _bytes, title) = test_parser_with_title();
        parser.process(b"\x1b]2;\xff\xfe\x07");
        assert_eq!(title.take_if_changed(), None);
    }

    #[test]
    fn latest_title_wins_before_sync() {
        let (mut parser, _bytes, title) = test_parser_with_title();
        parser.process(b"\x1b]2;first\x07");
        parser.process(b"\x1b]2;second\x07");
        assert_eq!(title.take_if_changed().as_deref(), Some("second"));
    }

    #[test]
    fn read_loop_publishes_title_and_flags_screen() {
        let (vpty, screen_changed, exited, title) = session_wiring();
        read_loop(
            &vpty,
            &screen_changed,
            &exited,
            std::io::Cursor::new(b"\x1b]2;hi\x07".to_vec()),
        );
        assert_eq!(title.take_if_changed().as_deref(), Some("hi"));
        assert!(screen_changed.swap(false, Ordering::Relaxed));
    }

    #[test]
    fn read_loop_output_lands_in_screen_and_sets_flag() {
        let (vpty, screen_changed, exited, _title) = session_wiring();
        read_loop(
            &vpty,
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
        let (vpty, screen_changed, exited, _title) = session_wiring();
        read_loop(
            &vpty,
            &screen_changed,
            &exited,
            std::io::Cursor::new(Vec::new()),
        );
        assert!(!screen_changed.swap(false, Ordering::Relaxed));
    }
}
