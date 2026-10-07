use crate::app::command::Command;
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

#[derive(Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub keybindings: KeyMap,
    pub terminal: TerminalConfig,
}

impl Config {
    /// Load from the XDG config path; return defaults if the file is missing.
    pub fn load() -> Result<Self> {
        let Some(config_dir) = dirs::config_dir() else {
            return Ok(Config::default());
        };
        let path = config_dir.join("multiplexer/config.toml");
        if !path.exists() {
            return Ok(Config::default());
        }

        let contents = std::fs::read_to_string(&path)?;
        let file_config: Config = toml::from_str(&contents)?;
        Ok(Config::default().merge(file_config))
    }

    fn merge(mut self, other: Config) -> Self {
        self.keybindings.bindings.extend(other.keybindings.bindings);
        self.terminal = other.terminal;
        self
    }
}

pub struct KeyMap {
    bindings: HashMap<KeySpec, Command>,
}

impl KeyMap {
    pub fn resolve(&self, key: KeyEvent, alternate: bool) -> Command {
        let spec = KeySpec::from(key);
        if let Some(command) = self.bindings.get(&spec) {
            return *command;
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
                return *command;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeySpec {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

impl From<KeyEvent> for KeySpec {
    fn from(key: KeyEvent) -> Self {
        Self {
            code: key.code,
            modifiers: key.modifiers,
        }
    }
}

impl FromStr for KeySpec {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split('-').collect();
        if parts.is_empty() {
            return Err("empty key spec".to_string());
        }

        let mut modifiers = KeyModifiers::empty();
        for part in &parts[..parts.len() - 1] {
            match part.to_ascii_lowercase().as_str() {
                "ctrl" => modifiers |= KeyModifiers::CONTROL,
                "alt" => modifiers |= KeyModifiers::ALT,
                "shift" => modifiers |= KeyModifiers::SHIFT,
                other => return Err(format!("unknown modifier: {other}")),
            }
        }

        let code = parse_key_code(parts.last().unwrap())?;
        Ok(KeySpec { code, modifiers })
    }
}

fn parse_key_code(s: &str) -> Result<KeyCode, String> {
    let s = s.to_ascii_lowercase();
    match s.as_str() {
        "home" => Ok(KeyCode::Home),
        "end" => Ok(KeyCode::End),
        "pageup" => Ok(KeyCode::PageUp),
        "pagedown" => Ok(KeyCode::PageDown),
        "up" => Ok(KeyCode::Up),
        "down" => Ok(KeyCode::Down),
        "left" => Ok(KeyCode::Left),
        "right" => Ok(KeyCode::Right),
        "tab" => Ok(KeyCode::Tab),
        "enter" | "return" => Ok(KeyCode::Enter),
        "backspace" => Ok(KeyCode::Backspace),
        "delete" | "del" => Ok(KeyCode::Delete),
        "insert" | "ins" => Ok(KeyCode::Insert),
        "escape" | "esc" => Ok(KeyCode::Esc),
        "space" => Ok(KeyCode::Char(' ')),
        "backtab" => Ok(KeyCode::BackTab),
        s if s.starts_with('f') => {
            let n: u8 = s[1..]
                .parse()
                .map_err(|_| format!("invalid function key: {s}"))?;
            if !(1..=12).contains(&n) {
                return Err(format!("function key out of range: f{n}"));
            }
            Ok(KeyCode::F(n))
        }
        s if s.chars().count() == 1 => {
            let c = s.chars().next().unwrap();
            Ok(KeyCode::Char(c))
        }
        _ => Err(format!("unknown key: {s}")),
    }
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
            _ => Err(format!("unknown command: {s}")),
        }
    }
}

#[derive(Default, Deserialize)]
pub struct TerminalConfig {
    pub shell: Option<PathBuf>,
    pub working_dir: Option<PathBuf>,
    pub env: HashMap<String, String>,
}

impl TerminalConfig {
    pub fn shell(&self) -> PathBuf {
        self.shell
            .clone()
            .or_else(|| std::env::var("SHELL").ok().map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("/bin/bash"))
    }

    pub fn working_dir(&self) -> PathBuf {
        self.working_dir
            .clone()
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."))
    }

    pub fn env(&self) -> Vec<(String, String)> {
        self.env
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_key_specs() {
        assert_eq!(
            "alt-c".parse::<KeySpec>().unwrap(),
            KeySpec {
                code: KeyCode::Char('c'),
                modifiers: KeyModifiers::ALT,
            }
        );
        assert_eq!(
            "ctrl-shift-t".parse::<KeySpec>().unwrap(),
            KeySpec {
                code: KeyCode::Char('t'),
                modifiers: KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            }
        );
        assert_eq!(
            "f12".parse::<KeySpec>().unwrap(),
            KeySpec {
                code: KeyCode::F(12),
                modifiers: KeyModifiers::NONE,
            }
        );
        assert_eq!(
            "home".parse::<KeySpec>().unwrap(),
            KeySpec {
                code: KeyCode::Home,
                modifiers: KeyModifiers::NONE,
            }
        );
    }

    #[test]
    fn keymap_overrides_defaults() {
        let toml = r#"
[keybindings]
alt-c = "next_tab"
"#;
        let config: Config = toml::from_str(toml).unwrap();
        let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::ALT);
        assert_eq!(config.keybindings.resolve(key, false), Command::NextTab);
    }

    #[test]
    fn unknown_command_is_ignored() {
        let toml = r#"
[keybindings]
alt-c = "not_a_command"
"#;
        let config: Config = toml::from_str(toml).unwrap();
        let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::ALT);
        assert_eq!(config.keybindings.resolve(key, false), Command::NewTab);
    }
}
