//! Native design preview, using only the integration-test fixture. No accounts or network.
#[path = "../tests/common/mod.rs"]
mod common;

use gpui_kit::component::TitleBar;
use gpui_kit::*;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let settings = args.iter().any(|arg| arg == "--settings");
    let accounts = args.iter().any(|arg| arg == "--accounts");
    let switcher = args.iter().any(|arg| arg == "--switcher");
    let spam = args.iter().any(|arg| arg == "--spam");
    let narrow = args.iter().any(|arg| arg == "--narrow");
    let light = args.iter().any(|arg| arg == "--light");
    let thread = args.iter().any(|arg| arg == "--thread");
    let palette = args.iter().any(|arg| arg == "--palette");
    let create = args.iter().any(|arg| arg == "--create-folder");
    let move_mail = args.iter().any(|arg| arg == "--move");
    let empty = args.iter().any(|arg| arg == "--empty");
    let no_accounts = args.iter().any(|arg| arg == "--no-accounts");
    let focus_search = args.iter().any(|arg| arg == "--focus-search");
    // Leaves the inbox and returns with `g i`, for checking the lens is reselected.
    let roundtrip = args.iter().any(|arg| arg == "--roundtrip");
    // `--split N` opens the Nth seeded split, for reviewing the category lenses.
    let split = args
        .iter()
        .position(|arg| arg == "--split")
        .and_then(|ix| args.get(ix + 1))
        .and_then(|value| value.parse::<usize>().ok());
    let done = args.iter().any(|arg| arg == "--done");
    gpui_kit::application()
        .with_assets(kite::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            let mail = if empty {
                common::Fixture::new(0)
            } else {
                common::Fixture::showcase()
            };
            if no_accounts {
                mail.accounts.lock().unwrap().clear();
            }
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
                        if settings {
                            view.run(kite::keys::cmd::SETTINGS, kite::app::Src::Key, window, cx);
                        }
                        if accounts {
                            view.run(kite::keys::cmd::MANAGE_ACCOUNTS, kite::app::Src::Key, window, cx);
                        }
                        if switcher {
                            view.run(kite::keys::cmd::ACCOUNT_SWITCHER, kite::app::Src::Key, window, cx);
                        }
                        if spam {
                            view.run(kite::keys::cmd::GO + 7, kite::app::Src::Key, window, cx);
                        }
                        if thread {
                            view.run(kite::keys::cmd::OPEN, kite::app::Src::Key, window, cx);
                        }
                        if create {
                            view.run(kite::keys::cmd::CREATE_FOLDER, kite::app::Src::Key, window, cx);
                        }
                        if move_mail {
                            view.run(kite::keys::cmd::MOVE, kite::app::Src::Key, window, cx);
                        }
                        if palette {
                            view.run(kite::keys::cmd::PALETTE, kite::app::Src::Key, window, cx);
                        }
                        if focus_search {
                            view.run(kite::keys::cmd::SEARCH, kite::app::Src::Key, window, cx);
                        }
                        if roundtrip {
                            view.run(kite::keys::cmd::GO + 1, kite::app::Src::Key, window, cx);
                            view.run(kite::keys::cmd::GO, kite::app::Src::Key, window, cx);
                        }
                        for _ in 0..split.unwrap_or(0) {
                            view.run(kite::keys::cmd::NEXT_SPLIT, kite::app::Src::Key, window, cx);
                        }
                        if done {
                            view.run(kite::keys::cmd::DONE, kite::app::Src::Key, window, cx);
                        }
                        view
                    })
                },
            )
            .expect("open design preview");
        });
}
