//! Watches the browser's download folder so a "Manual download" from Nexus
//! installs itself, no nxm:// link or Premium account needed.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::ops::InstallMeta;

const INTERVAL: Duration = Duration::from_secs(2);

pub struct Watcher {
    dir: PathBuf,
    /// Files already handled, or present before watching started.
    seen: HashSet<PathBuf>,
    /// New files still being written: size at the last look.
    growing: HashMap<PathBuf, u64>,
    last_scan: Instant,
    /// Take any archive, not just Nexus-named ones (for a mods-only folder).
    any_archive: bool,
}

impl Watcher {
    /// Start watching `dir`. Archives already there are left alone.
    pub fn new(dir: PathBuf, any_archive: bool) -> Self {
        let seen = archives(&dir).into_iter().map(|(p, _)| p).collect();
        Self { dir, seen, growing: HashMap::new(), last_scan: Instant::now(), any_archive }
    }

    /// New downloads that finished since the last call. Cheap to call
    /// every frame; the folder is only listed every couple of seconds.
    pub fn poll(&mut self) -> Vec<PathBuf> {
        if self.last_scan.elapsed() < INTERVAL {
            return Vec::new();
        }
        self.last_scan = Instant::now();
        self.scan()
    }

    fn scan(&mut self) -> Vec<PathBuf> {
        let mut ready = Vec::new();
        let mut growing = HashMap::new();
        for (path, size) in archives(&self.dir) {
            if self.seen.contains(&path) {
                continue;
            }
            // Only call a file finished once its size held still between two looks.
            if size > 0 && self.growing.get(&path) == Some(&size) {
                self.seen.insert(path.clone());
                if self.any_archive || is_nexus_download(&path) {
                    ready.push(path);
                }
            } else {
                growing.insert(path, size);
            }
        }
        self.growing = growing;
        ready.sort();
        ready
    }
}

/// Nexus names files `Name-<modid>-<version>-<timestamp>.ext`; anything else
/// in the download folder probably isn't a mod.
pub fn is_nexus_download(path: &Path) -> bool {
    crate::archive::is_supported(path) && InstallMeta::from_archive_name(path).nexus.is_some()
}

fn archives(dir: &Path) -> Vec<(PathBuf, u64)> {
    let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
    entries
        .flatten()
        .filter_map(|e| {
            let meta = e.metadata().ok().filter(|m| m.is_file())?;
            let path = e.path();
            crate::archive::is_supported(&path).then_some((path, meta.len()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_up_finished_nexus_downloads_only() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("Old Mod-100-1-0-1700000000.zip");
        fs::write(&old, "old").unwrap();
        let mut w = Watcher::new(dir.path().to_path_buf(), false);

        let new = dir.path().join("Cool Mod-12345-1-2-1760000000.zip");
        fs::write(&new, "part").unwrap();
        fs::write(dir.path().join("holiday photos.zip"), "x").unwrap();
        fs::write(dir.path().join("Cool Mod-12345-1-2-1760000000.zip.part"), "x").unwrap();
        assert!(w.scan().is_empty(), "first sighting may still be downloading");
        fs::write(&new, "partial").unwrap();
        assert!(w.scan().is_empty(), "still growing");
        assert_eq!(w.scan(), std::slice::from_ref(&new));
        assert!(w.scan().is_empty(), "reported once");
    }

    #[test]
    fn any_archive_mode_takes_renamed_files() {
        let dir = tempfile::tempdir().unwrap();
        let mut w = Watcher::new(dir.path().to_path_buf(), true);
        let renamed = dir.path().join("my favourite mod.7z");
        fs::write(&renamed, "x").unwrap();
        fs::write(dir.path().join("notes.txt"), "x").unwrap();
        assert!(w.scan().is_empty());
        assert_eq!(w.scan(), std::slice::from_ref(&renamed));
    }
}
