// The path helpers are wired into ServerHost in a later M2 task; until then
// the binary legitimately never touches some of them.
#![allow(dead_code)]

use crate::app::workspace::valid_session_name;
use std::fmt;
use std::fs;
use std::io;
use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// The TCP port a server listens on by default.
pub const DEFAULT_TCP_PORT: u16 = 7345;

#[derive(Debug)]
pub enum PathError {
    InvalidSession(String),
    NoBaseDir,
    Io(io::Error),
}

impl fmt::Display for PathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSession(session) => write!(f, "invalid session name: {session}"),
            Self::NoBaseDir => write!(f, "no state or data directory available"),
            Self::Io(error) => write!(f, "filesystem error: {error}"),
        }
    }
}

impl std::error::Error for PathError {}

impl From<io::Error> for PathError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

fn validate_session(session: &str) -> Result<(), PathError> {
    if valid_session_name(session) {
        Ok(())
    } else {
        Err(PathError::InvalidSession(session.to_string()))
    }
}

/// `$XDG_STATE_HOME` or the local data dir, when either is available.
pub fn state_base_dir() -> Option<PathBuf> {
    dirs::state_dir().or_else(dirs::data_local_dir)
}

pub fn tokens_dir() -> Option<PathBuf> {
    state_base_dir().map(|base| base.join("multiplexer/tokens"))
}

/// The token file for `session`.
pub fn token_path(session: &str) -> Result<PathBuf, PathError> {
    let dir = tokens_dir().ok_or(PathError::NoBaseDir)?;
    token_path_in(&dir, session)
}

/// The token file for `session` inside an explicit token directory. Session
/// names become file names, so reject anything that could escape `dir`.
pub(crate) fn token_path_in(dir: &Path, session: &str) -> Result<PathBuf, PathError> {
    validate_session(session)?;
    Ok(dir.join(format!("{session}.token")))
}

/// Pick the socket directory from an explicit runtime and state directory.
/// Kept separate from [`socket_dir`] so it can be tested without mutating the
/// process environment.
fn socket_dir_from(runtime: Option<&Path>, state: Option<&Path>) -> Option<PathBuf> {
    runtime
        .map(|dir| dir.join("multiplexer"))
        .or_else(|| state.map(|base| base.join("multiplexer/sockets")))
}

/// The directory holding this server's Unix socket. Prefer the per-user
/// runtime directory, falling back to the state directory when it is absent.
pub fn socket_dir() -> Option<PathBuf> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    let state = state_base_dir();
    socket_dir_from(runtime.as_deref(), state.as_deref())
}

/// The Unix socket path for `session`.
pub fn socket_path(session: &str) -> Result<PathBuf, PathError> {
    let dir = socket_dir().ok_or(PathError::NoBaseDir)?;
    socket_path_in(&dir, session)
}

/// The Unix socket path for `session` inside an explicit socket directory.
pub(crate) fn socket_path_in(dir: &Path, session: &str) -> Result<PathBuf, PathError> {
    validate_session(session)?;
    Ok(dir.join(format!("{session}.sock")))
}

/// The loopback address a server listens on by default.
pub fn default_tcp_addr() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], DEFAULT_TCP_PORT))
}

/// Create `dir` and all parents, then force the leaf directory to be private
/// (`0700`). Tightening an existing multiplexer-owned directory is intentional:
/// the token and socket directories must not be readable by other users.
pub fn ensure_private_dir(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
}

/// Restrict a Unix socket to its owner (`0600`).
pub fn secure_socket(path: &Path) -> io::Result<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Removes the directory on drop so tests leave no state behind.
    struct TestDir(PathBuf);

    impl TestDir {
        fn new(name: &str) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let unique = NEXT.fetch_add(1, Ordering::Relaxed);
            Self(std::env::temp_dir().join(format!(
                "multiplexer-paths-{name}-{}-{unique}",
                std::process::id()
            )))
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn token_path_in_builds_the_session_file() {
        let dir = Path::new("/tmp/tokens");

        let path = token_path_in(dir, "work").unwrap();

        assert_eq!(path, dir.join("work.token"));
    }

    #[test]
    fn socket_path_in_builds_the_session_socket() {
        let dir = Path::new("/tmp/sockets");

        let path = socket_path_in(dir, "work").unwrap();

        assert_eq!(path, dir.join("work.sock"));
    }

    #[test]
    fn path_helpers_reject_unsafe_session_names() {
        let dir = Path::new("/tmp/state");

        for session in ["", "..", "../evil", "a/b", "a\\b", ".hidden"] {
            let token = token_path_in(dir, session);
            let socket = socket_path_in(dir, session);

            assert!(
                matches!(token, Err(PathError::InvalidSession(_))),
                "token path should reject {session:?}, got {token:?}"
            );
            assert!(
                matches!(socket, Err(PathError::InvalidSession(_))),
                "socket path should reject {session:?}, got {socket:?}"
            );
        }
    }

    #[test]
    fn socket_dir_prefers_the_runtime_directory() {
        let runtime = Path::new("/run/user/1000");
        let state = Path::new("/home/user/.local/state");

        assert_eq!(
            socket_dir_from(Some(runtime), Some(state)),
            Some(runtime.join("multiplexer"))
        );
    }

    #[test]
    fn socket_dir_falls_back_to_state_sockets() {
        let state = Path::new("/home/user/.local/state");

        assert_eq!(
            socket_dir_from(None, Some(state)),
            Some(state.join("multiplexer/sockets"))
        );
        assert_eq!(socket_dir_from(None, None), None);
    }

    #[test]
    fn default_tcp_addr_is_loopback_on_the_default_port() {
        let addr = default_tcp_addr();

        assert_eq!(addr.ip().to_string(), "127.0.0.1");
        assert_eq!(addr.port(), DEFAULT_TCP_PORT);
    }

    #[test]
    fn ensure_private_dir_creates_nested_private_directories() {
        let temp = TestDir::new("nested");
        let dir = temp.0.join("multiplexer/tokens");

        ensure_private_dir(&dir).unwrap();

        assert!(dir.is_dir());
        assert_eq!(mode(&dir), 0o700);
    }

    #[test]
    fn ensure_private_dir_tightens_an_existing_directory() {
        let temp = TestDir::new("tighten");
        let dir = temp.0.join("multiplexer");
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();

        ensure_private_dir(&dir).unwrap();

        assert_eq!(mode(&dir), 0o700);
    }

    #[test]
    fn secure_socket_sets_owner_only_permissions() {
        let temp = TestDir::new("socket-mode");
        fs::create_dir_all(&temp.0).unwrap();
        let path = temp.0.join("work.sock");
        fs::write(&path, b"").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o777)).unwrap();

        secure_socket(&path).unwrap();

        assert_eq!(mode(&path), 0o600);
    }
}
