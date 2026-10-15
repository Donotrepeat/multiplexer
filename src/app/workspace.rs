use crate::app::config::TerminalConfig;
use crate::app::events::{PaneEvent, PaneId};
use crate::app::pane::Pane;
use crate::app::tabs::{Grid, Tab};
use crate::App;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::mpsc::Sender;

/// The workspace auto-saved on exit and restored on startup.
pub const DEFAULT_WORKSPACE: &str = "default";

#[derive(Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Workspace {
    pub active_tab: usize,
    pub tabs: Vec<TabLayout>,
}

#[derive(Default, Deserialize, Serialize)]
#[serde(default)]
pub struct TabLayout {
    pub grid: Grid,
    pub active_pane: usize,
    pub panes: Vec<PaneLayout>,
}

impl TabLayout {
    pub fn save_tab(tab: &Tab) -> Self {
        let active = tab.active;
        let panes = tab.panes.iter().map(|pane| pane.get_shell_info()).collect();

        TabLayout {
            grid: tab.grid,
            active_pane: active,
            panes,
        }
    }

    /// Spawn the tab this layout describes: one pane per saved pane. The
    /// saved active index is clamped, and a tab without panes is refused,
    /// because the app cannot display an empty tab.
    pub fn to_tab(&self, spawn: &mut SpawnContext) -> Result<Tab> {
        if self.panes.is_empty() {
            anyhow::bail!("a saved tab must have at least one pane");
        }
        let panes = self
            .panes
            .iter()
            .map(|pane| pane.to_pane(spawn))
            .collect::<Result<Vec<_>>>()?;
        Ok(Tab {
            active: self.active_pane.min(panes.len() - 1),
            panes,
            grid: self.grid,
        })
    }
}

#[derive(Default, Deserialize, Serialize)]
#[serde(default)]
pub struct PaneLayout {
    pub shell: Option<PathBuf>,
    pub working_dir: Option<PathBuf>,
}

impl PaneLayout {
    /// Spawn the pane this layout describes. The saved shell and working
    /// directory are validated as they are layered onto the base terminal
    /// config (see [`TerminalConfig::with_overrides`]); a path that has
    /// died since the session was saved falls back to the config's value
    /// instead of failing the spawn.
    pub fn to_pane(&self, spawn: &mut SpawnContext) -> Result<Pane> {
        let terminal = spawn
            .terminal
            .with_overrides(self.shell.as_ref(), self.working_dir.as_ref());
        Pane::new(
            spawn.rows,
            spawn.cols,
            spawn.next_pane_id(),
            spawn.tx.clone(),
            &terminal,
        )
    }
}

/// Everything needed to spawn panes while restoring a workspace: the
/// initial pane size, the event channel, a source of fresh pane ids, and
/// the base terminal config that each saved pane's overrides are layered
/// onto.
pub struct SpawnContext {
    rows: u16,
    cols: u16,
    tx: Sender<PaneEvent>,
    next_id: u64,
    terminal: TerminalConfig,
}

impl SpawnContext {
    pub fn new(
        rows: u16,
        cols: u16,
        tx: Sender<PaneEvent>,
        next_id: u64,
        terminal: &TerminalConfig,
    ) -> Self {
        Self {
            rows,
            cols,
            tx,
            next_id,
            terminal: terminal.clone(),
        }
    }

    fn next_pane_id(&mut self) -> PaneId {
        let id = PaneId(self.next_id);
        self.next_id += 1;
        id
    }
}

/// `$XDG_STATE_HOME/multiplexer/sessions`, or the local data dir when no
/// state dir is available.
fn sessions_dir() -> Option<PathBuf> {
    dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .map(|base| base.join("multiplexer/sessions"))
}

/// The file a named workspace is stored in, when a state dir exists.
fn session_path(name: &str) -> Option<PathBuf> {
    sessions_dir().map(|dir| dir.join(format!("{name}.toml")))
}

impl Workspace {
    pub fn load(name: &str) -> Result<Self> {
        let Some(path) = session_path(name) else {
            return Ok(Workspace::default());
        };
        if !path.exists() {
            return Ok(Workspace::default());
        }

        let contents = std::fs::read_to_string(&path)?;
        let file_config: Workspace = toml::from_str(&contents)?;
        Ok(file_config)
    }

    pub fn save(name: &str, workspace: &Self) -> Result<()> {
        let Some(path) = session_path(name) else {
            anyhow::bail!("no state or data directory to save workspace {name} into");
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, toml::to_string_pretty(workspace)?)?;
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
    use std::sync::mpsc;

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

    #[test]
    fn to_tab_spawns_one_pane_per_layout_and_clamps_active() -> Result<()> {
        let layout = TabLayout {
            active_pane: 9,
            panes: vec![PaneLayout::default(), PaneLayout::default()],
            grid: Grid::Horizontal,
        };
        let (tx, _rx) = mpsc::channel();
        let mut spawn = SpawnContext::new(4, 20, tx, 0, &TerminalConfig::default());

        let tab = layout.to_tab(&mut spawn)?;

        assert_eq!(tab.panes.len(), 2);
        // A stale active index out of range restores onto the last pane.
        assert_eq!(tab.active, 1);
        Ok(())
    }

    #[test]
    fn spawned_panes_take_fresh_ids_from_the_spawn_context() -> Result<()> {
        let layout = TabLayout {
            panes: vec![
                PaneLayout::default(),
                PaneLayout::default(),
                PaneLayout::default(),
            ],
            ..TabLayout::default()
        };
        let (tx, _rx) = mpsc::channel();
        let mut spawn = SpawnContext::new(4, 20, tx, 5, &TerminalConfig::default());

        let tab = layout.to_tab(&mut spawn)?;

        assert_eq!(tab.panes[0].id(), PaneId(5));
        assert_eq!(tab.panes[1].id(), PaneId(6));
        assert_eq!(tab.panes[2].id(), PaneId(7));
        assert_eq!(spawn.next_id, 8);
        Ok(())
    }

    #[test]
    fn to_tab_refuses_a_saved_tab_without_panes() {
        let (tx, _rx) = mpsc::channel();
        let mut spawn = SpawnContext::new(4, 20, tx, 0, &TerminalConfig::default());
        assert!(TabLayout::default().to_tab(&mut spawn).is_err());
    }
}
