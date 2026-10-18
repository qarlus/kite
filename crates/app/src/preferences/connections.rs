//! Connections: Proton Bridge discovery, Outlook sign-in and manual Bridge credentials.
use super::{button, card, heading, row};
use crate::app::Kite;
use crate::theme::Theme;
use gpui_kit::assets::IconName;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{Icon, Sizable};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kite_core::model::ProtonLogin;

pub struct State {
    pub email: Entity<InputState>,
    pub password: Entity<InputState>,
    /// The manual Proton form is open.
    pub proton: bool,
    /// `discover_proton` is running off the UI thread.
    pub detecting: bool,
    /// Accounts Bridge already set up on this machine; host and ports are filled in.
    pub discovered: Vec<ProtonLogin>,
    /// Discovery finished without finding a local Bridge.
    pub searched: bool,
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
            detecting: false,
            discovered: Vec::new(),
            searched: false,
        }
    }
}

/// The one accent action on each group. Provider work stays secondary and quiet.
fn primary(
    id: impl Into<ElementId>,
    label: &str,
    t: Theme,
) -> gpui_kit::base::ObservedElement<Stateful<Div>> {
    button(id, label, t)
        .bg(t.accent)
        .border_color(t.accent)
        .text_color(t.bg)
}

fn detecting_row(t: Theme) -> AnyElement {
    div()
        .id("detecting-proton")
        .test_support()
        .aria_label("Looking for Proton Bridge")
        .flex()
        .items_center()
        .gap_3()
        .child(Spinner::new().with_size(px(16.)).color(t.accent.into()))
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(div().text_size(t.px(13.)).child("Looking for Proton Bridge…"))
                .child(
                    div()
                        .text_size(t.px(12.))
                        .text_color(t.fg2)
                        .child("Reading Bridge's local vault. Nothing leaves this computer."),
                ),
        )
        .into_any_element()
}

fn no_bridge_found(t: Theme) -> AnyElement {
    div()
        .id("no-bridge-found")
        .test_support()
        .aria_label("No local Proton Bridge found")
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .text_size(t.px(13.))
                .child("No local Proton Bridge found"),
        )
        .child(
            div()
                .text_size(t.px(12.))
                .text_color(t.fg2)
                .child("Open Bridge and look again, or connect with the credentials below."),
        )
        .into_any_element()
}

impl Kite {
    /// Looks for Bridge accounts off the UI thread. A failure or an empty result
    /// opens the manual form so account setup is never blocked.
    fn find_proton_bridge(&mut self, cx: &mut Context<Self>) {
        if self.account_busy {
            return;
        }
        let (request, mail) = {
            let Some(page) = &mut self.preferences else {
                return;
            };
            if page.connections.detecting {
                return;
            }
            page.connections.detecting = true;
            page.connections.searched = false;
            page.connections.discovered.clear();
            page.error = None;
            page.notice = None;
            (page.request, self.mail.clone())
        };
        let task = cx
            .background_executor()
            .spawn(async move { mail.discover_proton() });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                let active = this
                    .preferences
                    .as_ref()
                    .is_some_and(|page| page.request == request);
                if let Some(page) = &mut this.preferences {
                    page.connections.detecting = false;
                }
                if active {
                    match result {
                        Ok(found) => {
                            let empty = found.is_empty();
                            if let Some(page) = &mut this.preferences {
                                page.connections.searched = true;
                                page.connections.discovered = found;
                                if empty {
                                    page.connections.proton = true;
                                }
                            }
                        }
                        Err(error) => {
                            if let Some(page) = &mut this.preferences {
                                page.connections.proton = true;
                                page.error =
                                    Some(format!("Could not look for Proton Bridge: {error}"));
                            }
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

    fn connect_discovered(&mut self, login: ProtonLogin, cx: &mut Context<Self>) {
        self.connect_login(Some(login), cx);
    }

    fn connect_account(&mut self, proton: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.account_busy {
            return;
        }
        let login = if proton {
            let Some(page) = &mut self.preferences else {
                return;
            };
            let email = page.connections.email.read(cx).value().trim().to_string();
            let password = page.connections.password.read(cx).value().to_string();
            if email.is_empty() || password.is_empty() {
                page.error = Some("Enter your Bridge username and password.".into());
                cx.notify();
                return;
            }
            page.connections
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
        self.connect_login(login, cx);
    }

    /// Starts a provider sign-in. `login` is a Bridge credential (typed or discovered),
    /// or `None` for Microsoft. Provider work runs off the UI thread.
    fn connect_login(&mut self, login: Option<ProtonLogin>, cx: &mut Context<Self>) {
        if self.account_busy {
            return;
        }
        let Some(page) = &mut self.preferences else {
            return;
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
                        let email = account.email.clone();
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
                            page.connections.proton = false;
                            page.connections.searched = false;
                            page.connections
                                .discovered
                                .retain(|login| login.email != email);
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
}

pub fn render(kite: &Kite, _window: &Window, cx: &mut Context<Kite>) -> AnyElement {
    let Some(page) = &kite.preferences else {
        return div().into_any_element();
    };
    let t = kite.theme;
    let busy = page.accounts.busy;
    let mut connect = Vec::new();

    if busy {
        connect.push(
            div()
                .id("account-connecting")
                .test_support()
                .aria_label("Working on account")
                .flex()
                .items_center()
                .gap_3()
                .child(Spinner::new().with_size(px(16.)).color(t.accent.into()))
                .child(
                    div()
                        .text_size(t.px(13.))
                        .text_color(t.accent)
                        .child("Working on your account…"),
                )
                .into_any_element(),
        );
    } else {
        if page.connections.detecting {
            connect.push(detecting_row(t));
        } else {
            connect.push(row(
                t,
                "Proton Mail",
                "Find the accounts Bridge already set up here — no credentials to type.",
                primary("detect-proton", "Find Proton Bridge", t)
                    .on_click(cx.listener(|this, _, _, cx| this.find_proton_bridge(cx))),
            ));
        }

        for (index, login) in page.connections.discovered.iter().enumerate() {
            let login = login.clone();
            let detail = match (&login.host, login.imap_port, login.smtp_port) {
                (Some(host), Some(imap), Some(smtp)) => {
                    format!("{host} · IMAP {imap} · SMTP {smtp}")
                }
                (Some(host), _, _) => host.clone(),
                _ => "Found on this computer".into(),
            };
            connect.push(
                div()
                    .id(format!("found-proton-{index}"))
                    .test_support()
                    .aria_label(format!("Found {}", login.email))
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .size(px(36.))
                            .flex_shrink_0()
                            .rounded_full()
                            .bg(Theme::alpha(t.accent, 0.16))
                            .text_color(t.accent)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(Icon::new(IconName::CircleUser).with_size(px(17.))),
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
                                    .text_size(t.px(13.))
                                    .font_weight(FontWeight(650.))
                                    .child(format!("Found {}", login.email)),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(t.px(12.))
                                    .text_color(t.fg2)
                                    .child(detail),
                            ),
                    )
                    .child(
                        primary(format!("connect-discovered-{index}"), "Connect", t).on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.connect_discovered(login.clone(), cx)
                            }),
                        ),
                    )
                    .into_any_element(),
            );
        }

        if page.connections.searched && page.connections.discovered.is_empty() {
            connect.push(no_bridge_found(t));
        }

        connect.push(row(
            t,
            "Microsoft Outlook",
            "Sign in through your browser. Kite never sees your password.",
            button("add-microsoft", "Connect Outlook", t).on_click(
                cx.listener(|this, _, window, cx| this.connect_account(false, window, cx)),
            ),
        ));

        connect.push(row(
            t,
            "Proton Mail with Bridge credentials",
            "Use the username and password Bridge shows you, not your Proton password.",
            button("add-proton", "Connect Proton Mail", t).on_click(cx.listener(
                |this, _, window, cx| {
                    if let Some(page) = &mut this.preferences {
                        page.connections.proton = true;
                        page.error = None;
                        let focus = page.connections.email.read(cx).focus_handle(cx);
                        window.focus(&focus, cx);
                    }
                    cx.notify();
                },
            )),
        ));

        if page.connections.proton {
            connect.push(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .text_size(t.px(12.))
                            .text_color(t.fg2)
                            .child("Bridge must be running on this computer. Credentials live in your OS keychain."),
                    )
                    .child(
                        Input::new(&page.connections.email)
                            .id("proton-username")
                            .aria_label("Bridge username"),
                    )
                    .child(
                        Input::new(&page.connections.password)
                            .id("proton-password")
                            .aria_label("Bridge password"),
                    )
                    .child(
                        primary("connect-proton", "Connect Bridge", t).on_click(cx.listener(
                            |this, _, window, cx| this.connect_account(true, window, cx),
                        )),
                    )
                    .into_any_element(),
            );
        }
    }

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
                    "Connect an account",
                    "Proton Bridge is discovered, never typed. Outlook opens a secure sign-in.",
                ))
                .child(card(t, connect)),
        )
        .into_any_element()
}
