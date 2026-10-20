//! Settings: a navigation rail, grouped cards and account setup.
//!
//! Preferences stay behind the Mail boundary; provider work never runs on the UI thread.
//! Each page owns its own file and state so a page can change without touching the shell.
pub mod accounts;
pub mod ai;
pub mod appearance;
pub mod connections;
pub mod general;
pub mod privacy;
pub mod shortcuts;
pub mod typography;

use crate::app::{Kite, Overlay};
use crate::theme::{self, RADIUS, RADIUS_SM, Theme};
use gpui_kit::assets::IconName;
use gpui_kit::component::{Icon, Sizable};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kite_core::model::Settings;

/// Rail indices, shared with the shortcuts and the sidebar so they cannot drift.
pub const OPEN_APPEARANCE: usize = 0;
pub const OPEN_ACCOUNTS: usize = 2;
pub const OPEN_CONNECTIONS: usize = 3;
pub const OPEN_SHORTCUTS: usize = 7;

pub struct Preferences {
    pub tab: usize,
    pub appearance: appearance::State,
    pub typography: typography::State,
    pub accounts: accounts::State,
    pub connections: connections::State,
    pub general: general::State,
    pub privacy: privacy::State,
    pub shortcuts: shortcuts::State,
    pub ai: ai::State,
    pub error: Option<String>,
    pub notice: Option<String>,
    /// Identifies this page instance so a slow account request cannot update a newer one.
    pub request: u64,
    pub scroll: ScrollHandle,
}

pub const TEMPLATE: &str = "{\n  \"name\": \"My midnight\",\n  \"base\": \"dark\",\n  \"colors\": {\n    \"bg\": \"#101820\",\n    \"bg2\": \"#16212b\",\n    \"accent\": \"#77c5dc\"\n  }\n}";

/// `(index, rail label, page subtitle, icon)`. The index is the id the tests and shortcuts use.
fn tabs() -> [(usize, &'static str, &'static str, IconName); 8] {
    [
        (0, "Appearance", "Colours and palette", IconName::Palette),
        (
            1,
            "Typography",
            "Type, size and colour",
            IconName::ALargeSmall,
        ),
        (
            2,
            "Accounts",
            "Mailboxes connected to Kite",
            IconName::CircleUser,
        ),
        (3, "Connections", "Proton and Outlook", IconName::Network),
        (4, "General", "Everyday defaults", IconName::Settings2),
        (5, "Privacy", "Remote images and tracking", IconName::EyeOff),
        (6, "AI", "Writing and summaries", IconName::Bot),
        (7, "Shortcuts", "Keys for every command", IconName::Keyboard),
    ]
}

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
            let request = self.seq;
            let settings = self.settings.clone();
            self.preferences = Some(Preferences {
                tab,
                appearance: appearance::State,
                typography: typography::State::new(&settings, window, cx),
                accounts: accounts::State::default(),
                connections: connections::State::new(window, cx),
                general: general::State,
                privacy: privacy::State,
                shortcuts: shortcuts::State::default(),
                ai: ai::State::new(&settings, window, cx),
                error: None,
                notice: None,
                request,
                scroll: ScrollHandle::new(),
            });
        }
        self.sidebar_dirty = true;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// Applies a settings change everywhere it is visible, or reports why it could not.
    pub(crate) fn save_preferences(
        &mut self,
        settings: Settings,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
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
}

/// A quiet section heading. Cards follow as siblings so a gallery can be its own surface.
pub(crate) fn heading(t: Theme, title: &str, description: &str) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .pb_3()
        .child(
            div()
                .text_size(t.px(13.))
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

/// A grouped card: rows separated by hairlines on a raised surface.
pub(crate) fn card(t: Theme, rows: Vec<AnyElement>) -> Div {
    card_with(t, rows, true)
}

/// A denser card for key tables, where a row is one line and does not need the full padding.
pub(crate) fn card_dense(t: Theme, rows: Vec<AnyElement>) -> Div {
    card_with(t, rows, false)
}

fn card_with(t: Theme, rows: Vec<AnyElement>, roomy: bool) -> Div {
    let count = rows.len();
    let mut body = div().flex().flex_col();
    for (ix, row) in rows.into_iter().enumerate() {
        body = body.child(
            div()
                .w_full()
                .when(roomy, |d| d.py_4())
                .when(!roomy, |d| d.py_2())
                .when(ix + 1 < count, |d| d.border_b_1().border_color(t.line))
                .child(row),
        );
    }
    div()
        .rounded(px(RADIUS))
        .border_1()
        .border_color(t.line)
        .bg(t.bg2)
        .px_4()
        .child(body)
}

/// One labelled control inside a card.
pub(crate) fn row(
    t: Theme,
    title: &str,
    description: &str,
    control: impl IntoElement,
) -> AnyElement {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap_4()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_1()
                .child(div().text_size(t.px(13.)).child(title.to_string()))
                .when(!description.is_empty(), |d| {
                    d.child(
                        div()
                            .text_size(t.px(12.))
                            .text_color(t.fg2)
                            .child(description.to_string()),
                    )
                }),
        )
        .child(div().flex_shrink_0().child(control))
        .into_any_element()
}

pub(crate) fn button(
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

pub fn render(kite: &Kite, window: &Window, cx: &mut Context<Kite>) -> AnyElement {
    let Some(page) = &kite.preferences else {
        return div().into_any_element();
    };
    let t = kite.theme;
    let compact = window.viewport_size().width < px(1000.);
    let rail = if compact { px(56.) } else { px(200.) };
    let (_, title, subtitle, _) = tabs()[page.tab.min(7)];
    let inset = if compact { px(24.) } else { px(40.) };

    let body = match page.tab {
        1 => typography::render(kite, window, cx),
        2 => accounts::render(kite, window, cx),
        3 => connections::render(kite, window, cx),
        4 => general::render(kite, window, cx),
        5 => privacy::render(kite, window, cx),
        6 => ai::render(kite, window, cx),
        7 => shortcuts::render(kite, window, cx),
        _ => appearance::render(kite, window, cx),
    };

    let mut content = div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .child(
            div()
                .flex_shrink_0()
                .px(inset)
                .pt_6()
                .pb_4()
                .flex()
                .items_start()
                .justify_between()
                .gap_4()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .min_w_0()
                        .child(
                            div()
                                .text_size(t.px(22.))
                                .font_weight(FontWeight(650.))
                                .child(title),
                        )
                        .child(div().text_size(t.px(12.)).text_color(t.fg2).child(subtitle)),
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
                .id("settings-scroll")
                .test_support()
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .track_scroll(&page.scroll)
                .px(inset)
                .pb_8()
                .flex()
                .justify_center()
                .items_start()
                .child(body),
        );

    if let Some(error) = &page.error {
        content = content.child(
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
        content = content.child(
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

    div()
        .id("settings-page")
        .test_support()
        .aria_label("Settings")
        .flex_1()
        .min_h_0()
        .flex()
        .child(
            div()
                .id("settings-nav")
                .test_support()
                .aria_label("Settings sections")
                .w(rail)
                .h_full()
                .flex_shrink_0()
                .bg(t.bg2)
                .border_r_1()
                .border_color(t.line)
                .px(if compact { px(8.) } else { px(12.) })
                .py_4()
                .flex()
                .flex_col()
                .gap_1()
                .when(!compact, |d| {
                    d.child(
                        div()
                            .px_2()
                            .pb_3()
                            .text_size(t.px(11.))
                            .font_weight(FontWeight(650.))
                            .text_color(t.fg3)
                            .child("SETTINGS"),
                    )
                })
                .children(tabs().into_iter().map(|(index, label, _, icon)| {
                    let active = page.tab == index;
                    div()
                        .id(("settings-tab", index))
                        .test_support()
                        .aria_label(label)
                        .aria_selected(active)
                        .h(px(34.))
                        .flex_shrink_0()
                        .rounded(px(RADIUS_SM))
                        .px_2()
                        .flex()
                        .items_center()
                        .gap_3()
                        .cursor_pointer()
                        .when(compact, |d| d.justify_center())
                        .when(active, |d| d.bg(t.sel))
                        .text_color(if active { t.fg } else { t.fg2 })
                        .hover(|s| s.bg(t.bg3))
                        .child(Icon::new(icon).with_size(px(16.)).text_color(if active {
                            t.accent
                        } else {
                            t.fg3
                        }))
                        .when(!compact, |d| {
                            d.child(
                                div()
                                    .text_size(t.px(13.))
                                    .when(active, |d| d.font_weight(FontWeight(650.)))
                                    .child(label),
                            )
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(page) = &mut this.preferences {
                                page.tab = index;
                                page.error = None;
                                page.notice = None;
                                page.scroll = ScrollHandle::new();
                            }
                            cx.notify();
                        }))
                })),
        )
        .child(content)
        .into_any_element()
}
