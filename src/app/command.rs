use crate::app::config::KeyMap;
use crossterm::event::KeyEvent;
use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Command {
    Quit,
    NewTab,
    NextTab,
    PrevTab,
    CycleGrid,
    DeletePane,
    NextPane,
    NewPane,
    ScrollToTop,
    ScrollToBottom,
    ScrollPageUp,
    ScrollPageDown,
    Paste,
    /// Not a multiplexer hotkey: forward the key to the active pane's PTY.
    /// Never read from config; constructed at runtime only.
    #[serde(skip)]
    SendKey(KeyEvent),
}

/// Map a key event to the command it triggers.
///
/// Multiplexer hotkeys (Alt+letter) win first, then scroll keys; anything
/// else is forwarded to the active pane.
pub fn resolve(key: KeyEvent, alternate: bool, keybindings: &KeyMap) -> Command {
    keybindings.resolve(key, alternate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::config::KeyMap;
    use crossterm::event::{KeyCode, KeyModifiers};

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn alt_letters_map_to_mux_commands() {
        let bindings = KeyMap::default();
        let cases = [
            ('w', Command::Quit),
            ('c', Command::NewTab),
            ('e', Command::NextTab),
            ('q', Command::PrevTab),
            ('j', Command::CycleGrid),
            ('r', Command::DeletePane),
            ('n', Command::NextPane),
            ('t', Command::NewPane),
            ('v', Command::Paste),
        ];
        for (c, expected) in cases {
            assert_eq!(
                resolve(key(KeyCode::Char(c), KeyModifiers::ALT), false, &bindings),
                expected,
                "Alt+{c}"
            );
        }
    }

    #[test]
    fn alt_with_extra_modifiers_still_matches() {
        assert_eq!(
            resolve(
                key(KeyCode::Char('w'), KeyModifiers::ALT | KeyModifiers::SHIFT),
                false,
                &KeyMap::default()
            ),
            Command::Quit
        );
    }

    #[test]
    fn plain_letter_is_sent_to_pane() {
        let k = key(KeyCode::Char('w'), KeyModifiers::NONE);
        assert_eq!(resolve(k, false, &KeyMap::default()), Command::SendKey(k));
    }

    #[test]
    fn unbound_alt_letter_falls_through_to_pane() {
        let k = key(KeyCode::Char('x'), KeyModifiers::ALT);
        assert_eq!(resolve(k, false, &KeyMap::default()), Command::SendKey(k));
    }

    #[test]
    fn scroll_keys_fire_regardless_of_modifiers() {
        let bindings = KeyMap::default();
        let cases = [
            (KeyCode::Home, Command::ScrollToTop),
            (KeyCode::End, Command::ScrollToBottom),
            (KeyCode::PageUp, Command::ScrollPageUp),
            (KeyCode::PageDown, Command::ScrollPageDown),
        ];
        for (code, expected) in cases {
            assert_eq!(
                resolve(key(code, KeyModifiers::NONE), false, &bindings),
                expected,
                "{code:?}"
            );
            assert_eq!(
                resolve(key(code, KeyModifiers::ALT), false, &bindings),
                expected,
                "Alt+{code:?}"
            );
        }
    }
}
