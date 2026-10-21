//! Command palette: fuzzy search over the command table, 600px wide, 18% from the top.
use crate::app::{Kite, Overlay};
use crate::keys::{self, Cmd};
use crate::list::{keys_pills, overlay};
use crate::theme::{self, RADIUS, RADIUS_SM};
use gpui_kit::assets::IconName;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{Icon, Sizable};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kite_core::model::{Mailbox, Settings};
use std::collections::HashSet;

pub struct Palette {
    pub query: String,
    pub sel: usize,
    /// Command ids matching the query, best first.
    pub items: Vec<u8>,
    /// The AI helpers stay out of the list until Settings › AI turns them on.
    pub show_ai: bool,
    mailbox: Mailbox,
}

impl Palette {
    pub fn new(settings: &Settings, mailbox: Mailbox) -> Self {
        let mut palette = Self {
            query: String::new(),
            sel: 0,
            items: Vec::new(),
            show_ai: settings.ai_enabled,
            mailbox,
        };
        palette.refilter();
        palette
    }
}

impl Palette {
    pub fn selected_id(&self) -> Option<u8> {
        self.items.get(self.sel).copied()
    }

    pub fn push(&mut self, c: char) {
        self.query.push(c);
        self.refilter();
    }

    pub fn backspace(&mut self) {
        self.query.pop();
        self.refilter();
    }

    pub fn move_selection(&mut self, delta: isize) {
        if self.items.is_empty() {
            return;
        }
        let last = self.items.len() - 1;
        self.sel = (self.sel as isize + delta).clamp(0, last as isize) as usize;
    }

    fn refilter(&mut self) {
        let query = self.query.trim().to_lowercase();
        let mut scored: Vec<(i32, u8)> = keys::COMMANDS
            .iter()
            .filter(|c| self.show_ai || !keys::needs_ai(c.id))
            .filter(|c| match c.id {
                keys::cmd::DONE_WAITING => self.mailbox == Mailbox::Waiting,
                keys::cmd::CANCEL_SCHEDULE | keys::cmd::RESCHEDULE | keys::cmd::SEND_NOW => {
                    self.mailbox == Mailbox::Scheduled
                }
                _ => true,
            })
            .filter_map(|c| {
                let score = if query.is_empty() {
                    Some(0)
                } else {
                    fuzzy(&query, c.title)
                }?;
                Some((score, c.id))
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        self.items = scored.into_iter().map(|(_, id)| id).collect();
        let mut seen = HashSet::new();
        self.items.retain(|id| seen.insert(*id));
        self.sel = 0;
    }
}

/// "Get me to zero": archive everything in the current view older than a picked age.
/// `sel` indexes [`AGES`]; the count behind the confirm button comes from `Mail::archivable`.
#[derive(Default)]
pub struct Zero {
    pub sel: usize,
    /// Starred conversations are kept unless this is on. Snoozed ones always keep waiting.
    pub include_starred: bool,
    pub count: usize,
}

/// The age presets the dialog offers, as (label, cutoff span in milliseconds).
pub const AGES: [(&str, i64); 5] = [
    ("1 day", 86_400_000),
    ("3 days", 3 * 86_400_000),
    ("1 week", 7 * 86_400_000),
    ("2 weeks", 14 * 86_400_000),
    ("1 month", 30 * 86_400_000),
];

/// The bulk-archive dialog: a small centred panel — preset pills, a starred toggle and a
/// confirm button that carries the live count.
pub fn render_zero(kite: &Kite, _window: &Window, cx: &mut Context<Kite>) -> AnyElement {
    let t = kite.theme;
    let Overlay::Zero(zero) = &kite.overlay else {
        return div().into_any_element();
    };
    let view = kite.view_name().to_string();
    let pills = div()
        .flex()
        .flex_wrap()
        .gap_2()
        .children(AGES.iter().enumerate().map(|(ix, (label, _))| {
            let selected = ix == zero.sel;
            div()
                .id(("zero-preset", ix))
                .test_support()
                .aria_label(*label)
                .aria_selected(selected)
                .px_3()
                .py_1p5()
                .rounded(px(RADIUS_SM))
                .border_1()
                .border_color(if selected { t.accent } else { t.line })
                .bg(if selected { t.sel } else { t.bg2 })
                .text_size(t.px(12.))
                .cursor_pointer()
                .hover(|s| s.bg(t.bg3))
                .child(*label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Overlay::Zero(zero) = &mut this.overlay {
                        zero.sel = ix;
                    }
                    this.recount_zero(cx);
                }))
                .into_any_element()
        }));
    let starred = Switch::new("zero-starred")
        .checked(zero.include_starred)
        .accessibility_label("Include starred")
        .on_click(cx.listener(|this, checked: &bool, _, cx| {
            if let Overlay::Zero(zero) = &mut this.overlay {
                zero.include_starred = *checked;
            }
            this.recount_zero(cx);
        }));
    let confirm = format!("Archive {} conversations", zero.count);
    overlay(
        &t,
        px(0.),
        true,
        div()
            .id("zero-dialog")
            .test_support()
            .aria_label("Archive older mail")
            .w(px(420.))
            .rounded(px(RADIUS))
            .bg(t.bg2)
            .border_1()
            .border_color(t.line)
            .shadow_lg()
            .p_5()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(t.px(15.))
                            .font_weight(FontWeight(650.))
                            .child("Archive everything older than"),
                    )
                    .child(
                        div()
                            .text_size(t.px(12.))
                            .text_color(t.fg2)
                            .child(format!("in {view}")),
                    ),
            )
            .child(pills)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .child(
                        div()
                            .text_size(t.px(12.))
                            .text_color(t.fg2)
                            .child("Include starred"),
                    )
                    .child(starred),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap_2()
                    .child(
                        crate::preferences::button("zero-cancel", "Cancel", t).on_click(
                            cx.listener(|this, _, window, cx| {
                                this.close_overlay(cx);
                                window.focus(&this.focus, cx);
                            }),
                        ),
                    )
                    .child(
                        crate::preferences::button("zero-confirm", &confirm, t)
                            .bg(theme::accent_glow(&t))
                            .text_color(gpui_kit::white())
                            .border_color(gpui_kit::transparent_black())
                            .on_click(
                                cx.listener(|this, _, window, cx| this.confirm_zero(window, cx)),
                            ),
                    ),
            ),
    )
    .into_any_element()
}

/// Subsequence match with bonuses for consecutive runs and word starts.
pub(crate) fn fuzzy(query: &str, text: &str) -> Option<i32> {
    let hay: Vec<char> = text.to_lowercase().chars().collect();
    let mut score = 0;
    let mut hay_ix = 0;
    let mut previous: Option<usize> = None;
    for c in query.chars() {
        let start = hay_ix;
        while hay_ix < hay.len() && hay[hay_ix] != c {
            hay_ix += 1;
        }
        if hay_ix == hay.len() {
            return None;
        }
        if previous.is_some_and(|p| p + 1 == hay_ix) {
            score += 6;
        }
        if hay_ix == 0 || hay[hay_ix - 1] == ' ' {
            score += 4;
        }
        // Prefer tight matches: penalise how far we had to search forward.
        score += 1 - (hay_ix - start).min(8) as i32;
        previous = Some(hay_ix);
        hay_ix += 1;
    }
    Some(score)
}

pub fn render(kite: &Kite, window: &Window, cx: &mut Context<Kite>) -> AnyElement {
    let t = kite.theme;
    let Overlay::Palette(palette) = &kite.overlay else {
        return div().into_any_element();
    };
    let rows: Vec<AnyElement> = palette
        .items
        .iter()
        .enumerate()
        .skip(palette.sel.saturating_sub(8))
        .take(9)
        .filter_map(|(ix, id)| {
            let cmd: &Cmd = keys::command(*id)?;
            let keys = keys::bound(&kite.settings, cmd);
            let selected = ix == palette.sel;
            let command_id = *id;
            Some(
                div()
                    .id(("palette-row", ix))
                    .test_support()
                    .aria_label(cmd.title)
                    .aria_selected(selected)
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.overlay = Overlay::None;
                        window.focus(&this.focus, cx);
                        this.run(command_id, crate::app::Src::Click, window, cx);
                    }))
                    .relative()
                    .flex()
                    .items_center()
                    .justify_between()
                    .h(px(36.))
                    .px_3()
                    .rounded(px(RADIUS_SM))
                    .when(selected, |d| d.bg(t.sel))
                    .when(!selected, |d| {
                        d.text_color(t.fg2).hover(|s| s.bg(t.bg3).text_color(t.fg))
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .min_w_0()
                            .child(
                                div()
                                    .text_size(t.px(14.))
                                    .when(selected, |d| d.text_color(t.fg))
                                    .child(cmd.title),
                            )
                            .child(
                                div()
                                    .text_size(t.px(11.))
                                    .text_color(t.fg3)
                                    .child(cmd.group.label()),
                            ),
                    )
                    .when(selected, |d| {
                        // DESIGN › Layout: the selected row glows accent on its left edge.
                        d.child(crate::list::selection_edge(&t))
                    })
                    .child(keys_pills(&t, &keys.join(" ")))
                    .into_any_element(),
            )
        })
        .collect();
    overlay(
        &t,
        window.viewport_size().height * 0.18,
        false,
        div()
            .id("palette")
            .test_support()
            .aria_label("Command palette")
            .w(px(600.))
            .rounded(px(RADIUS))
            .bg(t.bg2)
            .shadow(vec![
                BoxShadow::new(px(0.), px(20.), crate::theme::SHADOW.into()).blur_radius(px(60.)),
            ])
            .p_2()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .flex()
                    .items_center()
                    .h(px(48.))
                    .px_3()
                    .gap_3()
                    .text_size(t.px(14.))
                    .text_color(if palette.query.is_empty() {
                        t.fg3
                    } else {
                        t.fg
                    })
                    .child(
                        Icon::new(IconName::Search)
                            .with_size(px(18.))
                            .text_color(t.fg2),
                    )
                    .child(if palette.query.is_empty() {
                        "Type a command…".to_string()
                    } else {
                        palette.query.clone()
                    }),
            )
            .children(rows)
            .when(palette.items.is_empty(), |d| {
                d.child(
                    div()
                        .px_3()
                        .py_6()
                        .text_color(t.fg2)
                        .child("No matching commands"),
                )
            })
            .child(
                div()
                    .mt_2()
                    .px_3()
                    .py_2()
                    .border_t_1()
                    .border_color(t.line)
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_size(t.px(12.))
                    .text_color(t.fg2)
                    .child("↑ ↓ to navigate")
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(keys_pills(&t, "enter"))
                            .child("Run command"),
                    ),
            ),
    )
    .into_any_element()
}
