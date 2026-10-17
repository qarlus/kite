//! Command palette: fuzzy search over the command table, 600px wide, 18% from the top.
use crate::app::{Kite, Overlay};
use crate::keys::{self, Cmd};
use crate::list::{keys_pills, overlay};
use gpui_kit::prelude::*;
use gpui_kit::*;

pub struct Palette {
    pub query: String,
    pub sel: usize,
    /// Command ids matching the query, best first.
    pub items: Vec<u8>,
}

impl Default for Palette {
    fn default() -> Self {
        Self {
            query: String::new(),
            sel: 0,
            items: keys::COMMANDS.iter().map(|c| c.id).collect(),
        }
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

pub fn render(kite: &Kite, window: &Window, _cx: &mut Context<Kite>) -> AnyElement {
    let t = kite.theme;
    let Overlay::Palette(palette) = &kite.overlay else {
        return div().into_any_element();
    };
    let rows: Vec<AnyElement> = palette
        .items
        .iter()
        .take(9)
        .enumerate()
        .filter_map(|(ix, id)| {
            let cmd: &Cmd = keys::COMMANDS.iter().find(|c| c.id == *id)?;
            let selected = ix == palette.sel;
            Some(
                div()
                    .id(("palette-row", ix))
                    .test_support()
                    .aria_label(cmd.title)
                    .aria_selected(selected)
                    .relative()
                    .flex()
                    .items_center()
                    .justify_between()
                    .h(px(36.))
                    .px_3()
                    .rounded(px(8.))
                    .when(selected, |d| d.bg(t.sel))
                    .child(
                        div()
                            .text_size(px(14.))
                            .when(selected, |d| d.text_color(t.fg))
                            .child(cmd.title),
                    )
                    .when(selected, |d| {
                        d.child(
                            div()
                                .absolute()
                                .left_0()
                                .top_0()
                                .w(px(2.))
                                .h_full()
                                .rounded_full()
                                .bg(t.accent),
                        )
                    })
                    .child(keys_pills(t, cmd.keys[0]))
                    .into_any_element(),
            )
        })
        .collect();
    overlay(
        t,
        window.viewport_size().height * 0.18,
        false,
        div()
            .id("palette")
            .test_support()
            .aria_label("Command palette")
            .w(px(600.))
            .rounded(px(12.))
            .bg(t.bg2)
            .border_1()
            .border_color(t.line)
            .p_2()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .flex()
                    .items_center()
                    .h(px(36.))
                    .px_3()
                    .text_size(px(14.))
                    .text_color(if palette.query.is_empty() {
                        t.fg3
                    } else {
                        t.fg
                    })
                    .child(if palette.query.is_empty() {
                        "Type a command…".to_string()
                    } else {
                        palette.query.clone()
                    }),
            )
            .children(rows),
    )
    .into_any_element()
}
