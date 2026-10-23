mod frameworks_view;
mod install;
mod mods_view;
mod nexus_view;
mod settings;

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

use egui::{Color32, RichText};

use crate::config::Config;
use crate::nexus::{FileList, ModInfo, User};
use crate::nxm::NxmLink;
use crate::ops::{Conflict, Dirs};
use crate::paths::AppPaths;
use crate::registry::ModEntry;
use crate::worker::{self, Event, ItemStatus, Job, Level, Notifier, UpdateInfo, UpdateSource};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Mods,
    Install,
    Nexus,
    Settings,
    Log,
}

struct QueueItem {
    id: u64,
    label: String,
    status: ItemStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum FrameworkState {
    Managed { version: Option<String>, enabled: bool },
    /// Found in the game folder but installed outside the manager.
    Unmanaged,
    Missing,
}

struct PendingConflict {
    id: String,
    name: String,
    conflicts: Vec<Conflict>,
}

pub struct App {
    paths: AppPaths,
    config: Config,
    jobs: Sender<Job>,
    events: Receiver<Event>,
    notifier: Notifier,

    tab: Tab,
    mods: Vec<ModEntry>,
    selected: Option<String>,
    filter: String,
    confirm_remove: Option<String>,
    /// Scan results awaiting the user's pick: (name, files, selected).
    untracked: Option<Vec<(String, Vec<String>, bool)>>,
    conflicts: VecDeque<PendingConflict>,
    /// Framework picker: open flag and the ticked framework names.
    frameworks_open: bool,
    fw_picked: Vec<&'static str>,
    /// Nexus URL/ID typed into the details panel's "Link to Nexus" box.
    link_input: String,

    queue: Vec<QueueItem>,
    next_item: u64,

    log: Vec<(Level, String)>,
    status: Option<(Level, String)>,

    nexus_user: Option<User>,
    api_key_input: String,
    nexus_input: String,
    nexus_files: Option<(u64, ModInfo, FileList)>,
    updates: HashMap<String, UpdateInfo>,
    /// Run the startup update check once the mod list has loaded.
    startup_check_pending: bool,

    game_running: bool,
    last_game_check: Instant,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, nxm_link: Option<String>) -> anyhow::Result<Self> {
        let paths = AppPaths::discover()?;
        let (notifier, events) = Notifier::new(cc.egui_ctx.clone());
        let mut startup_log = Vec::new();

        let mut config = match Config::load(&paths.config_file) {
            Ok(c) => c,
            Err(e) => {
                startup_log.push((Level::Error, format!("Config unreadable, using defaults: {e:#}")));
                Config::default()
            }
        };
        if config.game_dir.is_none()
            && let Some(dir) = crate::game::detect()
        {
            startup_log.push((Level::Info, format!("Detected Cyberpunk 2077 at {}", dir.display())));
            config.game_dir = Some(dir);
            let _ = config.save(&paths.config_file);
        }

        let dirs = dirs_for(&paths, &config);
        let jobs = worker::spawn(paths.registry_file(), dirs, notifier.clone());

        let link_notifier = notifier.clone();
        if let Err(e) = crate::nxm::listen(move |link| link_notifier.send(Event::Nxm(link))) {
            startup_log.push((Level::Warn, format!("nxm:// links from the browser won't reach this window: {e:#}")));
        }
        if let Some(link) = nxm_link {
            notifier.send(Event::Nxm(link));
        }

        let startup_check = config.check_updates_on_start;
        let mut app = Self {
            api_key_input: config.nexus_api_key.clone().unwrap_or_default(),
            paths,
            config,
            jobs,
            events,
            notifier,
            tab: Tab::Mods,
            mods: Vec::new(),
            selected: None,
            filter: String::new(),
            confirm_remove: None,
            untracked: None,
            conflicts: VecDeque::new(),
            frameworks_open: false,
            fw_picked: Vec::new(),
            link_input: String::new(),
            queue: Vec::new(),
            next_item: 0,
            log: Vec::new(),
            status: None,
            nexus_user: None,
            nexus_input: String::new(),
            nexus_files: None,
            updates: HashMap::new(),
            startup_check_pending: startup_check,
            game_running: false,
            last_game_check: Instant::now() - Duration::from_secs(60),
        };
        for (level, msg) in startup_log {
            app.push_log(level, msg);
        }
        if app.config.game_dir.is_none() {
            app.tab = Tab::Settings;
            app.push_log(Level::Warn, "Couldn't find Cyberpunk 2077 automatically. Pick the game folder in Settings.");
        }
        if let Some(key) = app.config.nexus_api_key.clone() {
            worker::spawn_validate(key, app.notifier.clone());
        }
        Ok(app)
    }

    fn push_log(&mut self, level: Level, msg: impl Into<String>) {
        let msg = msg.into();
        self.status = Some((level, msg.clone()));
        self.log.push((level, msg));
    }

    fn send(&mut self, job: Job) {
        if self.jobs.send(job).is_err() {
            self.push_log(Level::Error, "Background worker stopped; please restart the app.");
        }
    }

    fn save_config(&mut self) {
        if let Err(e) = self.config.save(&self.paths.config_file) {
            self.push_log(Level::Error, format!("Saving settings failed: {e:#}"));
        }
    }

    fn holding_dir(&self) -> PathBuf {
        self.config.holding_dir.clone().unwrap_or_else(|| self.paths.default_holding_dir())
    }

    fn new_item(&mut self, label: String) -> u64 {
        let id = self.next_item;
        self.next_item += 1;
        self.queue.push(QueueItem { id, label, status: ItemStatus::Queued });
        id
    }

    fn api_key(&mut self) -> Option<String> {
        let key = self.config.nexus_api_key.clone().filter(|k| !k.trim().is_empty());
        if key.is_none() {
            self.tab = Tab::Nexus;
            self.push_log(Level::Warn, "Add your Nexus API key first.");
        }
        key
    }

    fn handle_events(&mut self) {
        while let Ok(ev) = self.events.try_recv() {
            match ev {
                Event::Mods(mods) => {
                    // An update counts as applied once the entry is replaced or its version changes.
                    self.updates.retain(|id, u| {
                        mods.iter().any(|m| &m.id == id && (u.version.is_none() || m.version != u.version))
                    });
                    self.mods = mods;
                    if std::mem::take(&mut self.startup_check_pending) {
                        self.check_updates(false);
                    }
                }
                Event::Item { item, status } => {
                    if let Some(q) = self.queue.iter_mut().find(|q| q.id == item) {
                        q.status = status;
                    }
                }
                Event::Conflicts { id, name, conflicts } => {
                    if self.config.always_overwrite {
                        self.send(Job::Enable { id, overwrite: true });
                    } else {
                        self.conflicts.push_back(PendingConflict { id, name, conflicts });
                    }
                }
                Event::Log(level, msg) => self.push_log(level, msg),
                Event::Nxm(link) => self.handle_nxm(&link),
                Event::NexusUser(res) => match res {
                    Ok(user) => {
                        self.push_log(
                            Level::Info,
                            format!("Nexus: signed in as {}{}", user.name, if user.is_premium { " (Premium)" } else { "" }),
                        );
                        self.nexus_user = Some(user);
                    }
                    Err(e) => {
                        self.nexus_user = None;
                        self.push_log(Level::Error, format!("Nexus API key rejected: {e}"));
                    }
                },
                Event::NexusFiles { mod_id, info, list } => self.nexus_files = Some((mod_id, info, list)),
                Event::Untracked(groups) => {
                    if !groups.is_empty() {
                        self.untracked = Some(groups.into_iter().map(|(n, f)| (n, f, true)).collect());
                    }
                }
                Event::Updates(list) => {
                    self.updates = list.into_iter().map(|u| (u.entry_id.clone(), u)).collect();
                }
            }
        }
    }

    fn handle_nxm(&mut self, link: &str) {
        let parsed = match NxmLink::parse(link) {
            Ok(l) => l,
            Err(e) => return self.push_log(Level::Error, format!("Ignoring link {link}: {e:#}")),
        };
        let Some(key) = self.api_key() else { return };
        // A download for a mod we already have keeps its name so it replaces it.
        let name = self
            .mods
            .iter()
            .find(|m| m.nexus.as_ref().is_some_and(|n| n.mod_id == parsed.mod_id && n.file_id.is_some())
                && self.updates.get(&m.id).is_some_and(|u| match u.source {
                    UpdateSource::Nexus { file_id, .. } => file_id.is_none_or(|f| f == parsed.file_id),
                    UpdateSource::Github(_) => false,
                }))
            .map(|m| m.name.clone());
        let item = self.new_item(format!("Nexus mod {} / file {}", parsed.mod_id, parsed.file_id));
        self.start_nexus_install(key, worker::NexusRequest {
            item,
            mod_id: parsed.mod_id,
            file_id: parsed.file_id,
            nxm_key: parsed.key.zip(parsed.expires),
            name,
        });
        self.tab = Tab::Install;
    }

    fn start_nexus_install(&mut self, key: String, req: worker::NexusRequest) {
        worker::spawn_nexus_install(key, req, self.install_opts(), self.jobs.clone(), self.notifier.clone());
    }

    /// `manual` logs a hint when there is nothing that can be checked.
    fn check_updates(&mut self, manual: bool) {
        let key = self.config.nexus_api_key.clone().filter(|k| !k.trim().is_empty());
        let checkable = self.mods.iter().any(|m| m.github.is_some() || (m.nexus.is_some() && key.is_some()));
        if !checkable {
            if manual {
                self.push_log(
                    Level::Info,
                    "Nothing to check: install frameworks here, install from Nexus, or link mods to their Nexus page \
                     (Nexus checks also need your API key).",
                );
            }
            return;
        }
        worker::spawn_update_check(key, self.mods.clone(), self.notifier.clone());
        self.push_log(Level::Info, "Checking for updates…");
    }

    fn install_opts(&self) -> worker::InstallOpts {
        worker::InstallOpts {
            downloads: self.paths.downloads_dir(),
            overwrite: self.config.always_overwrite,
            delete_after: self.config.delete_downloads,
        }
    }

    /// Install (or update) the named frameworks plus whatever they need that
    /// isn't installed yet.
    fn install_frameworks(&mut self, picked: &[&str]) {
        let have: Vec<&str> = crate::frameworks::CATALOG
            .iter()
            .filter(|fw| self.framework_state(fw) != FrameworkState::Missing)
            .map(|fw| fw.name)
            .collect();
        let order = crate::frameworks::install_order(picked, &have);
        if order.is_empty() {
            return;
        }
        let items: Vec<_> = order.into_iter().map(|fw| self.framework_job(format!("{} (GitHub)", fw.name), fw)).collect();
        worker::spawn_framework_install(items, self.install_opts(), self.jobs.clone(), self.notifier.clone());
        self.tab = Tab::Install;
    }

    fn framework_job(&mut self, label: String, fw: &'static crate::frameworks::Framework) -> worker::FrameworkJob {
        let existing = crate::frameworks::owners(&self.mods, fw).into_iter().cloned().collect();
        worker::FrameworkJob { item: self.new_item(label), fw, existing }
    }

    fn framework_state(&self, fw: &crate::frameworks::Framework) -> FrameworkState {
        match crate::frameworks::owners(&self.mods, fw).first() {
            Some(m) => FrameworkState::Managed { version: m.version.clone(), enabled: m.enabled },
            None if self.config.game_dir.as_ref().is_some_and(|g| crate::frameworks::is_present(g, fw)) => {
                FrameworkState::Unmanaged
            }
            None => FrameworkState::Missing,
        }
    }

    /// Start the update for one mod. Returns false when it needs a manual
    /// download on Nexus (free accounts / unknown file), in which case the
    /// files page is opened if `open_page` is set.
    fn apply_update(&mut self, ctx: &egui::Context, entry_id: &str, open_page: bool) -> bool {
        let Some(u) = self.updates.get(entry_id).cloned() else { return false };
        let name = self.mods.iter().find(|m| m.id == entry_id).map(|m| m.name.clone());
        match u.source {
            UpdateSource::Github(fw) => {
                let job = self.framework_job(format!("Update {} (GitHub)", fw.name), fw);
                worker::spawn_framework_install(vec![job], self.install_opts(), self.jobs.clone(), self.notifier.clone());
                self.tab = Tab::Install;
                true
            }
            UpdateSource::Nexus { mod_id, file_id: Some(file_id) }
                if self.nexus_user.as_ref().is_some_and(|u| u.is_premium) =>
            {
                let Some(key) = self.api_key() else { return false };
                let item = self.new_item(format!("Update {}", name.clone().unwrap_or_default()));
                self.start_nexus_install(key, worker::NexusRequest { item, mod_id, file_id, nxm_key: None, name });
                self.tab = Tab::Install;
                true
            }
            UpdateSource::Nexus { mod_id, .. } => {
                if open_page {
                    ctx.open_url(egui::OpenUrl::new_tab(format!("{}?tab=files", mods_view::nexus_page(mod_id))));
                }
                false
            }
        }
    }

    fn conflict_dialog(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.conflicts.front() else { return };
        let mut choice = None;
        egui::Modal::new(egui::Id::unique("conflict")).show(ctx, |ui| {
            let body = fit_modal(ui, 560.0);
            ui.heading(format!("{} conflicts with installed mods", pending.name));
            egui::ScrollArea::vertical().max_height(body).show(ui, |ui| {
                ui.label("These files are already provided by another enabled mod:");
                for c in &pending.conflicts {
                    ui.label(format!("{}  ←  {}", c.file, c.other_name));
                }
            });
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                if ui.button("Overwrite (new mod wins)").clicked() {
                    choice = Some(0);
                }
                if ui.button("Keep it disabled").clicked() {
                    choice = Some(1);
                }
                if ui.button("Cancel install").clicked() {
                    choice = Some(2);
                }
            });
            ui.small("Overwritten files are kept and come back when this mod is disabled or removed.");
        });
        if let Some(choice) = choice {
            let p = self.conflicts.pop_front().unwrap();
            match choice {
                0 => self.send(Job::Enable { id: p.id, overwrite: true }),
                1 => self.push_log(Level::Info, format!("{} installed but left disabled", p.name)),
                _ => self.send(Job::Remove { id: p.id }),
            }
        }
    }

    fn remove_dialog(&mut self, ctx: &egui::Context) {
        let Some(id) = self.confirm_remove.clone() else { return };
        let name = self.mods.iter().find(|m| m.id == id).map(|m| m.name.clone()).unwrap_or(id.clone());
        let mut close = false;
        egui::Modal::new(egui::Id::unique("confirm-remove")).show(ctx, |ui| {
            fit_modal(ui, 480.0);
            ui.heading("Remove mod?");
            ui.label(format!("{name} will be deleted from the game folder and the holding folder."));
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                if ui.button(RichText::new("Remove").color(ui.visuals().error_fg_color)).clicked() {
                    self.send(Job::Remove { id: id.clone() });
                    if self.selected.as_ref() == Some(&id) {
                        self.selected = None;
                    }
                    close = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
        if close {
            self.confirm_remove = None;
        }
    }

    fn untracked_dialog(&mut self, ctx: &egui::Context) {
        let Some(groups) = self.untracked.as_mut() else { return };
        let mut decision = None;
        egui::Modal::new(egui::Id::unique("untracked")).show(ctx, |ui| {
            let body = fit_modal(ui, 620.0);
            ui.heading("Untracked mod files");
            egui::ScrollArea::vertical().max_height(body).show(ui, |ui| {
                ui.label(
                    "These files are in the game's mod folders but the manager doesn't know about them \
                     (e.g. mods installed by hand). Adopt them to disable or remove them like any other mod.",
                );
                ui.horizontal(|ui| {
                    if ui.small_button("Select all").clicked() {
                        groups.iter_mut().for_each(|g| g.2 = true);
                    }
                    if ui.small_button("Select none").clicked() {
                        groups.iter_mut().for_each(|g| g.2 = false);
                    }
                });
                for (name, files, selected) in groups.iter_mut() {
                    ui.checkbox(selected, format!("{name}  ({} files)", files.len()))
                        .on_hover_text(files.iter().take(15).cloned().collect::<Vec<_>>().join("\n"));
                }
            });
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                if ui.button("Adopt selected").clicked() {
                    decision = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    decision = Some(false);
                }
            });
        });
        if let Some(adopt) = decision {
            let groups = self.untracked.take().unwrap_or_default();
            if adopt {
                let picked: Vec<_> = groups.into_iter().filter(|g| g.2).map(|(n, f, _)| (n, f)).collect();
                if !picked.is_empty() {
                    self.send(Job::Adopt(picked));
                }
            }
        }
    }

    fn handle_drops(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> =
            ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        if !dropped.is_empty() {
            self.queue_archives(dropped);
        }
    }

    fn check_game_running(&mut self) {
        if self.last_game_check.elapsed() > Duration::from_secs(5) {
            self.last_game_check = Instant::now();
            self.game_running = crate::game::is_running();
        }
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_events();
        self.handle_drops(ctx);
        self.check_game_running();
        // Keep polling the game process occasionally even when idle.
        ctx.request_repaint_after(Duration::from_secs(5));
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        egui::Panel::top("tabs").show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                ui.heading(RichText::new("Cyber Mod Manager").color(Color32::from_rgb(252, 238, 10)));
                ui.separator();
                let enabled = self.mods.iter().filter(|m| m.enabled).count();
                ui.selectable_value(&mut self.tab, Tab::Mods, format!("Mods ({enabled}/{})", self.mods.len()));
                let active = self.queue.iter().filter(|q| !matches!(q.status, ItemStatus::Done | ItemStatus::Skipped(_) | ItemStatus::Failed(_))).count();
                let install_label = if active > 0 { format!("Install ({active})") } else { "Install".into() };
                ui.selectable_value(&mut self.tab, Tab::Install, install_label);
                let nexus_label = if self.updates.is_empty() { "Nexus".to_string() } else { format!("Nexus ({} updates)", self.updates.len()) };
                ui.selectable_value(&mut self.tab, Tab::Nexus, nexus_label);
                ui.selectable_value(&mut self.tab, Tab::Settings, "Settings");
                ui.selectable_value(&mut self.tab, Tab::Log, "Log");
            });
            if self.game_running {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    "Cyberpunk 2077 is running. Changes take effect after you restart the game.",
                );
            }
            ui.add_space(2.0);
        });

        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| match &self.status {
                Some((level, msg)) => {
                    // Long messages are cut off here; the Log tab has them in full.
                    ui.add(egui::Label::new(RichText::new(msg).color(level_color(ui, *level))).truncate())
                        .on_hover_text(msg);
                }
                None => {
                    ui.weak("Drop .zip / .7z / .rar files anywhere to install them.");
                }
            });
        });

        egui::CentralPanel::default().show(ui, |ui| match self.tab {
            Tab::Mods => self.mods_tab(ui),
            Tab::Install => self.install_tab(ui),
            Tab::Nexus => self.nexus_tab(ui),
            Tab::Settings => self.settings_tab(ui),
            Tab::Log => self.log_tab(ui),
        });

        self.conflict_dialog(&ctx);
        self.remove_dialog(&ctx);
        self.untracked_dialog(&ctx);
        self.frameworks_dialog(&ctx);
    }
}

impl App {
    fn log_tab(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("Clear").clicked() {
                self.log.clear();
            }
        });
        ui.separator();
        egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink(false).show(ui, |ui| {
            for (level, msg) in &self.log {
                ui.colored_label(level_color(ui, *level), msg);
            }
        });
    }
}

/// Fit a modal to the window: caps its width at `width` and returns the
/// height left for its scrolling body after the heading and button row.
fn fit_modal(ui: &mut egui::Ui, width: f32) -> f32 {
    let screen = ui.ctx().content_rect();
    ui.set_max_width(width.min(screen.width() - 64.0).max(240.0));
    (screen.height() - 150.0).max(80.0)
}

fn level_color(ui: &egui::Ui, level: Level) -> Color32 {
    match level {
        Level::Info => ui.visuals().text_color(),
        Level::Warn => ui.visuals().warn_fg_color,
        Level::Error => ui.visuals().error_fg_color,
    }
}

fn dirs_for(paths: &AppPaths, config: &Config) -> Option<Dirs> {
    Some(Dirs {
        game: config.game_dir.clone()?,
        holding: config.holding_dir.clone().unwrap_or_else(|| paths.default_holding_dir()),
        backups: paths.backups_dir(),
        staging: paths.staging_dir(),
        replaced: paths.replaced_dir(),
    })
}
