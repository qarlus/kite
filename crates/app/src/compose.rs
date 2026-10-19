//! Compose: a floating panel for new mail, reply, reply-all and forward.
//! Drafts save through `Mail::save_draft`: debounced while typing, and on close.
use crate::app::{Kite, Overlay};
use crate::theme::{self, BAR_H, RADIUS, RADIUS_SM, SHADOW, Theme};
use chrono::{Local, TimeZone as _};
use gpui_kit::assets::IconName;
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::{Icon, Sizable};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kite_core::model::{Action as MailAction, Addr, Draft, Id, Message, Ms};
use std::collections::HashSet;
use std::time::Duration;

/// A draft the user is editing. Input state lives in the entities so an Escape
/// can hide the panel without losing a single keystroke.
pub struct Compose {
    pub to: Entity<InputState>,
    pub cc: Entity<InputState>,
    pub subject: Entity<InputState>,
    pub body: Entity<TextareaState>,
    pub account_id: Id,
    /// The saved draft this panel edits, so a later save replaces it.
    pub id: Option<Id>,
    /// The message being replied to (`None` for new mail and forward).
    pub reply_to: Option<Id>,
    pub show_cc: bool,
    bcc: Vec<Addr>,
    send_at: Option<Ms>,
    remind_at: Option<Ms>,
    title: &'static str,
    /// Unsaved edits exist since the last successful save.
    dirty: bool,
    /// The draft has been saved at least once, so the panel can say "Saved".
    saved: bool,
    /// Bumped on every edit and save, so a stale debounce cannot save newer state.
    save_gen: u64,
    _subs: Vec<Subscription>,
}

/// Everything a new panel starts with. Keeps the constructor readable.
struct Parts {
    account_id: Id,
    id: Option<Id>,
    title: &'static str,
    to: Vec<Addr>,
    cc: Vec<Addr>,
    bcc: Vec<Addr>,
    subject: String,
    body: String,
    reply_to: Option<Id>,
    send_at: Option<Ms>,
    remind_at: Option<Ms>,
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
        // Autosave is driven by real edits, so a keystroke never waits on the store.
        let mut subs = Vec::with_capacity(4);
        for input in [&to, &cc, &subject] {
            subs.push(
                cx.subscribe_in(input, window, |this, _, event: &InputEvent, _, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.compose_changed(cx);
                    }
                }),
            );
        }
        subs.push(
            cx.subscribe_in(&body, window, |this, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    this.compose_changed(cx);
                }
            }),
        );
        Self {
            to,
            cc,
            subject,
            body,
            account_id: parts.account_id,
            id: parts.id,
            reply_to: parts.reply_to,
            show_cc: !parts.cc.is_empty(),
            bcc: parts.bcc,
            send_at: parts.send_at,
            remind_at: parts.remind_at,
            title: parts.title,
            dirty: false,
            saved: parts.id.is_some(),
            save_gen: 0,
            _subs: subs,
        }
    }

    fn from_draft(window: &mut Window, cx: &mut Context<Kite>, draft: Draft) -> Self {
        Self::new(
            window,
            cx,
            Parts {
                account_id: draft.account_id,
                id: draft.id,
                title: "New message",
                to: draft.to,
                cc: draft.cc,
                bcc: draft.bcc,
                subject: draft.subject,
                body: draft.body,
                reply_to: draft.reply_to,
                send_at: draft.send_at,
                remind_at: draft.remind_at,
            },
        )
    }

    fn draft(&self, cx: &App) -> Draft {
        Draft {
            id: self.id,
            account_id: self.account_id,
            to: parse_addrs(&self.to.read(cx).value()),
            cc: parse_addrs(&self.cc.read(cx).value()),
            bcc: self.bcc.clone(),
            subject: self.subject.read(cx).value().trim().to_string(),
            body: self.body.read(cx).value().to_string(),
            reply_to: self.reply_to,
            send_at: self.send_at,
            remind_at: self.remind_at,
        }
    }

    fn is_empty(&self, cx: &App) -> bool {
        self.to.read(cx).value().trim().is_empty()
            && self.cc.read(cx).value().trim().is_empty()
            && self.subject.read(cx).value().trim().is_empty()
            && self.body.read(cx).value().trim().is_empty()
            && self.bcc.is_empty()
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

    pub(crate) fn has_draft(&self, cx: &App) -> bool {
        match &self.compose {
            Some(compose) => !compose.is_empty(cx),
            None => false,
        }
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
                    id: None,
                    title: "New message",
                    to: Vec::new(),
                    cc: Vec::new(),
                    bcc: Vec::new(),
                    subject: String::new(),
                    body: String::new(),
                    reply_to: None,
                    send_at: None,
                    remind_at: None,
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
        // A hidden compose still holds the user's draft: never replace it.
        if self.has_draft(cx) {
            self.compose_open = true;
            self.focus_compose(window, cx, false);
            self.toast("Draft kept", cx);
            cx.notify();
            return;
        }
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
            reply_all(&message, &own)
        } else {
            (reply_to(&message, &own), Vec::new())
        };
        let reply_to = if forward { None } else { Some(message.id) };
        let body = if forward {
            forward_body(&message)
        } else {
            quote(&message)
        };
        self.compose = Some(Compose::new(
            window,
            cx,
            Parts {
                account_id,
                id: None,
                title,
                to,
                cc,
                bcc: Vec::new(),
                subject: prefixed(&message.subject, prefix),
                body,
                reply_to,
                send_at: None,
                remind_at: None,
            },
        ));
        self.compose_open = true;
        self.focus_compose(window, cx, forward);
        cx.notify();
    }

    pub(crate) fn close_compose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.flush_compose(cx);
        self.compose_open = false;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// A keystroke in any field: paint the edit, then schedule a save after a quiet pause.
    pub(crate) fn compose_changed(&mut self, cx: &mut Context<Self>) {
        let Some(compose) = &mut self.compose else {
            return;
        };
        compose.dirty = true;
        compose.save_gen = compose.save_gen.wrapping_add(1);
        let generation = compose.save_gen;
        let timer = cx.background_executor().timer(Duration::from_millis(1500));
        cx.spawn(async move |this, cx| {
            timer.await;
            this.update(cx, |this, cx| {
                if this
                    .compose
                    .as_ref()
                    .is_some_and(|compose| compose.save_gen == generation)
                {
                    this.save_compose(cx);
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Saves the draft now. An empty draft is never written; one that was saved and
    /// then emptied is discarded instead.
    pub(crate) fn save_compose(&mut self, cx: &mut Context<Self>) {
        let empty = self
            .compose
            .as_mut()
            .map(|compose| {
                compose.save_gen = compose.save_gen.wrapping_add(1);
                compose.is_empty(cx)
            })
            .unwrap_or(true);
        if empty {
            let id = self.compose.as_mut().and_then(|compose| compose.id.take());
            if let Some(id) = id {
                self.mail.discard_draft(id).ok();
            }
            // Keep the panel open: an emptied editor can be typed into again.
            if let Some(compose) = &mut self.compose {
                compose.dirty = false;
                compose.saved = false;
            }
            cx.notify();
            return;
        }
        let draft = self.compose.as_ref().map(|compose| compose.draft(cx));
        let Some(draft) = draft else {
            return;
        };
        match self.mail.save_draft(draft) {
            Ok(id) => {
                if let Some(compose) = &mut self.compose {
                    compose.id = Some(id);
                    compose.dirty = false;
                    compose.saved = true;
                }
            }
            Err(error) => self.toast(format!("Could not save draft: {error}"), cx),
        }
        cx.notify();
    }

    fn flush_compose(&mut self, cx: &mut Context<Self>) {
        if self.compose.as_ref().is_some_and(|compose| compose.dirty) {
            self.save_compose(cx);
            return;
        }
        if self
            .compose
            .as_ref()
            .is_some_and(|compose| compose.is_empty(cx))
        {
            self.compose = None;
        }
    }

    /// Runs when the user confirms the shared snooze picker: it either snoozes the thread
    /// or, when opened from compose, schedules the send at the chosen time.
    pub(crate) fn confirm_snooze(&mut self, at: Ms, window: &mut Window, cx: &mut Context<Self>) {
        self.overlay = Overlay::None;
        if std::mem::take(&mut self.send_later) {
            if let Some(compose) = &mut self.compose {
                compose.send_at = Some(at);
            }
            if self.compose.is_some() {
                self.send_compose(window, cx);
            }
        } else {
            self.act(MailAction::Snooze { until: at }, true, "Snoozed", cx);
        }
    }

    /// Cmd/Ctrl+Shift+Enter: pick when to send, reusing the snooze presets and parser.
    pub(crate) fn schedule_send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.compose_open {
            return;
        }
        self.send_later = true;
        self.overlay = Overlay::Snooze(crate::snooze::Snooze::new(Local::now()));
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// Throws the draft away, on the provider too when it was saved.
    pub(crate) fn discard_compose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.compose.as_mut().and_then(|compose| compose.id.take());
        self.compose = None;
        self.compose_open = false;
        let error = id.and_then(|id| self.mail.discard_draft(id).err());
        match error {
            Some(error) => self.toast(format!("Could not discard draft: {error}"), cx),
            None => self.toast("Draft discarded", cx),
        }
        window.focus(&self.focus, cx);
        cx.notify();
    }

    pub(crate) fn send_compose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.compose_open {
            return;
        }
        let Some(compose) = &self.compose else {
            return;
        };
        let draft = compose.draft(cx);
        if draft.to.is_empty() && draft.cc.is_empty() && draft.bcc.is_empty() {
            self.toast("Add a recipient", cx);
            return;
        }
        if let Some(invalid) = draft
            .to
            .iter()
            .chain(&draft.cc)
            .chain(&draft.bcc)
            .map(|addr| addr.email.trim())
            .find(|&email| !valid_email(email))
        {
            self.toast(format!("Invalid address: {invalid}"), cx);
            return;
        }
        let scheduled = draft.send_at.is_some();
        match self.mail.send(draft) {
            Ok(id) => {
                self.record_sent(id);
                self.compose = None;
                self.compose_open = false;
                window.focus(&self.focus, cx);
                self.toast(
                    if scheduled {
                        "Send scheduled"
                    } else {
                        "Sending…"
                    },
                    cx,
                );
                // The send is undoable only for the configured window.
                let delay = Duration::from_secs(self.settings.undo_send_secs as u64);
                let timer = cx.background_executor().timer(delay);
                cx.spawn(async move |this, cx| {
                    timer.await;
                    this.update(cx, |this, cx| {
                        if this.sent == Some(id) {
                            this.sent = None;
                            cx.notify();
                        }
                    })
                    .ok();
                })
                .detach();
            }
            Err(error) => self.toast(format!("Could not send: {error}"), cx),
        }
    }

    /// Reopens the draft returned by `cancel_send`, or keeps an existing draft.
    /// Returns false when a non-empty draft was already there, so the caller does
    /// not overwrite the "Draft kept" toast.
    pub(crate) fn reopen_sent(
        &mut self,
        window: &mut Window,
        draft: Draft,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.has_draft(cx) {
            self.compose_open = true;
            self.focus_compose(window, cx, false);
            self.toast("Draft kept", cx);
            cx.notify();
            return false;
        }
        self.compose = Some(Compose::from_draft(window, cx, draft));
        self.compose_open = true;
        self.focus_compose(window, cx, false);
        cx.notify();
        true
    }

    /// The expanded message of the open thread (the latest by default), or of the selected row.
    fn reply_target(&self) -> Option<(Id, Message)> {
        if let Some(open) = &self.open {
            let ix = open.expanded.min(open.messages.len().saturating_sub(1));
            return open
                .messages
                .get(ix)
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
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        // A draft picked in the Drafts mailbox needs a window to build its fields, so it
        // is opened here rather than inside `open_thread`.
        if let Some(draft) = self.open_draft.take() {
            self.open = None;
            self.compose = Some(Compose::from_draft(window, cx, draft));
            self.compose_open = true;
            self.focus_compose(window, cx, false);
        }
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
        let status = if compose.dirty {
            "Saving…"
        } else if compose.saved {
            "Saved"
        } else {
            ""
        };
        // DESIGN: a reply within a thread takes the full content pane; new mail
        // and forwards float bottom-right at 560×480.
        let is_reply = compose.reply_to.is_some();
        let reply_left = px(crate::sidebar::width(window));
        Some(
            div()
                .id("compose")
                .test_support()
                .aria_label(title)
                .key_context("KiteCompose")
                .absolute()
                .when(is_reply, |d| {
                    d.top(px(BAR_H)).left(reply_left).right_0().bottom_0()
                })
                .when(!is_reply, |d| {
                    d.bottom(px(18.))
                        .right(px(18.))
                        .w(px(560.))
                        .h(px(480.))
                        .rounded(px(RADIUS))
                        .shadow(vec![
                            BoxShadow::new(px(0.), px(20.), SHADOW.into()).blur_radius(px(60.)),
                        ])
                })
                .flex()
                .flex_col()
                .bg(t.bg2)
                .border_1()
                .border_color(t.line)
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
                                .on_click(
                                    cx.listener(|this, _, window, cx| {
                                        this.close_compose(window, cx)
                                    }),
                                )
                                .child(Icon::new(IconName::X).with_size(px(14.)).text_color(t.fg2)),
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
                .child(field_row(
                    &t,
                    "Subject",
                    &subject,
                    "compose-subject",
                    "Subject",
                ))
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
                                .flex()
                                .items_center()
                                .gap_3()
                                .child(
                                    div()
                                        .text_size(t.px(11.))
                                        .text_color(t.fg3)
                                        .child("Esc keeps this draft"),
                                )
                                .child(
                                    div()
                                        .id("compose-status")
                                        .test_support()
                                        .aria_label(status.to_string())
                                        .text_size(t.px(11.))
                                        .text_color(t.fg3)
                                        .child(status),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_3()
                                .child(
                                    div()
                                        .id("compose-discard")
                                        .test_support()
                                        .aria_label("Discard draft")
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .px_3()
                                        .py_2()
                                        .rounded(px(RADIUS_SM))
                                        .cursor_pointer()
                                        .text_size(t.px(13.))
                                        .text_color(t.fg2)
                                        .hover(|s| s.bg(t.bg3).text_color(t.danger))
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.discard_compose(window, cx)
                                        }))
                                        .child("Discard")
                                        .child(
                                            div()
                                                .text_size(t.px(11.))
                                                .text_color(t.fg3)
                                                .child("⌘⇧D"),
                                        ),
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
                                            div().text_size(t.px(11.)).text_color(t.fg).child("⌘↵"),
                                        ),
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

/// `"Doe, Jane" <jane@example.com>, other@example.com`, as the fields hold it.
fn format_addrs(addrs: &[Addr]) -> String {
    addrs.iter().map(format_addr).collect::<Vec<_>>().join(", ")
}

fn format_addr(addr: &Addr) -> String {
    let name = addr.name.trim();
    let email = addr.email.trim();
    if name.is_empty() {
        email.to_string()
    } else if name_needs_quotes(name) {
        let escaped = name.replace('\\', "\\\\").replace('"', "\\\"");
        format!("\"{escaped}\" <{email}>")
    } else {
        format!("{name} <{email}>")
    }
}

fn name_needs_quotes(name: &str) -> bool {
    name.chars().any(|c| {
        matches!(
            c,
            ',' | ';' | '<' | '>' | '"' | '@' | '\\' | '(' | ')' | ':'
        )
    })
}

/// Accepts comma or semicolon separated `Name <email>` and bare addresses,
/// splitting only outside quotes and angle brackets.
fn parse_addrs(value: &str) -> Vec<Addr> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut in_angle = false;
    for c in value.chars() {
        match c {
            '"' => {
                in_quotes = !in_quotes;
                current.push(c);
            }
            '<' if !in_quotes => {
                in_angle = true;
                current.push(c);
            }
            '>' if !in_quotes => {
                in_angle = false;
                current.push(c);
            }
            ',' | ';' if !in_quotes && !in_angle => {
                push_addr(&mut out, &current);
                current.clear();
            }
            _ => current.push(c),
        }
    }
    push_addr(&mut out, &current);
    out
}

fn push_addr(out: &mut Vec<Addr>, part: &str) {
    let part = part.trim();
    if part.is_empty() {
        return;
    }
    if let Some(open) = part.rfind('<')
        && let Some(close) = part[open..].find('>')
    {
        let email = part[open + 1..open + close].trim();
        let name = unquote_name(&part[..open]);
        out.push(Addr {
            name,
            email: email.into(),
        });
    } else {
        out.push(Addr {
            name: String::new(),
            email: part.into(),
        });
    }
}

fn unquote_name(raw: &str) -> String {
    let name = raw.trim();
    if name.len() >= 2 && name.starts_with('"') && name.ends_with('"') {
        name[1..name.len() - 1]
            .replace("\\\"", "\"")
            .replace("\\\\", "\\")
    } else {
        name.to_string()
    }
}

/// Reply-all keeps the original Cc recipients in Cc: the sender and original To
/// go to To, the original Cc stays in Cc, deduped by address and minus ourselves.
fn reply_all(message: &Message, own: &str) -> (Vec<Addr>, Vec<Addr>) {
    let mut seen = HashSet::new();
    let mut to = Vec::new();
    let mut cc = Vec::new();
    let mut push = |addr: &Addr, out: &mut Vec<Addr>| {
        let email = addr.email.trim();
        if email.is_empty() || email.eq_ignore_ascii_case(own) {
            return;
        }
        if seen.insert(email.to_ascii_lowercase()) {
            out.push(addr.clone());
        }
    };
    push(&message.from, &mut to);
    for addr in &message.to {
        push(addr, &mut to);
    }
    for addr in &message.cc {
        push(addr, &mut cc);
    }
    (to, cc)
}

/// A plain reply goes to the sender, unless I wrote the message: then it goes to the
/// people I originally addressed, deduped and without my own address.
fn reply_to(message: &Message, own: &str) -> Vec<Addr> {
    if !message.from.email.eq_ignore_ascii_case(own) {
        return vec![message.from.clone()];
    }
    let mut seen = HashSet::new();
    message
        .to
        .iter()
        .filter(|addr| {
            let email = addr.email.trim();
            !email.is_empty()
                && !email.eq_ignore_ascii_case(own)
                && seen.insert(email.to_ascii_lowercase())
        })
        .cloned()
        .collect()
}

/// Adds a prefix once: any leading `Re:`/`Fwd:`/`Fw:` chain (in any case) is replaced
/// by the single marker the action wants, so subjects never stack.
fn prefixed(subject: &str, prefix: &str) -> String {
    let mut rest = subject.trim_start();
    loop {
        let next = ["re:", "fwd:", "fw:"].iter().find_map(|marker| {
            (rest.len() >= marker.len()
                && rest.as_bytes()[..marker.len()].eq_ignore_ascii_case(marker.as_bytes()))
            .then(|| &rest[marker.len()..])
        });
        match next {
            Some(next) => rest = next.trim_start(),
            None => break,
        }
    }
    format!("{prefix}{rest}")
}

fn format_date(ms: Ms) -> String {
    Local
        .timestamp_millis_opt(ms)
        .single()
        .map(|date| date.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_default()
}

fn quote_lines(text: &str) -> String {
    text.lines()
        .map(|line| format!("> {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The original plain text under a one-line attribution.
fn quote(message: &Message) -> String {
    let date = format_date(message.date);
    let from = format_addr(&message.from);
    format!(
        "\n\nOn {date}, {from} wrote:\n{}",
        quote_lines(&message.text)
    )
}

/// A forward carries the original headers above the quoted body.
fn forward_body(message: &Message) -> String {
    let mut out = String::from("\n\n---------- Forwarded message ----------\n");
    out.push_str(&format!("From: {}\n", format_addr(&message.from)));
    out.push_str(&format!("Date: {}\n", format_date(message.date)));
    out.push_str(&format!("Subject: {}\n", message.subject));
    out.push_str(&format!("To: {}\n", format_addrs(&message.to)));
    out.push('\n');
    out.push_str(&quote_lines(&message.text));
    out
}

/// A plausible `local@domain` shape for sending; not a full RFC 5322 validator.
fn valid_email(email: &str) -> bool {
    let email = email.trim();
    let Some((local, domain)) = email.rsplit_once('@') else {
        return false;
    };
    !local.is_empty()
        && !domain.is_empty()
        && !email.contains(char::is_whitespace)
        && !local.contains('@')
        && !domain.contains('@')
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
}
