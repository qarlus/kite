//! Summarize thread from the reading-pane toolbar: the click-first way to Shift+S.
mod common;
use common::{Fixture, mount};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, TestAppContext};
use kite_core::api::Mail as _;
use kite_core::model::{Action, AiRequest, Event, PRIVATE_TAG};

/// The command id behind the toolbar's Summarize button (keys::cmd::AI_SUMMARY).
const SUMMARIZE: usize = 60;

/// A fixture with AI turned on and a model of its own.
fn ai_on() -> std::sync::Arc<Fixture> {
    let fixture = Fixture::new(3);
    let mut settings = fixture.settings.lock().unwrap();
    settings.ai_enabled = true;
    settings.ai_base_url = "https://ai.example.com/v1".into();
    settings.ai_model = "kite-test".into();
    drop(settings);
    fixture
}

fn click_first() -> std::sync::Arc<Fixture> {
    let fixture = ai_on();
    fixture.settings.lock().unwrap().input_mode = "classic".into();
    fixture
}

#[gpui_kit::test]
fn the_toolbar_summarizes_with_the_shift_s_answer_in_keyboard_mode(cx: &mut TestAppContext) {
    let fixture = ai_on();
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "enter");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find(("action", SUMMARIZE)).label(),
            Some("Summarize (shift-s)"),
            "the button names the key that runs the same command"
        );
        window.click(("action", SUMMARIZE), cx);
        window.render_frame(cx);
    })
    .unwrap();
    let requests = fixture.ai_requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 1, "the click asks for a summary");
    assert!(
        matches!(requests[0].1, AiRequest::Summary { thread: 1 }),
        "it runs the same command as Shift+S, with the engine checking privacy"
    );
    // The answer streams into the card above the messages, exactly as it does for the key.
    fixture.emit(Event::Ai {
        req: requests[0].0,
        delta: "They agreed on the budget.".into(),
        done: true,
    });
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("ai-summary").label(),
            Some("They agreed on the budget.")
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn classic_mode_summarizes_from_the_toolbar_where_shift_s_is_unbound(cx: &mut TestAppContext) {
    let fixture = click_first();
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "enter");
    // The single key is unbound, which is why the button has to exist.
    cx.simulate_keystrokes(window.into(), "shift-s");
    assert!(
        fixture.ai_requests.lock().unwrap().is_empty(),
        "classic mode leaves typing alone"
    );
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find(("action", SUMMARIZE)).label(),
            Some("Summarize"),
            "a command with no bound key shows no dead hint"
        );
        window.click(("action", SUMMARIZE), cx);
        window.render_frame(cx);
    })
    .unwrap();
    let requests = fixture.ai_requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 1, "the button is the click-first Shift+S");
    assert!(matches!(requests[0].1, AiRequest::Summary { thread: 1 }));
}

#[gpui_kit::test]
fn a_private_conversation_refuses_the_toolbar_summary_and_says_why(cx: &mut TestAppContext) {
    let fixture = click_first();
    fixture
        .act(
            &[1],
            &Action::Label {
                tag: PRIVATE_TAG.into(),
            },
        )
        .unwrap();
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "enter");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(("action", SUMMARIZE), cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("toast").label(),
            Some("AI: This conversation is private, so AI can't read it"),
            "the refusal is the engine's, shown as it comes back"
        );
        assert!(
            window.try_find("ai-summary").is_none(),
            "nothing was summarized"
        );
    })
    .unwrap();
    assert!(
        fixture.ai_requests.lock().unwrap().is_empty(),
        "a Private conversation never reaches the AI"
    );
}

#[gpui_kit::test]
fn with_ai_off_the_toolbar_offers_no_summary_button(cx: &mut TestAppContext) {
    let fixture = Fixture::new(3);
    let (window, _) = mount(cx, fixture);
    cx.simulate_keystrokes(window.into(), "enter");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("thread-toolbar").is_some(),
            "the rest of the toolbar is still there"
        );
        assert!(
            window.try_find(("action", SUMMARIZE)).is_none(),
            "a helper that cannot run is not advertised, as in the palette"
        );
    })
    .unwrap();
}
