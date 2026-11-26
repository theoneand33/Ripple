//! Calendar: a month at a glance, today marked; the arrows page through months. Beside it, the
//! picked day's events; press one to edit it. With a Nextcloud account, events sync with a
//! Nextcloud calendar. Events from ICS links (Google, Outlook, iCloud) show beside them, read-only.
//! A timed Nextcloud event raises an alert shortly before it starts, with a Join button when its
//! title holds a meeting link.

use rune::{El, TextEdit, Transition, Tx, col, el, row, text};

use super::{Scrolls, accent, button, empty, field, pill, tool, undo_button};
use crate::config::{Item, Tab};
use crate::icons;
use crate::date::{days_in, format_date, shift_month, weekday};
use crate::island::Island;
use crate::ics::{event_day, feed_events, leading_time, meeting_link};
use crate::nextcloud::Kind;
use crate::reminders::{Reminder, Source};

#[derive(Default)]
pub struct Calendar {
    /// The month shown, as months from this one.
    pub offset: i32,
    /// The picked day; None is today.
    pub day: Option<(i32, u32, u32)>,
    pub event: TextEdit,
    /// The event the field edits, by key; None adds a new one.
    pub editing: Option<u64>,
    /// The ICS links' events, as events' texts, and why a link failed.
    pub feed_events: Vec<String>,
    pub feed_error: Option<String>,
    /// The links last fetched, and when (Unix seconds).
    feeds_loaded: (Vec<String>, i64),
}

impl Island {
    /// Fetches the ICS links once every 30 minutes, or at once when they changed.
    pub(super) fn calendar_feeds_load(&mut self) {
        let now = crate::date::now_secs();
        let feeds = self.cfg.calendar_feeds.clone();
        if feeds == self.calendar.feeds_loaded.0 && now - self.calendar.feeds_loaded.1 < 1800 { return }
        let Some(me) = self.me.clone() else { return };
        self.calendar.feeds_loaded = (feeds.clone(), now);
        let (y, m, d) = self.today;
        // A year back, as Nextcloud sync keeps.
        let since = crate::date::shift_month(y, m, -12);
        let since = format!("{:04}-{:02}-{d:02}", since.0, since.1);
        // Apply only a fetch for the current links, so a slow one for old links can't land last.
        me.spawn(move || {
            let (mut events, mut errors) = (Vec::new(), Vec::new());
            for url in &feeds {
                match crate::net::calendar_feed(url) { Ok(ics) => events.extend(feed_events(&ics, &since)), Err(e) => errors.push(e) }
            }
            (feeds, events, errors.into_iter().next())
        }, move |s, (feeds, events, error)| if s.cfg.calendar_feeds == feeds { (s.calendar.feed_events, s.calendar.feed_error) = (events, error) });
    }
}

const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

/// Minutes after its start that an event can still alert: one Ripple wasn't running for, or
/// whose alert waited.
const EVENT_GRACE: i64 = 15;

/// The timed events to alert now (`now` and `lead` in minutes, since 1970 on this clock): those
/// starting within `lead` minutes, or that started up to `EVENT_GRACE` minutes ago, and not in
/// `alerted`. Each is its id for `alerted`, its title, and its meeting link.
pub fn due_events(events: &[Item], now: i64, lead: i64, alerted: &[String]) -> Vec<(String, String, Option<String>)> {
    events.iter().filter_map(|e| {
        let ((y, m, d), title) = event_day(&e.text)?;
        let (Some((h, min)), rest) = leading_time(title) else { return None };
        let start = crate::date::days_from_civil(y, m, d) * 1440 + i64::from(h * 60 + min);
        let id = format!("{start} {rest}");
        if !(start - lead..=start + EVENT_GRACE).contains(&now) || alerted.contains(&id) { return None }
        let (link, title) = meeting_link(rest);
        Some((id, if title.is_empty() { "Meeting".into() } else { title }, link))
    }).collect()
}

impl Island {
    /// Once a second: an alert for the next timed event, `event_alert_minutes` before it.
    pub fn event_alerts(&mut self) {
        let lead = i64::from(self.cfg.event_alert_minutes);
        if lead == 0 { return }
        let minute = this_minute();
        let due = due_events(&self.cfg.events, minute, lead, &self.cfg.alerted_events);
        if due.is_empty() { return }
        self.save(|c| {
            // Ids start with the event's start minute; those over a day old can't alert again.
            c.alerted_events.retain(|id| id.split(' ').next().and_then(|m| m.parse::<i64>().ok()).is_some_and(|m| m >= minute - 1440));
            c.alerted_events.extend(due.iter().map(|d| d.0.clone()));
        });
        for (id, title, link) in due { self.offer(Reminder { source: Source::Event, id, title, link }) }
    }

    /// Whether the event a reminder `id` names may still alert: there, and not over
    /// `EVENT_GRACE` minutes past its start.
    pub fn event_due(&self, id: &str) -> bool {
        let lead = i64::from(self.cfg.event_alert_minutes);
        due_events(&self.cfg.events, this_minute(), lead, &[]).iter().any(|d| d.0 == id)
    }
}

/// Minutes since 1970 on this clock.
fn this_minute() -> i64 {
    let t = rune::time::LocalTime::at(crate::date::now_ms());
    crate::date::days_from_civil(t.year, t.month, t.day) * 1440 + i64::from(t.hour * 60 + t.minute)
}

/// Adds the typed event on the picked day, or saves the one being edited there.
fn add_event(s: &mut Island, _: &mut Tx) {
    let title = s.calendar.event.text.trim().to_string();
    if title.is_empty() { return }
    let (y, m, d) = s.calendar.day.unwrap_or(s.today);
    let text = format!("{y:04}-{m:02}-{d:02} {title}");
    let editing = s.calendar.editing.take();
    s.calendar.event.clear();
    s.save_item(Kind::Events, editing, text);
}

fn start_edit(s: &mut Island, key: u64) {
    let Some((day, title)) = s.cfg.events.iter().find(|e| e.key == key).and_then(|e| event_day(&e.text)) else { return };
    s.calendar.event.replace_text(title.to_string());
    s.calendar.event.focused = true;
    (s.calendar.editing, s.calendar.day) = (Some(key), Some(day));
}

/// As the tab opens: back to this month, an edit left open dropped, and events and feeds load.
pub(super) fn entered(s: &mut Island) {
    (s.calendar.offset, s.calendar.day) = (0, None);
    if s.calendar.editing.is_some() { cancel_edit(s) }
    s.nextcloud_sync(Kind::Events);
    s.calendar_feeds_load();
}

fn cancel_edit(s: &mut Island) {
    s.calendar.editing = None;
    s.calendar.event.clear();
}

fn delete_event(s: &mut Island, key: u64) {
    if s.calendar.editing == Some(key) { cancel_edit(s) }
    s.delete_item(Kind::Events, key);
}

/// Every event's text: the user's own, by key, then the ICS links', which have no key as they
/// can't be edited.
fn all_events(s: &Island) -> impl Iterator<Item = (Option<u64>, &str)> {
    let own = s.cfg.events.iter().map(|e| (Some(e.key), e.text.as_str()));
    own.chain(s.calendar.feed_events.iter().map(|t| (None, t.as_str())))
}

impl Island {
    /// A day's events, own and the links', by title, so timed ones (`HH:MM title`) run in order.
    pub fn events_on(&self, day: (i32, u32, u32)) -> Vec<(Option<u64>, &str)> {
        let mut events: Vec<_> = all_events(self).filter_map(|(key, text)| event_day(text).filter(|(d, _)| *d == day).map(|(_, title)| (key, title))).collect();
        events.sort_by_key(|e| e.1);
        events
    }
}

pub fn view(s: &Island) -> El {
    let fg = s.cfg.text();
    let (ty, tm, td) = s.today;
    let (year, month) = shift_month(ty, tm, s.calendar.offset);

    let mut header = row().key("calendar-header").w_full().h(28.0).gap(6.0).center_y()
        .child(
            row().key(format!("calendar-title-{year}-{month}")).grow(1.0).min_w(0.0).gap(6.0).center_y()
                .transition(Transition::blur_replace())
                .child(text(MONTHS[month as usize - 1]).font(s.cfg.font(17.0, 700.0)).color(fg))
                .child(text(year.to_string()).font(s.cfg.font(17.0, 400.0).features("'tnum' 1")).color(fg.alpha(0.5 * fg.a))),
        );
    if s.calendar.offset != 0 || s.calendar.day.is_some_and(|d| d != s.today) {
        header = header.child(pill(s, "calendar-today", "Today").on_press(|s: &mut Island| (s.calendar.offset, s.calendar.day) = (0, None)));
    }
    header = header
        .child(tool(s, "calendar-prev", &icons::CHEVRON_LEFT, "Previous month").on_press(|s: &mut Island| s.calendar.offset -= 1))
        .child(tool(s, "calendar-next", &icons::CHEVRON_RIGHT, "Next month").on_press(|s: &mut Island| s.calendar.offset += 1));

    let mut names = row().key("calendar-weekdays").w_full();
    for (i, d) in crate::date::WEEKDAY_LETTERS.into_iter().enumerate() {
        names = names.child(el().key(format!("weekday-{i}")).w(0.0).grow(1.0).h(16.0).center().a11y_hidden()
            .child(text(d).font(s.cfg.font(11.0, 600.0)).color(fg.alpha(0.45 * fg.a))));
    }

    // Six weeks always, so the Island keeps one height; days of the months around are dimmed.
    let first = weekday(year, month, 1) as i32;
    let picked = s.calendar.day.unwrap_or(s.today);
    let busy_days: Vec<_> = all_events(s).filter_map(|(_, t)| Some(event_day(t)?.0)).collect();
    let (py, pm) = shift_month(year, month, -1);
    let (len, prev_len) = (days_in(year, month) as i32, days_in(py, pm) as i32);
    let mut grid = col().key(format!("calendar-grid-{year}-{month}")).role("group").label(format!("{} {year}", MONTHS[month as usize - 1]))
        .w_full().gap(2.0).transition(Transition::opacity());
    for week in 0..6 {
        let mut line = row().w_full();
        for col_ in 0..7 {
            let n = week * 7 + col_ - first + 1;
            // A day of the month before or after pages there as it is picked.
            let (day, page) = if n < 1 { (prev_len + n, -1) } else if n > len { (n - len, 1) } else { (n, 0) };
            let (dy, dm) = shift_month(year, month, page);
            let date = (dy, dm, day as u32);
            let today = date == (ty, tm, td);
            let busy = busy_days.contains(&date);
            let color = if today { s.cfg.bg() } else if page != 0 { fg.alpha(0.25 * fg.a) } else if col_ == 0 || col_ == 6 { fg.alpha(0.5 * fg.a) } else { fg };
            let mut dot = el().size(28.0, 28.0).center().radius(s.cfg.radius(14.0))
                .child(text(day.to_string()).font(s.cfg.font(13.0, if today { 700.0 } else { 500.0 }).features("'tnum' 1")).color(color).a11y_hidden());
            if today { dot = dot.fill(fg) } else if busy { dot = dot.fill(fg.alpha(0.12 * fg.a)) }
            if date == picked && !today { dot = dot.border(1.5, fg.alpha(0.6 * fg.a)) }
            let label = format!("{}{}{}", format_date(dy, dm, day as u32), if today { ", today" } else { "" }, if busy { ", has events" } else { "" });
            line = line.child(
                el().key(format!("day-{week}-{col_}")).role("button").label(label).focusable(true).w(0.0).grow(1.0).h(28.0).center().child(dot)
                    .on_press(move |s: &mut Island| { s.calendar.day = Some(date); s.calendar.offset += page }),
            );
        }
        grid = grid.child(line);
    }

    let mut list = col().key("event-list").w_full().gap(6.0);
    for e in [&s.nextcloud[Kind::Events].error, &s.calendar.feed_error].into_iter().flatten() {
        list = list.child(text(e).font(s.cfg.font(12.0, 400.0)).color(fg.alpha(0.55 * fg.a)).wrap_anywhere());
    }
    let events = s.events_on(picked);
    if events.is_empty() { list = list.child(empty(s, "No events")) }
    // Each event gets a colour of its own, a bar at its left edge, as Droppy's calendar does.
    let tints = [accent::indigo(), accent::orange(), accent::green(), accent::teal()];
    for (i, (key, title)) in events.into_iter().enumerate() {
        let tint = tints[i % tints.len()];
        let line = row().key(key.map_or(format!("feed-event-{i}"), |k| format!("event-{k}"))).w_full().pl(8.0).pr(4.0).py(4.0).gap(8.0).center_y()
            .radius(s.cfg.radius(10.0)).fill(tint.alpha(0.16))
            .transition(Transition::opacity());
        let Some(key) = key else {
            list = list.child(line.min_h(30.0).child(el().w(3.0).h(22.0).radius(1.5).fill(tint).a11y_hidden())
                .child(text(title).font(s.cfg.font(14.0, 600.0)).color(tint).grow(1.0).min_w(0.0).wrap_anywhere()));
            continue;
        };
        let mut line = line;
        if s.calendar.editing == Some(key) { line = line.border(1.5, tint) }
        list = list.child(
            line
                .child(el().w(3.0).h(22.0).radius(1.5).fill(tint).a11y_hidden())
                .child(el().key("edit").role("button").label(format!("Edit {title}")).focusable(true).grow(1.0).min_w(0.0)
                    .on_press(move |s: &mut Island| start_edit(s, key))
                    .child(text(title).font(s.cfg.font(14.0, 600.0)).color(tint).w_full().wrap_anywhere().a11y_hidden()))
                .child(tool(s, "delete", &icons::X, &format!("Delete {title}")).on_press(move |s: &mut Island| delete_event(s, key))),
        );
    }
    let editing = s.calendar.editing.is_some();
    let (py_, pm_, pd_) = picked;
    let day = col().key("calendar-day").w(0.0).grow(1.0).h_full().gap(8.0)
        .child(row().key("calendar-day-title").w_full().h(28.0).center_y().justify_between()
            .child({
                // "THU Jan 1": the weekday in dimmed capitals, as Droppy heads its day.
                row().gap(6.0).center_y()
                    .child(text(crate::date::weekday_name(py_, pm_, pd_).to_uppercase()).font(s.cfg.font(15.0, 700.0)).color(fg.alpha(0.5 * fg.a)))
                    .child(text(crate::date::month_day(pm_, pd_)).font(s.cfg.font(15.0, 700.0)).color(fg))
            })
            .children(undo_button(s, Tab::Calendar))
            .children(editing.then(|| pill(s, "cancel-edit", "Cancel edit").on_press(cancel_edit))))
        .child(col().key("event-scroll").w_full().grow(1.0).min_h(0.0).scrolls().child(list))
        .child(
            row().key("event-input").w_full().gap(8.0).center_y()
                .child(field(s, "new-event", &s.calendar.event, if editing { "Event title" } else { "New event…" }, |s: &mut Island| &mut s.calendar.event, add_event))
                .child(button(s, "add-event", if editing { "Save" } else { "Add" }, true).on_press_tx(add_event)),
        );

    row().key("calendar").w_full().h_full().px(16.0).pt(14.0).pb(14.0).gap(16.0)
        .child(col().key("calendar-month").w(280.0).h_full().gap(8.0).child(header).child(names).child(grid))
        .child(day)
}

#[cfg(test)]
mod tests {
    use crate::config::{Config, Item, Remote, Store};
    use crate::island::Island;

    #[test]
    fn an_event_alerts_once_within_its_lead_with_its_meeting_link() {
        let events = ["2026-01-01 All day", "2026-01-01 09:30 Standup https://meet.example.com/abc daily", "2026-01-01 09:30 Review", "2026-01-02 09:30 https://meet.example.com/x", "2026-01-02 10:00 Sync http://meet.example.com/y"].map(Item::new);
        let start = crate::date::days_from_civil(2026, 1, 1) * 1440 + 9 * 60 + 30;
        assert!(super::due_events(&events, start - 11, 10, &[]).is_empty(), "too early");
        let due = super::due_events(&events, start - 10, 10, &[]);
        assert_eq!(due.len(), 2, "events starting together alert together");
        let (id, title, link) = &due[0];
        assert_eq!((title.as_str(), link.as_deref()), ("Standup daily", Some("https://meet.example.com/abc")));
        let alerted: Vec<String> = due.iter().map(|d| d.0.clone()).collect();
        assert!(super::due_events(&events, start, 10, &alerted).is_empty(), "once");
        let held = super::due_events(&events, start + 15, 10, &[]);
        assert_eq!(held.len(), 2, "held while Large was open");
        assert_eq!(&held[0].0, id);
        assert!(super::due_events(&events, start + 16, 10, &[]).is_empty(), "past");
        let next = super::due_events(&events, start + 1440, 0, &[]);
        assert_eq!((next[0].1.as_str(), next[0].2.as_deref()), ("Meeting", Some("https://meet.example.com/x")), "a link-only title");
        let http = super::due_events(&events, start + 1440 + 30, 0, &[]);
        assert_eq!((http[0].1.as_str(), http[0].2.as_deref()), ("Sync http://meet.example.com/y", None), "no Join for http");
    }

    #[test]
    fn an_edit_changes_the_event_in_place() {
        let synced = Item { remote: Some(Remote { id: "/cal/a.ics".into(), etag: "1".into(), text: "2026-01-01 Old".into() }), ..Item::new("2026-01-01 Old") };
        let (local, synced_key) = (Item::new("2026-01-02 Mine"), synced.key);
        let local_key = local.key;
        let mut s = Island::new(Store::memory(Config { events: vec![synced, local], ..Config::default() }), Vec::new());
        super::start_edit(&mut s, local_key);
        assert_eq!((s.calendar.event.text.as_str(), s.calendar.day), ("Mine", Some((2026, 1, 2))));
        s.calendar.event.replace_text("09:00 Mine, moved");
        s.calendar.day = Some((2026, 1, 3));
        super::add_event(&mut s, &mut rune::Tx::default());
        assert_eq!((s.cfg.events[1].text.as_str(), s.cfg.events[1].key, s.calendar.editing), ("2026-01-03 09:00 Mine, moved", local_key, None));
        super::start_edit(&mut s, synced_key);
        s.calendar.event.replace_text("New");
        super::add_event(&mut s, &mut rune::Tx::default());
        assert_eq!((s.cfg.events[0].text.as_str(), s.cfg.events[0].key), ("2026-01-01 New", synced_key));
        assert!(s.cfg.events[0].remote.is_some() && s.cfg.nextcloud.deleted_events.is_empty(), "it keeps its link, so sync writes over it");
    }
}
