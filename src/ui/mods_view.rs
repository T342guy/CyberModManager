use egui::RichText;

use super::App;
use crate::worker::Job;

impl App {
    pub(super) fn mods_tab(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("➕ Add archives…").clicked() {
                self.pick_archives();
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
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
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
                            ui.colored_label(
                                egui::Color32::from_rgb(90, 200, 120),
                                format!("update: {}", u.version.as_deref().unwrap_or("new file")),
                            );
                        }
                        if m.nexus.is_some() {
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
    }

    fn details(&mut self, ui: &mut egui::Ui) {
        let Some(m) = self.selected.as_ref().and_then(|id| self.mods.iter().find(|m| &m.id == id)).cloned() else {
            self.selected = None;
            return;
        };
        ui.horizontal(|ui| {
            ui.heading(&m.name);
            if ui.small_button("✖").clicked() {
                self.selected = None;
            }
        });
        if let Some(a) = &m.archive {
            ui.weak(format!("from {a}"));
        }
        ui.horizontal(|ui| {
            if let Some(u) = self.updates.get(&m.id).cloned() {
                let premium = self.nexus_user.as_ref().is_some_and(|u| u.is_premium);
                match u.file_id {
                    Some(file_id) if premium => {
                        if ui.button("Install update").clicked()
                            && let Some(key) = self.api_key()
                        {
                            let item = self.new_item(format!("Update {}", m.name));
                            self.start_nexus_install(key, crate::worker::NexusRequest {
                                item,
                                mod_id: u.mod_id,
                                file_id,
                                nxm_key: None,
                                name: Some(m.name.clone()),
                            });
                            self.tab = super::Tab::Install;
                        }
                    }
                    _ => {
                        if ui.button("Get update on Nexus").on_hover_text(
                            "Opens the files page. Click \"Mod Manager Download\" there and it installs here.",
                        ).clicked() {
                            ui.ctx().open_url(egui::OpenUrl::new_tab(format!("{}?tab=files", nexus_page(u.mod_id))));
                        }
                    }
                }
            }
            let folder = if m.enabled { None } else { Some(self.holding_dir().join(&m.id)) };
            if let Some(folder) = folder
                && ui.button("Open holding folder").clicked()
            {
                open_path(&folder);
            }
        });
        ui.separator();
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
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
