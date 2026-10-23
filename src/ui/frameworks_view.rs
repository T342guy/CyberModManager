use egui::{Color32, RichText};

use super::{App, FrameworkState};
use crate::frameworks::CATALOG;

impl App {
    pub(super) fn open_frameworks(&mut self) {
        // Pre-tick the essentials that aren't installed yet.
        self.fw_picked = CATALOG
            .iter()
            .filter(|fw| fw.essential && self.framework_state(fw) == FrameworkState::Missing)
            .map(|fw| fw.name)
            .collect();
        self.frameworks_open = true;
    }

    pub(super) fn frameworks_dialog(&mut self, ctx: &egui::Context) {
        if !self.frameworks_open {
            return;
        }
        let states: Vec<FrameworkState> = CATALOG.iter().map(|fw| self.framework_state(fw)).collect();
        let mut install = false;
        let mut close = false;
        egui::Modal::new(egui::Id::unique("frameworks")).show(ctx, |ui| {
            let body = super::fit_modal(ui, 640.0);
            ui.heading("Frameworks & dependencies");
            egui::ScrollArea::vertical().max_height(body).auto_shrink([false, true]).show(ui, |ui| {
                ui.label(
                    "Most mods need some of these. They download straight from the official GitHub releases \
                     (no Nexus account needed), and anything a framework needs is added automatically.",
                );
                ui.add_space(4.0);
                for (fw, state) in CATALOG.iter().zip(&states) {
                    ui.separator();
                    ui.horizontal_wrapped(|ui| {
                        let mut on = self.fw_picked.contains(&fw.name);
                        if ui.checkbox(&mut on, RichText::new(fw.name).strong()).changed() {
                            if on {
                                self.fw_picked.push(fw.name);
                            } else {
                                self.fw_picked.retain(|n| *n != fw.name);
                            }
                        }
                        match state {
                            FrameworkState::Managed { version, enabled } => {
                                let v = version.as_deref().map(|v| format!(" {v}")).unwrap_or_default();
                                let text =
                                    if *enabled { format!("installed{v}") } else { format!("installed{v}, disabled") };
                                ui.colored_label(Color32::from_rgb(90, 200, 120), text);
                            }
                            FrameworkState::Unmanaged => {
                                ui.colored_label(ui.visuals().warn_fg_color, "installed by hand").on_hover_text(
                                    "Installing it here replaces the hand-installed copy with the latest release, \
                                     managed by this app.",
                                );
                            }
                            FrameworkState::Missing => {
                                ui.weak("not installed");
                            }
                        }
                        ui.hyperlink_to("GitHub", format!("https://github.com/{}", fw.repo));
                        ui.hyperlink_to("Nexus", super::mods_view::nexus_page(fw.nexus_id));
                    });
                    ui.indent(fw.name, |ui| {
                        let mut about = fw.about.to_string();
                        if !fw.requires.is_empty() {
                            about.push_str(&format!(" · needs {}", fw.requires.join(", ")));
                        }
                        ui.weak(about);
                    });
                }
                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    ui.label("Select:");
                    if ui
                        .small_button("Essentials")
                        .on_hover_text("CET, RED4ext, redscript, TweakXL, ArchiveXL, Codeware")
                        .clicked()
                    {
                        self.fw_picked = CATALOG.iter().filter(|fw| fw.essential).map(|fw| fw.name).collect();
                    }
                    if ui.small_button("All").clicked() {
                        self.fw_picked = CATALOG.iter().map(|fw| fw.name).collect();
                    }
                    if ui.small_button("None").clicked() {
                        self.fw_picked.clear();
                    }
                });
                ui.weak("Ticking something that's already installed reinstalls it at the latest version.");
            });
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                let label = format!("Install selected ({})", self.fw_picked.len());
                if ui.add_enabled(!self.fw_picked.is_empty(), egui::Button::new(label)).clicked() {
                    install = true;
                }
                if ui.button("Close").clicked() {
                    close = true;
                }
            });
        });
        if install {
            let picked = std::mem::take(&mut self.fw_picked);
            self.install_frameworks(&picked);
            close = true;
        }
        if close {
            self.frameworks_open = false;
        }
    }
}
