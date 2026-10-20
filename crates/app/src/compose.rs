//! Compose: a floating panel for new mail, reply, reply-all and forward.
//! Drafts save through `Mail::save_draft`: debounced while typing, and on close.
use crate::app::{Kite, Overlay};
use crate::keys;
use crate::theme::{self, BAR_H, RADIUS, RADIUS_SM, SHADOW, Theme};
use chrono::{Local, TimeZone as _};
use gpui_kit::assets::IconName;
use gpui_kit::component::Sizable;
use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
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
    /// The original message for replies and forwards (`None` for new mail).
    pub reply_to: Option<Id>,
    pub show_cc: bool,
    pub bcc: Entity<InputState>,
    show_bcc: bool,
    expanded: bool,
    recipient_errors: [Option<String>; 3],
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
        let bcc = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Bcc")
                .default_value(format_addrs(&parts.bcc))
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
        let mut subs = Vec::with_capacity(5);
        for input in [&to, &cc, &bcc, &subject] {
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
            show_bcc: !parts.bcc.is_empty(),
            bcc,
            expanded: false,
            recipient_errors: Default::default(),
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
                title: if draft.reply_to.is_some()
                    && draft.subject.to_ascii_lowercase().starts_with("fwd:")
                {
                    "Forward"
                } else {
                    "New message"
                },
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

    pub(crate) fn draft(&self, cx: &App) -> Draft {
        Draft {
            id: self.id,
            account_id: self.account_id,
            to: parse_addrs(&self.to.read(cx).value()),
            cc: parse_addrs(&self.cc.read(cx).value()),
            bcc: parse_addrs(&self.bcc.read(cx).value()),
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
            && self.bcc.read(cx).value().trim().is_empty()
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
        let reply_to = Some(message.id);
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
        compose.recipient_errors = Default::default();
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
        if !self.validate_compose_recipients(window, cx) {
            return;
        }
        self.send_later = true;
        self.overlay = Overlay::Snooze(crate::snooze::Snooze::new(Local::now()));
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// Throws the draft away, on the provider too when it was saved.
    pub(crate) fn discard_compose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.compose.as_ref().and_then(|compose| compose.id);
        if let Some(error) = id.and_then(|id| self.mail.discard_draft(id).err()) {
            self.toast(format!("Could not discard draft: {error}"), cx);
            return;
        }
        self.compose = None;
        self.compose_open = false;
        self.reload();
        self.toast("Draft discarded", cx);
        window.focus(&self.focus, cx);
        cx.notify();
    }

    pub(crate) fn send_compose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.compose_open {
            return;
        }
        if !self.validate_compose_recipients(window, cx) {
            return;
        }
        let Some(compose) = &self.compose else {
            return;
        };
        let draft = compose.draft(cx);
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

    fn validate_compose_recipients(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(compose) = &mut self.compose else {
            return false;
        };
        let draft = compose.draft(cx);
        compose.recipient_errors = Default::default();
        for (ix, (field, recipients)) in [("To", &draft.to), ("Cc", &draft.cc), ("Bcc", &draft.bcc)]
            .into_iter()
            .enumerate()
        {
            let raw = [&compose.to, &compose.cc, &compose.bcc][ix]
                .read(cx)
                .value();
            if raw.contains(['\r', '\n'])
                || recipients
                    .iter()
                    .any(|addr| !kite_core::engine::valid_recipient_address(addr))
            {
                compose.recipient_errors[ix] = Some(format!(
                    "{field}: correct the address; use name@example.com or Name <name@example.com>."
                ));
            }
        }
        if draft.to.is_empty() && draft.cc.is_empty() && draft.bcc.is_empty() {
            compose.recipient_errors[0] = Some("Add a recipient in To, Cc or Bcc.".into());
        }
        if let Some(ix) = compose.recipient_errors.iter().position(Option::is_some) {
            compose.show_cc |= ix == 1;
            compose.show_bcc |= ix == 2;
            let input = [&compose.to, &compose.cc, &compose.bcc][ix];
            window.focus(&input.read(cx).focus_handle(cx), cx);
            cx.notify();
            return false;
        }
        true
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

    /// Put the streamed answer into the draft, above anything already written so a quoted
    /// message stays below it, then forget the answer.
    pub(crate) fn insert_ai(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ai) = self.ai.take() else {
            return;
        };
        let Some(compose) = &self.compose else {
            return;
        };
        let existing = compose.body.read(cx).value().to_string();
        let text = if existing.trim().is_empty() {
            ai.text.trim_end().to_string()
        } else {
            format!("{}\n\n{existing}", ai.text.trim_end())
        };
        compose.body.update(cx, |body, cx| {
            body.set_value(text, window, cx);
        });
        self.compose_changed(cx);
        cx.notify();
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
        let bcc = compose.bcc.clone();
        let subject = compose.subject.clone();
        let body = compose.body.clone();
        let show_cc = compose.show_cc;
        let show_bcc = compose.show_bcc;
        let errors = compose.recipient_errors.clone();
        let expanded = compose.expanded;
        let title = compose.title;
        let ai = match &self.ai {
            Some(ai) if matches!(ai.target, crate::app::AiFor::Compose) => {
                Some((ai.text.clone(), ai.done))
            }
            _ => None,
        };
        let status = if compose.dirty {
            "Saving…"
        } else if compose.saved {
            "Saved"
        } else {
            ""
        };
        let pane_left = crate::sidebar::width(window);
        let pane_width = (window.viewport_size().width.as_f32() - pane_left).max(0.);
        let available_height = (window.viewport_size().height.as_f32() - BAR_H).max(0.);
        let margin = if pane_width < 400. { 4. } else { 18. };
        let dock_width = (pane_width - margin * 2.).clamp(0., 560.);
        let dock_height = (available_height - margin * 2.).clamp(0., 480.);
        Some(
            div()
                .id("compose")
                .test_support()
                .aria_label(title)
                .key_context("KiteCompose")
                .absolute()
                .when(expanded, |d| {
                    d.top(px(BAR_H)).left(px(pane_left)).right_0().bottom_0()
                })
                .when(!expanded, |d| {
                    d.bottom(px(margin))
                        .right(px(margin))
                        .w(px(dock_width))
                        .h(px(dock_height))
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
                .overflow_hidden()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .flex_shrink_0()
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
                            Button::new("compose-expand")
                                .label(if expanded { "Collapse" } else { "Expand" })
                                .accessibility_label(if expanded {
                                    "Collapse compose"
                                } else {
                                    "Expand compose"
                                })
                                .small()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(compose) = &mut this.compose {
                                        compose.expanded = !compose.expanded;
                                    }
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("compose-close")
                                .accessibility_label("Close compose")
                                .icon(IconName::X)
                                .small()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.close_compose(window, cx)
                                })),
                        ),
                )
                .child(
                    div()
                        .id("compose-editor")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .flex()
                        .flex_col()
                        .child(field_row(&t, "To", &to, "compose-to", "To"))
                        .when_some(errors[0].clone(), |d, error| {
                            d.child(recipient_error(&t, "compose-to-error", error))
                        })
                        .when(show_cc, |d| {
                            d.child(field_row(&t, "Cc", &cc, "compose-cc", "Cc"))
                                .when_some(errors[1].clone(), |d, error| {
                                    d.child(recipient_error(&t, "compose-cc-error", error))
                                })
                        })
                        .when(show_bcc, |d| {
                            d.child(field_row(&t, "Bcc", &bcc, "compose-bcc", "Bcc"))
                                .when_some(errors[2].clone(), |d, error| {
                                    d.child(recipient_error(&t, "compose-bcc-error", error))
                                })
                        })
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_end()
                                .px_4()
                                .gap_3()
                                .when(!show_bcc, |d| {
                                    d.child(
                                        Button::new("compose-show-bcc")
                                            .label("Bcc")
                                            .accessibility_label("Show Bcc")
                                            .small()
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                if let Some(compose) = &mut this.compose {
                                                    compose.show_bcc = true;
                                                }
                                                cx.notify();
                                            })),
                                    )
                                })
                                .when(!show_cc, |d| {
                                    d.child(
                                        Button::new("compose-show-cc")
                                            .label("Cc")
                                            .accessibility_label("Show Cc")
                                            .small()
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                if let Some(compose) = &mut this.compose {
                                                    compose.show_cc = true;
                                                }
                                                cx.notify();
                                            })),
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
                        .when_some(ai, |d, (text, done)| {
                            d.child(
                                div()
                                    .id("compose-ai")
                                    .test_support()
                                    .aria_label(text.clone())
                                    .mx_4()
                                    .mb_2()
                                    .p_3()
                                    .rounded(px(RADIUS_SM))
                                    .bg(t.bg3)
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_size(t.px(11.))
                                            .font_weight(FontWeight(650.))
                                            .text_color(t.fg2)
                                            .child(if done { "AI draft" } else { "Writing…" }),
                                    )
                                    .child(div().text_size(t.px(13.)).child(text))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .child(
                                                crate::preferences::button(
                                                    "insert-ai",
                                                    "Insert",
                                                    t,
                                                )
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.insert_ai(window, cx)
                                                })),
                                            )
                                            .child(
                                                crate::preferences::button(
                                                    "discard-ai",
                                                    "Discard",
                                                    t,
                                                )
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.ai = None;
                                                    cx.notify();
                                                })),
                                            ),
                                    ),
                            )
                        })
                        .child(
                            div()
                                .id("compose-body")
                                .test_support()
                                .aria_label("Message")
                                .flex_1()
                                .min_h(px(120.))
                                .flex_shrink_0()
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
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_shrink_0()
                        .gap_2()
                        .px_4()
                        .py_3()
                        .border_t_1()
                        .border_color(t.line)
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_3()
                                .child(div().text_size(t.px(11.)).text_color(t.fg3).child(
                                    match keys::hint_for(&self.settings, keys::cmd::COMPOSE_CLOSE) {
                                        hint if hint.is_empty() => {
                                            "Closing keeps this draft".to_string()
                                        }
                                        hint => format!("{hint} keeps this draft"),
                                    },
                                ))
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
                                    Button::new("compose-discard")
                                        .label("Discard")
                                        .tooltip(format!(
                                            "Discard - {}",
                                            keys::hint_for(&self.settings, keys::cmd::DISCARD)
                                        ))
                                        .accessibility_label("Discard draft")
                                        .small()
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.discard_compose(window, cx)
                                        })),
                                )
                                .child(
                                    Button::new("compose-send-later")
                                        .label("Send later")
                                        .accessibility_label("Send later")
                                        .small()
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.schedule_send(window, cx)
                                        })),
                                )
                                .child(
                                    Button::new("compose-send")
                                        .label("Send")
                                        .tooltip(format!(
                                            "Send - {}",
                                            keys::hint_for(&self.settings, keys::cmd::SEND)
                                        ))
                                        .accessibility_label("Send")
                                        .small()
                                        .bg(theme::accent_glow(&t))
                                        .text_color(t.fg)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.send_compose(window, cx)
                                        })),
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
        .flex_shrink_0()
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
    let mut escaped = false;
    for c in value.chars() {
        if escaped {
            current.push(c);
            escaped = false;
            continue;
        }
        if c == '\\' && in_quotes {
            current.push(c);
            escaped = true;
            continue;
        }
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
        && part[open + close + 1..].trim().is_empty()
        && !part[open + 1..open + close].contains(['<', '>'])
        && valid_display_name(part[..open].trim())
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

fn valid_display_name(name: &str) -> bool {
    if name.chars().any(char::is_control) {
        return false;
    }
    if !name.starts_with('"') {
        return !name.contains(['"', '<', '>']);
    }
    let mut escaped = false;
    for (ix, c) in name.char_indices().skip(1) {
        if escaped {
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == '"' {
            return ix + 1 == name.len();
        }
    }
    false
}

fn recipient_error(t: &Theme, id: &'static str, error: String) -> AnyElement {
    div()
        .id(id)
        .test_support()
        .aria_label(error.clone())
        .px_4()
        .py_1()
        .flex_shrink_0()
        .text_size(t.px(12.))
        .text_color(t.danger)
        .child(error)
        .into_any_element()
}
