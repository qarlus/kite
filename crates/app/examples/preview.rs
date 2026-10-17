//! Native design preview, using only the integration-test fixture. No accounts or network.
#[path = "../tests/common/mod.rs"]
mod common;

use gpui_kit::component::TitleBar;
use gpui_kit::*;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let narrow = args.iter().any(|arg| arg == "--narrow");
    let light = args.iter().any(|arg| arg == "--light");
    let thread = args.iter().any(|arg| arg == "--thread");
    let palette = args.iter().any(|arg| arg == "--palette");
    let empty = args.iter().any(|arg| arg == "--empty");
    gpui_kit::application()
        .with_assets(kite::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            let mail = if empty {
                common::Fixture::new(0)
            } else {
                common::Fixture::showcase()
            };
            if light {
                mail.settings.lock().unwrap().theme = "light".into();
            }
            let bounds = Bounds::centered(
                None,
                size(px(if narrow { 720. } else { 1200. }), px(800.)),
                cx,
            );
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(720.), px(480.))),
                    titlebar: Some(TitlebarOptions {
                        title: Some("Kite — Design preview".into()),
                        ..TitleBar::title_bar_options()
                    }),
                    ..TitleBar::window_options()
                },
                cx,
                |window, cx| {
                    cx.new(|cx| {
                        let mut view = kite::app::Kite::new(mail, window, cx);
                        if thread {
                            view.run(kite::keys::cmd::OPEN, window, cx);
                        }
                        if palette {
                            view.run(kite::keys::cmd::PALETTE, window, cx);
                        }
                        view
                    })
                },
            )
            .expect("open design preview");
        });
}
