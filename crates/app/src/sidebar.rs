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
    theme: &'static Theme,
    parent: WeakEntity<Kite>,
}

impl Sidebar {
    pub(crate) fn new(kite: &Kite, parent: WeakEntity<Kite>) -> Self {
        Self {
            accounts: kite.accounts.clone(),
            folders: kite.folders.clone(),
            view: kite.view.clone(),
            theme: kite.theme,
            parent,
        }
    }
}

impl Render for Sidebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let mut accounts =
            vec![self.account_item(None, "All inboxes", "Every account, one place", cx)];
        for account in &self.accounts {
            let provider = match account.provider {
                kite_core::model::Provider::Proton => "Proton Mail",
                kite_core::model::Provider::Microsoft => "Outlook",
            };
            accounts.push(self.account_item(Some(account.id), provider, &account.email, cx));
        }
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
            .py_5()
            .gap_0()
            .overflow_y_scroll()
            .children(accounts)
            .child(div().h(px(1.)).bg(t.line).mx_2().my_4())
            .children(MAILBOXES.iter().map(|(mailbox, name, icon)| {
                let mailbox = *mailbox;
                let selected = self.view.mailbox == mailbox && self.view.folder.is_none();
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
                    .text_size(px(13.))
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
                    .child(div().text_size(px(12.)).text_color(t.fg2).child("Folders"))
                    .when(!self.accounts.is_empty(), |d| {
                        d.child(
                            div()
                                .id("new-folder")
                                .test_support()
                                .aria_label("New folder")
                                .text_size(px(12.))
                                .text_color(t.fg2)
                                .cursor_pointer()
                                .hover(|s| s.text_color(t.fg))
                                .child("New folder")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.parent
                                        .update(cx, |kite, cx| kite.show_create_folder(window, cx))
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
                        let selected = self.view.account_id == Some(folder.account_id)
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
                                    .text_size(px(13.))
                                    .child(folder.name.clone()),
                            )
                            .when(self.view.account_id.is_none(), |d| {
                                d.child(
                                    div()
                                        .truncate()
                                        .text_size(px(11.))
                                        .text_color(t.fg2)
                                        .child(detail),
                                )
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.parent
                                    .update(cx, |kite, cx| kite.select_folder(&folder, window, cx))
                                    .ok();
                            }))
                    }),
            )
            .into_any_element()
    }
}

impl Sidebar {
    fn account_item(
        &self,
        account: Option<Id>,
        name: &str,
        detail: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = self.theme;
        let selected = self.view.account_id == account;
        div()
            .id(format!("account-{}", account.unwrap_or(0)))
            .test_support()
            .aria_label(if account.is_none() {
                "All inboxes".to_string()
            } else {
                detail.to_string()
            })
            .aria_selected(selected)
            .flex()
            .flex_col()
            .justify_center()
            .gap_1()
            .px_3()
            .py_2()
            .rounded(px(7.))
            .cursor_pointer()
            .flex_shrink_0()
            .text_color(if selected { t.fg } else { t.fg2 })
            .when(selected, |d| d.bg(t.bg3))
            .when(!selected, |d| d.hover(|s| s.bg(t.bg3)))
            .child(
                div()
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .child(name.to_string()),
            )
            .child(
                div()
                    .truncate()
                    .text_size(px(11.))
                    .text_color(t.fg2)
                    .child(detail.to_string()),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.parent
                    .update(cx, |kite, cx| kite.select_account(account, window, cx))
                    .ok();
            }))
            .into_any_element()
    }
}
