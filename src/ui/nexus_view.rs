use egui::RichText;

use super::App;
use super::mods_view::nexus_page;
use crate::nexus::parse_mod_ref;
use crate::worker::{self, Level, NexusRequest};

impl App {
    pub(super) fn nexus_tab(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| self.nexus_contents(ui));
    }

    fn nexus_contents(&mut self, ui: &mut egui::Ui) {
        ui.heading("Account");
        ui.horizontal_wrapped(|ui| {
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
                ui.label(format!("Signed in as {} (free account).", u.name));
            }
            None => {
                ui.weak("Not signed in.");
            }
        }

        ui.add_space(6.0);
        ui.strong("Automatic install from your browser");
        ui.horizontal_wrapped(|ui| {
            if ui.checkbox(&mut self.config.watch_downloads, "Install Nexus downloads automatically").changed() {
                self.save_config();
                self.restart_watcher();
            }
            let dir = self.watch_dir().map(|d| d.display().to_string()).unwrap_or_else(|| "no folder set".into());
            ui.weak("from");
            ui.monospace(dir);
            if ui.button("Change…").clicked()
                && let Some(dir) = rfd::FileDialog::new().set_title("Folder your browser downloads to").pick_folder()
            {
                self.config.downloads_dir = Some(dir);
                self.save_config();
                self.restart_watcher();
            }
        });
        if ui
            .checkbox(&mut self.config.watch_any_archive, "Take any archive, not just Nexus-named ones")
            .on_hover_text(
                "For a folder your browser saves only mods into: renamed files and mods from other sites \
                 install too. Don't use it on your general Downloads folder.",
            )
            .changed()
        {
            self.save_config();
            self.restart_watcher();
        }
        ui.weak(
            "Works with any account: click \"Manual download\" on a mod's Files tab and it installs as soon as \
             the browser finishes saving it, linked to its Nexus page for update checks. Only archives that \
             arrive while the app is open are picked up, and only Nexus-style names (Name-12345-1-0-….zip) \
             unless you tick \"any archive\". Tip: point it at a folder just for mods.",
        );
        ui.add_space(6.0);
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
        ui.horizontal_wrapped(|ui| {
            let tracked = self.mods.iter().filter(|m| m.nexus.is_some()).count();
            if ui.button("Check for updates").clicked() {
                self.check_updates(true);
            }
            ui.weak(format!(
                "{tracked} installed mods are linked to Nexus. Link others from their details panel on the Mods tab."
            ));
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
        ui.horizontal_wrapped(|ui| {
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
        ui.horizontal_wrapped(|ui| {
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
        egui::ScrollArea::horizontal().id_salt("nexus-files-scroll").show(ui, |ui| {
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
