//! General: the everyday defaults.
use super::{button, card, heading};
use crate::app::Kite;
use gpui_kit::prelude::*;
use gpui_kit::*;

#[derive(Default)]
pub struct State;

pub fn render(kite: &Kite, _window: &Window, cx: &mut Context<Kite>) -> AnyElement {
    let t = kite.theme;
    let undo = div().flex().flex_wrap().gap_2().children(
        [0, 5, 10, 20, 30].into_iter().map(|seconds| {
            let selected = kite.settings.undo_send_secs == seconds;
            let label = if seconds == 0 {
                "Off".to_string()
            } else {
                format!("{seconds}s")
            };
            button(format!("undo-send-{seconds}"), &label, t)
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
                    vec![
                        // Stacked, not a row: five chips never fit beside the label at the
                        // 720px minimum window.
                        div()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(div().text_size(t.px(13.)).child("Hold before sending"))
                            .child(
                                div().text_size(t.px(12.)).text_color(t.fg2).child(
                                    "Press z or the toast button to take a message back.",
                                ),
                            )
                            .child(undo)
                            .into_any_element(),
                    ],
                )),
        )
        .into_any_element()
}
