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

/// A settings page. [`Page::ALL`] is the only place the rail order lives: a page's index is
/// its position there, so the rail, the shortcuts and every link cannot drift apart.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Page {
    General,
    Accounts,
    Connections,
    Appearance,
    Typography,
    Shortcuts,
    Privacy,
    Ai,
}

impl Page {
    /// Rail order. Reordering this list reorders the rail and moves every entry point.
    pub const ALL: [Page; 8] = [
        Page::General,
        Page::Accounts,
        Page::Connections,
        Page::Appearance,
        Page::Typography,
        Page::Shortcuts,
        Page::Privacy,
        Page::Ai,
    ];

    /// The rail index, which is also the id the tests and shortcuts use.
    pub const fn index(self) -> usize {
        let mut index = 0;
        while index < Self::ALL.len() {
            if Self::ALL[index] as u8 == self as u8 {
                return index;
            }
            index += 1;
        }
        unreachable!()
    }

    pub fn from_index(index: usize) -> Self {
        Self::ALL.get(index).copied().unwrap_or(Self::General)
    }

    fn label(self) -> &'static str {
        match self {
            Page::General => "General",
            Page::Accounts => "Accounts",
            Page::Connections => "Connections",
            Page::Appearance => "Appearance",
            Page::Typography => "Typography",
            Page::Shortcuts => "Shortcuts",
            Page::Privacy => "Privacy",
            Page::Ai => "AI",
        }
    }

    fn subtitle(self) -> &'static str {
        match self {
            Page::General => "Everyday defaults",
            Page::Accounts => "Mailboxes connected to Kite",
            Page::Connections => "Proton and Outlook",
            Page::Appearance => "Colours and palette",
            Page::Typography => "Type, size and colour",
            Page::Shortcuts => "Keys for every command",
            Page::Privacy => "Remote images and tracking",
            Page::Ai => "Writing and summaries",
        }
    }

    fn icon(self) -> IconName {
        match self {
            Page::General => IconName::Settings2,
            Page::Accounts => IconName::CircleUser,
            Page::Connections => IconName::Network,
            Page::Appearance => IconName::Palette,
            Page::Typography => IconName::ALargeSmall,
            Page::Shortcuts => IconName::Keyboard,
            Page::Privacy => IconName::EyeOff,
            Page::Ai => IconName::Bot,
        }
    }
}

/// Rail indices, shared with the shortcuts and the sidebar so they cannot drift.
pub const OPEN_GENERAL: usize = Page::General.index();
pub const OPEN_ACCOUNTS: usize = Page::Accounts.index();
pub const OPEN_CONNECTIONS: usize = Page::Connections.index();
pub const OPEN_SHORTCUTS: usize = Page::Shortcuts.index();

const RAIL_WIDE: f32 = 200.;
const RAIL_COMPACT: f32 = 56.;
const INSET_WIDE: f32 = 40.;
const INSET_COMPACT: f32 = 24.;
/// The widest a settings form gets; galleries size themselves against it.
pub(crate) const CONTENT_MAX: f32 = 720.;

/// Whether the window uses the icon rail and the narrower page padding.
pub(crate) fn compact(window: &Window) -> bool {
    window.viewport_size().width < px(1000.)
}

/// Width a page's column actually gets: the window minus the rail and both insets, capped.
pub(crate) fn content_width(window: &Window) -> f32 {
    let (rail, inset) = if compact(window) {
        (RAIL_COMPACT, INSET_COMPACT)
    } else {
        (RAIL_WIDE, INSET_WIDE)
    };
    (window.viewport_size().width.as_f32() - rail - inset * 2.).min(CONTENT_MAX)
}

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
                accounts: accounts::State::new(self, window, cx),
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
    let compact = compact(window);
    let rail = if compact { px(56.) } else { px(200.) };
    let current = Page::from_index(page.tab);
    let inset = if compact { px(24.) } else { px(40.) };

    let body = match current {
        Page::General => general::render(kite, window, cx),
        Page::Accounts => accounts::render(kite, window, cx),
        Page::Connections => connections::render(kite, window, cx),
        Page::Appearance => appearance::render(kite, window, cx),
        Page::Typography => typography::render(kite, window, cx),
        Page::Shortcuts => shortcuts::render(kite, window, cx),
        Page::Privacy => privacy::render(kite, window, cx),
        Page::Ai => ai::render(kite, window, cx),
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
                                .child(current.label()),
                        )
                        .child(
                            div()
                                .text_size(t.px(12.))
                                .text_color(t.fg2)
                                .child(current.subtitle()),
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
                .children(Page::ALL.into_iter().enumerate().map(|(index, tab)| {
                    let active = page.tab == index;
                    div()
                        .id(("settings-tab", index))
                        .test_support()
                        .aria_label(tab.label())
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
                        .child(
                            Icon::new(tab.icon())
                                .with_size(px(16.))
                                .text_color(if active { t.accent } else { t.fg3 }),
                        )
                        .when(!compact, |d| {
                            d.child(
                                div()
                                    .text_size(t.px(13.))
                                    .when(active, |d| d.font_weight(FontWeight(650.)))
                                    .child(tab.label()),
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
