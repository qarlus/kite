//! The command table. One list drives `cx.bind_keys`, the command palette and the cheat sheet.
use gpui_kit::*;
use kite_core::model::Mailbox;

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
    /// Go-to mailboxes, then account switching at `ACCOUNT + n`.
    pub const GO: u8 = 20;
    pub const ACCOUNT: u8 = 40;
}

pub struct Cmd {
    pub title: &'static str,
    /// Keystroke strings, as bound. The first is the one shown in hints.
    pub keys: &'static [&'static str],
    /// Key context the binding lives in.
    pub ctx: &'static str,
    pub id: u8,
}

const fn c(title: &'static str, keys: &'static [&'static str], ctx: &'static str, id: u8) -> Cmd {
    Cmd {
        title,
        keys,
        ctx,
        id,
    }
}

pub static COMMANDS: &[Cmd] = &[
    c("Next thread", &["j"], "Kite", cmd::NEXT),
    c("Previous thread", &["k"], "Kite", cmd::PREV),
    c("Open thread", &["enter", "o"], "KiteList", cmd::OPEN),
    c("Back", &["escape"], "Kite", cmd::BACK),
    c("Back to list", &["u"], "KiteThread", cmd::BACK),
    c("Mark done", &["e"], "KiteList", cmd::DONE),
    c("Move back to inbox", &["shift-e"], "KiteList", cmd::INBOX),
    c("Trash", &["#", "shift-3"], "KiteList", cmd::TRASH),
    c("Snooze / remind", &["h"], "KiteList", cmd::SNOOZE),
    c("Star", &["s"], "KiteList", cmd::STAR),
    c("Toggle read", &["shift-u"], "KiteList", cmd::TOGGLE_READ),
    c("Select", &["x"], "KiteList", cmd::SELECT),
    c("Search", &["/"], "KiteList", cmd::SEARCH),
    c("Command palette", &["secondary-k"], "Kite", cmd::PALETTE),
    c("Next split", &["tab"], "Kite", cmd::NEXT_SPLIT),
    c("Previous split", &["shift-tab"], "Kite", cmd::PREV_SPLIT),
    c("Undo", &["z"], "Kite", cmd::UNDO),
    c("Go to inbox", &["g i"], "Kite", cmd::GO),
    c("Go to starred", &["g s"], "Kite", cmd::GO + 1),
    c("Go to drafts", &["g d"], "Kite", cmd::GO + 2),
    c("Go to sent", &["g t"], "Kite", cmd::GO + 3),
    c("Go to snoozed", &["g h"], "Kite", cmd::GO + 4),
    c("Go to all mail", &["g a"], "Kite", cmd::GO + 5),
    c("Shortcut help", &["shift-/"], "Kite", cmd::HELP),
    c("Account 1", &["secondary-1"], "Kite", cmd::ACCOUNT),
    c("Account 2", &["secondary-2"], "Kite", cmd::ACCOUNT + 1),
    c("Account 3", &["secondary-3"], "Kite", cmd::ACCOUNT + 2),
    c("Account 4", &["secondary-4"], "Kite", cmd::ACCOUNT + 3),
    c("Account 5", &["secondary-5"], "Kite", cmd::ACCOUNT + 4),
    c("Account 6", &["secondary-6"], "Kite", cmd::ACCOUNT + 5),
    c("Account 7", &["secondary-7"], "Kite", cmd::ACCOUNT + 6),
    c("Account 8", &["secondary-8"], "Kite", cmd::ACCOUNT + 7),
    c("Account 9", &["secondary-9"], "Kite", cmd::ACCOUNT + 8),
];

/// The mailboxes reachable through the `g` sequences, in the same order as the table above.
pub const GO_MAILBOXES: [(Mailbox, &str); 6] = [
    (Mailbox::Inbox, "Inbox"),
    (Mailbox::Starred, "Starred"),
    (Mailbox::Drafts, "Drafts"),
    (Mailbox::Sent, "Sent"),
    (Mailbox::Snoozed, "Snoozed"),
    (Mailbox::All, "All Mail"),
];

#[derive(Clone, PartialEq, Debug, gpui_kit::Action)]
#[action(namespace = kite, no_json)]
pub struct Act {
    pub cmd: u8,
}

pub fn bind(cx: &mut App) {
    cx.bind_keys(
        COMMANDS
            .iter()
            .flat_map(|c| {
                c.keys
                    .iter()
                    .map(move |keys| KeyBinding::new(keys, Act { cmd: c.id }, Some(c.ctx)))
            })
            .collect::<Vec<_>>(),
    );
}
