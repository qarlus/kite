//! The virtualised thread list, the empty states and the shortcut cheat sheet.
use crate::app::Kite;
use crate::keys::{self, Cmd};
use crate::theme::{ROW_H, Theme};
use chrono::{DateTime, Datelike, Local, TimeZone as _, Timelike as _};
use gpui_kit::assets::IconName;
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::{Icon, Sizable};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kite_core::model::{Mailbox, Ms, ThreadSummary};
use std::ops::Range;
use std::time::Duration;

/// `9:41` today, `Mon` this week, `Oct 3` this year, `3/10/25` older.
pub fn when(ms: Ms, now: DateTime<Local>) -> String {
    let Some(at) = Local.timestamp_millis_opt(ms).single() else {
        return String::new();
    };
    let days = (now.date_naive() - at.date_naive()).num_days();
    if days <= 0 {
        format!("{}:{:02}", at.time().hour(), at.time().minute())
    } else if days < 7 {
        at.format("%a").to_string()
    } else if at.year() == now.year() {
        format!("{} {}", at.format("%b"), at.day())
    } else {
        format!("{}/{}/{}", at.month(), at.day(), at.year() % 100)
    }
}

impl Kite {
    pub fn render_list(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if self.rows.is_empty() {
            return self.render_empty();
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
                    let mut items = Vec::with_capacity(range.len());
                    for ix in range {
                        let Some(row) = this.rows.get(ix) else {
                            continue;
                        };
                        let sender_width = if window.viewport_size().width < px(900.) {
                            140.
                        } else {
                            180.
                        };
                        items.push(row_element(this, row, ix, sender_width, cx));
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
            .px_5()
            .pb_4()
            .child(list)
            .into_any_element()
    }

    fn render_empty(&self) -> AnyElement {
        let t = self.theme;
        let (title, note) = if self.view.query.is_some() {
            ("No results", "Try another name, subject, or search term.")
        } else if self.accounts.is_empty() {
            (
                "Welcome to Kite",
                "Your email deserves a little breathing room.",
            )
        } else {
            match self.view.mailbox {
                Mailbox::Inbox => ("Inbox Zero", "You're all caught up. Enjoy the quiet."),
                Mailbox::Starred => ("No starred mail", "Star a conversation to find it here."),
                Mailbox::Snoozed => (
                    "Nothing snoozed",
                    "Conversations you snooze will wait here.",
                ),
                Mailbox::Drafts => ("No drafts", "Your unfinished messages will appear here."),
                Mailbox::Sent => ("No sent mail", "Messages you send will appear here."),
                _ => ("No conversations", "There's no mail in this view."),
            }
        };
        div()
            .id("inbox-zero")
            .test_support()
            .aria_label(title)
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_3()
            .bg(linear_gradient(
                135.,
                linear_color_stop(Theme::alpha(t.accent, 0.12), 0.),
                linear_color_stop(Theme::alpha(t.accent2, 0.12), 1.),
            ))
            .child(
                div()
                    .text_size(px(28.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title),
            )
            .child(div().text_size(px(14.)).text_color(t.fg2).child(note))
            .when(self.accounts.is_empty(), |d| {
                d.child(
                    div()
                        .mt_4()
                        .text_size(px(12.))
                        .text_color(t.fg2)
                        .child("Account connection is not available in this build."),
                )
            })
            .into_any_element()
    }

    /// The `?` cheat sheet, generated from the same command table as the keymap.
    pub fn render_help(&mut self, _cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme;
        let half = keys::COMMANDS.len().div_ceil(2);
        let column = |cmds: &[Cmd]| {
            div()
                .flex()
                .flex_col()
                .flex_1()
                .children(cmds.iter().map(|c| {
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_4()
                        .h(px(24.))
                        .child(div().text_size(px(13.)).child(c.title))
                        .child(keys_pills(t, c.keys[0]))
                }))
        };
        overlay(
            t,
            px(0.),
            true,
            div()
                .id("help")
                .test_support()
                .aria_label("Keyboard shortcuts")
                .w(px(620.))
                .rounded(px(12.))
                .bg(t.bg2)
                .border_1()
                .border_color(t.line)
                .p_5()
                .flex()
                .flex_col()
                .gap_4()
                .child(
                    div()
                        .text_size(px(15.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Keyboard shortcuts"),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap_8()
                        .child(column(&keys::COMMANDS[..half]))
                        .child(column(&keys::COMMANDS[half..])),
                ),
        )
        .into_any_element()
    }
}

/// One list row. Borrows its summary, so painting a frame copies no threads.
fn row_element(
    kite: &Kite,
    row: &ThreadSummary,
    ix: usize,
    sender_width: f32,
    cx: &mut Context<Kite>,
) -> AnyElement {
    let t = kite.theme;
    let selected = ix == kite.sel;
    let checked = kite.multi.contains(&row.id);
    let label = format!("{}: {}", row.participants, row.subject);
    let now = Local::now();
    div()
        .id(("row", ix))
        .test_support()
        .aria_label(label)
        .aria_selected(selected)
        .group("row")
        .relative()
        .flex()
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .w_full()
        .h(px(ROW_H))
        .rounded(px(6.))
        .px_3()
        .gap_3()
        .cursor_pointer()
        .when(selected, |d| d.bg(t.sel))
        .when(!selected, |d| d.hover(|s| s.bg(t.bg3)))
        .when(selected, |d| {
            d.child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .w(px(2.))
                    .h_full()
                    .bg(t.accent),
            )
        })
        .child(
            div()
                .w(px(8.))
                .flex()
                .justify_center()
                .when(row.unread || checked, |d| {
                    d.child(div().size(px(6.)).rounded_full().bg(t.accent))
                }),
        )
        .child(
            div()
                .w(px(sender_width))
                .flex_shrink_0()
                .truncate()
                .text_size(px(13.))
                // DESIGN: unread senders are heavier (650 rather than 500).
                .font_weight(if row.unread {
                    FontWeight(650.)
                } else {
                    FontWeight(500.)
                })
                .text_color(if row.unread { t.fg } else { t.fg2 })
                .child(row.participants.clone()),
        )
        .child(
            div()
                .flex_1()
                .flex()
                .items_center()
                .gap_2()
                .min_w_0()
                .overflow_hidden()
                .child(
                    div()
                        .max_w(px(360.))
                        .min_w_0()
                        .truncate()
                        .font_weight(if row.unread {
                            FontWeight::MEDIUM
                        } else {
                            FontWeight::NORMAL
                        })
                        .text_size(px(13.))
                        .text_color(t.fg)
                        .child(row.subject.clone()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(px(13.))
                        .text_color(t.fg2)
                        .child(row.snippet.clone()),
                ),
        )
        .when(row.starred, |d| {
            d.child(
                Icon::new(IconName::Star)
                    .with_size(px(13.))
                    .text_color(t.accent),
            )
        })
        .when(!row.labels.is_empty(), |d| {
            d.child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .max_w(px(160.))
                    .overflow_hidden()
                    .flex_shrink_0()
                    .children(row.labels.iter().take(2).map(|label| {
                        div()
                            .max_w(px(120.))
                            .min_w_0()
                            .truncate()
                            .px_2()
                            .py_0p5()
                            .rounded(px(6.))
                            .bg(t.bg3)
                            .text_size(px(12.))
                            .text_color(t.fg2)
                            .child(label.clone())
                    })),
            )
        })
        .child(
            div()
                .id(("row-time", ix))
                .test_support()
                .aria_label(when(row.date, now))
                .relative()
                .w(px(64.))
                .flex_shrink_0()
                .flex()
                .justify_end()
                .text_size(px(12.))
                .text_color(t.fg3)
                .child(
                    div()
                        .group_hover("row", |s| s.opacity(0.))
                        .child(when(row.date, now)),
                )
                .child(
                    div()
                        .absolute()
                        .right_0()
                        .opacity(0.)
                        .group_hover("row", |s| s.opacity(1.))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1()
                                .child(keys_pills(t, "e"))
                                .child(keys_pills(t, "h")),
                        ),
                ),
        )
        .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
            this.sel = ix;
            window.focus(&this.focus, cx);
            if event.click_count() > 1 {
                this.open_thread(cx);
            } else {
                this.list_scroll.scroll_to_item(ix, ScrollStrategy::Nearest);
                cx.notify();
            }
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
    // DESIGN › Motion: overlays fade in over 100 ms (skipped under reduced motion).
    let panel = div()
        .with_animation(
            ("overlay-panel", 0usize),
            Animation::new(Duration::from_millis(100)).with_easing(ease_out_quint()),
            |panel, delta| panel.opacity(delta),
        )
        .child(panel);
    div()
        .id("overlay")
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

/// One `<kbd>` pill per keystroke in a (possibly multi-stroke) binding.
pub fn keys_pills(t: &Theme, binding: &str) -> impl IntoElement {
    div().flex().items_center().gap_1().children(
        binding
            .split_whitespace()
            .filter_map(|k| Keystroke::parse(k).ok())
            .map(move |k| Kbd::new(k).outline().text_color(t.fg2).bg(t.bg3)),
    )
}
