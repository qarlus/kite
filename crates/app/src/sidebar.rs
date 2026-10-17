//! Persistent account scope and mailbox navigation.
use crate::app::Kite;
use crate::theme::Theme;
use gpui_kit::assets::IconName;
use gpui_kit::component::{Icon, Sizable};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kite_core::model::{Account, Folder, Id, Mailbox, View};

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

pub fn width(window: &Window) -> f32 {
    if window.viewport_size().width < px(900.) {
        180.
    } else {
        216.
    }
}

pub fn mailbox_name(mailbox: Mailbox) -> &'static str {
    MAILBOXES
        .iter()
        .find(|(value, _, _)| *value == mailbox)
        .map(|(_, name, _)| *name)
        .unwrap_or("Mail")
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

/// Account and folder navigation does not change when the list selection moves.
pub(crate) struct Sidebar {
    accounts: Vec<Account>,
    folders: Vec<Folder>,
    view: View,
    theme: Theme,
    parent: WeakEntity<Kite>,
    settings_open: bool,
}

impl Sidebar {
    pub(crate) fn new(kite: &Kite, parent: WeakEntity<Kite>) -> Self {
        Self {
            accounts: kite.accounts.clone(),
            folders: kite.folders.clone(),
            view: kite.view.clone(),
            theme: kite.theme,
            parent,
            settings_open: kite.preferences.is_some(),
        }
    }
}

impl Render for Sidebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        div()
            .id("sidebar")
            .test_support()
            .aria_label("Mail navigation")
            .w(px(width(window)))
            .h_full()
            .flex_shrink_0()
            .min_h_0()
            .bg(t.bg2)
            .border_r_1()
            .border_color(t.line)
            .flex()
            .flex_col()
            .px_3()
            .py_3()
            .gap_0()
            .child(
                div()
                    .id("sidebar-scroll")
                    .test_support()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .pt_2()
                    .child(
                        div()
                            .px_3()
                            .pb_3()
                            .text_size(t.px(10.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(t.fg3)
                            .child("MAIL"),
                    )
                    .children(MAILBOXES.iter().map(|(mailbox, name, icon)| {
                        let mailbox = *mailbox;
                        let selected = !self.settings_open
                            && self.view.mailbox == mailbox
                            && self.view.folder.is_none();
                        div()
                            .id(("mailbox", mailbox as usize))
                            .test_support()
                            .aria_label(*name)
                            .aria_selected(selected)
                            .flex()
                            .items_center()
                            .gap_3()
                            .h(px(36.))
                            .px_3()
                            .flex_shrink_0()
                            .rounded(px(7.))
                            .cursor_pointer()
                            .text_size(t.px(13.))
                            .text_color(if selected { t.fg } else { t.fg2 })
                            .when(selected, |d| d.bg(t.sel).font_weight(FontWeight::SEMIBOLD))
                            .when(!selected, |d| d.hover(|s| s.bg(t.bg3).text_color(t.fg)))
                            .child(Icon::new(*icon).with_size(px(16.)))
                            .child(*name)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.parent
                                    .update(cx, |kite, cx| kite.select_mailbox(mailbox, window, cx))
                                    .ok();
                            }))
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .mt_4()
                            .px_3()
                            .py_2()
                            .child(
                                div()
                                    .text_size(t.px(12.))
                                    .text_color(t.fg2)
                                    .child("Folders"),
                            )
                            .when(!self.accounts.is_empty(), |d| {
                                d.child(
                                    div()
                                        .id("new-folder")
                                        .test_support()
                                        .aria_label("New folder")
                                        .text_size(t.px(12.))
                                        .text_color(t.fg2)
                                        .cursor_pointer()
                                        .hover(|s| s.text_color(t.fg))
                                        .child("New folder")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.parent
                                                .update(cx, |kite, cx| {
                                                    kite.show_create_folder(window, cx)
                                                })
                                                .ok();
                                        })),
                                )
                            }),
                    )
                    .children(
                        self.folders
                            .iter()
                            .filter(|folder| {
                                self.view
                                    .account_id
                                    .is_none_or(|id| id == folder.account_id)
                            })
                            .map(|folder| {
                                let selected = !self.settings_open
                                    && self.view.account_id == Some(folder.account_id)
                                    && self.view.folder.as_ref() == Some(&folder.id);
                                let folder = folder.clone();
                                let detail = self
                                    .accounts
                                    .iter()
                                    .find(|account| account.id == folder.account_id)
                                    .map(|account| account.email.clone())
                                    .unwrap_or_default();
                                div()
                                    .id(format!("folder-{}-{}", folder.account_id, folder.id))
                                    .test_support()
                                    .aria_label(format!("{} ({detail})", folder.name))
                                    .aria_selected(selected)
                                    .flex()
                                    .flex_col()
                                    .px_3()
                                    .py_2()
                                    .rounded(px(7.))
                                    .cursor_pointer()
                                    .when(selected, |d| d.bg(t.sel))
                                    .when(!selected, |d| d.hover(|s| s.bg(t.bg3)))
                                    .child(
                                        div()
                                            .truncate()
                                            .text_size(t.px(13.))
                                            .child(folder.name.clone()),
                                    )
                                    .when(self.view.account_id.is_none(), |d| {
                                        d.child(
                                            div()
                                                .truncate()
                                                .text_size(t.px(11.))
                                                .text_color(t.fg2)
                                                .child(detail),
                                        )
                                    })
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.parent
                                            .update(cx, |kite, cx| {
                                                kite.select_folder(&folder, window, cx)
                                            })
                                            .ok();
                                    }))
                            }),
                    ),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .pt_3()
                    .mt_2()
                    .border_t_1()
                    .border_color(t.line)
                    .child(
                        div()
                            .id("open-settings")
                            .test_support()
                            .aria_label("Settings")
                            .aria_selected(self.settings_open)
                            .flex()
                            .items_center()
                            .gap_3()
                            .px_3()
                            .h(px(36.))
                            .rounded(px(7.))
                            .cursor_pointer()
                            .text_size(t.px(13.))
                            .text_color(t.fg2)
                            .when(self.settings_open, |d| d.bg(t.sel).text_color(t.fg))
                            .hover(|s| s.bg(t.bg3))
                            .child(Icon::new(IconName::Settings).with_size(px(16.)))
                            .child("Settings")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.parent
                                    .update(cx, |kite, cx| kite.show_preferences(0, window, cx))
                                    .ok();
                            })),
                    )
                    .child(self.switcher(cx)),
            )
            .into_any_element()
    }
}

impl Sidebar {
    fn switcher(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme;
        let account = self
            .accounts
            .iter()
            .find(|a| Some(a.id) == self.view.account_id);
        let title = account
            .map(|a| a.name.as_str())
            .filter(|name| !name.is_empty())
            .unwrap_or(if self.accounts.is_empty() {
                "Your workspace"
            } else {
                "All inboxes"
            });
        let detail = account.map(|a| a.email.clone()).unwrap_or_else(|| {
            self.accounts
                .first()
                .map(|a| {
                    if self.accounts.len() > 1 {
                        format!("{} +{}", a.email, self.accounts.len() - 1)
                    } else {
                        a.email.clone()
                    }
                })
                .unwrap_or_else(|| "Add an account".into())
        });
        div()
            .id("account-switcher")
            .test_support()
            .aria_label(format!("Switch account: {detail}"))
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .py_3()
            .mt_1()
            .rounded(px(8.))
            .cursor_pointer()
            .hover(|s| s.bg(t.bg3))
            .child(if account.is_none() && self.accounts.len() > 1 {
                div()
                    .w(px(36.))
                    .h(px(30.))
                    .flex_shrink_0()
                    .relative()
                    .child(
                        avatar(&self.accounts[0].name, t)
                            .absolute()
                            .left_0()
                            .top_0()
                            .size(px(25.))
                            .border_2()
                            .border_color(t.bg2),
                    )
                    .child(
                        avatar(&self.accounts[1].name, t)
                            .absolute()
                            .left(px(11.))
                            .top(px(5.))
                            .size(px(25.))
                            .bg(t.bg3)
                            .text_color(t.fg2)
                            .border_2()
                            .border_color(t.bg2),
                    )
                    .into_any_element()
            } else {
                avatar(title, t).into_any_element()
            })
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
                            .text_size(t.px(12.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title.to_string()),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(t.px(10.))
                            .text_color(t.fg2)
                            .child(detail),
                    ),
            )
            .child(
                Icon::new(IconName::ChevronsUpDown)
                    .with_size(px(13.))
                    .text_color(t.fg3),
            )
            .on_click(cx.listener(|this, _, window, cx| {
                this.parent
                    .update(cx, |kite, cx| {
                        if kite.accounts.is_empty() {
                            kite.show_preferences(1, window, cx);
                        } else {
                            kite.run(crate::keys::cmd::ACCOUNT_SWITCHER, window, cx);
                        }
                    })
                    .ok();
            }))
            .into_any_element()
    }
}

fn avatar(name: &str, theme: Theme) -> Div {
    let initials: String = name
        .split_whitespace()
        .filter_map(|part| part.chars().next())
        .take(2)
        .collect::<String>()
        .to_uppercase();
    div()
        .size(px(30.))
        .flex_shrink_0()
        .rounded_full()
        .bg(theme.sel)
        .text_color(theme.accent)
        .flex()
        .items_center()
        .justify_center()
        .text_size(theme.px(11.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(initials)
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
        .left(px(12.))
        .bottom(px(
            if kite.settings.show_shortcuts && kite.preferences.is_none() {
                104.
            } else {
                66.
            },
        ))
        .w(px(280.))
        .max_h(window.viewport_size().height - px(130.))
        .overflow_y_scroll()
        .p_2()
        .bg(t.bg2)
        .border_1()
        .border_color(t.line)
        .rounded(px(12.))
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
                .rounded(px(7.))
                .cursor_pointer()
                .when(selected == index, |d| d.bg(t.sel))
                .hover(|s| s.bg(t.bg3))
                .child(avatar(&name, t))
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
                this.show_preferences(1, window, cx);
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
