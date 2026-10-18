//! Accounts: the connected mailboxes and provider sign-in.
use super::{button, card, heading};
use crate::app::Kite;
use crate::theme::RADIUS;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kite_core::model::{Id, ProtonLogin};

pub struct State {
    pub email: Entity<InputState>,
    pub password: Entity<InputState>,
    /// The manual Proton form is open.
    pub proton: bool,
    pub busy: bool,
    /// The account awaiting remove confirmation.
    pub remove: Option<Id>,
}

impl State {
    pub fn new(window: &mut Window, cx: &mut Context<Kite>) -> Self {
        let email = cx.new(|cx| InputState::new(window, cx).placeholder("Bridge username"));
        let password = cx.new(|cx| InputState::new(window, cx).placeholder("Bridge password"));
        password.update(cx, |input, cx| input.set_masked(true, window, cx));
        Self {
            email,
            password,
            proton: false,
            busy: false,
            remove: None,
        }
    }
}

impl Kite {
    pub(crate) fn connect_account(
        &mut self,
        proton: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(page) = &mut self.preferences else {
            return;
        };
        if self.account_busy {
            return;
        }
        let login = if proton {
            let email = page.accounts.email.read(cx).value().trim().to_string();
            let password = page.accounts.password.read(cx).value().to_string();
            if email.is_empty() || password.is_empty() {
                page.error = Some("Enter your Bridge username and password.".into());
                cx.notify();
                return;
            }
            page.accounts
                .password
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
        page.accounts.busy = true;
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
                    page.accounts.busy = false;
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
                            page.accounts.busy = false;
                            page.accounts.proton = false;
                            page.notice = Some("Account connected".into());
                        }
                    }
                    Err(error) => {
                        if active && let Some(page) = &mut this.preferences {
                            page.accounts.busy = false;
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

    pub(crate) fn disconnect_account(&mut self, id: Id, cx: &mut Context<Self>) {
        let Some(page) = &mut self.preferences else {
            return;
        };
        if self.account_busy {
            return;
        }
        self.account_busy = true;
        page.accounts.busy = true;
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
                    page.accounts.busy = false;
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
                            page.accounts.busy = false;
                            page.accounts.remove = None;
                            page.notice = Some("Account removed from Kite".into());
                        }
                    }
                    Err(error) => {
                        if active && let Some(page) = &mut this.preferences {
                            page.accounts.busy = false;
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

pub fn render(kite: &Kite, _window: &Window, cx: &mut Context<Kite>) -> AnyElement {
    let Some(page) = &kite.preferences else {
        return div().into_any_element();
    };
    let t = kite.theme;

    let mut rows = Vec::new();
    for account in &kite.accounts {
        let id = account.id;
        let provider = match account.provider {
            kite_core::model::Provider::Proton => "Proton Mail",
            kite_core::model::Provider::Microsoft => "Microsoft Outlook",
        };
        rows.push(
            div()
                .id(format!("settings-account-{id}"))
                .test_support()
                .aria_label(account.email.clone())
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
                .when(!page.accounts.busy, |d| {
                    d.child(
                        button(format!("remove-account-{id}"), "Remove", t).on_click(
                            cx.listener(move |this, _, _, cx| {
                                if let Some(page) = &mut this.preferences {
                                    page.accounts.remove = Some(id);
                                    page.error = None;
                                }
                                cx.notify();
                            }),
                        ),
                    )
                })
                .into_any_element(),
        );
    }

    let mut content = div()
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
                    "Your accounts",
                    "Keep accounts together, or give each inbox its own space.",
                ))
                .child(card(t, rows)),
        );

    if let Some(id) = page.accounts.remove {
        content = content.child(
            div()
                .p_4()
                .rounded(px(RADIUS))
                .bg(t.bg2)
                .flex()
                .flex_col()
                .gap_3()
                .child(
                    div()
                        .text_size(t.px(13.))
                        .child("Remove this account from Kite? Mail on the server is kept."),
                )
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
                                    page.accounts.remove = None;
                                }
                                cx.notify();
                            }),
                        )),
                ),
        );
    }

    let mut connect = Vec::new();
    if page.accounts.busy {
        connect.push(
            div()
                .id("account-connecting")
                .test_support()
                .aria_label("Working on account")
                .text_size(t.px(13.))
                .text_color(t.accent)
                .child("Working on your account…")
                .into_any_element(),
        );
    } else {
        connect.push(
            div()
                .flex()
                .gap_3()
                .child(
                    button("add-microsoft", "Connect Outlook", t).on_click(
                        cx.listener(|this, _, window, cx| this.connect_account(false, window, cx)),
                    ),
                )
                .child(button("add-proton", "Connect Proton Mail", t).on_click(
                    cx.listener(|this, _, window, cx| {
                        if let Some(page) = &mut this.preferences {
                            page.accounts.proton = true;
                            page.error = None;
                            let focus = page.accounts.email.read(cx).focus_handle(cx);
                            window.focus(&focus, cx);
                        }
                        cx.notify();
                    }),
                ))
                .into_any_element(),
        );
        if page.accounts.proton {
            connect.push(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .text_size(t.px(12.))
                            .text_color(t.fg2)
                            .child("Use credentials from Proton Mail Bridge, not your Proton password. Bridge must be running on this computer."),
                    )
                    .child(
                        Input::new(&page.accounts.email)
                            .id("proton-username")
                            .aria_label("Bridge username"),
                    )
                    .child(
                        Input::new(&page.accounts.password)
                            .id("proton-password")
                            .aria_label("Bridge password"),
                    )
                    .child(
                        button("connect-proton", "Connect Bridge", t).on_click(cx.listener(
                            |this, _, window, cx| this.connect_account(true, window, cx),
                        )),
                    )
                    .into_any_element(),
            );
        }
    }

    content
        .child(
            div()
                .flex()
                .flex_col()
                .child(heading(
                    t,
                    "Connect an account",
                    "Microsoft opens a secure sign-in. Proton uses your local Bridge credentials.",
                ))
                .child(card(t, connect)),
        )
        .into_any_element()
}
