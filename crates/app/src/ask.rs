//! Ask your inbox: a palette-style overlay that questions the local mail and streams
//! an answer back, citing the conversations it read. It is an overlay, not a side
//! pane, because it is a focused task — type, read, act, Escape — like the palette
//! itself, while the calendar pane is ambient content meant to sit beside the mail.
use crate::app::{AiFor, Kite, Overlay};
use crate::keys::cmd;
use crate::list::{keys_pills, overlay};
use crate::theme::{self, RADIUS, RADIUS_SM};
use gpui_kit::assets::IconName;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{Icon, Sizable};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kite_core::model::{AiRequest, Id};

/// The open Ask panel. The streamed text itself lives on `Kite::ai`, shared with the
/// other AI helpers; what is left here is what only a question needs.
pub struct Ask {
    pub input: Entity<InputState>,
    /// The conversations the answer cites, in `[n]` order, from `Event::AiSources`.
    pub sources: Vec<Id>,
    /// The question in flight, so Retry can resend it.
    pub question: String,
    /// A failure shown in the panel itself, with Retry.
    pub error: Option<String>,
}

impl Kite {
    /// Ctrl/Cmd+Shift+K or the palette: opens the question panel. AI-only, like the
    /// other helpers — the command is filtered out everywhere while AI is off.
    pub(crate) fn show_ask(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.settings.ai_enabled {
            self.toast("AI is off. Turn it on in Settings › AI.", cx);
            return;
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Ask your inbox…"));
        let focus = input.read(cx).focus_handle(cx);
        self.overlay = Overlay::Ask(Ask {
            input,
            sources: Vec::new(),
            question: String::new(),
            error: None,
        });
        window.focus(&focus, cx);
        cx.notify();
    }

    /// Enter in the question field: a follow-up replaces the previous answer. The
    /// account is the view's, or none in the combined inbox.
    pub(crate) fn ask_submit(&mut self, cx: &mut Context<Self>) {
        let Overlay::Ask(ask) = &mut self.overlay else {
            return;
        };
        let question = ask.input.read(cx).value().trim().to_string();
        if question.is_empty() {
            return;
        }
        ask.question = question.clone();
        ask.sources.clear();
        ask.error = None;
        let request = AiRequest::Ask {
            question,
            account: self.view.account_id,
        };
        self.ai = None;
        self.ai_request(cmd::ASK, AiFor::Ask, request, cx);
    }

    /// Resends the failed question without leaving the panel.
    pub(crate) fn ask_retry(&mut self, cx: &mut Context<Self>) {
        let Overlay::Ask(ask) = &mut self.overlay else {
            return;
        };
        ask.error = None;
        let request = AiRequest::Ask {
            question: ask.question.clone(),
            account: self.view.account_id,
        };
        self.ai = None;
        self.ai_request(cmd::ASK, AiFor::Ask, request, cx);
    }

    /// A citation chip or a source row: opens the cited conversation and leaves the panel.
    pub(crate) fn ask_open_source(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Overlay::Ask(ask) = &self.overlay else {
            return;
        };
        let Some(id) = ask.sources.get(ix).copied() else {
            return;
        };
        self.overlay = Overlay::None;
        self.refocus = true;
        self.open_any_thread(id, cx);
    }
}

/// The answer text with `[n]` citations as small accent chips that open the source.
fn cited(kite: &Kite, text: &str, cx: &mut Context<Kite>) -> AnyElement {
    let t = kite.theme;
    let mut parts = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_1()
        .text_size(t.px(14.))
        .line_height(relative(1.6));
    let mut rest = text;
    while let Some(start) = rest.find('[') {
        let valid = rest[start..]
            .find(']')
            .filter(|end| {
                rest[start + 1..start + *end]
                    .chars()
                    .all(|c| c.is_ascii_digit())
            })
            .map(|end| start + end);
        let Some(end) = valid else { break };
        if start > 0 {
            parts = parts.child(div().child(rest[..start].to_string()));
        }
        let n: usize = rest[start + 1..end].parse().unwrap_or(0);
        if n > 0 && n <= ask_sources(kite).len() {
            parts = parts.child(
                div()
                    .id(("cite", n))
                    .test_support()
                    .aria_label(format!("Source {n}"))
                    .px_2()
                    .rounded(px(RADIUS_SM))
                    .bg(t.sel)
                    .text_size(t.px(11.))
                    .text_color(t.accent)
                    .cursor_pointer()
                    .hover(|s| s.bg(t.bg3))
                    .child(format!("{n}"))
                    .on_click(cx.listener(move |this, _, _, cx| this.ask_open_source(n - 1, cx))),
            );
        } else {
            parts = parts.child(div().child(rest[start..=end].to_string()));
        }
        rest = &rest[end + 1..];
    }
    if !rest.is_empty() {
        parts = parts.child(div().child(rest.to_string()));
    }
    parts.into_any_element()
}

fn ask_sources(kite: &Kite) -> &[Id] {
    match &kite.overlay {
        Overlay::Ask(ask) => &ask.sources,
        _ => &[],
    }
}

/// One source row: sender, subject and date, clickable through to the conversation.
fn source_row(
    kite: &Kite,
    ix: usize,
    summary: &kite_core::model::ThreadSummary,
    cx: &mut Context<Kite>,
) -> AnyElement {
    let t = kite.theme;
    let date = crate::list::when(summary.date, chrono::Local::now());
    div()
        .id(("ask-source", ix))
        .test_support()
        .aria_label(format!("{}: {}", summary.participants, summary.subject))
        .flex()
        .items_center()
        .gap_2()
        .min_w_0()
        .h(px(32.))
        .px_2()
        .rounded(px(RADIUS_SM))
        .cursor_pointer()
        .hover(|s| s.bg(t.bg3))
        .child(
            div()
                .text_size(t.px(11.))
                .text_color(t.accent)
                .child(format!("[{}]", ix + 1)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(t.px(13.))
                .child(format!("{} — {}", summary.participants, summary.subject)),
        )
        .child(
            div()
                .flex_shrink_0()
                .text_size(t.px(12.))
                .text_color(t.fg3)
                .child(date),
        )
        .on_click(cx.listener(move |this, _, _, cx| this.ask_open_source(ix, cx)))
        .into_any_element()
}

pub fn render(kite: &Kite, window: &Window, cx: &mut Context<Kite>) -> AnyElement {
    let t = kite.theme;
    let Overlay::Ask(ask) = &kite.overlay else {
        return div().into_any_element();
    };
    let answer = match &kite.ai {
        Some(ai) if matches!(ai.target, AiFor::Ask) => Some((ai.text.clone(), ai.done)),
        _ => None,
    };
    let mut panel = div()
        .id("ask-panel")
        .test_support()
        .aria_label("Ask your inbox")
        .w(px(600.))
        .max_h(window.viewport_size().height * 0.7)
        .overflow_y_scroll()
        .rounded(px(RADIUS))
        .bg(t.bg2)
        .shadow(vec![
            BoxShadow::new(px(0.), px(20.), theme::SHADOW.into()).blur_radius(px(60.)),
        ])
        .p_2()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .flex()
                .items_center()
                .h(px(48.))
                .px_3()
                .gap_3()
                .child(
                    Icon::new(IconName::Sparkles)
                        .with_size(px(18.))
                        .text_color(t.accent),
                )
                .child(
                    div().flex_1().min_w_0().child(
                        Input::new(&ask.input)
                            .id("ask-input")
                            .aria_label("Question")
                            .appearance(false)
                            .bordered(false)
                            .w_full()
                            .text_size(t.px(14.)),
                    ),
                ),
        );

    if let Some(error) = &ask.error {
        panel = panel.child(
            div()
                .id("ask-error")
                .test_support()
                .aria_label(error.clone())
                .mx_3()
                .my_2()
                .flex()
                .items_center()
                .justify_between()
                .gap_4()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(t.px(13.))
                        .text_color(t.danger)
                        .child(error.clone()),
                )
                .child(
                    crate::preferences::button("ask-retry", "Retry", t)
                        .on_click(cx.listener(|this, _, _, cx| this.ask_retry(cx))),
                ),
        );
    }

    if let Some((text, done)) = answer {
        let mut card = div()
            .id("ask-answer")
            .test_support()
            .aria_label(text.clone())
            .mx_3()
            .my_2()
            .flex()
            .flex_col()
            .gap_3();
        if text.is_empty() && !done {
            card = card.child(
                div()
                    .id("ask-thinking")
                    .test_support()
                    .aria_label("Thinking…")
                    .text_size(t.px(13.))
                    .text_color(t.fg2)
                    .child("Thinking…"),
            );
        } else {
            card = card.child(cited(kite, &text, cx));
        }
        if !ask.sources.is_empty() {
            let mut sources = div().flex().flex_col().child(
                div()
                    .px_2()
                    .pb_1()
                    .text_size(t.px(11.))
                    .font_weight(FontWeight(650.))
                    .text_color(t.fg3)
                    .child("SOURCES"),
            );
            for (ix, id) in ask.sources.iter().enumerate() {
                if let Some(summary) = kite.summary_of(*id) {
                    sources = sources.child(source_row(kite, ix, &summary, cx));
                }
            }
            card = card.child(sources);
        }
        panel = panel.child(card);
    }

    overlay(
        &t,
        window.viewport_size().height * 0.15,
        false,
        panel.child(
            div()
                .mt_2()
                .px_3()
                .py_2()
                .border_t_1()
                .border_color(t.line)
                .flex()
                .items_center()
                .justify_between()
                .gap_4()
                .text_size(t.px(12.))
                .text_color(t.fg3)
                .child(
                    div().flex_1().min_w_0().child(format!(
                        "Searches your mail on this computer; only the most relevant non-Private conversations are sent to {}.",
                        kite.settings.ai_model
                    )),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .flex_shrink_0()
                        .child(keys_pills(&t, "enter"))
                        .child("Ask"),
                ),
        ),
    )
    .into_any_element()
}
