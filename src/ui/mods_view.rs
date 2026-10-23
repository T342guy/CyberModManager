use egui::RichText;

use super::App;
use crate::worker::{Job, Level, UpdateSource};

impl App {
    pub(super) fn mods_tab(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            if ui.button("➕ Add archives…").clicked() {
                self.pick_archives();
            }
            if ui.button("🧩 Frameworks…").on_hover_text("Install CET, RED4ext, redscript, TweakXL, ArchiveXL, …").clicked() {
                self.open_frameworks();
            }
            if ui.button("⟳ Check for updates").clicked() {
                self.check_updates(true);
            }
            if !self.updates.is_empty() && ui.button(format!("⬆ Update all ({})", self.updates.len())).clicked() {
                self.update_all(ui.ctx());
            }
            if ui.button("🔍 Find untracked mods…").on_hover_text("Adopt mods that were installed by hand").clicked() {
                self.send(Job::ScanUntracked);
            }
            if ui.button("Disable all").clicked() {
                self.send(Job::DisableAll);
            }
            ui.separator();
            ui.label("Filter:");
            ui.add(egui::TextEdit::singleline(&mut self.filter).desired_width(180.0));
        });
        ui.separator();

        if self.mods.is_empty() {
            ui.add_space(30.0);
            ui.vertical_centered(|ui| {
                ui.heading("No mods installed yet");
                ui.label("Drop archives onto this window, use \"Add archives…\", or install from Nexus.");
                ui.add_space(8.0);
                if ui.button("🧩 Start with the frameworks (CET, RED4ext, …)").clicked() {
                    self.open_frameworks();
                }
            });
            return;
        }

        if self.selected.is_some() {
            egui::Panel::bottom("details").resizable(true).default_size(180.0).show(ui, |ui| {
                self.details(ui);
            });
        }

        let filter = self.filter.to_lowercase();
        let mut actions: Vec<Job> = Vec::new();
        let mut update = None;
        egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
            egui::Grid::new("mods").striped(true).num_columns(6).spacing([14.0, 6.0]).show(ui, |ui| {
                ui.strong("On");
                ui.strong("Name");
                ui.strong("Version");
                ui.strong("Files");
                ui.strong("Status");
                ui.strong("");
                ui.end_row();

                for m in &self.mods {
                    if !filter.is_empty() && !m.name.to_lowercase().contains(&filter) {
                        continue;
                    }
                    let mut on = m.enabled;
                    if ui.checkbox(&mut on, "").changed() {
                        actions.push(if on {
                            Job::Enable { id: m.id.clone(), overwrite: false }
                        } else {
                            Job::Disable { id: m.id.clone() }
                        });
                    }
                    let selected = self.selected.as_ref() == Some(&m.id);
                    let name = if m.enabled { RichText::new(&m.name) } else { RichText::new(&m.name).weak() };
                    if ui.selectable_label(selected, name).clicked() {
                        self.selected = if selected { None } else { Some(m.id.clone()) };
                    }
                    ui.label(m.version.as_deref().unwrap_or("–"));
                    ui.label(m.files.len().to_string());

                    ui.horizontal(|ui| {
                        if !m.overridden.is_empty() {
                            ui.colored_label(ui.visuals().warn_fg_color, format!("{} overridden", m.overridden.len()))
                                .on_hover_text("Some files are replaced by a mod installed later.");
                        }
                        if let Some(u) = self.updates.get(&m.id) {
                            let text = RichText::new(format!("⬆ {}", u.version.as_deref().unwrap_or("update")))
                                .color(egui::Color32::from_rgb(90, 200, 120));
                            let hint = match u.source {
                                UpdateSource::Github(_) => "Update available: click to install it",
                                UpdateSource::Nexus { .. } => {
                                    "Update available: click to install it (Premium) or open the Nexus files page"
                                }
                            };
                            if ui.small_button(text).on_hover_text(hint).clicked() {
                                update = Some(m.id.clone());
                            }
                        }
                        if m.github.is_some() {
                            ui.weak("GitHub");
                        } else if m.nexus.is_some() {
                            ui.weak("Nexus");
                        }
                    });

                    ui.horizontal(|ui| {
                        if let Some(nexus) = &m.nexus
                            && ui.small_button("🌐").on_hover_text("Open on Nexus Mods").clicked()
                        {
                            ui.ctx().open_url(egui::OpenUrl::new_tab(nexus_page(nexus.mod_id)));
                        }
                        if ui.small_button("🗑").on_hover_text("Remove").clicked() {
                            self.confirm_remove = Some(m.id.clone());
                        }
                    });
                    ui.end_row();
                }
            });
        });
        for job in actions {
            self.send(job);
        }
        if let Some(id) = update {
            self.apply_update(ui.ctx(), &id, true);
        }
    }

    fn update_all(&mut self, ctx: &egui::Context) {
        let ids: Vec<String> = self.updates.keys().cloned().collect();
        let manual = ids.iter().filter(|id| !self.apply_update(ctx, id, false)).count();
        if manual > 0 {
            self.push_log(
                Level::Warn,
                format!(
                    "{manual} update(s) need a manual download: click their ⬆ badge to open the Nexus files page, \
                     then use \"Mod Manager Download\"."
                ),
            );
        }
    }

    fn details(&mut self, ui: &mut egui::Ui) {
        let Some(m) = self.selected.as_ref().and_then(|id| self.mods.iter().find(|m| &m.id == id)).cloned() else {
            self.selected = None;
            return;
        };
        ui.horizontal_wrapped(|ui| {
            ui.heading(&m.name);
            if ui.small_button("✖").clicked() {
                self.selected = None;
            }
        });
        if let Some(a) = &m.archive {
            ui.weak(format!("from {a}"));
        }
        ui.horizontal_wrapped(|ui| {
            if let Some(u) = self.updates.get(&m.id) {
                let premium = self.nexus_user.as_ref().is_some_and(|u| u.is_premium);
                let automatic = match u.source {
                    UpdateSource::Github(_) => true,
                    UpdateSource::Nexus { file_id, .. } => premium && file_id.is_some(),
                };
                let clicked = if automatic {
                    ui.button("Install update").clicked()
                } else {
                    ui.button("Get update on Nexus")
                        .on_hover_text("Opens the files page. Click \"Mod Manager Download\" there and it installs here.")
                        .clicked()
                };
                if clicked {
                    self.apply_update(ui.ctx(), &m.id, true);
                }
            }
            let folder = if m.enabled { None } else { Some(self.holding_dir().join(&m.id)) };
            if let Some(folder) = folder
                && ui.button("Open holding folder").clicked()
            {
                open_path(&folder);
            }
        });
        if m.github.is_none() {
            ui.horizontal_wrapped(|ui| match &m.nexus {
                Some(n) => {
                    ui.weak(format!("Linked to Nexus mod {}", n.mod_id));
                    if ui.small_button("Unlink").clicked() {
                        self.send(Job::LinkNexus { id: m.id.clone(), mod_id: None });
                    }
                }
                None => {
                    ui.label("Link to Nexus:");
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut self.link_input)
                            .hint_text("mod page URL or ID")
                            .desired_width(300.0),
                    );
                    let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if ui.button("Link").on_hover_text("Lets the update checker follow this mod").clicked() || enter {
                        match crate::nexus::parse_mod_ref(&self.link_input) {
                            Some(mod_id) => {
                                self.send(Job::LinkNexus { id: m.id.clone(), mod_id: Some(mod_id) });
                                self.link_input.clear();
                            }
                            None => self.push_log(Level::Warn, "That doesn't look like a Nexus mod URL or ID."),
                        }
                    }
                }
            });
        }
        ui.separator();
        egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
            for f in &m.files {
                let mut text = RichText::new(f).monospace();
                if m.overridden.contains(f) {
                    text = text.strikethrough();
                }
                let label = ui.label(text);
                if m.overridden.contains(f) {
                    label.on_hover_text("Overridden by another mod");
                } else if m.backed_up.contains(f) {
                    label.on_hover_text("Replaces an original game file (backed up)");
                }
            }
        });
    }
}

pub(super) fn nexus_page(mod_id: u64) -> String {
    format!("https://www.nexusmods.com/{}/mods/{mod_id}", crate::nexus::GAME_DOMAIN)
}

pub(super) fn open_path(path: &std::path::Path) {
    let _ = std::fs::create_dir_all(path);
    let _ = std::process::Command::new("xdg-open").arg(path).spawn();
}
