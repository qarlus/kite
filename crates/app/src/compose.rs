//! Compose: a floating panel for new mail, reply, reply-all and forward.
//! Drafts save through `Mail::save_draft`: debounced while typing, and on close.
use crate::app::{Kite, Overlay};
use crate::keys;
use crate::theme::{self, BAR_H, RADIUS, RADIUS_SM, SHADOW, Theme};
use chrono::{Local, TimeZone as _};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Icon, Sizable};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kite_core::model::{
    Account, Action as MailAction, Addr, Draft, Id, Message, Ms, Provider, Snippet,
};
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
    /// The signature block this draft carries ("\n\n-- \n…"), so switching From swaps
    /// it only while its text is untouched. Empty means no swappable signature.
    signature: String,
    /// Unsaved edits exist since the last successful save.
    dirty: bool,
    /// The draft has been saved at least once, so the panel can say "Saved".
    saved: bool,
    /// Bumped on every edit and save, so a stale debounce cannot save newer state.
    save_gen: u64,
    /// The open snippet picker, if any.
    snippet: Option<Snippets>,
    /// A `;` trigger the user escaped out of, so the same anchor does not reopen.
    snippet_dismissed: Option<usize>,
    /// Focus the picker takes while it is open, so its arrows never edit the draft.
    snippet_focus: FocusHandle,
    _subs: Vec<Subscription>,
}

/// The inline snippet picker: opened by `;` at a word start or by "Insert snippet…".
pub struct Snippets {
    /// Byte offset of the `;` in the body. `None` when opened from the command, in
    /// which case Enter inserts at the caret rather than replacing `;query`.
    anchor: Option<usize>,
    /// What filters the list: the text after `;`, or keystrokes in command mode.
    query: String,
    /// Selected row within the filtered list.
    sel: usize,
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
    /// The signature block tracked in `body` (already included when composing fresh).
    signature: String,
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
            cx.subscribe_in(&body, window, |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.compose_changed(cx);
                    this.snippet_scan(window, cx);
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
            signature: parts.signature,
            title: parts.title,
            dirty: false,
            saved: parts.id.is_some(),
            save_gen: 0,
            snippet: None,
            snippet_dismissed: None,
            snippet_focus: cx.focus_handle(),
            _subs: subs,
        }
    }

    fn from_draft(
        window: &mut Window,
        cx: &mut Context<Kite>,
        draft: Draft,
        signature: String,
    ) -> Self {
        Self::new(
            window,
            cx,
            Parts {
                signature,
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
        let body = self.body.read(cx).value();
        // A draft holding only its signature has nothing the user wrote yet.
        let empty_body = body.trim().is_empty()
            || (!self.signature.is_empty() && body.trim_end() == self.signature.trim_end());
        self.to.read(cx).value().trim().is_empty()
            && self.cc.read(cx).value().trim().is_empty()
            && self.subject.read(cx).value().trim().is_empty()
            && empty_body
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
            let signature = signature_block(&self.settings, account_id);
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
                    body: signature.clone(),
                    signature,
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

    /// A fresh draft addressed to `addr`, docked like `c` — the contact card's
    /// "Email" action uses it, keeping the open conversation readable.
    pub(crate) fn compose_address(
        &mut self,
        addr: Addr,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.compose.is_none() {
            let account_id = self
                .open
                .as_ref()
                .map(|open| open.account_id)
                .or(self.view.account_id)
                .or_else(|| self.accounts.first().map(|account| account.id))
                .unwrap_or(0);
            let signature = signature_block(&self.settings, account_id);
            self.compose = Some(Compose::new(
                window,
                cx,
                Parts {
                    account_id,
                    id: None,
                    title: "New message",
                    to: vec![addr],
                    cc: Vec::new(),
                    bcc: Vec::new(),
                    subject: String::new(),
                    body: signature.clone(),
                    signature,
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
        let (mut to, mut cc) = if forward {
            (Vec::new(), Vec::new())
        } else if all {
            reply_all(&message, &own)
        } else {
            (reply_to(&message, &own), Vec::new())
        };
        if self.view.mailbox == kite_core::model::Mailbox::Waiting {
            let is_own = |address: &Addr| {
                self.accounts
                    .iter()
                    .any(|a| a.email.eq_ignore_ascii_case(&address.email))
            };
            to.retain(|a| !is_own(a));
            cc.retain(|a| !is_own(a));
        }
        let reply_to = Some(message.id);
        let signature = signature_block(&self.settings, account_id);
        let body = signature.clone()
            + &if forward {
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
                signature,
                reply_to,
                send_at: None,
                remind_at: None,
            },
        ));
        self.compose_open = true;
        self.focus_compose(window, cx, forward);
        cx.notify();
    }

    /// A suggestion chip opens a reply with its text on top, above the signature and
    /// the quote, editable like anything typed. A kept draft is never clobbered.
    pub(crate) fn reply_with_suggestion(
        &mut self,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.has_draft(cx) {
            self.compose_open = true;
            self.focus_compose(window, cx, false);
            self.toast("Draft kept", cx);
            return;
        }
        self.compose_reply(window, cx, false, false);
        let Some(compose) = &self.compose else {
            return;
        };
        let existing = compose.body.read(cx).value().to_string();
        let text = format!("{}\n\n{}", text.trim_end(), existing);
        compose.body.update(cx, |body, cx| {
            body.set_value(text, window, cx);
        });
        self.compose_changed(cx);
        self.ai = None;
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
        if let Some(id) = self.reschedule.take() {
            self.send_later = false;
            self.change_schedule(id, Some(Some(at)), false, cx);
            return;
        }
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
                self.toast_undo(
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
        let signature = signature_block(&self.settings, draft.account_id);
        self.compose = Some(Compose::from_draft(window, cx, draft, signature));
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

    /// Picks the account the draft is sent from; the recipients and body stay untouched,
    /// except an untouched signature, which swaps to the new account's.
    pub fn set_compose_account(
        &mut self,
        account: Id,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let changed = match &mut self.compose {
            Some(compose) if compose.account_id != account => {
                compose.account_id = account;
                true
            }
            _ => false,
        };
        if !changed {
            return;
        }
        let old = std::mem::replace(
            &mut self.compose.as_mut().unwrap().signature,
            signature_block(&self.settings, account),
        );
        if let Some(compose) = &self.compose {
            let body = compose.body.read(cx).value().to_string();
            if !old.is_empty() && body.contains(&old) {
                let next = body.replacen(&old, &compose.signature, 1);
                compose.body.update(cx, |body, cx| {
                    body.set_value(next, window, cx);
                });
            }
        }
        self.compose_changed(cx);
    }

    /// Reveals a recipient field from the To row's quiet toggle, with the caret in it.
    fn reveal_recipient(&mut self, field: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(compose) = &mut self.compose else {
            return;
        };
        let input = if field == 1 {
            compose.show_cc = true;
            compose.cc.clone()
        } else {
            compose.show_bcc = true;
            compose.bcc.clone()
        };
        let handle = input.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        cx.notify();
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

    // ---- snippets ---------------------------------------------------------------

    /// A `;` typed at a word start opens the picker; what follows it is the
    /// query. The picker takes focus, so its arrows and Enter never reach the
    /// draft while the query text it will replace still lives in the body.
    fn snippet_scan(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(compose) = &mut self.compose else {
            return;
        };
        if self.settings.snippets.is_empty() || compose.snippet.is_some() {
            return;
        }
        let (cursor, text) = {
            let body = compose.body.read(cx);
            (body.cursor(), body.value().to_string())
        };
        let Some(anchor) = snippet_anchor(&text, cursor) else {
            compose.snippet_dismissed = None;
            return;
        };
        if compose.snippet_dismissed == Some(anchor) {
            return;
        }
        compose.snippet = Some(Snippets {
            anchor: Some(anchor),
            query: text[anchor + 1..cursor].to_string(),
            sel: 0,
        });
        let focus = compose.snippet_focus.clone();
        window.focus(&focus, cx);
        cx.notify();
    }

    /// "Insert snippet…" (palette / `Ctrl+;`): the picker opens with no `;` anchor, so
    /// the chosen body is inserted at the caret rather than replacing trigger text.
    pub(crate) fn open_snippets(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.compose_open {
            return;
        }
        let Some(compose) = &mut self.compose else {
            return;
        };
        compose.snippet = Some(Snippets {
            anchor: None,
            query: String::new(),
            sel: 0,
        });
        compose.snippet_dismissed = None;
        let focus = compose.snippet_focus.clone();
        window.focus(&focus, cx);
        cx.notify();
    }

    /// Whether a snippet picker is open, so `run` routes Escape and the confirm
    /// and navigation commands to it instead of the mail list.
    pub(crate) fn snippet_open(&self) -> bool {
        self.compose
            .as_ref()
            .is_some_and(|compose| compose.snippet.is_some())
    }

    /// Close the picker and hand the body its caret back. `dismiss` marks an
    /// Escape, so the same `;` does not immediately reopen while typing continues.
    pub(crate) fn close_snippets(
        &mut self,
        dismiss: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(compose) = &mut self.compose else {
            return;
        };
        if dismiss && let Some(anchor) = compose.snippet.as_ref().and_then(|s| s.anchor) {
            compose.snippet_dismissed = Some(anchor);
        }
        compose.snippet = None;
        let handle = compose.body.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        cx.notify();
    }

    /// Key events while the picker holds focus: unbound keys land here (arrows,
    /// Enter, Backspace, characters without a command). Bound single keys are
    /// actions instead and arrive through [`Kite::snippet_act`]. Returns whether
    /// the picker is open and consumed the key.
    fn snippet_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(compose) = &self.compose else {
            return false;
        };
        if compose.snippet.is_none() {
            return false;
        }
        match event.keystroke.key.as_str() {
            "up" => self.snippet_move(-1, cx),
            "down" => self.snippet_move(1, cx),
            "enter" => {
                let row = compose.snippet.as_ref().map(|s| s.sel).unwrap_or(0);
                self.insert_snippet_at(row, window, cx);
            }
            "escape" => self.close_snippets(true, window, cx),
            "backspace" => self.snippet_backspace(window, cx),
            _ => {
                if let Some(c) = keystroke_char(&event.keystroke) {
                    self.snippet_type(&c.to_string(), window, cx);
                }
            }
        }
        true
    }

    /// Actions that arrive while the picker holds focus: it owns the keyboard, so
    /// every command is swallowed. Escape closes, and a bound single character
    /// (`e`, `s`, `x`…) is the letter the user meant to type into the query.
    fn snippet_act(&mut self, action: &keys::Act, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .compose
            .as_ref()
            .is_none_or(|compose| compose.snippet.is_none())
        {
            return;
        }
        match action.cmd {
            keys::cmd::COMPOSE_CLOSE | keys::cmd::BACK => self.close_snippets(true, window, cx),
            id => {
                let typed = keys::command(id)
                    .and_then(|command| keys::bound(&self.settings, command).into_iter().next())
                    .and_then(|key| Keystroke::parse(&key).ok())
                    .and_then(|stroke| keystroke_char(&stroke));
                if let Some(c) = typed {
                    self.snippet_type(&c.to_string(), window, cx);
                }
            }
        }
        cx.stop_propagation();
    }

    /// Enter (the OPEN command) inserts the selected row, whichever mode opened
    /// the picker.
    pub(crate) fn snippet_confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let row = self
            .compose
            .as_ref()
            .and_then(|compose| compose.snippet.as_ref())
            .map(|picker| picker.sel)
            .unwrap_or(0);
        self.insert_snippet_at(row, window, cx);
    }

    pub(crate) fn snippet_move(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(compose) = &mut self.compose else {
            return;
        };
        let Some(picker) = &mut compose.snippet else {
            return;
        };
        let count = snippet_hits(&self.settings.snippets, &picker.query).len();
        if count > 0 {
            picker.sel = (picker.sel as isize + delta).clamp(0, count as isize - 1) as usize;
        }
        cx.notify();
    }

    /// A query character: it filters the list, and for a `;` trigger it is also
    /// the `;query` text living in the body, so it is appended after the anchor.
    fn snippet_type(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(compose) = &mut self.compose else {
            return;
        };
        let Some(picker) = &mut compose.snippet else {
            return;
        };
        // Whitespace ends the `;word`: keep it after the trigger text and close.
        if text.chars().all(char::is_whitespace) {
            if let Some(anchor) = picker.anchor {
                let at = anchor + 1 + picker.query.len();
                compose.body.update(cx, |body, cx| {
                    body.set_selected_range(at..at, cx);
                    body.insert(text.to_string(), window, cx);
                });
            }
            self.close_snippets(true, window, cx);
            return;
        }
        picker.query.push_str(text);
        picker.sel = 0;
        if let Some(anchor) = picker.anchor {
            let at = anchor + 1 + picker.query.len() - text.len();
            compose.body.update(cx, |body, cx| {
                body.set_selected_range(at..at, cx);
                body.insert(text.to_string(), window, cx);
            });
        }
        cx.notify();
    }

    fn snippet_backspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(compose) = &mut self.compose else {
            return;
        };
        let Some(picker) = &mut compose.snippet else {
            return;
        };
        let anchor = picker.anchor;
        match picker.query.pop() {
            Some(c) => {
                if let Some(anchor) = anchor {
                    let at = anchor + 1 + picker.query.len();
                    compose.body.update(cx, |body, cx| {
                        body.set_selected_range(at..at + c.len_utf8(), cx);
                        body.replace("", window, cx);
                    });
                }
                cx.notify();
            }
            // An empty query backspaces the `;` itself, which also ends the trigger.
            None => {
                if let Some(anchor) = anchor {
                    compose.body.update(cx, |body, cx| {
                        body.set_selected_range(anchor..anchor + 1, cx);
                        body.replace("", window, cx);
                    });
                }
                self.close_snippets(false, window, cx);
            }
        }
    }

    /// Insert the snippet at `row` in the filtered list: expand its variables,
    /// lift a `Subject:` line into an empty subject field, replace the `;query`
    /// text (or insert at the caret) and select the first variable left unfilled.
    fn insert_snippet_at(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(compose) = &self.compose else {
            return;
        };
        let Some(picker) = &compose.snippet else {
            return;
        };
        let hits = snippet_hits(&self.settings.snippets, &picker.query);
        let Some(snippet) = hits.get(row).map(|ix| self.settings.snippets[*ix].clone()) else {
            return;
        };
        let anchor = picker.anchor;
        let query_len = picker.query.len();
        let to = parse_addrs(&compose.to.read(cx).value());
        let account = self
            .accounts
            .iter()
            .find(|account| account.id == compose.account_id);
        let (subject, text) = expand_snippet(&snippet.body, to.first(), account, &Local::now());
        if let Some(subject) = subject
            && compose.subject.read(cx).value().trim().is_empty()
        {
            compose
                .subject
                .update(cx, |input, cx| input.set_value(subject, window, cx));
        }
        compose.body.update(cx, |body, cx| {
            if let Some(anchor) = anchor {
                body.set_selected_range(anchor..anchor + 1 + query_len, cx);
            }
            body.replace(text.clone(), window, cx);
            if let Some(range) = leftover_variable(&body.value()) {
                body.set_selected_range(range, cx);
            }
        });
        let compose = self.compose.as_mut().unwrap();
        compose.snippet = None;
        let handle = compose.body.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        cx.notify();
    }

    /// The picker panel under the body: the matching snippets with their `;` shortcut.
    /// It takes focus while open so its arrows and Enter never reach the draft.
    fn render_snippets(
        &self,
        picker: &Snippets,
        focus: &FocusHandle,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let hits = snippet_hits(&self.settings.snippets, &picker.query);
        let rows: Vec<AnyElement> = hits
            .iter()
            .take(8)
            .enumerate()
            .map(|(row, ix)| {
                let snippet = &self.settings.snippets[*ix];
                let selected = row == picker.sel;
                let preview = snippet.body.lines().next().unwrap_or("").to_string();
                div()
                    .id(("snippet-row", row))
                    .test_support()
                    .aria_label(snippet.name.clone())
                    .aria_selected(selected)
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.insert_snippet_at(row, window, cx)
                    }))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .px_3()
                    .py_2()
                    .when(selected, |d| d.bg(t.sel))
                    .when(!selected, |d| d.hover(|s| s.bg(t.bg3)))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .min_w_0()
                            .child(
                                div()
                                    .text_size(t.px(13.))
                                    .when(!selected, |d| d.text_color(t.fg))
                                    .child(snippet.name.clone()),
                            )
                            .child(
                                div()
                                    .text_size(t.px(11.))
                                    .text_color(t.fg3)
                                    .child(format!(";{}", snippet.shortcut)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(t.px(12.))
                                    .text_color(t.fg2)
                                    .child(preview),
                            ),
                    )
                    .into_any_element()
            })
            .collect();
        div()
            .id("snippet-picker")
            .test_support()
            .aria_label("Insert snippet")
            .track_focus(focus)
            // Actions are intercepted here, on the focused picker, not on the compose
            // panel: an Act listener on a node that always exists would shadow the
            // inputs' own dispatch and break typing.
            .on_action(cx.listener(|this, action: &keys::Act, window, cx| {
                this.snippet_act(action, window, cx);
            }))
            .on_key_down(cx.listener(|this, event, window, cx| {
                if this.snippet_key(event, window, cx) {
                    cx.stop_propagation();
                }
            }))
            .flex_shrink_0()
            .max_h(px(320.))
            .overflow_y_hidden()
            .mx_4()
            .mb_2()
            .rounded(px(RADIUS_SM))
            .border_1()
            .border_color(t.line)
            .bg(t.bg)
            .shadow(vec![
                BoxShadow::new(px(0.), px(8.), SHADOW.into()).blur_radius(px(30.)),
            ])
            .py_1()
            .flex()
            .flex_col()
            .children(rows)
            .when(hits.is_empty(), |d| {
                d.child(
                    div()
                        .px_3()
                        .py_4()
                        .text_size(t.px(12.))
                        .text_color(t.fg2)
                        .child("No matching snippets — create them in Settings › Snippets"),
                )
            })
            .into_any_element()
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
            let signature = signature_block(&self.settings, draft.account_id);
            self.compose = Some(Compose::from_draft(window, cx, draft, signature));
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
        let from_id = compose.account_id;
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
        let (send_ink, send_hover) = send_colors(&t);
        let discard_tip = format!(
            "Discard - {}",
            keys::hint_for(&self.settings, keys::cmd::DISCARD)
        );
        let later_tip = action_tip(
            "Send later",
            keys::hint_for(&self.settings, keys::cmd::SEND_LATER),
        );
        let send_tip = action_tip("Send", keys::hint_for(&self.settings, keys::cmd::SEND));
        let snippets = compose
            .snippet
            .as_ref()
            .map(|picker| self.render_snippets(picker, &compose.snippet_focus, &t, cx));
        let pane_left = crate::sidebar::width(window);
        let pane_width = (window.viewport_size().width.as_f32() - pane_left).max(0.);
        let available_height = (window.viewport_size().height.as_f32() - BAR_H).max(0.);
        let margin = if pane_width < 400. { 4. } else { 18. };
        let dock_width = (pane_width - margin * 2.).clamp(0., 560.);
        let dock_height = (available_height - margin * 2.).clamp(0., 480.);
        Some(
            div()
                .id("compose")
                // A floating panel owns every mouse event inside it: without this, a
                // click on its padding or a gap reaches the list row underneath.
                .occlude()
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
                            div()
                                .flex()
                                .items_center()
                                .gap_1()
                                .child(
                                    crate::app::strip_chip(&t, 26.)
                                        .id("compose-expand")
                                        .test_support()
                                        .aria_label(if expanded {
                                            "Collapse compose"
                                        } else {
                                            "Expand compose"
                                        })
                                        .text_color(t.fg2)
                                        .tooltip(move |window, cx| {
                                            Tooltip::new(if expanded {
                                                "Collapse"
                                            } else {
                                                "Expand"
                                            })
                                            .build(window, cx)
                                        })
                                        .child(
                                            Icon::new(if expanded {
                                                IconName::Minimize2
                                            } else {
                                                IconName::Maximize2
                                            })
                                            .with_size(px(15.)),
                                        )
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            if let Some(compose) = &mut this.compose {
                                                compose.expanded = !compose.expanded;
                                            }
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    crate::app::strip_chip(&t, 26.)
                                        .id("compose-close")
                                        .test_support()
                                        .aria_label("Close compose")
                                        .text_color(t.fg2)
                                        .child(Icon::new(IconName::X).with_size(px(15.)))
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.close_compose(window, cx)
                                        })),
                                ),
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
                        .when_some(
                            from_row(&t, self.mail.as_ref(), &self.accounts, from_id, cx),
                            |d, row| d.child(row),
                        )
                        .child(field_row(
                            &t,
                            "To",
                            &to,
                            "compose-to",
                            "To",
                            recipient_toggles(&t, show_cc, show_bcc, cx),
                        ))
                        .when_some(errors[0].clone(), |d, error| {
                            d.child(recipient_error(&t, "compose-to-error", error))
                        })
                        .when(show_cc, |d| {
                            d.child(field_row(&t, "Cc", &cc, "compose-cc", "Cc", None))
                                .when_some(errors[1].clone(), |d, error| {
                                    d.child(recipient_error(&t, "compose-cc-error", error))
                                })
                        })
                        .when(show_bcc, |d| {
                            d.child(field_row(&t, "Bcc", &bcc, "compose-bcc", "Bcc", None))
                                .when_some(errors[2].clone(), |d, error| {
                                    d.child(recipient_error(&t, "compose-bcc-error", error))
                                })
                        })
                        .child(field_row(
                            &t,
                            "Subject",
                            &subject,
                            "compose-subject",
                            "Subject",
                            None,
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
                                    // The textarea wrapper sizes itself to its content (one row),
                                    // so it must be told to take the whole body area: percentage
                                    // heights are refined last, over the component's own height.
                                    Textarea::new(&body)
                                        .aria_label("Message")
                                        .appearance(false)
                                        .bordered(false)
                                        .text_size(t.px(14.))
                                        .text_color(t.fg)
                                        .h_full(),
                                ),
                        ),
                )
                .when_some(snippets, |d, picker| d.child(picker))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_4()
                        .flex_shrink_0()
                        .px_4()
                        .py_3()
                        .border_t_1()
                        .border_color(t.line)
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .min_w_0()
                                .child(
                                    div()
                                        .truncate()
                                        .text_size(t.px(11.))
                                        .text_color(t.fg3)
                                        .child(
                                            match keys::hint_for(
                                                &self.settings,
                                                keys::cmd::COMPOSE_CLOSE,
                                            ) {
                                                hint if hint.is_empty() => {
                                                    "Closing keeps this draft".to_string()
                                                }
                                                hint => format!("{hint} keeps this draft"),
                                            },
                                        ),
                                )
                                .child(
                                    div()
                                        .id("compose-status")
                                        .test_support()
                                        .aria_label(status.to_string())
                                        .flex_shrink_0()
                                        .text_size(t.px(11.))
                                        .text_color(t.fg3)
                                        .child(status),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .flex_shrink_0()
                                .child(
                                    crate::app::strip_chip(&t, 32.)
                                        .id("compose-discard")
                                        .test_support()
                                        .aria_label("Discard draft")
                                        .text_color(t.fg2)
                                        .tooltip(move |window, cx| {
                                            Tooltip::new(discard_tip.clone()).build(window, cx)
                                        })
                                        .child(Icon::new(IconName::Trash).with_size(px(16.)))
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.discard_compose(window, cx)
                                        })),
                                )
                                .child(
                                    crate::app::strip_chip(&t, 32.)
                                        .id("compose-send-later")
                                        .test_support()
                                        .aria_label("Send later")
                                        .text_color(t.fg2)
                                        .tooltip(move |window, cx| {
                                            Tooltip::new(later_tip.clone()).build(window, cx)
                                        })
                                        .child(Icon::new(IconName::Clock).with_size(px(16.)))
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.schedule_send(window, cx)
                                        })),
                                )
                                .child(
                                    // The accent fill lives on the wrapper: hovering paints a
                                    // tint over it rather than swapping the colour out.
                                    div()
                                        .flex()
                                        .flex_shrink_0()
                                        .rounded(px(RADIUS_SM))
                                        .bg(t.accent)
                                        .child(
                                            div()
                                                .id("compose-send")
                                                .test_support()
                                                .aria_label("Send")
                                                .size(px(32.))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .rounded(px(RADIUS_SM))
                                                .cursor_pointer()
                                                .text_color(send_ink)
                                                .hover(move |s| s.bg(send_hover))
                                                .tooltip(move |window, cx| {
                                                    Tooltip::new(send_tip.clone()).build(window, cx)
                                                })
                                                .child(Icon::new(IconName::Send).with_size(px(16.)))
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.send_compose(window, cx)
                                                })),
                                        ),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }
}

/// One compose field: the shared label column, then the caller's content.
fn field(t: &Theme, label: &'static str, content: AnyElement) -> AnyElement {
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
        .child(content)
        .into_any_element()
}

fn field_row(
    t: &Theme,
    label: &'static str,
    input: &Entity<InputState>,
    id: &'static str,
    aria: &'static str,
    trailing: Option<AnyElement>,
) -> AnyElement {
    field(
        t,
        label,
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div().flex_1().min_w_0().child(
                    Input::new(input)
                        .id(id)
                        .aria_label(aria)
                        .appearance(false)
                        .bordered(false)
                        .text_size(t.px(14.))
                        .text_color(t.fg)
                        .w_full(),
                ),
            )
            .when_some(trailing, |d, trailing| d.child(trailing))
            .into_any_element(),
    )
}

/// The signature block a draft for `account` starts with: a blank line under the
/// typing point, then the standard "-- " separator, then the text. Empty adds nothing.
fn signature_block(settings: &kite_core::model::Settings, account: Id) -> String {
    let signature = settings
        .profiles
        .get(&account)
        .map(|profile| profile.signature.trim_end())
        .unwrap_or_default();
    if signature.trim().is_empty() {
        String::new()
    } else {
        format!("\n\n-- \n{signature}")
    }
}

/// The sending identity: a dropdown over every account, or plain text when there is only one.
fn from_row(
    t: &Theme,
    mail: &dyn kite_core::api::Mail,
    accounts: &[Account],
    from: Id,
    cx: &Context<Kite>,
) -> Option<AnyElement> {
    let account = accounts.iter().find(|account| account.id == from)?;
    let identity = from_identity(account);
    let address = account.email.clone();
    let picture = crate::sidebar::avatar_image(mail, from).map(|image| {
        img(image)
            .size(px(18.))
            .rounded_full()
            .flex_shrink_0()
            .into_any_element()
    });
    let content = if accounts.len() > 1 {
        let entity = cx.entity().downgrade();
        let choices: Vec<(Id, String)> = accounts
            .iter()
            .map(|account| (account.id, from_choice(account)))
            .collect();
        let current = account.id;
        Button::new("compose-from")
            .label(identity)
            .dropdown_caret(true)
            .ghost()
            .accessibility_label(address)
            .dropdown_menu(move |menu, _, _| {
                let mut menu = menu.min_w(px(280.));
                for (id, label) in &choices {
                    let entity = entity.clone();
                    let id = *id;
                    menu = menu.item(
                        PopupMenuItem::new(label.clone())
                            .checked(id == current)
                            .on_click(move |_, window, cx| {
                                let _ = entity.update(cx, |kite, cx| {
                                    kite.set_compose_account(id, window, cx)
                                });
                            }),
                    );
                }
                menu
            })
            .into_any_element()
    } else {
        div()
            .id("compose-from")
            .test_support()
            .aria_label(address)
            .flex()
            .items_center()
            .h(px(32.))
            .min_w_0()
            .truncate()
            .text_size(t.px(14.))
            .text_color(t.fg)
            .child(identity)
            .into_any_element()
    };
    let content = if let Some(picture) = picture {
        div()
            .flex()
            .items_center()
            .gap_2()
            .min_w_0()
            .child(picture)
            .child(content)
            .into_any_element()
    } else {
        content
    };
    Some(field(t, "From", content))
}

/// `Name <email>`, or just the address when the account has no name.
fn from_identity(account: &Account) -> String {
    let name = account.name.trim();
    if name.is_empty() {
        account.email.clone()
    } else {
        format!("{name} <{}>", account.email)
    }
}

/// The menu row for an account: who it is, then the provider that carries it.
fn from_choice(account: &Account) -> String {
    let provider = match account.provider {
        Provider::Proton => "Proton Mail",
        Provider::Microsoft => "Microsoft Outlook",
    };
    format!("{} · {provider}", from_identity(account))
}

/// The quiet `Cc` / `Bcc` toggles that close the To row, gone once their field shows.
fn recipient_toggles(
    t: &Theme,
    show_cc: bool,
    show_bcc: bool,
    cx: &mut Context<Kite>,
) -> Option<AnyElement> {
    if show_cc && show_bcc {
        return None;
    }
    let cc = cx.listener(|this, _, window, cx| this.reveal_recipient(1, window, cx));
    let bcc = cx.listener(|this, _, window, cx| this.reveal_recipient(2, window, cx));
    Some(
        div()
            .flex()
            .items_center()
            .gap_3()
            .when(!show_cc, |d| {
                d.child(recipient_toggle(t, "compose-show-cc", "Cc", cc))
            })
            .when(!show_bcc, |d| {
                d.child(recipient_toggle(t, "compose-show-bcc", "Bcc", bcc))
            })
            .into_any_element(),
    )
}

fn recipient_toggle(
    t: &Theme,
    id: &'static str,
    label: &'static str,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .test_support()
        .aria_label(format!("Show {label}"))
        .role(Role::Button)
        .flex_shrink_0()
        .cursor_pointer()
        .text_size(t.px(12.))
        .text_color(t.fg3)
        .hover(|s| s.text_color(t.fg))
        .on_click(on_click)
        .child(label)
        .into_any_element()
}

/// `"Send · Ctrl+Enter"` — the action and its bound key, or just the action when
/// the command is unbound.
fn action_tip(action: &'static str, hint: String) -> String {
    if hint.is_empty() {
        action.to_string()
    } else {
        format!("{action} · {hint}")
    }
}

/// The ink that reads against the accent, and the veil that hovers over it: the
/// tint moves away from the ink, so the label keeps its contrast on hover.
fn send_colors(t: &Theme) -> (Hsla, Hsla) {
    let (ink, away) = if theme::contrast(t.fg, t.accent) > theme::contrast(t.bg, t.accent) {
        (t.fg, t.bg)
    } else {
        (t.bg, t.fg)
    };
    (ink.into(), Theme::alpha(away, 0.12))
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

/// The `;` a picker can open on: at the text start or right after whitespace, with
/// only non-whitespace (the query) between it and the caret. Returns its byte offset.
fn snippet_anchor(text: &str, cursor: usize) -> Option<usize> {
    if !text.is_char_boundary(cursor) {
        return None;
    }
    let before = &text[..cursor];
    let anchor = before.rfind(';')?;
    if before[anchor + 1..].chars().any(char::is_whitespace) {
        return None;
    }
    if anchor > 0 && !before[..anchor].chars().next_back()?.is_whitespace() {
        return None;
    }
    Some(anchor)
}

/// Snippets matching the query, best first: a fuzzy hit on the name or the shortcut.
fn snippet_hits(snippets: &[Snippet], query: &str) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    let mut scored: Vec<(i32, usize)> = snippets
        .iter()
        .enumerate()
        .filter_map(|(ix, snippet)| {
            let score = if query.is_empty() {
                Some(0)
            } else {
                crate::palette::fuzzy(&query, &snippet.name)
                    .into_iter()
                    .chain(crate::palette::fuzzy(&query, &snippet.shortcut))
                    .max()
            }?;
            Some((score, ix))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, ix)| ix).collect()
}

/// The character a keystroke would type, if it types one.
fn keystroke_char(stroke: &Keystroke) -> Option<char> {
    if stroke.modifiers.control
        || stroke.modifiers.alt
        || stroke.modifiers.platform
        || stroke.modifiers.function
    {
        return None;
    }
    if let Some(c) = stroke.key_char.as_deref().and_then(|c| c.chars().next()) {
        return Some(c);
    }
    if stroke.key.chars().count() != 1 {
        return None;
    }
    let c = stroke.key.chars().next()?;
    Some(if stroke.modifiers.shift {
        c.to_ascii_uppercase()
    } else {
        c
    })
}

/// The first `{name}` a snippet body could not fill, for the caret to land on.
fn leftover_variable(text: &str) -> Option<std::ops::Range<usize>> {
    for (start, _) in text.match_indices('{') {
        if let Some(close) = text[start..].find('}') {
            let name = &text[start + 1..start + close];
            if !name.is_empty()
                && name.len() <= 24
                && name.chars().all(|c| c.is_ascii_lowercase() || c == '_')
            {
                return Some(start..start + close + 1);
            }
        }
    }
    None
}

fn capitalise(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

fn local_part(email: &str) -> &str {
    email.split('@').next().unwrap_or(email)
}

/// `{name}`/`{first_name}`: the display name's words, or the email local part.
fn recipient_name(addr: &Addr) -> Option<String> {
    let name = addr.name.trim();
    if !name.is_empty() {
        return Some(name.to_string());
    }
    let local = local_part(addr.email.trim());
    (!local.is_empty()).then(|| capitalise(local))
}

fn recipient_first_name(addr: &Addr) -> Option<String> {
    let name = addr.name.trim();
    if !name.is_empty() {
        return name.split_whitespace().next().map(str::to_string);
    }
    recipient_name(addr)
}

fn recipient_last_name(addr: &Addr) -> Option<String> {
    let mut words = addr.name.split_whitespace();
    match (words.next(), words.next_back()) {
        (Some(_), Some(last)) => Some(last.to_string()),
        _ => None,
    }
}

/// Expand the snippet: a leading `Subject:` line comes off as the subject, then the
/// `{…}` variables fill from the first To recipient, the From account and today.
/// A variable with no value stays in the text for the user to type over.
fn expand_snippet(
    raw: &str,
    to: Option<&Addr>,
    account: Option<&Account>,
    now: &chrono::DateTime<Local>,
) -> (Option<String>, String) {
    let (subject, mut body) = if let Some(rest) = raw.strip_prefix("Subject:") {
        let end = rest.find('\n').unwrap_or(rest.len());
        (
            Some(rest[..end].trim().to_string()),
            rest[end..].strip_prefix('\n').unwrap_or("").to_string(),
        )
    } else {
        (None, raw.to_string())
    };
    let own = account.map(|account| account.name.trim()).unwrap_or("");
    let values: [(&str, Option<String>); 9] = [
        ("{email}", {
            let email = to.map(|addr| addr.email.trim()).unwrap_or("");
            (!email.is_empty()).then(|| email.to_string())
        }),
        ("{name}", to.and_then(recipient_name)),
        ("{first_name}", to.and_then(recipient_first_name)),
        ("{last_name}", to.and_then(recipient_last_name)),
        ("{my_name}", (!own.is_empty()).then(|| own.to_string())),
        (
            "{my_first_name}",
            own.split_whitespace().next().map(str::to_string),
        ),
        (
            "{date}",
            Some(now.format("%e %B %Y").to_string().trim_start().to_string()),
        ),
        ("{day}", Some(now.format("%A").to_string())),
        ("{time}", Some(now.format("%H:%M").to_string())),
    ];
    for (token, value) in values {
        if let Some(value) = value {
            body = body.replace(token, &value);
        }
    }
    (subject, body)
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
