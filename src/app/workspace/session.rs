use std::path::PathBuf;

/// `$XDG_STATE_HOME/multiplexer/sessions`, or the local data dir when no
/// state dir is available.
pub(super) fn sessions_dir() -> Option<PathBuf> {
    dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .map(|base| base.join("multiplexer/sessions"))
}

/// The file a named workspace is stored in, when a state dir exists.
pub(super) fn session_path(name: &str) -> Option<PathBuf> {
    sessions_dir().map(|dir| dir.join(format!("{name}.toml")))
}

/// A session name becomes a file name, so it must not be able to escape the
/// sessions directory: no separators, no leading dot (which also rules out
/// `..`), and nothing outside a conservative ASCII set.
pub fn valid_session_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

/// All saved session names, sorted, for cycling with Next/PrevSession.
pub fn list_sessions() -> Vec<String> {
    let Some(dir) = sessions_dir() else {
        return Vec::new();
    };
    list_sessions_in(&dir)
}

pub(super) fn list_sessions_in(dir: &std::path::Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension()? != "toml" {
                return None;
            }
            path.file_stem()?.to_str().map(str::to_owned)
        })
        .collect();
    names.sort();
    names
}

/// The name `delta` steps away from `current` in a sorted session list,
/// wrapping around both ends. A `current` that is not on disk (never saved)
/// counts as just before the list when moving forward, just past it when
/// moving back. `None` when the list is empty or holds only `current`.
pub fn next_in_cycle(sessions: &[String], current: &str, delta: isize) -> Option<String> {
    if sessions.is_empty() {
        return None;
    }
    let len = sessions.len() as isize;
    let next = match sessions.iter().position(|s| s == current) {
        Some(i) => (i as isize + delta).rem_euclid(len) as usize,
        None if delta >= 0 => 0,
        None => sessions.len() - 1,
    };
    if sessions[next] == current {
        return None;
    }
    Some(sessions[next].clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unique directory under the OS temp dir, removed on drop, so save/
    /// load tests never touch the real XDG state directory.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(test: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "multiplexer-test-{test}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn list_sessions_in_sorts_names_and_ignores_non_toml_files() -> Result<(), std::io::Error> {
        let temp = TempDir::new("list");
        std::fs::create_dir_all(&temp.0)?;
        std::fs::write(temp.0.join("work.toml"), "")?;
        std::fs::write(temp.0.join("default.toml"), "")?;
        std::fs::write(temp.0.join("notes.txt"), "")?;
        std::fs::write(temp.0.join(".hidden.toml"), "")?;

        assert_eq!(
            list_sessions_in(&temp.0),
            vec![
                ".hidden".to_string(),
                "default".to_string(),
                "work".to_string()
            ]
        );
        Ok(())
    }

    #[test]
    fn valid_session_name_rejects_paths_and_empty_names() {
        assert!(valid_session_name("default"));
        assert!(valid_session_name("work-2.x_1"));
        assert!(!valid_session_name(""));
        assert!(!valid_session_name(".."));
        assert!(!valid_session_name(".hidden"));
        assert!(!valid_session_name("a/b"));
        assert!(!valid_session_name("a\\b"));
        assert!(!valid_session_name("a b"));
    }

    #[test]
    fn next_in_cycle_wraps_around_both_ends() {
        let sessions: Vec<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();
        assert_eq!(next_in_cycle(&sessions, "a", 1), Some("b".to_string()));
        assert_eq!(next_in_cycle(&sessions, "c", 1), Some("a".to_string()));
        assert_eq!(next_in_cycle(&sessions, "a", -1), Some("c".to_string()));
        // An unsaved current session enters at the nearest end.
        assert_eq!(
            next_in_cycle(&sessions, "unsaved", 1),
            Some("a".to_string())
        );
        assert_eq!(
            next_in_cycle(&sessions, "unsaved", -1),
            Some("c".to_string())
        );
        // Nothing to switch to: empty list, or only the current session.
        assert_eq!(next_in_cycle(&[], "a", 1), None);
        assert_eq!(next_in_cycle(&sessions[..1], "a", 1), None);
    }
}
