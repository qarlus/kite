//! AI: optional writing and summary helpers, off by default. Only free models whose
//! provider keeps no data are offered; the list comes from the endpoint, off the UI thread.
use super::{button, card, heading, row};
use crate::app::Kite;
use crate::theme::Theme;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::switch::Switch;
use gpui_kit::prelude::*;
use gpui_kit::*;
use kite_core::model::{AiModel, Settings};

const OPENCODE_URL: &str = "https://opencode.ai/zen/go/v1";
const OPENROUTER_URL: &str = "https://openrouter.ai/api/v1";

/// The AI endpoint presets. `Custom` keeps today's free-form base URL field.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Opencode,
    Openrouter,
    Custom,
}

impl Provider {
    const ALL: [Provider; 3] = [Provider::Opencode, Provider::Openrouter, Provider::Custom];

    fn url(self) -> Option<&'static str> {
        match self {
            Provider::Opencode => Some(OPENCODE_URL),
            Provider::Openrouter => Some(OPENROUTER_URL),
            Provider::Custom => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Provider::Opencode => "OpenCode Go",
            Provider::Openrouter => "OpenRouter",
            Provider::Custom => "Custom",
        }
    }

    fn id(self) -> &'static str {
        match self {
            Provider::Opencode => "ai-provider-opencode",
            Provider::Openrouter => "ai-provider-openrouter",
            Provider::Custom => "ai-provider-custom",
        }
    }

    fn for_url(url: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|provider| provider.url() == Some(url))
            .unwrap_or(Provider::Custom)
    }
}

/// The endpoint's model list, cached on the page so it is fetched once, not per frame.
pub enum Models {
    NotAsked,
    Loading,
    Ready(Vec<AiModel>),
    Failed(String),
}

pub struct State {
    pub base_url: Entity<InputState>,
    pub model: Entity<InputState>,
    pub key: Entity<InputState>,
    pub provider: Provider,
    pub models: Models,
    /// Bumped per fetch so a slow answer cannot overwrite a newer one.
    pub models_request: u64,
    /// Set once the first fetch is kicked off from render, which cannot mutate the page.
    pub asked: std::cell::Cell<bool>,
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
            provider: Provider::for_url(&settings.ai_base_url),
            models: Models::NotAsked,
            models_request: 0,
            asked: std::cell::Cell::new(false),
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
        self.fetch_ai_models(cx);
    }

    /// Picks a provider preset. A preset writes its base URL and refetches the model
    /// list; Custom only reveals the free-form endpoint field.
    fn choose_ai_provider(
        &mut self,
        provider: Provider,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(page) = &mut self.preferences {
            page.ai.provider = provider;
            if let Some(url) = provider.url() {
                page.ai
                    .base_url
                    .update(cx, |input, cx| input.set_value(url, window, cx));
            }
        }
        if let Some(url) = provider.url() {
            let mut settings = self.settings.clone();
            settings.ai_base_url = url.to_string();
            self.save_preferences(settings, window, cx);
            self.fetch_ai_models(cx);
        } else {
            cx.notify();
        }
    }

    /// Selects the model the helpers use. One model is always selected while AI is on,
    /// so this only ever turns a switch on; off-requests are dropped by the caller.
    fn choose_ai_model(&mut self, model: String, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(page) = &mut self.preferences {
            page.ai
                .model
                .update(cx, |input, cx| input.set_value(&model, window, cx));
        }
        let mut settings = self.settings.clone();
        settings.ai_model = model;
        self.save_preferences(settings, window, cx);
    }

    /// Asks the configured endpoint for its free zero-retention models. `ai_models`
    /// blocks on the network, so it rides the background executor; stale answers are
    /// dropped by matching `models_request`.
    pub(crate) fn fetch_ai_models(&mut self, cx: &mut Context<Self>) {
        if !self.settings.ai_enabled {
            return;
        }
        let Some(page) = &mut self.preferences else {
            return;
        };
        page.ai.models_request += 1;
        let request = page.ai.models_request;
        page.ai.models = Models::Loading;
        let mail = self.mail.clone();
        let task = cx
            .background_executor()
            .spawn(async move { mail.ai_models() });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if let Some(page) = &mut this.preferences
                    && page.ai.models_request == request
                {
                    page.ai.models = match result {
                        Ok(models) => Models::Ready(models),
                        Err(error) => Models::Failed(format!("{error:#}")),
                    };
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
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

/// A quiet policy tag on a model row.
fn badge(t: Theme, label: &str) -> Div {
    div()
        .px_2()
        .py(px(1.))
        .rounded_full()
        .bg(t.bg3)
        .text_size(t.px(11.))
        .text_color(t.fg2)
        .child(label.to_string())
}

/// One offered model: name, id, policy badges, and the single-choice switch.
/// The switch is controlled by `settings.ai_model`; asking to turn the active one off
/// is ignored, so exactly one model stays selected.
fn model_row(
    t: Theme,
    ix: usize,
    model: &AiModel,
    active: bool,
    weak: &WeakEntity<Kite>,
) -> AnyElement {
    let id = model.id.clone();
    let name = model.name.clone();
    let weak = weak.clone();
    div()
        .id(("ai-model", ix))
        .test_support()
        .aria_label(name.clone())
        .flex()
        .items_center()
        .justify_between()
        .gap_4()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(div().text_size(t.px(13.)).child(name.clone()))
                        .when(model.free, |d| d.child(badge(t, "Free")))
                        .when(model.zdr, |d| d.child(badge(t, "Zero retention"))),
                )
                .child(
                    div()
                        .text_size(t.px(11.))
                        .text_color(t.fg3)
                        .child(id.clone()),
                ),
        )
        .child(
            Switch::new(("ai-model-switch", ix))
                .checked(active)
                .accessibility_label(format!("Use {name}"))
                .on_click(move |checked, window, cx| {
                    let checked = *checked;
                    let id = id.clone();
                    weak.update(cx, |kite, cx| {
                        if checked {
                            kite.choose_ai_model(id, window, cx);
                        }
                    })
                    .ok();
                }),
        )
        .into_any_element()
}

/// The free-form model id field, used for Custom endpoints and empty model lists.
fn model_input(t: Theme, page: &super::Preferences) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap_3()
        .child(field(
            t,
            "Model",
            Input::new(&page.ai.model)
                .id("ai-model")
                .aria_label("AI model"),
        ))
        .child(
            div()
                .text_size(t.px(12.))
                .text_color(t.fg3)
                .child("Kite can't verify retention for custom endpoints."),
        )
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
                if checked {
                    kite.fetch_ai_models(cx);
                }
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
        // The first fetch is kicked from here because page state exists before the AI
        // page is ever shown; the spawn defers the entity update past this render.
        if !page.ai.asked.get() {
            page.ai.asked.set(true);
            cx.spawn(async move |this, cx| {
                this.update(cx, |kite, cx| kite.fetch_ai_models(cx)).ok();
            })
            .detach();
        }

        let provider = page.ai.provider;
        let chips = div()
            .flex()
            .gap_2()
            .children(Provider::ALL.into_iter().map(|choice| {
                let selected = provider == choice;
                button(choice.id(), choice.label(), t)
                    .aria_selected(selected)
                    .when(selected, |d| d.bg(t.sel).border_color(t.accent))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.choose_ai_provider(choice, window, cx)
                    }))
            }));

        let mut content = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(row(t, "Provider", "Free, zero-retention endpoints.", chips))
            .into_any_element();

        let mut details = div().flex().flex_col().gap_3();
        if provider == Provider::Custom {
            details = details.child(field(
                t,
                "Endpoint",
                Input::new(&page.ai.base_url)
                    .id("ai-base-url")
                    .aria_label("AI base URL"),
            ));
        }
        details = details.child(field(
            t,
            "API key",
            Input::new(&page.ai.key)
                .id("ai-key")
                .aria_label("AI API key"),
        ));

        if provider == Provider::Custom {
            details = details.child(model_input(t, page));
        } else {
            match &page.ai.models {
                Models::NotAsked | Models::Loading => {
                    details = details.child(
                        div()
                            .id("ai-models-loading")
                            .test_support()
                            .aria_label("Loading models…")
                            .text_size(t.px(12.))
                            .text_color(t.fg3)
                            .child("Loading models…"),
                    );
                }
                Models::Failed(reason) => {
                    details =
                        details.child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap_4()
                                .child(
                                    div()
                                        .id("ai-models-error")
                                        .test_support()
                                        .aria_label(format!("Couldn't load models: {reason}"))
                                        .flex_1()
                                        .min_w_0()
                                        .text_size(t.px(12.))
                                        .text_color(t.danger)
                                        .child(format!("Couldn't load models: {reason}")),
                                )
                                .child(button("ai-models-retry", "Retry", t).on_click(
                                    cx.listener(|this, _, _, cx| this.fetch_ai_models(cx)),
                                )),
                        );
                }
                Models::Ready(models) if models.is_empty() => {
                    details = details.child(model_input(t, page));
                }
                Models::Ready(models) => {
                    if !models
                        .iter()
                        .any(|model| model.id == kite.settings.ai_model)
                    {
                        let first = models[0].clone();
                        let weak = cx.weak_entity();
                        details = details.child(
                            div()
                                .id("ai-model-notice")
                                .test_support()
                                .aria_label(format!(
                                    "Your current model ({}) isn't a free zero-retention model",
                                    kite.settings.ai_model
                                ))
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap_4()
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .text_size(t.px(12.))
                                        .text_color(t.fg2)
                                        .child(format!(
                                            "Your current model ({}) isn't a free zero-retention model",
                                            kite.settings.ai_model
                                        )),
                                )
                                .child(
                                    button(
                                        "ai-model-notice-use",
                                        &format!("Use {}", first.name),
                                        t,
                                    )
                                    .on_click(move |_, window, cx| {
                                        let id = first.id.clone();
                                        weak.update(cx, |kite, cx| {
                                            kite.choose_ai_model(id, window, cx)
                                        })
                                        .ok();
                                    }),
                                ),
                        );
                    }
                    for (ix, model) in models.iter().enumerate() {
                        let active = model.id == kite.settings.ai_model;
                        details = details.child(model_row(t, ix, model, active, &cx.weak_entity()));
                    }
                }
            }
        }

        content = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(content)
            .child(details)
            .child(
                div()
                    .text_size(t.px(12.))
                    .text_color(t.fg3)
                    .child("Stored in your operating system keychain, never in Kite settings."),
            )
            .child(
                div()
                    .id("ai-privacy-note")
                    .test_support()
                    .aria_label(
                        "Kite only offers free models whose provider keeps no data and doesn't train on it.",
                    )
                    .text_size(t.px(12.))
                    .text_color(t.fg3)
                    .child(
                        "Kite only offers free models whose provider keeps no data and doesn't train on it.",
                    ),
            )
            .child(
                button("save-ai", "Save AI settings", t)
                    .on_click(cx.listener(|this, _, window, cx| this.save_ai(window, cx))),
            )
            .into_any_element();
        rows.push(content);
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
