//! Snippets: reusable text inserted in compose with `;shortcut`.
use super::{button, card, heading, row};
use crate::app::Kite;
use crate::theme::RADIUS_SM;
use gpui_kit::component::input::{Input, InputState, Textarea, TextareaState};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kite_core::model::Snippet;

const VARIABLES: &str = "Variables: {first_name} {last_name} {name} {email} {my_name} {my_first_name} {date} {day} {time}. A first line \"Subject: …\" fills an empty subject.";

#[derive(Default)]
pub struct State {
    /// The open editor: `index` is `None` for a new snippet, `Some` when editing.
    pub editing: Option<Editing>,
}

pub struct Editing {
    pub index: Option<usize>,
    pub name: Entity<InputState>,
    pub shortcut: Entity<InputState>,
    pub body: Entity<TextareaState>,
}

impl Kite {
    fn edit_snippet(&mut self, index: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let seed = index.and_then(|ix| self.settings.snippets.get(ix).cloned());
        let editing = Editing {
            index,
            name: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("Name")
                    .default_value(seed.as_ref().map(|s| s.name.clone()).unwrap_or_default())
            }),
            shortcut: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("thx")
                    .default_value(
                        seed.as_ref()
                            .map(|s| s.shortcut.clone())
                            .unwrap_or_default(),
                    )
            }),
            body: cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder("Hi {first_name},")
                    .default_value(seed.as_ref().map(|s| s.body.clone()).unwrap_or_default())
            }),
        };
        if let Some(page) = &mut self.preferences {
            page.snippets.editing = Some(editing);
        }
        cx.notify();
    }

    /// Persist the editor through the shared settings save path.
    fn save_snippet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(page) = &self.preferences else {
            return;
        };
        let Some(editing) = &page.snippets.editing else {
            return;
        };
        let name = editing.name.read(cx).value().trim().to_string();
        if name.is_empty() {
            if let Some(page) = &mut self.preferences {
                page.error = Some("A snippet needs a name".into());
                page.notice = None;
            }
            cx.notify();
            return;
        }
        let snippet = Snippet {
            name,
            shortcut: editing
                .shortcut
                .read(cx)
                .value()
                .trim()
                .trim_start_matches(';')
                .to_string(),
            body: editing.body.read(cx).value().to_string(),
        };
        let index = editing.index;
        let mut settings = self.settings.clone();
        match index {
            Some(ix) if ix < settings.snippets.len() => settings.snippets[ix] = snippet,
            _ => settings.snippets.push(snippet),
        }
        self.save_preferences(settings, window, cx);
        if let Some(page) = &mut self.preferences {
            page.snippets.editing = None;
        }
        cx.notify();
    }

    fn delete_snippet(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let mut settings = self.settings.clone();
        if index < settings.snippets.len() {
            settings.snippets.remove(index);
        }
        self.save_preferences(settings, window, cx);
        cx.notify();
    }
}

pub fn render(kite: &Kite, _window: &Window, cx: &mut Context<Kite>) -> AnyElement {
    let t = kite.theme;
    let page = &kite.preferences.as_ref().unwrap().snippets;

    let editor = page.editing.as_ref().map(|editing| {
        div()
            .flex()
            .flex_col()
            .child(heading(
                t,
                if editing.index.is_some() {
                    "Edit snippet"
                } else {
                    "New snippet"
                },
                "Type ; in compose to pick a snippet, or Ctrl/Cmd+; for the whole list.",
            ))
            .child(card(
                t,
                vec![
                    row(
                        t,
                        "Name",
                        "",
                        Input::new(&editing.name)
                            .id("snippet-name")
                            .aria_label("Snippet name")
                            .w(px(280.)),
                    ),
                    row(
                        t,
                        "Shortcut",
                        "What follows ; in the body to find it.",
                        Input::new(&editing.shortcut)
                            .id("snippet-shortcut")
                            .aria_label("Snippet shortcut")
                            .w(px(280.)),
                    ),
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .id("snippet-body")
                                .test_support()
                                .aria_label("Snippet body")
                                .rounded(px(RADIUS_SM))
                                .border_1()
                                .border_color(t.line)
                                .p_2()
                                .child(
                                    Textarea::new(&editing.body)
                                        .aria_label("Snippet body")
                                        .appearance(false)
                                        .bordered(false)
                                        .text_size(t.px(13.))
                                        .h(px(120.)),
                                ),
                        )
                        .child(
                            div()
                                .text_size(t.px(11.))
                                .text_color(t.fg3)
                                .child(VARIABLES),
                        )
                        .into_any_element(),
                    div()
                        .flex()
                        .gap_2()
                        .child(button("snippet-save", "Save snippet", t).on_click(
                            cx.listener(|this, _, window, cx| this.save_snippet(window, cx)),
                        ))
                        .child(button("snippet-cancel", "Cancel", t).on_click(cx.listener(
                            |this, _, _, cx| {
                                if let Some(page) = &mut this.preferences {
                                    page.snippets.editing = None;
                                    page.error = None;
                                }
                                cx.notify();
                            },
                        )))
                        .into_any_element(),
                ],
            ))
            .into_any_element()
    });

    let list = div()
        .flex()
        .flex_col()
        .child(heading(
            t,
            "Snippets",
            "Reusable text inserted in compose. Type ; at the start of a word, or Ctrl/Cmd+; for the whole list.",
        ))
        .child(
            div().pb_3().child(
                button("snippet-new", "New snippet", t).on_click(cx.listener(
                    |this, _, window, cx| this.edit_snippet(None, window, cx),
                )),
            ),
        )
        .child(if kite.settings.snippets.is_empty() {
            card(
                t,
                vec![div()
                    .text_size(t.px(12.))
                    .text_color(t.fg2)
                    .child("No snippets yet. New snippets insert with ;name in compose.")
                    .into_any_element()],
            )
        } else {
            card(
                t,
                kite.settings
                    .snippets
                    .iter()
                    .enumerate()
                    .map(|(ix, snippet)| {
                        let preview =
                            snippet.body.lines().next().unwrap_or("").to_string();
                        row(
                            t,
                            &format!("{}  ·  ;{}", snippet.name, snippet.shortcut),
                            &preview,
                            div()
                                .flex()
                                .gap_2()
                                .child(
                                    button(("snippet-edit", ix), "Edit", t).on_click(
                                        cx.listener(move |this, _, window, cx| {
                                            this.edit_snippet(Some(ix), window, cx)
                                        }),
                                    ),
                                )
                                .child(
                                    button(("snippet-delete", ix), "Delete", t).on_click(
                                        cx.listener(move |this, _, window, cx| {
                                            this.delete_snippet(ix, window, cx)
                                        }),
                                    ),
                                ),
                        )
                    })
                    .collect(),
            )
        })
        .into_any_element();

    div()
        .w_full()
        .max_w(px(720.))
        .pb_2()
        .flex()
        .flex_col()
        .gap_8()
        .when_some(editor, |d, editor| d.child(editor))
        .when(page.editing.is_none(), |d| d.child(list))
        .into_any_element()
}
