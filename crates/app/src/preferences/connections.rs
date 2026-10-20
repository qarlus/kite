//! Connections: native Proton sign-in and Outlook.
//!
//! Proton Mail signs in directly through Kite's own API client (SRP, 2FA, mailbox password),
//! so a free Proton plan works with no Bridge installed. Sign-in can stop on a second step —
//! an authenticator code, a separate mailbox password, or a browser verification challenge —
//! and the form asks for only that answer. Outlook signs in through the browser with the app
//! client ID compiled into this build.
use super::{button, card, heading, row};
use crate::app::Kite;
use crate::theme::Theme;
use gpui_kit::component::Sizable;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::prelude::*;
use gpui_kit::*;
use kite_core::model::{Account, ProtonAuth};

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
}

impl State {
    pub fn new(window: &mut Window, cx: &mut Context<Kite>) -> Self {
        let address = cx.new(|cx| InputState::new(window, cx).placeholder("you@proton.me"));
        let secret = cx.new(|cx| InputState::new(window, cx).placeholder("Proton password"));
        secret.update(cx, |input, cx| input.set_masked(true, window, cx));
        let answer = cx.new(|cx| InputState::new(window, cx).placeholder("Answer"));
        answer.update(cx, |input, cx| input.set_masked(true, window, cx));
        Self {
            native: true,
            step: Step::Password,
            address,
            secret,
            answer,
        }
    }
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

fn spinner(t: Theme) -> Spinner {
    Spinner::new().with_size(px(15.)).color(t.accent.into())
}

impl Kite {
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

    /// Starts the Outlook browser sign-in. Provider work runs off the UI thread.
    fn connect_outlook(&mut self, cx: &mut Context<Self>) {
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
        let task = cx
            .background_executor()
            .spawn(async move { mail.add_microsoft() });
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
                        this.account_created(account, cx);
                        if active && let Some(page) = &mut this.preferences {
                            page.notice = Some("Account connected".into());
                        }
                    }
                    Err(error) => {
                        if active && let Some(page) = &mut this.preferences {
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

    // Native Proton sign-in: no Bridge required, so it works on free plans.
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
            .on_click(cx.listener(|this, _, _, cx| this.connect_outlook(cx))),
    ));

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
                    "Sign in to Proton or Outlook. Proton works on free plans; Outlook opens a secure browser sign-in.",
                ))
                .child(card(t, connect)),
        )
        .into_any_element()
}
