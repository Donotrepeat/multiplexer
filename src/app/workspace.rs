use crate::app::config::Config;
use crate::app::tabs::Grid;
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

#[derive(Default, Deserialize)]
#[serde(default)]
pub struct PaneLayout {
    pub shell: Option<PathBuf>,
    pub working_dir: Option<PathBuf>,
}

impl Workspace {
    pub fn load(name: &str) -> Result<Option<Self>>;
    pub fn save(name: &str, workspace: &Self) -> Result<()>;

    /// Capture the current app state. Cheap; does not touch PTY internals.
    pub fn from_app(app: &App) -> Self;

    /// Apply a workspace to an empty app, spawning panes as configured.
    pub fn apply(self, app: &mut App, config: &Config) -> Result<()>;
}
