//! The account rail, the account switcher and the mailbox picker.
use crate::app::Kite;
use crate::theme::{self, RADIUS, RADIUS_SM, Theme};
use gpui_kit::assets::IconName;
use gpui_kit::component::{Icon, Sizable};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kite_core::model::{Account, Folder, Id, Mailbox, View};
use std::cell::Cell;

/// The icon rail: accounts down the left, window actions at the bottom.
pub const RAIL_W: f32 = 64.;

pub const MAILBOXES: [(Mailbox, &str, IconName); 9] = [
    (Mailbox::Inbox, "Inbox", IconName::Inbox),
    (Mailbox::Drafts, "Drafts", IconName::FileText),
    (Mailbox::Sent, "Sent", IconName::Send),
    (Mailbox::Starred, "Starred", IconName::Star),
    (Mailbox::Snoozed, "Snoozed", IconName::Clock),
    (Mailbox::Archive, "Archive", IconName::Archive),
    (Mailbox::Spam, "Spam", IconName::ShieldAlert),
    (Mailbox::Trash, "Trash", IconName::Trash),
    (Mailbox::All, "All mail", IconName::Mails),
];

thread_local! {
    /// Whether a conversation is read beside the list. Set once per frame by the root view, so
    /// the thread view and compose size themselves to the space that is really theirs.
    static PANE: Cell<bool> = const { Cell::new(true) };
}

/// The list column: wide enough for a sender, subject and time, narrower on small windows.
pub fn list_width(window: &Window) -> f32 {
    if window.viewport_size().width < px(1000.) {
        320.
    } else {
        408.
    }
}

/// Everything left of the reading pane: the rail, plus the list column while it is shown.
pub fn width(window: &Window) -> f32 {
    RAIL_W
        + if PANE.with(|pane| pane.get()) {
            list_width(window)
        } else {
            0.
        }
}

pub(crate) fn set_pane(open: bool) {
    PANE.with(|pane| pane.set(open));
}

pub fn mailbox_name(mailbox: Mailbox) -> &'static str {
    MAILBOXES
        .iter()
        .find(|(value, _, _)| *value == mailbox)
        .map(|(_, name, _)| *name)
        .unwrap_or("Mail")
}

/// Unread inbox conversations per account, for the rail badges.
pub(crate) fn unread_counts(
    mail: &dyn kite_core::api::Mail,
    accounts: &[Account],
) -> Vec<(Id, usize)> {
    accounts
        .iter()
        .map(|account| {
            let view = View {
                account_id: Some(account.id),
                mailbox: Mailbox::Inbox,
                split: None,
                query: None,
                folder: None,
            };
            let rows = mail.threads(&view, 0, 200).unwrap_or_default();
            (account.id, rows.iter().filter(|row| row.unread).count())
        })
        .collect()
}

impl Kite {
    pub(crate) fn select_mailbox(
        &mut self,
        mailbox: Mailbox,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut view = self.view.clone();
        view.mailbox = mailbox;
        view.split = None;
        view.folder = None;
        view.query = None;
        self.preferences = None;
        self.sidebar_dirty = true;
        self.open = None;
        self.search
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.set_view(view, cx);
        window.focus(&self.focus, cx);
    }

    pub(crate) fn select_account(
        &mut self,
        account: Option<Id>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut view = self.view.clone();
        view.account_id = account;
        if view.folder.is_some() || account.is_none() {
            view.mailbox = Mailbox::Inbox;
            view.split = None;
            view.query = None;
            self.search
                .update(cx, |input, cx| input.set_value("", window, cx));
        }
        view.folder = None;
        self.preferences = None;
        self.sidebar_dirty = true;
        self.open = None;
        self.set_view(view, cx);
        window.focus(&self.focus, cx);
    }
}

/// Account scope does not change when the list selection moves, so it stays a cached view.
pub(crate) struct Sidebar {
    accounts: Vec<Account>,
    unread: Vec<(Id, usize)>,
    view: View,
    theme: Theme,
    parent: WeakEntity<Kite>,
    settings_open: bool,
}

impl Sidebar {
    pub(crate) fn new(kite: &Kite, parent: WeakEntity<Kite>) -> Self {
        // The root view refreshes these with the account list; a fresh account is counted here.
        let unread = if kite.unread.len() == kite.accounts.len() {
            kite.unread.clone()
        } else {
            unread_counts(kite.mail.as_ref(), &kite.accounts)
        };
        Self {
            accounts: kite.accounts.clone(),
            unread,
            view: kite.view.clone(),
            theme: kite.theme,
            parent,
            settings_open: kite.preferences.is_some(),
        }
    }

    fn unread(&self, account: Id) -> usize {
        self.unread
            .iter()
            .find(|(id, _)| *id == account)
            .map(|(_, count)| *count)
            .unwrap_or(0)
    }
}

impl Render for Sidebar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let combined = self.view.account_id.is_none();
        div()
            .id("sidebar")
            .test_support()
            .aria_label("Mail navigation")
            .w(px(RAIL_W))
            .h_full()
            .flex_shrink_0()
            .min_h_0()
            .bg(theme::rail(&t))
            .border_r_1()
            .border_color(t.line)
            .flex()
            .flex_col()
            .items_center()
            .gap_2()
            // macOS floats its traffic lights over the top-left, which is the rail.
            .when(cfg!(target_os = "macos"), |d| d.pt(px(28.)))
            .when(!cfg!(target_os = "macos"), |d| d.pt_3())
            .pb_3()
            .child(
                rail_button(
                    &t,
                    "all-inboxes",
                    icon(IconName::GalleryVerticalEnd),
                    "All inboxes",
                    combined,
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.parent
                        .update(cx, |kite, cx| kite.select_account(None, window, cx))
                        .ok();
                })),
            )
            .child(
                div()
                    .id("rail-accounts")
                    .test_support()
                    .aria_label("Accounts")
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_2()
                    .overflow_y_scroll()
                    .children(self.accounts.iter().enumerate().map(|(index, account)| {
                        let id = account.id;
                        let selected = !combined && self.view.account_id == Some(id);
                        let unread = self.unread(id);
                        div()
                            .id(("rail-account", id as usize))
                            .test_support()
                            .aria_label(account.email.clone())
                            .aria_selected(selected)
                            .relative()
                            .flex_shrink_0()
                            .size(px(40.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(RADIUS))
                            .cursor_pointer()
                            .when(selected, |d| d.bg(Theme::alpha(t.fg, 0.1)))
                            .when(!selected, |d| d.hover(|s| s.bg(Theme::alpha(t.fg, 0.06))))
                            .child(avatar(&account.name, &account.email, index, &t))
                            .when(unread > 0, |d| {
                                d.child(
                                    div()
                                        .id(("rail-unread", id as usize))
                                        .test_support()
                                        .aria_label(unread.to_string())
                                        .absolute()
                                        .top_0()
                                        .right_0()
                                        .min_w(px(17.))
                                        .h(px(17.))
                                        .px(px(4.))
                                        .rounded_full()
                                        .bg(t.accent)
                                        .text_color(t.bg)
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .text_size(t.px(10.))
                                        .font_weight(FontWeight(650.))
                                        .child(if unread > 99 {
                                            "99+".to_string()
                                        } else {
                                            unread.to_string()
                                        }),
                                )
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.parent
                                    .update(cx, |kite, cx| {
                                        kite.select_account(Some(id), window, cx)
                                    })
                                    .ok();
                            }))
                    })),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_1()
                    .child(
                        rail_button(
                            &t,
                            "theme-toggle",
                            if t.dark {
                                IconName::Sun
                            } else {
                                IconName::Moon
                            },
                            if t.dark { "Light mode" } else { "Dark mode" },
                            false,
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.parent
                                .update(cx, |kite, cx| kite.toggle_theme(window, cx))
                                .ok();
                        })),
                    )
                    .child(
                        rail_button(
                            &t,
                            "shortcut-help",
                            help_glyph(&t),
                            "Keyboard shortcuts",
                            false,
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.parent
                                .update(cx, |kite, cx| kite.run(crate::keys::cmd::HELP, window, cx))
                                .ok();
                        })),
                    )
                    .child(
                        rail_button(
                            &t,
                            "open-settings",
                            IconName::Settings,
                            "Settings",
                            self.settings_open,
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.parent
                                .update(cx, |kite, cx| kite.toggle_settings(window, cx))
                                .ok();
                        })),
                    )
                    .child(
                        rail_button(&t, "add-account", IconName::Plus, "Add account", false)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.parent
                                    .update(cx, |kite, cx| {
                                        kite.show_preferences(
                                            crate::preferences::OPEN_CONNECTIONS,
                                            window,
                                            cx,
                                        )
                                    })
                                    .ok();
                            })),
                    ),
            )
            .into_any_element()
    }
}

fn icon(name: IconName) -> AnyElement {
    Icon::new(name).with_size(px(18.)).into_any_element()
}

/// The cheat sheet is opened by `?`, so the button shows the key rather than an icon.
fn help_glyph(t: &Theme) -> AnyElement {
    div()
        .text_size(t.px(15.))
        .font_weight(FontWeight(650.))
        .child("?")
        .into_any_element()
}

fn rail_button(
    t: &Theme,
    id: impl Into<ElementId>,
    glyph: impl IntoElement,
    label: &'static str,
    selected: bool,
) -> gpui_kit::base::ObservedElement<Stateful<Div>> {
    let t = *t;
    div()
        .id(id)
        .test_support()
        .aria_label(label)
        .aria_selected(selected)
        .flex_shrink_0()
        .size(px(40.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(RADIUS))
        .cursor_pointer()
        .text_color(if selected { t.fg } else { t.fg2 })
        .when(selected, |d| d.bg(Theme::alpha(t.fg, 0.1)))
        .when(!selected, |d| {
            d.hover(|s| s.bg(Theme::alpha(t.fg, 0.06)).text_color(t.fg))
        })
        .child(glyph)
}

/// Initials on an account's own colour.
fn avatar(name: &str, email: &str, index: usize, t: &Theme) -> Div {
    let source = if name.trim().is_empty() { email } else { name };
    let initials: String = source
        .split([' ', '.', '_', '-'])
        .filter_map(|part| part.chars().next())
        .take(2)
        .collect::<String>()
        .to_uppercase();
    let color = [t.accent, t.accent2, t.ok, t.danger][index % 4];
    div()
        .size(px(28.))
        .flex_shrink_0()
        .rounded_full()
        .bg(color)
        .text_color(t.bg)
        .flex()
        .items_center()
        .justify_center()
        .text_size(t.px(11.))
        .font_weight(FontWeight(650.))
        .child(initials)
}

/// One row of the mailbox picker.
#[derive(Clone)]
pub enum Pick {
    Mailbox(Mailbox),
    Folder(Folder),
}

/// Every destination the picker offers, in row order. The picker and its keyboard use this list.
pub fn picker_targets(kite: &Kite) -> Vec<Pick> {
    let mut targets: Vec<Pick> = MAILBOXES
        .iter()
        .map(|(mailbox, _, _)| Pick::Mailbox(*mailbox))
        .collect();
    targets.extend(
        kite.folders
            .iter()
            .filter(|folder| {
                kite.view
                    .account_id
                    .is_none_or(|id| id == folder.account_id)
            })
            .cloned()
            .map(Pick::Folder),
    );
    targets
}

/// The mailbox picker behind the list header's ▾.
pub fn mailbox_menu(kite: &Kite, window: &Window, cx: &mut Context<Kite>) -> AnyElement {
    let t = kite.theme;
    let selected = if let crate::app::Overlay::Mailboxes(index) = kite.overlay {
        index
    } else {
        0
    };
    let mut panel = div()
        .id("mailbox-menu")
        .test_support()
        .aria_label("Mailboxes")
        .absolute()
        .left(px(RAIL_W + 8.))
        .top(px(crate::theme::STRIP_H + 38.))
        .w(px(268.))
        .max_h(window.viewport_size().height - px(120.))
        .overflow_y_scroll()
        .p_2()
        .bg(t.bg2)
        .border_1()
        .border_color(t.line)
        .rounded(px(RADIUS))
        .shadow_lg()
        .occlude()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
    for (index, target) in picker_targets(kite).into_iter().enumerate() {
        match target {
            Pick::Mailbox(mailbox) => {
                let Some((_, name, icon)) =
                    MAILBOXES.iter().find(|(value, _, _)| *value == mailbox)
                else {
                    continue;
                };
                let active = kite.view.folder.is_none() && kite.view.mailbox == mailbox;
                panel = panel.child(
                    picker_row(
                        &t,
                        ("mailbox", mailbox as usize),
                        Some(Icon::new(*icon).with_size(px(15.)).into_any_element()),
                        name,
                        "",
                        active,
                        selected == index,
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.overlay = crate::app::Overlay::None;
                        this.select_mailbox(mailbox, window, cx);
                    })),
                );
            }
            Pick::Folder(folder) => {
                let active = kite.view.account_id == Some(folder.account_id)
                    && kite.view.folder.as_ref() == Some(&folder.id);
                // A combined view names the owner, because folder ids are per account.
                let detail = if kite.view.account_id.is_none() {
                    kite.accounts
                        .iter()
                        .find(|account| account.id == folder.account_id)
                        .map(|account| account.email.clone())
                        .unwrap_or_default()
                } else {
                    String::new()
                };
                panel = panel.child(
                    picker_row(
                        &t,
                        format!("folder-{}-{}", folder.account_id, folder.id),
                        Some(
                            Icon::new(IconName::FolderClosed)
                                .with_size(px(15.))
                                .into_any_element(),
                        ),
                        &folder.name,
                        &detail,
                        active,
                        selected == index,
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.overlay = crate::app::Overlay::None;
                        this.select_folder(&folder, window, cx);
                    })),
                );
            }
        }
    }
    panel = panel.child(
        div()
            .id("new-folder")
            .test_support()
            .aria_label("New folder")
            .mt_1()
            .px_3()
            .py_2()
            .rounded(px(RADIUS_SM))
            .text_size(t.px(12.))
            .text_color(t.fg2)
            .cursor_pointer()
            .hover(|s| s.bg(t.bg3).text_color(t.fg))
            .child("New folder")
            .on_click(cx.listener(|this, _, window, cx| {
                this.overlay = crate::app::Overlay::None;
                this.show_create_folder(window, cx);
            })),
    );
    div()
        .absolute()
        .inset_0()
        .occlude()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, window, cx| {
                this.overlay = crate::app::Overlay::None;
                window.focus(&this.focus, cx);
                cx.notify();
            }),
        )
        .child(panel)
        .into_any_element()
}

fn picker_row(
    t: &Theme,
    id: impl Into<ElementId>,
    icon: Option<AnyElement>,
    name: &str,
    detail: &str,
    active: bool,
    selected: bool,
) -> gpui_kit::base::ObservedElement<Stateful<Div>> {
    let t = *t;
    div()
        .id(id)
        .test_support()
        .aria_label(name.to_string())
        .aria_selected(active)
        .flex()
        .items_center()
        .gap_3()
        .h(px(34.))
        .px_3()
        .flex_shrink_0()
        .rounded(px(RADIUS_SM))
        .cursor_pointer()
        .text_size(t.px(13.))
        .text_color(if active { t.fg } else { t.fg2 })
        .when(selected, |d| d.bg(t.sel))
        .when(!selected, |d| d.hover(|s| s.bg(t.bg3).text_color(t.fg)))
        .children(icon)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .when(active, |d| d.font_weight(FontWeight(650.)))
                .child(name.to_string()),
        )
        .when(!detail.is_empty(), |d| {
            d.child(
                div()
                    .truncate()
                    .text_size(t.px(11.))
                    .text_color(t.fg3)
                    .child(detail.to_string()),
            )
        })
        .when(active, |d| {
            d.child(
                Icon::new(IconName::Check)
                    .with_size(px(14.))
                    .text_color(t.accent),
            )
        })
}

pub fn account_menu(kite: &Kite, window: &Window, cx: &mut Context<Kite>) -> AnyElement {
    let t = kite.theme;
    let selected = if let crate::app::Overlay::Accounts(index) = kite.overlay {
        index
    } else {
        0
    };
    let mut panel = div()
        .id("account-menu")
        .test_support()
        .aria_label("Switch account")
        .absolute()
        .left(px(RAIL_W + 8.))
        .bottom(px(56.))
        .w(px(280.))
        .max_h(window.viewport_size().height - px(130.))
        .overflow_y_scroll()
        .p_2()
        .bg(t.bg2)
        .border_1()
        .border_color(t.line)
        .rounded(px(RADIUS))
        .shadow_lg()
        .occlude()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
    for index in 0..=kite.accounts.len() {
        let account = index.checked_sub(1).and_then(|i| kite.accounts.get(i));
        let id = account.map(|a| a.id);
        let name = account
            .map(|a| a.name.clone())
            .unwrap_or_else(|| "All inboxes".into());
        let detail = account
            .map(|a| a.email.clone())
            .unwrap_or_else(|| "Every account, together".into());
        panel = panel.child(
            div()
                .id(format!("account-{}", id.unwrap_or(0)))
                .test_support()
                .aria_label(
                    account
                        .map(|a| a.email.clone())
                        .unwrap_or_else(|| "All inboxes".into()),
                )
                .aria_selected(kite.view.account_id == id)
                .flex()
                .items_center()
                .gap_3()
                .px_3()
                .py_3()
                .rounded(px(RADIUS_SM))
                .cursor_pointer()
                .when(selected == index, |d| d.bg(t.sel))
                .hover(|s| s.bg(t.bg3))
                .child(avatar(&name, &detail, index.saturating_sub(1), &t))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(div().text_size(t.px(13.)).child(name))
                        .child(
                            div()
                                .truncate()
                                .text_size(t.px(11.))
                                .text_color(t.fg2)
                                .child(detail),
                        ),
                )
                .when(kite.view.account_id == id, |d| {
                    d.child(
                        Icon::new(IconName::Check)
                            .with_size(px(14.))
                            .text_color(t.accent),
                    )
                })
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.overlay = crate::app::Overlay::None;
                    this.select_account(id, window, cx);
                })),
        );
    }
    panel = panel.child(
        div()
            .id("manage-accounts")
            .test_support()
            .aria_label("Manage accounts")
            .px_3()
            .py_3()
            .text_size(t.px(12.))
            .text_color(t.fg2)
            .cursor_pointer()
            .hover(|s| s.text_color(t.fg))
            .child("Manage accounts")
            .on_click(cx.listener(|this, _, window, cx| {
                this.overlay = crate::app::Overlay::None;
                this.show_preferences(crate::preferences::OPEN_ACCOUNTS, window, cx);
            })),
    );
    div()
        .absolute()
        .inset_0()
        .occlude()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, window, cx| {
                this.overlay = crate::app::Overlay::None;
                window.focus(&this.focus, cx);
                cx.notify();
            }),
        )
        .child(panel)
        .into_any_element()
}
