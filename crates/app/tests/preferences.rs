//! User-facing preferences, native file import and local persistence.
mod common;
use common::{Fixture, mount, open_window, update_content};
use gpui_kit::test::{TestAppContextExt, TestWindowExt};
use gpui_kit::{AppContext, ScrollDelta, TestAppContext, point, px};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

struct TempFile(PathBuf);
impl TempFile {
    /// Unique per instance: tests in one binary run in parallel, and a clock-based
    /// name alone can collide, letting one test's drop delete another's file.
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        Self(std::env::temp_dir().join(format!(
            "kite-ui-{}-{}-{}.json",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )))
    }
}
impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[gpui_kit::test]
fn a_short_window_keeps_the_settings_cog_and_switches_accounts_by_keyboard(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_kit::init);
    let fixture = Fixture::showcase();
    fixture
        .folders
        .lock()
        .unwrap()
        .extend((0..30).map(|i| kite_core::model::Folder {
            id: format!("{i}"),
            account_id: 1,
            name: format!("Project {i}"),
        }));
    let (window, kite) = open_window(
        cx,
        Some(gpui_kit::size(px(720.), px(480.))),
        |window, cx| cx.new(|cx| kite::app::Kite::new(fixture, window, cx)),
    );
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        // The cog moved to the top strip, where it is always reachable.
        let cog = window.find("open-settings").bounds();
        assert!(
            cog.bottom() <= px(34.),
            "the cog sits in the window strip, not the rail"
        );
        assert!(
            window.find("add-account").bounds().bottom() <= px(480.),
            "the remaining rail actions stay inside a short window"
        );
        assert!(window.find("add-account").bounds().origin.y > px(350.));
        // The account menu is a key away in any window size.
        assert!(window.try_find("account-menu").is_none());
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "secondary-shift-a");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("account-menu").label(), Some("Switch account"));
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "down down enter");
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.row_ids(), vec![2, 4, 6, 8, 10, 12])
    });
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("account-menu").is_none());
        assert_eq!(
            window.find(("rail-account", 2usize)).selected(),
            Some(true),
            "the rail marks the scoped account"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn theme_and_text_size_persist_to_disk_and_reopen_in_the_real_view(cx: &mut TestAppContext) {
    let file = TempFile::new();
    let fixture = Fixture::showcase();
    *fixture.settings_path.lock().unwrap() = Some(file.0.clone());
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "secondary-,");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("search").is_none());
        assert!(
            window.try_find("mailbox-picker").is_none(),
            "settings replaces the mail list, so no mailbox picker shows"
        );
        window.click(("settings-tab", 3usize), cx);
        window.render_frame(cx);
        window.click("theme-linen", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("theme-linen").selected(),
            Some(true),
            "the palette did not apply: {:?}",
            window
                .try_find("settings-error")
                .and_then(|e| e.label().map(str::to_owned))
        );
        window.click(("settings-tab", 4usize), cx);
        window.render_frame(cx);
        window.click("font-larger", cx);
        window.render_frame(cx);
        assert_eq!(window.find("font-size-value").label(), Some("15 px"));
        window.click("close-settings", cx);
        window.render_frame(cx);
        let height = window.find(("row", 0usize)).bounds().size.height.as_f32();
        assert!(
            (height - kite::theme::ROW_H * 2. * 15. / 14.).abs() <= 1.,
            "row height {height}"
        );
    })
    .unwrap();
    let saved = common::read_settings(&file.0).unwrap();
    assert_eq!(saved.theme, "linen");
    assert_eq!(saved.font_size, 15);
    let (reopened, _) = mount(cx, fixture);
    cx.simulate_keystrokes(reopened.into(), "secondary-,");
    cx.update_window(reopened.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(("settings-tab", 3usize), cx);
        window.render_frame(cx);
        assert_eq!(window.find("theme-linen").selected(), Some(true));
        window.click(("settings-tab", 4usize), cx);
        window.render_frame(cx);
        assert_eq!(window.find("font-size-value").label(), Some("15 px"));
    })
    .unwrap();
}

#[gpui_kit::test]
async fn importing_a_real_theme_file_applies_and_persists_its_palette(cx: &mut TestAppContext) {
    let file = TempFile::new();
    std::fs::write(&file.0, kite::preferences::TEMPLATE).unwrap();
    let fixture = Fixture::showcase();
    let (window, kite) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "secondary-,");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(("settings-tab", 3usize), cx);
        window.render_frame(cx);
        window.click("import-theme", cx);
    })
    .unwrap();
    assert!(cx.did_prompt_for_paths());
    cx.simulate_path_prompt_response(|options| {
        assert!(options.files && !options.multiple && !options.directories);
        Some(vec![file.0.clone()])
    });
    cx.wait_for(window.into(), Duration::from_secs(5), |window, _| {
        window.try_find("theme-custom").is_some()
    })
    .await;
    cx.update_window(window.into(), |_, window, _| {
        assert_eq!(
            window.find("theme-custom").label(),
            Some("My midnight theme")
        );
        assert_eq!(window.find("theme-custom").selected(), Some(true));
        assert_eq!(
            window.find("settings-notice").label(),
            Some("Theme imported and applied")
        );
    })
    .unwrap();
    common::update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(
            kite.colours().1,
            "#101820",
            "the imported palette is the page the app paints"
        );
    });
    let settings = fixture.settings.lock().unwrap();
    assert_eq!(settings.theme, "custom");
    assert_eq!(
        settings.custom_theme.as_ref().unwrap().colors["bg"],
        "#101820"
    );
}

#[gpui_kit::test]
async fn an_unreadable_import_keeps_the_current_theme_and_shows_a_visible_error(
    cx: &mut TestAppContext,
) {
    let file = TempFile::new();
    std::fs::write(
        &file.0,
        r##"{"name":"Invisible","base":"dark","colors":{"fg":"#101010","bg":"#101010"}}"##,
    )
    .unwrap();
    let fixture = Fixture::showcase();
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "secondary-,");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(("settings-tab", 3usize), cx);
        window.render_frame(cx);
        window.click("import-theme", cx);
    })
    .unwrap();
    cx.simulate_path_prompt_response(|_| Some(vec![file.0.clone()]));
    cx.wait_for(window.into(), Duration::from_secs(5), |window, _| {
        window.try_find("settings-error").is_some()
    })
    .await;
    cx.update_window(window.into(), |_, window, _| {
        let error = window.find("settings-error");
        let label = error.label().unwrap_or_default();
        assert!(
            label.contains("4.5:1"),
            "unexpected import error: {label:?}"
        );
        assert!(error.bounds().bottom() <= px(800.));
        assert!(window.try_find("theme-custom").is_none());
        assert_eq!(window.find("theme-dark").selected(), Some(true));
    })
    .unwrap();
    assert_eq!(fixture.settings.lock().unwrap().theme, "dark");
}

#[gpui_kit::test]
async fn typography_is_applied_and_bad_contrast_is_rejected_without_changing_mail(
    cx: &mut TestAppContext,
) {
    let fixture = Fixture::showcase();
    // A saved face is the row the font picker has to show: the headless text system
    // installs no fonts of its own.
    fixture.settings.lock().unwrap().font_family = "Georgia".into();
    let (window, kite) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "secondary-,");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(("settings-tab", 4usize), cx);
        window.render_frame(cx);
        assert_eq!(window.find("font-family").value(), Some("Georgia"));
        window.scroll(
            "settings-scroll",
            ScrollDelta::Pixels(point(px(0.), px(-480.))),
            cx,
        );
        window.render_frame(cx);
        // System default is the picker's first row, and it applies on confirm.
        window.within("font-family").click("input", cx);
        window.input("system", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| window.press("enter", cx))
        .unwrap();
    // Select commits through defer_in after the key dispatch returns.
    cx.wait_for(window.into(), Duration::from_millis(500), |window, _| {
        window.find("font-family").expanded() == Some(false)
    })
    .await;
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("typography-preview").label(),
            Some(format!("{} / 14 px", kite::theme::FONT).as_str()),
            "picking System default clears the family at once"
        );
        window.click("font-color", cx);
        window.input("#d8d8e8", cx);
        window.scroll(
            "settings-scroll",
            ScrollDelta::Pixels(point(px(0.), px(-240.))),
            cx,
        );
        window.render_frame(cx);
        window.click("apply-typography", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("typography-preview").label(),
            Some(format!("{} / 14 px", kite::theme::FONT).as_str())
        );
        window.click("font-color", cx);
        window.press("secondary-a", cx);
        window.input("#101010", cx);
        window.scroll(
            "settings-scroll",
            ScrollDelta::Pixels(point(px(0.), px(-240.))),
            cx,
        );
        window.render_frame(cx);
        window.click("apply-typography", cx);
        window.render_frame(cx);
        assert!(
            window
                .find("settings-error")
                .label()
                .unwrap()
                .contains("4.5:1")
        );
    })
    .unwrap();
    assert_eq!(fixture.settings.lock().unwrap().font_family, "");
    assert_eq!(
        fixture.settings.lock().unwrap().font_color.as_deref(),
        Some("#d8d8e8")
    );
    cx.simulate_keystrokes(window.into(), "escape");
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.row_count(), 12)
    });
    assert!(
        fixture.actions().is_empty(),
        "typing in preferences cannot run mail shortcuts"
    );
}

#[gpui_kit::test]
fn the_general_tab_controls_undo_send_and_the_chrome_keeps_shortcuts(cx: &mut TestAppContext) {
    let fixture = Fixture::showcase();
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "secondary-,");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(("settings-tab", 0usize), cx);
        window.render_frame(cx);
        window.click("undo-send-10", cx);
        window.click("close-settings", cx);
        window.render_frame(cx);
        // Keyboard help is a chrome entry now, not a strip across the mail pane.
        assert!(window.try_find("shortcut-help").is_some());
        window.click("shortcut-help", cx);
        window.render_frame(cx);
        assert!(window.try_find("help").is_some());
    })
    .unwrap();
    assert_eq!(fixture.settings.lock().unwrap().undo_send_secs, 10);
}

#[gpui_kit::test]
async fn account_connect_and_confirmed_remove_update_settings_and_switcher(
    cx: &mut TestAppContext,
) {
    let fixture = Fixture::showcase();
    let (window, _) = mount(cx, fixture);
    cx.simulate_keystrokes(window.into(), "secondary-,");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(("settings-tab", 2usize), cx);
        window.render_frame(cx);
        window.click("add-microsoft", cx);
        window.render_frame(cx);
        // The connected row lives on the Accounts section.
        window.click(("settings-tab", 1usize), cx);
    })
    .unwrap();
    cx.wait_for(window.into(), Duration::from_secs(5), |window, _| {
        window.try_find("settings-account-3").is_some()
    })
    .await;
    cx.update_window(window.into(), |_, window, cx| {
        window.click("remove-account-3", cx);
        window.render_frame(cx);
        window.scroll(
            "settings-scroll",
            ScrollDelta::Pixels(point(px(0.), px(-250.))),
            cx,
        );
        window.render_frame(cx);
        window.click("confirm-remove-account", cx);
    })
    .unwrap();
    cx.wait_for(window.into(), Duration::from_secs(5), |window, _| {
        window.try_find("settings-account-3").is_none()
    })
    .await;
    cx.update_window(window.into(), |_, window, cx| {
        window.scroll(
            "settings-scroll",
            ScrollDelta::Pixels(point(px(0.), px(1000.))),
            cx,
        );
        window.render_frame(cx);
        window.click("close-settings", cx);
        window.render_frame(cx);
        assert!(window.try_find(("rail-account", 3usize)).is_none());
        assert_eq!(
            window.find(("rail-account", 2usize)).label(),
            Some("karl@outlook.com")
        );
    })
    .unwrap();
}

#[gpui_kit::test]
async fn failed_account_connection_is_not_reported_as_success(cx: &mut TestAppContext) {
    let fixture = Fixture::showcase();
    *fixture.account_error.lock().unwrap() = Some("Sign-in cancelled".into());
    let (window, _) = mount(cx, fixture);
    cx.simulate_keystrokes(window.into(), "secondary-,");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(("settings-tab", 2usize), cx);
        window.render_frame(cx);
        window.click("add-microsoft", cx);
    })
    .unwrap();
    cx.wait_for(window.into(), Duration::from_secs(5), |window, _| {
        window.try_find("settings-error").is_some()
    })
    .await;
    cx.update_window(window.into(), |_, window, _| {
        assert_eq!(
            window.find("settings-error").label(),
            Some("Sign-in cancelled")
        );
        assert!(window.try_find("settings-account-3").is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
fn theme_tiles_keep_one_width_in_every_row_wide_or_compact(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (window, _) = open_window(
        cx,
        Some(gpui_kit::size(px(1200.), px(800.))),
        |window, cx| cx.new(|cx| kite::app::Kite::new(Fixture::showcase(), window, cx)),
    );
    cx.simulate_keystrokes(window.into(), "secondary-,");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(("settings-tab", 3usize), cx);
        window.render_frame(cx);
        let tiles: Vec<_> = [
            "theme-dark",
            "theme-light",
            "theme-midnight",
            "theme-linen",
            "theme-ember",
        ]
        .iter()
        .map(|id| window.find(*id).bounds())
        .collect();
        // All five fit the 720px column at once, so the gallery is a single row.
        for tile in &tiles {
            assert_eq!(
                tile.origin.y, tiles[0].origin.y,
                "a tile dropped to a second row"
            );
            assert_eq!(
                tile.size.width, tiles[0].size.width,
                "every tile keeps the same width"
            );
        }
        assert!(tiles[0].size.width >= px(124.), "tiles stay readable");
    })
    .unwrap();
    let (window, _) = open_window(
        cx,
        Some(gpui_kit::size(px(720.), px(480.))),
        |window, cx| cx.new(|cx| kite::app::Kite::new(Fixture::showcase(), window, cx)),
    );
    cx.simulate_keystrokes(window.into(), "secondary-,");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(("settings-tab", 3usize), cx);
        window.render_frame(cx);
        let carbon = window.find("theme-dark").bounds();
        let paper = window.find("theme-light").bounds();
        let midnight = window.find("theme-midnight").bounds();
        let ember = window.find("theme-ember").bounds();
        assert_eq!(paper.origin.y, carbon.origin.y);
        assert!(midnight.origin.y > carbon.origin.y);
        assert_eq!(midnight.origin.x, carbon.origin.x);
        // Ember starts the last row at the same width as every other tile, not stretched.
        assert!(ember.origin.y > midnight.origin.y);
        assert_eq!(ember.origin.x, carbon.origin.x);
        assert_eq!(ember.size.width, carbon.size.width);
        assert_eq!(midnight.size.width, carbon.size.width);
    })
    .unwrap();
}

#[gpui_kit::test]
fn compact_settings_use_the_whole_window(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (window, _) = open_window(
        cx,
        Some(gpui_kit::size(px(720.), px(480.))),
        |window, cx| cx.new(|cx| kite::app::Kite::new(Fixture::showcase(), window, cx)),
    );
    cx.simulate_keystrokes(window.into(), "secondary-,");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("sidebar").is_none(),
            "settings hides the mail rail and fills the window"
        );
        let page = window.find("settings-page").bounds();
        assert_eq!(page.origin.x, px(0.));
        assert!(page.right() <= px(720.));
        window.click(("settings-tab", 3usize), cx);
        window.render_frame(cx);
        assert!(window.find("theme-light").bounds().right() <= px(720.));
        assert!(window.find("open-settings").bounds().bottom() <= px(34.));
        window.click("close-settings", cx);
        window.render_frame(cx);
        assert!(window.try_find("settings-page").is_none());
        assert!(window.try_find("sidebar").is_some());
        assert!(window.try_find(("row", 0usize)).is_some());
    })
    .unwrap();
}

#[gpui_kit::test]
async fn changing_text_size_reflows_an_already_open_html_message(cx: &mut TestAppContext) {
    let fixture = Fixture::new(2);
    let (window, _) = mount(cx, fixture);
    cx.simulate_keystrokes(window.into(), "j enter");
    cx.wait_for(window.into(), Duration::from_secs(5), |window, _| {
        window
            .try_find("html-22")
            .is_some_and(|body| body.label() != Some("Rendering"))
    })
    .await;
    let before = cx
        .update_window(window.into(), |_, window, _| {
            window.find("html-22").bounds().size.height
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), "secondary-,");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(("settings-tab", 4usize), cx);
        window.render_frame(cx);
        window.click("font-larger", cx);
        window.render_frame(cx);
        window.click("font-larger", cx);
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "escape");
    cx.wait_for(window.into(), Duration::from_secs(5), |window, _| {
        window.try_find("html-22").is_some_and(|body| {
            body.label() != Some("Rendering") && body.bounds().size.height > before
        })
    })
    .await;
    cx.update_window(window.into(), |_, window, _| {
        assert_eq!(window.find("thread-subject").label(), Some("Thread 2"))
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_failed_save_keeps_the_applied_theme_and_explains_the_error(cx: &mut TestAppContext) {
    let file = TempFile::new();
    std::fs::write(&file.0, "not a directory").unwrap();
    let fixture = Fixture::showcase();
    let (window, _) = mount(cx, fixture.clone());
    *fixture.settings_path.lock().unwrap() = Some(file.0.join("settings.json"));
    cx.simulate_keystrokes(window.into(), "secondary-,");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(("settings-tab", 3usize), cx);
        window.render_frame(cx);
        window.click("theme-linen", cx);
        window.render_frame(cx);
        assert!(window.try_find("settings-error").is_some());
        assert_eq!(window.find("theme-dark").selected(), Some(true));
        assert_eq!(window.find("theme-linen").selected(), Some(false));
    })
    .unwrap();
    assert_eq!(fixture.settings.lock().unwrap().theme, "dark");
}

#[gpui_kit::test]
fn settings_opens_on_general_and_the_rail_lists_every_section_in_order(cx: &mut TestAppContext) {
    let fixture = Fixture::showcase();
    let (window, _) = mount(cx, fixture);
    cx.simulate_keystrokes(window.into(), "secondary-,");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        // General is the first page, so opening Settings lands there without a click.
        assert_eq!(
            window.find(("settings-tab", 0usize)).label(),
            Some("General")
        );
        assert_eq!(
            window.find(("settings-tab", 0usize)).selected(),
            Some(true),
            "Settings opens on General"
        );
        assert!(window.try_find("undo-send-10").is_some());
        // (rail index, rail label, an element only that section renders)
        let sections: [(usize, &str, &str); 8] = [
            (0, "General", "undo-send-10"),
            (1, "Accounts", "settings-account-1"),
            (2, "Connections", "proton-native"),
            (3, "Appearance", "theme-dark"),
            (4, "Typography", "font-size-value"),
            (5, "Shortcuts", "shortcut-reset-all"),
            (6, "Privacy", "remote-images"),
            (7, "AI", "ai-enabled"),
        ];
        for (index, label, marker) in sections {
            let tab = window.find(("settings-tab", index));
            assert_eq!(tab.label(), Some(label), "rail entry {index}");
            window.click(("settings-tab", index), cx);
            window.render_frame(cx);
            assert_eq!(
                window.find(("settings-tab", index)).selected(),
                Some(true),
                "section {index} was not selected"
            );
            assert!(
                window.try_find(marker).is_some(),
                "section {index} ({label}) did not render {marker}"
            );
        }
    })
    .unwrap();
}

#[gpui_kit::test]
fn each_settings_entry_point_lands_on_its_page(cx: &mut TestAppContext) {
    let fixture = Fixture::showcase();
    let (window, _) = mount(cx, fixture);
    // The cog and secondary-, open Settings on General.
    cx.update_window(window.into(), |_, window, cx| {
        window.click("open-settings", cx);
        window.render_frame(cx);
        assert_eq!(window.find(("settings-tab", 0usize)).selected(), Some(true));
        assert!(window.try_find("undo-send-10").is_some());
        window.click("close-settings", cx);
        window.render_frame(cx);
    })
    .unwrap();
    // Manage accounts lands on Accounts.
    cx.simulate_keystrokes(window.into(), "secondary-shift-,");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find(("settings-tab", 1usize)).label(),
            Some("Accounts")
        );
        assert_eq!(window.find(("settings-tab", 1usize)).selected(), Some(true));
        assert!(window.try_find("settings-account-1").is_some());
        window.click("close-settings", cx);
        window.render_frame(cx);
        // The rail's plus button lands on Connections.
        window.click("add-account", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find(("settings-tab", 2usize)).label(),
            Some("Connections")
        );
        assert_eq!(window.find(("settings-tab", 2usize)).selected(), Some(true));
        assert!(window.try_find("proton-native").is_some());
        window.click("close-settings", cx);
        window.render_frame(cx);
    })
    .unwrap();
    // The `?` help entry still opens the shortcut sheet.
    cx.simulate_keystrokes(window.into(), "shift-/");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("help").label(), Some("Keyboard shortcuts"));
    })
    .unwrap();
}
