use crate::app::command::Command;
use crate::app::workspace::valid_session_name;
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::text::Line;
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
}

impl fmt::Display for KeySpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.modifiers.contains(KeyModifiers::CONTROL) {
            write!(f, "Ctrl+")?;
        }
        if self.modifiers.contains(KeyModifiers::ALT) {
            write!(f, "Alt+")?;
        }
        if self.modifiers.contains(KeyModifiers::SHIFT) {
            write!(f, "Shift+")?;
        }
        match self.code {
            KeyCode::Char(' ') => write!(f, "Space"),
            KeyCode::Char(c) => write!(f, "{c}"),
            other => write!(f, "{other:?}"),
        }
    }
}
impl KeyMap {
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
        insert("alt-g", Command::HideBindings);
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

#[derive(Clone, Default, Deserialize)]
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

    /// Layer a saved pane's shell and working directory onto this config.
    /// Overrides that no longer exist on disk are dropped, so a stale
    /// session restores with this config's values instead of failing to
    /// spawn — and, deeper in the fallback chain, `$SHELL` then
    /// `/bin/bash` (see [`TerminalConfig::shell`]). Baking the fallbacks
    /// in here, at restore time, keeps dead paths out of saved files.
    pub fn with_overrides(&self, shell: Option<&PathBuf>, working_dir: Option<&PathBuf>) -> Self {
        Self {
            shell: live_path(shell).or_else(|| self.shell.clone()),
            working_dir: live_path(working_dir).or_else(|| self.working_dir.clone()),
            env: self.env.clone(),
        }
    }
}

/// The saved path if it still exists on disk; a dead path restores as unset.
fn live_path(path: Option<&PathBuf>) -> Option<PathBuf> {
    path.filter(|p| p.exists()).cloned()
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
    fn with_overrides_uses_live_paths_and_drops_dead_ones() {
        let base = TerminalConfig {
            shell: Some(PathBuf::from("/bin/sh")),
            working_dir: None,
            env: HashMap::from([("TERM".to_string(), "xterm-256color".to_string())]),
        };

        // Dead saved paths fall back to the base config's values.
        let merged = base.with_overrides(
            Some(&PathBuf::from("/definitely/not/a/shell")),
            Some(&PathBuf::from("/definitely/not/a/dir")),
        );
        assert_eq!(merged.shell, Some(PathBuf::from("/bin/sh")));
        assert_eq!(merged.working_dir, None);

        // Live saved paths win over the base config's values.
        let cwd = std::env::current_dir().unwrap();
        let merged = base.with_overrides(Some(&PathBuf::from("/bin/bash")), Some(&cwd));
        assert_eq!(merged.shell, Some(PathBuf::from("/bin/bash")));
        assert_eq!(merged.working_dir, Some(cwd));
        // The base env travels with the merged config untouched.
        assert_eq!(
            merged.env.get("TERM").map(String::as_str),
            Some("xterm-256color")
        );
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

    #[test]
    fn named_session_binding_resolves_from_config() {
        let toml = r#"
[keybindings]
alt-s = "save_session:work"
"#;
        let config: Config = toml::from_str(toml).unwrap();
        let key = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::ALT);
        assert_eq!(
            config.keybindings.resolve(key, false),
            Command::SaveSession(Some("work".to_string()))
        );
    }
}
