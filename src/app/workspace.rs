use crate::App;
use crate::app::workspace::session::session_path;
use anyhow::Result;
use serde::{Deserialize, Serialize};

pub mod layout;
pub mod session;

pub use layout::{PaneLayout, SpawnContext, TabLayout};
pub use session::{list_sessions, next_in_cycle, valid_session_name};

/// The workspace auto-saved on exit and restored on startup.
pub const DEFAULT_WORKSPACE: &str = "default";

#[derive(Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Workspace {
    pub active_tab: usize,
    pub tabs: Vec<TabLayout>,
}

impl Workspace {
    pub fn load(name: &str) -> Result<Self> {
        let Some(path) = session_path(name) else {
            return Ok(Workspace::default());
        };
        Self::load_path(&path)
    }

    fn load_path(path: &std::path::Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Workspace::default());
        }

        let contents = std::fs::read_to_string(path)?;
        let file_config: Workspace = toml::from_str(&contents)?;
        Ok(file_config)
    }

    pub fn save(name: &str, workspace: &Self) -> Result<()> {
        let Some(path) = session_path(name) else {
            anyhow::bail!("no state or data directory to save workspace {name} into");
        };
        Self::save_path(&path, workspace)
    }

    fn save_path(path: &std::path::Path, workspace: &Self) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, toml::to_string_pretty(workspace)?)?;
        Ok(())
    }

    /// Capture the current app state. Cheap; does not touch PTY internals.
    pub fn from_app(app: &App) -> Self {
        let tabs = app.tabs.iter().map(TabLayout::save_tab).collect();
        Workspace {
            active_tab: app.active_tab,
            tabs,
        }
    }

    /// Apply a workspace to an empty app, spawning panes as configured.
    /// Saved tabs without panes are skipped, and the active indices are
    /// clamped, so a hand-edited or stale file cannot leave the app
    /// pointing at a tab or pane that does not exist. The id counter is
    /// synced back so later panes never reuse a restored pane's id.
    pub fn apply(self, app: &mut App, spawn: &mut SpawnContext) -> Result<()> {
        let tabs = self
            .tabs
            .iter()
            .filter(|tab| !tab.panes.is_empty())
            .map(|tab| tab.to_tab(spawn))
            .collect::<Result<Vec<_>>>()?;
        app.tabs = tabs;
        app.active_tab = self.active_tab.min(app.tabs.len().saturating_sub(1));
        app.set_next_pane_id(spawn.next_id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tabs::Grid;
    use std::path::PathBuf;

    #[test]
    fn workspace_round_trips_through_toml() {
        let workspace = Workspace {
            active_tab: 1,
            tabs: vec![
                TabLayout::default(),
                TabLayout {
                    grid: Grid::Vertical,
                    active_pane: 1,
                    panes: vec![
                        PaneLayout::default(),
                        PaneLayout {
                            shell: Some(PathBuf::from("/bin/zsh")),
                            working_dir: Some(PathBuf::from("/tmp")),
                        },
                    ],
                },
            ],
        };

        let text = toml::to_string_pretty(&workspace).unwrap();
        let back: Workspace = toml::from_str(&text).unwrap();

        assert_eq!(back.active_tab, 1);
        assert_eq!(back.tabs.len(), 2);
        assert_eq!(back.tabs[1].grid, Grid::Vertical);
        assert_eq!(back.tabs[1].active_pane, 1);
        assert_eq!(back.tabs[1].panes[1].shell, Some(PathBuf::from("/bin/zsh")));
        assert_eq!(
            back.tabs[1].panes[1].working_dir,
            Some(PathBuf::from("/tmp"))
        );
    }

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
    fn save_then_load_round_trips_a_named_session() -> Result<()> {
        let temp = TempDir::new("named-round-trip");
        let path = temp.0.join("foo.toml");
        let workspace = Workspace {
            active_tab: 0,
            tabs: vec![TabLayout {
                grid: Grid::Vertical,
                active_pane: 0,
                panes: vec![PaneLayout {
                    shell: Some(PathBuf::from("/bin/zsh")),
                    working_dir: Some(PathBuf::from("/tmp")),
                }],
            }],
        };

        Workspace::save_path(&path, &workspace)?;
        let back = Workspace::load_path(&path)?;

        assert_eq!(back.tabs.len(), 1);
        assert_eq!(back.tabs[0].grid, Grid::Vertical);
        assert_eq!(back.tabs[0].panes[0].shell, Some(PathBuf::from("/bin/zsh")));
        Ok(())
    }

    #[test]
    fn load_of_a_missing_file_is_an_empty_workspace() -> Result<()> {
        let temp = TempDir::new("missing");
        let back = Workspace::load_path(&temp.0.join("nope.toml"))?;
        assert!(back.tabs.is_empty());
        Ok(())
    }
}
