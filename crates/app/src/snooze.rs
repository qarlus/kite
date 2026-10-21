//! Snooze picker: presets, a tiny natural-language field and a calendar panel.
use crate::app::{Kite, Overlay};
use crate::list::overlay_dismissable;
use crate::theme::{RADIUS, RADIUS_SM};
use chrono::{DateTime, Datelike as _, Duration, Local, NaiveTime, TimeZone as _, Timelike as _};
use gpui_kit::assets::IconName;
use gpui_kit::component::calendar::{Calendar, CalendarState, Matcher};
use gpui_kit::component::time_field::{TimeField, TimeFieldState};
use gpui_kit::component::{Icon, Sizable, Size};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kite_core::model::Ms;

pub struct Snooze {
    /// What has been typed when it does not match a preset.
    pub buf: String,
    /// Selection across the presets plus the trailing "Pick date & time" row.
    pub sel: usize,
    /// When the picker was opened. Presets and parsing are relative to this.
    pub now: DateTime<Local>,
    pub presets: Vec<(&'static str, Ms)>,
    /// The calendar + time sub-panel; `Some` while it is open.
    pub custom: Option<Custom>,
}

/// The calendar + time sub-panel's focusable states.
pub struct Custom {
    pub calendar: Entity<CalendarState>,
    pub time: Entity<TimeFieldState>,
}

impl Snooze {
    pub fn new(now: DateTime<Local>) -> Self {
        let presets = [
            ("Later today", now + Duration::hours(3)),
            (
                "Tomorrow",
                at(
                    now.date_naive().succ_opt().unwrap_or(now.date_naive()),
                    8,
                    0,
                ),
            ),
            (
                "This weekend",
                at(next_weekday(now, chrono::Weekday::Sat), 8, 0),
            ),
            (
                "Next week",
                at(
                    next_weekday(now + Duration::days(1), chrono::Weekday::Mon),
                    8,
                    0,
                ),
            ),
        ]
        .into_iter()
        .map(|(name, at)| (name, at.timestamp_millis()))
        .collect();
        Self {
            buf: String::new(),
            sel: 0,
            now,
            presets,
            custom: None,
        }
    }

    /// The trailing row's index: one past the last preset.
    pub fn custom_row(&self) -> usize {
        self.presets.len()
    }

    /// Opens the calendar sub-panel, defaulting to the next whole hour.
    /// Past days are disabled in the calendar; past times fail `custom_check`.
    pub fn open_custom(&mut self, window: &mut Window, cx: &mut App) {
        let today = self.now.date_naive();
        let next = self.now + Duration::hours(1);
        let calendar = cx.new(|cx| {
            CalendarState::new(window, cx)
                .disabled_matcher(Matcher::custom(move |date| *date < today))
        });
        calendar.update(cx, |state, cx| {
            state.set_date(next.date_naive(), window, cx)
        });
        let time = cx.new(|cx| TimeFieldState::new(window, cx));
        time.update(cx, |state, cx| {
            state.set_time(
                NaiveTime::from_hms_opt(next.hour(), 0, 0).unwrap_or_default(),
                window,
                cx,
            )
        });
        let focus = time.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        self.custom = Some(Custom { calendar, time });
    }

    /// `Ok(ms)` when the sub-panel holds a complete future time, else why not.
    pub fn custom_check(&self, cx: &App) -> Result<Ms, &'static str> {
        let Some(custom) = &self.custom else {
            return Err("Pick a day and a time");
        };
        let Some(date) = custom.calendar.read(cx).date().start() else {
            return Err("Pick a day");
        };
        let time = custom.time.read(cx).time();
        let when = at(date, time.hour(), time.minute());
        if when <= Local::now() {
            return Err("That time has already passed");
        }
        Ok(when.timestamp_millis())
    }

    /// The time Enter confirms: the typed expression, else the selected preset.
    pub fn pick(&self) -> Option<Ms> {
        let typed = parse_at(&self.buf, self.now);
        if let Some(at) = typed {
            return Some(at.timestamp_millis());
        }
        if self.buf.trim().is_empty() {
            return self.presets.get(self.sel).map(|(_, ms)| *ms);
        }
        None
    }

    pub fn confirm_preset(&self, ix: usize) -> Option<Ms> {
        self.presets.get(ix).map(|(_, ms)| *ms)
    }

    pub fn push(&mut self, c: char) {
        self.buf.push(c);
    }

    pub fn backspace(&mut self) {
        self.buf.pop();
    }

    pub fn move_selection(&mut self, delta: isize) {
        let last = self.custom_row();
        self.sel = (self.sel as isize + delta).clamp(0, last as isize) as usize;
    }
}

fn at(date: chrono::NaiveDate, hour: u32, minute: u32) -> DateTime<Local> {
    let time = NaiveTime::from_hms_opt(hour.min(23), minute.min(59), 0).unwrap_or_default();
    Local
        .from_local_datetime(&date.and_time(time))
        .earliest()
        .unwrap_or_else(|| Local.from_utc_datetime(&date.and_time(time)))
}

/// The next occurrence of `weekday` strictly after `now`.
fn next_weekday(now: DateTime<Local>, weekday: chrono::Weekday) -> chrono::NaiveDate {
    let mut date = now.date_naive();
    for _ in 0..8 {
        date = date.succ_opt().unwrap_or(date);
        if date.weekday() == weekday {
            return date;
        }
    }
    date
}

/// `2h`, `3d`, `45m`, `tomorrow 9am`, `fri`, `fri 9am`, `mon 14:00`, `today 18:00`.
pub fn parse_at(input: &str, now: DateTime<Local>) -> Option<DateTime<Local>> {
    let text = input.trim().to_lowercase();
    if text.is_empty() {
        return None;
    }
    let last = text.chars().last()?;
    if text.len() > 1
        && "mhdw".contains(last)
        && let Ok(n) = text[..text.len() - 1].parse::<i64>()
    {
        let delta = match last {
            'm' => Duration::minutes(n),
            'h' => Duration::hours(n),
            'd' => Duration::days(n),
            _ => Duration::weeks(n),
        };
        if delta != Duration::zero() {
            return Some(now + delta);
        }
    }
    let mut words = text.split_whitespace();
    let day = words.next()?;
    let rest = words.collect::<Vec<_>>().join(" ");
    let (date, default_hour) = match day {
        "today" => (now.date_naive(), (now.hour() + 3).min(22)),
        "tomorrow" => (now.date_naive().succ_opt()?, 8),
        "weekend" => (next_weekday(now, chrono::Weekday::Sat), 8),
        other => {
            let weekday = weekday_of(other)?;
            (next_weekday(now, weekday), 9)
        }
    };
    let (hour, minute) = if rest.is_empty() {
        (default_hour, 0)
    } else {
        clock(&rest)?
    };
    Some(at(date, hour, minute))
}

fn weekday_of(word: &str) -> Option<chrono::Weekday> {
    let word = &word[..word.len().min(3)];
    Some(match word {
        "mon" => chrono::Weekday::Mon,
        "tue" => chrono::Weekday::Tue,
        "wed" => chrono::Weekday::Wed,
        "thu" => chrono::Weekday::Thu,
        "fri" => chrono::Weekday::Fri,
        "sat" => chrono::Weekday::Sat,
        "sun" => chrono::Weekday::Sun,
        _ => return None,
    })
}

/// `9`, `9am`, `9:30`, `14:00`, `9.30pm`.
fn clock(text: &str) -> Option<(u32, u32)> {
    let text = text.trim();
    let (body, pm, am) = if let Some(body) = text.strip_suffix("am") {
        (body, false, true)
    } else if let Some(body) = text.strip_suffix("pm") {
        (body, true, false)
    } else {
        (text, false, false)
    };
    let (hours, minutes) = match body.split_once([':', '.']) {
        Some((h, m)) => (h.trim(), m.trim()),
        None => (body.trim(), "0"),
    };
    let mut hour: u32 = hours.parse().ok()?;
    let minute: u32 = minutes.parse().ok()?;
    if hour > 23 || minute > 59 || ((pm || am) && hour > 12) {
        return None;
    }
    if pm && hour < 12 {
        hour += 12;
    }
    Some((hour, minute))
}

pub fn render(kite: &Kite, window: &Window, cx: &mut Context<Kite>) -> AnyElement {
    let t = kite.theme;
    let Overlay::Snooze(snooze) = &kite.overlay else {
        return div().into_any_element();
    };
    let parsed = if snooze.buf.trim().is_empty() {
        None
    } else {
        parse_at(&snooze.buf, snooze.now)
    };
    // The same picker schedules a send when opened from compose.
    let (title, verb) = if kite.send_later {
        ("Send later", "Send")
    } else {
        ("Snooze", "Snooze until")
    };
    let mut panel = div()
        .id("snooze")
        .test_support()
        .aria_label(title)
        .w(px(340.))
        .rounded(px(RADIUS))
        .bg(t.bg2)
        .border_1()
        .border_color(t.line)
        .p_2()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .id("snooze-input")
                .test_support()
                .aria_label("Type a time")
                .flex()
                .items_center()
                .gap_2()
                .h(px(36.))
                .px_3()
                .rounded(px(RADIUS_SM))
                .border_1()
                .border_color(t.line)
                .text_size(t.px(14.))
                .child(
                    // The caret marks this as a text field even before typing.
                    div().flex_none().w(px(1.)).h(px(16.)).bg(t.accent),
                )
                .text_color(if snooze.buf.is_empty() { t.fg3 } else { t.fg })
                .child(if snooze.buf.is_empty() {
                    "Type a time: 30m, 2h, tomorrow 9am, fri 14:00".to_string()
                } else {
                    snooze.buf.clone()
                }),
        );
    if snooze.custom.is_some() {
        panel = panel.child(custom_panel(kite, snooze, cx));
    } else {
        for (ix, (name, ms)) in snooze.presets.iter().enumerate() {
            let selected = snooze.buf.trim().is_empty() && ix == snooze.sel;
            panel = panel.child(
                div()
                    .id(("preset", ix))
                    .test_support()
                    .aria_label(name.to_string())
                    .aria_selected(selected)
                    .flex()
                    .items_center()
                    .justify_between()
                    .relative()
                    .h(px(32.))
                    .px_3()
                    .rounded(px(RADIUS_SM))
                    .text_size(t.px(13.))
                    .cursor_pointer()
                    .hover(|d| d.bg(t.bg3))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let at = match &this.overlay {
                            Overlay::Snooze(snooze) => snooze.confirm_preset(ix),
                            _ => None,
                        };
                        if let Some(at) = at {
                            this.confirm_snooze(at, window, cx);
                        }
                    }))
                    .when(selected, |d| d.bg(t.sel))
                    .when(selected, |d| d.child(crate::list::selection_edge(&t)))
                    .child(*name)
                    .child(
                        div()
                            .text_size(t.px(12.))
                            .text_color(t.fg3)
                            .child(label(*ms)),
                    ),
            );
        }
        let selected = snooze.buf.trim().is_empty() && snooze.sel == snooze.custom_row();
        panel = panel.child(
            div()
                .id("snooze-custom")
                .test_support()
                .aria_label("Pick date and time")
                .aria_selected(selected)
                .flex()
                .items_center()
                .justify_between()
                .relative()
                .h(px(32.))
                .px_3()
                .rounded(px(RADIUS_SM))
                .text_size(t.px(13.))
                .cursor_pointer()
                .hover(|d| d.bg(t.bg3))
                .on_click(cx.listener(|this, _, window, cx| {
                    if let Overlay::Snooze(snooze) = &mut this.overlay {
                        snooze.open_custom(window, cx);
                    }
                    cx.notify();
                }))
                .when(selected, |d| d.bg(t.sel))
                .when(selected, |d| d.child(crate::list::selection_edge(&t)))
                .child("Pick date & time…")
                .child(
                    Icon::new(IconName::Calendar)
                        .with_size(px(14.))
                        .text_color(t.fg3),
                ),
        );
        if !snooze.buf.trim().is_empty() {
            panel = panel.child(match parsed {
                Some(when) => {
                    let ms = when.timestamp_millis();
                    div()
                        .id("snooze-parsed")
                        .test_support()
                        .aria_label(format!("{verb} {}", label(ms)))
                        .flex()
                        .items_center()
                        .h(px(28.))
                        .px_3()
                        .rounded(px(RADIUS_SM))
                        .text_size(t.px(12.))
                        .text_color(t.fg3)
                        .cursor_pointer()
                        .hover(|d| d.bg(t.bg3))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.confirm_snooze(ms, window, cx);
                        }))
                        .child(format!("{verb} {}", label(ms)))
                }
                None => div()
                    .id("snooze-hint")
                    .test_support()
                    .flex()
                    .items_center()
                    .h(px(28.))
                    .px_3()
                    .text_size(t.px(12.))
                    .text_color(t.fg3)
                    .child("Couldn't read that time".to_string()),
            });
        }
    }
    overlay_dismissable(
        &t,
        window.viewport_size().height * 0.3,
        false,
        panel,
        cx.listener(|this, _, _, cx| this.close_overlay(cx)),
    )
    .into_any_element()
}

/// The calendar + time sub-panel. Confirming needs a complete future time;
/// the button stays disabled with the reason next to it.
fn custom_panel(kite: &Kite, snooze: &Snooze, cx: &mut Context<Kite>) -> impl IntoElement {
    let t = kite.theme;
    let Some(custom) = &snooze.custom else {
        return div().into_any_element();
    };
    let check = snooze.custom_check(cx);
    let confirm = check.ok();
    let action = if kite.send_later {
        "Schedule send"
    } else {
        "Snooze"
    };
    let mut confirm_row = div()
        .id("snooze-confirm")
        .test_support()
        .aria_label(action)
        .flex()
        .items_center()
        .justify_center()
        .h(px(30.))
        .px_3()
        .rounded(px(RADIUS_SM))
        .text_size(t.px(13.));
    if let Some(at) = confirm {
        confirm_row = confirm_row
            .bg(t.accent)
            .text_color(rgb(0xffffff))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, window, cx| {
                this.confirm_snooze(at, window, cx);
            }));
    } else {
        confirm_row = confirm_row.bg(t.bg3).text_color(t.fg3);
    }
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .id("snooze-calendar")
                .test_support()
                .aria_label("Calendar")
                .child(
                    Calendar::new(&custom.calendar)
                        .with_size(Size::Small)
                        .w_full(),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .px_1()
                .child(TimeField::new(&custom.time).with_size(Size::Small))
                .when_some(check.err(), |d, reason| {
                    d.child(
                        div()
                            .id("snooze-custom-error")
                            .test_support()
                            .text_size(t.px(12.))
                            .text_color(t.fg3)
                            .child(reason),
                    )
                }),
        )
        .child(div().flex().justify_end().pt_1().child(confirm_row))
        .into_any_element()
}

fn label(ms: Ms) -> String {
    Local
        .timestamp_millis_opt(ms)
        .single()
        .map(|at| at.format("%a %-H:%M").to_string())
        .unwrap_or_default()
}
