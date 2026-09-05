use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// A high-level intent derived from a key event.
///
/// `App` translates raw key events into `Command`s via [`resolve`] and then
/// executes them, so the set of things a keypress can do is enumerated here
/// in one place instead of being spread across an `if`/`else if` cascade.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    /// Not a multiplexer hotkey: forward the key to the active pane's PTY.
    SendKey(KeyEvent),
}

/// Multiplexer hotkeys: Alt+letter → command.
const ALT_BINDINGS: &[(char, Command)] = &[
    ('w', Command::Quit),
    ('c', Command::NewTab),
    ('e', Command::NextTab),
    ('q', Command::PrevTab),
    ('j', Command::CycleGrid),
    ('r', Command::DeletePane),
    ('n', Command::NextPane),
    ('t', Command::NewPane),
];

/// Scroll keys, which fire regardless of modifiers.
const SCROLL_BINDINGS: &[(KeyCode, Command)] = &[
    (KeyCode::Home, Command::ScrollToTop),
    (KeyCode::End, Command::ScrollToBottom),
    (KeyCode::PageUp, Command::ScrollPageUp),
    (KeyCode::PageDown, Command::ScrollPageDown),
];

/// Map a key event to the command it triggers.
///
/// Multiplexer hotkeys (Alt+letter) win first, then scroll keys; anything
/// else is forwarded to the active pane.
pub fn resolve(key: KeyEvent) -> Command {
    if key.modifiers.contains(KeyModifiers::ALT)
        && let KeyCode::Char(c) = key.code
        && let Some((_, command)) = ALT_BINDINGS.iter().find(|(ch, _)| *ch == c)
    {
        return *command;
    }
    if let Some((_, command)) = SCROLL_BINDINGS.iter().find(|(code, _)| *code == key.code) {
        return *command;
    }
    Command::SendKey(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn alt_letters_map_to_mux_commands() {
        let cases = [
            ('w', Command::Quit),
            ('c', Command::NewTab),
            ('e', Command::NextTab),
            ('q', Command::PrevTab),
            ('j', Command::CycleGrid),
            ('r', Command::DeletePane),
            ('n', Command::NextPane),
            ('t', Command::NewPane),
        ];
        for (c, expected) in cases {
            assert_eq!(
                resolve(key(KeyCode::Char(c), KeyModifiers::ALT)),
                expected,
                "Alt+{c}"
            );
        }
    }

    #[test]
    fn alt_with_extra_modifiers_still_matches() {
        assert_eq!(
            resolve(key(
                KeyCode::Char('w'),
                KeyModifiers::ALT | KeyModifiers::SHIFT
            )),
            Command::Quit
        );
    }

    #[test]
    fn plain_letter_is_sent_to_pane() {
        let k = key(KeyCode::Char('w'), KeyModifiers::NONE);
        assert_eq!(resolve(k), Command::SendKey(k));
    }

    #[test]
    fn unbound_alt_letter_falls_through_to_pane() {
        let k = key(KeyCode::Char('x'), KeyModifiers::ALT);
        assert_eq!(resolve(k), Command::SendKey(k));
    }

    #[test]
    fn scroll_keys_fire_regardless_of_modifiers() {
        let cases = [
            (KeyCode::Home, Command::ScrollToTop),
            (KeyCode::End, Command::ScrollToBottom),
            (KeyCode::PageUp, Command::ScrollPageUp),
            (KeyCode::PageDown, Command::ScrollPageDown),
        ];
        for (code, expected) in cases {
            assert_eq!(resolve(key(code, KeyModifiers::NONE)), expected, "{code:?}");
            assert_eq!(
                resolve(key(code, KeyModifiers::ALT)),
                expected,
                "Alt+{code:?}"
            );
        }
    }
}
