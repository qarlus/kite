//! Compose: a floating panel for new mail, reply, reply-all and forward.
//! Phase 1 keeps the draft in memory; closing the panel without sending keeps it.
use crate::app::Kite;
use crate::theme::{self, RADIUS, RADIUS_SM, SHADOW, Theme};
use chrono::{Local, TimeZone as _};
use gpui_kit::assets::IconName;
use gpui_kit::component::input::{Input, InputState, Textarea, TextareaState};
use gpui_kit::component::{Icon, Sizable};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kite_core::model::{Addr, Draft, Id, Message};
use std::collections::HashSet;

/// A draft the user is editing. Input state lives in the entities so an Escape
/// can hide the panel without losing a single keystroke.
pub struct Compose {
    pub to: Entity<InputState>,
    pub cc: Entity<InputState>,
    pub subject: Entity<InputState>,
    pub body: Entity<TextareaState>,
    pub account_id: Id,
    /// The message being replied to (`None` for new mail and forward).
    pub reply_to: Option<Id>,
    pub show_cc: bool,
    title: &'static str,
}

/// Everything a new panel starts with. Keeps the constructor readable.
struct Parts {
    account_id: Id,
    title: &'static str,
    to: Vec<Addr>,
    cc: Vec<Addr>,
    subject: String,
    body: String,
    reply_to: Option<Id>,
}

impl Compose {
    fn new(window: &mut Window, cx: &mut Context<Kite>, parts: Parts) -> Self {
        let to = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("To")
                .default_value(format_addrs(&parts.to))
        });
        let cc = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Cc")
                .default_value(format_addrs(&parts.cc))
        });
        let subject = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Subject")
                .default_value(parts.subject)
        });
        let body = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Write a message")
                .default_value(parts.body)
        });
        Self {
            to,
            cc,
            subject,
            body,
            account_id: parts.account_id,
            reply_to: parts.reply_to,
            show_cc: !parts.cc.is_empty(),
            title: parts.title,
        }
    }

    fn from_draft(window: &mut Window, cx: &mut Context<Kite>, draft: Draft) -> Self {
        Self::new(
            window,
            cx,
            Parts {
                account_id: draft.account_id,
                title: "New message",
                to: draft.to,
                cc: draft.cc,
                subject: draft.subject,
                body: draft.body,
                reply_to: draft.reply_to,
            },
        )
    }

    fn draft(&self, cx: &App) -> Draft {
        Draft {
            account_id: self.account_id,
            to: parse_addrs(&self.to.read(cx).value()),
            cc: parse_addrs(&self.cc.read(cx).value()),
            bcc: Vec::new(),
            subject: self.subject.read(cx).value().trim().to_string(),
            body: self.body.read(cx).value().to_string(),
            reply_to: self.reply_to,
            send_at: None,
            remind_at: None,
        }
    }
}

impl Kite {
    pub fn compose_open(&self) -> bool {
        self.compose_open
    }

    pub fn compose_to(&self, cx: &App) -> String {
        self.compose
            .as_ref()
            .map(|compose| compose.to.read(cx).value().to_string())
            .unwrap_or_default()
    }

    pub fn compose_subject(&self, cx: &App) -> String {
        self.compose
            .as_ref()
            .map(|compose| compose.subject.read(cx).value().to_string())
            .unwrap_or_default()
    }

    pub fn compose_body(&self, cx: &App) -> String {
        self.compose
            .as_ref()
            .map(|compose| compose.body.read(cx).value().to_string())
            .unwrap_or_default()
    }

    pub(crate) fn open_compose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.compose.is_none() {
            let account_id = self
                .view
                .account_id
                .or_else(|| self.accounts.first().map(|account| account.id))
                .unwrap_or(0);
            self.compose = Some(Compose::new(
                window,
                cx,
                Parts {
                    account_id,
                    title: "New message",
                    to: Vec::new(),
                    cc: Vec::new(),
                    subject: String::new(),
                    body: String::new(),
                    reply_to: None,
                },
            ));
        }
        self.compose_open = true;
        self.focus_compose(window, cx, true);
        cx.notify();
    }

    pub(crate) fn compose_reply(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        all: bool,
        forward: bool,
    ) {
        let Some((account_id, message)) = self.reply_target() else {
            self.toast("Open a message first", cx);
            return;
        };
        let own = self
            .accounts
            .iter()
            .find(|account| account.id == account_id)
            .map(|account| account.email.clone())
            .unwrap_or_default();
        let (title, prefix) = if forward {
            ("Forward", "Fwd: ")
        } else if all {
            ("Reply all", "Re: ")
        } else {
            ("Reply", "Re: ")
        };
        let (to, cc) = if forward {
            (Vec::new(), Vec::new())
        } else if all {
            let recipients = reply_all(&message, &own);
            (recipients, Vec::new())
        } else {
            (vec![message.from.clone()], Vec::new())
        };
        let reply_to = if forward { None } else { Some(message.id) };
        self.compose = Some(Compose::new(
            window,
            cx,
            Parts {
                account_id,
                title,
                to,
                cc,
                subject: prefixed(&message.subject, prefix),
                body: quote(&message),
                reply_to,
            },
        ));
        self.compose_open = true;
        self.focus_compose(window, cx, forward);
        cx.notify();
    }

    pub(crate) fn close_compose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.compose_open = false;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    pub(crate) fn send_compose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(compose) = &self.compose else {
            return;
        };
        let draft = compose.draft(cx);
        if draft.to.is_empty() {
            self.toast("Add a recipient", cx);
            return;
        }
        match self.mail.send(draft) {
            Ok(id) => {
                self.sent = Some(id);
                self.compose = None;
                self.compose_open = false;
                window.focus(&self.focus, cx);
                self.toast("Sending…", cx);
            }
            Err(error) => self.toast(format!("Could not send: {error}"), cx),
        }
    }

    /// Reopens the draft returned by `cancel_send`.
    pub(crate) fn reopen_sent(&mut self, window: &mut Window, draft: Draft, cx: &mut Context<Self>) {
        self.compose = Some(Compose::from_draft(window, cx, draft));
        self.compose_open = true;
        self.focus_compose(window, cx, false);
        cx.notify();
    }

    /// The latest message of the open thread, or of the selected row.
    fn reply_target(&self) -> Option<(Id, Message)> {
        if let Some(open) = &self.open {
            return open
                .messages
                .last()
                .map(|message| (open.account_id, message.clone()));
        }
        let row = self.rows.get(self.sel)?;
        let messages = match self.msgs.get(&row.id) {
            Some(messages) => messages.clone(),
            None => self.mail.thread(row.id).unwrap_or_default(),
        };
        messages
            .last()
            .map(|message| (row.account_id, message.clone()))
    }

    fn focus_compose(&self, window: &mut Window, cx: &mut Context<Self>, to: bool) {
        let Some(compose) = &self.compose else {
            return;
        };
        let handle = if to {
            compose.to.read(cx).focus_handle(cx)
        } else {
            compose.body.read(cx).focus_handle(cx)
        };
        window.focus(&handle, cx);
    }

    pub fn render_compose(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.compose_open {
            return None;
        }
        let compose = self.compose.as_ref()?;
        let t = self.theme;
        let to = compose.to.clone();
        let cc = compose.cc.clone();
        let subject = compose.subject.clone();
        let body = compose.body.clone();
        let show_cc = compose.show_cc;
        let title = compose.title;
        Some(
            div()
                .id("compose")
                .test_support()
                .aria_label(title)
                .key_context("KiteCompose")
                .absolute()
                .bottom(px(18.))
                .right(px(18.))
                .w(px(560.))
                .h(px(480.))
                .flex()
                .flex_col()
                .rounded(px(RADIUS))
                .bg(t.bg2)
                .border_1()
                .border_color(t.line)
                .shadow(vec![
                    BoxShadow::new(px(0.), px(20.), SHADOW.into()).blur_radius(px(60.)),
                ])
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .px_4()
                        .pt_3()
                        .pb_2()
                        .child(
                            div()
                                .text_size(t.px(13.))
                                .font_weight(FontWeight(650.))
                                .child(title),
                        )
                        .child(
                            div()
                                .id("compose-close")
                                .test_support()
                                .aria_label("Close compose")
                                .flex()
                                .items_center()
                                .justify_center()
                                .size(px(24.))
                                .rounded(px(RADIUS_SM))
                                .cursor_pointer()
                                .hover(|s| s.bg(t.bg3))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.close_compose(window, cx)
                                }))
                                .child(
                                    Icon::new(IconName::X)
                                        .with_size(px(14.))
                                        .text_color(t.fg2),
                                ),
                        ),
                )
                .child(field_row(&t, "To", &to, "compose-to", "To"))
                .when(show_cc, |d| {
                    d.child(field_row(&t, "Cc", &cc, "compose-cc", "Cc"))
                })
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_end()
                        .px_4()
                        .when(!show_cc, |d| {
                            d.child(
                                div()
                                    .id("compose-show-cc")
                                    .test_support()
                                    .aria_label("Show Cc")
                                    .cursor_pointer()
                                    .text_size(t.px(11.))
                                    .text_color(t.fg3)
                                    .hover(|s| s.text_color(t.fg))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if let Some(compose) = &mut this.compose {
                                            compose.show_cc = true;
                                        }
                                        cx.notify();
                                    }))
                                    .child("Cc"),
                            )
                        }),
                )
                .child(field_row(&t, "Subject", &subject, "compose-subject", "Subject"))
                .child(
                    div()
                        .id("compose-body")
                        .test_support()
                        .aria_label("Message")
                        .flex_1()
                        .min_h_0()
                        .px_4()
                        .py_2()
                        .on_click(cx.listener(|this, _, window, cx| {
                            if let Some(compose) = &this.compose {
                                let handle = compose.body.read(cx).focus_handle(cx);
                                window.focus(&handle, cx);
                            }
                        }))
                        .child(
                            Textarea::new(&body)
                                .aria_label("Message")
                                .appearance(false)
                                .bordered(false)
                                .text_size(t.px(14.))
                                .text_color(t.fg),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .px_4()
                        .py_3()
                        .border_t_1()
                        .border_color(t.line)
                        .child(
                            div()
                                .text_size(t.px(11.))
                                .text_color(t.fg3)
                                .child("Esc keeps this draft"),
                        )
                        .child(
                            div()
                                .id("compose-send")
                                .test_support()
                                .aria_label("Send")
                                .flex()
                                .items_center()
                                .gap_2()
                                .px_3()
                                .py_2()
                                .rounded(px(RADIUS_SM))
                                .cursor_pointer()
                                .bg(theme::accent_glow(&t))
                                .text_color(t.fg)
                                .text_size(t.px(13.))
                                .font_weight(FontWeight(600.))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.send_compose(window, cx)
                                }))
                                .child("Send")
                                .child(
                                    div()
                                        .text_size(t.px(11.))
                                        .text_color(t.fg)
                                        .child("⌘↵"),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }
}

fn field_row(
    t: &Theme,
    label: &'static str,
    input: &Entity<InputState>,
    id: &'static str,
    aria: &'static str,
) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap_2()
        .px_4()
        .py_2()
        .border_b_1()
        .border_color(t.line)
        .child(
            div()
                .w(px(56.))
                .flex_shrink_0()
                .text_size(t.px(12.))
                .text_color(t.fg3)
                .child(label),
        )
        .child(
            Input::new(input)
                .id(id)
                .aria_label(aria)
                .appearance(false)
                .bordered(false)
                .text_size(t.px(14.))
                .text_color(t.fg)
                .w_full(),
        )
        .into_any_element()
}

/// `Name <email>, other@example.com`, as the fields hold it.
fn format_addrs(addrs: &[Addr]) -> String {
    addrs
        .iter()
        .map(|addr| {
            if addr.name.trim().is_empty() {
                addr.email.clone()
            } else {
                format!("{} <{}>", addr.name.trim(), addr.email.trim())
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Accepts comma or semicolon separated `Name <email>` and bare addresses.
fn parse_addrs(value: &str) -> Vec<Addr> {
    value
        .split([',', ';'])
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|part| {
            if let Some(open) = part.rfind('<')
                && let Some(close) = part[open..].find('>')
            {
                let email = part[open + 1..open + close].trim();
                let name = part[..open].trim().trim_matches('"').trim();
                return Addr {
                    name: name.into(),
                    email: email.into(),
                };
            }
            Addr {
                name: String::new(),
                email: part.into(),
            }
        })
        .collect()
}

/// The sender plus every To/Cc address except our own, deduped by address.
fn reply_all(message: &Message, own: &str) -> Vec<Addr> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for addr in std::iter::once(&message.from)
        .chain(message.to.iter())
        .chain(message.cc.iter())
    {
        let email = addr.email.trim();
        if email.is_empty() || email.eq_ignore_ascii_case(own) {
            continue;
        }
        if seen.insert(email.to_ascii_lowercase()) {
            out.push(addr.clone());
        }
    }
    out
}

/// Adds a prefix once, so replies and forwards never stack their own marker.
fn prefixed(subject: &str, prefix: &str) -> String {
    let lower = subject.trim_start().to_lowercase();
    let already = if prefix.starts_with("Re") {
        lower.starts_with("re:")
    } else {
        lower.starts_with("fwd:") || lower.starts_with("fw:")
    };
    if already {
        subject.to_string()
    } else {
        format!("{prefix}{subject}")
    }
}

/// The original plain text under a one-line attribution.
fn quote(message: &Message) -> String {
    let date = Local
        .timestamp_millis_opt(message.date)
        .single()
        .map(|date| date.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_default();
    let from = if message.from.name.trim().is_empty() {
        message.from.email.clone()
    } else {
        format!("{} <{}>", message.from.name.trim(), message.from.email.trim())
    };
    let quoted = message
        .text
        .lines()
        .map(|line| format!("> {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!("\n\nOn {date}, {from} wrote:\n{quoted}")
}
