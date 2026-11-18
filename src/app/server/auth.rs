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

    let outcome = match mac.verify_slice(&given) {
        Ok(()) => AuthOutcome::Ok,
        Err(_) => AuthOutcome::Error("authentication failed".into()),
    };
    send_message(w, &outcome)?;
    Ok(())
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

    #[test]
    fn test_token_generation() {
        let token = generate_token();
        assert_eq!(token.len(), 64);
        let token_2 = generate_token();
        assert_ne!(token, token_2);
    }
}
