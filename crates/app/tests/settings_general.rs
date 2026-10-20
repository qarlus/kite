//! General settings: privacy, AI and shortcut-free typing.
mod common;
use common::{Fixture, mount, open_window};
use gpui_kit::test::{TestAppContextExt, TestWindowExt};
use gpui_kit::{AppContext, ScrollDelta, TestAppContext, point, px};

/// Opens Settings on the given rail section.
fn open_section(
    window: gpui_kit::WindowHandle<gpui_kit::base::Root>,
    cx: &mut TestAppContext,
    index: usize,
) {
    cx.simulate_keystrokes(window.into(), "secondary-,");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(("settings-tab", index), cx);
        window.render_frame(cx);
    })
    .unwrap();
}

/// Scrolls the settings form to its end so the AI controls are in the viewport.
fn scroll_to_bottom(window: &mut gpui_kit::Window, cx: &mut gpui_kit::App) {
    window.scroll(
        "settings-scroll",
        ScrollDelta::Pixels(point(px(0.), px(-1_200.))),
        cx,
    );
    window.render_frame(cx);
}

#[gpui_kit::test]
fn remote_images_toggle_persists_and_is_off_by_default(cx: &mut TestAppContext) {
    let fixture = Fixture::showcase();
    let (window, _) = mount(cx, fixture.clone());
    open_section(window, cx, kite::preferences::Page::Privacy.index());
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("remote-images").checked(), Some(false));
        window.click("remote-images", cx);
        window.render_frame(cx);
        assert_eq!(window.find("remote-images").checked(), Some(true));
    })
    .unwrap();
    assert!(fixture.settings.lock().unwrap().load_remote_images);
}

#[gpui_kit::test]
fn ai_settings_persist_and_the_key_goes_to_the_keychain(cx: &mut TestAppContext) {
    let fixture = Fixture::showcase();
    let (window, _) = mount(cx, fixture.clone());
    open_section(window, cx, kite::preferences::Page::Ai.index());
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("ai-base-url").is_none(),
            "AI fields must stay out of the way while AI is off"
        );
        scroll_to_bottom(window, cx);
        window.click("ai-enabled", cx);
        window.render_frame(cx);
        assert_eq!(window.find("ai-enabled").checked(), Some(true));
        scroll_to_bottom(window, cx);
        window.click("ai-provider-custom", cx);
        window.render_frame(cx);
        scroll_to_bottom(window, cx);
        window.click("ai-base-url", cx);
        window.press("secondary-a", cx);
        window.input("https://example.com/v1", cx);
        window.click("ai-model", cx);
        window.press("secondary-a", cx);
        window.input("gpt-test", cx);
        window.click("ai-key", cx);
        window.input("sk-secret-key", cx);
        window.click("save-ai", cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
    let settings = fixture.settings.lock().unwrap();
    assert!(settings.ai_enabled);
    assert_eq!(settings.ai_base_url, "https://example.com/v1");
    assert_eq!(settings.ai_model, "gpt-test");
    drop(settings);
    assert_eq!(
        fixture.ai_key.lock().unwrap().as_deref(),
        Some("sk-secret-key")
    );
}

#[gpui_kit::test]
fn the_undo_send_chips_fit_the_minimum_window(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    // Five chips used to be laid out beside a label that could not shrink.
    let (window, _) = open_window(
        cx,
        Some(gpui_kit::size(px(720.), px(480.))),
        |window, cx| cx.new(|cx| kite::app::Kite::new(Fixture::showcase(), window, cx)),
    );
    cx.simulate_keystrokes(window.into(), "secondary-,");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(("settings-tab", 0usize), cx);
        window.render_frame(cx);
        let page = window.find("settings-page").bounds();
        for seconds in [0, 5, 10, 20, 30] {
            let chip = window.find(format!("undo-send-{seconds}")).bounds();
            assert!(
                chip.left() >= page.left() && chip.right() <= page.right(),
                "undo-send-{seconds} sits outside the settings page"
            );
        }
    })
    .unwrap();
}

#[gpui_kit::test]
fn typing_in_ai_inputs_never_runs_mail_shortcuts(cx: &mut TestAppContext) {
    let fixture = Fixture::showcase();
    let (window, _) = mount(cx, fixture.clone());
    open_section(window, cx, kite::preferences::Page::Ai.index());
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        scroll_to_bottom(window, cx);
        window.click("ai-enabled", cx);
        window.render_frame(cx);
        scroll_to_bottom(window, cx);
        window.click("ai-provider-custom", cx);
        window.render_frame(cx);
        scroll_to_bottom(window, cx);
        window.click("ai-base-url", cx);
        window.input("jekyll", cx);
        window.press("e", cx);
        window.press("j", cx);
        window.click("ai-model", cx);
        window.input("k9", cx);
        window.press("#", cx);
    })
    .unwrap();
    assert!(
        fixture.actions().is_empty(),
        "typing in General must never run mail shortcuts"
    );
}

#[gpui_kit::test]
fn the_input_mode_choice_is_shown_and_saved(cx: &mut TestAppContext) {
    let fixture = Fixture::showcase();
    let (window, _) = mount(cx, fixture.clone());
    open_section(window, cx, 0);
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("input-mode-keyboard").selected(),
            Some(true),
            "keyboard-first is the default"
        );
        window.click("input-mode-classic", cx);
        window.render_frame(cx);
        assert_eq!(window.find("input-mode-classic").selected(), Some(true));
        assert_eq!(window.find("input-mode-keyboard").selected(), Some(false));
    })
    .unwrap();
    assert_eq!(fixture.settings.lock().unwrap().input_mode, "classic");
}

#[gpui_kit::test]
fn the_undo_send_window_from_settings_decides_whether_z_still_cancels(cx: &mut TestAppContext) {
    // Off means send at once: by the time the writer can reach z, the mail is gone.
    let fixture = Fixture::new(2);
    let (window, _) = mount(cx, fixture.clone());
    open_section(window, cx, 0);
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("undo-send-0", cx);
        window.render_frame(cx);
        window.click("close-settings", cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert_eq!(fixture.settings.lock().unwrap().undo_send_secs, 0);
    cx.simulate_keystrokes(window.into(), "c");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("compose-to", cx);
        window.input("alice@example.com", cx);
        window.click("compose-body", cx);
        window.input("Sent for good", cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "secondary-enter");
    assert_eq!(fixture.drafts().len(), 1);
    cx.background_executor
        .advance_clock(std::time::Duration::from_secs(1));
    cx.run_until_parked();
    cx.simulate_keystrokes(window.into(), "z");
    assert!(
        fixture.cancelled().is_empty(),
        "a zero-second window means the message has already left"
    );
}

#[gpui_kit::test]
fn a_typed_undo_send_window_keeps_the_send_cancellable(cx: &mut TestAppContext) {
    let fixture = Fixture::new(2);
    fixture.settings.lock().unwrap().undo_send_secs = 30;
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "c");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("compose-to", cx);
        window.input("alice@example.com", cx);
        window.click("compose-body", cx);
        window.input("Still mine", cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "secondary-enter");
    cx.simulate_keystrokes(window.into(), "z");
    assert_eq!(
        fixture.cancelled(),
        vec![9001],
        "z takes the message back inside the configured window"
    );
}

// Keep this update stub local: the shared mail fixture should never contact GitHub.
struct UpdateMail {
    inner: std::sync::Arc<Fixture>,
    result: std::sync::Mutex<Result<Option<kite_core::update::Release>, String>>,
}

use anyhow::Result;
use kite_core::api::Mail;
use kite_core::model::*;

impl Mail for UpdateMail {
    fn accounts(&self) -> Result<Vec<Account>> {
        self.inner.accounts()
    }
    fn add_microsoft(&self) -> Result<Account> {
        self.inner.add_microsoft()
    }
    fn remove_account(&self, id: Id) -> Result<()> {
        self.inner.remove_account(id)
    }
    fn threads(&self, view: &View, offset: u32, limit: u32) -> Result<Vec<ThreadSummary>> {
        self.inner.threads(view, offset, limit)
    }
    fn thread(&self, id: Id) -> Result<Vec<Message>> {
        self.inner.thread(id)
    }
    fn act(&self, ids: &[Id], action: &Action) -> Result<i64> {
        self.inner.act(ids, action)
    }
    fn undo(&self, token: i64) -> Result<()> {
        self.inner.undo(token)
    }
    fn send(&self, draft: Draft) -> Result<Id> {
        self.inner.send(draft)
    }
    fn cancel_send(&self, id: Id) -> Result<Option<Draft>> {
        self.inner.cancel_send(id)
    }
    fn contacts(&self, query: &str) -> Result<Vec<Addr>> {
        self.inner.contacts(query)
    }
    fn open_attachment(&self, id: Id) -> Result<()> {
        self.inner.open_attachment(id)
    }
    fn splits(&self) -> Result<Vec<Split>> {
        self.inner.splits()
    }
    fn set_splits(&self, splits: Vec<Split>) -> Result<()> {
        self.inner.set_splits(splits)
    }
    fn settings(&self) -> Result<Settings> {
        self.inner.settings()
    }
    fn set_settings(&self, settings: Settings) -> Result<()> {
        self.inner.set_settings(settings)
    }
    fn set_ai_key(&self, key: &str) -> Result<()> {
        self.inner.set_ai_key(key)
    }
    fn calendar_today(&self, account: Id) -> Result<Vec<CalEvent>> {
        self.inner.calendar_today(account)
    }
    fn ai(&self, req: u32, request: AiRequest) -> Result<()> {
        self.inner.ai(req, request)
    }
    fn sync_now(&self) {
        self.inner.sync_now()
    }
    fn events(&self) -> async_channel::Receiver<Event> {
        self.inner.events()
    }
    fn check_update(&self, _current: &str) -> Result<Option<kite_core::update::Release>> {
        self.result
            .lock()
            .unwrap()
            .clone()
            .map_err(anyhow::Error::msg)
    }
}

#[gpui_kit::test]
async fn updates_show_current_version_and_manual_check_results(cx: &mut TestAppContext) {
    let fixture = Fixture::showcase();
    let mail = std::sync::Arc::new(UpdateMail {
        inner: fixture.clone(),
        result: std::sync::Mutex::new(Ok(None)),
    });
    let (window, _) = mount(cx, mail.clone());
    open_section(window, cx, 0);
    cx.update_window(window.into(), |_, window, cx| {
        scroll_to_bottom(window, cx);
        assert_eq!(
            window.find("kite-version").label(),
            Some(concat!("Kite ", env!("CARGO_PKG_VERSION")))
        );
        assert_eq!(window.find("auto-update-check").checked(), Some(true));
        window.click("auto-update-check", cx);
        window.render_frame(cx);
        window.click("check-update", cx);
    })
    .unwrap();
    assert!(!fixture.settings.lock().unwrap().auto_update_check);
    cx.wait_for(
        window.into(),
        std::time::Duration::from_secs(5),
        |window, _| window.find("update-status").label() == Some("Up to date"),
    )
    .await;
    *mail.result.lock().unwrap() = Ok(Some(kite_core::update::Release {
        version: "9.0.0".into(),
        notes: "A signed update".into(),
        windows: kite_core::update::Asset {
            url: "https://example.invalid/kite.exe".into(),
            sha256: "00".repeat(32),
            signature: String::new(),
        },
    }));
    cx.update_window(window.into(), |_, window, cx| {
        scroll_to_bottom(window, cx);
        window.click("check-update", cx);
    })
    .unwrap();
    cx.wait_for(
        window.into(),
        std::time::Duration::from_secs(5),
        |window, _| window.find("update-status").label() == Some("Kite 9.0.0 is available"),
    )
    .await;
    cx.update_window(window.into(), |_, window, cx| {
        scroll_to_bottom(window, cx);
        let label = if cfg!(all(windows, target_arch = "x86_64")) {
            "Update and restart"
        } else {
            "Download from GitHub"
        };
        assert_eq!(window.find("install-update").label(), Some(label));
    })
    .unwrap();
    *mail.result.lock().unwrap() = Err("Test network failure".into());
    cx.update_window(window.into(), |_, window, cx| {
        window.click("check-update", cx);
    })
    .unwrap();
    cx.wait_for(
        window.into(),
        std::time::Duration::from_secs(5),
        |window, _| {
            window
                .try_find("check-update")
                .is_some_and(|button| button.label() == Some("Retry"))
        },
    )
    .await;
    cx.update_window(window.into(), |_, window, _| {
        assert!(
            window
                .find("update-status")
                .label()
                .unwrap()
                .contains("Test network failure")
        );
    })
    .unwrap();
}
