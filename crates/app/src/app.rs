//! Root view: the split bar, the list or thread body, overlays and toasts.
//!
//! Optimistic actions mutate local state and repaint *before* the engine is told,
//! so every keystroke paints from local state on the same frame.
use crate::keys::{self, Act, GO_MAILBOXES, cmd};
use crate::theme::{self, MAX_COL, RADIUS_SM, STRIP_H, Theme};
use crate::{compose, folders, palette, snooze, tags, thread};
use gpui_kit::assets::IconName;
use gpui_kit::base::InteractiveElementExt as _;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::{Icon, Sizable, WindowExt as _};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kite_core::api::Mail;
use kite_core::model::{
    Account, Action as MailAction, AiRequest, Event, Folder, Id, Mailbox, Message, Settings,
    ThreadSummary, View,
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

/// Where a streamed AI answer belongs.
pub enum AiFor {
    Compose,
    Summary(Id),
}

/// One streamed AI answer, filled by `Event::Ai` chunks.
pub struct Ai {
    pub req: u32,
    pub text: String,
    pub done: bool,
    pub target: AiFor,
}

pub enum Overlay {
    None,
    Palette(palette::Palette),
    Help,
    Snooze(snooze::Snooze),
    Move(folders::MoveDialog),
    CreateFolder(folders::CreateDialog),
    Accounts(usize),
    /// The mailbox picker behind the list header's title, by row.
    Mailboxes(usize),
    /// The tag/action menu the keyboard opens (`Shift+L`); right-click uses a popup instead.
    Tags(tags::TagMenu),
    /// The "New tag…" name field, with the conversations it will apply to.
    NewTag(tags::NewTag),
}

/// Which pane the keyboard is working in: the list, or the open conversation beside it.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Pane {
    List,
    Thread,
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
    /// Unread inbox conversations per account, for the rail badges.
    pub(crate) unread: Vec<(Id, usize)>,
    /// Where the keyboard is working: the list, or the conversation in the reading pane.
    pub(crate) pane: Pane,
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
    /// A draft picked in the Drafts mailbox, opened on the next render when a window exists.
    pub(crate) open_draft: Option<kite_core::model::Draft>,
    /// The shared snooze picker is choosing a send time rather than a snooze time.
    pub(crate) send_later: bool,
    /// The outbox id of the mail just sent, so Undo can cancel it.
    pub(crate) sent: Option<Id>,
    pub(crate) sent_stamp: u64,
    pub(crate) undo_stamp: u64,
    pub(crate) undo_gen: u64,
    pub(crate) undo: Option<i64>,
    pub(crate) toast: Option<Toast>,
    pub(crate) ai: Option<Ai>,
    /// Remote images the reader loaded, by URL, as `data:` URLs.
    pub(crate) images: Arc<HashMap<String, String>>,
    /// URLs whose fetch is in flight, so one frame never asks twice.
    pub(crate) images_pending: std::collections::HashSet<String>,
    /// Messages the reader allowed remote images for, one at a time.
    pub(crate) images_allowed: std::collections::HashSet<Id>,
    /// Bumped when a fetched image lands, so bodies re-render with it inlined.
    pub(crate) images_version: u64,
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

    /// The resolved ink and page colours, so a test can see a palette reach the app.
    pub fn colours(&self) -> (String, String) {
        (theme::hex(self.theme.fg), theme::hex(self.theme.bg))
    }

    pub fn view(&self) -> &View {
        &self.view
    }

    pub fn new(mail: Arc<dyn Mail>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut settings = mail.settings().unwrap_or_default();
        let appearance = theme::resolve(&settings, window.appearance()).unwrap_or_else(|_| {
            settings = Settings::default();
            *theme::pick(theme::wants_dark("system", window.appearance()))
        });
        keys::bind(cx, &settings);
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
            unread: Vec::new(),
            pane: Pane::List,
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
            open_draft: None,
            send_later: false,
            sent: None,
            sent_stamp: 0,
            undo_stamp: 0,
            undo_gen: 0,
            undo: None,
            toast: None,
            ai: None,
            images: Arc::new(HashMap::new()),
            images_pending: std::collections::HashSet::new(),
            images_allowed: std::collections::HashSet::new(),
            images_version: 0,
            overlay: Overlay::None,
            search: search.clone(),
            focus: cx.focus_handle(),
            list_scroll: UniformListScrollHandle::new(),
            thread_scroll: ScrollHandle::new(),
            msgs: Lru::new(24),
            html_cache: thread::HtmlCache::new(24),
            html_bounds: Rc::new(RefCell::new(HashMap::new())),
            html_width: MAX_COL - 2. * crate::thread::PANE_PAD,
            html_scale: 1.,
            loaded_all: false,
            pulse: None,
            seq: 0,
            subs: Vec::new(),
        };
        this.accounts = this.mail.accounts().unwrap_or_default();
        this.folders = this.mail.folders().unwrap_or_default();
        this.unread = crate::sidebar::unread_counts(this.mail.as_ref(), &this.accounts);
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
        // A bare resize changes the reading pane's column, and a view that is not dirty is not
        // rendered again: without this a body keeps the frame it was laid out for.
        this.subs
            .push(cx.observe_window_bounds(window, |this, window, cx| {
                let width = body_column(window);
                if (width - this.html_width).abs() >= 1. || window.scale_factor() != this.html_scale
                {
                    this.html_width = width;
                    cx.notify();
                }
            }));
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
                self.unread = crate::sidebar::unread_counts(self.mail.as_ref(), &self.accounts);
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
            Event::Ai { req, delta, done } => {
                let Some(ai) = &mut self.ai else {
                    return;
                };
                if ai.req != req {
                    return;
                }
                ai.text.push_str(&delta);
                ai.done |= done;
                cx.notify();
            }
        }
    }

    /// The ids an action applies to: the open conversation while the keyboard is in it, else the
    /// multi-selection, else the selected row.
    pub(crate) fn target_ids(&self) -> Vec<Id> {
        if let Some(open) = &self.open
            && self.pane == Pane::Thread
        {
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

    /// Stamps and stores an undoable mail action, so Undo picks the latest one.
    pub(crate) fn record_undo(&mut self, token: i64) {
        self.undo_gen += 1;
        self.undo_stamp = self.undo_gen;
        self.undo = Some(token);
    }

    /// Stamps and stores a sent outbox id for the undo-send window.
    pub(crate) fn record_sent(&mut self, id: Id) {
        self.undo_gen += 1;
        self.sent_stamp = self.undo_gen;
        self.sent = Some(id);
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
        // In the reading pane, moving in the list does not change the open conversation.
        if self.open.is_some() && self.pane == Pane::Thread {
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
        // A saved draft opens in compose, prefilled, instead of the reading view.
        let draft = if self.view.mailbox == Mailbox::Drafts {
            self.mail.draft(row.id).unwrap_or(None)
        } else {
            None
        };
        if let Some(draft) = draft {
            self.multi.clear();
            self.open = None;
            if self.has_draft(cx) {
                // A draft already being written is never replaced by another one.
                self.compose_open = true;
                self.toast("Draft kept", cx);
            } else {
                self.open_draft = Some(draft);
            }
            cx.notify();
            return;
        }
        let messages = match self.msgs.get(&row.id) {
            Some(messages) => messages.clone(),
            None => {
                let messages = self.mail.thread(row.id).unwrap_or_default();
                self.msgs.insert(row.id, messages.clone());
                messages
            }
        };
        self.multi.clear();
        // The keyboard is now reading, so j/k move through conversations rather than the list.
        self.pane = Pane::Thread;
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
                self.record_undo(token);
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

    pub(crate) fn act(
        &mut self,
        action: MailAction,
        remove: bool,
        text: &str,
        cx: &mut Context<Self>,
    ) {
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
            Ok(token) => self.record_undo(token),
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
        // In the reading pane the open conversation is the target, so read its own state.
        let target = self.target_ids().first().copied();
        let unread = target
            .and_then(|id| self.rows.iter().find(|row| row.id == id))
            .or_else(|| self.rows.get(self.sel))
            .is_some_and(|row| row.unread);
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
        self.send_later = false;
        cx.notify();
    }

    /// The single entry point for every command, from the keymap or the palette.
    pub fn run(&mut self, id: u8, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.overlay, Overlay::Snooze(_)) && id != cmd::BACK {
            return;
        }
        if id == cmd::MANAGE_ACCOUNTS {
            self.show_preferences(crate::preferences::OPEN_ACCOUNTS, window, cx);
            return;
        }
        if id == cmd::SETTINGS {
            self.show_preferences(crate::preferences::OPEN_APPEARANCE, window, cx);
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
            cmd::REFRESH => {
                self.mail.sync_now();
                self.toast("Refresh requested", cx);
            }
            cmd::NEXT => self.next(1, cx),
            cmd::PREV => self.next(-1, cx),
            cmd::OPEN if !self.rows.is_empty() => self.open_thread(cx),
            cmd::BACK => {
                if self.compose_open {
                    self.close_compose(window, cx);
                } else if !matches!(self.overlay, Overlay::None) {
                    self.close_overlay(cx);
                } else if self.open.is_some() {
                    if self.settings.reading_pane {
                        // The conversation stays beside the list; the keyboard returns to it.
                        self.pane = Pane::List;
                        cx.notify();
                    } else {
                        self.close_thread(cx);
                    }
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
            cmd::SEND_LATER => self.schedule_send(window, cx),
            cmd::DISCARD => self.discard_compose(window, cx),
            cmd::SHOW_CC => {
                if let Some(compose) = &mut self.compose {
                    compose.show_cc = true;
                }
                cx.notify();
            }
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
                self.send_later = false;
                self.overlay = Overlay::Snooze(snooze::Snooze::new(now));
                self.pulse = Some((id, self.seq));
                cx.notify();
            }
            cmd::MOVE => self.show_move(window, cx),
            cmd::TAGS => self.show_tag_menu(window, cx),
            cmd::AI_WRITE => self.ai_write(cx),
            cmd::AI_SUMMARY => self.ai_summary(cx),
            cmd::CREATE_FOLDER => self.show_create_folder(window, cx),
            cmd::STAR => {
                self.star(cx);
                self.pulse = Some((id, self.seq));
            }
            cmd::TOGGLE_READ => self.toggle_read(cx),
            cmd::SELECT => self.select(cx),
            cmd::SEARCH => self.search_focus(window, cx),
            cmd::PALETTE => {
                self.overlay = Overlay::Palette(palette::Palette::new(&self.settings));
                cx.notify();
            }
            cmd::HELP => {
                self.overlay = Overlay::Help;
                cx.notify();
            }
            cmd::NEXT_SPLIT => self.cycle_split(1, cx),
            cmd::PREV_SPLIT => self.cycle_split(-1, cx),
            cmd::UNDO => {
                let cancel_send = match (self.sent, self.undo) {
                    (Some(_), Some(_)) => self.sent_stamp > self.undo_stamp,
                    (Some(_), None) => true,
                    _ => false,
                };
                if cancel_send {
                    if let Some(id) = self.sent.take() {
                        match self.mail.cancel_send(id) {
                            Ok(Some(draft)) => {
                                if self.reopen_sent(window, draft, cx) {
                                    self.toast("Send cancelled", cx);
                                }
                            }
                            Ok(None) => self.toast("Already sent", cx),
                            Err(error) => self.toast(format!("Could not cancel: {error}"), cx),
                        }
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

    // ---- AI --------------------------------------------------------------------

    /// One AI answer, shared by "Write with AI" and "Summarize thread". The engine owns the
    /// prompt, the endpoint and the model, so it — not the UI — decides what may leave the
    /// machine. It refuses Private conversations and streams the answer back as `Event::Ai`.
    fn ai_request(&mut self, id: u8, target: AiFor, request: AiRequest, cx: &mut Context<Self>) {
        if !self.settings.ai_enabled {
            self.toast("AI is off. Turn it on in Settings › AI.", cx);
            return;
        }
        if self.ai.as_ref().is_some_and(|ai| !ai.done) {
            self.toast("An AI answer is still arriving", cx);
            return;
        }
        self.seq += 1;
        let req = self.seq as u32;
        match self.mail.ai(req, request) {
            Ok(()) => {
                self.ai = Some(Ai {
                    req,
                    text: String::new(),
                    done: false,
                    target,
                });
                self.pulse = Some((id, self.seq));
            }
            Err(error) => self.toast(format!("AI: {error}"), cx),
        }
        cx.notify();
    }

    /// Cmd/Ctrl+J in compose: the engine writes from the draft, refusing when the draft
    /// answers a Private conversation.
    fn ai_write(&mut self, cx: &mut Context<Self>) {
        let Some(compose) = &self.compose else {
            self.toast("Open compose first", cx);
            return;
        };
        let request = AiRequest::Compose {
            draft: compose.draft(cx),
        };
        self.ai_request(cmd::AI_WRITE, AiFor::Compose, request, cx);
    }

    /// Shift+S in the reading pane: summarize the open conversation — unless it is Private.
    fn ai_summary(&mut self, cx: &mut Context<Self>) {
        let Some(open) = &self.open else {
            self.toast("Open a conversation first", cx);
            return;
        };
        let id = open.id;
        let request = AiRequest::Summary { thread: id };
        self.ai_request(cmd::AI_SUMMARY, AiFor::Summary(id), request, cx);
    }

    fn action(&mut self, action: &Act, window: &mut Window, cx: &mut Context<Self>) {
        // A shortcut being recorded needs the raw keystroke, not the command it currently runs.
        if crate::preferences::shortcuts::recording(self) {
            cx.propagate();
            return;
        }
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
        // Settings › Shortcuts records the next keystroke instead of running it.
        if crate::preferences::shortcuts::capture(self, event, window, cx) {
            cx.stop_propagation();
            return;
        }
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
            Overlay::Mailboxes(index) => {
                let step = match key {
                    "up" => -1,
                    "down" => 1,
                    _ => 0,
                };
                let current = (*index as isize + step).max(0) as usize;
                *index = current;
                let targets = crate::sidebar::picker_targets(self);
                let current = current.min(targets.len().saturating_sub(1));
                match key {
                    "escape" => self.overlay = Overlay::None,
                    "up" | "down" => {
                        if let Overlay::Mailboxes(index) = &mut self.overlay {
                            *index = current;
                        }
                    }
                    "enter" => {
                        let target = targets.get(current).cloned();
                        self.overlay = Overlay::None;
                        match target {
                            Some(crate::sidebar::Pick::Mailbox(mailbox)) => {
                                self.select_mailbox(mailbox, window, cx)
                            }
                            Some(crate::sidebar::Pick::Folder(folder)) => {
                                self.select_folder(&folder, window, cx)
                            }
                            None => {}
                        }
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
            Overlay::Tags(menu) => {
                let last = menu.items.len().saturating_sub(1);
                match key {
                    "escape" => self.overlay = Overlay::None,
                    "up" => menu.selected = menu.selected.saturating_sub(1),
                    "down" => menu.selected = (menu.selected + 1).min(last),
                    "enter" => {
                        let index = menu.selected;
                        self.activate_tag_item(index, window, cx);
                    }
                    _ => {}
                }
                cx.notify();
                cx.stop_propagation();
            }
            Overlay::NewTag(_) => match key {
                "escape" => {
                    self.close_overlay(cx);
                    window.focus(&self.focus, cx);
                    cx.stop_propagation();
                }
                "enter" => {
                    self.create_tag(cx);
                    cx.stop_propagation();
                }
                _ => {}
            },
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
                            self.confirm_snooze(at, window, cx);
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
                                self.confirm_snooze(at, window, cx);
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
        label: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let label = label.into();
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

    /// The window's top strip: the drag region, then the window controls where the OS draws none.
    fn window_strip(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme;
        let pane = self.settings.reading_pane && self.preferences.is_none();
        let settings_open = self.preferences.is_some();
        let wide = if pane {
            crate::sidebar::list_width(window)
        } else {
            0.
        };
        // One opaque fill under the whole strip. Ember paints a gradient behind the rail and the
        // list, so a segment with a fill of its own met the next one in a visible seam and every
        // control showed a different colour behind it.
        let mut strip = div()
            .id("window-strip")
            .test_support()
            .flex_shrink_0()
            .h(px(STRIP_H))
            .flex()
            .items_center()
            .bg(t.bg);
        // The list/reading-pane toggle opens the strip at the list column's left edge, and stays
        // on that spot when the list steps aside, so the control never jumps under the pointer.
        strip = if wide > 0. {
            strip.child(
                div()
                    .w(px(wide))
                    .h_full()
                    .flex()
                    .items_center()
                    .pl(px(STRIP_PAD))
                    .children((!settings_open).then(|| self.pane_toggle(cx))),
            )
        } else {
            strip.when(!settings_open, |d| {
                d.child(
                    div()
                        .h_full()
                        .flex()
                        .items_center()
                        .pl(px(STRIP_PAD))
                        .child(self.pane_toggle(cx)),
                )
            })
        };
        strip
            .child(
                div()
                    .id("window-drag")
                    .flex_1()
                    .h_full()
                    .window_control_area(WindowControlArea::Drag)
                    // Windows moves the window through the hit test; other platforms need this.
                    .when(!cfg!(target_os = "windows"), |d| {
                        d.on_mouse_down(MouseButton::Left, |_, window, _| {
                            window.start_window_move()
                        })
                    })
                    .on_double_click(|_, window, _| window.titlebar_double_click())
                    .test_support(),
            )
            .child(
                strip_chip(&t, STRIP_CHIP)
                    .id("open-settings")
                    .test_support()
                    .aria_label("Settings")
                    .aria_selected(settings_open)
                    // Only there when the OS draws no controls: keep it off the edge.
                    .when(!cfg!(target_os = "windows"), |d| d.mr_2())
                    .text_color(if settings_open { t.fg } else { t.fg2 })
                    .when(settings_open, |d| d.bg(Theme::alpha(t.fg, CHIP_HOVER)))
                    .child(Icon::new(IconName::Settings).with_size(px(16.)))
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_settings(window, cx))),
            )
            .child(self.window_controls(window))
            .into_any_element()
    }

    /// Side by side, or one at a time: `settings.reading_pane`, saved like any preference. It is
    /// a strip control above the list column it acts on, in the same chip as the cog, so the two
    /// icons share one line.
    fn pane_toggle(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme;
        let shown = self.settings.reading_pane;
        strip_chip(&t, STRIP_CHIP)
            .id("toggle-reading-pane")
            .test_support()
            .aria_label("Toggle list and reading pane")
            .aria_selected(shown)
            .text_color(if shown { t.fg } else { t.fg2 })
            .child(Icon::new(IconName::PanelRight).with_size(px(16.)))
            .on_click(cx.listener(|this, _, window, cx| {
                let mut settings = this.settings.clone();
                settings.reading_pane = !settings.reading_pane;
                this.save_preferences(settings, window, cx);
            }))
            .into_any_element()
    }

    /// Minimise, maximise and close, drawn by us because the titlebar is ours to draw.
    fn window_controls(&self, window: &Window) -> AnyElement {
        if !cfg!(target_os = "windows") {
            return div().into_any_element();
        }
        let t = self.theme;
        div()
            .id("window-controls")
            .test_support()
            .aria_label("Window controls")
            .flex_shrink_0()
            .flex()
            .h_full()
            .child(window_control(
                &t,
                "window-minimize",
                IconName::WindowMinimize,
                WindowControlArea::Min,
                "Minimize",
            ))
            .child(window_control(
                &t,
                "window-maximize",
                if window.is_maximized() {
                    IconName::WindowRestore
                } else {
                    IconName::WindowMaximize
                },
                WindowControlArea::Max,
                "Maximize",
            ))
            .child(window_control(
                &t,
                "window-close",
                IconName::WindowClose,
                WindowControlArea::Close,
                "Close",
            ))
            .into_any_element()
    }

    /// The search field, under the mailbox title.
    fn render_search(&mut self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let focused = self.search.read(cx).focus_handle(cx).is_focused(window);
        div().flex_shrink_0().px_4().pb_3().child(
            div()
                .id("search-field")
                .flex()
                .items_center()
                .gap_2()
                .h(px(32.))
                .px_2()
                .rounded(px(RADIUS_SM))
                .border_1()
                .border_color(if focused { t.accent } else { t.line })
                .bg(t.bg)
                .child(
                    Icon::new(IconName::Search)
                        .with_size(px(14.))
                        .text_color(t.fg2),
                )
                .child(
                    div().flex_1().min_w_0().child(
                        Input::new(&self.search)
                            .id("search")
                            .aria_label("Search")
                            .appearance(false)
                            .bordered(false)
                            .w_full()
                            .text_size(t.px(13.))
                            .text_color(t.fg2),
                    ),
                ),
        )
    }

    /// The list column: the mailbox header, search, splits and the virtualised list.
    fn render_list_column(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme;
        let splits = self.view.mailbox == Mailbox::Inbox
            && self.view.folder.is_none()
            && self.view.query.is_none();
        // Without a reading pane the list is the whole window; with one it keeps its column.
        let pane = self.settings.reading_pane;
        div()
            .id("list-column")
            .test_support()
            .aria_label("Message list")
            .h_full()
            .min_w_0()
            .flex()
            .flex_col()
            .when(pane, |d| {
                d.w(px(crate::sidebar::list_width(window))).flex_shrink_0()
            })
            .when(!pane, |d| d.flex_1())
            .bg(theme::panel(&t))
            .border_r_1()
            .border_color(t.line)
            .child(self.render_inbox_header(cx))
            .child(self.render_search(window, cx))
            .when(splits, |d| {
                d.child(
                    div()
                        .flex_shrink_0()
                        .px_4()
                        .pb_2()
                        .child(self.split_tabs(cx)),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(self.render_list(window, cx)),
            )
            .into_any_element()
    }

    /// The reading pane: the open conversation, or a quiet invitation when there is none.
    fn render_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme;
        let body = if self.open.is_some() {
            self.render_thread(window, cx)
        } else {
            self.render_pane_empty()
        };
        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(t.bg)
            .child(body)
            .into_any_element()
    }

    fn render_pane_empty(&self) -> AnyElement {
        let t = self.theme;
        let open = keys::hint_for(&self.settings, cmd::OPEN);
        div()
            .id("reading-empty")
            .test_support()
            .aria_label("No conversation selected")
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_3()
            .bg(t.bg)
            .child(
                Icon::new(IconName::Inbox)
                    .with_size(px(22.))
                    .text_color(t.fg3),
            )
            .child(
                div()
                    .text_size(t.px(13.))
                    .text_color(t.fg2)
                    .child("No conversation selected"),
            )
            .child(
                div()
                    .text_size(t.px(12.))
                    .text_color(t.fg3)
                    .child(if open.is_empty() {
                        "Select a conversation to read it".to_string()
                    } else {
                        format!("Select a conversation, then press {open}")
                    }),
            )
            .into_any_element()
    }

    fn open_mailbox_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.overlay = Overlay::Mailboxes(0);
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// The rail's light/dark switch.
    pub(crate) fn toggle_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut settings = self.settings.clone();
        settings.theme = if self.dark { "light" } else { "dark" }.into();
        self.save_preferences(settings, window, cx);
    }

    /// The inbox splits, as compact tabs above the list.
    fn split_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        div().flex().items_center().gap_4().children(
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
                        .pb_1()
                        .cursor_pointer()
                        .text_size(t.px(12.))
                        .font_weight(if selected {
                            FontWeight(650.)
                        } else {
                            FontWeight::MEDIUM
                        })
                        .text_color(if selected { t.fg } else { t.fg2 })
                        .hover(|s| s.text_color(t.fg))
                        .child(Icon::new(split_icon(&tab.name)).with_size(px(13.)))
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

    /// The list column header: the mailbox title with its picker, the account identity and compose.
    fn render_inbox_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
        let identity = account
            .map(|a| {
                if a.name.trim().is_empty() {
                    a.email.clone()
                } else {
                    format!("{}, {}", a.name, a.email)
                }
            })
            .unwrap_or_else(|| {
                if self.accounts.len() == 1 {
                    let only = &self.accounts[0];
                    if only.name.trim().is_empty() {
                        only.email.clone()
                    } else {
                        format!("{}, {}", only.name, only.email)
                    }
                } else if self.accounts.is_empty() {
                    "No accounts connected".to_string()
                } else {
                    format!("All inboxes, {} accounts", self.accounts.len())
                }
            });
        div()
            .id("inbox-header")
            .test_support()
            .aria_label(title.to_string())
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap_1()
            .px_4()
            .pt_2()
            .pb_1()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .id("mailbox-picker")
                            .test_support()
                            .aria_label(format!("Change mailbox, {title}"))
                            .flex()
                            .items_center()
                            .gap_1()
                            .min_w_0()
                            .ml(px(-8.))
                            .px_2()
                            .py_1()
                            .rounded(px(RADIUS_SM))
                            .cursor_pointer()
                            .hover(|s| s.bg(Theme::alpha(t.fg, 0.06)))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_mailbox_picker(window, cx)
                            }))
                            .child(
                                div()
                                    .id("mailbox-heading")
                                    .test_support()
                                    .aria_label(title.to_string())
                                    .min_w_0()
                                    .truncate()
                                    .text_size(t.px(15.))
                                    .font_weight(FontWeight(650.))
                                    .child(title.to_string()),
                            )
                            .child(
                                Icon::new(IconName::ChevronDown)
                                    .with_size(px(14.))
                                    .text_color(t.fg2),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .flex_shrink_0()
                            .child(
                                div()
                                    .id("refresh-mail")
                                    .test_support()
                                    .aria_label(format!(
                                        "Refresh mail, {}",
                                        keys::hint_for(&self.settings, cmd::REFRESH)
                                    ))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .size(px(30.))
                                    .flex_shrink_0()
                                    .rounded(px(RADIUS_SM))
                                    .cursor_pointer()
                                    .text_color(t.fg2)
                                    .hover(|s| s.bg(Theme::alpha(t.fg, 0.06)).text_color(t.fg))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.run(cmd::REFRESH, window, cx)
                                    }))
                                    .child(Icon::new(IconName::RefreshCw).with_size(px(16.))),
                            )
                            .child(
                                div()
                                    .id("compose-new")
                                    .test_support()
                                    .aria_label("New message")
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .size(px(30.))
                                    .flex_shrink_0()
                                    .rounded(px(RADIUS_SM))
                                    .cursor_pointer()
                                    .text_color(t.fg2)
                                    .hover(|s| s.bg(Theme::alpha(t.fg, 0.06)).text_color(t.fg))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.run(cmd::COMPOSE, window, cx)
                                    }))
                                    .child(Icon::new(IconName::SquarePen).with_size(px(16.))),
                            ),
                    ),
            )
            .child(
                div()
                    .id("account-identity")
                    .test_support()
                    .aria_label(identity.clone())
                    .px_2()
                    .truncate()
                    .text_size(t.px(12.))
                    .text_color(t.fg2)
                    .child(identity),
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

const WINDOW_CONTROL_W: f32 = 46.;
/// A strip icon button is a chip the size of the list header's icons.
const STRIP_CHIP: f32 = 30.;
/// Every hover fill in the strip uses the alpha the list header's icon buttons use.
const CHIP_HOVER: f32 = 0.06;
/// The window strip's inset before its first control, so the pane toggle sits at the list
/// column's left edge rather than against the rail.
const STRIP_PAD: f32 = 4.;

/// The width a message body is rendered at: the reading pane's column, inside its own padding.
fn body_column(window: &Window) -> f32 {
    ((window.viewport_size().width.as_f32() - crate::sidebar::width(window)).min(MAX_COL)
        - 2. * crate::thread::PANE_PAD)
        .max(120.)
}

/// A quiet icon button: a rounded hover chip, never a full-height square, so it matches the icon
/// buttons in the list header and the thread toolbar. It paints nothing until the pointer arrives.
pub(crate) fn strip_chip(t: &Theme, size: f32) -> Div {
    let t = *t;
    div()
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .size(px(size))
        .rounded(px(RADIUS_SM))
        .cursor_pointer()
        .hover(move |s| s.bg(Theme::alpha(t.fg, CHIP_HOVER)).text_color(t.fg))
}

/// One window button. Windows resolves the click through its hit test, so no handler is needed.
fn window_control(
    t: &Theme,
    id: &'static str,
    icon: IconName,
    area: WindowControlArea,
    label: &'static str,
) -> impl IntoElement {
    let t = *t;
    // Windows keeps the full-height hover for its own buttons; only the fill is shared.
    let hover = if area == WindowControlArea::Close {
        Theme::alpha(t.danger, 0.85)
    } else {
        Theme::alpha(t.fg, CHIP_HOVER)
    };
    div()
        .id(id)
        .test_support()
        .aria_label(label)
        .w(px(WINDOW_CONTROL_W))
        .h_full()
        .flex()
        .items_center()
        .justify_center()
        .text_color(t.fg2)
        .hover(move |s| s.bg(hover).text_color(t.fg))
        .window_control_area(area)
        .child(Icon::new(icon).with_size(px(14.)))
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
            // A new appearance re-keys the cache; the painted frame stays until it lands.
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
        let pane = self.settings.reading_pane;
        crate::sidebar::set_pane(pane);
        // The thread is laid out in the reading pane, not under the list; `html_width` is its
        // column inside the pane's padding, so a body renders at the width it is shown at.
        self.html_width = body_column(window);
        self.html_scale = window.scale_factor();
        let open = self.open.is_some();
        let preferences_open = self.preferences.is_some();
        // A focused text field owns the keyboard. The list and thread keys are scoped to their
        // pane, so typing in the search box (which sits in the list column) must leave both.
        let typing = window.has_focused_input(cx);
        let context = if preferences_open {
            "KitePreferences"
        } else if typing {
            "Kite"
        } else if open && self.pane == Pane::Thread {
            "KiteThread"
        } else {
            "KiteList"
        };
        let content = if preferences_open {
            div()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .flex()
                .flex_col()
                .bg(self.theme.bg)
                .child(self.window_strip(window, cx))
                .child(
                    div()
                        .id("body")
                        .flex_1()
                        .min_w_0()
                        .min_h_0()
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .key_context(context)
                        .track_focus(&self.focus)
                        .child(crate::preferences::render(self, window, cx)),
                )
                .into_any_element()
        } else if pane {
            div()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .flex()
                .flex_col()
                .child(self.window_strip(window, cx))
                .child(
                    div()
                        .id("body")
                        .flex_1()
                        .min_w_0()
                        .min_h_0()
                        .flex()
                        .key_context(context)
                        .track_focus(&self.focus)
                        .child(self.render_list_column(window, cx))
                        .child(self.render_pane(window, cx)),
                )
                .into_any_element()
        } else if open {
            div()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .flex()
                .flex_col()
                .child(self.window_strip(window, cx))
                .child(
                    div()
                        .id("body")
                        .flex_1()
                        .min_w_0()
                        .min_h_0()
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .key_context(context)
                        .track_focus(&self.focus)
                        .child(self.render_thread(window, cx)),
                )
                .into_any_element()
        } else {
            div()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .flex()
                .flex_col()
                .child(self.window_strip(window, cx))
                .child(
                    div()
                        .id("body")
                        .flex_1()
                        .min_w_0()
                        .min_h_0()
                        .flex()
                        .overflow_hidden()
                        .key_context(context)
                        .track_focus(&self.focus)
                        .child(self.render_list_column(window, cx)),
                )
                .into_any_element()
        };
        div()
            .size_full()
            .relative()
            .flex()
            .bg(theme::canvas(&self.theme))
            .text_color(self.theme.fg)
            .font_family(theme::font(&self.settings).to_string())
            .text_size(self.theme.px(14.))
            .key_context("Kite")
            .on_key_down(cx.listener(Self::key_down))
            .on_action(cx.listener(Self::action))
            // Settings takes the whole window, so the mail rail steps aside there.
            .when(!preferences_open, |d| {
                d.children(self.sidebar.as_ref().map(|sidebar| {
                    sidebar.clone().cached(
                        StyleRefinement::default()
                            .w(px(crate::sidebar::RAIL_W))
                            .h_full()
                            .flex_shrink_0(),
                    )
                }))
            })
            .child(content)
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
            Overlay::Mailboxes(_) => Some(crate::sidebar::mailbox_menu(self, window, cx)),
            Overlay::Help => Some(self.render_help(cx)),
            Overlay::Palette(_) => Some(palette::render(self, window, cx)),
            Overlay::Snooze(_) => Some(snooze::render(self, window, cx)),
            Overlay::Move(_) | Overlay::CreateFolder(_) => Some(folders::render(self, window, cx)),
            Overlay::Tags(_) | Overlay::NewTag(_) => Some(tags::render(self, window, cx)),
        }
    }
}
