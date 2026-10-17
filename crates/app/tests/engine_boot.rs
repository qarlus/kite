//! The real app boots over the production engine: sidebar, empty inbox and settings.
mod common;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, TestAppContext};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

#[gpui_kit::test]
fn the_engine_boots_the_sidebar_the_empty_inbox_and_settings(cx: &mut TestAppContext) {
    let dir = std::env::temp_dir().join(format!(
        "kite-engine-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let engine = kite_core::engine::Engine::open_with(
        &dir,
        kite_core::engine::Secrets::Memory(Mutex::new(HashMap::new())),
        kite_core::engine::Options::default(),
    )
    .expect("open the engine in a temp dir");
    cx.update(gpui_kit::init);
    let (window, _) = common::open_window(
        cx,
        Some(gpui_kit::size(gpui_kit::px(1200.), gpui_kit::px(800.))),
        |window, cx| cx.new(|cx| kite::app::Kite::new(engine, window, cx)),
    );
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("sidebar").is_some(),
            "the sidebar is visible"
        );
        assert_eq!(
            window.find("inbox-zero").label(),
            Some("Welcome to Kite"),
            "an engine with no accounts shows the first-run empty state"
        );
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "secondary-,");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("settings-page").is_some(),
            "Ctrl/Cmd+, opens Settings"
        );
    })
    .unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}
