//! Typography: base size, font family, text colour and a live preview.
use super::{button, card, heading, row};
use crate::app::Kite;
use crate::theme::{self, RADIUS, RADIUS_SM, Theme};
use gpui_kit::assets::IconName;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{Icon, Sizable};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kite_core::model::Settings;
use std::time::Duration;

pub struct State {
    pub font: Entity<InputState>,
    pub color: Entity<InputState>,
}

impl State {
    pub fn new(settings: &Settings, window: &mut Window, cx: &mut Context<Kite>) -> Self {
        let font = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("System font")
                .default_value(settings.font_family.clone())
        });
        let color = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Theme default")
                .default_value(settings.font_color.clone().unwrap_or_default())
        });
        // The swatch mirrors what is typed, so the page repaints while the field changes.
        cx.observe(&color, |_, _, cx| cx.notify()).detach();
        Self { font, color }
    }
}

/// A quiet square control, used for the text-size stepper. `button` is the labelled variant.
fn step(
    id: &'static str,
    label: &'static str,
    icon: IconName,
    t: Theme,
) -> gpui_kit::base::ObservedElement<Stateful<Div>> {
    div()
        .id(id)
        .test_support()
        .aria_label(label)
        .size(px(30.))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .text_color(t.fg2)
        .hover(|s| s.bg(t.bg3).text_color(t.fg))
        .child(Icon::new(icon).with_size(px(14.)))
}

/// The live typography preview: real mail proportions at the chosen font, size and colour.
fn preview(kite: &Kite, t: Theme) -> AnyElement {
    let font = theme::font(&kite.settings).to_string();
    let size = kite.settings.font_size;
    let body = size as f32;
    let meta = (body - 2.).max(11.);
    let subject = body + 3.;

    div()
        .id("typography-preview")
        .test_support()
        .aria_label(format!("{font} / {size} px"))
        .w_full()
        .rounded(px(RADIUS))
        .border_1()
        .border_color(t.line)
        .bg(t.bg2)
        .overflow_hidden()
        .flex()
        .flex_col()
        .font_family(font.clone())
        .child(
            div()
                .px_4()
                .pt_4()
                .flex()
                .items_center()
                .gap_3()
                .child(
                    div()
                        .size(px(28.))
                        .flex_shrink_0()
                        .rounded_full()
                        .bg(Theme::alpha(t.accent, 0.18))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(meta))
                        .font_weight(FontWeight(650.))
                        .text_color(t.accent)
                        .child("AC"),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .truncate()
                                .text_size(px(meta))
                                .font_weight(FontWeight::MEDIUM)
                                .child("Alice Chen"),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_size(px(meta))
                                .text_color(t.fg3)
                                .child("to me"),
                        ),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .text_size(px(meta))
                        .text_color(t.fg3)
                        .child("9:41"),
                ),
        )
        .child(
            div()
                .px_4()
                .pt_3()
                .pb_4()
                .max_w(px(520.))
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .text_size(px(subject))
                        .font_weight(FontWeight(650.))
                        .text_color(t.fg)
                        .child("A few thoughts on the next chapter"),
                )
                .child(
                    div()
                        .text_size(px(body))
                        .text_color(t.fg2)
                        .child("The draft reads well to me. I moved the schedule to Thursday and left the budget section for you — let me know if you would rather see the numbers in the appendix."),
                ),
        )
        .child(
            div()
                .border_t_1()
                .border_color(t.line)
                .px_4()
                .py_2()
                .flex()
                .items_center()
                .justify_between()
                .gap_4()
                .text_size(px(meta))
                .text_color(t.fg3)
                .child(div().flex_1().min_w_0().truncate().child(font))
                .child(
                    div()
                        .flex_shrink_0()
                        .child(format!("{size} px · {}", theme::hex(t.fg))),
                ),
        )
        // Motion is decorative: the resolved type settles in when it changes.
        .with_animation(
            ("typography-preview", kite.appearance_version),
            Animation::new(Duration::from_millis(140)).with_easing(ease_out_quint()),
            |d, delta| d.opacity(0.7 + 0.3 * delta),
        )
        .into_any_element()
}

pub fn render(kite: &Kite, _window: &Window, cx: &mut Context<Kite>) -> AnyElement {
    let Some(page) = &kite.preferences else {
        return div().into_any_element();
    };
    let t = kite.theme;
    let size = kite.settings.font_size;

    let size_control = div()
        .flex()
        .items_center()
        .rounded(px(RADIUS_SM))
        .border_1()
        .border_color(t.line)
        .bg(t.bg2)
        .overflow_hidden()
        .child(
            step("font-smaller", "Smaller text", IconName::Minus, t).on_click(cx.listener(
                |this, _, window, cx| {
                    let mut settings = this.settings.clone();
                    settings.font_size = settings.font_size.saturating_sub(1).clamp(12, 18);
                    this.save_preferences(settings, window, cx);
                },
            )),
        )
        .child(
            div()
                .id("font-size-value")
                .test_support()
                .aria_label(format!("{size} px"))
                .w(px(58.))
                .h(px(30.))
                .flex()
                .items_center()
                .justify_center()
                .border_l_1()
                .border_r_1()
                .border_color(t.line)
                .text_size(t.px(13.))
                .font_weight(FontWeight::MEDIUM)
                .child(format!("{size} px")),
        )
        .child(
            step("font-larger", "Larger text", IconName::Plus, t).on_click(cx.listener(
                |this, _, window, cx| {
                    let mut settings = this.settings.clone();
                    settings.font_size = (settings.font_size + 1).clamp(12, 18);
                    this.save_preferences(settings, window, cx);
                },
            )),
        );

    let font_control = div().w(px(220.)).child(
        Input::new(&page.typography.font)
            .id("font-family")
            .aria_label("Font family"),
    );

    let typed = page.typography.color.read(cx).value().trim().to_string();
    let parsed = theme::color(&typed).ok();
    let color_control = div()
        .flex()
        .items_center()
        .gap_2()
        .child(
            div().w(px(200.)).child(
                Input::new(&page.typography.color)
                    .id("font-color")
                    .aria_label("Text colour"),
            ),
        )
        .child(
            div()
                .id("font-color-swatch")
                .test_support()
                .aria_label(match parsed {
                    Some(color) => format!("Text colour {}", theme::hex(color)),
                    None => "Text colour not set".to_string(),
                })
                .size(px(24.))
                .flex_shrink_0()
                .rounded(px(RADIUS_SM))
                .border_1()
                .border_color(t.line)
                .bg(parsed.unwrap_or(t.bg3)),
        );

    let apply = div()
        .flex()
        .items_center()
        .justify_end()
        .gap_2()
        .child(button("reset-typography", "Reset", t).on_click(cx.listener(
            |this, _, window, cx| {
                let mut settings = this.settings.clone();
                settings.font_family.clear();
                settings.font_color = None;
                settings.font_size = 14;
                this.save_preferences(settings, window, cx);
                if let Some(page) = &this.preferences {
                    page.typography
                        .font
                        .update(cx, |input, cx| input.set_value("", window, cx));
                    page.typography
                        .color
                        .update(cx, |input, cx| input.set_value("", window, cx));
                }
            },
        )))
        .child(
            button("apply-typography", "Apply typography", t)
                .bg(Theme::alpha(t.accent, 0.18))
                .border_color(Theme::alpha(t.accent, 0.55))
                .font_weight(FontWeight(650.))
                .on_click(cx.listener(|this, _, window, cx| {
                    let Some(page) = &this.preferences else {
                        return;
                    };
                    let mut settings = this.settings.clone();
                    settings.font_family = page.typography.font.read(cx).value().trim().into();
                    let color = page.typography.color.read(cx).value().trim().to_string();
                    settings.font_color = (!color.is_empty()).then_some(color);
                    this.save_preferences(settings, window, cx);
                })),
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
                    "Typography",
                    "Any installed font, 12–18 px. Mail and the interface move together.",
                ))
                .child(card(
                    t,
                    vec![
                        row(
                            t,
                            "Text size",
                            "List rows, mail and the interface resize together.",
                            size_control,
                        ),
                        row(
                            t,
                            "Font family",
                            "Any font installed on this computer. Empty uses the system font.",
                            font_control,
                        ),
                        row(
                            t,
                            "Text colour",
                            "Optional hex. Applied only when it keeps 4.5:1 contrast.",
                            color_control,
                        ),
                        apply.into_any_element(),
                    ],
                ))
                .child(div().pt_3().child(preview(kite, t))),
        )
        .into_any_element()
}
