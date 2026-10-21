use crate::app::config::KeyMap;
use crate::app::workspace::valid_session_name;
use crossterm::event::KeyEvent;
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// Save the layout to a session file. `None` saves under the current
    /// session's name; `Some(name)` is "save as" and also makes that the
    /// current session.
    SaveSession(Option<String>),
    /// Replace the layout with a saved session. `None` reloads the current
    /// session from disk, discarding changes made since the last save.
    LoadSession(Option<String>),
    /// Switch to the next/previous session on disk (sorted), saving the
    /// current one first.
    NextSession,
    PrevSession,
    /// Not a multiplexer hotkey: forward the key to the active pane's PTY.
    /// Never read from config; constructed at runtime only.
    SendKey(KeyEvent),
    ShowBindings,
}

impl FromStr for Command {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "quit" => Ok(Command::Quit),
            "new_tab" => Ok(Command::NewTab),
            "next_tab" => Ok(Command::NextTab),
            "prev_tab" => Ok(Command::PrevTab),
            "cycle_grid" => Ok(Command::CycleGrid),
            "delete_pane" => Ok(Command::DeletePane),
            "next_pane" => Ok(Command::NextPane),
            "new_pane" => Ok(Command::NewPane),
            "scroll_to_top" => Ok(Command::ScrollToTop),
            "scroll_to_bottom" => Ok(Command::ScrollToBottom),
            "scroll_page_up" => Ok(Command::ScrollPageUp),
            "scroll_page_down" => Ok(Command::ScrollPageDown),
            "paste" => Ok(Command::Paste),
            "save_session" => Ok(Command::SaveSession(None)),
            "load_session" => Ok(Command::LoadSession(None)),
            "next_session" => Ok(Command::NextSession),
            "prev_session" => Ok(Command::PrevSession),
            _ => {
                if let Some((save, name)) = s
                    .strip_prefix("save_session:")
                    .map(|n| (true, n))
                    .or_else(|| s.strip_prefix("load_session:").map(|n| (false, n)))
                {
                    if !valid_session_name(name) {
                        return Err(format!("invalid session name: {name}"));
                    }
                    let name = Some(name.to_string());
                    return Ok(if save {
                        Command::SaveSession(name)
                    } else {
                        Command::LoadSession(name)
                    });
                }
                Err(format!("unknown command: {s}"))
            }
        }
    }
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

    #[test]
    fn session_commands_parse_with_and_without_names() {
        assert_eq!(
            "save_session".parse::<Command>(),
            Ok(Command::SaveSession(None))
        );
        assert_eq!(
            "load_session".parse::<Command>(),
            Ok(Command::LoadSession(None))
        );
        assert_eq!("next_session".parse::<Command>(), Ok(Command::NextSession));
        assert_eq!("prev_session".parse::<Command>(), Ok(Command::PrevSession));
        assert_eq!(
            "save_session:work".parse::<Command>(),
            Ok(Command::SaveSession(Some("work".to_string())))
        );
        assert_eq!(
            "load_session:work".parse::<Command>(),
            Ok(Command::LoadSession(Some("work".to_string())))
        );
    }

    #[test]
    fn session_commands_reject_unsafe_names() {
        assert!("save_session:".parse::<Command>().is_err());
        assert!("save_session:../evil".parse::<Command>().is_err());
        assert!("load_session:a/b".parse::<Command>().is_err());
    }
}
