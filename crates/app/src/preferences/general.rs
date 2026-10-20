//! General: the everyday defaults.
use super::{button, card, heading};
use crate::app::Kite;
use crate::theme::RADIUS_SM;
use gpui_kit::prelude::*;
use gpui_kit::*;

#[derive(Default)]
pub struct State;

pub fn render(kite: &Kite, _window: &Window, cx: &mut Context<Kite>) -> AnyElement {
    let t = kite.theme;
    let undo = div()
        .flex()
        .flex_wrap()
        .gap_2()
        .children([0, 5, 10, 20, 30].into_iter().map(|seconds| {
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
        }));
    // One tile per input mode. Switching applies to the next keystroke, with no restart.
    let mode = |value: &'static str, label: &'static str, description: &'static str| {
        let selected = kite.settings.input_mode == value;
        div()
            .id(format!("input-mode-{value}"))
            .test_support()
            .aria_label(label)
            .aria_selected(selected)
            .flex()
            .flex_col()
            .items_start()
            .gap_1()
            .px_3()
            .py_2()
            .rounded(px(RADIUS_SM))
            .border_1()
            .border_color(if selected { t.accent } else { t.line })
            .bg(if selected { t.sel } else { t.bg2 })
            .cursor_pointer()
            .hover(|s| s.bg(t.bg3))
            .child(
                div()
                    .text_size(t.px(13.))
                    .font_weight(FontWeight(500.))
                    .child(label),
            )
            .child(
                div()
                    .text_size(t.px(11.))
                    .text_color(t.fg2)
                    .child(description),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                if this.settings.input_mode == value {
                    return;
                }
                let mut settings = this.settings.clone();
                settings.input_mode = value.to_string();
                this.save_preferences(settings, window, cx);
                this.rebind(cx);
            }))
    };

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
                    "Input",
                    "Keyboard-first keeps every shortcut. Classic is click-first: single-key shortcuts are off so typing never runs a command, while Ctrl/Cmd chords, Escape, Enter and the arrows still work.",
                ))
                .child(card(
                    t,
                    vec![
                        div()
                            .flex()
                            .gap_2()
                            .child(mode(
                                "keyboard",
                                "Keyboard-first",
                                "Every command has a key.",
                            ))
                            .child(mode(
                                "classic",
                                "Classic (click)",
                                "Actions live on buttons in the list and thread.",
                            ))
                            .into_any_element(),
                    ],
                )),
        )
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
