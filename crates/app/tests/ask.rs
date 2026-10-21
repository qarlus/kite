//! Ask your inbox: the question panel, its streamed cited answer and its error state.
mod common;
use common::{Fixture, mount};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, TestAppContext};
use kite_core::model::AiRequest;

/// A fixture with AI turned on and a scripted cited answer.
fn ai_on() -> std::sync::Arc<Fixture> {
    let fixture = Fixture::new(3);
    let mut settings = fixture.settings.lock().unwrap();
    settings.ai_enabled = true;
    settings.ai_base_url = "https://ai.example.com/v1".into();
    settings.ai_model = "kite-test".into();
    drop(settings);
    *fixture.ai_reply.lock().unwrap() = "The budget moved to Q3 [1].".into();
    *fixture.ai_sources.lock().unwrap() = vec![2];
    fixture
}

fn ask(window: gpui_kit::WindowHandle<gpui_kit::base::Root>, cx: &mut TestAppContext) {
    cx.simulate_keystrokes(window.into(), "secondary-shift-k");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.input("what changed?", cx);
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "enter");
}

#[gpui_kit::test]
fn the_panel_sends_the_question_scoped_to_the_view_account(cx: &mut TestAppContext) {
    let fixture = ai_on();
    let (window, _) = mount(cx, fixture.clone());
    // Pick account 1 first: the ask is scoped to the view's account.
    cx.simulate_keystrokes(window.into(), "secondary-1");
    cx.simulate_keystrokes(window.into(), "secondary-shift-k");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("ask-panel").label(),
            Some("Ask your inbox"),
            "Ctrl/Cmd+Shift+K opens the panel"
        );
    })
    .unwrap();
    ask(window, cx);
    let requests = fixture.ai_requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 1, "enter asks the engine once");
    assert!(
        matches!(
            &requests[0].1,
            AiRequest::Ask {
                question,
                account: Some(1)
            } if question == "what changed?"
        ),
        "the question goes out with the view's account, not a prompt: {:?}",
        requests[0].1
    );
}

#[gpui_kit::test]
fn the_combined_inbox_asks_without_an_account(cx: &mut TestAppContext) {
    let fixture = ai_on();
    let (window, _) = mount(cx, fixture.clone());
    ask(window, cx);
    let requests = fixture.ai_requests.lock().unwrap().clone();
    assert!(
        matches!(&requests[0].1, AiRequest::Ask { account: None, .. }),
        "the combined view asks across accounts: {:?}",
        requests[0].1
    );
}

#[gpui_kit::test]
fn the_streamed_answer_cites_and_its_sources_open_the_conversation(cx: &mut TestAppContext) {
    let fixture = ai_on();
    let (window, _) = mount(cx, fixture.clone());
    ask(window, cx);
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("ask-answer").label(),
            Some("The budget moved to Q3 [1]."),
            "the streamed answer shows as it arrives"
        );
        // The [1] citation is a chip, and the source row names the conversation.
        assert_eq!(window.find(("cite", 1usize)).label(), Some("Source 1"));
        assert_eq!(
            window.find(("ask-source", 0usize)).label(),
            Some("Sender 2: Thread 2")
        );
        window.click(("cite", 1usize), cx);
        window.render_frame(cx);
        assert!(window.try_find("ask-panel").is_none());
        assert_eq!(
            window.find("thread-subject").label(),
            Some("Thread 2"),
            "the citation opens the conversation it points at"
        );
    })
    .unwrap();
    // A follow-up asks again; the source row opens the same conversation.
    ask(window, cx);
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(("ask-source", 0usize), cx);
        window.render_frame(cx);
        assert_eq!(window.find("thread-subject").label(), Some("Thread 2"));
    })
    .unwrap();
    assert_eq!(fixture.ai_requests.lock().unwrap().len(), 2);
}

#[gpui_kit::test]
fn an_engine_error_stays_in_the_panel_and_retry_asks_again(cx: &mut TestAppContext) {
    let fixture = ai_on();
    *fixture.ai_reply.lock().unwrap() = String::new();
    *fixture.ai_error.lock().unwrap() = Some("endpoint refused".into());
    let (window, _) = mount(cx, fixture.clone());
    ask(window, cx);
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("ask-error").label(),
            Some("AI: endpoint refused"),
            "the failure shows in the panel, not only a toast"
        );
        // Repair the endpoint and retry: the same question goes out again.
        *fixture.ai_error.lock().unwrap() = None;
        *fixture.ai_reply.lock().unwrap() = "All clear now.".into();
        window.click("ask-retry", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("ask-answer").label(), Some("All clear now."));
        assert!(window.try_find("ask-error").is_none());
    })
    .unwrap();
    assert_eq!(
        fixture.ai_requests.lock().unwrap().len(),
        2,
        "retry resends the question"
    );
}

#[gpui_kit::test]
fn with_ai_off_there_is_no_ask_command_and_no_suggest_button(cx: &mut TestAppContext) {
    let fixture = Fixture::new(3);
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "secondary-shift-k");
    cx.simulate_keystrokes(window.into(), "enter");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("ask-panel").is_none(),
            "a helper that cannot run is not offered"
        );
        assert!(
            window.try_find("suggest-replies").is_none(),
            "nor are reply suggestions"
        );
    })
    .unwrap();
    // And the palette does not list the command, as with the other AI helpers.
    cx.simulate_keystrokes(window.into(), "escape secondary-k a s k");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find(("palette-row", 0usize)).is_none());
    })
    .unwrap();
}
