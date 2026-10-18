//! Proton Bridge discovery on the Connections settings page.
mod common;
use common::{Fixture, mount};
use gpui_kit::test::{TestAppContextExt, TestWindowExt};
use gpui_kit::{AppContext, ScrollDelta, TestAppContext, point, px};
use kite_core::model::ProtonLogin;
use std::time::Duration;

#[gpui_kit::test]
async fn a_discovered_bridge_account_is_created_without_any_click(cx: &mut TestAppContext) {
    let fixture = Fixture::showcase();
    *fixture.discovered.lock().unwrap() = vec![ProtonLogin {
        email: "bridge@proton.me".into(),
        password: "generated-bridge-secret".into(),
        host: Some("127.0.0.1".into()),
        imap_port: Some(1143),
        smtp_port: Some(1025),
    }];
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "secondary-,");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        // Opening Connections is the only action: discovery and creation follow.
        window.click(("settings-tab", 3usize), cx);
        window.render_frame(cx);
        assert!(
            window.try_find("proton-bridge-status").is_some(),
            "the Connections page must show its Bridge status"
        );
        window.click(("settings-tab", 2usize), cx);
    })
    .unwrap();
    cx.wait_for(window.into(), Duration::from_secs(5), |window, _| {
        window.try_find("settings-account-3").is_some()
    })
    .await;
    cx.update_window(window.into(), |_, window, _| {
        assert_eq!(
            window.find("settings-account-3").label(),
            Some("bridge@proton.me")
        );
        assert_eq!(
            window.find("settings-notice").label(),
            Some("Account connected")
        );
    })
    .unwrap();
    assert!(
        fixture
            .accounts
            .lock()
            .unwrap()
            .iter()
            .any(|account| account.email == "bridge@proton.me"),
        "the discovered account was not created"
    );
}

#[gpui_kit::test]
async fn failed_discovery_shows_the_error_and_the_manual_form_still_connects(
    cx: &mut TestAppContext,
) {
    let fixture = Fixture::showcase();
    *fixture.discover_error.lock().unwrap() = Some("Bridge vault is locked".into());
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "secondary-,");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(("settings-tab", 3usize), cx);
    })
    .unwrap();
    cx.wait_for(window.into(), Duration::from_secs(5), |window, _| {
        window.try_find("settings-error").is_some() && window.try_find("proton-username").is_some()
    })
    .await;
    cx.update_window(window.into(), |_, window, cx| {
        assert_eq!(
            window.find("settings-error").label(),
            Some("Could not look for Proton Bridge: Bridge vault is locked")
        );
        window.scroll(
            "settings-scroll",
            ScrollDelta::Pixels(point(px(0.), px(-600.))),
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
        window.render_frame(cx);
        window.click(("settings-tab", 2usize), cx);
    })
    .unwrap();
    cx.wait_for(window.into(), Duration::from_secs(5), |window, _| {
        window.try_find("settings-account-3").is_some()
    })
    .await;
    cx.update_window(window.into(), |_, window, _| {
        assert_eq!(
            window.find("settings-account-3").label(),
            Some("bridge@example.com")
        );
    })
    .unwrap();
    assert!(
        fixture
            .accounts
            .lock()
            .unwrap()
            .iter()
            .any(|account| account.email == "bridge@example.com"),
        "the manually connected account was not persisted"
    );
}

#[gpui_kit::test]
async fn a_closed_bridge_is_started_and_its_account_is_created(cx: &mut TestAppContext) {
    let fixture = Fixture::showcase();
    *fixture.discover_error.lock().unwrap() = Some("Proton Bridge is not running".into());
    *fixture.bridge_installed.lock().unwrap() = true;
    *fixture.bridge_accounts.lock().unwrap() = vec![ProtonLogin {
        email: "bridge@proton.me".into(),
        password: "generated-bridge-secret".into(),
        host: Some("127.0.0.1".into()),
        imap_port: Some(1143),
        smtp_port: Some(1025),
    }];
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "secondary-,");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(("settings-tab", 3usize), cx);
        window.render_frame(cx);
        window.click(("settings-tab", 2usize), cx);
    })
    .unwrap();
    cx.wait_for(window.into(), Duration::from_secs(6), |window, _| {
        window.try_find("settings-account-3").is_some()
    })
    .await;
    assert_eq!(
        *fixture.bridge_started.lock().unwrap(),
        1,
        "Kite must start a closed Bridge exactly once"
    );
    assert!(
        fixture
            .accounts
            .lock()
            .unwrap()
            .iter()
            .any(|account| account.email == "bridge@proton.me"),
        "the account from the started Bridge was not created"
    );
}

#[gpui_kit::test]
async fn a_started_bridge_that_stays_locked_asks_for_a_proton_sign_in(cx: &mut TestAppContext) {
    let fixture = Fixture::showcase();
    *fixture.discover_error.lock().unwrap() = Some("Proton Bridge is not running".into());
    *fixture.bridge_installed.lock().unwrap() = true;
    // `bridge_accounts` stays empty: Bridge starts but is waiting to be unlocked.
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "secondary-,");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(("settings-tab", 3usize), cx);
    })
    .unwrap();
    cx.wait_for(window.into(), Duration::from_secs(15), |window, _| {
        window
            .try_find("proton-bridge-status")
            .and_then(|status| status.label().map(|label| label.contains("sign in to Proton")))
            .unwrap_or(false)
    })
    .await;
    cx.update_window(window.into(), |_, window, _| {
        assert!(
            window.try_find("settings-account-3").is_none(),
            "a locked Bridge must not create an account"
        );
    })
    .unwrap();
    assert_eq!(*fixture.bridge_started.lock().unwrap(), 1);
}
