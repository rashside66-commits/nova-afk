#![windows_subsystem = "windows"]

mod automation;
mod capture;
mod config;
mod input;
mod logger;
mod mover;
mod ui;
mod vision;

use parking_lot::Mutex;
use std::sync::{mpsc, Arc};

fn main() -> eframe::Result<()> {
    let log = logger::Logger::new(config::app_dir().join("nova_afk.log"));
    let (cfg, cfg_err) = config::Config::load();
    if let Some(e) = cfg_err {
        log.warn(e);
    }
    let shared = Arc::new(Mutex::new(automation::Shared::new()));
    let (tx, rx) = mpsc::channel();

    {
        let (shared, log) = (shared.clone(), log.clone());
        std::thread::Builder::new()
            .name("nova-engine".into())
            .spawn(move || automation::Engine::new(cfg, shared, log, rx).run())
            .expect("engine thread");
    }

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("NOVA AFK")
            .with_inner_size([460.0, 720.0])
            .with_min_inner_size([420.0, 560.0]),
        ..Default::default()
    };
    let result = eframe::run_native(
        "NOVA AFK",
        options,
        Box::new(move |cc| Ok(Box::new(ui::App::new(cc, shared, log, tx)))),
    );
    // Never leave a movement key or the mouse button held after closing.
    mover::release_everything();
    input::keep_awake(false);
    result
}
