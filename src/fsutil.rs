use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Move a file, creating parent dirs. Falls back to copy + delete when the
/// destination is on another filesystem. Overwrites an existing destination.
pub fn move_file(src: &Path, dst: &Path) -> Result<()> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    match fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::CrossesDevices => {
            fs::copy(src, dst)
                .with_context(|| format!("copying {} -> {}", src.display(), dst.display()))?;
            fs::remove_file(src).with_context(|| format!("removing {}", src.display()))?;
            Ok(())
        }
        Err(e) => {
            Err(e).with_context(|| format!("moving {} -> {}", src.display(), dst.display()))
        }
    }
}

/// Remove empty directories from `start` upwards, stopping at `stop` (exclusive)
/// and never removing any directory listed in `keep` (relative to `stop`).
pub fn prune_empty_dirs(start: &Path, stop: &Path, keep: &[&str]) {
    let mut dir = start.to_path_buf();
    while dir.starts_with(stop) && dir != stop {
        if let Ok(rel) = dir.strip_prefix(stop) {
            let rel = rel.to_string_lossy().replace('\\', "/").to_lowercase();
            if keep.iter().any(|k| *k == rel) {
                break;
            }
        }
        // remove_dir only succeeds on empty directories.
        if fs::remove_dir(&dir).is_err() {
            break;
        }
        match dir.parent() {
            Some(p) => dir = p.to_path_buf(),
            None => break,
        }
    }
}

/// Remove every empty directory below `root` (and `root` itself if empty).
pub fn remove_empty_tree(root: &Path) {
    if !root.is_dir() {
        return;
    }
    for entry in walkdir::WalkDir::new(root).contents_first(true) {
        if let Ok(entry) = entry
            && entry.file_type().is_dir()
        {
            let _ = fs::remove_dir(entry.path());
        }
    }
}

pub fn write_atomic(path: &Path, data: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, data).with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}

/// Join a `/`-separated relative path onto `base`, rejecting anything that
/// would escape it (absolute paths, `..`).
pub fn safe_join(base: &Path, rel: &str) -> Result<PathBuf> {
    let mut out = base.to_path_buf();
    for part in rel.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => anyhow::bail!("refusing path with '..': {rel}"),
            p => out.push(p),
        }
    }
    Ok(out)
}

/// Resolve `rel` against `base`, reusing the on-disk spelling of any directory
/// that already exists with different letter case (e.g. `Archive/PC/Mod` ->
/// `archive/pc/mod`). Wine is case-insensitive, Linux is not, so duplicates
/// with different case would confuse the game.
pub fn fix_case(base: &Path, rel: &str) -> String {
    let parts: Vec<&str> = rel.split('/').filter(|p| !p.is_empty()).collect();
    let mut cur = base.to_path_buf();
    let mut out = Vec::with_capacity(parts.len());
    for (i, part) in parts.iter().enumerate() {
        let is_last = i + 1 == parts.len();
        let mut chosen = part.to_string();
        if !is_last && !cur.join(part).exists()
            && let Ok(entries) = fs::read_dir(&cur)
        {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.eq_ignore_ascii_case(part) && entry.path().is_dir() {
                    chosen = name;
                    break;
                }
            }
        }
        cur.push(&chosen);
        out.push(chosen);
    }
    out.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fix_case_matches_existing_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("archive/pc/mod")).unwrap();
        assert_eq!(fix_case(tmp.path(), "Archive/PC/Mod/x.archive"), "archive/pc/mod/x.archive");
        assert_eq!(fix_case(tmp.path(), "r6/Scripts/a.reds"), "r6/Scripts/a.reds");
    }

    #[test]
    fn safe_join_rejects_parent() {
        assert!(safe_join(Path::new("/g"), "../etc/passwd").is_err());
        assert_eq!(safe_join(Path::new("/g"), "a/b").unwrap(), PathBuf::from("/g/a/b"));
    }

    #[test]
    fn prune_stops_at_keep() {
        let tmp = tempfile::tempdir().unwrap();
        let deep = tmp.path().join("archive/pc/mod/sub/deeper");
        fs::create_dir_all(&deep).unwrap();
        prune_empty_dirs(&deep, tmp.path(), &["archive/pc/mod"]);
        assert!(!tmp.path().join("archive/pc/mod/sub").exists());
        assert!(tmp.path().join("archive/pc/mod").exists());
    }
}
