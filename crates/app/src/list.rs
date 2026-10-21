//! The virtualised thread list, the empty states and the shortcut cheat sheet.
use crate::app::Kite;
use crate::keys::{self, Cmd, Group};
use crate::theme::{RADIUS, RADIUS_SM, ROW_H, Theme};
use chrono::{DateTime, Datelike, Local, TimeZone as _, Timelike as _};
use gpui_kit::assets::IconName;
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::menu::ContextMenuExt;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Icon, Sizable};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kite_core::model::{Action as MailAction, Id, Mailbox, Ms, Provider, ThreadSummary};
use std::ops::Range;
use std::time::Duration;

/// The room the hover actions take at a row's right edge: four 24px buttons, their gaps and
/// padding, plus a margin. Subject and snippet stop before it, so the buttons never cover a word.
const ACTION_SLOT: f32 = 124.;

/// How much of its subject line a row's tags may take. The list is virtualised and `j`/`k` must fit
/// in 4 ms, so how many chips fit is estimated from the tag names rather than shaped, which would
/// cost a text layout per tag on every repaint. `TAG_INSET` is what the row spends around the
/// subject: the card's padding, the multi-select box and its gap, and the subject's own minimum.
const TAG_INSET: f32 = 126.;
/// An 11px UI glyph's average advance, and what a chip spends on its padding and gaps.
const TAG_CHAR: f32 = 5.6;
const TAG_PAD: f32 = 12.;
const TAG_GAP: f32 = 4.;
/// The longest a chip gets; a longer name is ellipsised instead of pushing the subject out.
const TAG_MAX: f32 = 104.;

/// The mark and brand ink that name a provider in the combined inbox.
fn provider_mark(provider: Provider) -> (&'static str, Rgba) {
    match provider {
        Provider::Proton => ("providers/proton.svg", rgb(0x6d4aff)),
        Provider::Microsoft => ("providers/outlook.svg", rgb(0x0078d4)),
    }
}

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
                      window: &mut Window,
                      cx: &mut Context<Kite>| {
                    if !this.loaded_all && range.end + 50 >= this.rows.len() {
                        this.load_more(cx);
                    }
                    let now = Local::now();
                    // Tags ride the subject's line, so they may take what the column has left once
                    // the multi-select box, the hover actions and the subject have their room.
                    let column = if this.settings.reading_pane {
                        crate::sidebar::list_width(window)
                    } else {
                        window.viewport_size().width.as_f32() - crate::sidebar::RAIL_W
                    };
                    let tags = (column - ACTION_SLOT - TAG_INSET).max(0.);
                    let mut items = Vec::with_capacity(range.len());
                    for ix in range {
                        let Some(row) = this.rows.get(ix) else {
                            continue;
                        };
                        items.push(row_element(this, row, ix, now, tags, cx));
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
                "Sign in to Proton or Outlook to start syncing your mail.".to_string(),
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
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.show_preferences(crate::preferences::OPEN_ACCOUNTS, window, cx)
                        })),
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
                // The shortcut list outgrows short windows: it scrolls, the title and Close stay.
                .max_h(relative(0.9))
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
                    div()
                        .id("help-columns")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .flex()
                        .flex_row()
                        .items_start()
                        .gap_8()
                        .children(columns.into_iter().map(|column| {
                            div().flex().flex_col().flex_1().gap_4().children(column)
                        })),
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
                                    this.run(
                                        crate::keys::cmd::BACK,
                                        crate::app::Src::Click,
                                        window,
                                        cx,
                                    )
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

/// One list row: the sender, the account's mark (in a mixed inbox) and the time, the subject and
/// a two-line snippet. Borrows its summary, so painting a frame copies no threads.
fn row_element(
    kite: &Kite,
    row: &ThreadSummary,
    ix: usize,
    now: DateTime<Local>,
    tags: f32,
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
    // Only a mixed inbox has to say where a row came from. It says it with the provider's mark,
    // because a full address beside every row is noise; the address is the mark's tooltip.
    let mark = combined.then_some(account).flatten().map(|account| {
        (
            SharedString::from(account.email.as_str()),
            provider_mark(account.provider),
        )
    });
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
                                // The row's right-hand meta fades out on hover, where the
                                // actions take its place; leaving it in place keeps the row still.
                                .when_some(mark, |d, (email, (path, ink))| {
                                    d.child(
                                        div()
                                            .id(("row-account", ix))
                                            .test_support()
                                            .aria_label(email.clone())
                                            .flex()
                                            .items_center()
                                            .flex_shrink_0()
                                            .group_hover("row", |s| s.opacity(0.))
                                            .tooltip(move |window, cx| {
                                                // The address in full, for a row that only has
                                                // room to show the provider's mark.
                                                let address = email.clone();
                                                Tooltip::element(move |_, _| {
                                                    div()
                                                        .id("row-account-tip")
                                                        .test_support()
                                                        .aria_label(address.clone())
                                                        .child(address.clone())
                                                })
                                                .build(window, cx)
                                            })
                                            .child(
                                                svg().path(path).size(t.px(13.)).text_color(ink),
                                            ),
                                    )
                                })
                                .child(
                                    div()
                                        .id(("row-time", ix))
                                        .test_support()
                                        .aria_label(when.clone())
                                        .flex_shrink_0()
                                        .group_hover("row", |s| s.opacity(0.))
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
                                // Text stops before the action slot, so the buttons can never
                                // cover a word.
                                .mr(px(ACTION_SLOT))
                                .child(
                                    div()
                                        .id(("row-subject", ix))
                                        .test_support()
                                        .aria_label(row.subject.clone())
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_size(t.px(13.))
                                        .text_color(t.fg)
                                        .child(row.subject.clone()),
                                )
                                .when(!row.labels.is_empty(), |d| {
                                    d.child(row_tags(&t, ix, &row.labels, tags))
                                })
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
                                .id(("row-snippet", ix))
                                .test_support()
                                .aria_label(row.snippet.clone())
                                .mr(px(ACTION_SLOT))
                                .line_clamp(2)
                                .text_ellipsis()
                                .text_size(t.px(12.))
                                .line_height(relative(1.45))
                                .text_color(t.fg2)
                                .child(row.snippet.clone()),
                        ),
                ),
        )
        // DESIGN › Layout: hovering a row replaces the meta with the actions a click-first
        // client expects, so every triage action is reachable with the mouse. The strip sits in
        // the slot the text leaves free, so it never hides what is being read.
        .child(
            div()
                .id(("row-actions", ix))
                .test_support()
                .aria_label("Row actions")
                .absolute()
                .right(px(12.))
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
        // A right-click picks the row unless it is part of the multi-selection, which the
        // menu then acts on — the way desktop clients treat a secondary click.
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, _, window, cx| {
                if !this.multi.contains(&row_id) {
                    this.multi.clear();
                    this.sel = ix;
                }
                window.focus(&this.focus, cx);
                cx.notify();
            }),
        )
        .context_menu({
            let entity = cx.entity().downgrade();
            move |menu, window, cx| {
                let ids = entity
                    .upgrade()
                    .map(|kite| {
                        let state = kite.read(cx);
                        if state.multi.contains(&row_id) {
                            state.multi.clone()
                        } else {
                            vec![row_id]
                        }
                    })
                    .unwrap_or_else(|| vec![row_id]);
                crate::tags::popup_with(entity.clone(), ids, menu, window, cx)
            }
        })
        .into_any_element()
}

/// Every tag a row can show, then a `+N` for the rest. DESIGN › Layout fixes the row at 88px, so
/// the chips ride the subject's line instead of adding one.
fn row_tags(t: &Theme, ix: usize, labels: &[String], slot: f32) -> AnyElement {
    let (shown, hidden) = visible_tags(labels, slot);
    div()
        .flex()
        .items_center()
        .gap_1()
        .flex_shrink_0()
        .children(
            labels
                .iter()
                .take(shown)
                .enumerate()
                .map(|(chip, label)| row_tag(t, ix, chip, label)),
        )
        .when(hidden > 0, |d| {
            let more = format!("+{hidden}");
            d.child(
                tag_pill(t)
                    .id(("row-tag-more", ix))
                    .test_support()
                    .aria_label(more.clone())
                    .child(more),
            )
        })
        .into_any_element()
}

/// A row tag chip's identity: `("row-tag", row, chip)`. `ElementId` names its third part, so a
/// fixed digit table keeps the id allocation-free on every repaint.
pub fn row_tag_id(ix: usize, chip: usize) -> ElementId {
    const DIGITS: [&str; 10] = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"];
    (
        ElementId::from(("row-tag", ix)),
        DIGITS[chip.min(DIGITS.len() - 1)],
    )
        .into()
}

/// One tag of a conversation, named so a test can count them.
fn row_tag(t: &Theme, ix: usize, chip: usize, label: &str) -> AnyElement {
    let private = label.eq_ignore_ascii_case(kite_core::model::PRIVATE_TAG);
    tag_pill(t)
        .id(row_tag_id(ix, chip))
        .test_support()
        .aria_label(format!("Tag: {label}"))
        .when(private, |d| {
            d.child(
                Icon::new(IconName::Lock)
                    .with_size(px(10.))
                    .text_color(t.fg2),
            )
        })
        .child(label.to_string())
        .into_any_element()
}

/// How many of a row's tags fit on its subject line, and how many are left to a `+N` chip.
fn visible_tags(labels: &[String], slot: f32) -> (usize, usize) {
    let mut used = 0.;
    for (ix, label) in labels.iter().enumerate() {
        let rest = labels.len() - ix - 1;
        let width = tag_width(label) + if ix == 0 { 0. } else { TAG_GAP };
        // Whatever this chip pushes out needs the `+N` chip's own room.
        let overflow = match rest {
            0 => 0.,
            _ => TAG_GAP + TAG_PAD + TAG_CHAR * (1. + digits(rest) as f32),
        };
        if used + width + overflow > slot {
            // One tag still beats a bare `+N`, even where the first chip has to clip.
            let shown = ix.max(1);
            return (shown, labels.len() - shown);
        }
        used += width;
    }
    (labels.len(), 0)
}

fn tag_width(label: &str) -> f32 {
    let lock = if label.eq_ignore_ascii_case(kite_core::model::PRIVATE_TAG) {
        TAG_CHAR * 2.
    } else {
        0.
    };
    (label.chars().count() as f32 * TAG_CHAR + TAG_PAD + lock).min(TAG_MAX)
}

fn digits(mut n: usize) -> usize {
    let mut count = 1;
    while n > 9 {
        n /= 10;
        count += 1;
    }
    count
}

/// The look every row chip shares: 11px on `bg3`, as high as the subject's line rather than taller.
fn tag_pill(t: &Theme) -> Div {
    let t = *t;
    div()
        .h(t.px(16.))
        .max_w(t.px(TAG_MAX))
        .min_w_0()
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap_1()
        .truncate()
        .px_1p5()
        .rounded(px(RADIUS_SM))
        .bg(t.bg3)
        .text_size(t.px(11.))
        .text_color(t.fg2)
}

/// One button in a row's hover action strip.
struct RowAction {
    name: &'static str,
    icon: IconName,
    label: &'static str,
    action: MailAction,
    /// The command the click pretends to be, so it can teach its key.
    hint: u8,
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
            hint: keys::cmd::DONE,
            remove: true,
            text: "Marked done",
        },
        RowAction {
            name: "row-trash",
            icon: IconName::Trash,
            label: "Trash",
            action: MailAction::Trash,
            hint: keys::cmd::TRASH,
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
            hint: keys::cmd::STAR,
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
            hint: keys::cmd::TOGGLE_READ,
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
        hint,
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
            this.shortcut_hint(hint, cx);
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
