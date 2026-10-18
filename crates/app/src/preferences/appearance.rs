//! Appearance: the theme gallery, typography and the live preview.
use super::{button, card, heading, row};
use crate::app::Kite;
use crate::theme::{self, RADIUS, RADIUS_SM, Theme};
use gpui_kit::assets::IconName;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{Icon, Sizable};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kite_core::model::{CustomTheme, Settings};
use std::io::Read as _;
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

impl Kite {
    fn choose_theme(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let mut settings = self.settings.clone();
        settings.theme = name.into();
        settings.font_color = None;
        self.save_preferences(settings, window, cx);
        if let Some(page) = &self.preferences {
            page.appearance.color.update(cx, |input, cx| {
                input.set_value(
                    self.settings.font_color.clone().unwrap_or_default(),
                    window,
                    cx,
                )
            });
        }
    }

    fn import_theme(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Import a Kite theme (.json)".into()),
        });
        cx.spawn(async move |this, cx| {
            let paths = match paths.await {
                Ok(Ok(Some(paths))) => paths,
                Ok(Ok(None)) => return,
                result => {
                    this.update(cx, |this, cx| {
                        if let Some(page) = &mut this.preferences {
                            page.error = Some(format!("Could not open theme picker: {result:?}"));
                        }
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let task = cx.background_executor().spawn(async move {
                let mut data = String::new();
                std::fs::File::open(path)?
                    .take(65_537)
                    .read_to_string(&mut data)?;
                anyhow::ensure!(
                    data.len() <= 65_536,
                    "Theme files must be smaller than 64 KiB."
                );
                let definition: CustomTheme = serde_json::from_str(&data)?;
                theme::custom(&definition)?;
                Ok::<_, anyhow::Error>(definition)
            });
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(definition) => {
                        let mut settings = this.settings.clone();
                        settings.custom_theme = Some(definition);
                        settings.theme = "custom".into();
                        settings.font_color = None;
                        // Resolve against a fixed appearance: an imported theme declares its base.
                        match theme::resolve(&settings, WindowAppearance::Dark).and_then(|theme| {
                            this.mail.set_settings(settings.clone())?;
                            Ok(theme)
                        }) {
                            Ok(theme) => {
                                this.settings = settings;
                                this.theme = theme;
                                this.dark = theme.dark;
                                this.appearance_version += 1;
                                this.html_cache.clear();
                                this.sidebar_dirty = true;
                                theme::install(&theme, theme::font(&this.settings), cx);
                                if let Some(page) = &mut this.preferences {
                                    page.appearance
                                        .color
                                        .update(cx, |input, cx| input.set_value("", window, cx));
                                    page.error = None;
                                    page.notice = Some("Theme imported and applied".into());
                                }
                            }
                            Err(error) => {
                                if let Some(page) = &mut this.preferences {
                                    page.error = Some(error.to_string());
                                }
                            }
                        }
                    }
                    Err(error) => {
                        if let Some(page) = &mut this.preferences {
                            page.error = Some(format!("Could not import theme: {error}"));
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

/// DESIGN › Settings: theme tiles show a miniature native inbox, not a generic swatch.
fn inbox_preview(palette: Theme) -> Div {
    let faint = |a: f32| Theme::alpha(palette.fg3, a);
    div()
        .h(px(88.))
        .rounded(px(RADIUS_SM))
        .overflow_hidden()
        .flex()
        .bg(palette.bg)
        .child(
            div()
                .w(px(34.))
                .h_full()
                .flex_shrink_0()
                .bg(palette.bg2)
                .px_1()
                .pt(px(10.))
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .h(px(6.))
                        .w(px(18.))
                        .rounded(px(2.))
                        .bg(palette.accent),
                )
                .children(
                    (0..3).map(move |_| div().h(px(5.)).w_full().rounded(px(2.)).bg(faint(0.28))),
                ),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .px_2()
                .py_2()
                .gap_1()
                .child(
                    div()
                        .h(px(6.))
                        .w(px(36.))
                        .rounded(px(2.))
                        .bg(Theme::alpha(palette.fg, 0.75)),
                )
                .children((0..3).map(move |ix| {
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .px_1()
                        .h(px(15.))
                        .rounded(px(3.))
                        .when(ix == 0, |d| d.bg(palette.sel))
                        .child(div().size(px(4.)).rounded_full().bg(palette.accent))
                        .child(
                            div()
                                .h(px(5.))
                                .w(px(24.))
                                .rounded(px(2.))
                                .bg(Theme::alpha(palette.fg2, 0.9)),
                        )
                        .child(
                            div()
                                .flex_1()
                                .h(px(5.))
                                .rounded(px(2.))
                                .bg(Theme::alpha(palette.fg2, 0.3)),
                        )
                        .child(div().h(px(5.)).w(px(10.)).rounded(px(2.)).bg(faint(0.45)))
                })),
        )
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

pub fn render(kite: &Kite, window: &Window, cx: &mut Context<Kite>) -> AnyElement {
    let Some(page) = &kite.preferences else {
        return div().into_any_element();
    };
    let t = kite.theme;
    let narrow = window.viewport_size().width < px(1000.);
    let size = kite.settings.font_size;

    let mut themes = vec![
        ("dark", "Carbon", theme::CARBON),
        ("light", "Paper", theme::PAPER),
        ("midnight", "Midnight", theme::preset("midnight")),
        ("linen", "Linen", theme::preset("linen")),
    ];
    if let Some(custom) = &kite.settings.custom_theme
        && let Ok(palette) = theme::custom(custom)
    {
        themes.push(("custom", custom.name.as_str(), palette));
    }

    let mut grid = div().flex().flex_col().gap_3();
    for group in themes.chunks(if narrow { 2 } else { 4 }) {
        grid = grid.child(div().flex().gap_3().children(group.iter().map(
            |(id, name, palette)| {
                let id = *id;
                let palette = *palette;
                let selected = kite.settings.theme == id;
                // The selected tile wears the accent glow as a 1px ring; the rest keep a hairline.
                let ring: Background = if selected {
                    theme::accent_glow(&t)
                } else {
                    t.line.into()
                };
                div()
                    .id(format!("theme-{id}"))
                    .test_support()
                    .aria_label(format!("{name} theme"))
                    .aria_selected(selected)
                    .flex_1()
                    .min_w_0()
                    .p(px(1.))
                    .rounded(px(RADIUS))
                    .bg(ring)
                    .cursor_pointer()
                    .when(selected, |d| {
                        d.shadow(vec![
                            BoxShadow::new(px(0.), px(0.), Theme::alpha(t.accent, 0.35))
                                .blur_radius(px(16.)),
                        ])
                    })
                    .when(!selected, |d| d.hover(|s| s.bg(Theme::alpha(t.fg, 0.16))))
                    .child(
                        div()
                            .w_full()
                            .min_w_0()
                            .rounded(px(RADIUS - 1.))
                            .bg(t.bg2)
                            .p(px(5.))
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(inbox_preview(palette))
                            .child(
                                div()
                                    .h(px(24.))
                                    .px_1()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap_2()
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .text_size(t.px(12.))
                                            .font_weight(if selected {
                                                FontWeight(650.)
                                            } else {
                                                FontWeight::MEDIUM
                                            })
                                            .text_color(if selected { t.fg } else { t.fg2 })
                                            .child(name.to_string()),
                                    )
                                    .when(selected, |d| {
                                        d.child(
                                            Icon::new(IconName::Check)
                                                .with_size(px(13.))
                                                .text_color(t.accent)
                                                .with_animation(
                                                    format!("theme-check-{id}"),
                                                    Animation::new(Duration::from_millis(140))
                                                        .with_easing(ease_out_quint()),
                                                    |icon, delta| icon.opacity(delta),
                                                ),
                                        )
                                    }),
                            ),
                    )
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.choose_theme(id, window, cx)),
                    )
            },
        )));
    }

    let theme_actions = card(
        t,
        vec![
            row(
                t,
                "Match system",
                "Follows the OS between Carbon and Paper.",
                button("theme-system", "Match system", t)
                    .aria_selected(kite.settings.theme == "system")
                    .when(kite.settings.theme == "system", |d| {
                        d.bg(t.sel).border_color(t.accent)
                    })
                    .on_click(
                        cx.listener(|this, _, window, cx| this.choose_theme("system", window, cx)),
                    ),
            ),
            row(
                t,
                "Imported palettes",
                "Load a theme file, or copy the template to start from.",
                div()
                    .flex()
                    .gap_2()
                    .child(
                        button("import-theme", "Import theme", t)
                            .on_click(cx.listener(|this, _, _, cx| this.import_theme(cx))),
                    )
                    .child(button("theme-template", "Copy JSON template", t).on_click(
                        cx.listener(|this, _, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(
                                crate::preferences::TEMPLATE.into(),
                            ));
                            if let Some(page) = &mut this.preferences {
                                page.notice = Some("Theme template copied".into());
                                page.error = None;
                            }
                            cx.notify();
                        }),
                    )),
            ),
        ],
    );

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
        Input::new(&page.appearance.font)
            .id("font-family")
            .aria_label("Font family"),
    );

    let typed = page.appearance.color.read(cx).value().trim().to_string();
    let parsed = theme::color(&typed).ok();
    let color_control = div()
        .flex()
        .items_center()
        .gap_2()
        .child(
            div().w(px(200.)).child(
                Input::new(&page.appearance.color)
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
                    page.appearance
                        .font
                        .update(cx, |input, cx| input.set_value("", window, cx));
                    page.appearance
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
                    settings.font_family = page.appearance.font.read(cx).value().trim().into();
                    let color = page.appearance.color.read(cx).value().trim().to_string();
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
                    "Theme",
                    "Every palette previews your real inbox.",
                ))
                .child(grid)
                .child(div().pt_3().child(theme_actions)),
        )
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
