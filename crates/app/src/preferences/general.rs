//! General: the everyday defaults.
use super::{button, card, heading, row};
use crate::app::Kite;
use gpui_kit::prelude::*;
use gpui_kit::*;
use kite_core::model::Settings;

pub struct State;

impl State {
    pub fn new(_settings: &Settings, _window: &mut Window, _cx: &mut Context<Kite>) -> Self {
        Self
    }
}

pub fn render(kite: &Kite, _window: &Window, cx: &mut Context<Kite>) -> AnyElement {
    let t = kite.theme;
    let undo = div().flex().flex_wrap().gap_2().children(
        [0, 5, 10, 20, 30].into_iter().map(|seconds| {
            let selected = kite.settings.undo_send_secs == seconds;
            button(
                format!("undo-send-{seconds}"),
                &format!("{seconds} seconds"),
                t,
            )
            .aria_selected(selected)
            .when(selected, |d| d.bg(t.sel).border_color(t.accent))
            .on_click(cx.listener(move |this, _, window, cx| {
                let mut settings = this.settings.clone();
                settings.undo_send_secs = seconds;
                this.save_preferences(settings, window, cx);
            }))
        }),
    );

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
                    "Undo send",
                    "The engine holds outgoing mail for this window before sending.",
                ))
                .child(card(
                    t,
                    vec![row(
                        t,
                        "Hold before sending",
                        "Press z or the toast button to take a message back.",
                        undo,
                    )],
                )),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .child(heading(t, "Privacy", "Private by default."))
                .child(card(
                    t,
                    vec![
                        div()
                            .text_size(t.px(12.))
                            .text_color(t.fg2)
                            .child("Email scripts and remote images stay blocked. No tracking pixels or read receipts. Account secrets belong in the OS keychain, never in a theme or settings file.")
                            .into_any_element(),
                    ],
                )),
        )
        .into_any_element()
}
