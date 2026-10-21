//! The command table. One list drives `cx.bind_keys`, the command palette, the cheat sheet
//! and the shortcut editor.
use crate::app::Kite;
use gpui_kit::*;
use kite_core::model::{Mailbox, Settings};
use std::rc::Rc;

/// Command ids carried by [`Act`], so one handler serves every command.
pub mod cmd {
    pub const NEXT: u8 = 0;
    pub const PREV: u8 = 1;
    pub const OPEN: u8 = 2;
    pub const BACK: u8 = 3;
    pub const DONE: u8 = 4;
    pub const INBOX: u8 = 5;
    pub const TRASH: u8 = 6;
    pub const SNOOZE: u8 = 7;
    pub const STAR: u8 = 8;
    pub const TOGGLE_READ: u8 = 9;
    pub const SELECT: u8 = 10;
    pub const SEARCH: u8 = 11;
    pub const PALETTE: u8 = 13;
    pub const NEXT_SPLIT: u8 = 14;
    pub const PREV_SPLIT: u8 = 15;
    pub const UNDO: u8 = 16;
    pub const HELP: u8 = 17;
    pub const UNIFIED: u8 = 18;
    pub const MOVE: u8 = 19;
    /// Go-to mailboxes, then account switching at `ACCOUNT + n`.
    pub const GO: u8 = 20;
    pub const CREATE_FOLDER: u8 = 30;
    pub const SETTINGS: u8 = 31;
    pub const ACCOUNT_SWITCHER: u8 = 32;
    pub const MANAGE_ACCOUNTS: u8 = 33;
    pub const REFRESH: u8 = 34;
    pub const ACCOUNT: u8 = 40;
    pub const COMPOSE: u8 = 50;
    pub const REPLY: u8 = 51;
    pub const REPLY_ALL: u8 = 52;
    pub const FORWARD: u8 = 53;
    pub const SEND: u8 = 54;
    pub const COMPOSE_CLOSE: u8 = 55;
    pub const SHOW_CC: u8 = 56;
    pub const SEND_LATER: u8 = 57;
    pub const DISCARD: u8 = 58;
    pub const AI_WRITE: u8 = 59;
    pub const AI_SUMMARY: u8 = 60;
    pub const TAGS: u8 = 61;
    pub const UNSUBSCRIBE: u8 = 62;
    pub const INSERT_SNIPPET: u8 = 63;
    pub const ZERO: u8 = 64;
    pub const ASK: u8 = 65;
    /// Not in the command table: reply suggestions are a click-only helper.
    pub const SUGGEST: u8 = 66;
    pub const GO_WAITING: u8 = 67;
    pub const GO_SCHEDULED: u8 = 68;
    pub const DONE_WAITING: u8 = 69;
    pub const CANCEL_SCHEDULE: u8 = 70;
    pub const RESCHEDULE: u8 = 71;
    pub const SEND_NOW: u8 = 72;
}

/// The section a command is listed under in the palette, the cheat sheet and Settings.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Group {
    Navigation,
    Triage,
    Compose,
    GoTo,
    Accounts,
    App,
}

impl Group {
    pub const ALL: [Self; 6] = [
        Self::Navigation,
        Self::Triage,
        Self::Compose,
        Self::GoTo,
        Self::Accounts,
        Self::App,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Navigation => "Navigation",
            Self::Triage => "Triage",
            Self::Compose => "Compose",
            Self::GoTo => "Go to",
            Self::Accounts => "Accounts",
            Self::App => "App",
        }
    }
}

pub struct Cmd {
    /// Stable identifier stored in `Settings::shortcuts`.
    pub slug: &'static str,
    pub title: &'static str,
    /// Default keystroke strings, as bound. The first is the one shown in hints.
    pub keys: &'static [&'static str],
    /// Key context the binding lives in.
    pub ctx: &'static str,
    pub group: Group,
    pub id: u8,
}

const fn c(
    slug: &'static str,
    title: &'static str,
    keys: &'static [&'static str],
    ctx: &'static str,
    group: Group,
    id: u8,
) -> Cmd {
    Cmd {
        slug,
        title,
        keys,
        ctx,
        group,
        id,
    }
}

pub static COMMANDS: &[Cmd] = &[
    c(
        "go_waiting",
        "Go to Waiting on",
        &["g w"],
        "Kite",
        Group::GoTo,
        cmd::GO_WAITING,
    ),
    c(
        "go_scheduled",
        "Go to Scheduled",
        &["g l"],
        "Kite",
        Group::GoTo,
        cmd::GO_SCHEDULED,
    ),
    c(
        "done_waiting",
        "Done waiting",
        &[],
        "Kite",
        Group::Triage,
        cmd::DONE_WAITING,
    ),
    c(
        "cancel_schedule",
        "Cancel schedule",
        &[],
        "Kite",
        Group::Triage,
        cmd::CANCEL_SCHEDULE,
    ),
    c(
        "reschedule",
        "Reschedule",
        &[],
        "Kite",
        Group::Triage,
        cmd::RESCHEDULE,
    ),
    c(
        "send_now",
        "Send now",
        &[],
        "Kite",
        Group::Triage,
        cmd::SEND_NOW,
    ),
    c(
        "unsubscribe",
        "Unsubscribe",
        &["secondary-u"],
        "Kite",
        Group::Triage,
        cmd::UNSUBSCRIBE,
    ),
    c(
        "refresh",
        "Refresh mail",
        &["secondary-r"],
        "Kite",
        Group::App,
        cmd::REFRESH,
    ),
    c(
        "manage_accounts",
        "Manage accounts",
        &["secondary-shift-,"],
        "Kite",
        Group::Accounts,
        cmd::MANAGE_ACCOUNTS,
    ),
    c(
        "settings",
        "Settings",
        &["secondary-,"],
        "Kite",
        Group::App,
        cmd::SETTINGS,
    ),
    c(
        "switch_account",
        "Switch account",
        &["secondary-shift-a"],
        "Kite",
        Group::Accounts,
        cmd::ACCOUNT_SWITCHER,
    ),
    c(
        "next_thread",
        "Next thread",
        &["j"],
        "Kite",
        Group::Navigation,
        cmd::NEXT,
    ),
    c(
        "previous_thread",
        "Previous thread",
        &["k"],
        "Kite",
        Group::Navigation,
        cmd::PREV,
    ),
    c(
        "open_thread",
        "Open thread",
        &["enter", "o"],
        "KiteList",
        Group::Navigation,
        cmd::OPEN,
    ),
    c(
        "back",
        "Back",
        &["escape"],
        "Kite",
        Group::Navigation,
        cmd::BACK,
    ),
    c(
        "back_to_list",
        "Back to list",
        &["u"],
        "KiteThread",
        Group::Navigation,
        cmd::BACK,
    ),
    c(
        "mark_done",
        "Mark done",
        &["e"],
        "KiteList",
        Group::Triage,
        cmd::DONE,
    ),
    c(
        "move_to_inbox",
        "Move back to inbox",
        &["shift-e"],
        "KiteList",
        Group::Triage,
        cmd::INBOX,
    ),
    c(
        "trash",
        "Trash",
        &["#", "shift-3", "delete"],
        "KiteList",
        Group::Triage,
        cmd::TRASH,
    ),
    c(
        "snooze",
        "Snooze / remind",
        &["h"],
        "KiteList",
        Group::Triage,
        cmd::SNOOZE,
    ),
    c("star", "Star", &["s"], "KiteList", Group::Triage, cmd::STAR),
    c(
        "toggle_read",
        "Toggle read",
        &["shift-u"],
        "KiteList",
        Group::Triage,
        cmd::TOGGLE_READ,
    ),
    c(
        "move_to_folder",
        "Move to folder",
        &["l"],
        "KiteList",
        Group::Triage,
        cmd::MOVE,
    ),
    c(
        "tags",
        "Tag conversation",
        &["shift-l"],
        "KiteList",
        Group::Triage,
        cmd::TAGS,
    ),
    c(
        "new_folder",
        "New folder",
        &["secondary-shift-n"],
        "Kite",
        Group::App,
        cmd::CREATE_FOLDER,
    ),
    c(
        "select",
        "Select",
        &["x"],
        "KiteList",
        Group::Triage,
        cmd::SELECT,
    ),
    c(
        "search",
        "Search",
        &["/"],
        "KiteList",
        Group::App,
        cmd::SEARCH,
    ),
    c(
        "palette",
        "Command palette",
        &["secondary-k"],
        "Kite",
        Group::App,
        cmd::PALETTE,
    ),
    c(
        "compose",
        "Compose",
        &["c"],
        "Kite",
        Group::Compose,
        cmd::COMPOSE,
    ),
    c("reply", "Reply", &["r"], "Kite", Group::Compose, cmd::REPLY),
    c(
        "reply_all",
        "Reply all",
        &["a"],
        "Kite",
        Group::Compose,
        cmd::REPLY_ALL,
    ),
    c(
        "forward",
        "Forward",
        &["f"],
        "Kite",
        Group::Compose,
        cmd::FORWARD,
    ),
    c(
        "send",
        "Send",
        &["secondary-enter"],
        "KiteCompose",
        Group::Compose,
        cmd::SEND,
    ),
    c(
        "close_compose",
        "Close compose",
        &["escape"],
        "KiteCompose",
        Group::Compose,
        cmd::COMPOSE_CLOSE,
    ),
    c(
        "show_cc",
        "Show Cc",
        &["secondary-shift-c"],
        "KiteCompose",
        Group::Compose,
        cmd::SHOW_CC,
    ),
    c(
        "next_split",
        "Next split",
        &["tab"],
        "Kite",
        Group::Navigation,
        cmd::NEXT_SPLIT,
    ),
    c(
        "previous_split",
        "Previous split",
        &["shift-tab"],
        "Kite",
        Group::Navigation,
        cmd::PREV_SPLIT,
    ),
    c("undo", "Undo", &["z"], "Kite", Group::Triage, cmd::UNDO),
    c(
        "go_inbox",
        "Go to inbox",
        &["g i"],
        "Kite",
        Group::GoTo,
        cmd::GO,
    ),
    c(
        "go_starred",
        "Go to starred",
        &["g s"],
        "Kite",
        Group::GoTo,
        cmd::GO + 1,
    ),
    c(
        "go_drafts",
        "Go to drafts",
        &["g d"],
        "Kite",
        Group::GoTo,
        cmd::GO + 2,
    ),
    c(
        "go_sent",
        "Go to sent",
        &["g t"],
        "Kite",
        Group::GoTo,
        cmd::GO + 3,
    ),
    c(
        "go_snoozed",
        "Go to snoozed",
        &["g h"],
        "Kite",
        Group::GoTo,
        cmd::GO + 4,
    ),
    c(
        "go_all_mail",
        "Go to all mail",
        &["g a"],
        "Kite",
        Group::GoTo,
        cmd::GO + 5,
    ),
    c(
        "go_archive",
        "Go to archive",
        &["g e"],
        "Kite",
        Group::GoTo,
        cmd::GO + 6,
    ),
    c(
        "go_spam",
        "Go to spam",
        &["g p"],
        "Kite",
        Group::GoTo,
        cmd::GO + 7,
    ),
    c(
        "go_trash",
        "Go to trash",
        &["g b"],
        "Kite",
        Group::GoTo,
        cmd::GO + 8,
    ),
    c(
        "all_inboxes",
        "All inboxes",
        &["secondary-0"],
        "Kite",
        Group::Accounts,
        cmd::UNIFIED,
    ),
    c(
        "shortcut_help",
        "Shortcut help",
        &["shift-/"],
        "Kite",
        Group::App,
        cmd::HELP,
    ),
    c(
        "account_1",
        "Account 1",
        &["secondary-1"],
        "Kite",
        Group::Accounts,
        cmd::ACCOUNT,
    ),
    c(
        "account_2",
        "Account 2",
        &["secondary-2"],
        "Kite",
        Group::Accounts,
        cmd::ACCOUNT + 1,
    ),
    c(
        "account_3",
        "Account 3",
        &["secondary-3"],
        "Kite",
        Group::Accounts,
        cmd::ACCOUNT + 2,
    ),
    c(
        "account_4",
        "Account 4",
        &["secondary-4"],
        "Kite",
        Group::Accounts,
        cmd::ACCOUNT + 3,
    ),
    c(
        "account_5",
        "Account 5",
        &["secondary-5"],
        "Kite",
        Group::Accounts,
        cmd::ACCOUNT + 4,
    ),
    c(
        "account_6",
        "Account 6",
        &["secondary-6"],
        "Kite",
        Group::Accounts,
        cmd::ACCOUNT + 5,
    ),
    c(
        "account_7",
        "Account 7",
        &["secondary-7"],
        "Kite",
        Group::Accounts,
        cmd::ACCOUNT + 6,
    ),
    c(
        "account_8",
        "Account 8",
        &["secondary-8"],
        "Kite",
        Group::Accounts,
        cmd::ACCOUNT + 7,
    ),
    c(
        "account_9",
        "Account 9",
        &["secondary-9"],
        "Kite",
        Group::Accounts,
        cmd::ACCOUNT + 8,
    ),
    c(
        "send_later",
        "Send later",
        &["secondary-shift-enter"],
        "KiteCompose",
        Group::Compose,
        cmd::SEND_LATER,
    ),
    c(
        "discard_draft",
        "Discard draft",
        &["secondary-shift-d"],
        "KiteCompose",
        Group::Compose,
        cmd::DISCARD,
    ),
    c(
        "ai_write",
        "Write with AI",
        &["secondary-j"],
        "KiteCompose",
        Group::Compose,
        cmd::AI_WRITE,
    ),
    c(
        "ai_summary",
        "Summarize thread",
        &["shift-s"],
        "KiteThread",
        Group::Compose,
        cmd::AI_SUMMARY,
    ),
    c(
        "insert_snippet",
        "Insert snippet…",
        &["secondary-;"],
        "KiteCompose",
        Group::Compose,
        cmd::INSERT_SNIPPET,
    ),
    c(
        "ask_inbox",
        "Ask your inbox",
        &["secondary-shift-k"],
        "Kite",
        Group::App,
        cmd::ASK,
    ),
    // Palette-only: there is no default key, so the cheat sheet never lists it.
    c(
        "archive_older",
        "Archive older than…",
        &[],
        "Kite",
        Group::Triage,
        cmd::ZERO,
    ),
];

/// The mailboxes reachable through the `g` sequences, in the same order as the table above.
pub const GO_MAILBOXES: [(Mailbox, &str); 9] = [
    (Mailbox::Inbox, "Inbox"),
    (Mailbox::Starred, "Starred"),
    (Mailbox::Drafts, "Drafts"),
    (Mailbox::Sent, "Sent"),
    (Mailbox::Snoozed, "Snoozed"),
    (Mailbox::All, "All Mail"),
    (Mailbox::Archive, "Archive"),
    (Mailbox::Spam, "Spam"),
    (Mailbox::Trash, "Trash"),
];

/// Whether a command only exists while Settings › AI is on.
pub fn needs_ai(id: u8) -> bool {
    matches!(id, cmd::AI_WRITE | cmd::AI_SUMMARY | cmd::ASK)
}

/// Whether a command is offered at all. The AI helpers only exist once the reader turned AI on,
/// so the palette and the cheat sheet never advertise a command that cannot run.
pub fn available(settings: &Settings, command: &Cmd) -> bool {
    settings.ai_enabled || !needs_ai(command.id)
}

/// The command carrying `id`.
pub fn command(id: u8) -> Option<&'static Cmd> {
    COMMANDS.iter().find(|command| command.id == id)
}

/// Classic (click-first) mode unbinds single-key shortcuts so typing never runs a command.
/// Modifier chords, Escape, Enter, Delete and the navigation keys stay.
fn mode_keeps(key: &str) -> bool {
    let Ok(stroke) = Keystroke::parse(key) else {
        return true;
    };
    stroke.modifiers.control
        || stroke.modifiers.platform
        || stroke.modifiers.alt
        || matches!(
            stroke.key.as_str(),
            "escape"
                | "enter"
                | "delete"
                | "up"
                | "down"
                | "left"
                | "right"
                | "home"
                | "end"
                | "pageup"
                | "pagedown"
        )
}

/// The keystrokes currently bound to `command`: user overrides first, then the defaults,
/// filtered by the input mode. An override is always bound as written — the user asked for it.
pub fn bound(settings: &Settings, command: &Cmd) -> Vec<String> {
    let Some(keys) = settings.shortcuts.get(command.slug) else {
        return command
            .keys
            .iter()
            .filter(|key| settings.input_mode != "classic" || mode_keeps(key))
            .map(|key| (*key).to_string())
            .collect();
    };
    keys.clone()
}

/// The first current keystroke, for a button's key hint. Empty when the command is unbound.
pub fn hint(settings: &Settings, command: &Cmd) -> String {
    bound(settings, command)
        .into_iter()
        .next()
        .unwrap_or_default()
}

/// The current key hint for a command id.
pub fn hint_for(settings: &Settings, id: u8) -> String {
    command(id)
        .map(|command| hint(settings, command))
        .unwrap_or_default()
}

/// A binding string ("secondary-u", "g i") as a human reads it ("Ctrl+U", "G I"), for the
/// shortcut tips. Falls back to the raw binding for anything unparseable.
pub fn pretty(binding: &str) -> String {
    binding
        .split_whitespace()
        .map(|key| {
            Keystroke::parse(key)
                .ok()
                .as_ref()
                .map(gpui_kit::component::kbd::Kbd::format)
                .unwrap_or_else(|| key.to_string())
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The verb phrase a shortcut tip teaches: "Tip: press E to archive".
pub fn verb(command: &Cmd) -> String {
    match command.slug {
        "mark_done" => "archive",
        "move_to_inbox" => "move it back to the inbox",
        "snooze" => "snooze it",
        "toggle_read" => "toggle read",
        "move_to_folder" => "move it to a folder",
        "tags" => "tag it",
        "open_thread" => "open it",
        "back" | "back_to_list" => "go back",
        "select" => "select it",
        "settings" => "open Settings",
        "palette" => "open the palette",
        "shortcut_help" => "see every shortcut",
        "insert_snippet" => "insert a snippet",
        "new_folder" => "create a folder",
        "switch_account" => "switch account",
        _ => command.title,
    }
    .to_lowercase()
}

/// A recorded keystroke as a binding string that survives Settings and a restart.
pub fn source(stroke: &Keystroke) -> String {
    let mut source = String::new();
    // `secondary` is Ctrl on Windows and Cmd on macOS, so a recorded chord follows the platform.
    if stroke.modifiers.control || stroke.modifiers.platform {
        source.push_str("secondary-");
    }
    if stroke.modifiers.alt {
        source.push_str("alt-");
    }
    if stroke.modifiers.function {
        source.push_str("fn-");
    }
    if stroke.modifiers.shift {
        source.push_str("shift-");
    }
    source.push_str(&stroke.key);
    source
}

/// The title of another command that already uses `stroke` in the same scope, so an edit can
/// explain the clash instead of silently shadowing it.
pub fn conflict(settings: &Settings, slug: &str, stroke: &str) -> Option<&'static str> {
    let compose = |ctx: &str| ctx.starts_with("KiteCompose");
    let target = COMMANDS.iter().find(|command| command.slug == slug)?;
    COMMANDS
        .iter()
        .filter(|command| command.slug != slug && compose(command.ctx) == compose(target.ctx))
        .find(|command| bound(settings, command).iter().any(|key| key == stroke))
        .map(|command| command.title)
}

/// The bindings gpui-kit's components install at startup (text fields, menus, lists).
/// Captured once, before Kite binds anything, so a rebind can replace our own keys without
/// leaving text inputs without theirs.
struct ComponentKeys(Rc<[KeyBinding]>);

impl Global for ComponentKeys {}

/// Applies the keymap for `settings`. The single place that turns defaults, user overrides and
/// the input mode into bindings, so a change takes effect on the next keystroke.
pub fn bind(cx: &mut App, settings: &Settings) {
    let base = match cx.try_global::<ComponentKeys>() {
        Some(base) => base.0.clone(),
        None => {
            let captured: Rc<[KeyBinding]> = {
                let keymap = cx.key_bindings();
                let keymap = keymap.borrow();
                keymap.bindings().cloned().collect()
            };
            cx.set_global(ComponentKeys(captured.clone()));
            captured
        }
    };
    cx.clear_key_bindings();
    cx.bind_keys(base.iter().cloned().chain(bindings(settings)));
}

fn bindings(settings: &Settings) -> Vec<KeyBinding> {
    let mut bindings = Vec::with_capacity(COMMANDS.len() * 2 + 3);
    for command in COMMANDS {
        for key in bound(settings, command) {
            let act = Act { cmd: command.id };
            bindings.push(KeyBinding::new(&key, act.clone(), Some(context(command))));
            // Compose fields focus an Input, which binds Enter, Escape and
            // friends at a deeper context. Re-bind at that depth so the field
            // keys win; later bindings win a tie.
            if command.ctx == "KiteCompose" {
                bindings.push(KeyBinding::new(
                    &key,
                    act.clone(),
                    Some("KiteCompose > Input"),
                ));
            }
            // Reading actions must not match while the titlebar search input has focus.
            if matches!(
                command.id,
                cmd::DONE
                    | cmd::INBOX
                    | cmd::TRASH
                    | cmd::SNOOZE
                    | cmd::STAR
                    | cmd::TOGGLE_READ
                    | cmd::MOVE
                    | cmd::TAGS
            ) {
                bindings.push(KeyBinding::new(&key, act, Some("KiteThread")));
            }
        }
    }
    // The arrows move the selection the way a click-first client expects, in every input mode.
    // They stay list keys, so reading a long thread still scrolls with the same keys.
    for (key, id) in [("up", cmd::PREV), ("down", cmd::NEXT)] {
        bindings.push(KeyBinding::new(key, Act { cmd: id }, Some("KiteList")));
    }
    bindings
}

fn context(command: &Cmd) -> &'static str {
    if command.ctx == "Kite"
        && !matches!(
            command.id,
            cmd::BACK | cmd::SETTINGS | cmd::PALETTE | cmd::ACCOUNT_SWITCHER
        )
    {
        "Kite && !Input"
    } else {
        command.ctx
    }
}

#[derive(Clone, PartialEq, Debug, gpui_kit::Action)]
#[action(namespace = kite, no_json)]
pub struct Act {
    pub cmd: u8,
}

impl Kite {
    /// Re-applies the keymap after a settings change, so a mode switch or an edited shortcut
    /// works without a restart.
    pub(crate) fn rebind(&mut self, cx: &mut App) {
        bind(cx, &self.settings);
    }
}
