//! AI: optional writing and summary helpers, off by default.
use super::{button, card, heading, row};
use crate::app::Kite;
use crate::theme::Theme;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::switch::Switch;
use gpui_kit::prelude::*;
use gpui_kit::*;
use kite_core::model::Settings;

pub struct State {
    pub base_url: Entity<InputState>,
    pub model: Entity<InputState>,
    pub key: Entity<InputState>,
}

impl State {
    pub fn new(settings: &Settings, window: &mut Window, cx: &mut Context<Kite>) -> Self {
        let base_url = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("https://…/v1")
                .default_value(settings.ai_base_url.clone())
        });
        let model = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Model name")
                .default_value(settings.ai_model.clone())
        });
        let key = cx.new(|cx| InputState::new(window, cx).placeholder("Paste your API key"));
        key.update(cx, |input, cx| input.set_masked(true, window, cx));
        Self {
            base_url,
            model,
            key,
        }
    }
}

impl Kite {
    /// Persist the AI endpoint and model, then hand the key to the OS keychain.
    /// The key never enters settings: it is cleared from the form and written off-thread.
    fn save_ai(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(page) = &self.preferences else {
            return;
        };
        let mut settings = self.settings.clone();
        settings.ai_base_url = page.ai.base_url.read(cx).value().trim().to_string();
        settings.ai_model = page.ai.model.read(cx).value().trim().to_string();
        let key = page.ai.key.read(cx).value().trim().to_string();
        if !key.is_empty() {
            page.ai
                .key
                .update(cx, |input, cx| input.set_value("", window, cx));
            let mail = self.mail.clone();
            let task = cx
                .background_executor()
                .spawn(async move { mail.set_ai_key(&key) });
            cx.spawn(async move |this, cx| {
                if let Err(error) = task.await {
                    this.update(cx, |this, cx| {
                        if let Some(page) = &mut this.preferences {
                            page.error = Some(format!("Could not save the AI key: {error}"));
                        }
                        cx.notify();
                    })
                    .ok();
                }
            })
            .detach();
        }
        self.save_preferences(settings, window, cx);
    }
}

/// A labelled input stacked above its control.
fn field(t: Theme, label: &str, input: impl IntoElement) -> AnyElement {
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .text_size(t.px(12.))
                .text_color(t.fg2)
                .child(label.to_string()),
        )
        .child(input)
        .into_any_element()
}

pub fn render(kite: &Kite, _window: &Window, cx: &mut Context<Kite>) -> AnyElement {
    let Some(page) = &kite.preferences else {
        return div().into_any_element();
    };
    let t = kite.theme;
    let weak = cx.weak_entity();
    let enabled = kite.settings.ai_enabled;
    let ai_switch = Switch::new("ai-enabled")
        .checked(enabled)
        .accessibility_label("Enable AI assistance")
        .on_click(move |checked, window, cx| {
            let checked = *checked;
            weak.update(cx, |kite, cx| {
                let mut settings = kite.settings.clone();
                settings.ai_enabled = checked;
                kite.save_preferences(settings, window, cx);
            })
            .ok();
        });

    let mut rows: Vec<AnyElement> = vec![row(
        t,
        "Write and summarize",
        "Draft replies and summarize threads with any OpenAI-compatible endpoint.",
        ai_switch,
    )];
    if enabled {
        rows.push(
            div()
                .flex()
                .flex_col()
                .gap_3()
                .child(
                    div()
                        .flex()
                        .gap_4()
                        .child(field(
                            t,
                            "Endpoint",
                            Input::new(&page.ai.base_url)
                                .id("ai-base-url")
                                .aria_label("AI base URL"),
                        ))
                        .child(field(
                            t,
                            "Model",
                            Input::new(&page.ai.model)
                                .id("ai-model")
                                .aria_label("AI model"),
                        )),
                )
                .child(field(
                    t,
                    "API key",
                    Input::new(&page.ai.key)
                        .id("ai-key")
                        .aria_label("AI API key"),
                ))
                .child(
                    div()
                        .text_size(t.px(12.))
                        .text_color(t.fg3)
                        .child("Stored in your operating system keychain, never in Kite settings."),
                )
                .child(
                    button("save-ai", "Save AI settings", t)
                        .on_click(cx.listener(|this, _, window, cx| this.save_ai(window, cx))),
                )
                .into_any_element(),
        );
    }

    div()
        .w_full()
        .max_w(px(720.))
        .pb_2()
        .flex()
        .flex_col()
        .gap_8()
        .child(
            div()
                .flex()
                .flex_col()
                .child(heading(
                    t,
                    "AI assistance",
                    "Optional helpers, off by default.",
                ))
                .child(card(t, rows)),
        )
        .into_any_element()
}
