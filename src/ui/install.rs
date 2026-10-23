use std::path::PathBuf;

use super::{App, Tab};
use crate::ops::InstallMeta;
use crate::worker::{ItemStatus, Job, Level};

impl App {
    pub(super) fn pick_archives(&mut self) {
        let files = rfd::FileDialog::new()
            .set_title("Choose mod archives")
            .add_filter("Mod archives", &["zip", "7z", "rar"])
            .pick_files();
        if let Some(files) = files {
            self.queue_archives(files);
        }
    }

    /// Batch mode: queue every archive; the worker installs them one by one.
    pub(super) fn queue_archives(&mut self, paths: Vec<PathBuf>) {
        let mut added = 0;
        for path in paths {
            if !crate::archive::is_supported(&path) {
                self.push_log(Level::Warn, format!("Skipping {}: not a .zip/.7z/.rar", path.display()));
                continue;
            }
            let meta = InstallMeta::from_archive_name(&path);
            let item = self.new_item(meta.name.clone());
            self.send(Job::Install { item, path, meta, overwrite: self.config.always_overwrite, delete_after: false });
            added += 1;
        }
        if added > 0 {
            self.tab = Tab::Install;
        }
    }

    pub(super) fn install_tab(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("➕ Add archives…").clicked() {
                self.pick_archives();
            }
            if ui.button("Clear finished").clicked() {
                self.queue.retain(|q| !matches!(q.status, ItemStatus::Done | ItemStatus::Failed(_)));
            }
            ui.checkbox(&mut self.config.always_overwrite, "Newer mods overwrite conflicts without asking")
                .changed()
                .then(|| self.save_config());
        });
        ui.weak("Tip: select many archives at once, or drag them onto the window. They install in order.");
        ui.separator();

        if self.queue.is_empty() {
            ui.add_space(30.0);
            ui.vertical_centered(|ui| ui.label("Nothing queued."));
            return;
        }
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            egui::Grid::new("queue").striped(true).num_columns(2).spacing([14.0, 6.0]).show(ui, |ui| {
                for q in self.queue.iter().rev() {
                    ui.label(&q.label);
                    match &q.status {
                        ItemStatus::Queued => {
                            ui.weak("queued");
                        }
                        ItemStatus::Downloading(f) => {
                            ui.add(egui::ProgressBar::new(*f).show_percentage().desired_width(220.0));
                        }
                        ItemStatus::Installing => {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label("installing");
                            });
                        }
                        ItemStatus::Done => {
                            ui.colored_label(egui::Color32::from_rgb(90, 200, 120), "installed");
                        }
                        ItemStatus::NeedsDecision => {
                            ui.colored_label(ui.visuals().warn_fg_color, "conflict, waiting for your choice");
                        }
                        ItemStatus::Failed(e) => {
                            ui.colored_label(ui.visuals().error_fg_color, e);
                        }
                    }
                    ui.end_row();
                }
            });
        });
    }
}
