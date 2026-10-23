//! The common frameworks most mods depend on. They all publish ready-to-extract
//! game-root zips on GitHub, so they install without a Nexus account.

use std::path::Path;

use crate::registry::ModEntry;

#[derive(Debug)]
pub struct Framework {
    pub name: &'static str,
    pub repo: &'static str,
    pub nexus_id: u64,
    pub about: &'static str,
    /// Names of other frameworks this one needs.
    pub requires: &'static [&'static str],
    /// A file that is present when the framework is installed (by any means).
    pub marker: &'static str,
    /// Part of the recommended "essentials" set.
    pub essential: bool,
}

pub const CATALOG: &[Framework] = &[
    Framework {
        name: "Cyber Engine Tweaks",
        repo: "maximegmd/CyberEngineTweaks",
        nexus_id: 107,
        about: "Lua scripting and the in-game console (CET mods)",
        requires: &[],
        marker: "bin/x64/plugins/cyber_engine_tweaks.asi",
        essential: true,
    },
    Framework {
        name: "RED4ext",
        repo: "wopss/RED4ext",
        nexus_id: 2380,
        about: "Native plugin loader (red4ext/plugins)",
        requires: &[],
        marker: "red4ext/RED4ext.dll",
        essential: true,
    },
    Framework {
        name: "redscript",
        repo: "jac3km4/redscript",
        nexus_id: 1511,
        about: "Script compiler (.reds mods)",
        requires: &[],
        marker: "engine/tools/scc.exe",
        essential: true,
    },
    Framework {
        name: "TweakXL",
        repo: "psiberx/cp2077-tweak-xl",
        nexus_id: 4197,
        about: "TweakDB changes (r6/tweaks)",
        requires: &["RED4ext", "redscript"],
        marker: "red4ext/plugins/TweakXL/TweakXL.dll",
        essential: true,
    },
    Framework {
        name: "ArchiveXL",
        repo: "psiberx/cp2077-archive-xl",
        nexus_id: 4198,
        about: "Loads custom resources (.xl files, clothing, items)",
        requires: &["RED4ext", "redscript"],
        marker: "red4ext/plugins/ArchiveXL/ArchiveXL.dll",
        essential: true,
    },
    Framework {
        name: "Codeware",
        repo: "psiberx/cp2077-codeware",
        nexus_id: 7780,
        about: "Library used by many redscript and CET mods",
        requires: &["RED4ext", "redscript"],
        marker: "red4ext/plugins/Codeware/Codeware.dll",
        essential: true,
    },
    Framework {
        name: "Mod Settings",
        repo: "jackhumbert/mod_settings",
        nexus_id: 4885,
        about: "Settings menu for mods",
        requires: &["RED4ext", "redscript", "ArchiveXL"],
        marker: "red4ext/plugins/mod_settings/mod_settings.dll",
        essential: false,
    },
    Framework {
        name: "Input Loader",
        repo: "jackhumbert/cyberpunk2077-input-loader",
        nexus_id: 4575,
        about: "Custom key bindings for mods (r6/input)",
        requires: &["RED4ext"],
        marker: "red4ext/plugins/input_loader/input_loader.dll",
        essential: false,
    },
];

pub fn by_name(name: &str) -> Option<&'static Framework> {
    CATALOG.iter().find(|f| f.name.eq_ignore_ascii_case(name))
}

pub fn by_repo(repo: &str) -> Option<&'static Framework> {
    CATALOG.iter().find(|f| f.repo.eq_ignore_ascii_case(repo))
}

/// `picked` plus everything they need that isn't in `have`, dependencies first.
pub fn install_order(picked: &[&str], have: &[&str]) -> Vec<&'static Framework> {
    fn visit(name: &str, have: &[&str], out: &mut Vec<&'static Framework>, depth: usize) {
        let Some(fw) = by_name(name) else { return };
        if depth > CATALOG.len() || out.iter().any(|f| f.name == fw.name) {
            return;
        }
        for dep in fw.requires {
            if !have.iter().any(|h| h.eq_ignore_ascii_case(dep)) {
                visit(dep, have, out, depth + 1);
            }
        }
        if !out.iter().any(|f| f.name == fw.name) {
            out.push(fw);
        }
    }
    let mut out = Vec::new();
    // Catalog order keeps the result stable regardless of click order.
    for fw in CATALOG {
        if picked.iter().any(|p| p.eq_ignore_ascii_case(fw.name)) {
            visit(fw.name, have, &mut out, 0);
        }
    }
    out
}

/// Pick the Windows game zip out of a release's assets, skipping debug
/// symbols and builds for other platforms.
pub fn pick_asset<'a>(names: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    const SKIP: &[&str] = &["pdb", "symbols", "macos", "darwin", "linux", "src", "source"];
    let candidates: Vec<&str> = names
        .into_iter()
        .filter(|n| {
            let l = n.to_lowercase();
            l.ends_with(".zip") && !SKIP.iter().any(|s| l.contains(s))
        })
        .collect();
    candidates
        .iter()
        .find(|n| n.to_lowercase().contains("windows"))
        .or(candidates.first())
        .copied()
}

/// Every mod-list entry that is this framework, however it got installed:
/// from its GitHub repo or Nexus page, under its name, or by providing its
/// marker file (archives from Nexus and adopted files have other names).
pub fn owners<'a>(mods: &'a [ModEntry], fw: &Framework) -> Vec<&'a ModEntry> {
    mods.iter()
        .filter(|m| {
            m.github.as_ref().is_some_and(|g| g.repo.eq_ignore_ascii_case(fw.repo))
                || m.nexus.as_ref().is_some_and(|n| n.mod_id == fw.nexus_id)
                || m.name.eq_ignore_ascii_case(fw.name)
                || m.files.iter().any(|f| f.eq_ignore_ascii_case(fw.marker))
        })
        .collect()
}

pub fn is_present(game: &Path, fw: &Framework) -> bool {
    game.join(crate::fsutil::fix_case(game, fw.marker)).is_file()
}

/// Compare versions loosely: "v1.2.0" and "1.2" are the same.
pub fn same_version(a: &str, b: &str) -> bool {
    let norm = |s: &str| {
        let mut parts: Vec<u64> = s
            .trim()
            .trim_start_matches(['v', 'V'])
            .split(['.', '-', '_'])
            .map_while(|p| p.parse().ok())
            .collect();
        while parts.last() == Some(&0) {
            parts.pop();
        }
        parts
    };
    let (na, nb) = (norm(a), norm(b));
    if na.is_empty() || nb.is_empty() {
        return a.trim().trim_start_matches(['v', 'V']).eq_ignore_ascii_case(b.trim().trim_start_matches(['v', 'V']));
    }
    na == nb
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(v: Vec<&Framework>) -> Vec<&str> {
        v.into_iter().map(|f| f.name).collect()
    }

    #[test]
    fn dependencies_come_first() {
        assert_eq!(names(install_order(&["TweakXL"], &[])), ["RED4ext", "redscript", "TweakXL"]);
        assert_eq!(names(install_order(&["TweakXL"], &["RED4ext", "redscript"])), ["TweakXL"]);
        assert_eq!(
            names(install_order(&["Mod Settings", "Cyber Engine Tweaks"], &["RED4ext"])),
            ["Cyber Engine Tweaks", "redscript", "ArchiveXL", "Mod Settings"]
        );
    }

    #[test]
    fn every_dependency_exists() {
        for fw in CATALOG {
            for dep in fw.requires {
                assert!(by_name(dep).is_some(), "{} needs unknown {dep}", fw.name);
            }
        }
    }

    #[test]
    fn asset_choice() {
        assert_eq!(pick_asset(["TweakXL-1.11.5-pdb.zip", "TweakXL-1.11.5.zip"]), Some("TweakXL-1.11.5.zip"));
        assert_eq!(pick_asset(["red4ext-1.30.0.zip", "red4ext-symbols-1.30.0.zip"]), Some("red4ext-1.30.0.zip"));
        assert_eq!(
            pick_asset([
                "redscript-cli-aarch64-darwin",
                "redscript-cli.exe",
                "redscript-v0.5.32-macos.zip",
                "redscript-v0.5.32-windows.zip"
            ]),
            Some("redscript-v0.5.32-windows.zip")
        );
        assert_eq!(pick_asset(["notes.txt"]), None);
    }

    #[test]
    fn owners_found_by_any_route() {
        let entry = |id: &str, name: &str, files: &[&str]| ModEntry {
            id: id.into(),
            name: name.into(),
            version: None,
            nexus: None,
            github: None,
            archive: None,
            enabled: true,
            files: files.iter().map(|f| f.to_string()).collect(),
            overridden: Vec::new(),
            backed_up: Vec::new(),
            installed_at: 0,
        };
        let tweak = by_name("TweakXL").unwrap();
        let mut nexus = entry("a", "TweakXL 1.11.5", &[]);
        nexus.nexus = Some(crate::registry::NexusRef { mod_id: tweak.nexus_id, file_id: None });
        let mods = vec![
            nexus,
            entry("b", "untracked red4ext", &["red4ext/plugins/tweakxl/TweakXL.dll", "r6/scripts/x.reds"]),
            entry("c", "tweakxl", &[]),
            entry("d", "Something else", &["archive/pc/mod/x.archive"]),
        ];
        let ids: Vec<&str> = owners(&mods, tweak).iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["a", "b", "c"]);
    }

    #[test]
    fn version_comparison() {
        assert!(same_version("v1.38.0", "1.38"));
        assert!(same_version("1.2.3", "1.2.3"));
        assert!(!same_version("v1.38.0", "1.37.1"));
        assert!(same_version("beta", "Beta"));
    }
}
