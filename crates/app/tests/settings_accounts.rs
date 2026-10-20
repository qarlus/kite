//! Proton and Outlook sign-in on the Connections settings page.
mod common;
use common::{Fixture, mount};
use gpui_kit::test::{TestAppContextExt, TestWindowExt};
use gpui_kit::{AppContext, TestAppContext};
use kite_core::api::Mail;
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
        window.click(("settings-tab", 2usize), cx);
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
        window.click(("settings-tab", 2usize), cx);
        window.render_frame(cx);
        window.click("add-microsoft", cx);
        window.render_frame(cx);
        window.click(("settings-tab", 1usize), cx);
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
        window.click(("settings-tab", 2usize), cx);
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
        window.click(("settings-tab", 1usize), cx);
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
        window.click(("settings-tab", 2usize), cx);
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
        window.click(("settings-tab", 1usize), cx);
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

// ---- per-account profile -----------------------------------------------------

#[gpui_kit::test]
fn the_accounts_page_shows_profile_fields_per_account(cx: &mut TestAppContext) {
    let fixture = Fixture::showcase();
    let (window, _) = mount(cx, fixture);
    cx.simulate_keystrokes(window.into(), "secondary-,");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(("settings-tab", 1usize), cx);
        window.render_frame(cx);
        for id in [1, 2] {
            assert!(
                window.try_find(format!("settings-account-{id}")).is_some(),
                "account {id} card"
            );
            assert!(
                window.try_find(format!("account-signature-{id}")).is_some(),
                "account {id} signature field"
            );
            assert!(
                window.try_find(format!("choose-avatar-{id}")).is_some(),
                "account {id} avatar picker"
            );
            assert!(
                window.try_find(format!("account-name-{id}")).is_some(),
                "account {id} display name field"
            );
        }
    })
    .unwrap();
}

#[gpui_kit::test]
fn editing_the_signature_persists_it_through_settings(cx: &mut TestAppContext) {
    let fixture = Fixture::showcase();
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "secondary-,");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(("settings-tab", 1usize), cx);
        window.render_frame(cx);
        window.click("account-signature-1", cx);
        window.input("Regards, Karl", cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert_eq!(
        fixture
            .settings
            .lock()
            .unwrap()
            .profiles
            .get(&1)
            .map(|profile| profile.signature.clone())
            .unwrap_or_default(),
        "Regards, Karl",
        "the signature reached Mail::set_settings"
    );
}

#[gpui_kit::test]
fn editing_the_display_name_renames_the_account(cx: &mut TestAppContext) {
    let fixture = Fixture::showcase();
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "secondary-,");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(("settings-tab", 1usize), cx);
        window.render_frame(cx);
        window.click("account-name-1", cx);
        // The field holds the current name; clear it before typing the new one.
        for _ in 0..20 {
            window.press("backspace", cx);
        }
        window.input("Karl M", cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert_eq!(
        fixture.accounts.lock().unwrap()[0].name,
        "Karl M",
        "the display name reached the store through Mail::set_account_name"
    );
}

#[gpui_kit::test]
async fn a_late_added_accounts_remove_button_stays_reachable(cx: &mut TestAppContext) {
    // The account cards grow when profile fields exist, but a freshly connected
    // account's Remove button must stay above the notice bar (regression).
    let fixture = Fixture::showcase();
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "secondary-,");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(("settings-tab", 2usize), cx);
        window.render_frame(cx);
        window.click("add-microsoft", cx);
        window.render_frame(cx);
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
        assert!(
            window.try_find("confirm-remove-account").is_some(),
            "confirm appears"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_rail_shows_the_avatar_image_only_when_the_file_exists(cx: &mut TestAppContext) {
    let fixture = Fixture::showcase();
    assert!(
        fixture.avatar(1).is_none(),
        "no avatar file yet: initials must show"
    );
    // The user-picked file: a real PNG under the fixture's own data dir.
    fixture.save_avatar(1, common::PNG).unwrap();
    let (window, _) = mount(cx, fixture);
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find(("account-avatar", 0usize)).is_some(),
            "the rail shows the avatar image when the file exists"
        );
        assert!(
            window.try_find(("account-avatar", 1usize)).is_none(),
            "the second account still shows initials"
        );
    })
    .unwrap();
}
