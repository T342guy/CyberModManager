use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NexusRef {
    pub mod_id: u64,
    /// Known when installed through Nexus; guessed installs only know the mod id.
    pub file_id: Option<u64>,
}

/// Installed from a GitHub release (the built-in frameworks).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GithubRef {
    pub repo: String,
    pub tag: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModEntry {
    pub id: String,
    pub name: String,
    pub version: Option<String>,
    pub nexus: Option<NexusRef>,
    #[serde(default)]
    pub github: Option<GithubRef>,
    /// Archive file name the mod was installed from.
    pub archive: Option<String>,
    pub enabled: bool,
    /// Game-relative paths (`/`-separated) of every file this mod provides.
    pub files: Vec<String>,
    /// Files currently parked in holding because a later mod overrode them.
    #[serde(default)]
    pub overridden: Vec<String>,
    /// Pre-existing (unmanaged) game files this mod replaced, kept in backups.
    #[serde(default)]
    pub backed_up: Vec<String>,
    pub installed_at: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Registry {
    pub mods: Vec<ModEntry>,
    #[serde(skip)]
    path: PathBuf,
}

impl Registry {
    pub fn load(path: &Path) -> Result<Self> {
        let mut reg: Registry = match fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Registry::default(),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        reg.path = path.to_path_buf();
        Ok(reg)
    }

    pub fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        crate::fsutil::write_atomic(&self.path, serde_json::to_string_pretty(self)?.as_bytes())
    }

    pub fn get(&self, id: &str) -> Option<&ModEntry> {
        self.mods.iter().find(|m| m.id == id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut ModEntry> {
        self.mods.iter_mut().find(|m| m.id == id)
    }

    /// The enabled mod whose copy of `rel` is currently live in the game dir.
    pub fn live_owner(&self, rel: &str, except: &str) -> Option<&ModEntry> {
        self.mods.iter().find(|m| {
            m.id != except
                && m.enabled
                && m.files.iter().any(|f| f == rel)
                && !m.overridden.iter().any(|f| f == rel)
        })
    }

    /// Most recently installed enabled mod that has `rel` parked as overridden.
    pub fn overridden_owner(&self, rel: &str, except: &str) -> Option<String> {
        self.mods
            .iter()
            .filter(|m| m.id != except && m.enabled && m.overridden.iter().any(|f| f == rel))
            .max_by_key(|m| m.installed_at)
            .map(|m| m.id.clone())
    }

    pub fn unique_id(&self, name: &str) -> String {
        let base = crate::layout::sanitize_name(name).to_lowercase();
        let mut id = base.clone();
        let mut n = 2;
        while self.get(&id).is_some() {
            id = format!("{base}-{n}");
            n += 1;
        }
        id
    }

    /// Find an existing install of the same mod: same name, and same Nexus mod
    /// id when both are known (a Nexus page can host several separate files).
    pub fn find_same(&self, name: &str, nexus: Option<&NexusRef>) -> Option<String> {
        self.mods
            .iter()
            .find(|m| {
                m.name.eq_ignore_ascii_case(name)
                    && match (nexus, &m.nexus) {
                        (Some(a), Some(b)) => a.mod_id == b.mod_id,
                        _ => true,
                    }
            })
            .map(|m| m.id.clone())
    }
}
