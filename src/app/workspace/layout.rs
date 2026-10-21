use crate::app::config::TerminalConfig;
use crate::app::events::{PaneEvent, PaneId};
use crate::app::pane::Pane;
use crate::app::tabs::{Grid, Tab};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::mpsc::Sender;

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
    pub rows: u16,
    pub cols: u16,
    pub tx: Sender<PaneEvent>,
    pub next_id: u64,
    pub terminal: TerminalConfig,
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

    pub fn next_pane_id(&mut self) -> PaneId {
        let id = PaneId(self.next_id);
        self.next_id += 1;
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

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
