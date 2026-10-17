//! The conversation view. Older messages collapse to a one-line header; the latest expands.
//! HTML bodies are rendered by Blitz on the background executor and cached per
//! (message, width, theme, scale). Text bodies render as GPUI text.
use crate::app::{Kite, Lru};
use crate::html::{self, Rendered};
use crate::theme::{MAX_COL, Theme};
use chrono::{Local, TimeZone as _};
use gpui_kit::component::Sizable;
use gpui_kit::prelude::*;
use gpui_kit::*;
use kite_core::model::{Id, Message};
use std::sync::Arc;

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
    pub fn render_thread(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme;
        let scale = window.scale_factor();
        let body_width = ((window.viewport_size().width.as_f32() - crate::sidebar::width(window))
            .min(MAX_COL)
            - 80.)
            .max(120.);
        self.html_scale = scale;
        self.html_width = body_width;
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
            .gap_4()
            .py_6();
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
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .justify_center()
                    .px_6()
                    .pt_6()
                    .pb_2()
                    .child(
                        div()
                            .w_full()
                            .max_w(px(MAX_COL - 48.))
                            .flex()
                            .flex_col()
                            .gap_4()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .id("back-to-inbox")
                                            .test_support()
                                            .aria_label("Back to list")
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .text_size(t.px(12.))
                                            .text_color(t.fg2)
                                            .cursor_pointer()
                                            .hover(|s| s.text_color(t.fg))
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.run(crate::keys::cmd::BACK, window, cx)
                                            }))
                                            .child("Back to list")
                                            .child(crate::list::keys_pills(&t, "u")),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_1()
                                            .child(self.action_icon(
                                                crate::keys::cmd::DONE,
                                                gpui_kit::assets::IconName::Archive,
                                                "Mark done (e)",
                                                cx,
                                            ))
                                            .child(self.action_icon(
                                                crate::keys::cmd::SNOOZE,
                                                gpui_kit::assets::IconName::Clock,
                                                "Snooze (h)",
                                                cx,
                                            ))
                                            .child(self.action_icon(
                                                crate::keys::cmd::MOVE,
                                                gpui_kit::assets::IconName::FolderInput,
                                                "Move to folder (l)",
                                                cx,
                                            ))
                                            .child(self.action_icon(
                                                crate::keys::cmd::STAR,
                                                gpui_kit::assets::IconName::Star,
                                                "Star (s)",
                                                cx,
                                            )),
                                    ),
                            )
                            .child(
                                div()
                                    .id("thread-subject")
                                    .test_support()
                                    .aria_label(subject.clone())
                                    .text_size(t.px(20.))
                                    .font_weight(FontWeight(650.))
                                    .child(subject),
                            )
                            .child(
                                div()
                                    .id("thread-account")
                                    .test_support()
                                    .aria_label(account.clone())
                                    .text_size(t.px(12.))
                                    .text_color(t.fg2)
                                    .child(format!(
                                        "{count} {} · {account}",
                                        if count == 1 { "message" } else { "messages" }
                                    )),
                            ),
                    ),
            )
            .child(
                div()
                    .id("thread-scroll")
                    .test_support()
                    .flex_1()
                    .overflow_y_scroll()
                    .track_scroll(&scroll)
                    .flex()
                    .justify_center()
                    .child(column.px_6()),
            )
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
        let when = Local
            .timestamp_millis_opt(message.date)
            .single()
            .map(|at| at.format("%a, %-d %b %Y · %H:%M %:z").to_string())
            .unwrap_or_default();
        let metadata = |field: &'static str, value: String| {
            div()
                .id(format!("message-{}-{ix}", field.to_lowercase()))
                .test_support()
                .aria_label(format!("{field}: {value}"))
                .flex()
                .items_start()
                .gap_2()
                .text_size(t.px(12.))
                .text_color(t.fg2)
                .child(div().w(px(36.)).flex_shrink_0().child(field))
                .child(div().flex_1().min_w_0().child(value))
        };
        let mut details = div().flex_1().min_w_0().flex().flex_col().gap_1().child(
            div()
                .text_size(t.px(14.))
                .font_weight(FontWeight(650.))
                .child(if message.from.name.is_empty() {
                    message.from.email.clone()
                } else {
                    message.from.name.clone()
                }),
        );
        if open {
            details = details
                .child(metadata("From", address(&message.from)))
                .child(metadata("To", addresses(&message.to)))
                .when(!message.cc.is_empty(), |d| {
                    d.child(metadata("Cc", addresses(&message.cc)))
                })
                .child(metadata("Date", when));
            if self
                .open
                .as_ref()
                .is_some_and(|thread| thread.subject != message.subject)
            {
                details = details.child(metadata("Subject", message.subject.clone()));
            }
        } else {
            details = details
                .child(
                    div()
                        .truncate()
                        .text_size(t.px(12.))
                        .text_color(t.fg2)
                        .child(snippet(&message)),
                )
                .child(div().text_size(t.px(11.)).text_color(t.fg2).child(when));
        }
        let header = div()
            .flex()
            .items_start()
            .gap_3()
            .flex_shrink_0()
            .child(
                div()
                    .size(px(32.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .flex_shrink_0()
                    .rounded_full()
                    .bg(t.bg3)
                    .text_size(t.px(12.))
                    .text_color(t.fg2)
                    .child(initials(&message.from.name)),
            )
            .child(details);
        let card = div()
            .id(format!("msg-{ix}"))
            .test_support()
            .aria_label(format!("{}: {}", message.from.name, message.subject))
            .flex()
            .flex_col()
            .gap_3()
            .rounded(px(10.))
            .when(open || is_last, |d| d.bg(t.bg2).p_4())
            .when(!open, |d| {
                d.px_2()
                    .py_2()
                    .rounded(px(8.))
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
                .text_size(t.px(15.))
                .line_height(relative(1.7))
                .child(message.text.clone())
                .into_any_element(),
        };
        card.child(body)
            .when(!message.attachments.is_empty(), |d| {
                d.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .children(message.attachments.iter().map(|attachment| {
                            let id = attachment.id;
                            div()
                                .id(format!("attachment-{id}"))
                                .test_support()
                                .aria_label(format!(
                                    "{} ({} bytes)",
                                    attachment.name, attachment.size
                                ))
                                .flex()
                                .items_center()
                                .gap_2()
                                .px_3()
                                .py_2()
                                .rounded(px(6.))
                                .bg(t.bg3)
                                .text_size(t.px(12.))
                                .cursor_pointer()
                                .hover(|s| s.bg(t.sel))
                                .child(
                                    gpui_kit::component::Icon::new(
                                        gpui_kit::assets::IconName::Paperclip,
                                    )
                                    .with_size(px(14.)),
                                )
                                .child(format!(
                                    "{} · {} KB",
                                    attachment.name,
                                    attachment.size.div_ceil(1024)
                                ))
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
                        })),
                )
            })
            .into_any_element()
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
                .rounded(px(8.))
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
                self.html_width,
                self.html_scale,
                self.dark,
                self.appearance_version,
            );
            self.request_html(&source, key, cx);
        }
    }
}

fn initials(name: &str) -> String {
    name.split_whitespace()
        .filter_map(|part| part.chars().next())
        .take(2)
        .collect::<String>()
        .to_uppercase()
}

fn address(addr: &kite_core::model::Addr) -> String {
    if addr.name.is_empty() {
        addr.email.clone()
    } else {
        format!("{} <{}>", addr.name, addr.email)
    }
}

fn addresses(addrs: &[kite_core::model::Addr]) -> String {
    addrs.iter().map(address).collect::<Vec<_>>().join(", ")
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
