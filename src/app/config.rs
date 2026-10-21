use anyhow::Result;
use serde::Deserialize;

pub mod keymap;
pub mod keyspec;
pub mod terminal;

pub use keymap::KeyMap;
pub use keyspec::KeySpec;
pub use terminal::TerminalConfig;

#[derive(Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub keybindings: KeyMap,
    pub terminal: TerminalConfig,
}

impl Config {
    /// Load from the XDG config path; return defaults if the file is missing.
    pub fn load() -> Result<Self> {
        let Some(config_dir) = dirs::config_dir() else {
            return Ok(Config::default());
        };
        let path = config_dir.join("multiplexer/config.toml");
        if !path.exists() {
            return Ok(Config::default());
        }

        let contents = std::fs::read_to_string(&path)?;
        let file_config: Config = toml::from_str(&contents)?;
        Ok(Config::default().merge(file_config))
    }

    fn merge(mut self, other: Config) -> Self {
        self.keybindings.merge(other.keybindings);
        self.terminal = other.terminal;
        self
    }
}
