//! Snooze picker: four presets and a tiny natural-language parser.
use crate::app::{Kite, Overlay};
use crate::list::overlay;
use crate::theme::{RADIUS, RADIUS_SM};
use chrono::{DateTime, Datelike as _, Duration, Local, NaiveTime, TimeZone as _, Timelike as _};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kite_core::model::Ms;

pub struct Snooze {
    /// What has been typed when it does not match a preset.
    pub buf: String,
    pub sel: usize,
    /// When the picker was opened. Presets and parsing are relative to this.
    pub now: DateTime<Local>,
    pub presets: Vec<(&'static str, Ms)>,
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
        }
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
        if self.presets.is_empty() {
            return;
        }
        let last = self.presets.len() - 1;
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
                .flex()
                .items_center()
                .gap_2()
                .h(px(36.))
                .px_3()
                .text_size(t.px(14.))
                .text_color(if snooze.buf.is_empty() { t.fg3 } else { t.fg })
                .child(if snooze.buf.is_empty() {
                    "Later today, tomorrow 9am, fri 14:00…".to_string()
                } else {
                    snooze.buf.clone()
                }),
        );
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
    if let Some(when) = parsed {
        panel = panel.child(
            div()
                .px_3()
                .py_1()
                .text_size(t.px(12.))
                .text_color(t.fg3)
                .child(format!("{verb} {}", label(when.timestamp_millis()))),
        );
    }
    overlay(&t, window.viewport_size().height * 0.3, false, panel).into_any_element()
}

fn label(ms: Ms) -> String {
    Local
        .timestamp_millis_opt(ms)
        .single()
        .map(|at| at.format("%a %-H:%M").to_string())
        .unwrap_or_default()
}
