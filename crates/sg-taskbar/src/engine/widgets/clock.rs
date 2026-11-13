//! Ported from TachyonTaskbar (MIT), based on YASB (MIT).
//! `yasb.clock.ClockWidget`: a strftime-style time label `{%H:%M}`, a secondary label, a date tooltip,
//! and a calendar popup (`.clock-popup.calendar`) with weekday/month/year/big-day columns + a month grid.

use chrono::format::{Item, StrftimeItems};
use chrono::{Datelike, Local, NaiveDate, Timelike};
use serde::Deserialize;
use serde_yaml::Value;

use super::*;
use crate::engine::config::{options, Callbacks, PopupOptions};

#[derive(Deserialize, Clone)]
#[serde(default)]
struct CalendarConfig {
    blur: bool,
    round_corners: bool,
    round_corners_type: String,
    border_color: String,
    alignment: String,
    direction: String,
    offset_top: i32,
    offset_left: i32,
    show_holidays: bool,
    show_week_numbers: bool,
    show_years: bool,
    extended: bool,
}

impl Default for CalendarConfig {
    fn default() -> Self {
        CalendarConfig {
            blur: true,
            round_corners: true,
            round_corners_type: "normal".into(),
            border_color: "System".into(),
            alignment: "right".into(),
            direction: "down".into(),
            offset_top: 6,
            offset_left: 0,
            show_holidays: false,
            show_week_numbers: false,
            show_years: false,
            extended: false,
        }
    }
}

#[derive(Deserialize, Clone)]
#[serde(default)]
struct AlarmIcons {
    enabled: String,
    disabled: String,
    snooze: String,
}

impl Default for AlarmIcons {
    fn default() -> Self {
        AlarmIcons { enabled: "\u{f0f3}".into(), disabled: "\u{f0a2}".into(), snooze: "\u{f1f6}".into() }
    }
}

#[derive(Deserialize, Clone)]
#[serde(default)]
struct ClockConfig {
    label: String,
    label_alt: String,
    class_name: String,
    update_interval: u32,
    locale: String,
    tooltip: bool,
    timezones: Vec<String>,
    icons: std::collections::HashMap<String, String>,
    alarm_icons: AlarmIcons,
    calendar: CalendarConfig,
    callbacks: Callbacks,
}

impl Default for ClockConfig {
    fn default() -> Self {
        ClockConfig {
            label: "\u{f017} {%H:%M:%S}".into(),
            label_alt: "\u{f017} {%d-%m-%y %H:%M:%S}".into(),
            class_name: String::new(),
            update_interval: 1000,
            locale: String::new(),
            tooltip: true,
            timezones: Vec::new(),
            icons: Default::default(),
            alarm_icons: AlarmIcons::default(),
            calendar: CalendarConfig::default(),
            callbacks: Callbacks::default(),
        }
    }
}

const CAL_PREV: u32 = 2;
const CAL_DAY: u32 = 100;

pub struct Clock {
    base: Base,
    cfg: ClockConfig,
    /// the month being viewed on the calendar and the selected day
    page: (i32, u32),
    selected: NaiveDate,
}

/// Python-style strftime; an invalid format is returned verbatim, as in YASB.
pub fn strftime(now: &chrono::DateTime<Local>, fmt: &str, locale: &str) -> Option<String> {
    let fmt = fmt.replace("%#", "%-");
    let loc = if locale.is_empty() { None } else { chrono::Locale::try_from(locale.replace('-', "_").as_str()).ok() };
    let items: Vec<Item> = match loc {
        Some(l) => StrftimeItems::new_with_locale(&fmt, l).collect(),
        None => StrftimeItems::new(&fmt).collect(),
    };
    if items.iter().any(|i| matches!(i, Item::Error)) {
        return None;
    }
    use std::fmt::Write;
    let mut s = String::new();
    match loc {
        Some(l) => write!(s, "{}", now.format_localized_with_items(items.into_iter(), l)).ok()?,
        None => write!(s, "{}", now.format_with_items(items.into_iter())).ok()?,
    }
    Some(s)
}

/// Weekday/month names per the Windows locale (like QLocale.system()).
mod names {
    use windows::core::PCWSTR;
    use windows::Win32::Globalization::*;

    fn info(locale: &str, lctype: u32) -> String {
        let loc: Vec<u16> = locale.encode_utf16().chain(Some(0)).collect();
        let lp = if locale.is_empty() { PCWSTR::null() } else { PCWSTR(loc.as_ptr()) };
        let mut buf = [0u16; 128];
        let n = unsafe { GetLocaleInfoEx(lp, lctype, Some(&mut buf)) };
        if n <= 1 {
            return String::new();
        }
        String::from_utf16_lossy(&buf[..n as usize - 1])
    }

    /// weekday: 0 = Monday ... 6 = Sunday
    pub fn day(locale: &str, wd0: u32, short: bool) -> String {
        let base = if short { LOCALE_SABBREVDAYNAME1 } else { LOCALE_SDAYNAME1 };
        info(locale, base + wd0)
    }

    pub fn month(locale: &str, m1: u32) -> String {
        info(locale, LOCALE_SMONTHNAME1 + (m1 - 1))
    }

    /// 0 = Monday ... 6 = Sunday
    pub fn first_day(locale: &str) -> u32 {
        info(locale, LOCALE_IFIRSTDAYOFWEEK).parse().unwrap_or(0)
    }
}

impl Clock {
    pub fn new(name: &str, opts: &Value) -> Clock {
        let cfg: ClockConfig = options(name, opts);
        let cb = cfg.callbacks.resolve("toggle_calendar", "next_timezone", "toggle_label");
        let mut base = Base::new(name, &format!("clock-widget {}", cfg.class_name), &cfg.label, Some(&cfg.label_alt), cb);
        let now = Local::now();
        if cfg.tooltip {
            base.tooltip = Some(Self::tooltip_text(&now, &cfg.locale));
        }
        if !cfg.timezones.is_empty() {
            crate::engine::log::warn("Clock: múi giờ riêng (timezones) chưa hỗ trợ, dùng giờ máy.");
        }
        let today = now.date_naive();
        Clock { base, cfg, page: (today.year(), today.month()), selected: today }
    }

    fn tooltip_text(now: &chrono::DateTime<Local>, locale: &str) -> String {
        let date = strftime(now, "%A, %d %B %Y", locale).unwrap_or_default();
        let day = strftime(now, "%a", locale).unwrap_or_default();
        let time = strftime(now, "%H:%M", locale).unwrap_or_default();
        format!("{date}\n\n{day} {time} (Local time)")
    }

    fn icon_for_hour(&self, hour: u32) -> String {
        if let Some(i) = self.cfg.icons.get(&format!("clock_{hour:02}")) {
            return i.clone();
        }
        if (13..=23).contains(&hour)
            && let Some(i) = self.cfg.icons.get(&format!("clock_{:02}", hour - 12)) {
                return i.clone();
            }
        String::new()
    }

    fn fill(&self, part: &str, now: &chrono::DateTime<Local>) -> String {
        let mut p = part.replace("{icon}", &self.icon_for_hour(now.hour())).replace("{alarm}", "");
        // `re.search(r"\{(.*)}")`: from the first { to the last } is the format string
        if let (Some(a), Some(b)) = (p.find('{'), p.rfind('}'))
            && b > a
                && let Some(t) = strftime(now, &p[a + 1..b], &self.cfg.locale) {
                    p = format!("{}{}{}", &p[..a], t, &p[b + 1..]);
                }
        p
    }

    fn spec(&self) -> PopupSpec {
        let c = &self.cfg.calendar;
        PopupSpec {
            kind: 1,
            class: "clock-popup calendar".into(),
            opts: PopupOptions {
                blur: c.blur,
                round_corners: c.round_corners,
                round_corners_type: c.round_corners_type.clone(),
                border_color: c.border_color.clone(),
                alignment: c.alignment.clone(),
                direction: c.direction.clone(),
                offset_top: c.offset_top,
                offset_left: c.offset_left,
            },
            // The window's default QHBoxLayout: 11px margins (top-level), 6px spacing
            margins: Edges { top: 11.0, right: 11.0, bottom: 11.0, left: 11.0 },
            dir: Dir::Row,
        }
    }

    fn shift_month(&mut self, delta: i32) {
        let (mut y, mut m) = (self.page.0, self.page.1 as i32 + delta);
        while m < 1 {
            m += 12;
            y -= 1;
        }
        while m > 12 {
            m -= 12;
            y += 1;
        }
        self.page = (y, m as u32);
        // keep the selected day within the new month (like update_month_label)
        let dim = days_in_month(y, m as u32);
        let d = self.selected.day().min(dim);
        self.selected = NaiveDate::from_ymd_opt(y, m as u32, d).unwrap_or(self.selected);
    }

    fn calendar_view(&self, ui: &mut Ui) {
        let loc = self.locale_name();
        let first = names::first_day(&loc);
        let (y, m) = self.page;
        let first_of_month = NaiveDate::from_ymd_opt(y, m, 1).unwrap();
        let wd = first_of_month.weekday().num_days_from_monday();
        let offset = (wd + 7 - first) % 7;
        // Qt always leaves at least one day of the previous month in the first row when the month starts exactly on the first day of the week
        let offset = if offset == 0 { 7 } else { offset };
        let start = first_of_month - chrono::Duration::days(offset as i64);
        let today = Local::now().date_naive();
        let (cw, ch) = (32.0, 24.0);
        let mut table = Node::new("QTableView", "calendar-table");
        table.dir = Dir::Col;
        table.vpolicy = Policy::Fixed;
        table.hpolicy = Policy::Fixed;
        table.on_wheel = Some(CAL_PREV);
        table.item_align = Some((Align::Center, Align::Center));
        ui.with(table, |ui| {
            ui.layout(Dir::Row, |ui| {
                for i in 0..7 {
                    let d = (first + i) % 7;
                    // the header row is also a table cell, so it receives the `.calendar-table::item` color
                    let mut c = Node::new("QLabel", "");
                    c.sub_element = Some("item");
                    c.content = Content::Text(names::day(&loc, d, true));
                    c.halign = Align::Center;
                    c.fixed = (Some(cw), Some(ch));
                    ui.add(c);
                }
            });
            for row in 0..6 {
                ui.layout(Dir::Row, |ui| {
                    for col in 0..7 {
                        let date = start + chrono::Duration::days((row * 7 + col) as i64);
                        let mut c = Node::new("QLabel", "");
                        c.sub_element = Some("item");
                        c.content = Content::Text(date.day().to_string());
                        c.halign = Align::Center;
                        c.fixed = (Some(cw), Some(ch));
                        if date == self.selected {
                            c.states |= crate::engine::css::state::SELECTED;
                        }
                        c.on_click = Some(CAL_DAY + date.num_days_from_ce() as u32 % 100_000);
                        let _ = today;
                        ui.add(c);
                    }
                });
            }
        });
    }

    fn locale_name(&self) -> String {
        self.cfg.locale.replace('_', "-")
    }
}

pub fn days_in_month(y: i32, m: u32) -> u32 {
    let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
    NaiveDate::from_ymd_opt(ny, nm, 1).unwrap().pred_opt().unwrap().day()
}

impl Widget for Clock {
    fn view(&self, ui: &mut Ui) {
        let now = Local::now();
        let hour = format!("clock_{:02}", now.hour());
        let alt_cls = if self.base.show_alt { " alt" } else { "" };
        let show_alt = self.base.show_alt;
        self.base.shell(ui, |ui| {
            for (parts, is_alt) in [(&self.base.label, false), (&self.base.label_alt, true)] {
                for p in parts.iter() {
                    let active = is_alt == show_alt;
                    let (class, text, hidden) = if p.icon {
                        if p.text == "{icon}" {
                            (format!("icon{alt_cls} {hour}"), self.icon_for_hour(now.hour()), !active)
                        } else if p.text == "{alarm}" {
                            (p.class.clone(), String::new(), true)
                        } else {
                            (p.class.clone(), p.text.clone(), !active)
                        }
                    } else if active {
                        (format!("label{alt_cls} {hour}"), self.fill(&p.text, &now), false)
                    } else {
                        (p.class.clone(), p.text.clone(), true)
                    };
                    let id = ui.label_centered(&class, text);
                    ui.node(id).hidden = hidden;
                }
            }
        });
    }

    fn popup_view(&self, _kind: u32, ui: &mut Ui) {
        let loc = self.locale_name();
        let sel = self.selected;
        let c = &self.cfg.calendar;
        // left column: weekday, month, year, big day, week
        let mut left = Node::new("layout", "");
        left.dir = Dir::Col;
        left.pack = Some((Align::Center, Align::Center));
        ui.with(left, |ui| {
            let wd0 = sel.weekday().num_days_from_monday();
            ui.label_centered("day-label", names::day(&loc, wd0, false));
            ui.label_centered("month-label", names::month(&loc, sel.month()));
            if c.show_years {
                ui.label_centered("year-label", sel.year().to_string());
            }
            ui.label_centered("date-label", sel.day().to_string());
            if c.show_week_numbers {
                ui.label_centered("week-label", format!("Week {}", sel.iso_week().week()));
            }
            if c.show_holidays {
                ui.label_centered("holiday-label", "");
            }
        });
        ui.spacing(6.0);
        self.calendar_view(ui);
    }

    fn event(&mut self, ev: Ev, cx: &mut Cx) {
        match ev {
            Ev::Init | Ev::Timer => {
                let now = Local::now();
                if self.cfg.tooltip {
                    self.base.tooltip = Some(Self::tooltip_text(&now, &self.cfg.locale));
                }
                cx.redraw();
            }
            Ev::Click { id: BASE, btn } => {
                let (name, args) = split_callback(self.base.callback_for(btn));
                if base_callback(&name, &args) {
                    return;
                }
                match name.as_str() {
                    "toggle_calendar" => {
                        let today = Local::now().date_naive();
                        self.selected = today;
                        self.page = (today.year(), today.month());
                        cx.open_popup(self.spec());
                    }
                    "toggle_label" => {
                        self.base.show_alt = !self.base.show_alt;
                        cx.redraw();
                    }
                    "update_label" | "next_timezone" => cx.redraw(),
                    "context_menu" | "toggle_timer" | "toggle_alarm" => {
                        crate::engine::log::debug("Clock: báo thức/hẹn giờ chưa có trong bản Rust");
                    }
                    other => crate::engine::log::warn(&format!("Clock: callback không rõ '{other}'")),
                }
            }
            Ev::Click { id, .. } if id >= CAL_DAY => {
                // recover the date from the code (days since CE, mod 100000)
                let (y, m) = self.page;
                let base = NaiveDate::from_ymd_opt(y, m, 1).unwrap() - chrono::Duration::days(14);
                for k in 0..60 {
                    let d = base + chrono::Duration::days(k);
                    if CAL_DAY + d.num_days_from_ce() as u32 % 100_000 == id {
                        self.selected = d;
                        if d.month() != m {
                            self.page = (d.year(), d.month());
                        }
                        break;
                    }
                }
                cx.redraw();
            }
            Ev::Wheel { id: CAL_PREV, delta } => {
                self.shift_month(if delta > 0 { -1 } else { 1 });
                cx.redraw();
            }
            _ => {}
        }
    }

    fn interval(&self) -> u32 {
        self.cfg.update_interval.max(100)
    }
}
