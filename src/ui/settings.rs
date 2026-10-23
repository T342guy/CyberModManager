use super::App;
use super::mods_view::open_path;
use crate::worker::{Job, Level};

impl App {
    pub(super) fn settings_tab(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| self.settings_contents(ui));
    }

    fn settings_contents(&mut self, ui: &mut egui::Ui) {
        ui.heading("Game folder");
        ui.horizontal_wrapped(|ui| {
            match &self.config.game_dir {
                Some(d) => ui.monospace(d.display().to_string()),
                None => ui.colored_label(ui.visuals().warn_fg_color, "not set"),
            };
        });
        ui.horizontal_wrapped(|ui| {
            if ui.button("Auto-detect (Steam)").clicked() {
                match crate::game::detect() {
                    Some(dir) => self.set_game_dir(dir),
                    None => self.push_log(Level::Warn, "Cyberpunk 2077 not found in any Steam library."),
                }
            }
            if ui.button("Browse…").clicked()
                && let Some(dir) = rfd::FileDialog::new().set_title("Cyberpunk 2077 folder").pick_folder()
            {
                if crate::game::looks_like_game_dir(&dir) {
                    self.set_game_dir(dir);
                } else {
                    self.push_log(Level::Error, format!("{} doesn't contain bin/x64/Cyberpunk2077.exe", dir.display()));
                }
            }
            if let Some(d) = self.config.game_dir.clone()
                && ui.button("Open").clicked()
            {
                open_path(&d);
            }
        });

        ui.add_space(10.0);
        ui.heading("Holding folder (disabled mods)");
        let holding = self.holding_dir();
        ui.monospace(holding.display().to_string());
        ui.weak("Tip: put this on the same drive as the game so enabling/disabling is instant.");
        ui.horizontal_wrapped(|ui| {
            if ui.button("Change…").clicked()
                && let Some(dir) = rfd::FileDialog::new().set_title("Holding folder").pick_folder()
            {
                self.send(Job::RelocateHolding { from: holding.clone(), to: dir.clone() });
                self.config.holding_dir = Some(dir);
                self.save_config();
                self.push_dirs();
            }
            if self.config.holding_dir.is_some() && ui.button("Reset to default").clicked() {
                let to = self.paths.default_holding_dir();
                self.send(Job::RelocateHolding { from: holding.clone(), to });
                self.config.holding_dir = None;
                self.save_config();
                self.push_dirs();
            }
            if ui.button("Open").clicked() {
                open_path(&holding);
            }
        });

        ui.add_space(10.0);
        ui.heading("Behaviour");
        if ui.checkbox(&mut self.config.always_overwrite, "Newer mods overwrite conflicting files without asking").changed() {
            self.save_config();
        }
        if ui.checkbox(&mut self.config.delete_downloads, "Delete downloads after installing").changed() {
            self.save_config();
        }
        if ui.checkbox(&mut self.config.check_updates_on_start, "Check for mod updates when the app starts").changed() {
            self.save_config();
        }

        ui.add_space(10.0);
        ui.heading("Desktop");
        let mut open_with = crate::nxm::open_with_registered();
        if ui
            .checkbox(&mut open_with, "Show in \"Open with\" for .zip / .7z / .rar files")
            .on_hover_text(
                "Right-click an archive in your file manager or browser downloads and pick Cyber Mod Manager to \
                 install it. Your usual archive app stays the default.",
            )
            .changed()
        {
            let result = if open_with {
                crate::nxm::register_open_with().map(|p| format!("Added to \"Open with\" ({})", p.display()))
            } else {
                crate::nxm::unregister_open_with().map(|()| "Removed from \"Open with\"".to_string())
            };
            match result {
                Ok(msg) => self.push_log(Level::Info, msg),
                Err(e) => self.push_log(Level::Error, format!("Updating \"Open with\" failed: {e:#}")),
            }
        }

        ui.add_space(10.0);
        ui.heading("App data");
        ui.horizontal_wrapped(|ui| {
            ui.monospace(self.paths.data_dir.display().to_string());
            if ui.button("Open").clicked() {
                open_path(&self.paths.data_dir);
            }
        });
    }

    fn set_game_dir(&mut self, dir: std::path::PathBuf) {
        if self.mods.iter().any(|m| m.enabled) && self.config.game_dir.as_ref() != Some(&dir) {
            self.push_log(
                Level::Warn,
                "Game folder changed while mods are enabled; their files stay in the old folder. Disable them first if that's not what you want.",
            );
        }
        self.push_log(Level::Info, format!("Game folder: {}", dir.display()));
        self.config.game_dir = Some(dir);
        self.save_config();
        self.push_dirs();
    }

    fn push_dirs(&mut self) {
        if let Some(dirs) = super::dirs_for(&self.paths, &self.config) {
            self.send(Job::SetDirs(dirs));
        }
    }
}
