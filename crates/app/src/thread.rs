//! The conversation view. Older messages collapse to a one-line header; the latest expands.
//! HTML bodies are rendered by Blitz on the background executor and cached per
//! (message, width, theme, scale). Text bodies render as GPUI text.
use crate::app::{Kite, Lru};
use crate::html::{self, Rendered};
use crate::theme::{MAX_COL, Theme};
use chrono::{Local, TimeZone as _};
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
}

impl HtmlKey {
    pub fn new(id: Id, width: f32, scale: f32, dark: bool) -> Self {
        Self {
            id,
            // Quantised so a drag-resize does not miss the cache every frame.
            width: (width * 4.).round() as u32,
            scale: (scale * 20.).round() as u32,
            dark,
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
        let body_width = (window.viewport_size().width.as_f32().min(MAX_COL) - 48.).max(120.);
        self.html_scale = scale;
        self.html_width = body_width;
        let Some(open) = &self.open else {
            return div().into_any_element();
        };
        let subject = open.subject.clone();
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
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .px_6()
                    .py_4()
                    .border_b_1()
                    .border_color(t.line)
                    .child(
                        div()
                            .id("thread-subject")
                            .test_support()
                            .aria_label(subject.clone())
                            .text_size(px(20.))
                            .font_weight(FontWeight(650.))
                            .truncate()
                            .child(subject),
                    )
                    .child(crate::list::keys_pills(t, "u")),
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
            .map(|at| at.format("%b %-d, %-H:%M").to_string())
            .unwrap_or_default();
        let header = div()
            .flex()
            .items_center()
            .gap_3()
            .flex_shrink_0()
            .child(
                div()
                    .w(px(20.))
                    .text_size(px(13.))
                    .text_color(t.fg3)
                    .child(initials(&message.from.name)),
            )
            .child(
                div()
                    .text_size(px(14.))
                    .font_weight(FontWeight(650.))
                    .child(if message.from.name.is_empty() {
                        message.from.email.clone()
                    } else {
                        message.from.name.clone()
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .truncate()
                    .text_size(px(12.))
                    .text_color(t.fg3)
                    .child(if open {
                        format!("to {}", recipients(&message))
                    } else {
                        snippet(&message)
                    }),
            )
            .child(div().text_size(px(12.)).text_color(t.fg3).child(when))
            .when(!open, |d| d.text_color(t.fg2));
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
                .text_size(px(15.))
                .line_height(relative(1.5))
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
                        .children(message.attachments.iter().map(|a| {
                            div()
                                .px_2()
                                .py_1()
                                .rounded(px(6.))
                                .bg(t.bg3)
                                .text_size(px(12.))
                                .child(a.name.clone())
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
        let key = HtmlKey::new(message.id, width, scale, self.dark);
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
        let dark = key.dark;
        let task = cx
            .background_executor()
            .spawn(async move { html::render(&source, width, scale, dark) });
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
            let key = HtmlKey::new(message.id, self.html_width, self.html_scale, self.dark);
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

fn recipients(message: &Message) -> String {
    let names: Vec<String> = message
        .to
        .iter()
        .map(|a| {
            if a.name.is_empty() {
                a.email.clone()
            } else {
                a.name.clone()
            }
        })
        .collect();
    names.join(", ")
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
