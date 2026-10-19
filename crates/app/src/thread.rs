//! The conversation view: a title, then one flat list of messages under a hairline. Older
//! messages collapse to a one-line header; the latest expands. HTML bodies are rendered by Blitz
//! on the background executor and cached per (message, width, theme, scale). Text bodies render
//! as GPUI text.
use crate::app::{Kite, Lru};
use crate::html::{self, Rendered};
use crate::keys;
use crate::list::when;
use crate::theme::{MAX_COL, RADIUS_SM, Theme};
use chrono::{DateTime, Local, TimeZone as _};
use gpui_kit::assets::IconName;
use gpui_kit::component::{Icon, Sizable};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kite_core::model::{Addr, Id, Message};
use std::sync::Arc;

/// The reading pane's side padding. `app.rs` subtracts it from `html_width`, so the width Blitz
/// renders at is the width the body is shown at.
pub const PANE_PAD: f32 = 32.;

/// Past the avatar column, so everything a message holds lines up under its sender's name.
pub const BODY_INDENT: f32 = 44.;

/// Cache key: a body is re-rendered when the message, width, theme or DPI scale changes.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct HtmlKey {
    pub id: Id,
    width: u32,
    scale: u32,
    dark: bool,
    appearance: u64,
}

impl HtmlKey {
    pub fn new(id: Id, width: f32, scale: f32, dark: bool, appearance: u64) -> Self {
        Self {
            id,
            // Quantised so a drag-resize does not miss the cache every frame.
            width: (width * 4.).round() as u32,
            scale: (scale * 20.).round() as u32,
            dark,
            appearance,
        }
    }

    fn at(&self) -> (f32, f32) {
        (self.width as f32 / 4., self.scale as f32 / 20.)
    }
}

pub struct HtmlCache {
    inner: Lru<HtmlKey, Rendered>,
    pending: std::collections::HashSet<HtmlKey>,
}

impl HtmlCache {
    pub fn new(cap: usize) -> Self {
        Self {
            inner: Lru::new(cap),
            pending: std::collections::HashSet::new(),
        }
    }

    pub fn get(&self, key: &HtmlKey) -> Option<&Rendered> {
        self.inner.get(key)
    }

    pub fn clear(&mut self) {
        self.inner.clear();
        self.pending.clear();
    }

    fn start(&mut self, key: HtmlKey) -> bool {
        self.pending.insert(key)
    }

    fn finish(&mut self, key: HtmlKey, rendered: Rendered) {
        self.pending.remove(&key);
        self.inner.insert(key, rendered);
    }
}

impl Kite {
    /// The Blitz render width: the reading pane's column, less the indent a body hangs under.
    pub(crate) fn body_width(&self) -> f32 {
        (self.html_width - BODY_INDENT).max(120.)
    }

    pub fn render_thread(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme;
        let scale = window.scale_factor();
        self.html_scale = scale;
        let body_width = self.body_width();
        let Some(open) = &self.open else {
            return div().into_any_element();
        };
        let subject = open.subject.clone();
        let account = self
            .accounts
            .iter()
            .find(|account| account.id == open.account_id)
            .map(|account| account.email.clone())
            .unwrap_or_default();
        let messages = open.messages.clone();
        let expanded = open.expanded;
        let count = messages.len();
        let mut column = div()
            .flex()
            .flex_col()
            .w_full()
            .max_w(px(MAX_COL))
            .px(px(PANE_PAD))
            .pt_6()
            .pb_10()
            .child(
                div()
                    .id("thread-subject")
                    .test_support()
                    .aria_label(subject.clone())
                    .text_size(t.px(24.))
                    .font_weight(FontWeight(500.))
                    .child(subject),
            )
            .child(
                div()
                    .id("thread-account")
                    .test_support()
                    .aria_label(account.clone())
                    .mt_1()
                    .text_size(t.px(12.))
                    .text_color(t.fg2)
                    .child(format!(
                        "{count} {} · {account}",
                        if count == 1 { "message" } else { "messages" }
                    )),
            )
            .child(
                div()
                    .mt_6()
                    .flex()
                    .flex_col()
                    .border_t_1()
                    .border_color(t.line),
            );
        for (ix, message) in messages.into_iter().enumerate() {
            let is_last = ix + 1 == count;
            column =
                column.child(self.message(message, ix, expanded, is_last, (scale, body_width), cx));
        }
        let scroll = self.thread_scroll.clone();
        div()
            .flex_1()
            .flex()
            .flex_col()
            .min_h_0()
            .overflow_hidden()
            .child(self.thread_toolbar(cx))
            .child(
                div()
                    .id("thread-scroll")
                    .test_support()
                    .flex_1()
                    .overflow_y_scroll()
                    .track_scroll(&scroll)
                    .flex()
                    .justify_center()
                    .child(column),
            )
            .into_any_element()
    }

    /// The reading-pane toolbar: icon-only buttons right-aligned in what they do to the
    /// conversation, with the list/reading-pane toggle first. Each label carries the key that is
    /// bound right now, so a click-first reader loses nothing.
    fn thread_toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme;
        const ACTIONS: [(u8, IconName, &str); 6] = [
            (keys::cmd::DONE, IconName::Archive, "Mark done"),
            (keys::cmd::TRASH, IconName::Trash, "Trash"),
            (keys::cmd::REPLY, IconName::Reply, "Reply"),
            (keys::cmd::REPLY_ALL, IconName::ReplyAll, "Reply all"),
            (keys::cmd::FORWARD, IconName::Forward, "Forward"),
            (keys::cmd::STAR, IconName::Star, "Star"),
        ];
        // File it away, answer it, mark it: three groups, so the eye can find a verb.
        let mut bar = div()
            .id("thread-toolbar")
            .test_support()
            .aria_label("Thread actions")
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_4()
            .px_3()
            .h(px(48.))
            .border_b_1()
            .border_color(t.line)
            .child(self.pane_toggle(cx))
            .child(div().flex_1());
        for group in [[0, 2], [2, 5], [5, 6]] {
            bar = bar.child(
                div().flex().items_center().gap_1().children(
                    ACTIONS[group[0]..group[1]]
                        .iter()
                        .map(|(cmd, icon, label)| {
                            let hint = keys::hint_for(&self.settings, *cmd);
                            let label = if hint.is_empty() {
                                label.to_string()
                            } else {
                                format!("{label} ({hint})")
                            };
                            self.action_icon(*cmd, *icon, label, cx)
                        }),
                ),
            );
        }
        bar.into_any_element()
    }

    /// Side by side, or one at a time: `settings.reading_pane`, saved like any preference.
    fn pane_toggle(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme;
        let shown = self.settings.reading_pane;
        div()
            .id("toggle-reading-pane")
            .test_support()
            .aria_label("Toggle list and reading pane")
            .aria_selected(shown)
            .flex()
            .items_center()
            .justify_center()
            .size(px(30.))
            .rounded(px(RADIUS_SM))
            .cursor_pointer()
            .hover(|s| s.bg(t.bg3))
            .child(
                Icon::new(IconName::PanelRight)
                    .with_size(px(16.))
                    .text_color(if shown { t.fg } else { t.fg2 }),
            )
            .on_click(cx.listener(|this, _, window, cx| {
                let mut settings = this.settings.clone();
                settings.reading_pane = !settings.reading_pane;
                this.save_preferences(settings, window, cx);
            }))
            .into_any_element()
    }

    fn message(
        &mut self,
        message: Message,
        ix: usize,
        expanded: usize,
        is_last: bool,
        layout: (f32, f32),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (scale, body_width) = layout;
        let t = self.theme;
        let open = expanded == ix;
        let sender = if message.from.name.is_empty() {
            message.from.email.clone()
        } else {
            message.from.name.clone()
        };
        let at = Local.timestamp_millis_opt(message.date).single();
        let stamp = when(message.date, Local::now());
        let exact = at
            .map(|at: DateTime<Local>| at.format("%a, %-d %b %Y · %H:%M %:z").to_string())
            .unwrap_or_default();
        let mut header = div()
            .flex()
            .items_start()
            .gap_3()
            .flex_shrink_0()
            .child(avatar(&sender, ix, if open { 32. } else { 24. }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .id(format!("message-from-{ix}"))
                            .test_support()
                            .aria_label(format!("From: {}", address(&message.from)))
                            .flex()
                            .items_baseline()
                            .gap_2()
                            .min_w_0()
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_size(t.px(14.))
                                    .font_weight(FontWeight(650.))
                                    .child(sender),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(t.px(12.))
                                    .text_color(t.fg2)
                                    .child(message.from.email.clone()),
                            )
                            .child(
                                div()
                                    .id(format!("message-date-{ix}"))
                                    .test_support()
                                    .aria_label(exact)
                                    .flex_shrink_0()
                                    .text_size(t.px(12.))
                                    .text_color(t.fg2)
                                    .child(stamp),
                            ),
                    )
                    .when(open, |d| {
                        d.child(
                            div()
                                .id(format!("message-to-{ix}"))
                                .test_support()
                                .aria_label(format!("To: {}", addresses(&message.to)))
                                .truncate()
                                .text_size(t.px(12.))
                                .text_color(t.fg2)
                                .child(format!("To {}", names(&message.to))),
                        )
                        .when(!message.cc.is_empty(), |d| {
                            d.child(
                                div()
                                    .id(format!("message-cc-{ix}"))
                                    .test_support()
                                    .aria_label(format!("Cc: {}", addresses(&message.cc)))
                                    .truncate()
                                    .text_size(t.px(12.))
                                    .text_color(t.fg2)
                                    .child(format!("Cc {}", names(&message.cc))),
                            )
                        })
                    }),
            );
        if !open {
            header = header.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(t.px(12.))
                    .text_color(t.fg2)
                    .child(snippet(&message)),
            );
        }
        let card = div()
            .id(format!("msg-{ix}"))
            .test_support()
            .aria_label(format!("{}: {}", message.from.name, message.subject))
            .flex()
            .flex_col()
            .when(ix > 0, |d| d.border_t_1().border_color(t.line))
            .when(open, |d| d.pt_5().pb_6())
            .when(!open, |d| {
                d.py_2()
                    .px_2()
                    .rounded(px(RADIUS_SM))
                    .cursor_pointer()
                    .hover(|s| s.bg(t.bg3))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(open) = &mut this.open {
                            open.expanded = ix;
                        }
                        cx.notify();
                    }))
            })
            .child(header);
        if !open {
            return card.into_any_element();
        }
        let body = match &message.html {
            Some(source) => self.html_body(&message, source, scale, body_width, cx),
            None => div()
                .id(format!("text-{}", message.id))
                .test_support()
                .aria_label(message.text.clone())
                .max_w(px(body_width))
                .text_size(t.px(15.))
                .line_height(relative(1.7))
                .child(message.text.clone())
                .into_any_element(),
        };
        // Everything the message holds starts where its sender's name does.
        let mut column = div()
            .pt_5()
            .pl(px(BODY_INDENT))
            .flex()
            .flex_col()
            .gap_4()
            .child(body);
        if !message.attachments.is_empty() {
            column = column.child(div().flex().flex_wrap().gap_2().children(
                message.attachments.iter().map(|attachment| {
                    let id = attachment.id;
                    div()
                        .id(format!("attachment-{id}"))
                        .test_support()
                        .aria_label(format!("{} ({} bytes)", attachment.name, attachment.size))
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .h(px(26.))
                        .px_3()
                        .rounded_full()
                        .border_1()
                        .border_color(t.line)
                        .text_size(t.px(12.))
                        .cursor_pointer()
                        .hover(|s| s.bg(t.bg3))
                        .child(
                            Icon::new(IconName::Paperclip)
                                .with_size(px(12.))
                                .text_color(t.fg2),
                        )
                        .child(attachment.name.clone())
                        .child(div().text_color(t.fg2).child(format_size(attachment.size)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let mail = this.mail.clone();
                            let task = cx
                                .background_executor()
                                .spawn(async move { mail.open_attachment(id) });
                            cx.spawn(async move |this, cx| {
                                if let Err(error) = task.await {
                                    this.update(cx, |this, cx| {
                                        this.toast(
                                            format!("Could not open attachment: {error}"),
                                            cx,
                                        )
                                    })
                                    .ok();
                                }
                            })
                            .detach();
                        }))
                }),
            ));
        }
        // The conversation ends where you answer it.
        if is_last {
            let recipients = message.to.len() + message.cc.len();
            column = column.child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(reply_button(
                        &t,
                        keys::cmd::REPLY,
                        IconName::Reply,
                        "Reply",
                        cx,
                    ))
                    .when(recipients > 1, |d| {
                        d.child(reply_button(
                            &t,
                            keys::cmd::REPLY_ALL,
                            IconName::ReplyAll,
                            "Reply all",
                            cx,
                        ))
                    })
                    .child(reply_button(
                        &t,
                        keys::cmd::FORWARD,
                        IconName::Forward,
                        "Forward",
                        cx,
                    )),
            );
        }
        card.child(column).into_any_element()
    }

    /// The rendered HTML body: an image at the exact rendered height, with link hit testing.
    fn html_body(
        &mut self,
        message: &Message,
        source: &str,
        scale: f32,
        body_width: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = self.theme;
        let width = (body_width * scale).round() / scale;
        let key = HtmlKey::new(message.id, width, scale, self.dark, self.appearance_version);
        self.request_html(source, key, cx);
        match self.html_cache.get(&key) {
            Some(rendered) => {
                let image: Arc<RenderImage> = rendered.image.clone();
                let height = rendered.height;
                let id = message.id;
                let bounds = self.html_bounds.clone();
                div()
                    .id(format!("html-{id}"))
                    .test_support()
                    .aria_label(format!("HTML body, {height:.0}px"))
                    .relative()
                    .w(px(body_width))
                    .h(px(height))
                    .child(
                        img(ImageSource::Render(image))
                            .w(px(body_width))
                            .h(px(height)),
                    )
                    .child(
                        canvas(
                            {
                                let bounds = bounds.clone();
                                move |area, _, _| {
                                    bounds.borrow_mut().insert(id, area);
                                }
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .inset_0(),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            let Some(area) = this.html_bounds.borrow().get(&id).copied() else {
                                return;
                            };
                            let at = event.position - area.origin;
                            if let Some(href) = this
                                .html_cache
                                .get(&key)
                                .and_then(|r| r.link_at(at.x.into(), at.y.into()))
                            {
                                cx.open_url(&href);
                                cx.stop_propagation();
                            }
                        }),
                    )
                    .into_any_element()
            }
            None => div()
                .id(format!("html-{}", message.id))
                .test_support()
                .aria_label("Rendering")
                .w(px(body_width))
                .h(px(120.))
                .rounded(px(RADIUS_SM))
                .bg(Theme::alpha(t.bg3, 0.6))
                .into_any_element(),
        }
    }
}

impl Kite {
    /// Renders one message body in the background unless it is already cached or in flight.
    fn request_html(&mut self, source: &str, key: HtmlKey, cx: &mut Context<Self>) {
        if self.html_cache.get(&key).is_some() || !self.html_cache.start(key) {
            return;
        }
        let (width, scale) = key.at();
        let source = source.to_string();
        let theme = self.theme;
        let font = crate::theme::font(&self.settings).to_string();
        let task = cx
            .background_executor()
            .spawn(async move { html::render_styled(&source, width, scale, theme, &font) });
        cx.spawn(async move |this, cx| {
            let rendered = task.await;
            this.update(cx, |this, cx| {
                this.html_cache.finish(key, rendered);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Renders the bodies of a prefetched thread so opening it is instant.
    pub(crate) fn warm_html(&mut self, messages: &[Message], cx: &mut Context<Self>) {
        for message in messages {
            let Some(source) = message.html.clone() else {
                continue;
            };
            let key = HtmlKey::new(
                message.id,
                self.body_width(),
                self.html_scale,
                self.dark,
                self.appearance_version,
            );
            self.request_html(&source, key, cx);
        }
    }
}

/// A reply-shaped outline button: the same verb the toolbar offers, where the reader is looking.
fn reply_button(
    t: &Theme,
    cmd: u8,
    icon: IconName,
    label: &str,
    cx: &mut Context<Kite>,
) -> AnyElement {
    div()
        .id(("inline-action", cmd as usize))
        .test_support()
        .aria_label(label.to_string())
        .flex()
        .items_center()
        .gap_1p5()
        .h(px(32.))
        .px_4()
        .rounded(px(RADIUS_SM))
        .border_1()
        .border_color(t.line)
        .cursor_pointer()
        .hover(|s| s.bg(t.bg3))
        .child(Icon::new(icon).with_size(px(15.)).text_color(t.fg2))
        .child(
            div()
                .text_size(t.px(13.))
                .font_weight(FontWeight(500.))
                .child(label.to_string()),
        )
        .on_click(cx.listener(move |this, _, window, cx| this.run(cmd, window, cx)))
        .into_any_element()
}

/// Avatar.svelte: the same address is always the same mid-dark hue, so the initials read white
/// in either appearance.
fn avatar(sender: &str, ix: usize, size: f32) -> AnyElement {
    let hue = sender.chars().fold(0u32, |hash, c| {
        (hash.wrapping_mul(31).wrapping_add(c as u32)) % 360
    });
    let initials = initials(sender);
    div()
        .id(("avatar", ix))
        .test_support()
        .aria_label(initials.clone())
        .size(px(size))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(hsla(hue as f32 / 360., 0.34, 0.42, 1.))
        .text_size(px(size * 0.4))
        .font_weight(FontWeight(650.))
        .text_color(white())
        .child(initials)
        .into_any_element()
}

fn initials(name: &str) -> String {
    name.split_whitespace()
        .filter_map(|part| part.chars().next())
        .take(2)
        .collect::<String>()
        .to_uppercase()
}

fn address(addr: &Addr) -> String {
    if addr.name.is_empty() {
        addr.email.clone()
    } else {
        format!("{} <{}>", addr.name, addr.email)
    }
}

fn addresses(addrs: &[Addr]) -> String {
    addrs.iter().map(address).collect::<Vec<_>>().join(", ")
}

/// Names only, as the reading pane's `To` and `Cc` lines show them.
fn names(addrs: &[Addr]) -> String {
    addrs
        .iter()
        .map(|addr| {
            if addr.name.is_empty() {
                addr.email.clone()
            } else {
                addr.name.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// `178 KB`, `2.4 MB`: what an attachment chip shows.
fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{} KB", bytes.div_ceil(1024))
    } else {
        format!("{:.1} MB", bytes as f64 / (1024. * 1024.))
    }
}

/// The first 120 characters of the text part, for collapsed headers.
fn snippet(message: &Message) -> String {
    let source = message.text.trim();
    if source.is_empty() {
        String::new()
    } else {
        source
            .lines()
            .next()
            .unwrap_or_default()
            .chars()
            .take(120)
            .collect()
    }
}
