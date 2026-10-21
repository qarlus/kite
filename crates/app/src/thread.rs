//! The conversation view: a title, then one flat list of messages under a hairline. Older
//! messages collapse to a one-line header; the latest expands. HTML bodies are rendered by Blitz
//! on the background executor and cached per (message, width, theme, scale). Text bodies render
//! as GPUI text.
use crate::app::{Kite, Lru};
use crate::html::{self, Rendered};
use crate::keys;
use crate::list::when;
use crate::theme::{MAX_COL, RADIUS, RADIUS_SM, Theme};
use chrono::{DateTime, Local, TimeZone as _};
use gpui_kit::assets::IconName;
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Icon, Sizable};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kite_core::model::{Addr, Id, Message, PRIVATE_TAG};
use std::ops::Range;
use std::sync::Arc;

/// The reading pane's side padding. `app.rs` subtracts it from `html_width`, so the width Blitz
/// renders at is the width the body is shown at.
pub const PANE_PAD: f32 = 32.;

/// Past the avatar column, so text bodies and everything else a message holds line up under its
/// sender's name. An HTML body is not indented: it is the pane's own column, aligned with the
/// header above it, so authored mail gets the whole width.
pub const BODY_INDENT: f32 = 44.;

/// How many fetched remote images are kept in memory. A body that asks for more keeps its
/// remaining images blocked, which bounds a long session without a retry loop.
const IMAGE_CACHE: usize = 128;

/// Cache key: a body is re-rendered when the message, width, DPI scale, the appearance (the pane
/// and ink a body is adapted to), the set of fetched remote images or the message's image
/// permission changes.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct HtmlKey {
    pub id: Id,
    width: u32,
    scale: u32,
    appearance: u64,
    images: u64,
    allowed: bool,
}

impl HtmlKey {
    pub fn new(
        id: Id,
        width: f32,
        scale: f32,
        appearance: u64,
        images: u64,
        allowed: bool,
    ) -> Self {
        Self {
            id,
            // Quantised to whole pixels: a drag-resize re-renders once per pixel, not once per
            // frame.
            width: width.round() as u32,
            scale: (scale * 20.).round() as u32,
            appearance,
            images,
            allowed,
        }
    }

    fn at(&self) -> (f32, f32) {
        (self.width as f32, self.scale as f32 / 20.)
    }
}

/// The link under the pointer right now, in whichever body it sits.
#[derive(Clone, PartialEq)]
pub(crate) struct Hover {
    pub message: Id,
    pub href: String,
    /// The link's visible text; for a text-body link it is the URL itself.
    pub text: String,
    /// The link's box inside its HTML body, for the accent highlight. `None` for a
    /// plain-text link, which is highlighted as a run instead.
    pub rect: Option<(f32, f32, f32, f32)>,
    /// The plain-text link's index in its body's ranges, for the same highlight.
    pub range: Option<usize>,
}

/// A link whose visible text and destination disagree, held for "Open anyway".
pub(crate) struct Confirm {
    pub url: String,
    pub text_host: String,
    pub link_host: String,
}

pub struct HtmlCache {
    inner: Lru<HtmlKey, Rendered>,
    pending: std::collections::HashSet<HtmlKey>,
    /// The key each message last painted, so a re-render under a new key keeps the previous
    /// frame on screen instead of dropping back to the placeholder.
    painted: std::collections::HashMap<Id, HtmlKey>,
    /// The link under the pointer, for the cursor, the highlight and the status pill.
    pub(crate) hover: Option<Hover>,
    /// The suspicious link waiting on the reader's decision.
    pub(crate) confirm: Option<Confirm>,
}

impl HtmlCache {
    pub fn new(cap: usize) -> Self {
        Self {
            inner: Lru::new(cap),
            pending: std::collections::HashSet::new(),
            painted: std::collections::HashMap::new(),
            hover: None,
            confirm: None,
        }
    }

    pub fn get(&self, key: &HtmlKey) -> Option<&Rendered> {
        self.inner.get(key)
    }

    /// The render a message last showed, while its new key is still rendering.
    pub fn shown(&self, id: Id) -> Option<&Rendered> {
        self.painted.get(&id).and_then(|key| self.inner.get(key))
    }

    /// Called when a body paints, so the fallback is what is on screen — a stale render
    /// finishing late must not point it at a frame the reader never saw.
    fn mark_shown(&mut self, key: HtmlKey) {
        self.painted.insert(key.id, key);
    }

    pub fn clear(&mut self) {
        self.inner.clear();
        self.pending.clear();
        self.painted.clear();
    }

    fn start(&mut self, key: HtmlKey) -> bool {
        self.pending.insert(key)
    }

    fn finish(&mut self, key: HtmlKey, rendered: Rendered) {
        self.pending.remove(&key);
        self.inner.insert(key, rendered);
        // A body that was never shown yet still counts as the best thing to paint while a
        // newer key renders; a painted key that fell out of the cache cannot be shown again.
        if self
            .painted
            .get(&key.id)
            .is_none_or(|painted| self.inner.get(painted).is_none())
        {
            self.painted.insert(key.id, key);
        }
    }
}

impl Kite {
    /// The Blitz render width: the reading pane's column, edge to edge, so wide tables and long
    /// lines are never clipped.
    pub(crate) fn body_width(&self) -> f32 {
        self.html_width.max(120.)
    }

    /// What a body takes from the appearance: the reader's type scale and font, plus the pane and
    /// ink its colours are adapted to.
    fn html_appearance(&self) -> u64 {
        use std::hash::{Hash as _, Hasher as _};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        ((self.theme.scale * 1000.).round() as u64).hash(&mut hasher);
        crate::theme::font(&self.settings).hash(&mut hasher);
        self.theme.dark.hash(&mut hasher);
        crate::theme::hex(self.theme.fg).hash(&mut hasher);
        crate::theme::hex(self.theme.accent).hash(&mut hasher);
        hasher.finish()
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
        let open_id = open.id;
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
        // A summary sits under the thread header, above the messages it describes.
        let summary = match &self.ai {
            Some(ai) => match ai.target {
                crate::app::AiFor::Summary(id) if id == open_id => Some((ai.text.clone(), ai.done)),
                _ => None,
            },
            None => None,
        };
        if let Some((text, done)) = summary {
            column = column.child(self.ai_summary_card(&text, done, cx));
        }
        for (ix, message) in messages.into_iter().enumerate() {
            let is_last = ix + 1 == count;
            column =
                column.child(self.message(message, ix, expanded, is_last, (scale, body_width), cx));
        }
        let scroll = self.thread_scroll.clone();
        let entity = cx.entity().downgrade();
        // One context menu for the whole reading pane: over a link it is the link's menu,
        // anywhere else it is the conversation's tag menu. A second menu inside would open
        // on top of the first — neither stops right-click propagation.
        let menu = move |menu: PopupMenu, window: &mut Window, cx: &mut Context<PopupMenu>| {
            let link = entity
                .upgrade()
                .and_then(|kite| kite.read(cx).html_cache.hover.clone());
            match link {
                Some(link) => link_menu(entity.clone(), link, menu),
                None => crate::tags::popup_with(entity.clone(), vec![open_id], menu, window, cx),
            }
        };
        div()
            .id("reading-pane")
            .flex_1()
            .flex()
            .flex_col()
            .min_h_0()
            .overflow_hidden()
            // One listener for every body: hit-test the painted links against the pointer.
            // Only a change repaints, so j/k stays under its frame budget.
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                this.hover_link(event.position, cx);
            }))
            .on_hover(cx.listener(|this, inside: &bool, _, cx| {
                if !*inside && this.html_cache.hover.take().is_some() {
                    cx.notify();
                }
            }))
            .context_menu(menu)
            .child(self.thread_toolbar(cx))
            .child(
                div()
                    .id("thread-scroll")
                    .test_support()
                    .flex_1()
                    // Without it a flex item is never shorter than its content, so nothing scrolls.
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&scroll)
                    // A column, not a row: in a row the conversation would be stretched to the
                    // pane's height and overflow itself instead of the scroll area.
                    .flex()
                    .flex_col()
                    .items_center()
                    .child(column),
            )
            .when_some(self.link_confirm(cx), |d, card| d.child(card))
            .when_some(self.link_pill(), |d, pill| d.child(pill))
            .into_any_element()
    }

    /// Hit-tests the pointer against the links of whatever painted bodies are on screen:
    /// O(messages × links), no re-render of anything.
    fn hover_link(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let mut hover = None;
        for (id, area) in self.html_bounds.borrow().iter() {
            if !area.contains(&position) {
                continue;
            }
            let at = position - area.origin;
            if let Some(link) = self
                .html_cache
                .shown(*id)
                .and_then(|r| r.hit(at.x.into(), at.y.into()))
            {
                hover = Some(Hover {
                    message: *id,
                    href: link.href.clone(),
                    text: link.text.clone(),
                    rect: Some((link.x, link.y, link.w, link.h)),
                    range: None,
                });
            }
            break;
        }
        if self.html_cache.hover != hover {
            self.html_cache.hover = hover;
            cx.notify();
        }
    }

    /// InteractiveText's character hover, mapped back onto the text body's link ranges.
    fn hover_text(
        &mut self,
        message: Id,
        index: Option<usize>,
        ranges: &[Range<usize>],
        hrefs: &[String],
        cx: &mut Context<Self>,
    ) {
        let hover = index
            .and_then(|ix| ranges.iter().position(|range| range.contains(&ix)))
            .map(|range| Hover {
                message,
                href: hrefs[range].clone(),
                text: hrefs[range].clone(),
                rect: None,
                range: Some(range),
            });
        if self.html_cache.hover != hover {
            self.html_cache.hover = hover;
            cx.notify();
        }
    }

    /// Runs a link. `mailto:` opens compose addressed to it, `http(s)` opens the browser
    /// once tracking parameters are stripped and the text/href phishing check has passed,
    /// and every other scheme (`javascript:`, `data:`, `file:`…) never leaves the app.
    pub(crate) fn open_link(
        &mut self,
        href: &str,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let href = href.trim();
        if let Some(addr) = crate::html::mailto(href) {
            self.open_compose(window, cx);
            if let Some(compose) = &self.compose {
                compose.to.update(cx, |state, cx| {
                    state.set_value(addr, window, cx);
                });
            }
            return;
        }
        let lower = href.get(..10).unwrap_or(href).to_ascii_lowercase();
        if !(lower.starts_with("http://") || lower.starts_with("https://")) {
            return;
        }
        if let Some((text_host, link_host)) = crate::html::link_warning(text, href) {
            self.html_cache.confirm = Some(Confirm {
                url: crate::html::clean_url(href),
                text_host,
                link_host,
            });
            cx.notify();
            return;
        }
        cx.open_url(&crate::html::clean_url(href));
    }

    /// The browser-style status pill at the reading pane's bottom-left, shown while a link
    /// is under the pointer. It tells the truth about the destination: the cleaned URL,
    /// or the mismatch warning when text and href disagree.
    fn link_pill(&self) -> Option<AnyElement> {
        let hover = self.html_cache.hover.as_ref()?;
        let t = self.theme;
        let pill = div()
            .id("link-pill")
            .test_support()
            .absolute()
            .left(px(PANE_PAD))
            .bottom(px(10.))
            .flex()
            .items_center()
            .gap_1p5()
            .max_w(px(440.))
            .h(px(24.))
            .px(px(10.))
            .rounded(px(RADIUS_SM))
            .bg(t.bg2)
            .border_1()
            .border_color(t.line)
            .text_size(t.px(11.));
        if let Some((text_host, link_host)) = crate::html::link_warning(&hover.text, &hover.href) {
            let label = format!("Text says {text_host} — link goes to {link_host}");
            return Some(
                pill.aria_label(label.clone())
                    .child(
                        Icon::new(IconName::ShieldAlert)
                            .with_size(px(12.))
                            .text_color(t.danger)
                            .flex_shrink_0(),
                    )
                    .child(div().min_w_0().truncate().child(label))
                    .into_any_element(),
            );
        }
        let url = crate::html::clean_url(&hover.href);
        let (pre, host, rest) = url_parts(&url);
        Some(
            pill.aria_label(url.clone())
                .child(div().flex_shrink_0().text_color(t.fg3).child(pre))
                .child(div().flex_shrink_0().child(host))
                .child(div().min_w_0().truncate().text_color(t.fg3).child(rest))
                .into_any_element(),
        )
    }

    /// The "Open anyway?" card for a link whose visible text disagrees with its destination.
    fn link_confirm(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let confirm = self.html_cache.confirm.as_ref()?;
        let t = self.theme;
        let url = confirm.url.clone();
        let label = format!(
            "Text says {} — link goes to {}",
            confirm.text_host, confirm.link_host
        );
        Some(
            div()
                .id("link-confirm")
                .test_support()
                .aria_label(label.clone())
                .absolute()
                .left(px(PANE_PAD))
                .bottom(px(10.))
                .w(px(360.))
                .p_3()
                .rounded(px(RADIUS))
                .bg(t.bg2)
                .border_1()
                .border_color(t.line)
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .flex()
                        .items_start()
                        .gap_2()
                        .child(
                            Icon::new(IconName::ShieldAlert)
                                .with_size(px(14.))
                                .text_color(t.danger)
                                .flex_shrink_0(),
                        )
                        .child(div().text_size(t.px(12.)).child(label)),
                )
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            crate::preferences::button("link-confirm-open", "Open anyway", t)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.html_cache.confirm = None;
                                    cx.open_url(&url);
                                })),
                        )
                        .child(
                            crate::preferences::button("link-confirm-cancel", "Cancel", t)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.html_cache.confirm = None;
                                    cx.notify();
                                })),
                        ),
                )
                .into_any_element(),
        )
    }

    /// The reading-pane toolbar: icon-only buttons in the order they act on the conversation,
    /// with Back first. Each label carries the key that is bound right now, so a click-first
    /// reader loses nothing. The list/reading-pane toggle lives in the window strip, above the
    /// list column it acts on.
    fn thread_toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme;
        const GROUPS: [&[(u8, IconName, &str)]; 5] = [
            &[
                (keys::cmd::DONE, IconName::Archive, "Mark done"),
                (keys::cmd::TRASH, IconName::Trash, "Trash"),
            ],
            &[
                (keys::cmd::MOVE, IconName::FolderInput, "Move to folder"),
                (keys::cmd::SNOOZE, IconName::Clock, "Snooze"),
            ],
            &[
                (keys::cmd::REPLY, IconName::Reply, "Reply"),
                (keys::cmd::REPLY_ALL, IconName::ReplyAll, "Reply all"),
                (keys::cmd::FORWARD, IconName::Forward, "Forward"),
            ],
            // Shift+S is a single key, so click-first mode has no other way to summarize. It
            // goes away when AI is off, the rule the palette and the cheat sheet keep.
            &[(keys::cmd::AI_SUMMARY, IconName::Sparkles, "Summarize")],
            &[
                (keys::cmd::TOGGLE_READ, IconName::Mails, "Mark unread"),
                (keys::cmd::STAR, IconName::Star, "Star"),
            ],
        ];
        let hint = |cmd: u8, label: &str| {
            let hint = keys::hint_for(&self.settings, cmd);
            if hint.is_empty() {
                label.to_string()
            } else {
                format!("{label} ({hint})")
            }
        };
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
            .child(self.action_icon(
                keys::cmd::BACK,
                IconName::ArrowLeft,
                hint(keys::cmd::BACK, "Back"),
                cx,
            ))
            .child(div().flex_1());
        for group in GROUPS {
            if !self.settings.ai_enabled && group.iter().all(|(cmd, _, _)| keys::needs_ai(*cmd)) {
                continue;
            }
            bar = bar.child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .children(group.iter().map(|(cmd, icon, label)| {
                        self.action_icon(*cmd, *icon, hint(*cmd, label), cx)
                    })),
            );
        }
        bar.into_any_element()
    }

    /// The streamed answer to Shift+S, above the messages it summarizes.
    fn ai_summary_card(&self, text: &str, done: bool, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme;
        div()
            .id("ai-summary")
            .test_support()
            .aria_label(text.to_string())
            .mt_4()
            .p_4()
            .rounded(px(RADIUS_SM))
            .bg(t.bg2)
            .border_1()
            .border_color(t.line)
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .child(
                        div()
                            .text_size(t.px(12.))
                            .font_weight(FontWeight(650.))
                            .text_color(t.fg2)
                            .child(if done { "AI summary" } else { "Summarizing…" }),
                    )
                    .child(
                        crate::preferences::button("ai-summary-dismiss", "Dismiss", t).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.ai = None;
                                cx.notify();
                            }),
                        ),
                    ),
            )
            .child(
                div()
                    .text_size(t.px(13.))
                    .line_height(relative(1.6))
                    .child(text.to_string()),
            )
            .into_any_element()
    }

    /// Whether the open conversation carries the Private tag, which bars AI helpers.
    fn open_is_private(&self) -> bool {
        self.open.as_ref().is_some_and(|open| {
            self.rows
                .iter()
                .find(|row| row.id == open.id)
                .is_some_and(|row| {
                    row.labels
                        .iter()
                        .any(|tag| tag.eq_ignore_ascii_case(PRIVATE_TAG))
                })
        })
    }

    /// "Suggest replies" or the three chips it streamed: numbered lines the engine
    /// emits in the conversation's own words, each one a reply starter.
    fn suggestions(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme;
        let open_id = self.open.as_ref().map(|open| open.id).unwrap_or(0);
        let in_flight = match &self.ai {
            Some(ai) if matches!(ai.target, crate::app::AiFor::Replies(id) if id == open_id) => {
                Some((suggested(&ai.text), ai.done))
            }
            _ => None,
        };
        match in_flight {
            None => div()
                .id("suggest-replies")
                .test_support()
                .aria_label("Suggest replies")
                .flex()
                .items_center()
                .gap_1p5()
                .h(px(28.))
                .px_3()
                .rounded(px(RADIUS_SM))
                .cursor_pointer()
                .text_color(t.fg2)
                .hover(|s| s.bg(t.bg3).text_color(t.fg))
                .child(
                    Icon::new(IconName::Sparkles)
                        .with_size(px(13.))
                        .text_color(t.fg2),
                )
                .child(div().text_size(t.px(12.)).child("Suggest replies"))
                .on_click(cx.listener(|this, _, _, cx| this.suggest_replies(cx)))
                .into_any_element(),
            Some((items, done)) => {
                let mut row = div()
                    .id("suggestions")
                    .test_support()
                    .aria_label("Reply suggestions")
                    .flex()
                    .flex_wrap()
                    .gap_2();
                if items.is_empty() && !done {
                    // Three quiet placeholders while the stream warms up.
                    for ix in 0..3usize {
                        row = row.child(
                            div()
                                .id(("suggestion-skeleton", ix))
                                .test_support()
                                .aria_label("Loading suggestion")
                                .w(px(160.))
                                .h(px(32.))
                                .rounded(px(RADIUS_SM))
                                .bg(t.bg3),
                        );
                    }
                }
                for (ix, text) in items.into_iter().enumerate() {
                    row = row.child(
                        div()
                            .id(("suggestion", ix))
                            .test_support()
                            .aria_label(text.clone())
                            .max_w(px(560.))
                            .px_3()
                            .py_1p5()
                            .rounded(px(RADIUS_SM))
                            .border_1()
                            .border_color(t.line)
                            .cursor_pointer()
                            .text_size(t.px(13.))
                            .hover(|s| s.bg(t.bg3))
                            .child(text.clone())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.reply_with_suggestion(text.clone(), window, cx)
                            })),
                    );
                }
                row.into_any_element()
            }
        }
    }

    /// "Remote images are blocked" plus the way to load them for this message only.
    fn image_notice(&self, id: Id, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme;
        div()
            .id(format!("image-notice-{id}"))
            .test_support()
            .aria_label("Remote images are blocked")
            .flex()
            .items_center()
            .gap_3()
            .px_3()
            .py_2()
            .rounded(px(RADIUS_SM))
            .bg(t.bg2)
            .text_size(t.px(12.))
            .text_color(t.fg2)
            .child("Remote images are blocked.")
            .child(
                crate::preferences::button(format!("load-images-{id}"), "Load images", t).on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.images_allowed.insert(id);
                        // `allowed` is part of the render key, so the body re-renders with its
                        // images inlined — and keeps painting the old frame until it lands.
                        cx.notify();
                    }),
                ),
            )
            .into_any_element()
    }

    /// Fetches the images a message is now allowed to load, once each, off the UI thread.
    fn request_images(&mut self, urls: &[String], cx: &mut Context<Self>) {
        for url in urls {
            if self.images.len() >= IMAGE_CACHE
                || self.images.contains_key(url)
                || !self.images_pending.insert(url.clone())
            {
                continue;
            }
            let url = url.clone();
            let mail = self.mail.clone();
            let asked = url.clone();
            let task = cx
                .background_executor()
                .spawn(async move { mail.fetch_image(&asked) });
            cx.spawn(async move |this, cx| {
                let result = task.await;
                this.update(cx, |this, cx| {
                    // A URL only leaves the asked set once it has bytes: a failure is not
                    // retried on every frame, which would hammer a sender's server.
                    let Ok(bytes) = result else {
                        return;
                    };
                    this.images_pending.remove(&url);
                    let mut images = (*this.images).clone();
                    images.insert(url, crate::html::data_url(&bytes));
                    this.images = std::sync::Arc::new(images);
                    // A fetched image is part of the render key: the body re-renders with it
                    // inlined while the last painted frame stays up — no placeholder flash.
                    this.images_version += 1;
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
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
        let date = || {
            div()
                .id(format!("message-date-{ix}"))
                .test_support()
                .aria_label(exact.clone())
                .flex_shrink_0()
                .text_size(t.px(12.))
                .text_color(t.fg2)
                .child(stamp.clone())
        };
        let name = || {
            div()
                .text_size(t.px(14.))
                .font_weight(FontWeight(650.))
                .child(sender.clone())
        };
        let header = if open {
            div()
                .id(format!("message-from-{ix}"))
                .test_support()
                .aria_label(format!("From: {}", address(&message.from)))
                .flex()
                .items_start()
                .gap_3()
                .flex_shrink_0()
                .child(avatar(&sender, ix, 32.))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .flex()
                                .items_baseline()
                                .gap_2()
                                .min_w_0()
                                .child(name().flex_shrink_0())
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_size(t.px(12.))
                                        .text_color(t.fg2)
                                        .child(message.from.email.clone()),
                                )
                                .child(date()),
                        )
                        .child(
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
                        }),
                )
        } else {
            // One line: who, what, when. The snippet takes whatever the name leaves and the
            // date sits at the row's right edge, in the column an expanded message's date is in.
            div()
                .id(format!("message-from-{ix}"))
                .test_support()
                .aria_label(format!("From: {}", address(&message.from)))
                .flex()
                .items_center()
                .gap_3()
                .flex_shrink_0()
                .child(avatar(&sender, ix, 24.))
                .child(
                    name()
                        .id(format!("message-sender-{ix}"))
                        .test_support()
                        .aria_label(sender.clone())
                        .min_w_0()
                        .flex_shrink_1()
                        .truncate(),
                )
                .child(
                    div()
                        .id(format!("message-snippet-{ix}"))
                        .test_support()
                        .aria_label(snippet(&message))
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(t.px(12.))
                        .text_color(t.fg2)
                        .child(snippet(&message)),
                )
                .child(date())
        };
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
            Some(source) => {
                // Only a reader who asked for remote images can load them here.
                let allowed =
                    self.settings.load_remote_images || self.images_allowed.contains(&message.id);
                let remote = crate::html::remote_images(source);
                let blocked = !allowed && !remote.is_empty();
                if allowed && !remote.is_empty() {
                    self.request_images(&remote, cx);
                }
                let mut stack = div().flex().flex_col().gap_3();
                // The tracker shield's quiet report; a body without pixels shows no line.
                let found = crate::html::trackers(source);
                if !found.is_empty() {
                    let mut names: Vec<&str> = Vec::new();
                    for name in &found {
                        if !names.contains(&name.as_str()) {
                            names.push(name);
                        }
                    }
                    let list = names.join(", ");
                    let count = found.len();
                    stack = stack.child(
                        div()
                            .id(format!("trackers-{}", message.id))
                            .test_support()
                            .aria_label(format!(
                                "{count} tracker{} blocked: {list}",
                                if count == 1 { "" } else { "s" }
                            ))
                            .ml(px(BODY_INDENT))
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .text_size(t.px(12.))
                            .text_color(t.fg3)
                            .child(
                                Icon::new(IconName::ShieldAlert)
                                    .with_size(px(13.))
                                    .text_color(t.fg3),
                            )
                            .child(format!(
                                "{count} tracker{} blocked: {list}",
                                if count == 1 { "" } else { "s" }
                            ))
                            .tooltip(move |window, cx| {
                                Tooltip::new(format!("Blocked trackers: {list}")).build(window, cx)
                            }),
                    );
                }
                if blocked {
                    stack = stack.child(self.image_notice(message.id, cx));
                }
                stack
                    .child(self.html_body(&message, source, allowed, scale, body_width, cx))
                    .into_any_element()
            }
            // Text bodies keep their theme type and hang under the sender's name. URLs and
            // addresses in them are real links: pointer cursor, accent ink, click and hover.
            None => {
                let (ranges, hrefs) = text_links(&message.text);
                let body = div()
                    .id(format!("text-{}", message.id))
                    .test_support()
                    .aria_label(message.text.clone())
                    .ml(px(BODY_INDENT))
                    .max_w(px(body_width - BODY_INDENT))
                    .text_size(t.px(15.))
                    .line_height(relative(1.7));
                if ranges.is_empty() {
                    body.child(message.text.clone()).into_any_element()
                } else {
                    let hovered = self
                        .html_cache
                        .hover
                        .as_ref()
                        .filter(|h| h.message == message.id)
                        .and_then(|h| h.range);
                    let styled = StyledText::new(message.text.clone()).with_highlights(
                        ranges.iter().enumerate().map(|(ix, range)| {
                            (
                                range.clone(),
                                HighlightStyle {
                                    color: Some(t.accent.into()),
                                    underline: Some(UnderlineStyle {
                                        thickness: px(1.),
                                        color: Some(t.accent.into()),
                                        wavy: false,
                                    }),
                                    background_color: (hovered == Some(ix))
                                        .then(|| Theme::alpha(t.accent, 0.16)),
                                    ..Default::default()
                                },
                            )
                        }),
                    );
                    let weak = cx.entity().downgrade();
                    let mid = message.id;
                    let click = {
                        let hrefs = hrefs.clone();
                        let weak = weak.clone();
                        move |ix: usize, window: &mut Window, cx: &mut App| {
                            let href = hrefs[ix].clone();
                            let _ = weak
                                .update(cx, |this, cx| this.open_link(&href, &href, window, cx));
                        }
                    };
                    let hover = {
                        let ranges = ranges.clone();
                        let hrefs = hrefs.clone();
                        move |ix: Option<usize>, _: MouseMoveEvent, _: &mut Window, cx: &mut App| {
                            let _ = weak.update(cx, |this, cx| {
                                this.hover_text(mid, ix, &ranges, &hrefs, cx)
                            });
                        }
                    };
                    body.child(
                        InteractiveText::new(("text-links", message.id as usize), styled)
                            .on_click(ranges.clone(), click)
                            .on_hover(hover),
                    )
                    .into_any_element()
                }
            }
        };
        let mut column = div().pt_5().flex().flex_col().gap_4().child(body);
        // Attachment chips and the reply row still start where the sender's name does.
        let mut tail = div().pl(px(BODY_INDENT)).flex().flex_col().gap_4();
        let mut holds_tail = false;
        if !message.attachments.is_empty() {
            holds_tail = true;
            tail = tail.child(div().flex().flex_wrap().gap_2().children(
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
            holds_tail = true;
            let recipients = message.to.len() + message.cc.len();
            tail = tail.child(
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
            // AI reply suggestions sit below the reply row, quiet until asked for.
            // Private conversations never offer them.
            if self.settings.ai_enabled && !self.open_is_private() {
                tail = tail.child(self.suggestions(cx));
            }
        }
        if holds_tail {
            column = column.child(tail);
        }
        card.child(column).into_any_element()
    }

    /// The rendered HTML body: the image at its exact rendered size, on the pane's own canvas,
    /// with link hit testing.
    fn html_body(
        &mut self,
        message: &Message,
        source: &str,
        images_allowed: bool,
        scale: f32,
        body_width: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = self.theme;
        let key = HtmlKey::new(
            message.id,
            body_width,
            scale,
            self.html_appearance(),
            self.images_version,
            images_allowed,
        );
        // A blocked body has every remote source replaced, so nothing can reach the network.
        let images = if images_allowed {
            self.images.clone()
        } else {
            std::sync::Arc::new(std::collections::HashMap::new())
        };
        self.request_html(source, key, images, cx);
        if self.html_cache.get(&key).is_some() {
            self.html_cache.mark_shown(key);
        }
        // While a re-render for a new key is in flight the last painted frame stays up:
        // a body that was shown once never drops back to the placeholder.
        match self
            .html_cache
            .get(&key)
            .or_else(|| self.html_cache.shown(message.id))
        {
            Some(rendered) => {
                let image: Arc<RenderImage> = rendered.image.clone();
                let (width, height) = (rendered.width, rendered.height);
                let id = message.id;
                let bounds = self.html_bounds.clone();
                let hover = self.html_cache.hover.clone().filter(|h| h.message == id);
                // No card, border or padding: the render is the pane's own canvas, so it has no
                // edge to round.
                let page = img(ImageSource::Render(image)).w(px(width)).h(px(height));
                let mut body = div()
                    .id(format!("html-{id}"))
                    .test_support()
                    .aria_label(format!("HTML body, {height:.0}px"))
                    .relative()
                    .w(px(width))
                    .h(px(height))
                    .when(hover.is_some(), |d| d.cursor_pointer())
                    .child(page)
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
                    );
                // The browser-style link cue: a soft accent wash over the link's box.
                if let Some((x, y, w, h)) = hover.and_then(|h| h.rect) {
                    body = body.child(
                        div()
                            .id("link-highlight")
                            .test_support()
                            .aria_label("Link")
                            .absolute()
                            .left(px(x - 2.))
                            .top(px(y - 2.))
                            .w(px(w + 4.))
                            .h(px(h + 4.))
                            .rounded(px(4.))
                            .bg(Theme::alpha(t.accent, 0.16)),
                    );
                }
                body.on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                        let Some(area) = this.html_bounds.borrow().get(&id).copied() else {
                            return;
                        };
                        let at = event.position - area.origin;
                        let link = this
                            .html_cache
                            .get(&key)
                            .or_else(|| this.html_cache.shown(id))
                            .and_then(|r| r.hit(at.x.into(), at.y.into()))
                            .map(|l| (l.href.clone(), l.text.clone()));
                        if let Some((href, text)) = link {
                            this.open_link(&href, &text, window, cx);
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
                .rounded(px(RADIUS))
                .bg(Theme::alpha(t.bg3, 0.6))
                .into_any_element(),
        }
    }
}

impl Kite {
    /// Renders one message body in the background unless it is already cached or in flight.
    fn request_html(
        &mut self,
        source: &str,
        key: HtmlKey,
        images: Arc<std::collections::HashMap<String, String>>,
        cx: &mut Context<Self>,
    ) {
        if self.html_cache.get(&key).is_some() || !self.html_cache.start(key) {
            return;
        }
        let (width, scale) = key.at();
        let source = source.to_string();
        let theme = self.theme;
        let font = crate::theme::font(&self.settings).to_string();
        let task = cx.background_executor().spawn(async move {
            let html = crate::html::inline_images(&source, &images);
            html::render_styled(&html, width, scale, theme, &font)
        });
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
        let images = self.images.clone();
        for message in messages {
            let Some(source) = message.html.clone() else {
                continue;
            };
            let allowed =
                self.settings.load_remote_images || self.images_allowed.contains(&message.id);
            let key = HtmlKey::new(
                message.id,
                self.body_width(),
                self.html_scale,
                self.html_appearance(),
                self.images_version,
                allowed,
            );
            let images = if allowed {
                images.clone()
            } else {
                Arc::new(std::collections::HashMap::new())
            };
            self.request_html(&source, key, images, cx);
        }
    }
}

/// The engine streams replies as numbered lines ("1. …"); the list is what the chips offer.
fn suggested(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            let (number, rest) = line.split_once(['.', ')'])?;
            number.trim().parse::<u32>().ok()?;
            let rest = rest.trim();
            (!rest.is_empty()).then(|| rest.to_string())
        })
        .take(3)
        .collect()
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
        .on_click(
            cx.listener(move |this, _, window, cx| {
                this.run(cmd, crate::app::Src::Click, window, cx)
            }),
        )
        .into_any_element()
}

/// Avatar.svelte: the same address is always the same mid-dark hue, so the initials read white
/// in either appearance.
pub(crate) fn avatar(sender: &str, ix: usize, size: f32) -> AnyElement {
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

/// The right-click menu over a link, chosen by the reading pane's single context menu when
/// the pointer is on one. The same rules a click follows: web links open in the browser,
/// mailto opens compose, everything else is copy-only.
fn link_menu(entity: WeakEntity<Kite>, link: Hover, mut menu: PopupMenu) -> PopupMenu {
    let href = link.href.trim().to_string();
    let mailto = crate::html::mailto(&href);
    let web = {
        let lower = href.get(..10).unwrap_or(&href).to_ascii_lowercase();
        lower.starts_with("http://") || lower.starts_with("https://")
    };
    if web || mailto.is_some() {
        let (entity, link) = (entity.clone(), link.clone());
        let label = if mailto.is_some() {
            "Compose to this address"
        } else {
            "Open in browser"
        };
        menu = menu.item(PopupMenuItem::new(label).on_click(move |_, window, cx| {
            let _ = entity.update(cx, |kite, cx| {
                kite.open_link(&link.href, &link.text, window, cx);
            });
        }));
    }
    let label = if mailto.is_some() {
        "Copy address"
    } else {
        "Copy link"
    };
    let value = mailto.unwrap_or_else(|| crate::html::clean_url(&href));
    menu.item(PopupMenuItem::new(label).on_click(move |_, _, cx| {
        cx.write_to_clipboard(ClipboardItem::new_string(value.clone()));
    }))
}

/// `(byte range, href)` of every URL or address in a plain-text body. Web URLs open in the
/// browser, `user@domain` addresses open compose — the same treatment HTML links get.
fn text_links(text: &str) -> (Vec<Range<usize>>, Vec<String>) {
    /// Punctuation that hugs a token in prose but is never part of its URL or address.
    const TRIM: &str = ".,;:!?'\"()[]{}<>»”";
    let mut ranges = Vec::new();
    let mut hrefs = Vec::new();
    let mut start = None;
    for (ix, c) in text
        .char_indices()
        .chain(std::iter::once((text.len(), ' ')))
    {
        if !c.is_whitespace() {
            start.get_or_insert(ix);
            continue;
        }
        let Some(begin) = start.take() else {
            continue;
        };
        let raw = &text[begin..ix];
        let token = raw.trim_matches(|c: char| TRIM.contains(c));
        if token.is_empty() {
            continue;
        }
        let at = begin + raw.find(token).unwrap_or(0);
        let lower = token.to_ascii_lowercase();
        let href = if lower.starts_with("http://") || lower.starts_with("https://") {
            token.to_string()
        } else if !token.contains("://") && !token.contains(' ') && {
            let mut parts = token.rsplitn(2, '@');
            let domain = parts.next().unwrap_or_default();
            !parts.next().unwrap_or_default().is_empty() && domain.contains('.')
        } {
            format!("mailto:{token}")
        } else {
            continue;
        };
        ranges.push(at..at + token.len());
        hrefs.push(href);
    }
    (ranges, hrefs)
}

/// `(scheme, host, rest)` of a URL for the status pill, so the host reads in `fg` and the
/// rest in `fg3`. `mailto:` counts as a scheme; anything else is all "rest".
fn url_parts(url: &str) -> (String, String, String) {
    let after = match url.find("://") {
        Some(at) => at + 3,
        None if url
            .get(..7)
            .is_some_and(|s| s.eq_ignore_ascii_case("mailto:")) =>
        {
            7
        }
        None => return (String::new(), String::new(), url.to_string()),
    };
    let rest = &url[after..];
    let host_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    (
        url[..after].to_string(),
        rest[..host_end].to_string(),
        rest[host_end..].to_string(),
    )
}

/// `178 KB`, `2.4 MB`: what an attachment chip shows.
pub(crate) fn format_size(bytes: u64) -> String {
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
