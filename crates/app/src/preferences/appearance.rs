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
        .h(px(90.))
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
                .pt(px(12.))
                .flex()
                .flex_col()
                .gap_1()
                .child(div().h(px(7.)).w_full().rounded(px(2.)).bg(palette.sel))
                .children(
                    (0..3).map(move |_| div().h(px(5.)).w_full().rounded(px(2.)).bg(faint(0.3))),
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
                        .h(px(7.))
                        .w(px(38.))
                        .rounded(px(2.))
                        .bg(Theme::alpha(palette.fg, 0.7)),
                )
                .children((0..3).map(move |ix| {
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .px_1()
                        .h(px(16.))
                        .rounded(px(3.))
                        .when(ix == 0, |d| d.bg(palette.sel))
                        .child(div().size(px(4.)).rounded_full().bg(palette.accent))
                        .child(
                            div()
                                .h(px(5.))
                                .w(px(26.))
                                .rounded(px(2.))
                                .bg(Theme::alpha(palette.fg2, 0.85)),
                        )
                        .child(
                            div()
                                .flex_1()
                                .h(px(5.))
                                .rounded(px(2.))
                                .bg(Theme::alpha(palette.fg2, 0.35)),
                        )
                        .child(div().h(px(5.)).w(px(12.)).rounded(px(2.)).bg(faint(0.5)))
                })),
        )
}

pub fn render(kite: &Kite, window: &Window, cx: &mut Context<Kite>) -> AnyElement {
    let Some(page) = &kite.preferences else {
        return div().into_any_element();
    };
    let t = kite.theme;
    let narrow = window.viewport_size().width < px(1000.);

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
                div()
                    .id(format!("theme-{id}"))
                    .test_support()
                    .aria_label(format!("{name} theme"))
                    .aria_selected(selected)
                    .flex_1()
                    .min_w_0()
                    .p_1()
                    .rounded(px(RADIUS))
                    .border_2()
                    .border_color(if selected { t.accent } else { t.line })
                    .cursor_pointer()
                    .hover(|s| s.border_color(t.fg3))
                    .child(inbox_preview(palette))
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .items_center()
                            .px_2()
                            .py_2()
                            .text_size(t.px(12.))
                            .child(name.to_string())
                            .when(selected, |d| {
                                d.child(
                                    Icon::new(IconName::Check)
                                        .with_size(px(13.))
                                        .text_color(t.accent),
                                )
                            }),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.choose_theme(id, window, cx)
                    }))
            },
        )));
    }

    let actions = div()
        .flex()
        .flex_wrap()
        .gap_2()
        .child(
            button("theme-system", "Match system", t)
                .aria_selected(kite.settings.theme == "system")
                .when(kite.settings.theme == "system", |d| {
                    d.bg(t.sel).border_color(t.accent)
                })
                .on_click(cx.listener(|this, _, window, cx| {
                    this.choose_theme("system", window, cx)
                })),
        )
        .child(
            button("import-theme", "Import theme", t)
                .on_click(cx.listener(|this, _, _, cx| this.import_theme(cx))),
        )
        .child(button("theme-template", "Copy JSON template", t).on_click(cx.listener(
            |this, _, _, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(
                    crate::preferences::TEMPLATE.into(),
                ));
                if let Some(page) = &mut this.preferences {
                    page.notice = Some("Theme template copied".into());
                    page.error = None;
                }
                cx.notify();
            },
        )));

    let size = div()
        .flex()
        .items_center()
        .gap_3()
        .child(button("font-smaller", "−", t).on_click(cx.listener(
            |this, _, window, cx| {
                let mut settings = this.settings.clone();
                settings.font_size = settings.font_size.saturating_sub(1).max(12);
                this.save_preferences(settings, window, cx);
            },
        )))
        .child(
            div()
                .id("font-size-value")
                .test_support()
                .aria_label(format!("{} px", kite.settings.font_size))
                .w(px(40.))
                .text_size(t.px(13.))
                .child(format!("{} px", kite.settings.font_size)),
        )
        .child(button("font-larger", "+", t).on_click(cx.listener(
            |this, _, window, cx| {
                let mut settings = this.settings.clone();
                settings.font_size = (settings.font_size + 1).min(18);
                this.save_preferences(settings, window, cx);
            },
        )));

    let inputs = div()
        .flex()
        .gap_4()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .text_size(t.px(12.))
                        .text_color(t.fg2)
                        .child("Font family"),
                )
                .child(
                    Input::new(&page.appearance.font)
                        .id("font-family")
                        .aria_label("Font family"),
                ),
        )
        .child(
            div()
                .w(px(156.))
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .text_size(t.px(12.))
                        .text_color(t.fg2)
                        .child("Text colour · optional"),
                )
                .child(
                    Input::new(&page.appearance.color)
                        .id("font-color")
                        .aria_label("Text colour"),
                ),
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
                .child(heading(t, "Theme", "Palettes preview the real inbox."))
                .child(grid),
        )
        .child(actions)
        .child(
            div()
                .flex()
                .flex_col()
                .child(heading(
                    t,
                    "Typography",
                    "Use any installed font. Mail text and the interface resize together.",
                ))
                .child(card(
                    t,
                    vec![
                        row(t, "Text size", "12–18 px, applied to list and mail.", size),
                        inputs.into_any_element(),
                        div()
                            .id("typography-preview")
                            .test_support()
                            .aria_label(format!(
                                "{} / {} px",
                                theme::font(&kite.settings),
                                kite.settings.font_size
                            ))
                            .bg(t.bg3)
                            .rounded(px(RADIUS_SM))
                            .p_4()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(t.px(18.))
                                    .font_weight(FontWeight(650.))
                                    .child("Less noise. More room to think."),
                            )
                            .child(
                                div()
                                    .text_size(t.px(13.))
                                    .text_color(t.fg2)
                                    .child("This is how your conversations will read."),
                            )
                            .into_any_element(),
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                button("apply-typography", "Apply typography", t).on_click(
                                    cx.listener(|this, _, window, cx| {
                                        let Some(page) = &this.preferences else {
                                            return;
                                        };
                                        let mut settings = this.settings.clone();
                                        settings.font_family =
                                            page.appearance.font.read(cx).value().trim().into();
                                        let color =
                                            page.appearance.color.read(cx).value().trim().to_string();
                                        settings.font_color =
                                            (!color.is_empty()).then_some(color);
                                        this.save_preferences(settings, window, cx);
                                    }),
                                ),
                            )
                            .child(button("reset-typography", "Reset", t).on_click(
                                cx.listener(|this, _, window, cx| {
                                    let mut settings = this.settings.clone();
                                    settings.font_family.clear();
                                    settings.font_color = None;
                                    settings.font_size = 14;
                                    this.save_preferences(settings, window, cx);
                                    if let Some(page) = &this.preferences {
                                        page.appearance.font.update(cx, |input, cx| {
                                            input.set_value("", window, cx)
                                        });
                                        page.appearance.color.update(cx, |input, cx| {
                                            input.set_value("", window, cx)
                                        });
                                    }
                                }),
                            ))
                            .into_any_element(),
                    ],
                )),
        )
        .into_any_element()
}
