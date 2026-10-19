//! Shortcuts: every command with the keys bound right now, and a way to change them.
use super::{button, card, heading};
use crate::app::Kite;
use crate::keys::{self, Cmd, Group};
use crate::list::keys_pills;
use crate::theme::{RADIUS_SM, Theme};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::prelude::*;
use gpui_kit::*;

#[derive(Default)]
pub struct State {
    /// The command whose new keystroke is being recorded.
    pub recording: Option<&'static str>,
    /// Why the last recording was refused, naming the command it clashed with.
    pub conflict: Option<String>,
}

pub fn render(kite: &Kite, _window: &Window, cx: &mut Context<Kite>) -> AnyElement {
    let t = kite.theme;
    let mut blocks = vec![
        div()
            .flex()
            .items_start()
            .justify_between()
            .gap_4()
            .child(heading(
                t,
                "Keys",
                "Press Change, then the keys you want. Escape cancels.",
            ))
            .child(
                button("shortcut-reset-all", "Reset all", t).on_click(cx.listener(
                    |this, _, window, cx| {
                        let mut settings = this.settings.clone();
                        settings.shortcuts.clear();
                        if let Some(page) = &mut this.preferences {
                            page.shortcuts.recording = None;
                            page.shortcuts.conflict = None;
                        }
                        this.save_preferences(settings, window, cx);
                        this.rebind(cx);
                    },
                )),
            )
            .into_any_element(),
    ];
    if let Some(conflict) = kite
        .preferences
        .as_ref()
        .and_then(|page| page.shortcuts.conflict.as_ref())
    {
        blocks.push(
            div()
                .id("shortcut-conflict")
                .test_support()
                .aria_label(conflict.clone())
                .rounded(px(RADIUS_SM))
                .border_1()
                .border_color(t.danger)
                .px_3()
                .py_2()
                .text_size(t.px(12.))
                .text_color(t.danger)
                .child(conflict.clone())
                .into_any_element(),
        );
    }
    for group in Group::ALL {
        let rows: Vec<AnyElement> = keys::COMMANDS
            .iter()
            .filter(|command| command.group == group)
            .map(|command| row(t, kite, command, cx))
            .collect();
        if !rows.is_empty() {
            blocks.push(heading(t, group.label(), "").into_any_element());
            blocks.push(card(t, rows).into_any_element());
        }
    }
    div()
        .w_full()
        .max_w(px(720.))
        .pb_2()
        .flex()
        .flex_col()
        .gap_4()
        .children(blocks)
        .into_any_element()
}

/// One command: its current keys, a Change action and a Reset action.
fn row(t: Theme, kite: &Kite, command: &'static Cmd, cx: &mut Context<Kite>) -> AnyElement {
    let keys = keys::bound(&kite.settings, command);
    let text = keys.join(" ");
    let recording = kite
        .preferences
        .as_ref()
        .is_some_and(|page| page.shortcuts.recording == Some(command.slug));
    let shown = if recording {
        "Press a key".to_string()
    } else if !text.is_empty() {
        text.clone()
    } else if kite.settings.shortcuts.contains_key(command.slug) {
        "Off".to_string()
    } else {
        "Not bound in this mode".to_string()
    };
    div()
        .id(format!("shortcut-row-{}", command.slug))
        .test_support()
        .aria_label(format!("{}: {shown}", command.title))
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
                .child(div().text_size(t.px(13.)).child(command.title))
                .child(
                    div()
                        .text_size(t.px(11.))
                        .text_color(if recording { t.accent } else { t.fg2 })
                        .child(shown),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .flex_shrink_0()
                .when(!recording && !text.is_empty(), |d| {
                    d.child(keys_pills(&t, &text))
                })
                .child(
                    button(
                        format!("shortcut-change-{}", command.slug),
                        if recording { "Cancel" } else { "Change" },
                        t,
                    )
                    .aria_selected(recording)
                    .when(recording, |d| d.bg(t.sel).border_color(t.accent))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let slug = command.slug;
                        if let Some(page) = &mut this.preferences {
                            page.shortcuts.conflict = None;
                            page.shortcuts.recording = if page.shortcuts.recording == Some(slug) {
                                None
                            } else {
                                Some(slug)
                            };
                        }
                        cx.notify();
                    })),
                )
                .child(
                    button(format!("shortcut-reset-{}", command.slug), "Reset", t).on_click(
                        cx.listener(move |this, _, window, cx| {
                            let mut settings = this.settings.clone();
                            settings.shortcuts.remove(command.slug);
                            if let Some(page) = &mut this.preferences {
                                page.shortcuts.recording = None;
                                page.shortcuts.conflict = None;
                            }
                            this.save_preferences(settings, window, cx);
                            this.rebind(cx);
                        }),
                    ),
                ),
        )
        .into_any_element()
}

/// Whether the shortcuts page is waiting for a keystroke to record.
pub fn recording(kite: &Kite) -> bool {
    kite.preferences
        .as_ref()
        .is_some_and(|page| page.tab == super::OPEN_SHORTCUTS && page.shortcuts.recording.is_some())
}

/// Records the next keystroke for the command being edited. Returns true when it used the key.
pub fn capture(
    kite: &mut Kite,
    event: &KeyDownEvent,
    window: &Window,
    cx: &mut Context<Kite>,
) -> bool {
    let Some(slug) = kite.preferences.as_mut().and_then(|page| {
        // Leaving the page mid-recording forgets the pending key instead of swallowing the
        // next keystroke somewhere else.
        if page.tab == super::OPEN_SHORTCUTS {
            page.shortcuts.recording
        } else {
            page.shortcuts.recording = None;
            None
        }
    }) else {
        return false;
    };
    let stroke = &event.keystroke;
    if stroke.key == "escape" && !stroke.modifiers.modified() {
        if let Some(page) = &mut kite.preferences {
            page.shortcuts.recording = None;
        }
        cx.notify();
        return true;
    }
    // A modifier on its own is the start of the chord the user is still building.
    if matches!(
        stroke.key.as_str(),
        "shift" | "control" | "ctrl" | "alt" | "platform" | "cmd" | "win" | "function" | "fn"
    ) {
        return true;
    }
    let key = keys::source(stroke);
    if let Some(other) = keys::conflict(&kite.settings, slug, &key) {
        if let Some(page) = &mut kite.preferences {
            page.shortcuts.conflict = Some(format!(
                "{} already runs {other}. Press another key, or Escape.",
                Kbd::format(stroke)
            ));
        }
        cx.notify();
        return true;
    }
    let mut settings = kite.settings.clone();
    settings.shortcuts.insert(slug.to_string(), vec![key]);
    if let Some(page) = &mut kite.preferences {
        page.shortcuts.recording = None;
        page.shortcuts.conflict = None;
    }
    kite.save_preferences(settings, window, cx);
    kite.rebind(cx);
    true
}
