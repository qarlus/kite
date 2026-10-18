//! Privacy: what mail is allowed to load, and what Kite never sends.
use super::{card, heading, row};
use crate::app::Kite;
use gpui_kit::component::switch::Switch;
use gpui_kit::prelude::*;
use gpui_kit::*;

#[derive(Default)]
pub struct State;

pub fn render(kite: &Kite, _window: &Window, cx: &mut Context<Kite>) -> AnyElement {
    let t = kite.theme;
    let weak = cx.weak_entity();
    let remote_images = Switch::new("remote-images")
        .checked(kite.settings.load_remote_images)
        .accessibility_label("Load remote images")
        .on_click(move |checked, window, cx| {
            let checked = *checked;
            weak.update(cx, |kite, cx| {
                let mut settings = kite.settings.clone();
                settings.load_remote_images = checked;
                kite.save_preferences(settings, window, cx);
            })
            .ok();
        });

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
                .child(heading(t, "Privacy", "Private by default."))
                .child(card(
                    t,
                    vec![
                        row(
                            t,
                            "Remote images",
                            "Off keeps tracking pixels from learning when you read a message.",
                            remote_images,
                        ),
                        div()
                            .text_size(t.px(12.))
                            .text_color(t.fg2)
                            .child("Email scripts stay blocked and no read receipts are sent. Account secrets live in the operating system keychain, never in settings.")
                            .into_any_element(),
                    ],
                )),
        )
        .into_any_element()
}
