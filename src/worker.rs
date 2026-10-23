//! Background threads. All mod file operations run on one serial worker so
//! they never race; Nexus network calls run on their own threads and hand
//! finished downloads to the worker as install jobs.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};

use anyhow::{Context, Result};

use crate::nexus::{FileList, ModInfo, Nexus, User};
use crate::ops::{self, Conflict, Dirs, EnableOutcome, InstallMeta};
use crate::registry::{ModEntry, NexusRef, Registry};

pub enum Job {
    Install { item: u64, path: PathBuf, meta: InstallMeta, overwrite: bool, delete_after: bool },
    Enable { id: String, overwrite: bool },
    Disable { id: String },
    Remove { id: String },
    DisableAll,
    ScanUntracked,
    Adopt(Vec<(String, Vec<String>)>),
    SetDirs(Dirs),
    RelocateHolding { from: PathBuf, to: PathBuf },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ItemStatus {
    Queued,
    Downloading(f32),
    Installing,
    Done,
    NeedsDecision,
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone)]
pub struct UpdateInfo {
    pub entry_id: String,
    pub mod_id: u64,
    pub file_id: Option<u64>,
    pub version: Option<String>,
}

pub enum Event {
    Mods(Vec<ModEntry>),
    Item { item: u64, status: ItemStatus },
    Conflicts { id: String, name: String, conflicts: Vec<Conflict> },
    Log(Level, String),
    Nxm(String),
    NexusUser(Result<User, String>),
    NexusFiles { mod_id: u64, info: ModInfo, list: FileList },
    Updates(Vec<UpdateInfo>),
    /// Groups of untracked files found in the game's mod folders.
    Untracked(Vec<(String, Vec<String>)>),
}

/// Sends events to the UI and wakes it up.
#[derive(Clone)]
pub struct Notifier {
    tx: Sender<Event>,
    ctx: egui::Context,
}

impl Notifier {
    pub fn new(ctx: egui::Context) -> (Self, Receiver<Event>) {
        let (tx, rx) = channel();
        (Self { tx, ctx }, rx)
    }

    pub fn send(&self, ev: Event) {
        let _ = self.tx.send(ev);
        self.ctx.request_repaint();
    }

    pub fn log(&self, level: Level, msg: impl Into<String>) {
        self.send(Event::Log(level, msg.into()));
    }

    fn item(&self, item: u64, status: ItemStatus) {
        self.send(Event::Item { item, status });
    }
}

struct Worker {
    reg: Option<Registry>,
    dirs: Option<Dirs>,
    n: Notifier,
}

pub fn spawn(registry_file: PathBuf, dirs: Option<Dirs>, n: Notifier) -> Sender<Job> {
    let (tx, rx) = channel::<Job>();
    std::thread::spawn(move || {
        let reg = match Registry::load(&registry_file) {
            Ok(r) => Some(r),
            Err(e) => {
                n.log(Level::Error, format!("Could not load mod list, refusing to make changes: {e:#}"));
                None
            }
        };
        let mut w = Worker { reg, dirs, n };
        w.publish();
        for job in rx {
            if let Err(e) = w.handle(job) {
                w.n.log(Level::Error, format!("{e:#}"));
            }
            w.publish();
        }
    });
    tx
}

impl Worker {
    fn publish(&self) {
        if let Some(reg) = &self.reg {
            self.n.send(Event::Mods(reg.mods.clone()));
        }
    }

    fn ctx(&mut self) -> Result<(&Dirs, &mut Registry)> {
        let dirs = self.dirs.as_ref().context("Set the Cyberpunk 2077 game folder in Settings first")?;
        let reg = self.reg.as_mut().context("Mod list failed to load")?;
        Ok((dirs, reg))
    }

    fn name_of(&self, id: &str) -> String {
        self.reg.as_ref().and_then(|r| r.get(id)).map(|m| m.name.clone()).unwrap_or_else(|| id.to_string())
    }

    fn handle(&mut self, job: Job) -> Result<()> {
        match job {
            Job::SetDirs(dirs) => self.dirs = Some(dirs),
            Job::RelocateHolding { from, to } => {
                let reg = self.reg.as_ref().context("Mod list failed to load")?;
                ops::relocate_holding(reg, &from, &to)?;
                self.n.log(Level::Info, format!("Moved holding folder to {}", to.display()));
            }
            Job::Install { item, path, meta, overwrite, delete_after } => {
                self.n.item(item, ItemStatus::Installing);
                match self.install(&path, meta, overwrite) {
                    Ok(true) => {
                        self.n.item(item, ItemStatus::Done);
                        if delete_after {
                            let _ = std::fs::remove_file(&path);
                        }
                    }
                    Ok(false) => self.n.item(item, ItemStatus::NeedsDecision),
                    Err(e) => {
                        self.n.item(item, ItemStatus::Failed(format!("{e:#}")));
                        return Err(e.context(format!("installing {}", path.display())));
                    }
                }
            }
            Job::Enable { id, overwrite } => {
                self.enable(&id, overwrite)?;
            }
            Job::Disable { id } => {
                let (dirs, reg) = self.ctx()?;
                ops::disable(dirs, reg, &id)?;
                self.n.log(Level::Info, format!("Disabled {}", self.name_of(&id)));
            }
            Job::Remove { id } => {
                let name = self.name_of(&id);
                let (dirs, reg) = self.ctx()?;
                ops::remove(dirs, reg, &id)?;
                self.n.log(Level::Info, format!("Removed {name}"));
            }
            Job::ScanUntracked => {
                let (dirs, reg) = self.ctx()?;
                let found = ops::scan_untracked(dirs, reg);
                if found.is_empty() {
                    self.n.log(Level::Info, "No untracked mod files in the game folder.");
                }
                self.n.send(Event::Untracked(found));
            }
            Job::Adopt(groups) => {
                let reg = self.reg.as_mut().context("Mod list failed to load")?;
                let mut adopted = 0;
                for (name, files) in groups {
                    match ops::adopt(reg, &name, files) {
                        Ok(_) => adopted += 1,
                        Err(e) => self.n.log(Level::Warn, format!("{e:#}")),
                    }
                }
                self.n.log(Level::Info, format!("Adopted {adopted} untracked mod(s)"));
            }
            Job::DisableAll => {
                let (dirs, reg) = self.ctx()?;
                // Newest first so parked files unwind cleanly.
                let mut ids: Vec<(u64, String)> =
                    reg.mods.iter().filter(|m| m.enabled).map(|m| (m.installed_at, m.id.clone())).collect();
                ids.sort_by(|a, b| b.cmp(a));
                for (_, id) in &ids {
                    ops::disable(dirs, reg, id)?;
                }
                self.n.log(Level::Info, format!("Disabled {} mods", ids.len()));
            }
        }
        Ok(())
    }

    /// Returns false when the mod was installed but left disabled pending a conflict decision.
    fn install(&mut self, path: &std::path::Path, meta: InstallMeta, overwrite: bool) -> Result<bool> {
        let (dirs, reg) = self.ctx()?;
        let id = ops::install(dirs, reg, path, meta)?;
        self.enable(&id, overwrite)
    }

    fn enable(&mut self, id: &str, overwrite: bool) -> Result<bool> {
        let (dirs, reg) = self.ctx()?;
        let outcome = ops::enable(dirs, reg, id, overwrite)?;
        let name = self.name_of(id);
        match outcome {
            EnableOutcome::Enabled { overrode } => {
                let mut msg = format!("Enabled {name}");
                if !overrode.is_empty() {
                    let mut others: Vec<&str> = overrode.iter().map(|c| c.other_name.as_str()).collect();
                    others.dedup();
                    msg.push_str(&format!(" (overrides {} file(s) from {})", overrode.len(), others.join(", ")));
                }
                self.n.log(Level::Info, msg);
                Ok(true)
            }
            EnableOutcome::Conflicts(conflicts) => {
                self.n.send(Event::Conflicts { id: id.to_string(), name, conflicts });
                Ok(false)
            }
        }
    }
}

// ---- Nexus threads -------------------------------------------------------

pub struct NexusRequest {
    pub item: u64,
    pub mod_id: u64,
    pub file_id: u64,
    pub nxm_key: Option<(String, u64)>,
    /// Keep this name so the download replaces an existing install (updates).
    pub name: Option<String>,
}

pub struct InstallOpts {
    pub downloads: PathBuf,
    pub overwrite: bool,
    pub delete_after: bool,
}

pub fn spawn_nexus_install(api_key: String, req: NexusRequest, opts: InstallOpts, jobs: Sender<Job>, n: Notifier) {
    std::thread::spawn(move || {
        let item = req.item;
        let result = (|| -> Result<()> {
            let nx = Nexus::new(&api_key)?;
            n.item(item, ItemStatus::Downloading(0.0));
            let list = nx.files(req.mod_id)?;
            let file = list.files.iter().find(|f| f.file_id == req.file_id).cloned();
            let info = nx.mod_info(req.mod_id).ok();
            let url = nx.download_url(req.mod_id, req.file_id, req.nxm_key.as_ref().map(|(k, e)| (k.as_str(), *e)))?;
            let file_name = file
                .as_ref()
                .map(|f| f.file_name.clone())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| format!("{}-{}.zip", req.mod_id, req.file_id));
            let mut last = -1.0f32;
            let path = nx.download(&url, &opts.downloads, &file_name, |done, total| {
                if let Some(total) = total.filter(|t| *t > 0) {
                    let frac = done as f32 / total as f32;
                    if frac - last >= 0.01 {
                        last = frac;
                        n.item(item, ItemStatus::Downloading(frac));
                    }
                }
            })?;
            let mod_name = info.as_ref().and_then(|i| i.name.clone());
            let name = req.name.clone().unwrap_or_else(|| match (&mod_name, &file) {
                (Some(m), Some(f)) if f.category_name.as_deref() != Some("MAIN") && !f.name.is_empty() => {
                    format!("{m} - {}", f.name)
                }
                (Some(m), _) => m.clone(),
                (None, Some(f)) if !f.name.is_empty() => f.name.clone(),
                _ => InstallMeta::from_archive_name(&path).name,
            });
            let meta = InstallMeta {
                name,
                version: file.as_ref().and_then(|f| f.version.clone()).or(info.and_then(|i| i.version)),
                nexus: Some(NexusRef { mod_id: req.mod_id, file_id: Some(req.file_id) }),
                archive: Some(file_name),
            };
            jobs.send(Job::Install { item, path, meta, overwrite: opts.overwrite, delete_after: opts.delete_after })
                .ok()
                .context("worker stopped")?;
            Ok(())
        })();
        if let Err(e) = result {
            n.item(item, ItemStatus::Failed(format!("{e:#}")));
            n.log(Level::Error, format!("Nexus download failed: {e:#}"));
        }
    });
}

pub fn spawn_validate(api_key: String, n: Notifier) {
    std::thread::spawn(move || {
        let res = Nexus::new(&api_key).and_then(|nx| nx.validate()).map_err(|e| format!("{e:#}"));
        n.send(Event::NexusUser(res));
    });
}

pub fn spawn_fetch_files(api_key: String, mod_id: u64, n: Notifier) {
    std::thread::spawn(move || {
        let res = Nexus::new(&api_key).and_then(|nx| Ok((nx.mod_info(mod_id)?, nx.files(mod_id)?)));
        match res {
            Ok((info, list)) => n.send(Event::NexusFiles { mod_id, info, list }),
            Err(e) => n.log(Level::Error, format!("Fetching mod {mod_id} failed: {e:#}")),
        }
    });
}

pub fn spawn_update_check(api_key: String, mods: Vec<ModEntry>, n: Notifier) {
    std::thread::spawn(move || {
        let nx = match Nexus::new(&api_key) {
            Ok(nx) => nx,
            Err(e) => return n.log(Level::Error, format!("{e:#}")),
        };
        let mut updates = Vec::new();
        let mut checked = 0;
        for m in mods {
            let Some(nexus) = &m.nexus else { continue };
            checked += 1;
            let found = (|| -> Result<Option<UpdateInfo>> {
                let list = nx.files(nexus.mod_id)?;
                if let Some(fid) = nexus.file_id {
                    return Ok(list.newest_replacement(fid).map(|f| UpdateInfo {
                        entry_id: m.id.clone(),
                        mod_id: nexus.mod_id,
                        file_id: Some(f.file_id),
                        version: f.version.clone(),
                    }));
                }
                // Installed from a local archive: compare against the mod page version.
                let info = nx.mod_info(nexus.mod_id)?;
                let newer = match (&info.version, &m.version) {
                    (Some(latest), Some(mine)) => latest.trim_start_matches('v') != mine.trim_start_matches('v'),
                    _ => false,
                };
                Ok(newer.then(|| UpdateInfo {
                    entry_id: m.id.clone(),
                    mod_id: nexus.mod_id,
                    file_id: None,
                    version: info.version.clone(),
                }))
            })();
            match found {
                Ok(Some(u)) => updates.push(u),
                Ok(None) => {}
                Err(e) => n.log(Level::Warn, format!("Update check for {} failed: {e:#}", m.name)),
            }
        }
        n.log(Level::Info, format!("Checked {checked} Nexus mods, {} update(s) available", updates.len()));
        n.send(Event::Updates(updates));
    });
}
