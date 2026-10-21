use serde::Deserialize;
use std::path::PathBuf;

#[derive(Clone, Default, Deserialize)]
pub struct TerminalConfig {
    pub shell: Option<PathBuf>,
    pub working_dir: Option<PathBuf>,
    pub env: std::collections::HashMap<String, String>,
}

impl TerminalConfig {
    pub fn shell(&self) -> PathBuf {
        self.shell
            .clone()
            .or_else(|| std::env::var("SHELL").ok().map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("/bin/bash"))
    }

    pub fn working_dir(&self) -> PathBuf {
        self.working_dir
            .clone()
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."))
    }

    pub fn env(&self) -> Vec<(String, String)> {
        self.env
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// Layer a saved pane's shell and working directory onto this config.
    /// Overrides that no longer exist on disk are dropped, so a stale
    /// session restores with this config's values instead of failing to
    /// spawn — and, deeper in the fallback chain, `$SHELL` then
    /// `/bin/bash` (see [`TerminalConfig::shell`]). Baking the fallbacks
    /// in here, at restore time, keeps dead paths out of saved files.
    pub fn with_overrides(&self, shell: Option<&PathBuf>, working_dir: Option<&PathBuf>) -> Self {
        Self {
            shell: live_path(shell).or_else(|| self.shell.clone()),
            working_dir: live_path(working_dir).or_else(|| self.working_dir.clone()),
            env: self.env.clone(),
        }
    }
}

/// The saved path if it still exists on disk; a dead path restores as unset.
fn live_path(path: Option<&PathBuf>) -> Option<PathBuf> {
    path.filter(|p| p.exists()).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn with_overrides_uses_live_paths_and_drops_dead_ones() {
        let base = TerminalConfig {
            shell: Some(PathBuf::from("/bin/sh")),
            working_dir: None,
            env: HashMap::from([("TERM".to_string(), "xterm-256color".to_string())]),
        };

        // Dead saved paths fall back to the base config's values.
        let merged = base.with_overrides(
            Some(&PathBuf::from("/definitely/not/a/shell")),
            Some(&PathBuf::from("/definitely/not/a/dir")),
        );
        assert_eq!(merged.shell, Some(PathBuf::from("/bin/sh")));
        assert_eq!(merged.working_dir, None);

        // Live saved paths win over the base config's values.
        let cwd = std::env::current_dir().unwrap();
        let merged = base.with_overrides(Some(&PathBuf::from("/bin/bash")), Some(&cwd));
        assert_eq!(merged.shell, Some(PathBuf::from("/bin/bash")));
        assert_eq!(merged.working_dir, Some(cwd));
        // The base env travels with the merged config untouched.
        assert_eq!(
            merged.env.get("TERM").map(String::as_str),
            Some("xterm-256color")
        );
    }
}
