mod archive;
mod config;
mod fsutil;
mod game;
mod layout;
mod nexus;
mod nxm;
mod ops;
mod paths;
mod registry;
mod ui;
mod worker;

fn main() -> eframe::Result {
    // The browser launches us with an nxm:// link; hand it to a running window if there is one.
    let nxm_link = std::env::args().skip(1).find(|a| a.starts_with("nxm://"));
    if let Some(link) = &nxm_link
        && nxm::forward_to_running(link)
    {
        return Ok(());
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Cyber Mod Manager")
            .with_app_id("cyber-mod-manager")
            .with_inner_size([1000.0, 680.0])
            .with_min_inner_size([640.0, 400.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Cyber Mod Manager",
        options,
        Box::new(move |cc| Ok(Box::new(ui::App::new(cc, nxm_link)?))),
    )
}
