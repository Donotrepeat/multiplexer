use crate::app::pane::session::PtySession;
use std::path::PathBuf;

impl PtySession {
    /// The child's current working directory, probed from the kernel the
    /// same way tmux reads `pane_current_path`. Any failure — the child
    /// gone, no `/proc` — falls back to the directory it was spawned in.
    /// Must not be called after the child has been reaped: its pid is
    /// then free for reuse and could name an unrelated process.
    pub fn live_cwd(&self) -> PathBuf {
        match self.probe_cwd() {
            Some(cwd) => cwd,
            None => self.spawn_dir.clone(),
        }
    }

    /// `/proc/{pid}/cwd`, readable while the child owns its pid. Linux
    /// only; other platforms have no `/proc` and always fall back.
    #[cfg(target_os = "linux")]
    fn probe_cwd(&self) -> Option<PathBuf> {
        std::fs::read_link(format!("/proc/{}/cwd", self.pid)).ok()
    }

    #[cfg(not(target_os = "linux"))]
    fn probe_cwd(&self) -> Option<PathBuf> {
        None
    }
}
