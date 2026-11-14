use rand::{rngs::SysRng, TryRng};
use std::fmt::Write as format_write;
use std::fs;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
pub(super) fn tokens_dir() -> Option<PathBuf> {
    dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .map(|base| base.join("multiplexer/tokens"))
}

fn token_path(session: &str) -> Option<PathBuf> {
    tokens_dir().map(|dir| dir.join(format!("{session}.token")))
}

fn generate_token() -> String {
    let mut key = [0u8; 32];
    let _ = SysRng.try_fill_bytes(&mut key);
    let mut token = String::with_capacity(key.len() * 2);
    for &b in &key {
        write!(&mut token, "{b:02x}").unwrap();
    }
    token
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
// pub fn rotate_token(session){}

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
