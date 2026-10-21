//! The contact sidebar: who this person is, our history together, and the actions on it.
//! Everything on the card comes from `Mail::contact`, which reads only the local store,
//! so showing it never touches the network and never costs a keystroke's frame budget.
use crate::app::Kite;
use crate::list::when;
use crate::theme::{self, RADIUS_SM, Theme};
use chrono::Local;
use gpui_kit::assets::IconName;
use gpui_kit::component::{Icon, Sizable};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kite_core::model::{Addr, ContactCard};
use std::sync::Arc;

/// The panel's width.
pub const CONTACT_W: f32 = 260.;
/// The reading pane keeps at least this much before the card steps aside on its own.
const MIN_PANE: f32 = 400.;

impl Kite {
    /// The person the card is about: the sender of the latest message not from me, else
    /// the first To recipient when the conversation is all mine.
    pub(crate) fn correspondent(&self) -> Option<Addr> {
        let open = self.open.as_ref()?;
        let mine = |a: &Addr| {
            a.email.is_empty()
                || self
                    .accounts
                    .iter()
                    .any(|me| me.email.eq_ignore_ascii_case(&a.email))
        };
        if let Some(addr) = open
            .messages
            .iter()
            .rev()
            .map(|m| &m.from)
            .find(|a| !mine(a))
        {
            return Some(addr.clone());
        }
        open.messages
            .iter()
            .rev()
            .flat_map(|m| m.to.iter())
            .find(|a| !mine(a))
            .cloned()
    }

    /// Whether the card is on screen this frame: the setting, an open conversation and a
    /// window wide enough that the reading pane still gets `MIN_PANE` beside it.
    pub(crate) fn contact_shown(&self, window: &Window) -> bool {
        self.settings.contact_panel
            && self.preferences.is_none()
            && self.open.is_some()
            && window.viewport_size().width.as_f32() - crate::sidebar::width(window)
                >= MIN_PANE + CONTACT_W
    }

    /// Loads the card when the open conversation or its correspondent changed, and again
    /// after a store change while the key is the same (`force`). The query runs on the
    /// background executor, so `j`/`k` never waits on SQLite.
    pub(crate) fn refresh_contact(&mut self, force: bool, cx: &mut Context<Self>) {
        let key = self.open.as_ref().map(|open| open.id).zip(
            self.settings
                .contact_panel
                .then(|| self.correspondent())
                .flatten()
                .map(|addr| addr.email.to_ascii_lowercase()),
        );
        if !force && key == self.contact_for {
            return;
        }
        let Some((thread, email)) = key else {
            self.contact_for = None;
            if self.contact.take().is_some() {
                cx.notify();
            }
            return;
        };
        self.contact_for = Some((thread, email.clone()));
        let mail = self.mail.clone();
        let want = email.clone();
        let task = cx
            .background_executor()
            .spawn(async move { mail.contact(&want, Some(thread)) });
        cx.spawn(async move |this, cx| {
            let card = task.await.ok().flatten();
            this.update(cx, |this, cx| {
                if this
                    .contact_for
                    .as_ref()
                    .is_some_and(|(t, e)| (*t, e.as_str()) == (thread, email.as_str()))
                {
                    this.contact = card.map(Arc::new);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// The right-hand panel: identity, actions, history, recent conversations, shared files.
    /// The header paints from the open conversation at once; the store-backed card lands
    /// a frame later.
    pub(crate) fn contact_panel(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.contact_shown(window) {
            return None;
        }
        let t = self.theme;
        let who = self.correspondent()?;
        // A card loading for the previous correspondent never shows for the new one.
        let card = self
            .contact
            .clone()
            .filter(|card| card.email.eq_ignore_ascii_case(&who.email));
        let name = card
            .as_ref()
            .map(|card| card.name.clone())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| {
                if who.name.is_empty() {
                    who.email.clone()
                } else {
                    who.name.clone()
                }
            });
        let email = who.email.clone();
        let domain = card
            .as_ref()
            .map(|card| card.domain.clone())
            .unwrap_or_else(|| email.rsplit('@').next().unwrap_or_default().to_string());
        let first = name.split_whitespace().next().unwrap_or(&name).to_string();
        let mut panel = div()
            .id("contact-panel")
            .test_support()
            .aria_label(format!("About {name}"))
            .w(px(CONTACT_W))
            .flex_shrink_0()
            .h_full()
            .flex()
            .flex_col()
            .px_4()
            .py_5()
            .gap_4()
            .overflow_hidden()
            .bg(theme::panel(&t))
            .border_l_1()
            .border_color(t.line)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_1()
                    .child(crate::thread::avatar(&name, usize::MAX, 44.))
                    .child(
                        div()
                            .id("contact-name")
                            .test_support()
                            .aria_label(name.clone())
                            .mt_1()
                            .max_w_full()
                            .truncate()
                            .text_size(t.px(15.))
                            .font_weight(FontWeight(650.))
                            .child(name.clone()),
                    )
                    .child(
                        div()
                            .id("contact-email")
                            .test_support()
                            .aria_label(format!("{email}, click to copy"))
                            .max_w_full()
                            .truncate()
                            .px_1()
                            .rounded(px(RADIUS_SM))
                            .cursor_pointer()
                            .text_size(t.px(12.))
                            .text_color(t.fg2)
                            .hover(|s| s.bg(t.bg3).text_color(t.fg))
                            .child(email.clone())
                            .on_click(cx.listener({
                                let email = email.clone();
                                move |this, _, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(email.clone()));
                                    this.toast("Copied email address", cx);
                                }
                            })),
                    )
                    .child(
                        div()
                            .id("contact-domain")
                            .test_support()
                            .aria_label(domain.clone())
                            .text_size(t.px(12.))
                            .text_color(t.fg3)
                            .child(domain),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(action_row(
                        &t,
                        0,
                        IconName::SquarePen,
                        format!("Email {first}"),
                        cx.listener(move |this, _, window, cx| {
                            this.compose_address(who.clone(), window, cx)
                        }),
                    ))
                    .child(action_row(
                        &t,
                        1,
                        IconName::Search,
                        "Search all mail from them".to_string(),
                        cx.listener({
                            let email = email.clone();
                            move |this, _, window, cx| {
                                let query = format!("from:{email}");
                                this.search.update(cx, |state, cx| {
                                    state.set_value(query.clone(), window, cx)
                                });
                                this.set_query(query, cx);
                            }
                        }),
                    ))
                    .child(action_row(
                        &t,
                        2,
                        IconName::Copy,
                        "Copy email".to_string(),
                        cx.listener({
                            let email = email.clone();
                            move |this, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(email.clone()));
                                this.toast("Copied email address", cx);
                            }
                        }),
                    )),
            );
        if let Some(card) = &card {
            panel = panel
                .child(stats(&t, card))
                .child(recent(&t, card, cx))
                .child(files(&t, card, cx));
        }
        Some(panel.into_any_element())
    }
}

/// A quiet action row: icon plus words, like the list header's verbs.
fn action_row(
    t: &Theme,
    ix: usize,
    icon: IconName,
    label: String,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let t = *t;
    div()
        .id(("contact-action", ix))
        .test_support()
        .aria_label(label.clone())
        .flex()
        .items_center()
        .gap_2()
        .h(px(28.))
        .px_2()
        .rounded(px(RADIUS_SM))
        .cursor_pointer()
        .text_size(t.px(13.))
        .text_color(t.fg2)
        .hover(move |s| s.bg(t.bg3).text_color(t.fg))
        .on_click(on_click)
        .child(Icon::new(icon).with_size(px(14.)))
        .child(label)
        .into_any_element()
}

fn section(t: &Theme, title: &'static str) -> Div {
    div().text_size(t.px(12.)).text_color(t.fg3).child(title)
}

/// The history numbers: since when, how much, and the reply habits both ways.
fn stats(t: &Theme, card: &Arc<ContactCard>) -> Div {
    let now = Local::now();
    let mut rows = div()
        .flex()
        .flex_col()
        .gap_1()
        .child(section(t, "History"))
        .child(stat_row(t, 0, "First message", when(card.first, now)))
        .child(stat_row(t, 1, "Latest", when(card.latest, now)))
        .child(stat_row(
            t,
            2,
            "Conversations",
            card.conversations.to_string(),
        ));
    if let Some(median) = card.reply_median {
        rows = rows.child(stat_row(
            t,
            3,
            "You reply in",
            format!("~{}", human_ms(median)),
        ));
    }
    if let Some(replies) = card.replies_back {
        rows = rows.child(stat_row(
            t,
            4,
            "They reply",
            if replies { "usually" } else { "rarely" }.to_string(),
        ));
    }
    rows
}

fn stat_row(t: &Theme, ix: usize, label: &'static str, value: String) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap_2()
        .px_2()
        .text_size(t.px(12.))
        .child(div().text_color(t.fg3).child(label))
        .child(
            div()
                .id(("contact-stat", ix))
                .test_support()
                .aria_label(value.clone())
                .min_w_0()
                .truncate()
                .text_color(t.fg)
                .child(value),
        )
}

/// Up to five of their other conversations: subject and date, opening on click.
fn recent(t: &Theme, card: &Arc<ContactCard>, cx: &mut Context<Kite>) -> Div {
    let now = Local::now();
    let mut rows = div()
        .flex()
        .flex_col()
        .gap_1()
        .child(section(t, "Recent conversations"));
    if card.recent.is_empty() {
        rows = rows.child(
            div()
                .px_2()
                .text_size(t.px(12.))
                .text_color(t.fg3)
                .child("This is the only one"),
        );
    }
    for (ix, thread) in card.recent.iter().enumerate() {
        let id = thread.id;
        rows = rows.child(
            div()
                .id(("contact-thread", ix))
                .test_support()
                .aria_label(thread.subject.clone())
                .flex()
                .items_center()
                .gap_2()
                .h(px(26.))
                .px_2()
                .rounded(px(RADIUS_SM))
                .cursor_pointer()
                .hover(move |s| s.bg(t.bg3))
                .on_click(cx.listener(move |this, _, _, cx| this.open_any_thread(id, cx)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(t.px(13.))
                        .child(thread.subject.clone()),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .text_size(t.px(12.))
                        .text_color(t.fg3)
                        .child(when(thread.date, now)),
                ),
        );
    }
    rows
}

/// Up to five recent attachments exchanged with them; one opens its conversation.
fn files(t: &Theme, card: &Arc<ContactCard>, cx: &mut Context<Kite>) -> Div {
    if card.files.is_empty() {
        return div();
    }
    let now = Local::now();
    let mut rows = div()
        .flex()
        .flex_col()
        .gap_1()
        .child(section(t, "Shared files"));
    for (ix, file) in card.files.iter().enumerate() {
        let thread = file.thread;
        let meta = format!(
            "{} · {}",
            crate::thread::format_size(file.size),
            when(file.date, now)
        );
        rows = rows.child(
            div()
                .id(("contact-file", ix))
                .test_support()
                .aria_label(format!("{}, {}", file.name, meta))
                .flex()
                .items_center()
                .gap_2()
                .h(px(26.))
                .px_2()
                .rounded(px(RADIUS_SM))
                .cursor_pointer()
                .hover(move |s| s.bg(t.bg3))
                .on_click(cx.listener(move |this, _, _, cx| this.open_any_thread(thread, cx)))
                .child(
                    Icon::new(IconName::Paperclip)
                        .with_size(px(13.))
                        .text_color(t.fg3),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(t.px(13.))
                        .child(file.name.clone()),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .text_size(t.px(12.))
                        .text_color(t.fg3)
                        .child(meta),
                ),
        );
    }
    rows
}

/// `42 min`, `~5 h`→`5 h`, `3 days`: the reply-time stat at a glance.
fn human_ms(ms: i64) -> String {
    let mins = ms.max(0) / 60_000;
    if mins < 60 {
        format!("{} min", mins.max(1))
    } else if mins < 60 * 48 {
        format!("{} h", (mins + 30) / 60)
    } else {
        format!("{} days", (mins + 720) / 1440)
    }
}
