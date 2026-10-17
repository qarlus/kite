//! User-facing preferences, native file import and local persistence.
mod common;
use common::{Fixture, mount, open_window, update_content};
use gpui_kit::test::{TestAppContextExt, TestWindowExt};
use gpui_kit::{AppContext, ScrollDelta, TestAppContext, point, px};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

struct TempFile(PathBuf);
impl TempFile {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!(
                "kite-ui-{}-{}.json",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            )))
    }
}
impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[gpui_kit::test]
fn short_sidebar_keeps_the_account_switcher_anchored_and_switches_by_keyboard(
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
        let switcher = window.find("account-switcher").bounds();
        assert!(switcher.bottom() <= px(480.));
        assert!(switcher.origin.y > px(350.));
        assert!(window.try_find("account-2").is_none());
        window.scroll(
            "sidebar-scroll",
            ScrollDelta::Pixels(point(px(0.), px(-600.))),
            cx,
        );
        window.render_frame(cx);
        assert_eq!(window.find("account-switcher").bounds(), switcher);
        window.click("account-switcher", cx);
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
        assert_eq!(
            window.find("account-switcher").label(),
            Some("Switch account: karl@outlook.com")
        );
        assert!(window.try_find("account-menu").is_none());
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
        assert_eq!(
            window
                .find(("mailbox", kite_core::model::Mailbox::Inbox as usize))
                .selected(),
            Some(false)
        );
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
        window.click("font-larger", cx);
        window.render_frame(cx);
        assert_eq!(window.find("font-size-value").label(), Some("15 px"));
        window.click("close-settings", cx);
        window.render_frame(cx);
        let height = window.find(("row", 0usize)).bounds().size.height.as_f32();
        assert!(
            (height - 44. * 15. / 14.).abs() <= 1.,
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
        assert_eq!(window.find("theme-linen").selected(), Some(true));
        assert_eq!(window.find("font-size-value").label(), Some("15 px"));
    })
    .unwrap();
}

#[gpui_kit::test]
async fn importing_a_real_theme_file_applies_and_persists_its_palette(cx: &mut TestAppContext) {
    let file = TempFile::new();
    std::fs::write(&file.0, kite::preferences::TEMPLATE).unwrap();
    let fixture = Fixture::showcase();
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "secondary-,");
    cx.update_window(window.into(), |_, window, cx| {
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
        assert!(error.label().unwrap().contains("4.5:1"));
        assert!(error.bounds().bottom() <= px(800.));
        assert!(window.try_find("theme-custom").is_none());
        assert_eq!(window.find("theme-dark").selected(), Some(true));
    })
    .unwrap();
    assert_eq!(fixture.settings.lock().unwrap().theme, "dark");
}

#[gpui_kit::test]
fn typography_is_applied_and_bad_contrast_is_rejected_without_changing_mail(
    cx: &mut TestAppContext,
) {
    let fixture = Fixture::showcase();
    let (window, kite) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "secondary-,");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.scroll(
            "settings-scroll",
            ScrollDelta::Pixels(point(px(0.), px(-480.))),
            cx,
        );
        window.render_frame(cx);
        window.click("font-family", cx);
        window.input("Georgia", cx);
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
            Some("Georgia / 14 px")
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
fn general_settings_control_the_actual_footer(cx: &mut TestAppContext) {
    let fixture = Fixture::showcase();
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "secondary-,");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(("settings-tab", 2usize), cx);
        window.render_frame(cx);
        window.click("toggle-shortcuts", cx);
        window.click("undo-send-10", cx);
        window.click("close-settings", cx);
        window.render_frame(cx);
        assert!(window.try_find("shortcut-help").is_none());
        window.click("open-settings", cx);
        window.render_frame(cx);
        window.click(("settings-tab", 2usize), cx);
        window.render_frame(cx);
        assert_eq!(window.find("toggle-shortcuts").label(), Some("Off"));
        assert_eq!(window.find("undo-send-10").selected(), Some(true));
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
        window.click(("settings-tab", 1usize), cx);
        window.render_frame(cx);
        window.click("add-microsoft", cx);
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
        window.click("account-switcher", cx);
        window.render_frame(cx);
        assert!(window.try_find("account-3").is_none());
        assert_eq!(window.find("account-2").label(), Some("karl@outlook.com"));
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
        window.click(("settings-tab", 1usize), cx);
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
fn theme_tiles_preview_the_native_inbox_four_across_or_two_across(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (window, _) = open_window(
        cx,
        Some(gpui_kit::size(px(1200.), px(800.))),
        |window, cx| cx.new(|cx| kite::app::Kite::new(Fixture::showcase(), window, cx)),
    );
    cx.simulate_keystrokes(window.into(), "secondary-,");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        let carbon = window.find("theme-dark").bounds();
        let paper = window.find("theme-light").bounds();
        let midnight = window.find("theme-midnight").bounds();
        assert_eq!(paper.origin.y, carbon.origin.y);
        assert_eq!(midnight.origin.y, carbon.origin.y);
        assert!(midnight.origin.x > paper.origin.x);
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
        let carbon = window.find("theme-dark").bounds();
        let paper = window.find("theme-light").bounds();
        let midnight = window.find("theme-midnight").bounds();
        assert_eq!(paper.origin.y, carbon.origin.y);
        assert!(midnight.origin.y > carbon.origin.y);
        assert_eq!(midnight.origin.x, carbon.origin.x);
    })
    .unwrap();
}

#[gpui_kit::test]
fn compact_settings_fit_beside_the_sidebar(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (window, _) = open_window(
        cx,
        Some(gpui_kit::size(px(720.), px(480.))),
        |window, cx| cx.new(|cx| kite::app::Kite::new(Fixture::showcase(), window, cx)),
    );
    cx.simulate_keystrokes(window.into(), "secondary-,");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        let sidebar = window.find("sidebar").bounds();
        let page = window.find("settings-page").bounds();
        assert!(page.origin.x >= sidebar.right());
        assert!(page.right() <= px(720.));
        assert!(window.find("theme-light").bounds().right() <= px(720.));
        assert!(window.find("account-switcher").bounds().bottom() <= px(480.));
        window.click("close-settings", cx);
        window.render_frame(cx);
        assert!(window.try_find("settings-page").is_none());
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
async fn bridge_errors_clear_the_password_and_require_reentry(cx: &mut TestAppContext) {
    let fixture = Fixture::showcase();
    *fixture.account_error.lock().unwrap() = Some("Bridge is not running".into());
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "secondary-shift-,");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("add-proton", cx);
        window.render_frame(cx);
        window.scroll(
            "settings-scroll",
            ScrollDelta::Pixels(point(px(0.), px(-500.))),
            cx,
        );
        window.render_frame(cx);
        window.click("proton-username", cx);
        window.input("bridge@example.com", cx);
        window.click("proton-password", cx);
        window.input("temporary-bridge-secret", cx);
        window.scroll(
            "settings-scroll",
            ScrollDelta::Pixels(point(px(0.), px(-300.))),
            cx,
        );
        window.render_frame(cx);
        window.click("connect-proton", cx);
    })
    .unwrap();
    cx.wait_for(window.into(), Duration::from_secs(5), |window, _| {
        window.try_find("settings-error").is_some()
    })
    .await;
    cx.update_window(window.into(), |_, window, cx| {
        assert_eq!(
            window.find("settings-error").label(),
            Some("Bridge is not running")
        );
        window.scroll(
            "settings-scroll",
            ScrollDelta::Pixels(point(px(0.), px(-500.))),
            cx,
        );
        window.render_frame(cx);
        window.click("connect-proton", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("settings-error").label(),
            Some("Enter your Bridge username and password.")
        );
    })
    .unwrap();
    assert_eq!(fixture.accounts.lock().unwrap().len(), 2);
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
        window.click("theme-linen", cx);
        window.render_frame(cx);
        assert!(window.try_find("settings-error").is_some());
        assert_eq!(window.find("theme-dark").selected(), Some(true));
        assert_eq!(window.find("theme-linen").selected(), Some(false));
    })
    .unwrap();
    assert_eq!(fixture.settings.lock().unwrap().theme, "dark");
}
