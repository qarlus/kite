//! Connections: native Proton sign-in, Proton Bridge discovery, Outlook.
//!
//! Proton Mail signs in directly through Kite's own API client (SRP, 2FA, mailbox password),
//! so a free Proton plan works with no Bridge installed. Sign-in can stop on a second step —
//! an authenticator code, a separate mailbox password, or a browser verification challenge —
//! and the form asks for only that answer. Bridge stays below as the option for people who
//! already run it: opening this page looks for Bridge, starts it when installed, and creates
//! every account it finds.
use super::{button, card, heading, row};
use crate::app::Kite;
use crate::theme::{RADIUS_SM, Theme};
use gpui_kit::assets::IconName;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{Icon, Sizable};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kite_core::model::{Account, ProtonAuth, ProtonLogin, Settings};
use std::time::Duration;

/// How long to wait for a started Bridge to answer, in 500 ms steps (10 s).
const POLL_INTERVAL: Duration = Duration::from_millis(500);
const POLL_ATTEMPTS: usize = 20;

/// Which answer the native Proton form is waiting for.
#[derive(Clone, Default)]
pub enum Step {
    /// Address and password.
    #[default]
    Password,
    /// A six-digit code from an authenticator app.
    TwoFactor,
    /// The account's separate mailbox password.
    MailboxPassword,
    /// Proton's browser challenge; the user pastes the token it gives back.
    HumanVerification(String),
}

pub struct State {
    /// The native Proton form is showing.
    pub native: bool,
    pub step: Step,
    pub address: Entity<InputState>,
    pub secret: Entity<InputState>,
    /// The answer to the current step (code, mailbox password or verification token).
    pub answer: Entity<InputState>,
    pub email: Entity<InputState>,
    pub password: Entity<InputState>,
    /// The Microsoft app (client) ID Outlook signs in as.
    pub ms_client_id: Entity<InputState>,
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
    pub fn new(settings: &Settings, window: &mut Window, cx: &mut Context<Kite>) -> Self {
        let address = cx.new(|cx| InputState::new(window, cx).placeholder("you@proton.me"));
        let secret = cx.new(|cx| InputState::new(window, cx).placeholder("Proton password"));
        secret.update(cx, |input, cx| input.set_masked(true, window, cx));
        let answer = cx.new(|cx| InputState::new(window, cx).placeholder("Answer"));
        answer.update(cx, |input, cx| input.set_masked(true, window, cx));
        let email = cx.new(|cx| InputState::new(window, cx).placeholder("Bridge username"));
        let password = cx.new(|cx| InputState::new(window, cx).placeholder("Bridge password"));
        password.update(cx, |input, cx| input.set_masked(true, window, cx));
        let ms_client_id = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Application (client) ID")
                .default_value(settings.ms_client_id.clone())
        });
        Self {
            native: true,
            step: Step::Password,
            address,
            secret,
            answer,
            email,
            password,
            ms_client_id,
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

/// The Outlook app ID is the registration's Application (client) ID — a GUID. A typo here
/// only fails inside the browser, so it is checked before sign-in opens.
fn is_guid(value: &str) -> bool {
    let parts: Vec<&str> = value.split('-').collect();
    parts.len() == 5
        && [8usize, 4, 4, 4, 12]
            .into_iter()
            .zip(&parts)
            .all(|(len, part)| part.len() == len && part.chars().all(|c| c.is_ascii_hexdigit()))
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
                            page.error = Some(format!("Could not look for Proton Bridge: {error}"));
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

    /// Starts the native Proton sign-in (no Bridge) with the typed address and password.
    pub(crate) fn start_proton_sign_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.account_busy {
            return;
        }
        let Some(page) = &mut self.preferences else {
            return;
        };
        let email = page.connections.address.read(cx).value().trim().to_string();
        let password = page.connections.secret.read(cx).value().to_string();
        if email.is_empty() || password.is_empty() {
            page.error = Some("Enter your Proton address and password.".into());
            cx.notify();
            return;
        }
        page.connections
            .secret
            .update(cx, |input, cx| input.set_value("", window, cx));
        let mail = self.mail.clone();
        self.proton_busy(cx);
        let task = cx
            .background_executor()
            .spawn(async move { mail.proton_sign_in(&email, &password) });
        self.await_proton(task, cx);
    }

    /// Answers the step the engine asked for (code, mailbox password or token).
    pub(crate) fn answer_proton_step(&mut self, cx: &mut Context<Self>) {
        if self.account_busy {
            return;
        }
        let Some(page) = &mut self.preferences else {
            return;
        };
        let answer = page.connections.answer.read(cx).value().trim().to_string();
        // Verification has nothing to type: Proton already knows the puzzle was solved.
        let verifying = matches!(page.connections.step, Step::HumanVerification(_));
        if answer.is_empty() && !verifying {
            page.error = Some("Enter the answer Proton asked for.".into());
            cx.notify();
            return;
        }
        let mail = self.mail.clone();
        self.proton_busy(cx);
        let task = cx
            .background_executor()
            .spawn(async move { mail.proton_sign_in_step(&answer) });
        self.await_proton(task, cx);
    }

    fn proton_busy(&mut self, cx: &mut Context<Self>) {
        self.account_busy = true;
        if let Some(page) = &mut self.preferences {
            page.accounts.busy = true;
            page.error = None;
            page.notice = None;
        }
        cx.notify();
    }

    /// Runs one sign-in call off the UI thread and turns its result into the next form step.
    fn await_proton<T>(&mut self, task: T, cx: &mut Context<Self>)
    where
        T: std::future::Future<Output = anyhow::Result<ProtonAuth>> + 'static,
    {
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.account_busy = false;
                if let Some(page) = &mut this.preferences {
                    page.accounts.busy = false;
                }
                match result {
                    Ok(ProtonAuth::Done(account)) => {
                        this.account_created(account, cx);
                        if let Some(page) = &mut this.preferences {
                            page.connections.native = false;
                            page.connections.step = Step::Password;
                            page.notice = Some("Account connected".into());
                        }
                    }
                    Ok(ProtonAuth::TwoFactor) => this.proton_ask(Step::TwoFactor),
                    Ok(ProtonAuth::MailboxPassword) => this.proton_ask(Step::MailboxPassword),
                    Ok(ProtonAuth::HumanVerification { url }) => {
                        this.proton_ask(Step::HumanVerification(url))
                    }
                    Err(error) => {
                        if let Some(page) = &mut this.preferences {
                            // The whole chain: the outer context alone ("Proton key salts") hides the reason.
                            page.error = Some(format!("{error:#}"));
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

    /// Shows the question the account has to answer next.
    fn proton_ask(&mut self, step: Step) {
        if let Some(page) = &mut self.preferences {
            page.connections.step = step;
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
            // The app ID is saved first: the engine reads it when sign-in starts.
            let id = match &self.preferences {
                Some(page) => page
                    .connections
                    .ms_client_id
                    .read(cx)
                    .value()
                    .trim()
                    .to_string(),
                None => return,
            };
            if !id.is_empty() && !is_guid(&id) {
                if let Some(page) = &mut self.preferences {
                    page.error = Some(
                        "The Outlook app ID is the Application (client) ID from the Entra \
                         app registration — a GUID like 11111111-2222-3333-4444-555555555555, \
                         not the object ID or a secret."
                            .into(),
                    );
                }
                cx.notify();
                return;
            }
            if id != self.settings.ms_client_id {
                let mut settings = self.settings.clone();
                settings.ms_client_id = id;
                self.save_preferences(settings, window, cx);
                if self
                    .preferences
                    .as_ref()
                    .is_some_and(|page| page.error.is_some())
                {
                    return;
                }
            }
            None
        };
        self.connect_login(login, cx);
    }

    /// Publishes a freshly created account to the sidebar and the open view.
    fn account_created(&mut self, account: Account, cx: &mut Context<Self>) {
        self.accounts = self
            .mail
            .accounts()
            .unwrap_or_else(|_| self.accounts.clone());
        if !self
            .accounts
            .iter()
            .any(|existing| existing.id == account.id)
        {
            self.accounts.push(account);
        }
        self.folders = self.mail.folders().unwrap_or_default();
        self.sidebar_dirty = true;
        self.reload();
        cx.notify();
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
                        this.account_created(account, cx);
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
            d.child(
                quiet("look-again-proton", "Look again", t)
                    .on_click(cx.listener(|this, _, _, cx| this.find_proton_bridge(cx))),
            )
        })
        .into_any_element()
}

/// The native Proton form: address and password, then only the answer the account needs.
fn proton_form(kite: &Kite, t: Theme, cx: &mut Context<Kite>) -> AnyElement {
    let Some(page) = &kite.preferences else {
        return div().into_any_element();
    };
    let connections = &page.connections;
    let hint = |text: &'static str| div().text_size(t.px(12.)).text_color(t.fg2).child(text);
    let answer = || {
        Input::new(&connections.answer)
            .id("proton-answer")
            .aria_label("Proton answer")
    };
    let mut form = div().flex().flex_col().gap_3();
    match &connections.step {
        Step::Password => {
            form = form
                .child(hint(
                    "Use your normal Proton password. Kite never stores it.",
                ))
                .child(
                    Input::new(&connections.address)
                        .id("proton-address")
                        .aria_label("Proton address"),
                )
                .child(
                    Input::new(&connections.secret)
                        .id("proton-secret")
                        .aria_label("Proton password"),
                )
                .child(primary("proton-sign-in", "Sign in", t).on_click(
                    cx.listener(|this, _, window, cx| this.start_proton_sign_in(window, cx)),
                ));
        }
        Step::TwoFactor => {
            form = form
                .child(hint(
                    "Enter the six-digit code from your authenticator app.",
                ))
                .child(answer())
                .child(
                    primary("proton-step", "Verify code", t)
                        .on_click(cx.listener(|this, _, _, cx| this.answer_proton_step(cx))),
                );
        }
        Step::MailboxPassword => {
            form = form
                .child(hint(
                    "This account has a separate mailbox password. It unlocks your mail.",
                ))
                .child(answer())
                .child(
                    primary("proton-step", "Unlock mail", t)
                        .on_click(cx.listener(|this, _, _, cx| this.answer_proton_step(cx))),
                );
        }
        Step::HumanVerification(url) => {
            let url = url.clone();
            form = form
                .child(hint(
                    "Proton wants to check you are human. Open the page, solve the puzzle until it says Verification completed, then come back here.",
                ))
                .child(
                    button("proton-verify", "Open verification page", t).on_click(
                        cx.listener(move |_, _, _, cx| cx.open_url(&url)),
                    ),
                )
                .child(
                    primary("proton-step", "I've completed the verification", t).on_click(
                        cx.listener(|this, _, _, cx| this.answer_proton_step(cx)),
                    ),
                );
        }
    }
    form.into_any_element()
}

pub fn render(kite: &Kite, _window: &Window, cx: &mut Context<Kite>) -> AnyElement {
    let Some(page) = &kite.preferences else {
        return div().into_any_element();
    };
    let t = kite.theme;
    let busy = page.accounts.busy;

    let mut connect = vec![];

    // Native Proton sign-in: the primary path, and the only one that needs no Bridge.
    connect.push(row(
        t,
        "Proton Mail",
        "Sign in with your Proton address. Works on free plans, with no Bridge running.",
        button("proton-native", "Sign in to Proton", t).on_click(cx.listener(
            |this, _, window, cx| {
                if let Some(page) = &mut this.preferences {
                    page.connections.native = true;
                    page.connections.step = Step::Password;
                    page.error = None;
                    let focus = page.connections.address.read(cx).focus_handle(cx);
                    window.focus(&focus, cx);
                }
                cx.notify();
            },
        )),
    ));

    if page.connections.native {
        connect.push(proton_form(kite, t, cx));
    }

    connect.push(bridge_status(kite, t, cx));

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
        "Sign in through your browser — Microsoft 365 and personal Outlook.com accounts both work. Kite never sees your password.",
        button("add-microsoft", "Connect Outlook", t)
            .on_click(cx.listener(|this, _, window, cx| this.connect_account(false, window, cx))),
    ));

    let instruction = |text: &'static str| {
        div()
            .text_size(t.px(12.))
            .text_color(t.fg2)
            .child(text)
    };
    connect.push(
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(instruction(
                "Outlook sign-in needs a Microsoft Entra app registration. To create one: \
                 entra.microsoft.com → App registrations → New registration, then under \
                 Authentication add a Mobile and desktop redirect URI for http://localhost \
                 and enable public client flows.",
            ))
            .child(instruction(
                "For a personal Outlook.com mailbox, the registration's supported account \
                 types must include personal Microsoft accounts — pick \"any organizational \
                 directory and personal Microsoft accounts\", or \"personal Microsoft accounts \
                 only\".",
            ))
            .child(instruction(
                "Copy the Application (client) ID here. No client secret is needed. If Kite \
                 was built with an app ID, this field can stay empty.",
            ))
            .child(
                Input::new(&page.connections.ms_client_id)
                    .id("ms-client-id")
                    .aria_label("Outlook app ID"),
            )
            .into_any_element(),
    );

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
                .child(div().text_size(t.px(12.)).text_color(t.fg2).child(
                    "Use the username and password Bridge shows you, not your Proton password.",
                ))
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
                .child(primary("connect-proton", "Connect Bridge", t).on_click(
                    cx.listener(|this, _, window, cx| this.connect_account(true, window, cx)),
                ))
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
                    "Sign in to Proton directly, or let Kite find Proton Bridge. Outlook opens a secure sign-in.",
                ))
                .child(card(t, connect)),
        )
        .into_any_element()
}
