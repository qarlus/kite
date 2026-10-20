//! Appearance: the theme gallery, typography and its live preview, through the real view.
mod common;
use common::{Fixture, mount, read_settings, update_content};
use gpui_kit::base::Root;
use gpui_kit::test::{TestAppContextExt, TestWindowExt};
use gpui_kit::{AppContext, ScrollDelta, TestAppContext, WindowHandle, point, px};
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
            "kite-appearance-{}-{}-{}.json",
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

/// The resolved system font, which is what the preview names when no family is chosen.
fn system_font() -> String {
    kite::theme::FONT.to_string()
}

/// Opens the font picker, types a query, and confirms the first match with the keyboard.
async fn pick_font(window: WindowHandle<Root>, cx: &mut TestAppContext, query: &str) {
    cx.update_window(window.into(), |_, window, cx| {
        window.within("font-family").click("input", cx);
        window.input(query, cx);
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
}

#[gpui_kit::test]
fn choosing_a_theme_and_a_larger_text_size_persists_across_a_reopen(cx: &mut TestAppContext) {
    let file = TempFile::new();
    let fixture = Fixture::showcase();
    let (window, _) = mount(cx, fixture.clone());
    // Set the path after mounting so the fixture starts from its in-memory Carbon default.
    *fixture.settings_path.lock().unwrap() = Some(file.0.clone());
    cx.simulate_keystrokes(window.into(), "secondary-,");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("theme-dark").selected(), Some(true));
        window.click("theme-midnight", cx);
        window.render_frame(cx);
        assert_eq!(window.find("theme-midnight").selected(), Some(true));
        assert_eq!(window.find("theme-dark").selected(), Some(false));
        window.click(("settings-tab", 1usize), cx);
        window.render_frame(cx);
        window.click("font-larger", cx);
        window.render_frame(cx);
        assert_eq!(window.find("font-size-value").label(), Some("15 px"));
        assert_eq!(
            window.find("typography-preview").label(),
            Some(format!("{} / 15 px", system_font()).as_str())
        );
    })
    .unwrap();
    let saved = read_settings(&file.0).unwrap();
    assert_eq!(saved.theme, "midnight");
    assert_eq!(saved.font_size, 15);

    let (reopened, _) = mount(cx, fixture);
    cx.simulate_keystrokes(reopened.into(), "secondary-,");
    cx.update_window(reopened.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("theme-midnight").selected(), Some(true));
        assert_eq!(window.find("theme-dark").selected(), Some(false));
        window.click(("settings-tab", 1usize), cx);
        window.render_frame(cx);
        assert_eq!(window.find("font-size-value").label(), Some("15 px"));
        assert_eq!(
            window.find("typography-preview").label(),
            Some(format!("{} / 15 px", system_font()).as_str())
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_live_preview_and_its_colour_swatch_follow_the_applied_typography(cx: &mut TestAppContext) {
    let fixture = Fixture::showcase();
    // The headless text system reports no installed fonts; a saved family is the row
    // the picker has to keep listing, and it is what the preview names.
    fixture.settings.lock().unwrap().font_family = "Georgia".into();
    let (window, kite) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "secondary-,");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(("settings-tab", 1usize), cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("font-color-swatch").label(),
            Some("Text colour not set")
        );
        window.scroll(
            "settings-scroll",
            ScrollDelta::Pixels(point(px(0.), px(-480.))),
            cx,
        );
        window.render_frame(cx);
        assert_eq!(
            window.find("font-family").value(),
            Some("Georgia"),
            "a saved face is the selected value even when it is not installed"
        );
        window.click("font-color", cx);
        window.input("#d8d8e8", cx);
        // The swatch mirrors the field as it is typed, without applying it.
        assert_eq!(
            window.find("font-color-swatch").label(),
            Some("Text colour #d8d8e8")
        );
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
    })
    .unwrap();
    let applied = fixture.settings.lock().unwrap().clone();
    assert_eq!(applied.font_family, "Georgia");
    assert_eq!(applied.font_color.as_deref(), Some("#d8d8e8"));
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(
            kite.colours().0,
            "#d8d8e8",
            "the chosen text colour is the ink the app paints"
        );
    });

    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("reset-typography", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("typography-preview").label(),
            Some(format!("{} / 14 px", system_font()).as_str())
        );
        assert_eq!(
            window.find("font-color-swatch").label(),
            Some("Text colour not set")
        );
    })
    .unwrap();
    let settings = fixture.settings.lock().unwrap().clone();
    assert_eq!(settings.font_family, "");
    assert_eq!(settings.font_color, None);
    assert_eq!(settings.font_size, 14);
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.colours().0, "#e9e9ee", "Reset returns to Carbon's ink");
    });
}

#[gpui_kit::test]
async fn a_font_is_picked_from_the_list_and_system_default_clears_it(cx: &mut TestAppContext) {
    let fixture = Fixture::showcase();
    // The headless text system installs no fonts, so a saved face is the one extra row:
    // it has to show as the value and stay pickable instead of crashing.
    fixture.settings.lock().unwrap().font_family = "Georgia".into();
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "secondary-,");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(("settings-tab", 1usize), cx);
        window.render_frame(cx);
        assert_eq!(window.find("font-family").value(), Some("Georgia"));
        assert_eq!(
            window.find("typography-preview").label(),
            Some("Georgia / 14 px")
        );
        window.scroll(
            "settings-scroll",
            ScrollDelta::Pixels(point(px(0.), px(-480.))),
            cx,
        );
        window.render_frame(cx);
        assert_eq!(
            window.find("font-family").expanded(),
            Some(false),
            "the picker starts closed"
        );
    })
    .unwrap();

    // Choosing the first row applies it and repaints the preview in the system face.
    pick_font(window, cx, "system").await;
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("font-family").value(), Some("System default"));
        assert_eq!(
            window.find("typography-preview").label(),
            Some(format!("{} / 14 px", system_font()).as_str())
        );
    })
    .unwrap();
    assert_eq!(fixture.settings.lock().unwrap().font_family, "");

    // And picking a named face again saves it straight away.
    pick_font(window, cx, "geor").await;
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("font-family").value(), Some("Georgia"));
        assert_eq!(
            window.find("typography-preview").label(),
            Some("Georgia / 14 px")
        );
    })
    .unwrap();
    assert_eq!(fixture.settings.lock().unwrap().font_family, "Georgia");
}

#[gpui_kit::test]
fn the_ember_theme_can_be_picked_and_persists(cx: &mut TestAppContext) {
    let file = TempFile::new();
    let fixture = Fixture::showcase();
    let (window, kite) = mount(cx, fixture.clone());
    *fixture.settings_path.lock().unwrap() = Some(file.0.clone());
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.colours().0, "#e9e9ee", "Carbon's ink first");
    });
    cx.simulate_keystrokes(window.into(), "secondary-,");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("theme-ember", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("theme-ember").selected(),
            Some(true),
            "picked: {:?}",
            window
                .try_find("settings-error")
                .and_then(|e| e.label().map(str::to_owned))
        );
        assert_eq!(window.find("theme-dark").selected(), Some(false));
    })
    .unwrap();
    // The palette reaches the running app, not only the settings form.
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(
            kite.colours(),
            ("#f3eee8".to_string(), "#0b0b0b".to_string()),
            "Ember paints its own ink and page"
        );
    });
    assert_eq!(read_settings(&file.0).unwrap().theme, "ember");

    let (reopened, kite) = mount(cx, fixture);
    cx.simulate_keystrokes(reopened.into(), "secondary-,");
    cx.update_window(reopened.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("theme-ember").selected(), Some(true));
    })
    .unwrap();
    update_content(reopened, &kite, cx, |kite, _, _| {
        assert_eq!(kite.colours().1, "#0b0b0b", "and it survives a reopen");
    });
}

#[gpui_kit::test]
fn a_larger_text_size_reflows_the_real_list_and_reading_pane(cx: &mut TestAppContext) {
    let fixture = Fixture::showcase();
    let (window, _) = mount(cx, fixture.clone());
    let before = cx
        .update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            window.find(("row", 0usize)).bounds().size.height
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), "secondary-,");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(("settings-tab", 1usize), cx);
        window.render_frame(cx);
        window.click("font-larger", cx);
        window.click("font-larger", cx);
        window.render_frame(cx);
        assert_eq!(window.find("font-size-value").label(), Some("16 px"));
        window.click("close-settings", cx);
        window.render_frame(cx);
        let after = window.find(("row", 0usize)).bounds().size.height;
        assert!(
            after > before,
            "the list rows grow with the type: {before:?} then {after:?}"
        );
    })
    .unwrap();
    assert_eq!(fixture.settings.lock().unwrap().font_size, 16);
}
