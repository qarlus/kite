//! Connections: Proton Bridge discovery, Outlook sign-in and manual Bridge credentials.
//!
//! Opening this page looks for Bridge, starts it when it is installed but closed, and creates
//! every account it finds — so the everyday path is zero clicks. Bridge keeps the Proton
//! sign-in (SRP, 2FA, security keys); when it opens locked, Kite waits and tells the user to
//! finish there. The manual form is only a fallback.
use super::{button, card, heading, row};
use crate::app::Kite;
use crate::theme::{RADIUS_SM, Theme};
use gpui_kit::assets::IconName;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{Icon, Sizable};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kite_core::model::ProtonLogin;
use std::time::Duration;

/// How long to wait for a started Bridge to answer, in 500 ms steps (10 s).
const POLL_INTERVAL: Duration = Duration::from_millis(500);
const POLL_ATTEMPTS: usize = 20;

pub struct State {
    pub email: Entity<InputState>,
    pub password: Entity<InputState>,
    /// The manual Proton form is open.
    pub proton: bool,
    /// Discovery or a Bridge start is running off the UI thread.
    pub detecting: bool,
    /// Bridge was started but never answered, so it is waiting for a Proton sign-in.
    pub needs_signin: bool,
    /// Every account Bridge reported, connected or not.
    pub found: Vec<ProtonLogin>,
    /// Discovered accounts still waiting to be created, oldest first.
    pub queue: Vec<ProtonLogin>,
    /// Discovery ran at least once for this page.
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
            needs_signin: false,
            found: Vec::new(),
            queue: Vec::new(),
            searched: false,
        }
    }
}

/// What one look-for-Bridge pass produced.
enum Outcome {
    Found(Vec<ProtonLogin>),
    /// Bridge is not installed, so there is nothing to start.
    NoBridge,
    /// Bridge is running but still locked, waiting for the user to sign in to Proton.
    SignIn,
    /// Bridge is present but its vault could not be read.
    Failed(String),
}

/// The one accent action on the page. Everything else stays quiet.
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

/// A small, borderless action for repeats, so it never competes with the page.
fn quiet(
    id: impl Into<ElementId>,
    label: &str,
    t: Theme,
) -> gpui_kit::base::ObservedElement<Stateful<Div>> {
    div()
        .id(id)
        .test_support()
        .aria_label(label.to_string())
        .px_2()
        .py_1()
        .rounded(px(RADIUS_SM))
        .text_size(t.px(12.))
        .text_color(t.fg2)
        .cursor_pointer()
        .hover(|s| s.bg(t.bg3).text_color(t.fg))
        .child(label.to_string())
}

fn spinner(t: Theme) -> Spinner {
    Spinner::new().with_size(px(15.)).color(t.accent.into())
}

impl Kite {
    /// Looks for Bridge off the UI thread, starting it when installed but closed, then creates
    /// every account it has that Kite is not already holding.
    pub(crate) fn find_proton_bridge(&mut self, cx: &mut Context<Self>) {
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
            page.connections.needs_signin = false;
            page.connections.found.clear();
            page.connections.queue.clear();
            page.error = None;
            page.notice = None;
            (page.request, self.mail.clone())
        };

        let executor = cx.background_executor().clone();
        let wait = executor.clone();
        let task = executor.spawn(async move {
            // 1. Bridge may already be up with a readable vault.
            let first = mail.discover_proton();
            let first_error = match first {
                Ok(accounts) if !accounts.is_empty() => return Outcome::Found(accounts),
                Ok(_) => None,
                Err(error) => Some(error.to_string()),
            };
            // 2. Start it when it is installed.
            if !mail.start_proton_bridge().unwrap_or(false) {
                return match first_error {
                    Some(error) => Outcome::Failed(error),
                    None => Outcome::NoBridge,
                };
            }
            // 3. Wait for the port. A locked Bridge opens its own sign-in window meanwhile.
            for attempt in 0..POLL_ATTEMPTS {
                if attempt > 0 {
                    wait.timer(POLL_INTERVAL).await;
                }
                if let Ok(accounts) = mail.discover_proton()
                    && !accounts.is_empty()
                {
                    return Outcome::Found(accounts);
                }
            }
            Outcome::SignIn
        });

        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            this.update(cx, |this, cx| {
                let active = this
                    .preferences
                    .as_ref()
                    .is_some_and(|page| page.request == request);
                if let Some(page) = &mut this.preferences {
                    page.connections.detecting = false;
                }
                if !active {
                    return;
                }
                match outcome {
                    Outcome::Found(found) => this.accept_discovered(found, cx),
                    Outcome::NoBridge => {
                        if let Some(page) = &mut this.preferences {
                            page.connections.searched = true;
                            page.connections.proton = false;
                        }
                    }
                    Outcome::SignIn => {
                        if let Some(page) = &mut this.preferences {
                            page.connections.searched = true;
                            page.connections.needs_signin = true;
                            page.connections.proton = false;
                        }
                    }
                    Outcome::Failed(error) => {
                        if let Some(page) = &mut this.preferences {
                            page.connections.searched = true;
                            page.connections.proton = true;
                            page.error =
                                Some(format!("Could not look for Proton Bridge: {error}"));
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

    /// Queues every discovered account Kite is not already holding and starts creating them.
    fn accept_discovered(&mut self, found: Vec<ProtonLogin>, cx: &mut Context<Self>) {
        let connected: Vec<String> = self.accounts.iter().map(|a| a.email.clone()).collect();
        let next = {
            let Some(page) = &mut self.preferences else {
                return;
            };
            page.connections.searched = true;
            page.connections.needs_signin = false;
            page.connections.proton = false;
            page.connections.found = found;
            page.connections.queue = page
                .connections
                .found
                .iter()
                .filter(|login| !connected.contains(&login.email))
                .cloned()
                .collect();
            page.connections.queue.first().cloned()
        };
        if let Some(login) = next {
            self.connect_login(Some(login), cx);
        }
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

    /// Starts a provider sign-in. `login` is a Bridge credential (discovered or typed),
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
        let proton = login.is_some();
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
                        let mut next = None;
                        if active && let Some(page) = &mut this.preferences {
                            page.accounts.busy = false;
                            page.connections.proton = false;
                            page.connections.queue.retain(|login| login.email != email);
                            page.notice = Some("Account connected".into());
                            next = page.connections.queue.first().cloned();
                        }
                        // Bridge can hold several accounts; create them one after another.
                        if let Some(login) = next {
                            this.connect_login(Some(login), cx);
                        }
                    }
                    Err(error) => {
                        if active && let Some(page) = &mut this.preferences {
                            page.accounts.busy = false;
                            // Stop the queue so a failing credential is not retried in a loop.
                            page.connections.queue.clear();
                            if proton {
                                page.connections.proton = true;
                            }
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

/// The always-present Proton Bridge row: it reports what discovery found and offers a
/// quiet repeat, never a second required click.
fn bridge_status(kite: &Kite, t: Theme, cx: &mut Context<Kite>) -> AnyElement {
    let Some(page) = &kite.preferences else {
        return div().into_any_element();
    };
    let connections = &page.connections;
    let searching = connections.detecting || page.accounts.busy;
    let description = if connections.detecting {
        "Looking for Proton Bridge, starting it if needed…".to_string()
    } else if page.accounts.busy {
        "Creating your account from Bridge…".to_string()
    } else if connections.needs_signin {
        "Proton Bridge is open and waiting for you to sign in to Proton. Finish signing in there, then look again.".to_string()
    } else if !connections.found.is_empty() {
        format!(
            "Connected {} from Bridge — nothing to type.",
            connections
                .found
                .iter()
                .map(|login| login.email.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    } else if connections.searched {
        "No local Proton Bridge found. Install Bridge, or connect Outlook below.".to_string()
    } else {
        "Uses the accounts Proton Bridge already set up on this computer.".to_string()
    };

    div()
        .id("proton-bridge-status")
        .test_support()
        .aria_label(description.clone())
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
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(div().text_size(t.px(13.)).child("Proton Bridge"))
                        .when(searching, |d| d.child(spinner(t))),
                )
                .child(
                    div()
                        .text_size(t.px(12.))
                        .text_color(if connections.found.is_empty() {
                            t.fg2
                        } else {
                            t.ok
                        })
                        .child(description.clone()),
                ),
        )
        .when(!searching, |d| {
            d.child(quiet("look-again-proton", "Look again", t).on_click(
                cx.listener(|this, _, _, cx| this.find_proton_bridge(cx)),
            ))
        })
        .into_any_element()
}

pub fn render(kite: &Kite, _window: &Window, cx: &mut Context<Kite>) -> AnyElement {
    let Some(page) = &kite.preferences else {
        return div().into_any_element();
    };
    let t = kite.theme;
    let busy = page.accounts.busy;

    let mut connect = vec![bridge_status(kite, t, cx)];

    // Bridge found: confirm each account compactly, with the provider it belongs to.
    for (index, login) in page.connections.found.iter().enumerate() {
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
                        .size(px(32.))
                        .flex_shrink_0()
                        .rounded_full()
                        .bg(Theme::alpha(t.ok, 0.16))
                        .text_color(t.ok)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(Icon::new(IconName::CircleCheck).with_size(px(16.))),
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
                                .child(login.email.clone()),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_size(t.px(12.))
                                .text_color(t.fg2)
                                .child(match (&login.host, login.imap_port, login.smtp_port) {
                                    (Some(host), Some(imap), Some(smtp)) => {
                                        format!("{host} · IMAP {imap} · SMTP {smtp}")
                                    }
                                    _ => "Found on this computer".into(),
                                }),
                        ),
                )
                .into_any_element(),
        );
    }

    if busy {
        connect.push(
            div()
                .id("account-connecting")
                .test_support()
                .aria_label("Working on account")
                .flex()
                .items_center()
                .gap_3()
                .child(spinner(t))
                .child(
                    div()
                        .text_size(t.px(13.))
                        .text_color(t.accent)
                        .child("Working on your account…"),
                )
                .into_any_element(),
        );
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
        "Only needed if Bridge is not running on this computer.",
        button("add-proton", "Enter credentials", t).on_click(cx.listener(
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
                        .child("Use the username and password Bridge shows you, not your Proton password."),
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
                    "Proton Bridge is found and connected for you. Outlook opens a secure sign-in.",
                ))
                .child(card(t, connect)),
        )
        .into_any_element()
}
