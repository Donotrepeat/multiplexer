use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub(super) fn key_to_bytes(key: &KeyEvent) -> Vec<u8> {
    if let Some(seq) = csi_key_to_bytes(key) {
        return seq;
    }

    let mut bytes = match key.code {
        KeyCode::Enter => b"\r".to_vec(),
        KeyCode::Tab => b"\t".to_vec(),
        KeyCode::Backspace => b"\x7f".to_vec(),
        KeyCode::Esc => b"\x1b".to_vec(),
        KeyCode::Char(c) if key.modifiers.contains(KeyModifiers::CONTROL) => {
            match control_char_byte(c) {
                Some(b) => vec![b],
                None => c.to_string().into_bytes(),
            }
        }
        KeyCode::Char(c) => c.to_string().into_bytes(),
        _ => Vec::new(),
    };

    if key.modifiers.contains(KeyModifiers::ALT) && !bytes.is_empty() {
        bytes.insert(0, 0x1b);
    }
    bytes
}

fn csi_key_to_bytes(key: &KeyEvent) -> Option<Vec<u8>> {
    let modifiers =
        key.modifiers & (KeyModifiers::SHIFT | KeyModifiers::ALT | KeyModifiers::CONTROL);
    let m = modifier_param(modifiers);
    let modified = !modifiers.is_empty();

    match key.code {
        KeyCode::Up => Some(ansi_csi("A", modified, m)),
        KeyCode::Down => Some(ansi_csi("B", modified, m)),
        KeyCode::Right => Some(ansi_csi("C", modified, m)),
        KeyCode::Left => Some(ansi_csi("D", modified, m)),
        KeyCode::Home => Some(ansi_csi("H", modified, m)),
        KeyCode::End => Some(ansi_csi("F", modified, m)),
        KeyCode::Insert => Some(ansi_tilde(2, modified, m)),
        KeyCode::Delete => Some(ansi_tilde(3, modified, m)),
        KeyCode::PageUp => Some(ansi_tilde(5, modified, m)),
        KeyCode::PageDown => Some(ansi_tilde(6, modified, m)),
        KeyCode::F(n) if (1..=12).contains(&n) => Some(f_key_to_bytes(n, modified, m)),
        KeyCode::Tab if modifiers.contains(KeyModifiers::SHIFT) => Some(b"\x1b[Z".to_vec()),
        _ => None,
    }
}

fn ansi_csi(final_byte: &str, modified: bool, m: u8) -> Vec<u8> {
    if modified {
        format!("\x1b[1;{}{}", m, final_byte).into_bytes()
    } else {
        format!("\x1b[{}", final_byte).into_bytes()
    }
}

fn ansi_tilde(n: u8, modified: bool, m: u8) -> Vec<u8> {
    if modified {
        format!("\x1b[{};{}~", n, m).into_bytes()
    } else {
        format!("\x1b[{}~", n).into_bytes()
    }
}

fn f_key_to_bytes(n: u8, modified: bool, m: u8) -> Vec<u8> {
    match n {
        1..=4 if modified => format!("\x1b[1;{}{}", m, f_final(n)).into_bytes(),
        1..=4 => format!("\x1bO{}", f_final(n)).into_bytes(),
        _ => {
            let code = match n {
                5 => 15,
                6 => 17,
                7 => 18,
                8 => 19,
                9 => 20,
                10 => 21,
                11 => 23,
                12 => 24,
                _ => unreachable!(),
            };
            ansi_tilde(code, modified, m)
        }
    }
}

fn f_final(n: u8) -> char {
    match n {
        1 => 'P',
        2 => 'Q',
        3 => 'R',
        4 => 'S',
        _ => unreachable!(),
    }
}

fn modifier_param(modifiers: KeyModifiers) -> u8 {
    let shift = modifiers.contains(KeyModifiers::SHIFT) as u8;
    let alt = modifiers.contains(KeyModifiers::ALT) as u8;
    let ctrl = modifiers.contains(KeyModifiers::CONTROL) as u8;
    1 + shift + 2 * alt + 4 * ctrl
}

fn control_char_byte(c: char) -> Option<u8> {
    let c = c.to_ascii_lowercase();
    match c {
        'a'..='z' => Some(c as u8 - b'a' + 1),
        '@' | ' ' => Some(0x00),
        '[' => Some(0x1b),
        '\\' => Some(0x1c),
        ']' => Some(0x1d),
        '^' => Some(0x1e),
        '_' | '/' => Some(0x1f),
        '?' => Some(0x7f),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pressed(code: KeyCode, modifiers: KeyModifiers) -> Vec<u8> {
        key_to_bytes(&KeyEvent::new(code, modifiers))
    }

    #[test]
    fn control_letters_encode_to_c0() {
        for (i, c) in ('a'..='z').enumerate() {
            assert_eq!(
                pressed(KeyCode::Char(c), KeyModifiers::CONTROL),
                vec![(i + 1) as u8],
                "Ctrl+{c}"
            );
        }
    }

    #[test]
    fn control_uppercase_and_shift_are_normalized() {
        assert_eq!(
            pressed(KeyCode::Char('C'), KeyModifiers::CONTROL),
            vec![0x03],
            "Ctrl+C reported as uppercase"
        );
        assert_eq!(
            pressed(
                KeyCode::Char('C'),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT
            ),
            vec![0x03],
            "Ctrl+Shift+C"
        );
    }

    #[test]
    fn control_punctuation_covers_rest_of_c0() {
        let cases = [
            ('@', 0x00),
            (' ', 0x00),
            ('[', 0x1b),
            ('\\', 0x1c),
            (']', 0x1d),
            ('^', 0x1e),
            ('_', 0x1f),
            ('/', 0x1f),
            ('?', 0x7f),
        ];
        for (c, expected) in cases {
            assert_eq!(
                pressed(KeyCode::Char(c), KeyModifiers::CONTROL),
                vec![expected],
                "Ctrl+{c}"
            );
        }
    }

    #[test]
    fn control_on_unmappable_char_is_sent_literally() {
        assert_eq!(
            pressed(KeyCode::Char('1'), KeyModifiers::CONTROL),
            b"1".to_vec()
        );
        assert_eq!(
            pressed(KeyCode::Char('é'), KeyModifiers::CONTROL),
            "é".as_bytes().to_vec()
        );
    }

    #[test]
    fn navigation_and_edit_keys_get_terminal_sequences() {
        let cases = [
            (KeyCode::Enter, &b"\r"[..]),
            (KeyCode::Tab, b"\t"),
            (KeyCode::Backspace, b"\x7f"),
            (KeyCode::Esc, b"\x1b"),
            (KeyCode::Up, b"\x1b[A"),
            (KeyCode::Down, b"\x1b[B"),
            (KeyCode::Right, b"\x1b[C"),
            (KeyCode::Left, b"\x1b[D"),
            (KeyCode::Delete, b"\x1b[3~"),
            (KeyCode::Null, b""),
        ];
        for (code, expected) in cases {
            assert_eq!(pressed(code, KeyModifiers::NONE), expected, "{code:?}");
        }
    }

    #[test]
    fn plain_chars_are_sent_as_utf8() {
        assert_eq!(
            pressed(KeyCode::Char('a'), KeyModifiers::NONE),
            b"a".to_vec()
        );
        assert_eq!(
            pressed(KeyCode::Char('é'), KeyModifiers::NONE),
            "é".as_bytes().to_vec()
        );
    }

    #[test]
    fn alt_keys_are_esc_prefixed() {
        // xterm "meta sends escape": Alt+key = ESC + unmodified bytes.
        assert_eq!(
            pressed(KeyCode::Char('x'), KeyModifiers::ALT),
            b"\x1bx".to_vec()
        );
        assert_eq!(
            pressed(KeyCode::Enter, KeyModifiers::ALT),
            b"\x1b\r".to_vec()
        );
        assert_eq!(
            pressed(KeyCode::Backspace, KeyModifiers::ALT),
            b"\x1b\x7f".to_vec()
        );
        // Alt on CSI keys uses the modifier-parameter form, not ESC-prefix.
        assert_eq!(
            pressed(KeyCode::Up, KeyModifiers::ALT),
            b"\x1b[1;3A".to_vec()
        );
        // Alt composes with Ctrl: ESC + the control byte.
        assert_eq!(
            pressed(
                KeyCode::Char('c'),
                KeyModifiers::ALT | KeyModifiers::CONTROL
            ),
            b"\x1b\x03".to_vec()
        );
    }

    #[test]
    fn csi_keys_encode_with_modifier_parameter() {
        let cases: &[(KeyCode, KeyModifiers, &[u8])] = &[
            (KeyCode::Home, KeyModifiers::NONE, b"\x1b[H"),
            (KeyCode::Home, KeyModifiers::SHIFT, b"\x1b[1;2H"),
            (KeyCode::Home, KeyModifiers::CONTROL, b"\x1b[1;5H"),
            (KeyCode::Home, KeyModifiers::ALT, b"\x1b[1;3H"),
            (
                KeyCode::Home,
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
                b"\x1b[1;6H",
            ),
            (KeyCode::End, KeyModifiers::NONE, b"\x1b[F"),
            (KeyCode::End, KeyModifiers::SHIFT, b"\x1b[1;2F"),
            (KeyCode::End, KeyModifiers::CONTROL, b"\x1b[1;5F"),
            (KeyCode::End, KeyModifiers::ALT, b"\x1b[1;3F"),
            (
                KeyCode::End,
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
                b"\x1b[1;6F",
            ),
            (KeyCode::Insert, KeyModifiers::NONE, b"\x1b[2~"),
            (KeyCode::Insert, KeyModifiers::SHIFT, b"\x1b[2;2~"),
            (KeyCode::Insert, KeyModifiers::CONTROL, b"\x1b[2;5~"),
            (KeyCode::Insert, KeyModifiers::ALT, b"\x1b[2;3~"),
            (
                KeyCode::Insert,
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
                b"\x1b[2;6~",
            ),
            (KeyCode::PageUp, KeyModifiers::NONE, b"\x1b[5~"),
            (KeyCode::PageUp, KeyModifiers::SHIFT, b"\x1b[5;2~"),
            (KeyCode::PageUp, KeyModifiers::CONTROL, b"\x1b[5;5~"),
            (KeyCode::PageUp, KeyModifiers::ALT, b"\x1b[5;3~"),
            (
                KeyCode::PageUp,
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
                b"\x1b[5;6~",
            ),
            (KeyCode::PageDown, KeyModifiers::NONE, b"\x1b[6~"),
            (KeyCode::PageDown, KeyModifiers::SHIFT, b"\x1b[6;2~"),
            (KeyCode::PageDown, KeyModifiers::CONTROL, b"\x1b[6;5~"),
            (KeyCode::PageDown, KeyModifiers::ALT, b"\x1b[6;3~"),
            (
                KeyCode::PageDown,
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
                b"\x1b[6;6~",
            ),
            (KeyCode::F(1), KeyModifiers::NONE, b"\x1bOP"),
            (KeyCode::F(1), KeyModifiers::SHIFT, b"\x1b[1;2P"),
            (KeyCode::F(1), KeyModifiers::CONTROL, b"\x1b[1;5P"),
            (KeyCode::F(1), KeyModifiers::ALT, b"\x1b[1;3P"),
            (
                KeyCode::F(1),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
                b"\x1b[1;6P",
            ),
            (KeyCode::F(4), KeyModifiers::NONE, b"\x1bOS"),
            (KeyCode::F(4), KeyModifiers::SHIFT, b"\x1b[1;2S"),
            (KeyCode::F(4), KeyModifiers::CONTROL, b"\x1b[1;5S"),
            (KeyCode::F(4), KeyModifiers::ALT, b"\x1b[1;3S"),
            (
                KeyCode::F(4),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
                b"\x1b[1;6S",
            ),
            (KeyCode::F(5), KeyModifiers::NONE, b"\x1b[15~"),
            (KeyCode::F(5), KeyModifiers::SHIFT, b"\x1b[15;2~"),
            (KeyCode::F(5), KeyModifiers::CONTROL, b"\x1b[15;5~"),
            (KeyCode::F(5), KeyModifiers::ALT, b"\x1b[15;3~"),
            (
                KeyCode::F(5),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
                b"\x1b[15;6~",
            ),
            (KeyCode::F(12), KeyModifiers::NONE, b"\x1b[24~"),
            (KeyCode::F(12), KeyModifiers::SHIFT, b"\x1b[24;2~"),
            (KeyCode::F(12), KeyModifiers::CONTROL, b"\x1b[24;5~"),
            (KeyCode::F(12), KeyModifiers::ALT, b"\x1b[24;3~"),
            (
                KeyCode::F(12),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
                b"\x1b[24;6~",
            ),
        ];

        for (code, modifiers, expected) in cases {
            assert_eq!(
                pressed(*code, *modifiers),
                expected.to_vec(),
                "{code:?} with {modifiers:?}"
            );
        }

        assert_eq!(
            pressed(KeyCode::Tab, KeyModifiers::SHIFT),
            b"\x1b[Z".to_vec()
        );
    }
}
