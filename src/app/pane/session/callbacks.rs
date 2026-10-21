use crate::app::events::{PaneEvent, PaneId};
use std::sync::mpsc::Sender;

pub(super) struct MuxCallbacks {
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
    pub(super) fn new(id: PaneId, tx: Sender<PaneEvent>) -> Self {
        Self {
            id,
            tx,
            reply: Vec::new(),
            last_title: None,
        }
    }

    fn set_title(&mut self, title: &[u8]) {
        if let Ok(title) = std::str::from_utf8(title)
            && self.last_title.as_deref() != Some(title)
        {
            self.last_title = Some(title.to_string());
            let _ = self.tx.send(PaneEvent::Title(self.id, title.to_string()));
        }
    }

    pub(super) fn take_reply(&mut self) -> Vec<u8> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn test_parser() -> vt100::Parser<MuxCallbacks> {
        test_parser_with_events().0
    }

    fn test_parser_with_events() -> (vt100::Parser<MuxCallbacks>, mpsc::Receiver<PaneEvent>) {
        let (tx, rx) = mpsc::channel();
        let parser = vt100::Parser::new_with_callbacks(
            24,
            80,
            super::super::SCROLLBACK_SIZE,
            MuxCallbacks::new(PaneId(1), tx),
        );
        (parser, rx)
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
}
