use std::path::PathBuf;

use anyhow::{Context, Result};
use directories::ProjectDirs;

/// Fixed locations the manager uses for its own state.
#[derive(Debug, Clone)]
pub struct AppPaths {
    pub config_file: PathBuf,
    pub data_dir: PathBuf,
}

impl AppPaths {
    pub fn discover() -> Result<Self> {
        let dirs = ProjectDirs::from("", "", "cyber-mod-manager")
            .context("could not determine home directory")?;
        Ok(Self {
            config_file: dirs.config_dir().join("config.toml"),
            data_dir: dirs.data_dir().to_path_buf(),
        })
    }

    pub fn registry_file(&self) -> PathBuf {
        self.data_dir.join("mods.json")
    }

    pub fn default_holding_dir(&self) -> PathBuf {
        self.data_dir.join("holding")
    }

    pub fn backups_dir(&self) -> PathBuf {
        self.data_dir.join("backups")
    }

    pub fn downloads_dir(&self) -> PathBuf {
        self.data_dir.join("downloads")
    }

    pub fn replaced_dir(&self) -> PathBuf {
        self.data_dir.join("replaced")
    }

    pub fn staging_dir(&self) -> PathBuf {
        self.data_dir.join("staging")
    }

    /// Socket used to hand nxm:// links to an already running instance.
    pub fn ipc_socket() -> PathBuf {
        let base = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        base.join("cyber-mod-manager.sock")
    }
}
