//! Preferences stay behind the Mail boundary; provider work never runs on the UI thread.
use crate::app::{Kite, Overlay};
use crate::theme::{self, RADIUS, RADIUS_SM, Theme};
use gpui_kit::assets::IconName;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{Icon, Sizable};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kite_core::model::{CustomTheme, Id, ProtonLogin, Settings};
use std::io::Read as _;

pub struct Preferences {
    pub tab: usize,
    font: Entity<InputState>,
    color: Entity<InputState>,
    email: Entity<InputState>,
    password: Entity<InputState>,
    proton: bool,
    busy: bool,
    remove: Option<Id>,
    error: Option<String>,
    notice: Option<String>,
    request: u64,
    scroll: ScrollHandle,
}

pub const TEMPLATE: &str = "{\n  \"name\": \"My midnight\",\n  \"base\": \"dark\",\n  \"colors\": {\n    \"bg\": \"#101820\",\n    \"bg2\": \"#16212b\",\n    \"accent\": \"#77c5dc\"\n  }\n}";

impl Kite {
    pub(crate) fn show_preferences(
        &mut self,
        tab: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.overlay = Overlay::None;
        if let Some(preferences) = &mut self.preferences {
            preferences.tab = tab;
        } else {
            self.seq += 1;
            let font = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("System font")
                    .default_value(self.settings.font_family.clone())
            });
            let color = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("Theme default")
                    .default_value(self.settings.font_color.clone().unwrap_or_default())
            });
            let email = cx.new(|cx| InputState::new(window, cx).placeholder("Bridge username"));
            let password = cx.new(|cx| InputState::new(window, cx).placeholder("Bridge password"));
            password.update(cx, |input, cx| input.set_masked(true, window, cx));
            self.preferences = Some(Preferences {
                tab,
                font,
                color,
                email,
                password,
                proton: false,
                busy: self.account_busy,
                remove: None,
                error: None,
                notice: None,
                request: self.seq,
                scroll: ScrollHandle::new(),
            });
        }
        self.sidebar_dirty = true;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn save_preferences(&mut self, settings: Settings, window: &Window, cx: &mut Context<Self>) {
        let result = theme::resolve(&settings, window.appearance()).and_then(|theme| {
            self.mail.set_settings(settings.clone())?;
            Ok(theme)
        });
        match result {
            Ok(theme) => {
                self.settings = settings;
                self.theme = theme;
                self.dark = theme.dark;
                self.appearance_version += 1;
                self.html_cache.clear();
                self.sidebar_dirty = true;
                theme::install(&theme, theme::font(&self.settings), cx);
                if let Some(page) = &mut self.preferences {
                    page.error = None;
                    page.notice = Some("Changes saved".into());
                }
            }
            Err(error) => {
                if let Some(page) = &mut self.preferences {
                    page.error = Some(error.to_string());
                    page.notice = None;
                }
            }
        }
        cx.notify();
    }

    fn choose_theme(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let mut settings = self.settings.clone();
        settings.theme = name.into();
        settings.font_color = None;
        self.save_preferences(settings, window, cx);
        if let Some(page) = &self.preferences {
            page.color.update(cx, |input, cx| {
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
                                    page.color
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

    fn connect_account(&mut self, proton: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(page) = &mut self.preferences else {
            return;
        };
        if self.account_busy {
            return;
        }
        let login = if proton {
            let email = page.email.read(cx).value().trim().to_string();
            let password = page.password.read(cx).value().to_string();
            if email.is_empty() || password.is_empty() {
                page.error = Some("Enter your Bridge username and password.".into());
                cx.notify();
                return;
            }
            page.password
                .update(cx, |input, cx| input.set_value("", window, cx));
            Some(ProtonLogin {
                email,
                password,
                host: None,
                imap_port: None,
                smtp_port: None,
            })
        } else {
            None
        };
        self.account_busy = true;
        page.busy = true;
        page.error = None;
        page.notice = None;
        let request = page.request;
        let mail = self.mail.clone();
        let task = cx.background_executor().spawn(async move {
            if let Some(login) = login {
                mail.add_proton(login)
            } else {
                mail.add_microsoft()
            }
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.account_busy = false;
                if let Some(page) = &mut this.preferences {
                    page.busy = false;
                }
                let active = this
                    .preferences
                    .as_ref()
                    .is_some_and(|page| page.request == request);
                match result {
                    Ok(account) => {
                        this.accounts = this
                            .mail
                            .accounts()
                            .unwrap_or_else(|_| this.accounts.clone());
                        if !this
                            .accounts
                            .iter()
                            .any(|existing| existing.id == account.id)
                        {
                            this.accounts.push(account);
                        }
                        this.folders = this.mail.folders().unwrap_or_default();
                        this.sidebar_dirty = true;
                        this.reload();
                        if active && let Some(page) = &mut this.preferences {
                            page.busy = false;
                            page.proton = false;
                            page.notice = Some("Account connected".into());
                        }
                    }
                    Err(error) => {
                        if active && let Some(page) = &mut this.preferences {
                            page.busy = false;
                            page.error = Some(error.to_string());
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn disconnect_account(&mut self, id: Id, cx: &mut Context<Self>) {
        let Some(page) = &mut self.preferences else {
            return;
        };
        if self.account_busy {
            return;
        }
        self.account_busy = true;
        page.busy = true;
        let request = page.request;
        let mail = self.mail.clone();
        let task = cx
            .background_executor()
            .spawn(async move { mail.remove_account(id) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.account_busy = false;
                if let Some(page) = &mut this.preferences {
                    page.busy = false;
                }
                let active = this
                    .preferences
                    .as_ref()
                    .is_some_and(|page| page.request == request);
                match result {
                    Ok(()) => {
                        this.accounts.retain(|account| account.id != id);
                        this.folders.retain(|folder| folder.account_id != id);
                        if this.view.account_id == Some(id) {
                            this.view.account_id = None;
                            this.view.folder = None;
                        }
                        if this.open.as_ref().is_some_and(|open| open.account_id == id) {
                            this.open = None;
                        }
                        this.sidebar_dirty = true;
                        this.reload();
                        if active && let Some(page) = &mut this.preferences {
                            page.busy = false;
                            page.remove = None;
                            page.notice = Some("Account removed from Kite".into());
                        }
                    }
                    Err(error) => {
                        if active && let Some(page) = &mut this.preferences {
                            page.busy = false;
                            page.error = Some(error.to_string());
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
}

fn heading(t: Theme, title: &str, description: &str) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .text_size(t.px(20.))
                .font_weight(FontWeight(650.))
                .child(title.to_string()),
        )
        .child(
            div()
                .text_size(t.px(12.))
                .text_color(t.fg2)
                .child(description.to_string()),
        )
}

fn button(
    id: impl Into<ElementId>,
    label: &str,
    t: Theme,
) -> gpui_kit::base::ObservedElement<Stateful<Div>> {
    div()
        .id(id)
        .test_support()
        .aria_label(label.to_string())
        .px_3()
        .py_2()
        .rounded(px(RADIUS_SM))
        .border_1()
        .border_color(t.line)
        .bg(t.bg2)
        .text_size(t.px(12.))
        .cursor_pointer()
        .hover(|s| s.bg(t.bg3))
        .child(label.to_string())
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
    let mut content = div()
        .w_full()
        .max_w(px(820.))
        .flex()
        .flex_col()
        .gap_6()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .text_size(t.px(10.))
                                .font_weight(FontWeight(650.))
                                .text_color(t.accent)
                                .child("YOUR EMAIL"),
                        )
                        .child(
                            div()
                                .text_size(t.px(28.))
                                .font_weight(FontWeight(650.))
                                .child("Settings"),
                        ),
                )
                .child(
                    button("close-settings", "Back to mail", t).on_click(cx.listener(
                        |this, _, window, cx| {
                            this.preferences = None;
                            this.sidebar_dirty = true;
                            window.focus(&this.focus, cx);
                            cx.notify();
                        },
                    )),
                ),
        )
        .child(
            div()
                .flex()
                .gap_6()
                .border_b_1()
                .border_color(t.line)
                .children(
                    ["Appearance", "Accounts", "General"]
                        .into_iter()
                        .enumerate()
                        .map(|(index, label)| {
                            div()
                                .id(("settings-tab", index))
                                .test_support()
                                .aria_label(label)
                                .aria_selected(page.tab == index)
                                .pb_3()
                                .text_size(t.px(13.))
                                .text_color(if page.tab == index { t.fg } else { t.fg2 })
                                .when(page.tab == index, |d| {
                                    d.border_b_2()
                                        .border_color(t.accent)
                                        .font_weight(FontWeight(650.))
                                })
                                .cursor_pointer()
                                .child(label)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(page) = &mut this.preferences {
                                        page.tab = index;
                                        page.error = None;
                                        page.notice = None;
                                        page.scroll = ScrollHandle::new();
                                    }
                                    cx.notify();
                                }))
                        }),
                ),
        );
    match page.tab {
        0 => {
            content = content.child(heading(
                t,
                "A space that feels like you",
                "Choose a palette, tune the type, and make room for your mail.",
            ));
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
            for row in themes.chunks(if narrow { 2 } else { 4 }) {
                grid = grid.child(div().flex().gap_3().children(row.iter().map(
                    |(id, name, palette)| {
                        let id = *id;
                        let palette = *palette;
                        div()
                            .id(format!("theme-{id}"))
                            .test_support()
                            .aria_label(format!("{name} theme"))
                            .aria_selected(kite.settings.theme == id)
                            .flex_1()
                            .min_w_0()
                            .p_1()
                            .rounded(px(RADIUS))
                            .border_2()
                            .border_color(if kite.settings.theme == id {
                                t.accent
                            } else {
                                t.line
                            })
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
                                    .when(kite.settings.theme == id, |d| {
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
            content = content.child(grid).child(
                div()
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
                    .child(button("theme-template", "Copy JSON template", t).on_click(
                        cx.listener(|this, _, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(TEMPLATE.into()));
                            if let Some(page) = &mut this.preferences {
                                page.notice = Some("Theme template copied".into());
                                page.error = None;
                            }
                            cx.notify();
                        }),
                    )),
            );
            content = content.child(div().h(px(1.)).bg(t.line)).child(heading(
                t,
                "Typography",
                "Use any installed font. Mail text and the interface resize together.",
            ));
            content = content.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().text_size(t.px(13.)).child("Text size"))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(button("font-smaller", "−", t).on_click(cx.listener(
                                |this, _, window, cx| {
                                    let mut settings = this.settings.clone();
                                    settings.font_size =
                                        settings.font_size.saturating_sub(1).max(12);
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
                            ))),
                    ),
            );
            content = content.child(
                div()
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
                                Input::new(&page.font)
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
                                Input::new(&page.color)
                                    .id("font-color")
                                    .aria_label("Text colour"),
                            ),
                    ),
            );
            content = content.child(
                div()
                    .id("typography-preview")
                    .test_support()
                    .aria_label(format!(
                        "{} / {} px",
                        theme::font(&kite.settings),
                        kite.settings.font_size
                    ))
                    .bg(t.bg2)
                    .rounded(px(RADIUS))
                    .p_5()
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
                    ),
            );
            content =
                content.child(
                    div()
                        .flex()
                        .gap_2()
                        .child(button("apply-typography", "Apply typography", t).on_click(
                            cx.listener(|this, _, window, cx| {
                                let Some(page) = &this.preferences else {
                                    return;
                                };
                                let mut settings = this.settings.clone();
                                settings.font_family = page.font.read(cx).value().trim().into();
                                let color = page.color.read(cx).value().trim().to_string();
                                settings.font_color = (!color.is_empty()).then_some(color);
                                this.save_preferences(settings, window, cx);
                            }),
                        ))
                        .child(button("reset-typography", "Reset", t).on_click(cx.listener(
                            |this, _, window, cx| {
                                let mut settings = this.settings.clone();
                                settings.font_family.clear();
                                settings.font_color = None;
                                settings.font_size = 14;
                                this.save_preferences(settings, window, cx);
                                if let Some(page) = &this.preferences {
                                    page.font
                                        .update(cx, |input, cx| input.set_value("", window, cx));
                                    page.color
                                        .update(cx, |input, cx| input.set_value("", window, cx));
                                }
                            },
                        ))),
                );
        }
        1 => {
            content = content.child(heading(
                t,
                "All your mail. One place.",
                "Keep accounts together, or give each inbox its own space.",
            ));
            for account in &kite.accounts {
                let id = account.id;
                let provider = match account.provider {
                    kite_core::model::Provider::Proton => "Proton Mail",
                    kite_core::model::Provider::Microsoft => "Microsoft Outlook",
                };
                content = content.child(
                    div()
                        .id(format!("settings-account-{id}"))
                        .test_support()
                        .aria_label(account.email.clone())
                        .py_4()
                        .border_b_1()
                        .border_color(t.line)
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(
                            div()
                                .size(px(40.))
                                .rounded_full()
                                .bg(t.sel)
                                .text_color(t.accent)
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    account
                                        .name
                                        .chars()
                                        .take(2)
                                        .collect::<String>()
                                        .to_uppercase(),
                                ),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(
                                    div()
                                        .truncate()
                                        .text_size(t.px(14.))
                                        .font_weight(FontWeight(650.))
                                        .child(account.email.clone()),
                                )
                                .child(
                                    div().text_size(t.px(12.)).text_color(t.fg2).child(provider),
                                ),
                        )
                        .when(!page.busy, |d| {
                            d.child(
                                button(format!("remove-account-{id}"), "Remove", t).on_click(
                                    cx.listener(move |this, _, _, cx| {
                                        if let Some(page) = &mut this.preferences {
                                            page.remove = Some(id);
                                            page.error = None;
                                        }
                                        cx.notify();
                                    }),
                                ),
                            )
                        }),
                );
            }
            if let Some(id) = page.remove {
                content =
                    content.child(
                        div()
                            .p_4()
                            .rounded(px(RADIUS_SM))
                            .bg(t.bg2)
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(div().text_size(t.px(13.)).child(
                                "Remove this account from Kite? Mail on the server is kept.",
                            ))
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .child(
                                        button("confirm-remove-account", "Remove account", t)
                                            .text_color(t.danger)
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.disconnect_account(id, cx)
                                            })),
                                    )
                                    .child(button("cancel-remove-account", "Cancel", t).on_click(
                                        cx.listener(|this, _, _, cx| {
                                            if let Some(page) = &mut this.preferences {
                                                page.remove = None;
                                            }
                                            cx.notify();
                                        }),
                                    )),
                            ),
                    );
            }
            content = content.child(heading(
                t,
                "Connect an account",
                "Microsoft opens a secure sign-in. Proton uses your local Bridge credentials.",
            ));
            if page.busy {
                content = content.child(
                    div()
                        .id("account-connecting")
                        .test_support()
                        .aria_label("Working on account")
                        .text_size(t.px(13.))
                        .text_color(t.accent)
                        .child("Working on your account…"),
                );
            } else {
                content = content.child(
                    div()
                        .flex()
                        .gap_3()
                        .child(
                            button("add-microsoft", "Connect Outlook", t).on_click(cx.listener(
                                |this, _, window, cx| this.connect_account(false, window, cx),
                            )),
                        )
                        .child(button("add-proton", "Connect Proton Mail", t).on_click(
                            cx.listener(|this, _, window, cx| {
                                if let Some(page) = &mut this.preferences {
                                    page.proton = true;
                                    page.error = None;
                                    let focus = page.email.read(cx).focus_handle(cx);
                                    window.focus(&focus, cx);
                                }
                                cx.notify();
                            }),
                        )),
                );
                if page.proton {
                    content = content.child(div().p_5().bg(t.bg2).rounded(px(RADIUS)).flex().flex_col().gap_3().child(div().text_size(t.px(12.)).text_color(t.fg2).child("Use credentials from Proton Mail Bridge, not your Proton password. Bridge must be running on this computer."))
                        .child(Input::new(&page.email).id("proton-username").aria_label("Bridge username"))
                        .child(Input::new(&page.password).id("proton-password").aria_label("Bridge password"))
                        .child(button("connect-proton", "Connect Bridge", t).on_click(cx.listener(|this, _, window, cx| this.connect_account(true, window, cx)))));
                }
            }
        }
        _ => {
            content = content.child(heading(
                t,
                "A few useful defaults",
                "Small choices that make the everyday feel effortless.",
            ));
            content = content.child(div().h(px(1.)).bg(t.line)).child(heading(
                t,
                "Undo send",
                "The engine holds outgoing mail for this window before sending.",
            ));
            content = content.child(div().flex().flex_wrap().gap_2().children(
                [0, 5, 10, 20, 30].into_iter().map(|seconds| {
                    button(
                        format!("undo-send-{seconds}"),
                        &format!("{seconds} seconds"),
                        t,
                    )
                    .aria_selected(kite.settings.undo_send_secs == seconds)
                    .when(kite.settings.undo_send_secs == seconds, |d| {
                        d.bg(t.sel).border_color(t.accent)
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let mut settings = this.settings.clone();
                        settings.undo_send_secs = seconds;
                        this.save_preferences(settings, window, cx);
                    }))
                }),
            ));
            content = content.child(div().p_5().rounded(px(RADIUS)).bg(t.bg2).flex().flex_col().gap_2().child(div().text_size(t.px(13.)).font_weight(FontWeight(650.)).child("Private by default")).child(div().text_size(t.px(12.)).text_color(t.fg2).child("Email scripts and remote images stay blocked. No tracking pixels or read receipts. Account secrets belong in the OS keychain, never in a theme or settings file.")));
        }
    }
    let mut root = div()
        .id("settings-page")
        .test_support()
        .aria_label("Settings")
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .child(
            div()
                .id("settings-scroll")
                .test_support()
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .track_scroll(&page.scroll)
                .px(if narrow { px(24.) } else { px(48.) })
                .py_8()
                .flex()
                .justify_center()
                .items_start()
                .child(content.pb_8()),
        );
    if let Some(error) = &page.error {
        root = root.child(
            div()
                .id("settings-error")
                .test_support()
                .aria_label(error.clone())
                .flex_shrink_0()
                .px_6()
                .py_3()
                .bg(t.bg2)
                .border_t_1()
                .border_color(t.line)
                .text_size(t.px(12.))
                .text_color(t.danger)
                .child(error.clone()),
        );
    } else if let Some(notice) = &page.notice {
        root = root.child(
            div()
                .id("settings-notice")
                .test_support()
                .aria_label(notice.clone())
                .flex_shrink_0()
                .px_6()
                .py_3()
                .bg(t.bg2)
                .border_t_1()
                .border_color(t.line)
                .text_size(t.px(12.))
                .text_color(t.ok)
                .child(notice.clone()),
        );
    }
    root.into_any_element()
}

/// Local shell storage until the engine owns preferences. Credentials are never included.
pub fn read_settings(path: &std::path::Path) -> anyhow::Result<Settings> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
        Err(error) => Err(error.into()),
    }
}

pub fn write_settings(path: &std::path::Path, settings: &Settings) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(settings)?;
    std::fs::write(&temp, bytes)?;
    std::fs::rename(temp, path)?;
    Ok(())
}
