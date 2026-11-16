use crate::app::server::protocol::decode_message;
use crate::app::server::protocol::send_message;
use anyhow::Result;
use hmac::Hmac;
use hmac::KeyInit;
use rand::{rngs::SysRng, TryRng};
use serde::{Deserialize, Serialize};
use sha2;
use std::fmt::Write as format_write;
use std::fs;
use std::io::Read;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
#[derive(Serialize, Deserialize)]
struct Challenge {
    session: String,
    nonce_hex: String,
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
    r: R,
    w: &mut W,
    session: &str,
    token: &str,
) -> Result<(), AuthError> {
    let nonce = generate_token();
    let msg = Challenge {
        session: session.to_owned(),
        nonce_hex: nonce,
    };
    send_message(w, &msg)?;
    let response: AuthResponse = decode_message(&mut r)?;
    let Some(given) = from_hex(&response.response_hex) else {
        send_message(w, &AuthOutcome::Error("malformed handshake".into()))?;
        return Err(AuthError::Rejected);
    };

    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(token.as_bytes())
        .expect("HMAC-SHA256 accepts any key length");
    mac.update(&nonce);

    let outcome = if mac.verify(&given).is_ok() {
        AuthOutcome::Ok
    } else {
        AuthOutcome::Error("authentication failed".into())
    };
    send_message(w, &outcome)?;
    Ok(())
}
//pub fn respond<R: Read, W: Write>(r: R, w: W, token: &str) -> Result<()> {}

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
    if s.len() % 2 != 0 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
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
    let dir = tokens_dir().unwrap_or(PathBuf::new());
    let new_token = rotate_token_in(&dir, session);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_generation() {
        let token = generate_token();
        assert_eq!(token.len(), 64);
        let token_2 = generate_token();
        assert_ne!(token, token_2);
    }
}
