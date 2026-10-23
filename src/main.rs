mod archive;
mod config;
mod frameworks;
mod fsutil;
mod game;
mod github;
mod layout;
mod nexus;
mod nxm;
mod ops;
mod paths;
mod registry;
mod ui;
mod watch;
mod worker;

fn main() -> eframe::Result {
    // The browser or file manager launches us with nxm:// links or archives
    // ("Open with"); hand them to a running window if there is one.
    let handoff: Vec<String> = std::env::args()
        .skip(1)
        .filter_map(|arg| {
            if arg.starts_with("nxm://") {
                return Some(arg);
            }
            let path = std::path::Path::new(&arg);
            (archive::is_supported(path) && path.is_file())
                .then(|| std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()).display().to_string())
        })
        .collect();
    if !handoff.is_empty() && handoff.iter().all(|arg| nxm::forward_to_running(arg)) {
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
        Box::new(move |cc| Ok(Box::new(ui::App::new(cc, handoff)?))),
    )
}
