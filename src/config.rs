use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Cyberpunk 2077 install folder (the one containing `bin/` and `archive/`).
    pub game_dir: Option<PathBuf>,
    /// Where disabled mods are kept. Defaults to the data dir when unset.
    pub holding_dir: Option<PathBuf>,
    pub nexus_api_key: Option<String>,
    /// Skip the conflict prompt and always let the newest mod win.
    pub always_overwrite: bool,
    /// Delete downloaded archives after a successful Nexus install.
    pub delete_downloads: bool,
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        match fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).with_context(|| format!("parsing {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(self)?;
        crate::fsutil::write_atomic(path, text.as_bytes())?;
        // The file holds the Nexus API key, keep it private.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }
}
