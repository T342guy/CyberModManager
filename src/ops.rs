//! Install / enable / disable / remove. Every mod lives in exactly one place:
//! enabled mods in the game dir, disabled mods in `holding/<id>/`.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};

use crate::fsutil::{fix_case, move_file, prune_empty_dirs, remove_empty_tree, safe_join};
use crate::layout::{self, PROTECTED_DIRS};
use crate::registry::{GithubRef, ModEntry, NexusRef, Registry};

#[derive(Debug, Clone)]
pub struct Dirs {
    pub game: PathBuf,
    pub holding: PathBuf,
    pub backups: PathBuf,
    pub staging: PathBuf,
    /// Untracked old mod files a managed mod replaced. Kept just in case, never restored.
    pub replaced: PathBuf,
}

#[derive(Debug, Clone, Default)]
pub struct InstallMeta {
    pub name: String,
    pub version: Option<String>,
    pub nexus: Option<NexusRef>,
    pub github: Option<GithubRef>,
    pub archive: Option<String>,
    /// Entries this install replaces even though their name differs.
    pub replaces: Vec<String>,
}

impl InstallMeta {
    /// Guess name / Nexus mod id / version from a Nexus-style file name such as
    /// `Cyber Engine Tweaks-107-1-32-3-1720000000.zip`.
    pub fn from_archive_name(path: &Path) -> Self {
        let file_name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let stem = path.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let parts: Vec<&str> = stem.split('-').collect();
        let numeric = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
        // Nexus appends -<modid>-<version parts...>-<timestamp>; find where the all-numeric tail starts.
        let tail_start = (1..parts.len()).find(|&i| parts[i..].iter().all(|p| numeric(p)));
        let mut meta = InstallMeta { name: stem.clone(), archive: Some(file_name), ..Default::default() };
        if let Some(i) = tail_start
            && parts.len() - i >= 3
        {
            meta.name = parts[..i].join("-").trim().to_string();
            meta.nexus = parts[i].parse().ok().map(|mod_id| NexusRef { mod_id, file_id: None });
            let version = parts[i + 1..parts.len() - 1].join(".");
            meta.version = (!version.is_empty()).then_some(version);
        }
        meta
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub file: String,
    pub other_id: String,
    pub other_name: String,
}

#[derive(Debug)]
pub enum EnableOutcome {
    Enabled { overrode: Vec<Conflict> },
    /// Nothing was changed; call again with `overwrite = true` to proceed.
    Conflicts(Vec<Conflict>),
}

/// Records moves so a failed operation can be undone.
#[derive(Default)]
struct Journal(Vec<(PathBuf, PathBuf)>);

impl Journal {
    fn mv(&mut self, from: PathBuf, to: PathBuf) -> Result<()> {
        move_file(&from, &to)?;
        self.0.push((from, to));
        Ok(())
    }

    fn rollback(self) {
        for (from, to) in self.0.into_iter().rev() {
            if let Err(e) = move_file(&to, &from) {
                eprintln!("rollback failed: {e:#}");
            }
        }
    }
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Extract `archive` and register it as a (disabled) mod parked in holding.
/// An existing install of the same mod is removed first. Returns the new id.
pub fn install(dirs: &Dirs, reg: &mut Registry, archive: &Path, mut meta: InstallMeta) -> Result<String> {
    fs::create_dir_all(&dirs.staging)?;
    clean_staging(&dirs.staging);
    let tmp = tempfile::Builder::new()
        .prefix("extract-")
        .tempdir_in(&dirs.staging)
        .context("creating staging dir")?;
    crate::archive::extract(archive, tmp.path())?;
    let plan = layout::plan(tmp.path(), &meta.name)?;

    let mut previous: Vec<String> = meta.replaces.iter().filter(|id| reg.get(id).is_some()).cloned().collect();
    if let Some(same) = reg.find_same(&meta.name, meta.nexus.as_ref())
        && !previous.contains(&same)
    {
        previous.push(same);
    }
    for old in previous {
        // Keep a Nexus link the user set by hand; only the mod id still applies.
        if meta.nexus.is_none() {
            meta.nexus = reg.get(&old).and_then(|m| m.nexus.as_ref()).map(|n| NexusRef { mod_id: n.mod_id, file_id: None });
        }
        remove(dirs, reg, &old).with_context(|| format!("removing previous version of {}", meta.name))?;
    }

    let id = reg.unique_id(&meta.name);
    let base = dirs.holding.join(&id);
    if base.exists() {
        // Leftover from an interrupted run; no registry entry points at it.
        fs::remove_dir_all(&base)?;
    }
    let mut files = Vec::with_capacity(plan.len());
    for (rel, src) in plan {
        let rel = fix_case(&dirs.game, &rel);
        move_file(&src, &safe_join(&base, &rel)?)?;
        files.push(rel);
    }
    files.sort();
    files.dedup();

    reg.mods.push(ModEntry {
        id: id.clone(),
        name: meta.name,
        version: meta.version,
        nexus: meta.nexus,
        github: meta.github,
        archive: meta.archive,
        enabled: false,
        files,
        overridden: Vec::new(),
        backed_up: Vec::new(),
        installed_at: now(),
    });
    reg.save()?;
    Ok(id)
}

/// Delete extraction leftovers from earlier failed or interrupted installs.
/// Installs run one at a time, so nothing else is using these.
fn clean_staging(staging: &Path) {
    let Ok(entries) = fs::read_dir(staging) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let _ = crate::archive::make_owner_writable(&path);
        let _ = fs::remove_dir_all(&path);
    }
}

/// Files of `id` that another enabled mod currently provides.
pub fn conflicts_for(reg: &Registry, id: &str) -> Vec<Conflict> {
    let Some(m) = reg.get(id) else { return Vec::new() };
    m.files
        .iter()
        .filter_map(|rel| {
            reg.live_owner(rel, id).map(|o| Conflict {
                file: rel.clone(),
                other_id: o.id.clone(),
                other_name: o.name.clone(),
            })
        })
        .collect()
}

pub fn enable(dirs: &Dirs, reg: &mut Registry, id: &str, overwrite: bool) -> Result<EnableOutcome> {
    let Some(m) = reg.get(id).cloned() else { bail!("no mod with id {id}") };
    if m.enabled {
        return Ok(EnableOutcome::Enabled { overrode: Vec::new() });
    }
    let conflicts = conflicts_for(reg, id);
    if !conflicts.is_empty() && !overwrite {
        return Ok(EnableOutcome::Conflicts(conflicts));
    }

    let mine = dirs.holding.join(id);
    let mut journal = Journal::default();
    let mut backed_up = Vec::new();
    let mut parked: Vec<(String, String)> = Vec::new(); // (other mod id, rel)
    let result = (|| -> Result<()> {
        for rel in &m.files {
            let src = safe_join(&mine, rel)?;
            let dst = safe_join(&dirs.game, rel)?;
            if !src.exists() {
                eprintln!("{}: missing {rel} in holding, skipping", m.name);
                continue;
            }
            if let Some(owner) = reg.live_owner(rel, id) {
                journal.mv(dst.clone(), safe_join(&dirs.holding.join(&owner.id), rel)?)?;
                parked.push((owner.id.clone(), rel.clone()));
            } else if dst.exists() && layout::is_mod_only_path(rel) {
                // Leftover from a manual install: set aside, don't bring it back later.
                journal.mv(dst.clone(), safe_join(&dirs.replaced, rel)?)?;
            } else if dst.exists() {
                journal.mv(dst.clone(), safe_join(&dirs.backups.join(id), rel)?)?;
                backed_up.push(rel.clone());
            }
            journal.mv(src, dst)?;
        }
        Ok(())
    })();
    if let Err(e) = result {
        journal.rollback();
        return Err(e);
    }

    for (other, rel) in parked {
        if let Some(o) = reg.get_mut(&other) {
            o.overridden.push(rel);
        }
    }
    let entry = reg.get_mut(id).unwrap();
    entry.enabled = true;
    entry.overridden.clear();
    entry.backed_up = backed_up;
    remove_empty_tree(&mine);
    reg.save()?;
    Ok(EnableOutcome::Enabled { overrode: conflicts })
}

pub fn disable(dirs: &Dirs, reg: &mut Registry, id: &str) -> Result<()> {
    let Some(m) = reg.get(id).cloned() else { bail!("no mod with id {id}") };
    if !m.enabled {
        return Ok(());
    }
    let mine = dirs.holding.join(id);
    let mut journal = Journal::default();
    let mut restored: Vec<(String, String)> = Vec::new(); // (other mod id, rel) back in game
    let mut backup_transfers: Vec<(String, String)> = Vec::new(); // (live owner id, rel)
    let mut emptied: Vec<PathBuf> = Vec::new();
    let result = (|| -> Result<()> {
        for rel in &m.files {
            let game_path = safe_join(&dirs.game, rel)?;
            if m.overridden.contains(rel) {
                // Our copy is already parked. If we were holding the original
                // game file's backup, the mod now on top inherits it.
                if m.backed_up.contains(rel)
                    && let Some(live) = reg.live_owner(rel, id)
                {
                    journal.mv(
                        safe_join(&dirs.backups.join(id), rel)?,
                        safe_join(&dirs.backups.join(&live.id), rel)?,
                    )?;
                    backup_transfers.push((live.id.clone(), rel.clone()));
                }
                continue;
            }
            if game_path.exists() {
                journal.mv(game_path.clone(), safe_join(&mine, rel)?)?;
            }
            if let Some(other) = reg.overridden_owner(rel, id) {
                journal.mv(safe_join(&dirs.holding.join(&other), rel)?, game_path)?;
                restored.push((other, rel.clone()));
            } else if m.backed_up.contains(rel) {
                journal.mv(safe_join(&dirs.backups.join(id), rel)?, game_path)?;
            } else if let Some(parent) = game_path.parent() {
                emptied.push(parent.to_path_buf());
            }
        }
        Ok(())
    })();
    if let Err(e) = result {
        journal.rollback();
        return Err(e);
    }

    for dir in emptied {
        prune_empty_dirs(&dir, &dirs.game, PROTECTED_DIRS);
    }
    for (other, rel) in restored {
        if let Some(o) = reg.get_mut(&other) {
            o.overridden.retain(|f| *f != rel);
        }
    }
    for (live, rel) in backup_transfers {
        if let Some(o) = reg.get_mut(&live) {
            o.backed_up.push(rel);
        }
    }
    let entry = reg.get_mut(id).unwrap();
    entry.enabled = false;
    entry.overridden.clear();
    entry.backed_up.clear();
    remove_empty_tree(&dirs.backups.join(id));
    reg.save()?;
    Ok(())
}

pub fn remove(dirs: &Dirs, reg: &mut Registry, id: &str) -> Result<()> {
    disable(dirs, reg, id)?;
    let mine = dirs.holding.join(id);
    if mine.exists() {
        fs::remove_dir_all(&mine).with_context(|| format!("deleting {}", mine.display()))?;
    }
    remove_empty_tree(&dirs.backups.join(id));
    reg.mods.retain(|m| m.id != id);
    reg.save()
}

/// Files in the game's mod folders that no managed mod accounts for, grouped
/// into likely mods.
pub fn scan_untracked(dirs: &Dirs, reg: &Registry) -> Vec<(String, Vec<String>)> {
    let tracked: std::collections::HashSet<String> =
        reg.mods.iter().filter(|m| m.enabled).flat_map(|m| m.files.iter().map(|f| f.to_lowercase())).collect();
    let mut files = Vec::new();
    for root in layout::mod_only_roots() {
        for entry in walkdir::WalkDir::new(dirs.game.join(root)).into_iter().flatten() {
            if !entry.file_type().is_file() {
                continue;
            }
            let Ok(rel) = entry.path().strip_prefix(&dirs.game) else { continue };
            let rel = rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/");
            if layout::is_mod_only_path(&rel) && !tracked.contains(&rel.to_lowercase()) {
                files.push(rel);
            }
        }
    }
    files.sort();
    files.dedup();
    // Logs/settings a managed mod wrote into its own folder aren't a separate mod.
    // (`tracked` is lowercased, so compare group names case-insensitively.)
    let tracked: Vec<String> = tracked.into_iter().collect();
    let managed_groups: std::collections::HashSet<String> =
        layout::group_untracked(&tracked).into_keys().map(|k| k.to_lowercase()).collect();
    layout::group_untracked(&files)
        .into_iter()
        .filter(|(name, _)| name == layout::LOADERS_GROUP || !managed_groups.contains(&name.to_lowercase()))
        .collect()
}

/// Register files already in the game folder as an enabled mod, so they can be
/// disabled or removed like anything else. Returns the new id.
pub fn adopt(reg: &mut Registry, name: &str, mut files: Vec<String>) -> Result<String> {
    // Never claim a file another mod already owns.
    files.retain(|f| reg.live_owner(f, "").is_none());
    if files.is_empty() {
        bail!("nothing left to adopt for {name}");
    }
    let id = reg.unique_id(name);
    reg.mods.push(ModEntry {
        id: id.clone(),
        name: name.to_string(),
        version: None,
        nexus: None,
        github: None,
        archive: None,
        enabled: true,
        files,
        overridden: Vec::new(),
        backed_up: Vec::new(),
        installed_at: now(),
    });
    reg.save()?;
    Ok(id)
}

/// Move every disabled/parked mod folder to a new holding dir.
pub fn relocate_holding(reg: &Registry, from: &Path, to: &Path) -> Result<()> {
    if from == to {
        return Ok(());
    }
    for m in &reg.mods {
        let src = from.join(&m.id);
        if !src.exists() {
            continue;
        }
        for entry in walkdir::WalkDir::new(&src).into_iter().flatten() {
            if entry.file_type().is_file() {
                let rel = entry.path().strip_prefix(&src).unwrap();
                move_file(entry.path(), &to.join(&m.id).join(rel))?;
            }
        }
        remove_empty_tree(&src);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    struct Env {
        _tmp: tempfile::TempDir,
        dirs: Dirs,
        reg: Registry,
        archives: PathBuf,
    }

    fn env() -> Env {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let dirs = Dirs {
            game: root.join("game"),
            holding: root.join("holding"),
            backups: root.join("backups"),
            staging: root.join("staging"),
            replaced: root.join("replaced"),
        };
        fs::create_dir_all(dirs.game.join("archive/pc/mod")).unwrap();
        fs::create_dir_all(dirs.game.join("archive/pc/content")).unwrap();
        fs::write(dirs.game.join("archive/pc/content/basegame.archive"), "vanilla").unwrap();
        let reg = Registry::load(&root.join("mods.json")).unwrap();
        let archives = root.join("archives");
        fs::create_dir_all(&archives).unwrap();
        Env { _tmp: tmp, dirs, reg, archives }
    }

    fn make_zip(env: &Env, name: &str, files: &[(&str, &str)]) -> PathBuf {
        let path = env.archives.join(name);
        let mut w = zip::ZipWriter::new(fs::File::create(&path).unwrap());
        for (rel, body) in files {
            w.start_file(*rel, zip::write::SimpleFileOptions::default()).unwrap();
            w.write_all(body.as_bytes()).unwrap();
        }
        w.finish().unwrap();
        path
    }

    fn read(env: &Env, rel: &str) -> Option<String> {
        fs::read_to_string(env.dirs.game.join(rel)).ok()
    }

    fn install_enable(env: &mut Env, zip: &Path, overwrite: bool) -> (String, EnableOutcome) {
        let id = install(&env.dirs, &mut env.reg, zip, InstallMeta::from_archive_name(zip)).unwrap();
        let out = enable(&env.dirs, &mut env.reg, &id, overwrite).unwrap();
        (id, out)
    }

    #[test]
    fn archive_name_parsing() {
        let m = InstallMeta::from_archive_name(Path::new("Cyber Engine Tweaks-107-1-32-3-1720000000.zip"));
        assert_eq!(m.name, "Cyber Engine Tweaks");
        assert_eq!(m.nexus, Some(NexusRef { mod_id: 107, file_id: None }));
        assert_eq!(m.version.as_deref(), Some("1.32.3"));
        let m = InstallMeta::from_archive_name(Path::new("my-cool-mod.7z"));
        assert_eq!(m.name, "my-cool-mod");
        assert!(m.nexus.is_none());
    }

    #[test]
    fn install_disable_enable_remove_roundtrip() {
        let mut env = env();
        let zip = make_zip(&env, "Neat-1-1-0-5.zip", &[
            ("Neat/archive/pc/mod/neat.archive", "neat"),
            ("Neat/r6/scripts/Neat/neat.reds", "reds"),
        ]);
        let (id, out) = install_enable(&mut env, &zip, false);
        assert!(matches!(out, EnableOutcome::Enabled { .. }));
        assert_eq!(read(&env, "archive/pc/mod/neat.archive").as_deref(), Some("neat"));
        assert_eq!(read(&env, "r6/scripts/Neat/neat.reds").as_deref(), Some("reds"));
        assert!(!env.dirs.holding.join(&id).exists());

        disable(&env.dirs, &mut env.reg, &id).unwrap();
        assert!(read(&env, "archive/pc/mod/neat.archive").is_none());
        assert!(!env.dirs.game.join("r6/scripts/Neat").exists(), "empty mod dir pruned");
        assert!(env.dirs.game.join("r6/scripts").exists(), "protected dir kept");
        assert!(env.dirs.game.join("archive/pc/mod").exists());
        assert!(env.dirs.holding.join(&id).join("archive/pc/mod/neat.archive").is_file());

        enable(&env.dirs, &mut env.reg, &id, false).unwrap();
        assert_eq!(read(&env, "archive/pc/mod/neat.archive").as_deref(), Some("neat"));

        remove(&env.dirs, &mut env.reg, &id).unwrap();
        assert!(read(&env, "archive/pc/mod/neat.archive").is_none());
        assert!(!env.dirs.holding.join(&id).exists());
        assert!(env.reg.mods.is_empty());
        // Persisted.
        assert!(Registry::load(&env.dirs.game.parent().unwrap().join("mods.json")).unwrap().mods.is_empty());
    }

    #[test]
    fn conflicts_park_and_restore() {
        let mut env = env();
        let a = make_zip(&env, "A.zip", &[("archive/pc/mod/shared.archive", "A"), ("archive/pc/mod/a.archive", "a")]);
        let b = make_zip(&env, "B.zip", &[("archive/pc/mod/shared.archive", "B")]);
        let (a_id, _) = install_enable(&mut env, &a, false);
        let (b_id, out) = install_enable(&mut env, &b, false);
        match out {
            EnableOutcome::Conflicts(c) => assert_eq!(c[0].other_id, a_id),
            other => panic!("expected conflict, got {other:?}"),
        }
        assert_eq!(read(&env, "archive/pc/mod/shared.archive").as_deref(), Some("A"));

        enable(&env.dirs, &mut env.reg, &b_id, true).unwrap();
        assert_eq!(read(&env, "archive/pc/mod/shared.archive").as_deref(), Some("B"));

        // Removing B brings A's copy back.
        remove(&env.dirs, &mut env.reg, &b_id).unwrap();
        assert_eq!(read(&env, "archive/pc/mod/shared.archive").as_deref(), Some("A"));
        assert!(env.reg.get(&a_id).unwrap().overridden.is_empty());

        disable(&env.dirs, &mut env.reg, &a_id).unwrap();
        assert!(read(&env, "archive/pc/mod/shared.archive").is_none());
    }

    #[test]
    fn vanilla_file_backed_up_and_restored() {
        let mut env = env();
        let a = make_zip(&env, "A.zip", &[("archive/pc/content/basegame.archive", "A")]);
        let b = make_zip(&env, "B.zip", &[("archive/pc/content/basegame.archive", "B")]);
        let (a_id, _) = install_enable(&mut env, &a, false);
        assert_eq!(read(&env, "archive/pc/content/basegame.archive").as_deref(), Some("A"));
        let (b_id, _) = install_enable(&mut env, &b, true);
        assert_eq!(read(&env, "archive/pc/content/basegame.archive").as_deref(), Some("B"));

        // A goes away while overridden: the vanilla backup moves to B.
        remove(&env.dirs, &mut env.reg, &a_id).unwrap();
        assert_eq!(read(&env, "archive/pc/content/basegame.archive").as_deref(), Some("B"));
        remove(&env.dirs, &mut env.reg, &b_id).unwrap();
        assert_eq!(read(&env, "archive/pc/content/basegame.archive").as_deref(), Some("vanilla"));
    }

    #[test]
    fn manual_leftovers_are_not_restored() {
        let mut env = env();
        // Pretend these came from an old manual install.
        fs::create_dir_all(env.dirs.game.join("bin/x64")).unwrap();
        fs::write(env.dirs.game.join("bin/x64/winmm.dll"), "old").unwrap();
        fs::write(env.dirs.game.join("archive/pc/mod/thing.archive"), "old").unwrap();
        let zip = make_zip(&env, "Loader.zip", &[("bin/x64/winmm.dll", "new"), ("archive/pc/mod/thing.archive", "new")]);
        let (id, _) = install_enable(&mut env, &zip, false);
        assert_eq!(read(&env, "bin/x64/winmm.dll").as_deref(), Some("new"));
        assert!(env.reg.get(&id).unwrap().backed_up.is_empty());

        remove(&env.dirs, &mut env.reg, &id).unwrap();
        assert!(read(&env, "bin/x64/winmm.dll").is_none(), "old manual copy must not come back");
        assert!(read(&env, "archive/pc/mod/thing.archive").is_none());
        assert_eq!(fs::read_to_string(env.dirs.replaced.join("bin/x64/winmm.dll")).unwrap(), "old");
    }

    #[test]
    fn adopt_untracked_then_remove() {
        let mut env = env();
        let managed = make_zip(&env, "Managed.zip", &[("archive/pc/mod/managed.archive", "m")]);
        install_enable(&mut env, &managed, false);
        for rel in [
            "bin/x64/plugins/cyber_engine_tweaks/mods/Loot/init.lua",
            "r6/tweaks/Guns/a.yaml",
            "archive/pc/mod/Ragdoll.archive",
        ] {
            let p = env.dirs.game.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, "x").unwrap();
        }
        // A runtime file written into a managed mod's folder is not reported.
        let managed_cet =
            make_zip(&env, "Cet.zip", &[("bin/x64/plugins/cyber_engine_tweaks/mods/Managed/init.lua", "m")]);
        install_enable(&mut env, &managed_cet, false);
        fs::write(env.dirs.game.join("bin/x64/plugins/cyber_engine_tweaks/mods/Managed/db.sqlite3"), "").unwrap();
        let found = scan_untracked(&env.dirs, &env.reg);
        let names: Vec<&str> = found.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["CET mod: Loot", "Guns", "Ragdoll"], "managed.archive and vanilla files excluded");

        for (name, files) in found {
            adopt(&mut env.reg, &name, files).unwrap();
        }
        assert!(scan_untracked(&env.dirs, &env.reg).is_empty());
        let guns = env.reg.mods.iter().find(|m| m.name == "Guns").unwrap().id.clone();
        disable(&env.dirs, &mut env.reg, &guns).unwrap();
        assert!(read(&env, "r6/tweaks/Guns/a.yaml").is_none());
        assert!(env.dirs.holding.join(&guns).join("r6/tweaks/Guns/a.yaml").is_file());
        remove(&env.dirs, &mut env.reg, &guns).unwrap();
        assert!(!env.dirs.holding.join(&guns).exists());
    }

    #[test]
    fn reinstall_replaces_previous() {
        let mut env = env();
        let v1 = make_zip(&env, "Thing-5-1-0-100.zip", &[("archive/pc/mod/old.archive", "1")]);
        let v2 = make_zip(&env, "Thing-5-2-0-200.zip", &[("archive/pc/mod/new.archive", "2")]);
        install_enable(&mut env, &v1, false);
        install_enable(&mut env, &v2, false);
        assert_eq!(env.reg.mods.len(), 1);
        assert_eq!(env.reg.mods[0].version.as_deref(), Some("2.0"));
        assert!(read(&env, "archive/pc/mod/old.archive").is_none());
        assert_eq!(read(&env, "archive/pc/mod/new.archive").as_deref(), Some("2"));
    }

    #[test]
    fn reinstall_keeps_manual_nexus_link() {
        let mut env = env();
        let v1 = make_zip(&env, "Thing.zip", &[("archive/pc/mod/a.archive", "1")]);
        let (id, _) = install_enable(&mut env, &v1, false);
        env.reg.get_mut(&id).unwrap().nexus = Some(NexusRef { mod_id: 42, file_id: Some(7) });
        install_enable(&mut env, &v1, false);
        assert_eq!(env.reg.mods.len(), 1);
        assert_eq!(env.reg.mods[0].nexus, Some(NexusRef { mod_id: 42, file_id: None }));
    }

    #[test]
    fn replaces_differently_named_entries() {
        let mut env = env();
        let a = make_zip(&env, "TweakXL 1.11.4.zip", &[("red4ext/plugins/TweakXL/TweakXL.dll", "1")]);
        let b = make_zip(&env, "tweak copy.zip", &[("red4ext/plugins/TweakXL/TweakXL.dll", "1")]);
        let (first, _) = install_enable(&mut env, &a, false);
        let (second, _) = install_enable(&mut env, &b, true);
        let new = make_zip(&env, "TweakXL-1.11.5.zip", &[("red4ext/plugins/TweakXL/TweakXL.dll", "2")]);
        let meta = InstallMeta { name: "TweakXL".into(), replaces: vec![first, second], ..Default::default() };
        let id = install(&env.dirs, &mut env.reg, &new, meta).unwrap();
        enable(&env.dirs, &mut env.reg, &id, false).unwrap();
        assert_eq!(env.reg.mods.len(), 1);
        assert_eq!(fs::read_to_string(env.dirs.game.join("red4ext/plugins/TweakXL/TweakXL.dll")).unwrap(), "2");
    }

    #[test]
    fn bad_archive_leaves_state_untouched() {
        let mut env = env();
        let bad = make_zip(&env, "Bad.zip", &[("docs/readme.txt", "x")]);
        assert!(install(&env.dirs, &mut env.reg, &bad, InstallMeta::from_archive_name(&bad)).is_err());
        assert!(env.reg.mods.is_empty());
    }
}
