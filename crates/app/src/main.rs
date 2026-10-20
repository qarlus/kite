#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! Opens the local engine and runs the Kite window.
use gpui_kit::component::TitleBar;
use gpui_kit::*;

fn main() {
    std::thread::spawn(|| {
        if let Err(error) = kite_core::update::cleanup_old() {
            eprintln!("{error:#}");
        }
    });
    let directory = kite_core::engine::data_dir();
    let engine = match kite_core::engine::Engine::open(&directory) {
        Ok(engine) => engine,
        Err(error) => {
            eprintln!(
                "Kite could not open its local mail database in {}: {error:#}",
                directory.display()
            );
            std::process::exit(1);
        }
    };
    gpui_kit::application()
        .with_assets(kite::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            let bounds = Bounds::centered(None, size(px(1200.), px(800.)), cx);
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(720.), px(480.))),
                    ..TitleBar::window_options()
                },
                cx,
                |window, cx| {
                    cx.new(|cx| {
                        let kite = kite::app::Kite::new(engine, window, cx);
                        kite.start_update_checks(cx);
                        kite
                    })
                },
            )
            .expect("failed to open window");
        });
}
