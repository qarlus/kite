//! The virtualised thread list, the empty states and the shortcut cheat sheet.
use crate::app::Kite;
use crate::keys::{self, Cmd, Group};
use crate::theme::{RADIUS, RADIUS_SM, ROW_H, Theme};
use chrono::{DateTime, Datelike, Local, TimeZone as _, Timelike as _};
use gpui_kit::assets::IconName;
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::{Icon, Sizable};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kite_core::model::{Action as MailAction, Id, Mailbox, Ms, ThreadSummary};
use std::ops::Range;
use std::time::Duration;

/// A row's own timestamp: `9:41` today, `Yesterday`, a weekday inside the last week,
/// then `Oct 2` this year and `Mar 3, 2025` across a year boundary.
pub fn when(ms: Ms, now: DateTime<Local>) -> String {
    let Some(at) = at_or_now(ms, now) else {
        return String::new();
    };
    match days_ago(at, now) {
        0 => format!("{}:{:02}", at.time().hour(), at.time().minute()),
        1 => "Yesterday".to_string(),
        2..=6 => at.format("%a").to_string(),
        _ if at.year() == now.year() => format!("{} {}", at.format("%b"), at.day()),
        _ => format!("{} {}, {}", at.format("%b"), at.day(), at.year()),
    }
}

/// A sender's wrong clock (or a still-importing message) is filed under today, as mail clients do.
fn at_or_now(ms: Ms, now: DateTime<Local>) -> Option<DateTime<Local>> {
    let at = Local.timestamp_millis_opt(ms).single()?;
    Some(if at > now { now } else { at })
}

fn days_ago(at: DateTime<Local>, now: DateTime<Local>) -> i64 {
    (now.date_naive() - at.date_naive()).num_days()
}

impl Kite {
    pub fn render_list(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if self.rows.is_empty() {
            return self.render_empty(cx);
        }
        let scroll = self.list_scroll.clone();
        let list = uniform_list(
            "list",
            self.rows.len(),
            cx.processor(
                move |this: &mut Kite,
                      range: Range<usize>,
                      _window: &mut Window,
                      cx: &mut Context<Kite>| {
                    if !this.loaded_all && range.end + 50 >= this.rows.len() {
                        this.load_more(cx);
                    }
                    let now = Local::now();
                    let mut items = Vec::with_capacity(range.len());
                    for ix in range {
                        let Some(row) = this.rows.get(ix) else {
                            continue;
                        };
                        items.push(row_element(this, row, ix, now, cx));
                    }
                    items
                },
            ),
        )
        .track_scroll(&scroll)
        .w_full()
        .h_full();
        div()
            .flex_1()
            .flex()
            .flex_col()
            .min_h_0()
            .overflow_hidden()
            .pb_2()
            .child(list)
            .into_any_element()
    }

    fn render_empty(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme;
        let folder = self.folders.iter().find(|folder| {
            Some(folder.account_id) == self.view.account_id
                && Some(&folder.id) == self.view.folder.as_ref()
        });
        let (title, note) = if self.view.query.is_some() {
            (
                "No results".to_string(),
                "Try another name, subject, or search term.".to_string(),
            )
        } else if let Some(folder) = folder {
            let move_hint = keys::hint_for(&self.settings, keys::cmd::MOVE);
            (
                format!("{} is empty", folder.name),
                if move_hint.is_empty() {
                    "Move mail here with the folder menu.".to_string()
                } else {
                    format!("Move mail here with the folder menu or press {move_hint}")
                },
            )
        } else if self.accounts.is_empty() {
            (
                "Welcome to Kite".to_string(),
                "Connect Proton Bridge or Outlook to start syncing your mail.".to_string(),
            )
        } else {
            match self.view.mailbox {
                Mailbox::Inbox => (
                    "Inbox Zero".to_string(),
                    "You're all caught up. Enjoy the quiet.".to_string(),
                ),
                Mailbox::Starred => (
                    "No starred mail".to_string(),
                    "Star a conversation to find it here.".to_string(),
                ),
                Mailbox::Snoozed => (
                    "Nothing snoozed".to_string(),
                    "Conversations you snooze will wait here.".to_string(),
                ),
                Mailbox::Drafts => (
                    "No drafts".to_string(),
                    "Your unfinished messages will appear here.".to_string(),
                ),
                Mailbox::Sent => (
                    "No sent mail".to_string(),
                    "Messages you send will appear here.".to_string(),
                ),
                Mailbox::Archive => (
                    "Nothing archived".to_string(),
                    "Conversations you mark done are filed here.".to_string(),
                ),
                Mailbox::Spam => (
                    "No spam".to_string(),
                    "A little less noise. Nothing to review here.".to_string(),
                ),
                Mailbox::Trash => (
                    "Trash is empty".to_string(),
                    "Deleted conversations will appear here.".to_string(),
                ),
                Mailbox::All => (
                    "No mail".to_string(),
                    "Nothing has arrived in this account yet.".to_string(),
                ),
            }
        };
        div()
            .id("inbox-zero")
            .test_support()
            .aria_label(title.clone())
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_3()
            .child(
                div()
                    .size(px(44.))
                    .rounded_full()
                    .bg(t.bg2)
                    .flex()
                    .items_center()
                    .justify_center()
                    .mb_2()
                    .child(
                        Icon::new(
                            crate::sidebar::MAILBOXES
                                .iter()
                                .find(|(mailbox, _, _)| *mailbox == self.view.mailbox)
                                .map(|(_, _, icon)| *icon)
                                .unwrap_or(IconName::Inbox),
                        )
                        .with_size(px(22.))
                        .text_color(t.fg3),
                    ),
            )
            .child(
                div()
                    .text_size(t.px(24.))
                    .font_weight(FontWeight(650.))
                    .child(title),
            )
            .child(
                div()
                    .id("empty-note")
                    .test_support()
                    .aria_label(note.clone())
                    .text_size(t.px(14.))
                    .text_color(t.fg2)
                    .child(note),
            )
            .when(self.accounts.is_empty(), |d| {
                d.child(
                    div()
                        .mt_4()
                        .text_size(t.px(12.))
                        .text_color(t.fg2)
                        .id("empty-add-account")
                        .test_support()
                        .aria_label("Add an account")
                        .px_3()
                        .py_2()
                        .rounded(px(6.))
                        .bg(t.bg2)
                        .cursor_pointer()
                        .child("Add an account")
                        .on_click(
                            cx.listener(|this, _, window, cx| this.show_preferences(1, window, cx)),
                        ),
                )
            })
            .into_any_element()
    }

    /// The `?` cheat sheet, generated from the same command table as the keymap, so it always
    /// shows the keys that are bound right now — including the user's own edits.
    pub fn render_help(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme;
        let mut columns = [Vec::new(), Vec::new()];
        for (ix, group) in Group::ALL.into_iter().enumerate() {
            let commands: Vec<(&Cmd, String)> = keys::COMMANDS
                .iter()
                .filter(|command| {
                    command.group == group && keys::available(&self.settings, command)
                })
                .map(|command| (command, keys::bound(&self.settings, command).join(" ")))
                .filter(|(_, keys)| !keys.is_empty())
                .collect();
            if !commands.is_empty() {
                columns[ix / 3].push(help_section(&t, group, commands));
            }
        }
        let note = if self.settings.input_mode == "classic" {
            "Classic (click) mode: single-key shortcuts are off. Ctrl/Cmd chords and the arrows still work."
        } else {
            "Arrows move the selection and Delete trashes it in either input mode."
        };
        overlay(
            &t,
            px(0.),
            true,
            div()
                .id("help")
                .test_support()
                .aria_label("Keyboard shortcuts")
                .w(px(620.))
                .rounded(px(RADIUS))
                .bg(t.bg2)
                .border_1()
                .border_color(t.line)
                .p_5()
                .flex()
                .flex_col()
                .gap_4()
                .child(
                    div()
                        .text_size(t.px(15.))
                        .font_weight(FontWeight(650.))
                        .child("Keyboard shortcuts"),
                )
                .child(
                    div().flex().flex_row().items_start().gap_8().children(
                        columns.into_iter().map(|column| {
                            div().flex().flex_col().flex_1().gap_4().children(column)
                        }),
                    ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_4()
                        .child(
                            div()
                                .flex_1()
                                .text_size(t.px(11.))
                                .text_color(t.fg3)
                                .child(note),
                        )
                        .child(
                            crate::preferences::button("help-close", "Close", t).on_click(
                                cx.listener(|this, _, window, cx| {
                                    this.run(crate::keys::cmd::BACK, window, cx)
                                }),
                            ),
                        ),
                ),
        )
        .into_any_element()
    }
}

/// One labelled block of the cheat sheet.
fn help_section(t: &Theme, group: Group, commands: Vec<(&Cmd, String)>) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .text_size(t.px(11.))
                .font_weight(FontWeight(650.))
                .text_color(t.fg3)
                .child(group.label()),
        )
        .children(commands.into_iter().map(|(command, keys)| {
            div()
                .id(format!("help-{}", command.slug))
                .test_support()
                .aria_label(format!("{}: {keys}", command.title))
                .flex()
                .items_center()
                .justify_between()
                .gap_4()
                .h(px(22.))
                .child(div().text_size(t.px(13.)).child(command.title))
                .child(keys_pills(t, &keys))
        }))
        .into_any_element()
}

/// One list row: the sender and time, the subject and a two-line snippet.
/// Borrows its summary, so painting a frame copies no threads.
fn row_element(
    kite: &Kite,
    row: &ThreadSummary,
    ix: usize,
    now: DateTime<Local>,
    cx: &mut Context<Kite>,
) -> AnyElement {
    let t = kite.theme;
    let selected = ix == kite.sel;
    let checked = kite.multi.contains(&row.id);
    let label = format!("{}: {}", row.participants, row.subject);
    let account = kite
        .accounts
        .iter()
        .find(|account| account.id == row.account_id);
    let row_id = row.id;
    let buttons = row_actions(row);
    let when = when(row.date, now);
    let sender = row.participants.clone();
    let unread = if row.unread {
        format!("Unread: {sender}")
    } else {
        sender.clone()
    };
    let combined = kite.view.account_id.is_none() && kite.accounts.len() > 1;
    let account_email = account
        .map(|account| account.email.clone())
        .unwrap_or_default();
    div()
        .id(("row", ix))
        .test_support()
        .aria_label(label)
        .aria_selected(selected)
        .group("row")
        .relative()
        .flex()
        .flex_row()
        .flex_shrink_0()
        .w_full()
        // Two design units tall, and the same for every row: `uniform_list` measures only the
        // first one, so a taller row here would misalign the whole list.
        .h(px(ROW_H * 2. * t.scale))
        .cursor_pointer()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .h_full()
                .flex()
                .flex_row()
                .gap_2()
                .px_3()
                // Tight enough that sender, subject and a two-line snippet fit the fixed
                // row height instead of clipping the snippet's second line.
                .py_1p5()
                .rounded(px(RADIUS_SM))
                .overflow_hidden()
                .when(selected, |d| d.bg(t.sel))
                .when(!selected, |d| d.hover(|s| s.bg(t.bg3)))
                .child(
                    div()
                        .id(("select-row", ix))
                        .test_support()
                        .aria_label("Select conversation")
                        .aria_selected(checked)
                        .w(px(14.))
                        .h_full()
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            if let Some(index) = this.multi.iter().position(|id| *id == row_id) {
                                this.multi.remove(index);
                            } else {
                                this.multi.push(row_id);
                            }
                            window.focus(&this.focus, cx);
                            cx.notify();
                            cx.stop_propagation();
                        }))
                        .when(checked, |d| {
                            d.child(
                                Icon::new(IconName::Check)
                                    .with_size(px(13.))
                                    .text_color(t.accent),
                            )
                        })
                        .when(!checked, |d| {
                            // Unread is ink weight here, never a coloured dot, so the box
                            // only appears once the pointer is on the row.
                            d.child(
                                div()
                                    .size(px(13.))
                                    .rounded(px(4.))
                                    .border_1()
                                    .border_color(t.line)
                                    .opacity(0.)
                                    .group_hover("row", |s| s.opacity(1.)),
                            )
                        }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .justify_center()
                        .gap_0p5()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .id(("row-sender", ix))
                                        .test_support()
                                        .aria_label(unread)
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_size(t.px(13.))
                                        .font_weight(if row.unread {
                                            FontWeight(650.)
                                        } else {
                                            FontWeight(500.)
                                        })
                                        .text_color(t.fg)
                                        .child(sender),
                                )
                                .when(combined, |d| {
                                    d.child(
                                        div()
                                            .id(("row-account", ix))
                                            .test_support()
                                            .aria_label(account_email.clone())
                                            .max_w(px(140.))
                                            .min_w_0()
                                            .flex_shrink_0()
                                            .truncate()
                                            .text_size(t.px(11.))
                                            .text_color(t.fg2)
                                            .child(account_email.clone()),
                                    )
                                })
                                .child(
                                    div()
                                        .id(("row-time", ix))
                                        .test_support()
                                        .aria_label(when.clone())
                                        .flex_shrink_0()
                                        .text_size(t.px(12.))
                                        .text_color(t.fg3)
                                        .child(when),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1p5()
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_size(t.px(13.))
                                        .text_color(t.fg)
                                        .child(row.subject.clone()),
                                )
                                .children(row.labels.iter().take(1).map(|label| {
                                    div()
                                        .max_w(px(120.))
                                        .min_w_0()
                                        .flex_shrink_0()
                                        .truncate()
                                        .px_2()
                                        .rounded(px(RADIUS_SM))
                                        .bg(t.bg3)
                                        .text_size(t.px(11.))
                                        .text_color(t.fg2)
                                        .child(label.clone())
                                }))
                                .when(row.attachments, |d| {
                                    d.child(
                                        Icon::new(IconName::Paperclip)
                                            .with_size(px(13.))
                                            .text_color(t.fg2),
                                    )
                                })
                                .when(row.starred, |d| {
                                    d.child(
                                        Icon::new(IconName::StarFill)
                                            .with_size(px(13.))
                                            .text_color(t.accent),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .line_clamp(2)
                                .text_size(t.px(12.))
                                .line_height(relative(1.45))
                                .text_color(t.fg2)
                                .child(row.snippet.clone()),
                        ),
                ),
        )
        // DESIGN › Layout: hovering a row replaces the time with the actions a click-first
        // client expects, so every triage action is reachable with the mouse.
        .child(
            div()
                .id(("row-actions", ix))
                .test_support()
                .aria_label("Row actions")
                .absolute()
                .right_1()
                .top_0()
                .h_full()
                .flex()
                .items_center()
                .gap_1()
                .px_1()
                .rounded(px(RADIUS_SM))
                .bg(if selected { t.sel } else { t.bg3 })
                .opacity(0.)
                .group_hover("row", |s| s.opacity(1.))
                .children(
                    buttons
                        .into_iter()
                        .map(|button| row_action(&t, ix, row_id, button, cx)),
                ),
        )
        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
            this.sel = ix;
            window.focus(&this.focus, cx);
            this.open_thread(cx);
        }))
        .into_any_element()
}

/// One button in a row's hover action strip.
struct RowAction {
    name: &'static str,
    icon: IconName,
    label: &'static str,
    action: MailAction,
    /// Whether the action removes the row from the list.
    remove: bool,
    text: &'static str,
}

/// The actions a row offers: the ones a traditional client puts in its toolbar.
fn row_actions(row: &ThreadSummary) -> [RowAction; 4] {
    [
        RowAction {
            name: "row-archive",
            icon: IconName::Archive,
            label: "Archive",
            action: MailAction::Archive,
            remove: true,
            text: "Marked done",
        },
        RowAction {
            name: "row-trash",
            icon: IconName::Trash,
            label: "Trash",
            action: MailAction::Trash,
            remove: true,
            text: "Trashed",
        },
        RowAction {
            name: "row-star",
            icon: IconName::Star,
            label: if row.starred { "Unstar" } else { "Star" },
            action: if row.starred {
                MailAction::Unstar
            } else {
                MailAction::Star
            },
            remove: false,
            text: if row.starred { "Unstarred" } else { "Starred" },
        },
        RowAction {
            name: "row-read",
            icon: IconName::MailOpen,
            label: if row.unread {
                "Mark read"
            } else {
                "Mark unread"
            },
            action: if row.unread {
                MailAction::Read
            } else {
                MailAction::Unread
            },
            remove: false,
            text: if row.unread {
                "Marked read"
            } else {
                "Marked unread"
            },
        },
    ]
}

/// A row action acts on its own row, never on the selection, so the button does what it says.
fn row_action(
    t: &Theme,
    ix: usize,
    row_id: Id,
    button: RowAction,
    cx: &mut Context<Kite>,
) -> AnyElement {
    let RowAction {
        name,
        icon,
        label,
        action,
        remove,
        text,
    } = button;
    div()
        .id((name, ix))
        .test_support()
        .aria_label(label)
        .flex()
        .items_center()
        .justify_center()
        .size(px(24.))
        .rounded(px(RADIUS_SM))
        .cursor_pointer()
        .hover(|s| s.bg(t.bg2))
        .child(Icon::new(icon).with_size(px(14.)).text_color(t.fg2))
        .on_click(cx.listener(move |this, _, window, cx| {
            this.act_on(vec![row_id], action.clone(), remove, text, cx);
            window.focus(&this.focus, cx);
            cx.stop_propagation();
        }))
        .into_any_element()
}

/// A dimmed overlay surface. `offset` pushes its content below the top edge;
/// add `.justify_center()` for a vertically centred panel.
pub fn overlay(
    t: &Theme,
    offset: Pixels,
    centered: bool,
    panel: impl IntoElement,
) -> impl IntoElement {
    // DESIGN › Motion: overlays fade in and rise 4px over 100 ms (skipped under reduced motion).
    let panel = div()
        .with_animation(
            ("overlay-panel", 0usize),
            Animation::new(Duration::from_millis(100)).with_easing(ease_out_quint()),
            |panel, delta| panel.relative().top(px(4. * (1. - delta))).opacity(delta),
        )
        .child(panel);
    div()
        .id("overlay")
        .occlude()
        .test_support()
        .absolute()
        .inset_0()
        .flex()
        .flex_col()
        .items_center()
        .when(centered, |d| d.justify_center())
        .when(!centered, |d| d.pt(offset))
        .bg(Theme::alpha(t.bg, 0.72))
        .child(panel)
}

/// The accent edge that marks the selected row, wherever rows are selected.
pub fn selection_edge(t: &Theme) -> Div {
    div()
        .absolute()
        .left_0()
        .top_0()
        .w(px(2.))
        .h_full()
        .rounded_full()
        .bg(crate::theme::accent_glow(t))
        .shadow(vec![
            BoxShadow::new(px(0.), px(0.), Theme::alpha(t.accent, 0.45)).blur_radius(px(8.)),
        ])
}

/// One `<kbd>` pill per keystroke in a (possibly multi-stroke) binding.
pub fn keys_pills(t: &Theme, binding: &str) -> impl IntoElement {
    div().flex().items_center().gap_1().children(
        binding
            .split_whitespace()
            .filter_map(|k| Keystroke::parse(k).ok())
            .map(move |k| Kbd::new(k).outline().text_color(t.fg2).bg(t.bg3)),
    )
}
