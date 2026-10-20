//! Appearance: the theme gallery and native theme import.
use super::{button, card, heading, row};
use crate::app::Kite;
use crate::theme::{self, RADIUS, RADIUS_SM, Theme};
use gpui_kit::assets::IconName;
use gpui_kit::component::{Icon, Sizable};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kite_core::model::CustomTheme;
use std::io::Read as _;
use std::time::Duration;

#[derive(Default)]
pub struct State;

impl Kite {
    fn choose_theme(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let mut settings = self.settings.clone();
        settings.theme = name.into();
        settings.font_color = None;
        self.save_preferences(settings, window, cx);
        if let Some(page) = &self.preferences {
            page.typography.color.update(cx, |input, cx| {
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
                                    page.typography
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

pub fn render(kite: &Kite, window: &Window, cx: &mut Context<Kite>) -> AnyElement {
    let t = kite.theme;

    let mut themes = vec![
        ("dark", "Carbon", theme::CARBON),
        ("light", "Paper", theme::PAPER),
        ("midnight", "Midnight", theme::preset("midnight")),
        ("linen", "Linen", theme::preset("linen")),
        ("ember", "Ember", theme::preset("ember")),
    ];
    if let Some(custom) = &kite.settings.custom_theme
        && let Ok(palette) = theme::custom(custom)
    {
        themes.push(("custom", custom.name.as_str(), palette));
    }

    // Tiles keep one width in every row: the grid wraps at the count that fits the content
    // column at TILE_MIN, and a short last row stays left-aligned instead of stretching.
    const TILE_MIN: f32 = 124.;
    const GAP: f32 = 12.;
    let content = super::content_width(window);
    let columns = if super::compact(window) {
        2
    } else {
        ((content + GAP) / (TILE_MIN + GAP)) as usize
    }
    .clamp(1, themes.len());
    let tile_w = (content - GAP * (columns - 1) as f32) / columns as f32;

    let mut grid = div().flex().flex_col().gap_3();
    for group in themes.chunks(columns) {
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
                    .w(px(tile_w))
                    .flex_shrink_0()
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
                    "Layout",
                    "How much of the window the conversation gets.",
                ))
                .child(card(
                    t,
                    vec![row(
                        t,
                        "Reading pane",
                        "Read beside the list; off opens a conversation across the window.",
                        switch("reading-pane", kite.settings.reading_pane, t).on_click(
                            cx.listener(|this, _, window, cx| {
                                let mut settings = this.settings.clone();
                                settings.reading_pane = !settings.reading_pane;
                                this.save_preferences(settings, window, cx);
                            }),
                        ),
                    )],
                )),
        )
        .into_any_element()
}

/// An on/off setting drawn as a switch: a filled track when on.
fn switch(id: &'static str, on: bool, t: Theme) -> gpui_kit::base::ObservedElement<Stateful<Div>> {
    div()
        .id(id)
        .test_support()
        .aria_label("Reading pane")
        .aria_selected(on)
        .w(px(36.))
        .h(px(20.))
        .flex()
        .items_center()
        .p(px(2.))
        .when(on, |d| d.justify_end())
        .rounded_full()
        .cursor_pointer()
        .bg(if on {
            t.accent.into()
        } else {
            Theme::alpha(t.fg3, 0.45)
        })
        .child(div().size(px(16.)).rounded_full().bg(t.bg))
}
