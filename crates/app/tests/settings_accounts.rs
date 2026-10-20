//! Proton and Outlook sign-in on the Connections settings page.
mod common;
use common::{Fixture, mount};
use gpui_kit::test::{TestAppContextExt, TestWindowExt};
use gpui_kit::{AppContext, TestAppContext};
use kite_core::model::ProtonAuth;
use std::time::Duration;

#[gpui_kit::test]
fn the_connections_page_offers_proton_and_outlook_without_bridge_or_app_id_fields(
    cx: &mut TestAppContext,
) {
    let fixture = Fixture::showcase();
    let (window, _) = mount(cx, fixture);
    cx.simulate_keystrokes(window.into(), "secondary-,");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(("settings-tab", 3usize), cx);
        window.render_frame(cx);
        assert!(
            window.try_find("proton-native").is_some(),
            "the Proton sign-in row must be on the page"
        );
        assert!(
            window.try_find("proton-address").is_some(),
            "the native Proton form must be open"
        );
        assert!(
            window.try_find("add-microsoft").is_some(),
            "the Connect Outlook button must be on the page"
        );
        for gone in [
            "proton-bridge-status",
            "look-again-proton",
            "ms-client-id",
            "add-proton",
            "connect-proton",
            "proton-username",
            "proton-password",
        ] {
            assert!(
                window.try_find(gone).is_none(),
                "{gone} must no longer be on the Connections page"
            );
        }
    })
    .unwrap();
}

#[gpui_kit::test]
async fn outlook_connect_adds_the_account(cx: &mut TestAppContext) {
    let fixture = Fixture::showcase();
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "secondary-,");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(("settings-tab", 3usize), cx);
        window.render_frame(cx);
        window.click("add-microsoft", cx);
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
            Some("new@outlook.com")
        );
    })
    .unwrap();
    assert!(
        fixture
            .accounts
            .lock()
            .unwrap()
            .iter()
            .any(|account| account.email == "new@outlook.com"),
        "the Outlook account was not persisted"
    );
}

#[gpui_kit::test]
async fn a_native_proton_sign_in_takes_the_second_step_and_adds_the_account(
    cx: &mut TestAppContext,
) {
    let fixture = Fixture::showcase();
    *fixture.proton_step.lock().unwrap() = Some(ProtonAuth::TwoFactor);
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "secondary-,");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(("settings-tab", 3usize), cx);
        window.render_frame(cx);
        window.click("proton-address", cx);
        window.input("kite@proton.me", cx);
        window.click("proton-secret", cx);
        window.input("my proton password", cx);
        window.click("proton-sign-in", cx);
    })
    .unwrap();
    // The engine stopped on 2FA: the form asks for the code instead of the password.
    cx.wait_for(window.into(), Duration::from_secs(5), |window, _| {
        window.try_find("proton-answer").is_some()
    })
    .await;
    cx.update_window(window.into(), |_, window, cx| {
        assert!(
            window.try_find("proton-secret").is_none(),
            "the password form is replaced by the step it is waiting for"
        );
        window.click("proton-answer", cx);
        window.input("123456", cx);
        window.click("proton-step", cx);
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
            Some("kite@proton.me")
        );
    })
    .unwrap();
    assert_eq!(
        *fixture.proton_attempts.lock().unwrap(),
        vec![(
            "kite@proton.me".to_string(),
            "my proton password".to_string()
        )],
        "the address and password reach the engine, not the Bridge path"
    );
    assert_eq!(
        *fixture.proton_answers.lock().unwrap(),
        vec!["123456".to_string()],
        "the 2FA code is answered on the same sign-in"
    );
}

#[gpui_kit::test]
async fn human_verification_finishes_without_asking_for_a_token(cx: &mut TestAppContext) {
    let fixture = Fixture::showcase();
    *fixture.proton_step.lock().unwrap() = Some(ProtonAuth::HumanVerification {
        url: "https://verify.proton.me/?methods=captcha&token=challenge".into(),
    });
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "secondary-,");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(("settings-tab", 3usize), cx);
        window.render_frame(cx);
        window.click("proton-address", cx);
        window.input("kite@proton.me", cx);
        window.click("proton-secret", cx);
        window.input("my proton password", cx);
        window.click("proton-sign-in", cx);
    })
    .unwrap();
    cx.wait_for(window.into(), Duration::from_secs(5), |window, _| {
        window.try_find("proton-verify").is_some()
    })
    .await;
    cx.update_window(window.into(), |_, window, cx| {
        // Proton's verify page hands no token to the user, so there is nothing to type.
        assert!(window.try_find("proton-answer").is_none());
        window.click("proton-step", cx);
        window.render_frame(cx);
        window.click(("settings-tab", 2usize), cx);
    })
    .unwrap();
    cx.wait_for(window.into(), Duration::from_secs(5), |window, _| {
        window.try_find("settings-account-3").is_some()
    })
    .await;
    assert_eq!(
        *fixture.proton_answers.lock().unwrap(),
        vec![String::new()],
        "the retry carries no typed answer; the engine reuses Proton's challenge"
    );
}
