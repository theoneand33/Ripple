//! iCalendar text: reading and writing the to-dos and events Ripple keeps as one line of text
//! each (`task::Task`, and `YYYY-MM-DD [HH:MM ]title` for events), and editing a calendar
//! object in place so what Ripple doesn't show survives.

use crate::date::days_from_civil;
use crate::task::{Every, Task};

/// The calendar object with its first `comp`'s properties named in `drop` taken out and `add`
/// put in. Alarms and everything else stay as they were.
fn rewrite(ics: &str, comp: &str, drop: &[&str], add: &[String]) -> String {
    let (begin, end) = (format!("BEGIN:{comp}"), format!("END:{comp}"));
    let (mut out, mut inside, mut nested, mut done) = (String::new(), false, 0, false);
    for line in unfold(ics).lines() {
        let upper = line.to_ascii_uppercase();
        if !done && upper == begin { inside = true }
        else if inside && upper == end {
            for a in add { out += a; out += "\r\n" }
            (inside, done) = (false, true);
        }
        else if inside && upper.starts_with("BEGIN:") { nested += 1 }
        // A stray END closes nothing, as in `props`.
        else if inside && upper.starts_with("END:") { nested = (nested - 1).max(0) }
        else if inside && nested == 0 && drop.iter().any(|d| upper.split([':', ';']).next() == Some(d)) { continue }
        out += line;
        out += "\r\n";
    }
    out
}

/// The first `comp`'s own properties, outside its alarms and other nested blocks:
/// (name in upper case, parameters, value).
fn props(ics: &str, comp: &str) -> Vec<(String, String, String)> {
    let (begin, end) = (format!("BEGIN:{comp}"), format!("END:{comp}"));
    let (mut inside, mut nested, mut out) = (false, 0, Vec::new());
    for line in unfold(ics).lines() {
        let upper = line.to_ascii_uppercase();
        if !inside { inside = upper == begin; continue }
        if upper == end { break }
        if upper.starts_with("BEGIN:") { nested += 1; continue }
        // A stray END closes nothing, so it doesn't hide the properties after it.
        if upper.starts_with("END:") { nested = (nested - 1).max(0); continue }
        let Some((head, value)) = line.split_once(':').filter(|_| nested == 0) else { continue };
        let (name, params) = head.split_once(';').unwrap_or((head, ""));
        out.push((name.to_ascii_uppercase(), params.to_string(), value.to_string()));
    }
    out
}

/// A property of the first `comp`, outside its alarms: (parameters, value).
fn property(ics: &str, comp: &str, name: &str) -> Option<(String, String)> {
    props(ics, comp).into_iter().find(|p| p.0.eq_ignore_ascii_case(name)).map(|(_, params, value)| (params, value))
}

/// Minutes since 1970 on this clock, as `ics_time` reads a value.
fn minutes(((y, m, d), time): ((i32, u32, u32), Option<(u32, u32)>)) -> i64 {
    let (h, min) = time.unwrap_or((0, 0));
    days_from_civil(y, m, d) * 1440 + i64::from(h * 60 + min)
}

/// Minutes since 1970 on this clock as UTC minutes, at that moment's offset.
fn to_utc(local: i64) -> i64 {
    let offset = |m: i64| i64::from(rune::time::LocalTime::at(m * 60_000).offset) / 60;
    local - offset(local - offset(local))
}

/// Today on this clock, for the `today` and `tomorrow` a hand-edited task may hold.
fn today() -> (i32, u32, u32) {
    let t = rune::time::LocalTime::at(crate::date::now_ms());
    (t.year, t.month, t.day)
}

/// A DATE (`all_day`) or local DATE-TIME value at `at` minutes since 1970.
fn ics_value(at: i64, all_day: bool) -> String {
    let t = rune::time::LocalTime::at_offset(at * 60_000, 0);
    if all_day { format!("{:04}{:02}{:02}", t.year, t.month, t.day) } else { format!("{:04}{:02}{:02}T{:02}{:02}00", t.year, t.month, t.day, t.hour, t.minute) }
}

/// An event's object with its text's edit made: the title, or the day and time. A moved event
/// keeps its length and time zone.
pub(crate) fn edited_event(ics: &str, text: &str) -> Option<String> {
    let old = event(ics)?;
    let ((oday, otitle), (day, title)) = (event_day(&old)?, event_day(text)?);
    let ((mut otime, mut osummary), (mut time, mut summary)) = (leading_time(otitle), leading_time(title));
    let start = property(ics, "VEVENT", "DTSTART")?;
    let old_start = ics_time(&start.1)?;
    // An all-day event whose title starts with a time: that time is its title's, not its start.
    if old_start.1.is_none() && otime.is_some() && otime == time { (otime, osummary, time, summary) = (None, otitle, None, title) }
    let (mut drop, mut add) = (Vec::new(), Vec::new());
    if osummary != summary { drop.push("SUMMARY"); add.push(format!("SUMMARY:{}", escape_text(summary))) }
    if (oday, otime) != (day, time) {
        // Ripple shows a series once, on its first day: moving that would move every occurrence.
        if property(ics, "VEVENT", "RRULE").is_some() || property(ics, "VEVENT", "RECURRENCE-ID").is_some() { return None }
        // DTEND, or DURATION's days and hours; an all-day event without either lasts a day.
        let length = property(ics, "VEVENT", "DTEND").and_then(|(_, v)| Some(minutes(ics_time(&v)?) - minutes(old_start)))
            .or_else(|| property(ics, "VEVENT", "DURATION").and_then(|(_, v)| duration(&v)))
            .or(otime.is_none().then_some(1440));
        let timed = time.is_some() && otime.is_some();
        let tz = start.0.split(';').find(|p| p.to_ascii_uppercase().starts_with("TZID=")).filter(|_| timed && !start.1.ends_with('Z'));
        let params = tz.map_or(String::new(), |tz| format!(";{tz}"));
        // A UTC time stays UTC; others are written on this clock.
        let utc = timed && start.1.trim().ends_with('Z');
        let value = |local: i64| if utc { format!("{}Z", ics_value(to_utc(local), false)) } else { ics_value(local, false) };
        let at = minutes((day, time));
        drop.extend(["DTSTART", "DTEND", "DURATION"]);
        match time {
            Some(_) => {
                add.push(format!("DTSTART{params}:{}", value(at)));
                // Timed to timed keeps its length; an all-day event made timed has none, as new ones.
                if let Some(len) = length.filter(|_| otime.is_some()) { add.push(format!("DTEND{params}:{}", value(at + len))) }
            }
            None => {
                let days = length.filter(|_| otime.is_none()).map_or(1, |l| (l / 1440).max(1));
                add.push(format!("DTSTART;VALUE=DATE:{}", ics_value(at, true)));
                add.push(format!("DTEND;VALUE=DATE:{}", ics_value(at + days * 1440, true)));
            }
        }
    }
    Some(rewrite(ics, "VEVENT", &drop, &add))
}

/// A DURATION's length in minutes: weeks, days, hours and minutes (`P1DT2H`, `PT45M`).
fn duration(v: &str) -> Option<i64> {
    let v = v.trim().strip_prefix('P')?;
    let (mut total, mut n) = (0i64, 0i64);
    for c in v.chars() {
        match c {
            '0'..='9' => n = n * 10 + i64::from(c as u8 - b'0'),
            'W' => { total += n * 10_080; n = 0 }
            'D' => { total += n * 1440; n = 0 }
            'H' => { total += n * 60; n = 0 }
            'M' => { total += n; n = 0 }
            'T' | 'S' => n = 0,
            _ => return None,
        }
    }
    Some(total)
}

/// A to-do's object with its text's edit made: only the parts that changed are written.
pub(crate) fn edited_task(ics: &str, text: &str) -> Option<String> {
    let (old, new) = (Task::parse(&open_task(ics)?, today()), Task::parse(text, today()));
    let fresh = new_task("", text, "");
    let line = |name: &str| fresh.lines().find(|l| l.split([':', ';']).next() == Some(name)).map(str::to_string);
    let (mut drop, mut add) = (Vec::new(), Vec::new());
    for (name, changed) in [("SUMMARY", old.title != new.title), ("PRIORITY", old.priority != new.priority), ("DUE", old.due != new.due), ("RRULE", old.every != new.every)] {
        if !changed { continue }
        drop.push(name);
        add.extend(line(name));
    }
    Some(rewrite(ics, "VTODO", &drop, &add))
}

/// iCalendar lines with their folds joined.
fn unfold(ics: &str) -> String { ics.replace("\r\n", "\n").replace("\n ", "").replace("\n\t", "") }

/// The calendar object's to-do as a task.s text (`task::Task`), unless it is done.
pub(crate) fn open_task(ics: &str) -> Option<String> {
    let (mut summary, mut open) = (None, true);
    let mut task = Task::default();
    for (name, _, value) in props(ics, "VTODO") {
        let value = value.as_str();
        match name.as_str() {
            "SUMMARY" => summary = Some(unescape_text(value)),
            "STATUS" => open &= !matches!(value.trim().to_ascii_uppercase().as_str(), "COMPLETED" | "CANCELLED"),
            "COMPLETED" => open = false,
            // 1-4 high, 5 medium, 6-9 low; 0 none (RFC 5545 3.8.1.9).
            "PRIORITY" => task.priority = match value.trim().parse::<u8>() { Ok(1..=4) => 3, Ok(5) => 2, Ok(6..=9) => 1, _ => 0 },
            "DUE" => task.due = ics_time(value),
            "RRULE" => task.every = Some(simple_rule(value).unwrap_or(Every::Custom)),
            _ => {}
        }
    }
    task.title = summary.filter(|_| open)?;
    Some(task.to_string())
}

/// A repeat rule as Ripple's daily, weekly or monthly, when it is only that (plus a week start or
/// an interval of 1); anything more is `Every::Custom`, which Ripple leaves to Nextcloud.
fn simple_rule(rule: &str) -> Option<Every> {
    let mut every = None;
    for part in rule.split(';').map(|p| p.trim().to_ascii_uppercase()) {
        match part.as_str() {
            "FREQ=DAILY" => every = Some(Every::Day),
            "FREQ=WEEKLY" => every = Some(Every::Week),
            "FREQ=MONTHLY" => every = Some(Every::Month),
            "INTERVAL=1" | "" => {}
            p if p.starts_with("WKST=") => {}
            _ => return None,
        }
    }
    every
}

/// A DATE or DATE-TIME value: its day, and its time on this clock. UTC moves to this clock; a
/// TZID time is taken as written, as it is usually this zone.
fn ics_time(value: &str) -> Option<((i32, u32, u32), Option<(u32, u32)>)> {
    let value = value.trim();
    let num = |from: usize, to: usize| value.get(from..to)?.parse::<u32>().ok();
    let (y, m, d) = (num(0, 4)? as i32, num(4, 6)?, num(6, 8)?);
    match value.get(8..)? {
        "" => Some(((y, m, d), None)),
        t if t.starts_with('T') => {
            let (h, min) = (num(9, 11)?, num(11, 13)?);
            if !t.ends_with('Z') { return Some(((y, m, d), Some((h, min)))) }
            let l = rune::time::LocalTime::at((days_from_civil(y, m, d) * 86_400 + i64::from(h * 3600 + min * 60)) * 1000);
            Some(((l.year, l.month, l.day), Some((l.hour, l.minute))))
        }
        _ => None,
    }
}

/// An iCalendar TEXT value; a line break becomes a space, as tasks are one line here.
fn unescape_text(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        out.push(match c {
            '\\' => match chars.next() { Some('n' | 'N') => ' ', Some(c) => c, None => '\\' },
            c => c,
        });
    }
    out
}

fn escape_text(value: &str) -> String { value.replace('\\', "\\\\").replace(';', "\\;").replace(',', "\\,").replace('\n', "\\n").replace('\r', "") }

/// A to-do from a task's text; `open_task` reads it back as the same text.
pub(crate) fn new_task(uid: &str, text: &str, stamp: &str) -> String {
    let task = Task::parse(text, today());
    let summary = escape_text(&task.title);
    let mut extra = String::new();
    if task.priority > 0 { extra += &format!("PRIORITY:{}\r\n", [0, 9, 5, 1][usize::from(task.priority)]) }
    match task.due {
        Some(((y, m, d), None)) => extra += &format!("DUE;VALUE=DATE:{y:04}{m:02}{d:02}\r\n"),
        Some(((y, m, d), Some((h, min)))) => extra += &format!("DUE:{y:04}{m:02}{d:02}T{h:02}{min:02}00\r\n"),
        None => {}
    }
    let freq = match task.every { Some(Every::Day) => Some("DAILY"), Some(Every::Week) => Some("WEEKLY"), Some(Every::Month) => Some("MONTHLY"), Some(Every::Custom) | None => None };
    if let Some(freq) = freq { extra += &format!("RRULE:FREQ={freq}\r\n") }
    format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Ripple//EN\r\nBEGIN:VTODO\r\nUID:{uid}\r\nDTSTAMP:{stamp}\r\nCREATED:{stamp}\r\nSUMMARY:{summary}\r\n{extra}STATUS:NEEDS-ACTION\r\nEND:VTODO\r\nEND:VCALENDAR\r\n")
}

/// The calendar object with its to-dos marked done, everything else kept.
pub(crate) fn completed(ics: &str, stamp: &str) -> String {
    let mut out = String::new();
    let mut inside = false;
    for line in unfold(ics).lines() {
        let upper = line.to_ascii_uppercase();
        let name = upper.split([':', ';']).next().unwrap_or_default();
        if upper == "BEGIN:VTODO" { inside = true }
        if inside && ["STATUS", "COMPLETED", "PERCENT-COMPLETE"].contains(&name) { continue }
        if inside && upper == "END:VTODO" {
            out.push_str(&format!("STATUS:COMPLETED\r\nCOMPLETED:{stamp}\r\nPERCENT-COMPLETE:100\r\n"));
            inside = false;
        }
        out.push_str(line);
        out.push_str("\r\n");
    }
    out
}

/// An event's day and title: its text is `YYYY-MM-DD title`, where the title starts with the
/// time (`HH:MM`) when the event has one.
pub fn event_day(text: &str) -> Option<((i32, u32, u32), &str)> {
    let (date, title) = text.split_once(' ')?;
    Some((crate::date::parse_ymd(date)?, title))
}

/// Every event of an ICS file, as events' texts, from `since` (`YYYY-MM-DD`) on.
pub fn feed_events(ics: &str, since: &str) -> Vec<String> {
    let mut events: Vec<String> = ics.split("BEGIN:VEVENT").skip(1).filter_map(|e| event(&format!("BEGIN:VEVENT{e}"))).filter(|t| t.as_str() >= since).collect();
    events.sort();
    events
}

/// The calendar object's first event as an event's text, unless it was cancelled.
/// shortcut: a recurring event shows on its first day only; expand RRULEs if people ask.
pub(crate) fn event(ics: &str) -> Option<String> {
    let (mut summary, mut start, mut open) = (String::new(), None, true);
    for (name, _, value) in props(ics, "VEVENT") {
        let value = value.as_str();
        match name.as_str() {
            "SUMMARY" => summary = unescape_text(value),
            "DTSTART" => start = Some(value.trim().to_string()),
            "STATUS" => open = !value.trim().eq_ignore_ascii_case("CANCELLED"),
            _ => {}
        }
    }
    let ((y, m, d), time) = ics_time(&start?)?;
    let summary = if summary.trim().is_empty() { "Untitled".to_string() } else { summary };
    let text = match time {
        None => format!("{y:04}-{m:02}-{d:02} {summary}"),
        Some((h, min)) => format!("{y:04}-{m:02}-{d:02} {h:02}:{min:02} {summary}"),
    };
    open.then_some(text)
}

/// An event on `day`: at the title's leading `HH:MM`, on this clock, else all day. `event` reads
/// either back as the same text.
pub(crate) fn new_event(uid: &str, (y, m, d): (i32, u32, u32), title: &str, stamp: &str) -> String {
    let (when, summary) = match leading_time(title) {
        (Some((h, min)), rest) => (format!("DTSTART:{y:04}{m:02}{d:02}T{h:02}{min:02}00"), rest),
        (None, _) => {
            let (ny, nm, nd) = crate::date::add_days((y, m, d), 1);
            (format!("DTSTART;VALUE=DATE:{y:04}{m:02}{d:02}\r\nDTEND;VALUE=DATE:{ny:04}{nm:02}{nd:02}"), title)
        }
    };
    format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Ripple//EN\r\nBEGIN:VEVENT\r\nUID:{uid}\r\nDTSTAMP:{stamp}\r\nCREATED:{stamp}\r\n{when}\r\nSUMMARY:{}\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n", escape_text(summary))
}

/// An event title's leading `HH:MM`, and the rest.
pub fn leading_time(title: &str) -> (Option<(u32, u32)>, &str) {
    let time = title.split_once(' ').filter(|(t, _)| t.len() == 5 && t.as_bytes()[2] == b':')
        .and_then(|(t, rest)| Some(((t[..2].parse::<u32>().ok().filter(|h| *h < 24)?, t[3..].parse::<u32>().ok().filter(|m| *m < 60)?), rest)));
    match time { Some((t, rest)) => (Some(t), rest), None => (None, title) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_feed_lists_its_events_from_a_day_on() {
        let ics = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nDTSTART;VALUE=DATE:20260105\r\nSUMMARY:Dentist\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nDTSTART:20260102T093000\r\nSUMMARY:Stand\r\n up\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nDTSTART:20250101T100000\r\nSUMMARY:Old\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nDTSTART:20260103T100000\r\nSUMMARY:Off\r\nSTATUS:CANCELLED\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        assert_eq!(feed_events(ics, "2026-01-01"), ["2026-01-02 09:30 Standup", "2026-01-05 Dentist"]);
    }

    #[test]
    fn todos_read_and_complete() {
        let ics = "BEGIN:VCALENDAR\r\nBEGIN:VTODO\r\nUID:1\r\nSUMMARY;LANGUAGE=en:Buy milk\\, eggs and a very long\r\n  line\r\nSTATUS:NEEDS-ACTION\r\nDUE:20260101\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";
        assert_eq!(open_task(ics).as_deref(), Some("Buy milk, eggs and a very long line due:2026-01-01"));
        let done = completed(ics, "20261118T000000Z");
        assert_eq!(open_task(&done), None);
        assert!(done.contains("DUE:20260101\r\nSTATUS:COMPLETED\r\nCOMPLETED:20261118T000000Z\r\nPERCENT-COMPLETE:100\r\nEND:VTODO") && !done.contains("NEEDS-ACTION"));
        assert_eq!(open_task(&new_task("u", "a, b; c \\n", "20260101T000000Z")).as_deref(), Some("a, b; c \\n"));
        let rich = "!!! Pay rent due:2026-12-01T09:30 every:month";
        let written = new_task("u", rich, "20260101T000000Z");
        assert!(written.contains("PRIORITY:1\r\nDUE:20261201T093000\r\nRRULE:FREQ=MONTHLY\r\n") && written.contains("SUMMARY:Pay rent\r\n"));
        assert_eq!(open_task(&written).as_deref(), Some(rich));
        assert_eq!(open_task("BEGIN:VTODO\r\nSUMMARY:Low\r\nPRIORITY:7\r\nRRULE:FREQ=WEEKLY;WKST=MO;INTERVAL=1\r\nEND:VTODO").as_deref(), Some("! Low every:week"));
        let fortnightly = "BEGIN:VCALENDAR\r\nBEGIN:VTODO\r\nSUMMARY:Bins\r\nRRULE:FREQ=WEEKLY;INTERVAL=2\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";
        assert_eq!(open_task(fortnightly).as_deref(), Some("Bins every:custom"), "a rule Ripple can't follow is left to Nextcloud");
        assert!(edited_task(fortnightly, "Take the bins out every:custom").unwrap().contains("RRULE:FREQ=WEEKLY;INTERVAL=2"));
        assert!(!new_task("u", "Bins every:custom", "").contains("RRULE"));
        assert_eq!(open_task("BEGIN:VTODO\r\nSUMMARY:Task\r\nBEGIN:VALARM\r\nSUMMARY:Alarm\r\nEND:VALARM\r\nEND:VTODO").as_deref(), Some("Task"));
    }

    #[test]
    fn events_read_and_write() {
        let ics = "BEGIN:VCALENDAR\r\nBEGIN:VTIMEZONE\r\nBEGIN:STANDARD\r\nDTSTART:19701025T030000\r\nEND:STANDARD\r\nEND:VTIMEZONE\r\nBEGIN:VEVENT\r\nDTSTART;TZID=Europe/Berlin:20261119T150000\r\nSUMMARY:Dentist\\, upstairs\r\nBEGIN:VALARM\r\nSUMMARY:Alarm\r\nEND:VALARM\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        assert_eq!(event(ics).as_deref(), Some("2026-11-19 15:00 Dentist, upstairs"));
        assert_eq!(event("BEGIN:VEVENT\r\nDTSTART;VALUE=DATE:20261231\r\nEND:VEVENT").as_deref(), Some("2026-12-31 Untitled"));
        assert_eq!(event("BEGIN:VEVENT\r\nDTSTART:20261231\r\nSTATUS:CANCELLED\r\nEND:VEVENT"), None);
        let new = new_event("u", (2026, 12, 31), "Party; late", "20261118T000000Z");
        assert!(new.contains("DTSTART;VALUE=DATE:20261231\r\nDTEND;VALUE=DATE:20270101\r\n"));
        assert_eq!(event(&new).as_deref(), Some("2026-12-31 Party; late"), "an event round-trips to the same text");
        let timed = new_event("u", (2026, 11, 19), "15:00 Dentist", "20261118T000000Z");
        assert!(timed.contains("DTSTART:20261119T150000\r\nSUMMARY:Dentist\r\n"), "a leading time is the event's time, not its title");
        assert_eq!(event(&timed).as_deref(), Some("2026-11-19 15:00 Dentist"));
        assert_eq!(event(&new_event("u", (2026, 11, 19), "25:00 Odd", "s")).as_deref(), Some("2026-11-19 25:00 Odd"));
        assert_eq!(event_day("2026-02-03 9:00 Standup"), Some(((2026, 2, 3), "9:00 Standup")));
        assert_eq!((event_day("2026-13-01 x"), event_day("Standup")), (None, None));
    }

    #[test]
    fn edits_rewrite_only_what_changed() {
        let stray = "BEGIN:VEVENT\r\nDTSTART;VALUE=DATE:20261119\r\nEND:X-STRAY\r\nSUMMARY:Old\r\nEND:VEVENT\r\n";
        assert_eq!(event(stray).as_deref(), Some("2026-11-19 Old"), "a stray END hides nothing after it");
        let renamed = edited_event(stray, "2026-11-19 New").unwrap();
        assert!(renamed.contains("SUMMARY:New") && !renamed.contains("SUMMARY:Old"), "{renamed}");
        let ics = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:1\r\nDTSTART;TZID=Europe/Berlin:20261119T150000\r\nDTEND;TZID=Europe/Berlin:20261119T163000\r\nSUMMARY:Dentist\r\nDESCRIPTION:Bring the card\r\nBEGIN:VALARM\r\nSUMMARY:Alarm\r\nTRIGGER:-PT15M\r\nEND:VALARM\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let renamed = edited_event(ics, "2026-11-19 15:00 Dentist, upstairs").unwrap();
        assert!(renamed.contains("SUMMARY:Dentist\\, upstairs\r\nEND:VEVENT") && renamed.contains("SUMMARY:Alarm") && renamed.contains("DESCRIPTION:Bring the card"));
        assert!(renamed.contains("DTSTART;TZID=Europe/Berlin:20261119T150000"), "an untouched time stays as written");
        let moved = edited_event(ics, "2026-11-20 09:00 Dentist").unwrap();
        assert!(moved.contains("DTSTART;TZID=Europe/Berlin:20261120T090000\r\nDTEND;TZID=Europe/Berlin:20261120T103000\r\n"), "{moved}");
        assert_eq!(event(&moved).as_deref(), Some("2026-11-20 09:00 Dentist"));
        let all_day = edited_event(ics, "2026-11-21 Dentist").unwrap();
        assert!(all_day.contains("DTSTART;VALUE=DATE:20261121\r\nDTEND;VALUE=DATE:20261122\r\n") && !all_day.contains("T150000"));
        let utc = "BEGIN:VEVENT\r\nDTSTART:20261119T150000Z\r\nDTEND:20261119T160000Z\r\nSUMMARY:Call\r\nEND:VEVENT\r\n";
        let moved_utc = edited_event(utc, "2026-11-20 09:00 Call").unwrap();
        assert_eq!(event(&moved_utc).as_deref(), Some("2026-11-20 09:00 Call"), "{moved_utc}");
        assert!(moved_utc.lines().filter(|l| l.starts_with("DTSTART:") || l.starts_with("DTEND:")).all(|l| l.ends_with('Z')), "a UTC event stays UTC: {moved_utc}");
        let meeting = "BEGIN:VEVENT\r\nDTSTART;VALUE=DATE:20261119\r\nDTEND;VALUE=DATE:20261120\r\nSUMMARY:10:00 Meeting\r\nEND:VEVENT\r\n";
        let moved_meeting = edited_event(meeting, "2026-11-20 10:00 Meeting").unwrap();
        assert!(moved_meeting.contains("DTSTART;VALUE=DATE:20261120\r\nDTEND;VALUE=DATE:20261121\r\n") && moved_meeting.contains("SUMMARY:10:00 Meeting"), "{moved_meeting}");
        let series = "BEGIN:VEVENT\r\nDTSTART;VALUE=DATE:20261119\r\nRRULE:FREQ=WEEKLY;BYDAY=TH\r\nSUMMARY:Gym\r\nEND:VEVENT\r\n";
        assert_eq!(edited_event(series, "2026-11-20 Gym"), None, "a series isn't moved");
        assert!(edited_event(series, "2026-11-19 Gym class").unwrap().contains("SUMMARY:Gym class"), "but it can be renamed");
        let trip = "BEGIN:VEVENT\r\nDTSTART;VALUE=DATE:20261101\r\nDURATION:P3D\r\nSUMMARY:Trip\r\nEND:VEVENT\r\n";
        assert!(edited_event(trip, "2026-11-10 Trip").unwrap().contains("DTSTART;VALUE=DATE:20261110\r\nDTEND;VALUE=DATE:20261113\r\n"), "a moved event keeps its length");
        let todo = "BEGIN:VCALENDAR\r\nBEGIN:VTODO\r\nSUMMARY:Rent\r\nRRULE:FREQ=MONTHLY;BYMONTHDAY=1\r\nDESCRIPTION:Landlord\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";
        let task = edited_task(todo, "!! Pay rent every:custom").unwrap();
        assert!(task.contains("RRULE:FREQ=MONTHLY;BYMONTHDAY=1") && task.contains("DESCRIPTION:Landlord"), "an unchanged repeat keeps its rule");
        assert!(task.contains("SUMMARY:Pay rent\r\nPRIORITY:5\r\nEND:VTODO"));
        assert_eq!(open_task(&task).as_deref(), Some("!! Pay rent every:custom"));
        assert_eq!((duration("P1DT2H30M"), duration("PT30M"), duration("P2W"), duration("1H")), (Some(1590), Some(30), Some(20_160), None));
        assert_eq!(ics_time("20261119T1500"), Some(((2026, 11, 19), Some((15, 0)))));
        assert_eq!((ics_time("2026111"), ics_time("20261119X")), (None, None));
    }
}
