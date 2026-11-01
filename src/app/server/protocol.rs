use anyhow::Result;
use crossterm::event::{Event, KeyEvent};
use ratatui::style::{Color, Modifier, Style};
use serde::de::DeserializeOwned;
use serde::de::{
    self, Deserialize, Deserializer, EnumAccess, SeqAccess, Unexpected, VariantAccess, Visitor,
};
use serde::ser::{Serialize, SerializeTupleVariant, Serializer};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::io::Read;

const PROTOCOL_VERSION: u8 = 1;
const MAX_MESSAGE_LEN: usize = 3000;

#[derive(Debug)]
pub enum ProtocolError {
    MessageTooLarge { len: usize, max: usize },
    UnsupportedVersion(u8),
    Io(std::io::Error),
    Json(serde_json::Error),
}
#[derive(Serialize)]
struct Envelope<'a, T> {
    version: u8,
    message: &'a T,
}

#[derive(Deserialize)]
struct DecodeEnvelope<T> {
    version: u8,
    message: T,
}
pub fn encode_message<T: Serialize>(message: &T) -> Result<Vec<u8>, ProtocolError> {
    let payload = serde_json::to_vec(&Envelope {
        version: 1,
        message,
    })
    .map_err(ProtocolError::Json)?;

    if payload.len() > MAX_MESSAGE_LEN {
        return Err(ProtocolError::MessageTooLarge {
            len: payload.len(),
            max: MAX_MESSAGE_LEN,
        });
    }

    let len = u32::try_from(payload.len()).map_err(|_| ProtocolError::MessageTooLarge {
        len: payload.len(),
        max: u32::MAX as usize,
    })?;

    let mut framed = Vec::with_capacity(4 + payload.len());
    framed.extend_from_slice(&len.to_be_bytes());
    framed.extend_from_slice(&payload);
    Ok(framed)
}

fn length_prefix(buffer: &[u8]) -> Vec<u8> {
    let length_bytes: [u8; 4] = buffer[0..4].try_into().unwrap();
    let mut msg = Vec::from(&length_bytes);
    msg.extend_from_slice(buffer);
    msg
}

#[derive(Clone, Serialize, Deserialize)]
struct FramePacket {
    rows: usize,
    cols: usize,
    tab_bar: Vec<String>,
    active_tab: usize,
    panes: Vec<PaneFrame>,
}

#[derive(Clone, Serialize, Deserialize)]
struct PaneFrame {
    id: usize,
    title: String,
    exited: bool,
    cursor: Option<(usize, usize)>,
    cells: Vec<Cell>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Cell {
    row: usize,
    col: usize,
    symbol: String,
    style: Style,
}
pub enum ClientEvent {
    Key(KeyEvent),
    Resize(u16, u16),
    Detach,
}

impl Serialize for ClientEvent {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Key(key) => serializer.serialize_newtype_variant("ClientEvent", 0, "Key", key),
            Self::Detach => serializer.serialize_unit_variant("ClientEvent", 2, "Detach"),
            Self::Resize(w, h) => {
                let mut tv = serializer.serialize_tuple_variant("ClientEvent", 1, "Resize", 2)?;
                tv.serialize_field(w)?;
                tv.serialize_field(h)?;
                tv.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for ClientEvent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        // 1. Which variant is it?
        enum Field {
            Key,
            Resize,
            Detach,
        }

        struct FieldVisitor;

        impl<'de> Visitor<'de> for FieldVisitor {
            type Value = Field;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("`Key`, `Resize`, or `Detach`")
            }

            // Binary formats (bincode, etc.) send the index
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Field, E> {
                match v {
                    0 => Ok(Field::Key),
                    1 => Ok(Field::Resize),
                    2 => Ok(Field::Detach),
                    _ => Err(E::invalid_value(
                        Unexpected::Unsigned(v),
                        &"variant index 0..=2",
                    )),
                }
            }

            // Self-describing formats (JSON, etc.) send the name
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Field, E> {
                match v {
                    "Key" => Ok(Field::Key),
                    "Resize" => Ok(Field::Resize),
                    "Detach" => Ok(Field::Detach),
                    _ => Err(E::unknown_variant(v, VARIANTS)),
                }
            }
        }

        impl<'de> Deserialize<'de> for Field {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                d.deserialize_identifier(FieldVisitor)
            }
        }

        // 2. Visitor for Resize(u16, u16)
        struct ResizeVisitor;

        impl<'de> Visitor<'de> for ResizeVisitor {
            type Value = ClientEvent;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("tuple variant ClientEvent::Resize")
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<ClientEvent, A::Error> {
                let w = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(0, &self))?;
                let h = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(1, &self))?;
                Ok(ClientEvent::Resize(w, h))
            }
        }

        // 3. Visitor for the whole enum
        struct ClientEventVisitor;

        impl<'de> Visitor<'de> for ClientEventVisitor {
            type Value = ClientEvent;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("enum ClientEvent")
            }

            fn visit_enum<A: EnumAccess<'de>>(self, data: A) -> Result<ClientEvent, A::Error> {
                match data.variant()? {
                    (Field::Key, v) => v.newtype_variant().map(ClientEvent::Key),
                    (Field::Resize, v) => v.tuple_variant(2, ResizeVisitor),
                    (Field::Detach, v) => {
                        v.unit_variant()?;
                        Ok(ClientEvent::Detach)
                    }
                }
            }
        }

        const VARIANTS: &[&str] = &["Key", "Resize", "Detach"];
        deserializer.deserialize_enum("ClientEvent", VARIANTS, ClientEventVisitor)
    }
}

pub enum ServerEvent {
    Key(KeyEvent),
    Resize(u16, u16),
    Detach,
    Frame(FramePacket),
}
impl Serialize for ServerEvent {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Key(key) => serializer.serialize_newtype_variant("ServerEvent", 0, "Key", key),
            Self::Detach => serializer.serialize_unit_variant("ServerEvent", 2, "Detach"),
            Self::Resize(w, h) => {
                let mut tv = serializer.serialize_tuple_variant("ServerEvent", 1, "Resize", 2)?;
                tv.serialize_field(w)?;
                tv.serialize_field(h)?;
                tv.end()
            }

            Self::Frame(frame) => {
                serializer.serialize_newtype_variant("ServerEvent", 3, "Frame", frame)
            }
        }
    }
}

pub fn decode_message<T: DeserializeOwned, R: Read>(reader: &mut R) -> Result<T, ProtocolError> {
    let mut prefix = [0_u8; 4];
    reader.read_exact(&mut prefix).map_err(ProtocolError::Io)?;

    let len = u32::from_be_bytes(prefix) as usize;

    if len > MAX_MESSAGE_LEN {
        return Err(ProtocolError::MessageTooLarge {
            len,
            max: MAX_MESSAGE_LEN,
        });
    }

    let mut payload = vec![0; len];
    reader.read_exact(&mut payload).map_err(ProtocolError::Io)?;

    let envelope: DecodeEnvelope<T> =
        serde_json::from_slice(&payload).map_err(ProtocolError::Json)?;

    if envelope.version != PROTOCOL_VERSION {
        return Err(ProtocolError::UnsupportedVersion(envelope.version));
    }

    Ok(envelope.message)
}

#[cfg(test)]
mod tests {
    use std::io::{self, Cursor, Read};

    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
    use ratatui::style::{Color, Modifier, Style};

    use super::{
        decode_message, encode_message, length_prefix, Cell, ClientEvent, FramePacket, PaneFrame,
        ProtocolError, ServerEvent, MAX_MESSAGE_LEN,
    };

    struct OneByteReader<R> {
        inner: R,
    }

    impl<R: Read> Read for OneByteReader<R> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if buf.is_empty() {
                return Ok(0);
            }
            self.inner.read(&mut buf[..1])
        }
    }

    #[test]
    fn key_message_round_trips() {
        let key = KeyEvent {
            code: KeyCode::Char('a'),
            modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        };
        let encoded = encode_message(&ClientEvent::Key(key)).unwrap();

        let decoded: ClientEvent = decode_message(&mut Cursor::new(encoded)).unwrap();

        let ClientEvent::Key(decoded) = decoded else {
            panic!("expected a key message");
        };
        assert_eq!(decoded.code, key.code);
        assert_eq!(decoded.modifiers, key.modifiers);
        assert_eq!(decoded.kind, key.kind);
        assert_eq!(decoded.state, key.state);
    }

    #[test]
    fn frame_message_round_trips() {
        let frame = FramePacket {
            rows: 1,
            cols: 2,
            tab_bar: Vec::new(),
            active_tab: 0,
            panes: vec![PaneFrame {
                id: 7,
                title: "shell".to_string(),
                exited: false,
                cursor: Some((1, 0)),
                cells: vec![
                    Cell {
                        row: 0,
                        col: 0,
                        symbol: "h".to_string(),
                        style: Style::new().fg(Color::Red).add_modifier(Modifier::BOLD),
                    },
                    Cell {
                        row: 0,
                        col: 1,
                        symbol: "i".to_string(),
                        style: Style::default(),
                    },
                ],
            }],
        };
        let encoded = encode_message(&ServerEvent::Frame(frame.clone())).unwrap();

        let decoded: ServerEvent = decode_message(&mut Cursor::new(encoded)).unwrap();

        assert_eq!(decoded, ServerEvent::Frame(frame));
    }

    #[test]
    fn length_prefix_is_big_endian() {
        assert_eq!(
            length_prefix(b"abc"),
            vec![0x00, 0x00, 0x00, 0x03, 0x61, 0x62, 0x63]
        );
    }

    #[test]
    fn fragmented_message_still_decodes() {
        let encoded = encode_message(&ClientEvent::Resize(120, 40)).unwrap();
        let mut reader = OneByteReader {
            inner: Cursor::new(encoded),
        };

        let decoded: ClientEvent = decode_message(&mut reader).unwrap();

        assert_eq!(decoded, ClientEvent::Resize(120, 40));
    }

    #[test]
    fn consecutive_messages_decode_separately() {
        let mut encoded = encode_message(&ClientEvent::Detach).unwrap();
        encoded.extend(encode_message(&ClientEvent::Resize(80, 24)).unwrap());
        let mut reader = Cursor::new(encoded);

        let first: ClientEvent = decode_message(&mut reader).unwrap();
        let second: ClientEvent = decode_message(&mut reader).unwrap();

        assert_eq!(first, ClientEvent::Detach);
        assert_eq!(second, ClientEvent::Resize(80, 24));
    }

    #[test]
    fn truncated_message_returns_error() {
        let mut encoded = (5_u32).to_be_bytes().to_vec();
        encoded.extend_from_slice(b"ab");

        let decoded: Result<ClientEvent, _> = decode_message(&mut Cursor::new(encoded));

        assert!(decoded.is_err());
    }

    #[test]
    fn malformed_json_returns_error() {
        let payload = b"not json";
        let mut encoded = (payload.len() as u32).to_be_bytes().to_vec();
        encoded.extend_from_slice(payload);

        let decoded: Result<ClientEvent, _> = decode_message(&mut Cursor::new(encoded));

        assert!(decoded.is_err());
    }

    #[test]
    fn oversized_length_is_rejected() {
        let encoded = ((MAX_MESSAGE_LEN as u32) + 1).to_be_bytes();

        let decoded: Result<ClientEvent, ProtocolError> = decode_message(&mut Cursor::new(encoded));

        assert!(
            matches!(decoded, Err(ProtocolError::MessageTooLarge { .. })),
            "expected an excessive-length error before reading the payload, got {decoded:?}"
        );
    }

    #[test]
    fn unsupported_version_is_rejected() {
        let mut encoded = encode_message(&ClientEvent::Detach).unwrap();
        let version = br#""version":1"#;
        let position = encoded
            .windows(version.len())
            .position(|window| window == version)
            .expect("encoded message should declare protocol version 1");
        encoded[position + version.len() - 1] = b'2';

        let decoded: Result<ClientEvent, ProtocolError> = decode_message(&mut Cursor::new(encoded));

        assert!(
            matches!(decoded, Err(ProtocolError::UnsupportedVersion(2))),
            "expected protocol version 2 to be rejected, got {decoded:?}"
        );
    }
}
