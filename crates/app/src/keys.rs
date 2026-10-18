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
    pub const UNIFIED: u8 = 18;
    pub const MOVE: u8 = 19;
    /// Go-to mailboxes, then account switching at `ACCOUNT + n`.
    pub const GO: u8 = 20;
    pub const CREATE_FOLDER: u8 = 30;
    pub const SETTINGS: u8 = 31;
    pub const ACCOUNT_SWITCHER: u8 = 32;
    pub const MANAGE_ACCOUNTS: u8 = 33;
    pub const ACCOUNT: u8 = 40;
    pub const COMPOSE: u8 = 50;
    pub const REPLY: u8 = 51;
    pub const REPLY_ALL: u8 = 52;
    pub const FORWARD: u8 = 53;
    pub const SEND: u8 = 54;
    pub const COMPOSE_CLOSE: u8 = 55;
    pub const SHOW_CC: u8 = 56;
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
    c(
        "Manage accounts",
        &["secondary-shift-,"],
        "Kite",
        cmd::MANAGE_ACCOUNTS,
    ),
    c("Settings", &["secondary-,"], "Kite", cmd::SETTINGS),
    c(
        "Switch account",
        &["secondary-shift-a"],
        "Kite",
        cmd::ACCOUNT_SWITCHER,
    ),
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
    c("Move to folder", &["l"], "KiteList", cmd::MOVE),
    c(
        "New folder",
        &["secondary-shift-n"],
        "Kite",
        cmd::CREATE_FOLDER,
    ),
    c("Select", &["x"], "KiteList", cmd::SELECT),
    c("Search", &["/"], "KiteList", cmd::SEARCH),
    c("Command palette", &["secondary-k"], "Kite", cmd::PALETTE),
    c("Compose", &["c"], "Kite", cmd::COMPOSE),
    c("Reply", &["r"], "Kite", cmd::REPLY),
    c("Reply all", &["a"], "Kite", cmd::REPLY_ALL),
    c("Forward", &["f"], "Kite", cmd::FORWARD),
    c("Send", &["secondary-enter"], "KiteCompose", cmd::SEND),
    c(
        "Close compose",
        &["escape"],
        "KiteCompose",
        cmd::COMPOSE_CLOSE,
    ),
    c("Show Cc", &["secondary-shift-c"], "KiteCompose", cmd::SHOW_CC),
    c("Next split", &["tab"], "Kite", cmd::NEXT_SPLIT),
    c("Previous split", &["shift-tab"], "Kite", cmd::PREV_SPLIT),
    c("Undo", &["z"], "Kite", cmd::UNDO),
    c("Go to inbox", &["g i"], "Kite", cmd::GO),
    c("Go to starred", &["g s"], "Kite", cmd::GO + 1),
    c("Go to drafts", &["g d"], "Kite", cmd::GO + 2),
    c("Go to sent", &["g t"], "Kite", cmd::GO + 3),
    c("Go to snoozed", &["g h"], "Kite", cmd::GO + 4),
    c("Go to all mail", &["g a"], "Kite", cmd::GO + 5),
    c("Go to archive", &["g e"], "Kite", cmd::GO + 6),
    c("Go to spam", &["g p"], "Kite", cmd::GO + 7),
    c("Go to trash", &["g b"], "Kite", cmd::GO + 8),
    c("All inboxes", &["secondary-0"], "Kite", cmd::UNIFIED),
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

#[derive(Clone, PartialEq, Debug, gpui_kit::Action)]
#[action(namespace = kite, no_json)]
pub struct Act {
    pub cmd: u8,
}

pub fn bind(cx: &mut App) {
    let mut bindings = Vec::new();
    for command in COMMANDS {
        for key in command.keys {
            bindings.push(KeyBinding::new(
                key,
                Act { cmd: command.id },
                Some(
                    if command.ctx == "Kite"
                        && !matches!(
                            command.id,
                            cmd::BACK | cmd::SETTINGS | cmd::PALETTE | cmd::ACCOUNT_SWITCHER
                        )
                    {
                        "Kite && !Input"
                    } else {
                        command.ctx
                    },
                ),
            ));
            // Compose fields focus an Input, which binds Enter, Escape and
            // friends at a deeper context. Re-bind at that depth so the field
            // keys win; later bindings win a tie.
            if command.ctx == "KiteCompose" {
                bindings.push(KeyBinding::new(
                    key,
                    Act { cmd: command.id },
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
            ) {
                bindings.push(KeyBinding::new(
                    key,
                    Act { cmd: command.id },
                    Some("KiteThread"),
                ));
            }
        }
    }
    cx.bind_keys(bindings);
}
