//! Root view. Wave 2 replaces the body with the split bar, list, thread view and overlays.
use crate::theme::{BAR_H, CARBON, Theme};
use gpui_kit::*;

pub struct Kite;

impl Render for Kite {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let t = &CARBON;
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(t.bg)
            .text_color(t.fg)
            .child(div().h(px(BAR_H)).flex_none())
            .child(
                div()
                    .id("inbox-zero")
                    .test_support()
                    .aria_label("Inbox Zero")
                    .flex_1()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .bg(linear_gradient(
                        135.,
                        linear_color_stop(Theme::alpha(t.accent, 0.12), 0.),
                        linear_color_stop(Theme::alpha(t.accent2, 0.12), 1.),
                    ))
                    .child(
                        div()
                            .text_size(px(28.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Inbox Zero"),
                    )
                    .child(div().text_color(t.fg2).child("You're all caught up.")),
            )
    }
}
