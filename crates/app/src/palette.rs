//! Command palette: fuzzy search over the command table, 600px wide, 18% from the top.
use crate::app::{Kite, Overlay};
use crate::keys::{self, Cmd};
use crate::list::{keys_pills, overlay};
use crate::theme::{RADIUS, RADIUS_SM};
use gpui_kit::assets::IconName;
use gpui_kit::component::{Icon, Sizable};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kite_core::model::Settings;
use std::collections::HashSet;

pub struct Palette {
    pub query: String,
    pub sel: usize,
    /// Command ids matching the query, best first.
    pub items: Vec<u8>,
    /// The AI helpers stay out of the list until Settings › AI turns them on.
    pub show_ai: bool,
}

impl Palette {
    pub fn new(settings: &Settings) -> Self {
        let mut palette = Self {
            query: String::new(),
            sel: 0,
            items: Vec::new(),
            show_ai: settings.ai_enabled,
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

/// Subsequence match with bonuses for consecutive runs and word starts.
fn fuzzy(query: &str, text: &str) -> Option<i32> {
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
                        this.run(command_id, window, cx);
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
