use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::fmt;
use std::str::FromStr;

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
}
