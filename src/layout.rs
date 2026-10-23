//! Turn an extracted mod archive into a list of game-relative file paths.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use walkdir::WalkDir;

/// Top-level game folders a mod can install into.
const ROOT_MARKERS: &[&str] = &["archive", "bin", "r6", "red4ext", "engine", "mods"];

/// Game directories that must survive when the last mod file in them is removed.
pub const PROTECTED_DIRS: &[&str] = &[
    "archive",
    "archive/pc",
    "archive/pc/mod",
    "archive/pc/content",
    "bin",
    "bin/x64",
    "bin/x64/plugins",
    "bin/x64/plugins/cyber_engine_tweaks",
    "bin/x64/plugins/cyber_engine_tweaks/mods",
    "r6",
    "r6/scripts",
    "r6/tweaks",
    "r6/cache",
    "r6/config",
    "red4ext",
    "red4ext/plugins",
    "engine",
    "engine/config",
    "mods",
];

/// Folders that only ever contain mod files (nothing here ships with the game).
const MOD_ONLY_DIRS: &[&str] = &[
    "archive/pc/mod/",
    "r6/scripts/",
    "r6/tweaks/",
    "r6/config/cybercmd/",
    "r6/config/redsuserhints/",
    "red4ext/",
    "bin/x64/plugins/",
    "mods/",
];

/// Mod loader files that live next to the game executable.
const MOD_ONLY_FILES: &[&str] = &["bin/x64/winmm.dll", "bin/x64/version.dll", "bin/x64/global.ini"];

/// True for paths that can only belong to a mod, never to the base game.
/// Old copies of these are not worth restoring when a managed mod replaces them.
pub fn is_mod_only_path(rel: &str) -> bool {
    let lower = rel.to_lowercase();
    if lower == "mods/.stub" {
        return false;
    }
    MOD_ONLY_FILES.contains(&lower.as_str()) || MOD_ONLY_DIRS.iter().any(|d| lower.starts_with(d))
}

/// Scan roots for [`is_mod_only_path`] files, relative to the game dir.
pub fn mod_only_roots() -> impl Iterator<Item = &'static str> {
    MOD_ONLY_DIRS.iter().map(|d| d.trim_end_matches('/')).chain(MOD_ONLY_FILES.iter().copied())
}

/// Catch-all group for mod framework files (CET, RED4ext, loaders).
pub const LOADERS_GROUP: &str = "Frameworks & loaders (CET, RED4ext, ...)";

/// Group loose game files into likely mods, by the folder layout each mod type uses.
pub fn group_untracked(files: &[String]) -> BTreeMap<String, Vec<String>> {
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for rel in files {
        let parts: Vec<&str> = rel.split('/').collect();
        let lower = rel.to_lowercase();
        let stem = |s: &str| s.split('.').next().unwrap_or(s).to_string();
        let sub = |depth: usize| -> Option<&str> { (parts.len() > depth + 1).then(|| parts[depth]) };
        let name = if lower.starts_with("bin/x64/plugins/cyber_engine_tweaks/mods/") {
            sub(5).map(|m| format!("CET mod: {m}"))
        } else if lower.starts_with("red4ext/plugins/") {
            sub(2).map(|m| format!("RED4ext plugin: {m}"))
        } else if lower.starts_with("mods/") {
            sub(1).map(|m| format!("REDmod: {m}"))
        } else if lower.starts_with("archive/pc/mod/") {
            Some(stem(parts[parts.len() - 1]))
        } else if lower.starts_with("r6/scripts/") || lower.starts_with("r6/tweaks/") {
            Some(sub(2).map(str::to_string).unwrap_or_else(|| stem(parts[parts.len() - 1])))
        } else {
            None
        };
        let name = name.unwrap_or_else(|| LOADERS_GROUP.to_string());
        groups.entry(name).or_default().push(rel.clone());
    }
    groups
}

/// Map of game-relative destination (`/`-separated) -> extracted source file.
pub type Plan = BTreeMap<String, PathBuf>;

pub fn plan(extracted: &Path, mod_name: &str) -> Result<Plan> {
    if let Some(root) = find_game_root(extracted) {
        let mut plan = Plan::new();
        for marker in dir_children(&root) {
            if !is_marker(&marker) {
                continue;
            }
            add_tree(&mut plan, &marker, &root);
        }
        if !plan.is_empty() {
            return Ok(plan);
        }
    }

    // No game-root folders: place loose files by type.
    let mut plan = Plan::new();
    let folder = sanitize_name(mod_name);
    for entry in WalkDir::new(extracted).into_iter().flatten() {
        let path = entry.path();
        if entry.file_type().is_dir() && path.join("init.lua").is_file() {
            // Cyber Engine Tweaks mod folder.
            let name = path.file_name().unwrap().to_string_lossy();
            let dest_base = format!("bin/x64/plugins/cyber_engine_tweaks/mods/{name}");
            for f in WalkDir::new(path).into_iter().flatten() {
                if f.file_type().is_file() {
                    let rel = f.path().strip_prefix(path).unwrap();
                    plan.insert(format!("{dest_base}/{}", to_slash(rel)), f.path().to_path_buf());
                }
            }
            continue;
        }
        if !entry.file_type().is_file() || in_plan(&plan, path) {
            continue;
        }
        let file_name = entry.file_name().to_string_lossy();
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let dest = match ext.as_str() {
            "archive" | "xl" => format!("archive/pc/mod/{file_name}"),
            "reds" => format!("r6/scripts/{folder}/{file_name}"),
            "yaml" | "yml" => format!("r6/tweaks/{file_name}"),
            _ => continue,
        };
        plan.insert(dest, path.to_path_buf());
    }

    if plan.is_empty() {
        let top: Vec<String> = dir_entries(extracted)
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        bail!(
            "unrecognized mod layout (no archive/, bin/, r6/, red4ext/ folders or .archive/.reds files). Top-level contents: {}",
            if top.is_empty() { "<empty>".to_string() } else { top.join(", ") }
        );
    }
    Ok(plan)
}

/// Shallowest directory (depth <= 3) containing at least one marker folder.
fn find_game_root(extracted: &Path) -> Option<PathBuf> {
    let mut level = vec![extracted.to_path_buf()];
    for _ in 0..=3 {
        let mut next = Vec::new();
        for dir in &level {
            let children = dir_children(dir);
            if children.iter().any(|c| is_marker(c)) {
                return Some(dir.clone());
            }
            next.extend(children.into_iter().filter(|c| !is_junk_dir(c)));
        }
        level = next;
    }
    None
}

/// Windows/macOS metadata files that sometimes get zipped up with mods.
fn is_junk_file(path: &Path) -> bool {
    let name = path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    matches!(name.as_str(), "desktop.ini" | "thumbs.db" | ".ds_store")
}

fn add_tree(plan: &mut Plan, dir: &Path, root: &Path) {
    for entry in WalkDir::new(dir).into_iter().flatten() {
        if entry.file_type().is_file() && !is_junk_file(entry.path()) {
            let rel = entry.path().strip_prefix(root).unwrap();
            plan.insert(to_slash(rel), entry.path().to_path_buf());
        }
    }
}

fn in_plan(plan: &Plan, path: &Path) -> bool {
    plan.values().any(|p| p == path)
}

fn is_marker(dir: &Path) -> bool {
    let name = dir.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    ROOT_MARKERS.contains(&name.as_str())
}

fn is_junk_dir(dir: &Path) -> bool {
    let name = dir.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    matches!(name.as_str(), "fomod" | "__macosx" | "images" | "screenshots")
}

fn dir_entries(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| rd.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    out.sort();
    out
}

fn dir_children(dir: &Path) -> Vec<PathBuf> {
    dir_entries(dir)
        .into_iter()
        .filter(|p| p.symlink_metadata().map(|m| m.is_dir()).unwrap_or(false))
        .collect()
}

fn to_slash(p: &Path) -> String {
    p.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

pub fn sanitize_name(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    let s = s.trim_matches('_').to_string();
    if s.is_empty() { "mod".into() } else { s }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn touch(base: &Path, rel: &str) {
        let p = base.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, rel).unwrap();
    }

    fn keys(plan: &Plan) -> Vec<&str> {
        plan.keys().map(String::as_str).collect()
    }

    #[test]
    fn root_markers_at_top() {
        let t = tempfile::tempdir().unwrap();
        touch(t.path(), "archive/pc/mod/a.archive");
        touch(t.path(), "r6/scripts/m/a.reds");
        touch(t.path(), "r6/scripts/m/desktop.ini");
        touch(t.path(), "readme.txt");
        let p = plan(t.path(), "M").unwrap();
        assert_eq!(keys(&p), ["archive/pc/mod/a.archive", "r6/scripts/m/a.reds"]);
    }

    #[test]
    fn nested_root() {
        let t = tempfile::tempdir().unwrap();
        touch(t.path(), "My Mod v2/bin/x64/plugins/cyber_engine_tweaks/mods/m/init.lua");
        touch(t.path(), "My Mod v2/readme.md");
        let p = plan(t.path(), "M").unwrap();
        assert_eq!(keys(&p), ["bin/x64/plugins/cyber_engine_tweaks/mods/m/init.lua"]);
    }

    #[test]
    fn loose_files() {
        let t = tempfile::tempdir().unwrap();
        touch(t.path(), "Option A/thing.archive");
        touch(t.path(), "thing.archive.xl");
        touch(t.path(), "script.reds");
        touch(t.path(), "tweak.yaml");
        touch(t.path(), "readme.txt");
        let p = plan(t.path(), "Cool Mod").unwrap();
        assert_eq!(
            keys(&p),
            [
                "archive/pc/mod/thing.archive",
                "archive/pc/mod/thing.archive.xl",
                "r6/scripts/Cool_Mod/script.reds",
                "r6/tweaks/tweak.yaml"
            ]
        );
    }

    #[test]
    fn loose_cet_folder() {
        let t = tempfile::tempdir().unwrap();
        touch(t.path(), "MyCetMod/init.lua");
        touch(t.path(), "MyCetMod/modules/x.lua");
        let p = plan(t.path(), "M").unwrap();
        assert_eq!(
            keys(&p),
            [
                "bin/x64/plugins/cyber_engine_tweaks/mods/MyCetMod/init.lua",
                "bin/x64/plugins/cyber_engine_tweaks/mods/MyCetMod/modules/x.lua"
            ]
        );
    }

    #[test]
    fn mod_only_paths() {
        assert!(is_mod_only_path("bin/x64/winmm.dll"));
        assert!(is_mod_only_path("r6/config/redsUserHints/ArchiveXL.toml"));
        assert!(is_mod_only_path("archive/pc/mod/x.archive"));
        assert!(!is_mod_only_path("archive/pc/content/basegame_1_engine.archive"));
        assert!(!is_mod_only_path("engine/config/base/scripts.ini"));
        assert!(!is_mod_only_path("mods/.stub"));
    }

    #[test]
    fn groups_untracked_files() {
        let files: Vec<String> = [
            "bin/x64/plugins/cyber_engine_tweaks/mods/Loot/init.lua",
            "bin/x64/plugins/cyber_engine_tweaks/mods/Loot/Modules/a.lua",
            "bin/x64/plugins/cyber_engine_tweaks.asi",
            "bin/x64/winmm.dll",
            "archive/pc/mod/Ragdoll.archive",
            "archive/pc/mod/Ragdoll.archive.xl",
            "r6/tweaks/Guns/a.yaml",
            "r6/scripts/Stamina.reds",
        ]
        .map(String::from)
        .to_vec();
        let g = group_untracked(&files);
        assert_eq!(g["CET mod: Loot"].len(), 2);
        assert_eq!(g["Ragdoll"].len(), 2);
        assert_eq!(g["Guns"], ["r6/tweaks/Guns/a.yaml"]);
        assert_eq!(g["Stamina"], ["r6/scripts/Stamina.reds"]);
        assert_eq!(g[LOADERS_GROUP].len(), 2);
    }

    #[test]
    fn unrecognized() {
        let t = tempfile::tempdir().unwrap();
        touch(t.path(), "stuff/readme.txt");
        let err = plan(t.path(), "M").unwrap_err().to_string();
        assert!(err.contains("stuff"), "{err}");
    }
}
