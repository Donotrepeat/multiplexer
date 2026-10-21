use crate::app::command::Command;
use crate::app::config::KeySpec;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::text::Line;
use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use std::collections::HashMap;
use std::fmt;

pub struct KeyMap {
    bindings: HashMap<KeySpec, Command>,
}

impl KeyMap {
    /// Overlay `other` onto `self`, with `other` winning key conflicts.
    pub fn merge(&mut self, other: KeyMap) {
        self.bindings.extend(other.bindings);
    }

    /// One line per binding, e.g. "Alt+C    NewTab".
    pub fn lines(&self) -> Vec<Line<'static>> {
        let mut rows: Vec<(String, String)> = self
            .bindings
            .iter()
            .map(|(spec, cmd)| (spec.to_string(), format!("{cmd:?}")))
            .collect();
        rows.sort(); // HashMap iteration order is random; sort for stable output
        rows.into_iter()
            .map(|(key, cmd)| Line::from(format!("{key:<10} {cmd}")))
            .collect()
    }

    pub fn resolve(&self, key: KeyEvent, alternate: bool) -> Command {
        let spec = KeySpec::from(key);
        if let Some(command) = self.bindings.get(&spec) {
            return command.clone();
        }

        // Char bindings match even with extra modifiers (Alt+Shift+c still
        // triggers an Alt+c binding), but a plain char binding only matches
        // exactly.
        if let KeyCode::Char(c) = key.code {
            let c = c.to_ascii_lowercase();
            let pressed =
                key.modifiers & (KeyModifiers::SHIFT | KeyModifiers::ALT | KeyModifiers::CONTROL);
            let mut best: Option<&Command> = None;
            let mut best_bits = 0;
            for (binding, command) in &self.bindings {
                let KeyCode::Char(bc) = binding.code else {
                    continue;
                };
                if bc.to_ascii_lowercase() != c {
                    continue;
                }
                let needed = binding.modifiers
                    & (KeyModifiers::SHIFT | KeyModifiers::ALT | KeyModifiers::CONTROL);
                if needed.is_empty() || !pressed.contains(needed) {
                    continue;
                }
                let bits = needed.bits().count_ones();
                if bits > best_bits {
                    best = Some(command);
                    best_bits = bits;
                }
            }
            if let Some(command) = best {
                return command.clone();
            }
        }

        if !alternate {
            match key.code {
                KeyCode::Home => return Command::ScrollToTop,
                KeyCode::End => return Command::ScrollToBottom,
                KeyCode::PageUp => return Command::ScrollPageUp,
                KeyCode::PageDown => return Command::ScrollPageDown,
                _ => {}
            }
        }

        Command::SendKey(key)
    }
}

impl Default for KeyMap {
    fn default() -> Self {
        let mut bindings = HashMap::new();
        let mut insert = |spec: &str, cmd: Command| {
            if let Ok(s) = spec.parse::<KeySpec>() {
                bindings.insert(s, cmd);
            }
        };
        insert("alt-w", Command::Quit);
        insert("alt-c", Command::NewTab);
        insert("alt-e", Command::NextTab);
        insert("alt-q", Command::PrevTab);
        insert("alt-j", Command::CycleGrid);
        insert("alt-r", Command::DeletePane);
        insert("alt-n", Command::NextPane);
        insert("alt-t", Command::NewPane);
        insert("alt-v", Command::Paste);
        insert("alt-h", Command::ShowBindings);
        insert("alt-o", Command::NextSession);
        insert("alt-i", Command::PrevSession);
        Self { bindings }
    }
}

impl<'de> Deserialize<'de> for KeyMap {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct KeyMapVisitor;

        impl<'de> Visitor<'de> for KeyMapVisitor {
            type Value = KeyMap;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a table of key specs to commands")
            }

            fn visit_map<M>(self, mut access: M) -> Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut map = HashMap::new();
                while let Some((key, value)) = access.next_entry::<String, String>()? {
                    let key = key.to_ascii_lowercase();
                    let value = value.to_ascii_lowercase();
                    match key.parse::<KeySpec>() {
                        Ok(spec) => match value.parse::<Command>() {
                            Ok(command) => {
                                map.insert(spec, command);
                            }
                            Err(err) => {
                                log::warn!("config: ignoring binding {key}: {err}");
                            }
                        },
                        Err(err) => {
                            log::warn!("config: ignoring key {key}: {err}");
                        }
                    }
                }
                let mut defaults = KeyMap::default();
                defaults.bindings.extend(map);
                Ok(defaults)
            }
        }

        deserializer.deserialize_map(KeyMapVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyModifiers};

    #[test]
    fn keymap_overrides_defaults() {
        let toml = r#"
[keybindings]
alt-c = "next_tab"
"#;
        let config: crate::app::config::Config = toml::from_str(toml).unwrap();
        let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::ALT);
        assert_eq!(config.keybindings.resolve(key, false), Command::NextTab);
    }

    #[test]
    fn unknown_command_is_ignored() {
        let toml = r#"
[keybindings]
alt-c = "not_a_command"
"#;
        let config: crate::app::config::Config = toml::from_str(toml).unwrap();
        let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::ALT);
        assert_eq!(config.keybindings.resolve(key, false), Command::NewTab);
    }

    #[test]
    fn named_session_binding_resolves_from_config() {
        let toml = r#"
[keybindings]
alt-s = "save_session:work"
"#;
        let config: crate::app::config::Config = toml::from_str(toml).unwrap();
        let key = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::ALT);
        assert_eq!(
            config.keybindings.resolve(key, false),
            Command::SaveSession(Some("work".to_string()))
        );
    }
}
