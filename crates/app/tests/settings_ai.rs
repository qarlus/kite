//! AI settings: the optional writing and summary helpers, and their absence when AI is off.
mod common;
use common::{Fixture, mount, update_content};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, TestAppContext};
use kite_core::model::{AiRequest, Event};

/// A fixture with AI turned on and a model of its own.
fn ai_on() -> std::sync::Arc<Fixture> {
    let fixture = Fixture::new(2);
    let mut settings = fixture.settings.lock().unwrap();
    settings.ai_enabled = true;
    settings.ai_base_url = "https://ai.example.com/v1".into();
    settings.ai_model = "kite-test".into();
    drop(settings);
    fixture
}

#[gpui_kit::test]
fn with_ai_off_shift_s_explains_itself_and_the_interface_hides_the_helper(cx: &mut TestAppContext) {
    let fixture = Fixture::new(2);
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "enter");
    cx.simulate_keystrokes(window.into(), "shift-s");
    assert!(
        fixture.ai_requests.lock().unwrap().is_empty(),
        "an off switch must not reach the engine"
    );
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("toast").label(),
            Some("AI is off. Turn it on in Settings › AI.")
        );
        assert!(window.try_find("ai-summary").is_none());
        window.click("shortcut-help", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("help-ai_summary").is_none(),
            "the cheat sheet does not advertise a helper that cannot run"
        );
        window.click("help-close", cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "secondary-k");
    cx.simulate_keystrokes(window.into(), "s u m m");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find(("palette-row", 0usize)).is_none(),
            "nor does the palette"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn shift_s_summarizes_the_open_thread_with_the_configured_endpoint(cx: &mut TestAppContext) {
    let fixture = ai_on();
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "enter");
    cx.simulate_keystrokes(window.into(), "shift-s");
    let requests = fixture.ai_requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 1, "shift-s asks the engine for a summary");
    let req = requests[0].0;
    assert!(
        matches!(requests[0].1, AiRequest::Summary { thread: 1 }),
        "the engine receives the source identity and builds the prompt after checking privacy"
    );
    // The engine streams the answer back as one delta per chunk.
    fixture.emit(Event::Ai {
        req,
        delta: "They asked for ".into(),
        done: false,
    });
    fixture.emit(Event::Ai {
        req,
        delta: "the budget.".into(),
        done: true,
    });
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("ai-summary").label(),
            Some("They asked for the budget."),
            "the streamed answer is shown as it arrives"
        );
        window.click("ai-summary-dismiss", cx);
        window.render_frame(cx);
        assert!(window.try_find("ai-summary").is_none());
    })
    .unwrap();
    // With the switch on, the same command is discoverable from the palette.
    cx.simulate_keystrokes(window.into(), "secondary-k");
    cx.simulate_keystrokes(window.into(), "s u m m");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find(("palette-row", 0usize)).label(),
            Some("Summarize thread")
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn ctrl_j_writes_a_draft_that_can_be_inserted_into_compose(cx: &mut TestAppContext) {
    let fixture = ai_on();
    let (window, kite) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "c");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("compose-to", cx);
        window.input("alice@example.com", cx);
        window.click("compose-subject", cx);
        window.input("Q3 plan", cx);
        window.click("compose-body", cx);
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "secondary-j");
    let requests = fixture.ai_requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 1, "ctrl-j asks the engine to write");
    let req = requests[0].0;
    assert!(
        matches!(&requests[0].1, AiRequest::Compose { draft } if draft.subject == "Q3 plan"),
        "the draft reaches the engine with its source identity"
    );
    fixture.emit(Event::Ai {
        req,
        delta: "Happy to help with the plan.".into(),
        done: true,
    });
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("compose-ai").label(),
            Some("Happy to help with the plan."),
            "the answer waits in the panel"
        );
        window.click("insert-ai", cx);
        window.render_frame(cx);
        assert!(window.try_find("compose-ai").is_none());
    })
    .unwrap();
    update_content(window, &kite, cx, |kite, _, cx| {
        assert_eq!(kite.compose_body(cx), "Happy to help with the plan.");
    });
}
