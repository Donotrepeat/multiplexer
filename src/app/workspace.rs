use crate::app::config::Config;
use crate::app::tabs::{Grid, Tab};
use crate::App;
use anyhow::Result;
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Default, Deserialize)]
#[serde(default)]
pub struct Workspace {
    pub active_tab: usize,
    pub tabs: Vec<TabLayout>,
}

#[derive(Default, Deserialize)]
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
}

#[derive(Default, Deserialize)]
#[serde(default)]
pub struct PaneLayout {
    pub shell: Option<PathBuf>,
    pub working_dir: Option<PathBuf>,
}

impl Workspace {
    pub fn load(name: &str) -> Result<Self> {
        let Some(config_dir) = dirs::data_local_dir() else {
            return Ok(Workspace::default());
        };
        let path = config_dir.join("multiplexer/").join(format!("{name}.toml"));

        if !path.exists() {
            return Ok(Workspace::default());
        }

        let contents = std::fs::read_to_string(&path)?;
        let file_config: Workspace = toml::from_str(&contents)?;
        Ok(file_config)
    }

    pub fn save(name: &str, workspace: &Self) -> Result<()>;

    /// Capture the current app state. Cheap; does not touch PTY internals.
    pub fn from_app(app: &App) -> Self {
        let active = app.active_tab;
        let tabs = app
            .tabs
            .iter()
            .map(|tab| TabLayout::save_tab(&tab))
            .collect();
        Workspace {
            active_tab: active,
            tabs,
        }
    }

    /// Apply a workspace to an empty app, spawning panes as configured.
    pub fn apply(self, app: &mut App, config: &Config) -> Result<()>;
}
