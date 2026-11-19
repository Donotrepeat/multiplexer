// The auth handshake is only wired into the running server in later M2 tasks
// (ServerHost startup, client manager); until then the binary legitimately
// never touches it.
#![allow(dead_code)]

use crate::app::server::protocol::ProtocolError;
use crate::app::server::protocol::decode_message;
use crate::app::server::protocol::send_message;
use anyhow::Result;
use hmac::Hmac;
use hmac::KeyInit;
use hmac::Mac;
use rand::{TryRng, rngs::SysRng};
use serde::{Deserialize, Serialize};
use sha2;
use std::fmt;
use std::fmt::Write as format_write;
use std::fs;
use std::io::Read;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
#[derive(Debug)]
pub enum AuthError {
    Rejected(String),
    TokenFile(std::io::Error),
    Protocol(ProtocolError),
}

impl fmt::Display for AuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rejected(reason) => write!(f, "authentication rejected: {reason}"),
            Self::TokenFile(e) => write!(f, "could not read token file: {e}"),
            Self::Protocol(e) => write!(f, "handshake protocol error: {e}"),
        }
    }
}

impl std::error::Error for AuthError {}

impl From<ProtocolError> for AuthError {
    fn from(e: ProtocolError) -> Self {
        Self::Protocol(e)
    }
}

#[derive(Serialize, Deserialize)]
struct Challenge {
    session: String,
    nonce_hex: [u8; 32],
} // server → client
#[derive(Serialize, Deserialize)]
struct AuthResponse {
    response_hex: String,
} // client → server
#[derive(Serialize, Deserialize)]
enum AuthOutcome {
    Ok,
    Error(String),
} // server → client
//
pub fn authenticate<R: Read, W: Write>(
    r: &mut R,
    w: &mut W,
    session: &str,
    token: &str,
) -> Result<(), AuthError> {
    let nonce = random_bytes();
    let msg = Challenge {
        session: session.to_owned(),
        nonce_hex: nonce,
    };
    send_message(w, &msg)?;
    let response: AuthResponse = decode_message(r)?;
    let Some(given) = from_hex(&response.response_hex) else {
        send_message(w, &AuthOutcome::Error("malformed handshake".into()))?;
        return Err(AuthError::Rejected(String::from("malformed handshake")));
    };
    if given.len() != 32 {
        send_message(w, &AuthOutcome::Error("malformed response".into()))?;
        return Err(AuthError::Rejected("malformed response".into()));
    }
    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(token.as_bytes())
        .expect("HMAC-SHA256 accepts any key length");
    mac.update(&nonce);

    match mac.verify_slice(&given) {
        Ok(()) => {
            send_message(w, &AuthOutcome::Ok)?;
            Ok(())
        }
        Err(_) => {
            let reason = "authentication failed";
            send_message(w, &AuthOutcome::Error(reason.into()))?;
            Err(AuthError::Rejected(reason.into()))
        }
    }
}

pub fn respond<R: Read, W: Write>(r: &mut R, w: &mut W, token: &str) -> Result<(), AuthError> {
    //read Challenge
    let response: Challenge = decode_message(r)?;

    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(token.as_bytes())
        .expect("HMAC-SHA256 accepts any key length");
    mac.update(&response.nonce_hex);
    let out = mac.finalize();
    let tag: [u8; 32] = (*out.as_bytes()).into();
    send_message(
        w,
        &AuthResponse {
            response_hex: to_hex(tag),
        },
    )?;
    //read AuthOutcome
    let verif: AuthOutcome = decode_message(r)?;
    match verif {
        AuthOutcome::Ok => Ok(()),
        AuthOutcome::Error(e) => Err(AuthError::Rejected(e)),
    }
}

pub(super) fn tokens_dir() -> Option<PathBuf> {
    dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .map(|base| base.join("multiplexer/tokens"))
}

fn token_path(session: &str) -> Option<PathBuf> {
    tokens_dir().map(|dir| dir.join(format!("{session}.token")))
}

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut buf = [0u8; N];
    SysRng.try_fill_bytes(&mut buf).unwrap();
    buf
}
fn to_hex(bytes: [u8; 32]) -> String {
    let mut token = String::with_capacity(bytes.len() * 2);
    for &b in &bytes {
        write!(&mut token, "{b:02x}").unwrap();
    }
    token
}
fn from_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

fn generate_token() -> String {
    let key: [u8; 32] = random_bytes();
    to_hex(key)
}
fn rotate_token_in(dir: &PathBuf, session: &str) -> Result<String> {
    let token = generate_token();
    fs::create_dir_all(dir)?; // ← creates the tokens dir
    let path = dir.join(format!("{session}.token"));
    fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600) // unix: owner read/write only
        .open(&path)?
        .write_all(token.as_bytes())?; // ← creates/overwrites the file
    Ok(token)
}
pub fn rotate_token(session: &str) {
    let dir = tokens_dir().unwrap_or_default();
    let _new_token = rotate_token_in(&dir, session);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Duration;

    /// Removes the directory on drop so tests leave no state behind.
    struct TestDir(PathBuf);

    impl TestDir {
        fn new(name: &str) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let unique = NEXT.fetch_add(1, Ordering::Relaxed);
            Self(std::env::temp_dir().join(format!(
                "multiplexer-auth-{name}-{}-{unique}",
                std::process::id()
            )))
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn test_token_generation() {
        let token = generate_token();
        assert_eq!(token.len(), 64);
        let token_2 = generate_token();
        assert_ne!(token, token_2);
    }

    #[test]
    fn rotate_token_in_writes_private_token_and_rotates() {
        let dir = TestDir::new("rotate");
        let path = dir.0.join("demo.token");

        let first = rotate_token_in(&dir.0, "demo").unwrap();

        assert!(path.is_file());
        assert_eq!(fs::read_to_string(&path).unwrap(), first);
        assert_eq!(first.len(), 64);
        assert!(
            first
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "token must be 64 lowercase hex characters, got {first}"
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );

        let second = rotate_token_in(&dir.0, "demo").unwrap();

        assert_ne!(first, second, "rotation must produce a new token");
        assert_eq!(fs::read_to_string(&path).unwrap(), second);
    }

    #[test]
    fn hmac_matches_rfc_4231_case_2() {
        let mut mac = Hmac::<sha2::Sha256>::new_from_slice(b"Jefe").unwrap();
        mac.update(b"what do ya want for nothing?");
        let out = mac.finalize();
        let tag: [u8; 32] = (*out.as_bytes()).into();

        assert_eq!(
            to_hex(tag),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    // Minimal in-memory duplex: each endpoint reads one buffer and writes the
    // other. Reads block (by polling) while the peer has not written yet.
    struct PipeReader(Arc<Mutex<Vec<u8>>>);

    struct PipeWriter(Arc<Mutex<Vec<u8>>>);

    impl std::io::Read for PipeReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if buf.is_empty() {
                return Ok(0);
            }
            loop {
                let mut incoming = self.0.lock().unwrap();
                if !incoming.is_empty() {
                    let n = buf.len().min(incoming.len());
                    buf[..n].copy_from_slice(&incoming[..n]);
                    incoming.drain(..n);
                    return Ok(n);
                }
                drop(incoming);
                thread::sleep(Duration::from_millis(1));
            }
        }
    }

    impl std::io::Write for PipeWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    type Endpoint = (PipeReader, PipeWriter);

    fn duplex() -> (Endpoint, Endpoint) {
        let server_to_client = Arc::new(Mutex::new(Vec::new()));
        let client_to_server = Arc::new(Mutex::new(Vec::new()));
        (
            (
                PipeReader(client_to_server.clone()),
                PipeWriter(server_to_client.clone()),
            ),
            (PipeReader(server_to_client), PipeWriter(client_to_server)),
        )
    }

    #[test]
    fn handshake_succeeds_with_matching_token() {
        let ((mut server_r, mut server_w), (mut client_r, mut client_w)) = duplex();
        let token = generate_token();
        let server_token = token.clone();
        let server = thread::spawn(move || {
            authenticate(&mut server_r, &mut server_w, "session", &server_token)
        });

        let client_result = respond(&mut client_r, &mut client_w, &token);

        assert!(
            client_result.is_ok(),
            "client handshake failed: {client_result:?}"
        );
        let server_result = server.join().expect("server thread panicked");
        assert!(
            server_result.is_ok(),
            "server handshake failed: {server_result:?}"
        );
    }

    #[test]
    fn handshake_rejects_wrong_token() {
        let ((mut server_r, mut server_w), (mut client_r, mut client_w)) = duplex();
        let server = thread::spawn(move || {
            authenticate(&mut server_r, &mut server_w, "session", "right-token")
        });

        let client_result = respond(&mut client_r, &mut client_w, "wrong-token");

        // `respond` only returns `Rejected` after decoding `AuthOutcome::Error`.
        assert!(
            matches!(
                &client_result,
                Err(AuthError::Rejected(reason)) if reason.as_str() == "authentication failed"
            ),
            "client should receive AuthOutcome::Error, got {client_result:?}"
        );
        let server_result = server.join().expect("server thread panicked");
        assert!(
            matches!(
                &server_result,
                Err(AuthError::Rejected(reason)) if reason.as_str() == "authentication failed"
            ),
            "authenticate should reject the wrong token, got {server_result:?}"
        );
    }

    #[test]
    fn non_json_response_is_a_protocol_error() {
        let ((mut server_r, mut server_w), (_client_r, mut client_w)) = duplex();
        let payload = b"not json";
        let mut framed = (payload.len() as u32).to_be_bytes().to_vec();
        framed.extend_from_slice(payload);
        client_w.write_all(&framed).unwrap();

        let result = authenticate(&mut server_r, &mut server_w, "session", "token");

        assert!(
            matches!(&result, Err(AuthError::Protocol(ProtocolError::Json(_)))),
            "expected the server to report a JSON protocol error, got {result:?}"
        );
    }
}
