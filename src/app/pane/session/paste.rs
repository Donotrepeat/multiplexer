/// Encode clipboard text as the bytes to write to a PTY.
///
/// Newlines are normalized to `\r` (the byte `Enter` sends), collapsing `\r\n`
/// first so Windows line endings do not become `\r\r`. Control characters,
/// `ESC` included, are stripped so a paste cannot inject terminal sequences
/// or close the bracketed-paste guard early. When `bracketed`, the payload is
/// wrapped in `ESC[200~` … `ESC[201~`, which tells a bracketed-paste-aware
/// application (zsh's zle, bash's readline, vim, …) to insert it as editable
/// text instead of executing each line.
pub(super) fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    let normalized = text.replace("\r\n", "\n");
    let mut payload = String::with_capacity(normalized.len());
    for c in normalized.chars() {
        match c {
            '\n' => payload.push('\r'),
            '\t' | '\r' => payload.push(c),
            c if c.is_control() => {}
            c => payload.push(c),
        }
    }

    if bracketed {
        let mut bytes = Vec::with_capacity(payload.len() + 12);
        bytes.extend_from_slice(b"\x1b[200~");
        bytes.extend_from_slice(payload.as_bytes());
        bytes.extend_from_slice(b"\x1b[201~");
        bytes
    } else {
        payload.into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bracketed_paste_mode_tracks_enable_and_disable() {
        let mut parser = vt100::Parser::new(24, 80, 1200);
        assert!(!parser.screen().bracketed_paste());
        parser.process(b"\x1b[?2004h");
        assert!(parser.screen().bracketed_paste());
        parser.process(b"\x1b[?2004l");
        assert!(!parser.screen().bracketed_paste());
    }

    #[test]
    fn paste_wraps_only_when_bracketed() {
        assert_eq!(
            paste_bytes("ls -l\ncargo test", true),
            b"\x1b[200~ls -l\rcargo test\x1b[201~".to_vec()
        );
        assert_eq!(
            paste_bytes("ls -l\ncargo test", false),
            b"ls -l\rcargo test".to_vec()
        );
    }

    #[test]
    fn paste_normalizes_newlines_to_carriage_return() {
        assert_eq!(paste_bytes("a\nb", false), b"a\rb".to_vec());
        assert_eq!(paste_bytes("a\r\nb\nc", false), b"a\rb\rc".to_vec());
        // A lone CR is already what Enter sends; it must not be doubled.
        assert_eq!(paste_bytes("a\rb\tc", false), b"a\rb\tc".to_vec());
    }

    #[test]
    fn paste_strips_control_characters() {
        // ESC could inject terminal sequences or close the bracket early;
        // Ctrl+letter controls would signal or edit the child.
        assert_eq!(paste_bytes("a\x1b[201~b\x03", false), b"a[201~b".to_vec());
    }

    #[test]
    fn paste_wrapper_is_the_only_escape_left() {
        let bytes = paste_bytes("evil\x1b[201~", true);
        assert_eq!(bytes, b"\x1b[200~evil[201~\x1b[201~".to_vec());
        assert_eq!(bytes.iter().filter(|&&b| b == 0x1b).count(), 2);
    }

    #[test]
    fn paste_decision_follows_the_parsed_mode() {
        let mut parser = vt100::Parser::new(24, 80, 1200);
        parser.process(b"\x1b[?2004h");
        assert_eq!(
            paste_bytes("hi\nthere", parser.screen().bracketed_paste()),
            b"\x1b[200~hi\rthere\x1b[201~".to_vec()
        );
    }
}
