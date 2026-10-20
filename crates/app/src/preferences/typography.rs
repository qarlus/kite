//! Typography: base size, font family, text colour and a live preview.
use super::{button, card, heading, row};
use crate::app::Kite;
use crate::theme::{self, RADIUS, RADIUS_SM, Theme};
use gpui_kit::assets::IconName;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::searchable_list::{SearchableListDelegate, SearchableListItem};
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::{ActiveTheme, Icon, IndexPath, Sizable};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kite_core::model::Settings;
use std::time::Duration;

/// The faces offered first, in preference order. Only the ones installed are listed.
const RECOMMENDED: [&str; 21] = [
    "Segoe UI Variable",
    "Segoe UI",
    "Inter",
    "Aptos",
    "Calibri",
    "Arial",
    "Helvetica Neue",
    "SF Pro Text",
    "Roboto",
    "Open Sans",
    "Source Sans 3",
    "IBM Plex Sans",
    "Georgia",
    "Cambria",
    "Charter",
    "Iowan Old Style",
    "Source Serif 4",
    "Cascadia Code",
    "Consolas",
    "JetBrains Mono",
    "SF Mono",
];

pub struct State {
    pub font: Entity<SelectState<FontList>>,
    pub color: Entity<InputState>,
}

impl State {
    pub fn new(settings: &Settings, window: &mut Window, cx: &mut Context<Kite>) -> Self {
        let family = settings.font_family.clone();
        // The installed fonts are read once here: a frame must never touch the platform.
        let font = cx.new(|cx| {
            let list = FontList::new(settings, cx);
            let selected = list.position(&family);
            SelectState::new(list, selected, window, cx).searchable(true)
        });
        // Picking a face applies it at once, the way the size stepper does.
        cx.subscribe_in(
            &font,
            window,
            |kite, _, event: &SelectEvent<FontList>, window, cx| {
                let SelectEvent::Confirm(Some(family)) = event else {
                    return;
                };
                if *family == kite.settings.font_family {
                    return;
                }
                let mut settings = kite.settings.clone();
                settings.font_family = family.clone();
                kite.save_preferences(settings, window, cx);
            },
        )
        .detach();
        let color = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Theme default")
                .default_value(settings.font_color.clone().unwrap_or_default())
        });
        // The swatch mirrors what is typed, so the page repaints while the field changes.
        cx.observe(&color, |_, _, cx| cx.notify()).detach();
        Self { font, color }
    }
}

/// One row of the picker. An empty family is the platform's own UI font.
#[derive(Clone)]
pub struct FontChoice {
    family: String,
    title: SharedString,
}

impl FontChoice {
    fn new(family: &str) -> Self {
        Self {
            family: family.to_string(),
            title: if family.is_empty() {
                "System default".into()
            } else {
                family.into()
            },
        }
    }

    /// The face the row draws itself in, so the list previews every choice.
    fn face(&self) -> SharedString {
        if self.family.is_empty() {
            theme::FONT.into()
        } else {
            SharedString::from(self.family.clone())
        }
    }
}

impl SearchableListItem for FontChoice {
    type Value = String;

    fn title(&self) -> SharedString {
        self.title.clone()
    }

    fn value(&self) -> &String {
        &self.family
    }

    fn display_title(&self) -> Option<AnyElement> {
        Some(
            div()
                .font_family(self.face())
                .child(self.title.clone())
                .into_any_element(),
        )
    }

    fn render(&self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div()
            .id((ElementId::from("font-choice"), self.family.clone()))
            .test_support()
            .aria_label(self.title.clone())
            .font_family(self.face())
            .child(self.title.clone())
    }
}

/// The picker's rows, cached at startup: System default, the saved family when it is
/// not installed, the installed recommended faces, then every other installed family.
/// `matched` mirrors the query so a keystroke only re-filters indices instead of
/// re-reading the system fonts.
pub struct FontList {
    groups: Vec<(SharedString, Vec<FontChoice>)>,
    /// `(group, rows)` of every section the current query keeps, in display order.
    matched: Vec<(usize, Vec<usize>)>,
}

impl FontList {
    fn new(settings: &Settings, cx: &App) -> Self {
        let installed: Vec<String> = cx
            .text_system()
            .all_font_names()
            .into_iter()
            // Hidden and vertical ("@face") aliases are not choosable families.
            .filter(|name| !name.is_empty() && !name.starts_with(['@', '.']))
            .collect();
        let has = |wanted: &str| {
            installed
                .iter()
                .any(|name| name.eq_ignore_ascii_case(wanted))
        };

        let recommended: Vec<FontChoice> = RECOMMENDED
            .into_iter()
            .filter(|name| has(name))
            .map(FontChoice::new)
            .collect();
        let others: Vec<FontChoice> = installed
            .iter()
            .filter(|name| {
                !RECOMMENDED
                    .iter()
                    .any(|face| face.eq_ignore_ascii_case(name))
            })
            .map(|name| FontChoice::new(name))
            .collect();

        // Every group carries a title: the list measures section 0's header once and
        // reuses that height for every section, so no group may start headerless.
        let mut groups: Vec<(SharedString, Vec<FontChoice>)> =
            vec![("Default".into(), vec![FontChoice::new("")])];
        // A saved family that is no longer installed keeps its own row, never a crash.
        let saved = settings.font_family.trim();
        if !saved.is_empty() && !has(saved) {
            groups.push(("Current".into(), vec![FontChoice::new(saved)]));
        }
        groups.push(("Recommended".into(), recommended));
        groups.push(("All fonts".into(), others));

        let matched = groups
            .iter()
            .enumerate()
            .filter(|(_, (_, items))| !items.is_empty())
            .map(|(group, (_, items))| (group, (0..items.len()).collect()))
            .collect();
        Self { groups, matched }
    }
}

impl SearchableListDelegate for FontList {
    type Item = FontChoice;

    fn sections_count(&self, _: &App) -> usize {
        self.matched.len()
    }

    fn items_count(&self, section: usize) -> usize {
        self.matched.get(section).map_or(0, |(_, rows)| rows.len())
    }

    fn item(&self, ix: IndexPath) -> Option<&Self::Item> {
        let (group, rows) = self.matched.get(ix.section)?;
        self.groups.get(*group)?.1.get(*rows.get(ix.row)?)
    }

    fn position<V>(&self, family: &V) -> Option<IndexPath>
    where
        Self::Item: SearchableListItem<Value = V>,
        V: PartialEq,
    {
        self.matched
            .iter()
            .enumerate()
            .find_map(|(section, (group, rows))| {
                let row = rows
                    .iter()
                    .position(|row| self.groups[*group].1[*row].value() == family)?;
                Some(IndexPath::default().section(section).row(row))
            })
    }

    fn perform_search(&mut self, query: &str, _: &mut Window, _: &mut App) -> Task<()> {
        let query = query.trim().to_lowercase();
        self.matched = self
            .groups
            .iter()
            .enumerate()
            .filter_map(|(group, (title, items))| {
                let rows: Vec<usize> = if query.is_empty() || title.to_lowercase().contains(&query)
                {
                    (0..items.len()).collect()
                } else {
                    items
                        .iter()
                        .enumerate()
                        .filter(|(_, item)| item.matches(&query))
                        .map(|(row, _)| row)
                        .collect()
                };
                (!rows.is_empty()).then_some((group, rows))
            })
            .collect();
        Task::ready(())
    }

    fn render_section_header(
        &self,
        section: usize,
        _: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        let (group, _) = self.matched.get(section)?;
        let title = self.groups.get(*group)?.0.clone();
        // The list measures section 0's header once and gives every section's header row
        // that height. A missing or auto-sized header would therefore be a zero-height
        // row, painting the next group's label on top of its first item, so this is a
        // fixed-height block for every group, filtered or not.
        Some(
            div()
                .id(("font-group", section))
                .test_support()
                .aria_label(title.to_string())
                .w_full()
                .h(px(24.))
                .flex()
                .items_center()
                .px_1()
                .pt_1()
                .text_size(px(11.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(cx.theme().muted_foreground.opacity(0.8))
                .child(SharedString::from(title.to_uppercase()))
                .into_any_element(),
        )
    }
}

/// A quiet square control, used for the text-size stepper. `button` is the labelled variant.
fn step(
    id: &'static str,
    label: &'static str,
    icon: IconName,
    t: Theme,
) -> gpui_kit::base::ObservedElement<Stateful<Div>> {
    div()
        .id(id)
        .test_support()
        .aria_label(label)
        .size(px(30.))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .text_color(t.fg2)
        .hover(|s| s.bg(t.bg3).text_color(t.fg))
        .child(Icon::new(icon).with_size(px(14.)))
}

/// The live typography preview: real mail proportions at the chosen font, size and colour.
fn preview(kite: &Kite, t: Theme) -> AnyElement {
    let font = theme::font(&kite.settings).to_string();
    let size = kite.settings.font_size;
    let body = size as f32;
    let meta = (body - 2.).max(11.);
    let subject = body + 3.;

    div()
        .id("typography-preview")
        .test_support()
        .aria_label(format!("{font} / {size} px"))
        .w_full()
        .rounded(px(RADIUS))
        .border_1()
        .border_color(t.line)
        .bg(t.bg2)
        .overflow_hidden()
        .flex()
        .flex_col()
        .font_family(font.clone())
        .child(
            div()
                .px_4()
                .pt_4()
                .flex()
                .items_center()
                .gap_3()
                .child(
                    div()
                        .size(px(28.))
                        .flex_shrink_0()
                        .rounded_full()
                        .bg(Theme::alpha(t.accent, 0.18))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(meta))
                        .font_weight(FontWeight(650.))
                        .text_color(t.accent)
                        .child("AC"),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .truncate()
                                .text_size(px(meta))
                                .font_weight(FontWeight::MEDIUM)
                                .child("Alice Chen"),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_size(px(meta))
                                .text_color(t.fg3)
                                .child("to me"),
                        ),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .text_size(px(meta))
                        .text_color(t.fg3)
                        .child("9:41"),
                ),
        )
        .child(
            div()
                .px_4()
                .pt_3()
                .pb_4()
                .max_w(px(520.))
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .text_size(px(subject))
                        .font_weight(FontWeight(650.))
                        .text_color(t.fg)
                        .child("A few thoughts on the next chapter"),
                )
                .child(
                    div()
                        .text_size(px(body))
                        .text_color(t.fg2)
                        .child("The draft reads well to me. I moved the schedule to Thursday and left the budget section for you — let me know if you would rather see the numbers in the appendix."),
                ),
        )
        .child(
            div()
                .border_t_1()
                .border_color(t.line)
                .px_4()
                .py_2()
                .flex()
                .items_center()
                .justify_between()
                .gap_4()
                .text_size(px(meta))
                .text_color(t.fg3)
                .child(div().flex_1().min_w_0().truncate().child(font))
                .child(
                    div()
                        .flex_shrink_0()
                        .child(format!("{size} px · {}", theme::hex(t.fg))),
                ),
        )
        // Motion is decorative: the resolved type settles in when it changes.
        .with_animation(
            ("typography-preview", kite.appearance_version),
            Animation::new(Duration::from_millis(140)).with_easing(ease_out_quint()),
            |d, delta| d.opacity(0.7 + 0.3 * delta),
        )
        .into_any_element()
}

pub fn render(kite: &Kite, _window: &Window, cx: &mut Context<Kite>) -> AnyElement {
    let Some(page) = &kite.preferences else {
        return div().into_any_element();
    };
    let t = kite.theme;
    let size = kite.settings.font_size;

    let size_control = div()
        .flex()
        .items_center()
        .rounded(px(RADIUS_SM))
        .border_1()
        .border_color(t.line)
        .bg(t.bg2)
        .overflow_hidden()
        .child(
            step("font-smaller", "Smaller text", IconName::Minus, t).on_click(cx.listener(
                |this, _, window, cx| {
                    let mut settings = this.settings.clone();
                    settings.font_size = settings.font_size.saturating_sub(1).clamp(12, 18);
                    this.save_preferences(settings, window, cx);
                },
            )),
        )
        .child(
            div()
                .id("font-size-value")
                .test_support()
                .aria_label(format!("{size} px"))
                .w(px(58.))
                .h(px(30.))
                .flex()
                .items_center()
                .justify_center()
                .border_l_1()
                .border_r_1()
                .border_color(t.line)
                .text_size(t.px(13.))
                .font_weight(FontWeight::MEDIUM)
                .child(format!("{size} px")),
        )
        .child(
            step("font-larger", "Larger text", IconName::Plus, t).on_click(cx.listener(
                |this, _, window, cx| {
                    let mut settings = this.settings.clone();
                    settings.font_size = (settings.font_size + 1).clamp(12, 18);
                    this.save_preferences(settings, window, cx);
                },
            )),
        );

    let font_control = div().w(px(220.)).h(px(32.)).child(
        Select::new(&page.typography.font)
            .id("font-family")
            .accessibility_label("Font family")
            .placeholder("System default")
            .search_placeholder("Search fonts")
            .menu_width(px(260.))
            .menu_max_h(px(320.)),
    );

    let typed = page.typography.color.read(cx).value().trim().to_string();
    let parsed = theme::color(&typed).ok();
    let color_control = div()
        .flex()
        .items_center()
        .gap_2()
        .child(
            div().w(px(200.)).child(
                Input::new(&page.typography.color)
                    .id("font-color")
                    .aria_label("Text colour"),
            ),
        )
        .child(
            div()
                .id("font-color-swatch")
                .test_support()
                .aria_label(match parsed {
                    Some(color) => format!("Text colour {}", theme::hex(color)),
                    None => "Text colour not set".to_string(),
                })
                .size(px(24.))
                .flex_shrink_0()
                .rounded(px(RADIUS_SM))
                .border_1()
                .border_color(t.line)
                .bg(parsed.unwrap_or(t.bg3)),
        );

    let apply = div()
        .flex()
        .items_center()
        .justify_end()
        .gap_2()
        .child(button("reset-typography", "Reset", t).on_click(cx.listener(
            |this, _, window, cx| {
                let mut settings = this.settings.clone();
                settings.font_family.clear();
                settings.font_color = None;
                settings.font_size = 14;
                this.save_preferences(settings, window, cx);
                if let Some(page) = &this.preferences {
                    page.typography.font.update(cx, |select, cx| {
                        select.set_selected_value(&String::new(), window, cx)
                    });
                    page.typography
                        .color
                        .update(cx, |input, cx| input.set_value("", window, cx));
                }
            },
        )))
        .child(
            button("apply-typography", "Apply typography", t)
                .bg(Theme::alpha(t.accent, 0.18))
                .border_color(Theme::alpha(t.accent, 0.55))
                .font_weight(FontWeight(650.))
                .on_click(cx.listener(|this, _, window, cx| {
                    let Some(page) = &this.preferences else {
                        return;
                    };
                    let mut settings = this.settings.clone();
                    settings.font_family = page
                        .typography
                        .font
                        .read(cx)
                        .selected_value()
                        .cloned()
                        .unwrap_or_default();
                    let color = page.typography.color.read(cx).value().trim().to_string();
                    settings.font_color = (!color.is_empty()).then_some(color);
                    this.save_preferences(settings, window, cx);
                })),
        );

    div()
        .w_full()
        .max_w(px(720.))
        .pb_2()
        .flex()
        .flex_col()
        .gap_8()
        .child(
            div()
                .flex()
                .flex_col()
                .child(heading(
                    t,
                    "Typography",
                    "Any installed font, 12–18 px. Mail and the interface move together.",
                ))
                .child(card(
                    t,
                    vec![
                        row(
                            t,
                            "Text size",
                            "List rows, mail and the interface resize together.",
                            size_control,
                        ),
                        row(
                            t,
                            "Font family",
                            "A font installed on this computer, or the system default.",
                            font_control,
                        ),
                        row(
                            t,
                            "Text colour",
                            "Optional hex. Applied only when it keeps 4.5:1 contrast.",
                            color_control,
                        ),
                        apply.into_any_element(),
                    ],
                ))
                .child(div().pt_3().child(preview(kite, t))),
        )
        .into_any_element()
}
