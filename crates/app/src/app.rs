//! Root view: the split bar, the list or thread body, overlays and toasts.
//!
//! Optimistic actions mutate local state and repaint *before* the engine is told,
//! so every keystroke paints from local state on the same frame.
use crate::keys::{self, Act, GO_MAILBOXES, cmd};
use crate::theme::{self, BAR_H, MAX_COL, RADIUS_SM, Theme};
use crate::{compose, folders, palette, snooze, thread};
use gpui_kit::assets::IconName;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::{Icon, Sizable, TitleBar};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kite_core::api::Mail;
use kite_core::model::{
    Account, Action as MailAction, Event, Folder, Id, Mailbox, Message, Settings, ThreadSummary,
    View,
};
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

/// Threads fetched per page.
pub const PAGE: u32 = 200;

/// Insertion-ordered cache with a fixed capacity. Used for thread bodies and HTML renders.
pub struct Lru<K, V> {
    map: HashMap<K, V>,
    order: VecDeque<K>,
    cap: usize,
}

impl<K: Eq + std::hash::Hash + Clone, V> Lru<K, V> {
    pub fn new(cap: usize) -> Self {
        Self {
            map: HashMap::new(),
            order: VecDeque::new(),
            cap,
        }
    }

    pub fn get(&self, key: &K) -> Option<&V> {
        self.map.get(key)
    }

    pub fn insert(&mut self, key: K, value: V) {
        if self.map.insert(key.clone(), value).is_none() {
            self.order.push_back(key);
        }
        while self.order.len() > self.cap {
            if let Some(old) = self.order.pop_front() {
                self.map.remove(&old);
            }
        }
    }

    pub fn clear(&mut self) {
        self.map.clear();
        self.order.clear();
    }
}

/// A split-inbox tab. `split: None` is the unqualified inbox.
pub struct Tab {
    pub name: String,
    pub split: Option<Id>,
}

pub(crate) struct Toast {
    text: String,
    seq: u64,
    /// A completed state change, which gets the DESIGN › Icons draw acknowledgement.
    mark: bool,
}

pub struct Open {
    pub id: Id,
    pub account_id: Id,
    pub subject: String,
    pub messages: Vec<Message>,
    pub expanded: usize,
}

pub enum Overlay {
    None,
    Palette(palette::Palette),
    Help,
    Snooze(snooze::Snooze),
    Move(folders::MoveDialog),
    CreateFolder(folders::CreateDialog),
    Accounts(usize),
}

pub struct Kite {
    pub(crate) mail: Arc<dyn Mail>,
    pub(crate) theme: Theme,
    pub(crate) dark: bool,
    pub(crate) settings: Settings,
    pub(crate) preferences: Option<crate::preferences::Preferences>,
    pub(crate) appearance_version: u64,
    pub(crate) account_busy: bool,
    pub(crate) accounts: Vec<Account>,
    pub(crate) folders: Vec<Folder>,
    sidebar: Option<Entity<crate::sidebar::Sidebar>>,
    pub(crate) sidebar_dirty: bool,
    pub(crate) refocus: bool,
    pub(crate) tabs: Vec<Tab>,
    pub(crate) view: View,
    pub(crate) rows: Vec<ThreadSummary>,
    pub(crate) sel: usize,
    pub(crate) multi: Vec<Id>,
    pub(crate) open: Option<Open>,
    pub(crate) compose: Option<compose::Compose>,
    pub(crate) compose_open: bool,
    /// The outbox id of the mail just sent, so Undo can cancel it.
    pub(crate) sent: Option<Id>,
    pub(crate) undo: Option<i64>,
    pub(crate) toast: Option<Toast>,
    pub(crate) overlay: Overlay,
    pub(crate) search: Entity<InputState>,
    pub(crate) focus: FocusHandle,
    pub(crate) list_scroll: UniformListScrollHandle,
    pub(crate) thread_scroll: ScrollHandle,
    pub(crate) msgs: Lru<Id, Vec<Message>>,
    pub(crate) html_cache: thread::HtmlCache,
    /// Where each message's rendered body sat in the last frame, for link hit testing.
    pub(crate) html_bounds: Rc<RefCell<HashMap<Id, Bounds<Pixels>>>>,
    /// The body geometry of the last thread frame, so prefetched renders match it.
    pub(crate) html_width: f32,
    pub(crate) html_scale: f32,
    pub(crate) loaded_all: bool,
    /// The last action that ran, with a sequence number, so its icon can animate once.
    pub(crate) pulse: Option<(u8, u64)>,
    pub(crate) seq: u64,
    subs: Vec<Subscription>,
}

impl Kite {
    // ---- accessors used by tests -------------------------------------------------
    pub fn sel_index(&self) -> usize {
        self.sel
    }

    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    pub fn row_ids(&self) -> Vec<Id> {
        self.rows.iter().map(|r| r.id).collect()
    }

    pub fn open_id(&self) -> Option<Id> {
        self.open.as_ref().map(|o| o.id)
    }

    pub fn view(&self) -> &View {
        &self.view
    }

    pub fn new(mail: Arc<dyn Mail>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        keys::bind(cx);
        let mut settings = mail.settings().unwrap_or_default();
        let appearance = theme::resolve(&settings, window.appearance()).unwrap_or_else(|_| {
            settings = Settings::default();
            *theme::pick(theme::wants_dark("system", window.appearance()))
        });
        let dark = appearance.dark;
        theme::install(&appearance, theme::font(&settings), cx);
        // Escape clears the field; the resulting empty value leaves search mode.
        let search = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Search")
                .clean_on_escape()
        });
        let splits = mail.splits().unwrap_or_default();
        let mut tabs: Vec<Tab> = splits
            .iter()
            .map(|s| Tab {
                name: s.name.clone(),
                split: Some(s.id),
            })
            .collect();
        if tabs.is_empty() {
            tabs.push(Tab {
                name: "Inbox".into(),
                split: None,
            });
        }
        let mut this = Self {
            mail,
            theme: appearance,
            dark,
            settings,
            preferences: None,
            appearance_version: 0,
            account_busy: false,
            accounts: Vec::new(),
            folders: Vec::new(),
            sidebar: None,
            sidebar_dirty: true,
            refocus: false,
            view: View {
                account_id: None,
                mailbox: Mailbox::Inbox,
                split: tabs[0].split,
                query: None,
                folder: None,
            },
            tabs,
            rows: Vec::new(),
            sel: 0,
            multi: Vec::new(),
            open: None,
            compose: None,
            compose_open: false,
            sent: None,
            undo: None,
            toast: None,
            overlay: Overlay::None,
            search: search.clone(),
            focus: cx.focus_handle(),
            list_scroll: UniformListScrollHandle::new(),
            thread_scroll: ScrollHandle::new(),
            msgs: Lru::new(24),
            html_cache: thread::HtmlCache::new(24),
            html_bounds: Rc::new(RefCell::new(HashMap::new())),
            html_width: MAX_COL - 80.,
            html_scale: 1.,
            loaded_all: false,
            pulse: None,
            seq: 0,
            subs: Vec::new(),
        };
        this.accounts = this.mail.accounts().unwrap_or_default();
        this.folders = this.mail.folders().unwrap_or_default();
        this.subs.push(cx.subscribe_in(
            &search,
            window,
            |this, state, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    let query = state.read(cx).value().to_string();
                    if query.is_empty() && this.view.query.is_some() {
                        this.clear_search(window, cx);
                    } else {
                        this.set_query(query, cx);
                    }
                }
                InputEvent::PressEnter { .. } => this.run(cmd::OPEN, window, cx),
                _ => {}
            },
        ));
        let events = this.mail.events();
        cx.spawn(async move |this, cx| {
            while let Ok(event) = events.recv().await {
                if this
                    .update(cx, |this, cx| {
                        this.on_event(event, cx);
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        this.reload();
        window.focus(&this.focus, cx);
        this
    }

    // ---- data -----------------------------------------------------------------

    pub(crate) fn reload(&mut self) {
        self.rows = self.mail.threads(&self.view, 0, PAGE).unwrap_or_default();
        self.loaded_all = self.rows.len() < PAGE as usize;
        self.sel = self.sel.min(self.rows.len().saturating_sub(1));
        self.multi.clear();
    }

    pub(crate) fn load_more(&mut self, cx: &mut Context<Self>) {
        if self.loaded_all || self.rows.len() < PAGE as usize {
            self.loaded_all = true;
            return;
        }
        let offset = self.rows.len() as u32;
        let page = self
            .mail
            .threads(&self.view, offset, PAGE)
            .unwrap_or_default();
        self.loaded_all = page.len() < PAGE as usize;
        self.rows.extend(page);
        cx.notify();
    }

    pub(crate) fn set_view(&mut self, mut view: View, cx: &mut Context<Self>) {
        // The inbox is always shown through a lens. The plain inbox is the first
        // seeded split, so the tab strip never renders with nothing selected.
        if view.mailbox == Mailbox::Inbox && view.folder.is_none() && view.split.is_none() {
            view.split = self.tabs.first().and_then(|tab| tab.split);
        }
        let changed = view.account_id != self.view.account_id
            || view.mailbox != self.view.mailbox
            || view.split != self.view.split
            || view.query != self.view.query
            || view.folder != self.view.folder;
        self.view = view;
        if changed {
            self.sidebar_dirty = true;
            self.open = None;
            self.sel = 0;
            self.list_scroll = UniformListScrollHandle::new();
            self.reload();
        }
        cx.notify();
    }

    fn set_query(&mut self, query: String, cx: &mut Context<Self>) {
        let mut view = self.view.clone();
        view.query = if query.trim().is_empty() {
            None
        } else {
            Some(query)
        };
        self.set_view(view, cx);
    }

    fn on_event(&mut self, event: Event, cx: &mut Context<Self>) {
        match event {
            Event::Changed(_) => {
                self.folders = self.mail.folders().unwrap_or_default();
                self.sidebar_dirty = true;
                self.reload();
                if let Some(open) = &self.open {
                    let id = open.id;
                    let messages = self.mail.thread(id).unwrap_or_default();
                    if let Some(open) = &mut self.open {
                        open.expanded = open.expanded.min(messages.len().saturating_sub(1));
                        open.messages = messages;
                    }
                    self.msgs.clear();
                }
                cx.notify();
            }
            Event::Error(message) => self.toast(message, cx),
            Event::Ai { .. } => {}
        }
    }

    /// The ids an action applies to: the multi-selection, or the selected row.
    pub(crate) fn target_ids(&self) -> Vec<Id> {
        if let Some(open) = &self.open {
            return vec![open.id];
        }
        if !self.multi.is_empty() {
            return self.multi.clone();
        }
        self.rows
            .get(self.sel)
            .map(|r| vec![r.id])
            .unwrap_or_default()
    }

    pub(crate) fn toast(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.show_toast(text.into(), false, cx);
    }

    fn show_toast(&mut self, text: String, mark: bool, cx: &mut Context<Self>) {
        self.seq += 1;
        let seq = self.seq;
        self.toast = Some(Toast { text, seq, mark });
        let timer = cx.background_executor().timer(Duration::from_secs(4));
        cx.spawn(async move |this, cx| {
            timer.await;
            this.update(cx, |this, cx| {
                if this.toast.as_ref().is_some_and(|t| t.seq == seq) {
                    this.toast = None;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    // ---- commands -------------------------------------------------------------

    pub(crate) fn toggle_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let cmd = if self.preferences.is_some() {
            cmd::BACK
        } else {
            cmd::SETTINGS
        };
        self.run(cmd, window, cx);
    }

    fn next(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.rows.is_empty() {
            return;
        }
        let last = self.rows.len() - 1;
        self.sel = (self.sel as isize + delta).clamp(0, last as isize) as usize;
        if self.open.is_some() {
            self.open_thread(cx);
        } else {
            self.list_scroll
                .scroll_to_item(self.sel, ScrollStrategy::Nearest);
        }
        cx.notify();
    }

    pub fn open_thread(&mut self, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get(self.sel).cloned() else {
            return;
        };
        let messages = match self.msgs.get(&row.id) {
            Some(messages) => messages.clone(),
            None => {
                let messages = self.mail.thread(row.id).unwrap_or_default();
                self.msgs.insert(row.id, messages.clone());
                messages
            }
        };
        self.multi.clear();
        self.open = Some(Open {
            id: row.id,
            account_id: row.account_id,
            subject: row.subject.clone(),
            expanded: messages.len().saturating_sub(1),
            messages,
        });
        self.thread_scroll = ScrollHandle::new();
        if row.unread {
            if let Some(r) = self.rows.iter_mut().find(|r| r.id == row.id) {
                r.unread = false;
            }
            if let Ok(token) = self.mail.act(&[row.id], &MailAction::Read) {
                self.undo = Some(token);
            }
        }
        self.prefetch(cx);
        cx.notify();
    }

    fn close_thread(&mut self, cx: &mut Context<Self>) {
        self.open = None;
        cx.notify();
    }

    /// Warms the thread-body cache for the neighbours so `j`/`k` in thread view are instant.
    fn prefetch(&mut self, cx: &mut Context<Self>) {
        let neighbours: Vec<Id> = [self.sel.wrapping_sub(1), self.sel + 1]
            .iter()
            .filter_map(|ix| self.rows.get(*ix).map(|r| r.id))
            .filter(|id| self.msgs.get(id).is_none())
            .collect();
        for id in neighbours {
            let mail = self.mail.clone();
            let task = cx
                .background_executor()
                .spawn(async move { mail.thread(id).unwrap_or_default() });
            cx.spawn(async move |this, cx| {
                let messages = task.await;
                this.update(cx, |this, cx| {
                    this.warm_html(&messages, cx);
                    this.msgs.insert(id, messages);
                })
                .ok();
            })
            .detach();
        }
    }

    fn act(&mut self, action: MailAction, remove: bool, text: &str, cx: &mut Context<Self>) {
        self.act_on(self.target_ids(), action, remove, text, cx);
    }

    pub(crate) fn act_on(
        &mut self,
        ids: Vec<Id>,
        action: MailAction,
        remove: bool,
        text: &str,
        cx: &mut Context<Self>,
    ) {
        if ids.is_empty() {
            return;
        }
        if remove {
            self.rows.retain(|r| !ids.contains(&r.id));
            self.sel = self.sel.min(self.rows.len().saturating_sub(1));
            if self.open.as_ref().is_some_and(|o| ids.contains(&o.id)) {
                self.open = None;
            }
        } else {
            for row in self.rows.iter_mut().filter(|r| ids.contains(&r.id)) {
                match action {
                    MailAction::Star => row.starred = true,
                    MailAction::Unstar => row.starred = false,
                    MailAction::Read => row.unread = false,
                    MailAction::Unread => row.unread = true,
                    _ => {}
                }
            }
        }
        self.multi.clear();
        // Paint the new state first; only then tell the engine.
        cx.notify();
        match self.mail.act(&ids, &action) {
            Ok(token) => self.undo = Some(token),
            Err(error) => {
                self.reload();
                self.toast(format!("Could not change mail: {error}"), cx);
                return;
            }
        }
        let mark = matches!(action, MailAction::Archive);
        self.show_toast(text.into(), mark, cx);
    }

    fn star(&mut self, cx: &mut Context<Self>) {
        let starred = self.rows.get(self.sel).is_some_and(|r| r.starred);
        let action = if starred {
            MailAction::Unstar
        } else {
            MailAction::Star
        };
        let text = if starred { "Unstarred" } else { "Starred" };
        self.act(action, false, text, cx);
    }

    fn toggle_read(&mut self, cx: &mut Context<Self>) {
        let unread = self.rows.get(self.sel).is_some_and(|r| r.unread);
        let (action, text) = if unread {
            (MailAction::Read, "Marked read")
        } else {
            (MailAction::Unread, "Marked unread")
        };
        self.act(action, false, text, cx);
    }

    fn select(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.rows.get(self.sel).map(|r| r.id) {
            if let Some(ix) = self.multi.iter().position(|m| *m == id) {
                self.multi.swap_remove(ix);
            } else {
                self.multi.push(id);
            }
        }
        self.next(1, cx);
    }

    fn select_split(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(ix) else {
            return;
        };
        let mut view = self.view.clone();
        view.split = tab.split;
        view.mailbox = Mailbox::Inbox;
        view.folder = None;
        self.set_view(view, cx);
    }

    fn cycle_split(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.tabs.is_empty() {
            return;
        }
        let current = self
            .tabs
            .iter()
            .position(|t| t.split == self.view.split)
            .unwrap_or(0) as isize;
        let next = (current + delta).rem_euclid(self.tabs.len() as isize) as usize;
        self.select_split(next, cx);
    }

    fn search_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let handle = self.search.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
    }

    fn clear_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search
            .update(cx, |state, cx| state.set_value("", window, cx));
        window.focus(&self.focus, cx);
        self.set_query(String::new(), cx);
    }

    fn close_overlay(&mut self, cx: &mut Context<Self>) {
        self.overlay = Overlay::None;
        cx.notify();
    }

    /// The single entry point for every command, from the keymap or the palette.
    pub fn run(&mut self, id: u8, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.overlay, Overlay::Snooze(_)) && id != cmd::BACK {
            return;
        }
        if id == cmd::MANAGE_ACCOUNTS {
            self.show_preferences(1, window, cx);
            return;
        }
        if id == cmd::SETTINGS {
            self.show_preferences(0, window, cx);
            return;
        }
        if id == cmd::ACCOUNT_SWITCHER {
            self.overlay = Overlay::Accounts(0);
            window.focus(&self.focus, cx);
            cx.notify();
            return;
        }
        if self.preferences.is_some() {
            if id == cmd::BACK {
                self.preferences = None;
                self.sidebar_dirty = true;
                window.focus(&self.focus, cx);
                cx.notify();
            }
            return;
        }
        match id {
            cmd::NEXT => self.next(1, cx),
            cmd::PREV => self.next(-1, cx),
            cmd::OPEN if !self.rows.is_empty() => self.open_thread(cx),
            cmd::BACK => {
                if self.compose_open {
                    self.close_compose(window, cx);
                } else if !matches!(self.overlay, Overlay::None) {
                    self.close_overlay(cx);
                } else if self.open.is_some() {
                    self.close_thread(cx);
                } else if self.view.query.is_some() {
                    self.clear_search(window, cx);
                }
            }
            cmd::COMPOSE => self.open_compose(window, cx),
            cmd::REPLY => self.compose_reply(window, cx, false, false),
            cmd::REPLY_ALL => self.compose_reply(window, cx, true, false),
            cmd::FORWARD => self.compose_reply(window, cx, false, true),
            cmd::COMPOSE_CLOSE => self.close_compose(window, cx),
            cmd::SEND => self.send_compose(window, cx),
            cmd::DONE => {
                self.act(MailAction::Archive, true, "Marked done", cx);
                self.pulse = Some((id, self.seq));
            }
            cmd::INBOX => {
                // Moving back to the inbox only leaves the list if we are not
                // already looking at it (or at all mail, which still contains it).
                let remove = !matches!(self.view.mailbox, Mailbox::Inbox | Mailbox::All);
                self.act(MailAction::Inbox, remove, "Moved to inbox", cx);
            }
            cmd::TRASH => {
                self.act(MailAction::Trash, true, "Trashed", cx);
                self.pulse = Some((id, self.seq));
            }
            cmd::SNOOZE => {
                let now = chrono::Local::now();
                self.overlay = Overlay::Snooze(snooze::Snooze::new(now));
                self.pulse = Some((id, self.seq));
                cx.notify();
            }
            cmd::MOVE => self.show_move(window, cx),
            cmd::CREATE_FOLDER => self.show_create_folder(window, cx),
            cmd::STAR => {
                self.star(cx);
                self.pulse = Some((id, self.seq));
            }
            cmd::TOGGLE_READ => self.toggle_read(cx),
            cmd::SELECT => self.select(cx),
            cmd::SEARCH => self.search_focus(window, cx),
            cmd::PALETTE => {
                self.overlay = Overlay::Palette(palette::Palette::default());
                cx.notify();
            }
            cmd::HELP => {
                self.overlay = Overlay::Help;
                cx.notify();
            }
            cmd::NEXT_SPLIT => self.cycle_split(1, cx),
            cmd::PREV_SPLIT => self.cycle_split(-1, cx),
            cmd::UNDO => {
                if let Some(id) = self.sent.take() {
                    match self.mail.cancel_send(id) {
                        Ok(Some(draft)) => {
                            self.reopen_sent(window, draft, cx);
                            self.toast("Send cancelled", cx);
                        }
                        Ok(None) => self.toast("Already sent", cx),
                        Err(error) => self.toast(format!("Could not cancel: {error}"), cx),
                    }
                } else if let Some(token) = self.undo.take() {
                    self.mail.undo(token).ok();
                    self.reload();
                    if let Some(open) = &self.open {
                        let messages = self.mail.thread(open.id).unwrap_or_default();
                        if let Some(open) = &mut self.open {
                            open.messages = messages;
                        }
                    }
                    self.toast("Undone", cx);
                }
            }
            cmd::UNIFIED => self.select_account(None, window, cx),
            id if (cmd::GO..cmd::GO + GO_MAILBOXES.len() as u8).contains(&id) => {
                let ix = (id - cmd::GO) as usize;
                if let Some(mailbox) = GO_MAILBOXES.get(ix) {
                    self.select_mailbox(mailbox.0, window, cx);
                }
            }
            id if (cmd::ACCOUNT..=cmd::ACCOUNT + 8).contains(&id) => {
                let ix = (id - cmd::ACCOUNT) as usize;
                if let Some(account) = self.accounts.get(ix) {
                    self.select_account(Some(account.id), window, cx);
                }
            }
            _ => {}
        }
    }

    fn action(&mut self, action: &Act, window: &mut Window, cx: &mut Context<Self>) {
        // An overlay owns the keyboard: let the raw key handler below deal with it.
        if !matches!(self.overlay, Overlay::None) {
            cx.propagate();
            return;
        }
        self.run(action.cmd, window, cx);
    }

    // ---- keyboard -----------------------------------------------------------------

    /// Raw keys for the overlays, which need text and arrows that are not commands.
    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let typed = typed_char(event);
        match &mut self.overlay {
            Overlay::None => {}
            Overlay::Accounts(index) => {
                match key {
                    "escape" => self.overlay = Overlay::None,
                    "up" => *index = index.saturating_sub(1),
                    "down" => *index = (*index + 1).min(self.accounts.len()),
                    "enter" => {
                        let account = index
                            .checked_sub(1)
                            .and_then(|index| self.accounts.get(index))
                            .map(|a| a.id);
                        self.overlay = Overlay::None;
                        self.select_account(account, window, cx);
                    }
                    _ => {}
                }
                cx.notify();
                cx.stop_propagation();
            }
            Overlay::Move(dialog) => {
                match key {
                    "escape" => self.overlay = Overlay::None,
                    "up" => dialog.selected = dialog.selected.saturating_sub(1),
                    "down" => {
                        dialog.selected =
                            (dialog.selected + 1).min(dialog.targets.len().saturating_sub(1))
                    }
                    "enter" => {
                        let index = dialog.selected;
                        self.confirm_move(index, cx);
                    }
                    _ => {}
                }
                cx.notify();
                cx.stop_propagation();
            }
            Overlay::CreateFolder(dialog) => match key {
                "up" | "down" if !dialog.busy && !self.accounts.is_empty() => {
                    let current = self
                        .accounts
                        .iter()
                        .position(|account| account.id == dialog.account)
                        .unwrap_or(0);
                    let delta = if key == "up" { -1 } else { 1 };
                    let next = (current as isize + delta).rem_euclid(self.accounts.len() as isize)
                        as usize;
                    dialog.account = self.accounts[next].id;
                    dialog.error = None;
                    cx.notify();
                    cx.stop_propagation();
                }
                "escape" => {
                    self.close_overlay(cx);
                    window.focus(&self.focus, cx);
                    cx.stop_propagation();
                }
                "enter" => {
                    self.create_folder(cx);
                    cx.stop_propagation();
                }
                _ => {}
            },
            Overlay::Help => {
                if key == "escape" || typed == Some('?') {
                    self.close_overlay(cx);
                    cx.stop_propagation();
                }
            }
            Overlay::Palette(palette) => {
                let handled = match key {
                    "escape" => {
                        self.close_overlay(cx);
                        true
                    }
                    "enter" => {
                        let id = palette.selected_id();
                        self.overlay = Overlay::None;
                        if let Some(id) = id {
                            self.run(id, window, cx);
                        }
                        true
                    }
                    "up" | "ctrl-p" => {
                        palette.move_selection(-1);
                        true
                    }
                    "down" | "ctrl-n" => {
                        palette.move_selection(1);
                        true
                    }
                    "backspace" => {
                        palette.backspace();
                        true
                    }
                    "space" => {
                        palette.push(' ');
                        true
                    }
                    _ => match typed {
                        Some(c) => {
                            palette.push(c);
                            true
                        }
                        None => false,
                    },
                };
                if handled {
                    cx.notify();
                    cx.stop_propagation();
                }
            }
            Overlay::Snooze(snooze) => {
                let handled = match key {
                    "escape" => {
                        self.close_overlay(cx);
                        true
                    }
                    "enter" => {
                        if let Some(at) = snooze.pick() {
                            self.overlay = Overlay::None;
                            self.act(MailAction::Snooze { until: at }, true, "Snoozed", cx);
                        }
                        true
                    }
                    "up" => {
                        snooze.move_selection(-1);
                        true
                    }
                    "down" => {
                        snooze.move_selection(1);
                        true
                    }
                    "backspace" => {
                        snooze.backspace();
                        true
                    }
                    "space" => {
                        snooze.push(' ');
                        true
                    }
                    _ => match typed {
                        Some(c @ '1'..='4') if snooze.buf.is_empty() => {
                            let ix = c as usize - '1' as usize;
                            if let Some(at) = snooze.confirm_preset(ix) {
                                self.overlay = Overlay::None;
                                self.act(MailAction::Snooze { until: at }, true, "Snoozed", cx);
                            }
                            true
                        }
                        Some(c) => {
                            snooze.push(c);
                            true
                        }
                        None => false,
                    },
                };
                if handled {
                    cx.notify();
                    cx.stop_propagation();
                }
            }
        }
    }

    // ---- view --------------------------------------------------------------------

    /// A toolbar icon that plays its DESIGN › Icons animation when its action runs or on hover.
    pub(crate) fn action_icon(
        &self,
        cmd: u8,
        icon: IconName,
        label: &'static str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = self.theme;
        let pulse = self.pulse.filter(|(c, _)| *c == cmd).map(|(_, seq)| seq);
        let base = Icon::new(icon).with_size(px(16.)).text_color(t.fg2);
        // A fresh element id per run restarts the one-shot animation.
        let glyph = match pulse {
            Some(seq) => base
                .with_animation(
                    ("icon", seq as usize),
                    Animation::new(Duration::from_millis(220)).with_easing(ease_out_quint()),
                    move |icon, delta| {
                        // 0 at both ends, 1 in the middle: one out-and-back pass.
                        let swing = 1. - (2. * delta - 1.).abs();
                        match cmd {
                            // wiggle: ±12°, twice.
                            cmd::SNOOZE => icon.transform(Transformation::rotate(radians(
                                (delta * std::f32::consts::TAU * 2.).sin() * 0.21,
                            ))),
                            cmd::STAR => icon.transform(Transformation::scale(size(
                                1. + 0.18 * swing,
                                1. + 0.18 * swing,
                            ))),
                            // nudge down: archive, and the folder move that shares its direction.
                            _ => icon.transform(Transformation::translate(point(
                                px(0.),
                                px(2. * swing),
                            ))),
                        }
                    },
                )
                .into_any_element(),
            None => base.into_any_element(),
        };
        div()
            .id(("action", cmd as usize))
            .test_support()
            .aria_label(label)
            .flex()
            .items_center()
            .justify_center()
            .size(px(30.))
            .rounded(px(RADIUS_SM))
            .cursor_pointer()
            .hover(|s| s.bg(t.bg3))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered {
                    this.seq += 1;
                    this.pulse = Some((cmd, this.seq));
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |this, _, window, cx| this.run(cmd, window, cx)))
            .child(glyph)
            .into_any_element()
    }

    fn render_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let focused = self.search.read(cx).focus_handle(cx).is_focused(window);
        TitleBar::new()
            .h(px(BAR_H))
            .bg(t.bg)
            .border_b_1()
            .border_color(t.line)
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .items_center()
                    .gap_2()
                    .child(div().flex_1())
                    .when(self.preferences.is_none(), |d| {
                        d.child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .px_2()
                                .h(px(30.))
                                .rounded(px(RADIUS_SM))
                                .border_2()
                                .border_color(Theme::alpha(t.accent, if focused { 1. } else { 0. }))
                                .when(focused, |d| d.bg(t.bg3))
                                .child(
                                    Icon::new(IconName::Search)
                                        .with_size(px(14.))
                                        .text_color(t.fg2),
                                )
                                .child(
                                    Input::new(&self.search)
                                        .id("search")
                                        .aria_label("Search")
                                        .appearance(false)
                                        .bordered(false)
                                        .w(px(
                                            if focused && window.viewport_size().width >= px(900.) {
                                                210.
                                            } else {
                                                120.
                                            },
                                        ))
                                        .text_size(t.px(13.))
                                        .text_color(t.fg2),
                                ),
                        )
                        .child(
                            // Keyboard help lives in the chrome, beside search.
                            div()
                                .id("shortcut-help")
                                .test_support()
                                .aria_label("Keyboard shortcuts")
                                .flex()
                                .items_center()
                                .justify_center()
                                .size(px(30.))
                                .rounded(px(RADIUS_SM))
                                .cursor_pointer()
                                .hover(|s| s.bg(t.bg3))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.run(cmd::HELP, window, cx)
                                }))
                                .child(
                                    div()
                                        .text_size(t.px(14.))
                                        .font_weight(FontWeight(650.))
                                        .text_color(t.fg2)
                                        .child("?"),
                                ),
                        )
                    }),
            )
    }

    /// The inbox splits, as icon tabs directly above the mailbox heading.
    fn split_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        div().flex().items_center().gap_5().pb_4().children(
            self.tabs
                .iter()
                .enumerate()
                .filter(|(_, tab)| tab.split.is_some())
                .map(|(ix, tab)| {
                    let selected = tab.split == self.view.split;
                    div()
                        .id(("tab", ix))
                        .test_support()
                        .aria_label(tab.name.clone())
                        .aria_selected(selected)
                        .relative()
                        .flex()
                        .items_center()
                        .gap_2()
                        .pb_2()
                        .cursor_pointer()
                        .text_size(t.px(13.))
                        .font_weight(if selected {
                            FontWeight(650.)
                        } else {
                            FontWeight::MEDIUM
                        })
                        .text_color(if selected { t.fg } else { t.fg2 })
                        .hover(|s| s.text_color(t.fg))
                        .child(Icon::new(split_icon(&tab.name)).with_size(px(14.)))
                        .child(tab.name.clone())
                        .when(selected, |d| {
                            d.child(
                                div()
                                    .absolute()
                                    .bottom_0()
                                    .left_0()
                                    .right_0()
                                    .h(px(2.))
                                    .rounded_full()
                                    .bg(theme::accent_glow(&t)),
                            )
                        })
                        .on_click(cx.listener(move |this, _, _, cx| this.select_split(ix, cx)))
                }),
        )
    }

    fn render_inbox_header(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let title = if let Some(folder) = self.folders.iter().find(|folder| {
            Some(folder.account_id) == self.view.account_id
                && Some(&folder.id) == self.view.folder.as_ref()
        }) {
            folder.name.as_str()
        } else if self.view.query.is_some() {
            "Search results"
        } else {
            crate::sidebar::mailbox_name(self.view.mailbox)
        };
        let account = self
            .view
            .account_id
            .and_then(|id| self.accounts.iter().find(|a| a.id == id));
        let identity = account.map(|a| a.email.as_str()).unwrap_or_else(|| {
            if self.accounts.len() == 1 {
                self.accounts[0].email.as_str()
            } else if self.accounts.is_empty() {
                "No accounts connected"
            } else {
                "All inboxes"
            }
        });
        let count = self.rows.len();
        let summary = if self.multi.is_empty() {
            format!(
                "{count}{} {}",
                if self.loaded_all { "" } else { "+" },
                if count == 1 {
                    "conversation"
                } else {
                    "conversations"
                }
            )
        } else {
            format!("{} selected", self.multi.len())
        };
        div()
            .id("inbox-header")
            .test_support()
            .aria_label(title.to_string())
            .flex()
            .flex_shrink_0()
            .flex_col()
            .px_8()
            .py_6()
            .when(
                self.view.mailbox == Mailbox::Inbox
                    && self.view.folder.is_none()
                    && self.view.query.is_none(),
                |d| d.child(self.split_tabs(cx)),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .id("mailbox-heading")
                                    .test_support()
                                    .aria_label(title.to_string())
                                    .min_w_0()
                                    .truncate()
                                    .text_size(t.px(24.))
                                    .font_weight(FontWeight(650.))
                                    .child(title.to_string()),
                            )
                            .child(
                                div()
                                    .text_size(t.px(12.))
                                    .text_color(t.fg2)
                                    .child(identity.to_string()),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_4()
                            .when(!self.rows.is_empty(), |d| {
                                d.when(
                                    !self.multi.is_empty()
                                        || window.viewport_size().width >= px(1000.),
                                    |d| {
                                        d.child(
                                            div()
                                                .text_size(t.px(12.))
                                                .text_color(t.fg2)
                                                .child(summary),
                                        )
                                    },
                                )
                                .child(div().w(px(1.)).h(px(18.)).bg(t.line))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_1()
                                        .child(self.action_icon(
                                            cmd::DONE,
                                            IconName::Archive,
                                            "Mark done (e)",
                                            cx,
                                        ))
                                        .child(self.action_icon(
                                            cmd::SNOOZE,
                                            IconName::Clock,
                                            "Snooze (h)",
                                            cx,
                                        ))
                                        .child(self.action_icon(
                                            cmd::MOVE,
                                            IconName::FolderInput,
                                            "Move to folder (l)",
                                            cx,
                                        ))
                                        .child(self.action_icon(
                                            cmd::STAR,
                                            IconName::Star,
                                            "Star (s)",
                                            cx,
                                        )),
                                )
                            }),
                    ),
            )
    }

    fn render_toast(&self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let toast = self.toast.as_ref()?;
        let t = self.theme;
        let can_undo = self.sent.is_some() || self.undo.is_some();
        Some(
            div()
                .absolute()
                .bottom(px(54.))
                .left_0()
                .w_full()
                .flex()
                .justify_center()
                .child(
                    // DESIGN › Icons: `draw` acknowledges a completed action as the toast appears.
                    div()
                        .with_animation(
                            ("toast-in", toast.seq as usize),
                            Animation::new(Duration::from_millis(180))
                                .with_easing(ease_out_quint()),
                            |toast, delta| toast.opacity(delta),
                        )
                        .child(
                            div()
                                .id("toast")
                                .test_support()
                                .aria_label(toast.text.clone())
                                .when(can_undo, |d| {
                                    d.cursor_pointer().on_click(cx.listener(
                                        |this, _, window, cx| this.run(cmd::UNDO, window, cx),
                                    ))
                                })
                                .flex()
                                .items_center()
                                .gap_2()
                                .px_3()
                                .py_2()
                                .rounded_full()
                                .bg(t.bg2)
                                .border_1()
                                .border_color(t.line)
                                .text_size(t.px(13.))
                                .when(toast.mark, |d| {
                                    d.child(
                                        Icon::new(IconName::Check)
                                            .with_size(px(13.))
                                            .text_color(t.ok)
                                            .with_animation(
                                                ("toast-mark", toast.seq as usize),
                                                Animation::new(Duration::from_millis(220))
                                                    .with_easing(ease_out_quint()),
                                                |icon, delta| {
                                                    icon.opacity(delta).transform(
                                                        Transformation::scale(size(
                                                            0.8 + 0.2 * delta,
                                                            0.8 + 0.2 * delta,
                                                        )),
                                                    )
                                                },
                                            ),
                                    )
                                })
                                .child(toast.text.clone())
                                .when_some(
                                    Kbd::binding_for_action(
                                        &Act { cmd: cmd::UNDO },
                                        Some("Kite"),
                                        window,
                                    ),
                                    |d, hint| d.child(hint.outline()),
                                )
                                .child(div().text_color(t.fg3).child("to undo")),
                        ),
                )
                .into_any_element(),
        )
    }
}

/// Splits arrive as a name and a query, so the icon is chosen from the name.
/// Names are user-editable, so anything unrecognised falls back.
fn split_icon(name: &str) -> IconName {
    if name.eq_ignore_ascii_case("inbox") {
        IconName::Inbox
    } else if name.eq_ignore_ascii_case("social") {
        IconName::CircleUser
    } else if name.eq_ignore_ascii_case("newsletters") {
        IconName::BookOpen
    } else if name.eq_ignore_ascii_case("github") {
        IconName::Github
    } else {
        IconName::Ellipsis
    }
}

/// The character a keystroke would type, ignoring any modifier that means a command.
fn typed_char(event: &KeyDownEvent) -> Option<char> {
    let m = event.keystroke.modifiers;
    if m.control || m.platform || m.alt {
        return None;
    }
    if let Some(text) = event.keystroke.key_char.as_deref()
        && let Some(c) = text.chars().next()
    {
        return Some(c);
    }
    let key = event.keystroke.key.as_str();
    let mut chars = key.chars();
    let c = chars.next()?;
    chars.next().is_none().then_some(c)
}

impl Render for Kite {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.refocus {
            window.focus(&self.focus, cx);
            self.refocus = false;
        }
        let dark = theme::wants_dark(&self.settings.theme, window.appearance());
        if self.settings.theme == "system" && dark != self.dark {
            self.dark = dark;
            self.theme =
                theme::resolve(&self.settings, window.appearance()).unwrap_or(*theme::pick(dark));
            self.sidebar_dirty = true;
            theme::install(&self.theme, theme::font(&self.settings), cx);
            self.html_cache.clear();
            self.appearance_version += 1;
        }
        if self.sidebar_dirty {
            let state = crate::sidebar::Sidebar::new(self, cx.entity().downgrade());
            if let Some(sidebar) = &self.sidebar {
                sidebar.update(cx, |sidebar, cx| {
                    *sidebar = state;
                    cx.notify();
                });
            } else {
                self.sidebar = Some(cx.new(|_| state));
            }
            self.sidebar_dirty = false;
        }
        self.html_width = ((window.viewport_size().width.as_f32() - crate::sidebar::width(window))
            .min(MAX_COL)
            - 80.)
            .max(120.);
        self.html_scale = window.scale_factor();
        let open = self.open.is_some();
        let preferences_open = self.preferences.is_some();
        let body = if preferences_open {
            crate::preferences::render(self, window, cx)
        } else if open {
            self.render_thread(window, cx)
        } else {
            self.render_list(window, cx)
        };
        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(self.theme.bg)
            .text_color(self.theme.fg)
            .font_family(theme::font(&self.settings).to_string())
            .text_size(self.theme.px(14.))
            .key_context("Kite")
            .on_key_down(cx.listener(Self::key_down))
            .on_action(cx.listener(Self::action))
            .child(self.render_bar(window, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .children(self.sidebar.as_ref().map(|sidebar| {
                        sidebar.clone().cached(
                            StyleRefinement::default()
                                .w(px(crate::sidebar::width(window)))
                                .h_full()
                                .flex_shrink_0(),
                        )
                    }))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .when(!open && !preferences_open, |d| {
                                d.child(self.render_inbox_header(window, cx))
                            })
                            .child(
                                div()
                                    .id("body")
                                    .flex_1()
                                    .min_h_0()
                                    .flex()
                                    .flex_col()
                                    .overflow_hidden()
                                    .key_context(if preferences_open {
                                        "KitePreferences"
                                    } else if open {
                                        "KiteThread"
                                    } else {
                                        "KiteList"
                                    })
                                    .track_focus(&self.focus)
                                    .child(body),
                            ),
                    ),
            )
            .children(self.render_compose(window, cx))
            .children(self.render_overlay(window, cx))
            .children(self.render_toast(window, cx))
    }
}

impl Kite {
    fn render_overlay(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        match &self.overlay {
            Overlay::None => None,
            Overlay::Accounts(_) => Some(crate::sidebar::account_menu(self, window, cx)),
            Overlay::Help => Some(self.render_help(cx)),
            Overlay::Palette(_) => Some(palette::render(self, window, cx)),
            Overlay::Snooze(_) => Some(snooze::render(self, window, cx)),
            Overlay::Move(_) | Overlay::CreateFolder(_) => Some(folders::render(self, window, cx)),
        }
    }
}
