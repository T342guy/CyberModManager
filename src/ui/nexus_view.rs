use egui::RichText;

use super::App;
use super::mods_view::nexus_page;
use crate::nexus::parse_mod_ref;
use crate::worker::{self, Level, NexusRequest};

impl App {
    pub(super) fn nexus_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Account");
        ui.horizontal(|ui| {
            ui.label("API key:");
            ui.add(egui::TextEdit::singleline(&mut self.api_key_input).password(true).desired_width(320.0));
            if ui.button("Save & check").clicked() {
                let key = self.api_key_input.trim().to_string();
                self.config.nexus_api_key = (!key.is_empty()).then(|| key.clone());
                self.save_config();
                if key.is_empty() {
                    self.nexus_user = None;
                } else {
                    worker::spawn_validate(key, self.notifier.clone());
                }
            }
            ui.hyperlink_to("Get your key", "https://www.nexusmods.com/users/myaccount?tab=api");
        });
        match &self.nexus_user {
            Some(u) if u.is_premium => {
                ui.label(format!("Signed in as {} (Premium): direct downloads are available.", u.name));
            }
            Some(u) => {
                ui.label(format!(
                    "Signed in as {}. Free accounts install through the \"Mod Manager Download\" button on Nexus.",
                    u.name
                ));
            }
            None => {
                ui.weak("Not signed in.");
            }
        }
        if ui.button("Register as nxm:// link handler").on_hover_text(
            "Makes the \"Mod Manager Download\" button on nexusmods.com open this app.",
        ).clicked() {
            match crate::nxm::register_handler() {
                Ok(p) => self.push_log(Level::Info, format!("Registered nxm:// handler ({})", p.display())),
                Err(e) => self.push_log(Level::Error, format!("Registering nxm handler failed: {e:#}")),
            }
        }

        ui.separator();
        ui.heading("Updates");
        ui.horizontal(|ui| {
            let tracked = self.mods.iter().filter(|m| m.nexus.is_some()).count();
            if ui.button("Check for updates").clicked()
                && let Some(key) = self.api_key()
            {
                worker::spawn_update_check(key, self.mods.clone(), self.notifier.clone());
                self.push_log(Level::Info, "Checking Nexus for updates…");
            }
            ui.weak(format!("{tracked} installed mods are linked to Nexus."));
        });
        let mut open = None;
        for u in self.updates.values() {
            let name = self.mods.iter().find(|m| m.id == u.entry_id).map(|m| m.name.as_str()).unwrap_or("?");
            ui.horizontal(|ui| {
                ui.label(format!("{name} → {}", u.version.as_deref().unwrap_or("newer file")));
                if ui.small_button("details").clicked() {
                    open = Some(u.entry_id.clone());
                }
            });
        }
        if let Some(id) = open {
            self.selected = Some(id);
            self.tab = super::Tab::Mods;
        }

        ui.separator();
        ui.heading("Install from Nexus");
        ui.horizontal(|ui| {
            ui.label("Mod URL or ID:");
            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.nexus_input)
                    .hint_text("https://www.nexusmods.com/cyberpunk2077/mods/107")
                    .desired_width(360.0),
            );
            let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (ui.button("Look up").clicked() || enter)
                && let Some(key) = self.api_key()
            {
                match parse_mod_ref(&self.nexus_input) {
                    Some(id) => worker::spawn_fetch_files(key, id, self.notifier.clone()),
                    None => self.push_log(Level::Warn, "That doesn't look like a Nexus mod URL or ID."),
                }
            }
        });

        let Some((mod_id, info, list)) = self.nexus_files.clone() else { return };
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(info.name.clone().unwrap_or(format!("Mod {mod_id}"))).strong());
            if let Some(v) = &info.version {
                ui.weak(format!("v{v}"));
            }
            ui.hyperlink_to("open page", nexus_page(mod_id));
        });
        if let Some(s) = &info.summary {
            ui.label(s);
        }
        let premium = self.nexus_user.as_ref().is_some_and(|u| u.is_premium);
        if !premium {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                "Direct download needs Premium. Use \"Mod Manager Download\" on the files page instead.",
            );
            if ui.button("Open files page").clicked() {
                ui.ctx().open_url(egui::OpenUrl::new_tab(format!("{}?tab=files", nexus_page(mod_id))));
            }
        }
        let mut install = None;
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            egui::Grid::new("nexus-files").striped(true).num_columns(5).spacing([12.0, 6.0]).show(ui, |ui| {
                for f in list.visible() {
                    ui.label(&f.name);
                    ui.weak(f.category_name.as_deref().unwrap_or("").to_lowercase());
                    ui.label(f.version.as_deref().unwrap_or(""));
                    ui.label(f.size_kb.map(|kb| format!("{:.1} MB", kb as f64 / 1024.0)).unwrap_or_default());
                    if ui.add_enabled(premium, egui::Button::new("Install")).clicked() {
                        install = Some(f.clone());
                    }
                    ui.end_row();
                }
            });
        });
        if let Some(f) = install
            && let Some(key) = self.api_key()
        {
            let item = self.new_item(format!("{} ({})", info.name.clone().unwrap_or_default(), f.name));
            self.start_nexus_install(key, NexusRequest { item, mod_id, file_id: f.file_id, nxm_key: None, name: None });
            self.tab = super::Tab::Install;
        }
    }
}
