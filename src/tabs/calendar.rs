//! Calendar: a month at a glance, today marked; the arrows page through months. Beside it, the
//! picked day's events. With a Nextcloud account, events sync with a Nextcloud calendar.

use rune::{El, ScrollSpec, Transition, Tx, col, el, row, text};

use super::{button, empty, field, tool};
use crate::config::Item;
use crate::icons;
use crate::island::{Island, format_date, weekday};
use crate::nextcloud::{Kind, event_day};

const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

/// The month `offset` months from (year, month).
fn shift(year: i32, month: u32, offset: i32) -> (i32, u32) {
    let i = year * 12 + month as i32 - 1 + offset;
    (i.div_euclid(12), i.rem_euclid(12) as u32 + 1)
}

fn days_in(year: i32, month: u32) -> u32 {
    match month {
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

fn add_event(s: &mut Island, _: &mut Tx) {
    let title = s.event.text.trim().to_string();
    if title.is_empty() { return }
    let (y, m, d) = s.calendar_day.unwrap_or(s.today);
    s.save(|c| c.events.push(Item::new(format!("{y:04}-{m:02}-{d:02} {title}"))));
    s.event.clear();
    s.nextcloud_sync(Kind::Events);
}

fn delete_event(s: &mut Island, key: u64) {
    s.save(|c| {
        let Some(i) = c.events.iter().position(|e| e.key == key) else { return };
        if let Some(r) = c.events.remove(i).remote { c.nextcloud.deleted_events.push(r.id) }
    });
    s.nextcloud_sync(Kind::Events);
}

/// The picked day's events, by title, so timed ones (`HH:MM title`) run in order.
fn day_events(s: &Island, day: (i32, u32, u32)) -> Vec<(u64, &str)> {
    let mut events: Vec<_> = s.cfg.events.iter().filter_map(|e| event_day(&e.text).filter(|(d, _)| *d == day).map(|(_, title)| (e.key, title))).collect();
    events.sort_by_key(|e| e.1);
    events
}

pub fn view(s: &Island) -> El {
    let fg = s.cfg.text();
    let ink = s.cfg.bg().alpha(1.0);
    let (ty, tm, td) = s.today;
    let (year, month) = shift(ty, tm, s.calendar_offset);

    let mut header = row().key("calendar-header").w_full().h(28.0).gap(6.0).center_y()
        .child(
            row().key(format!("calendar-title-{year}-{month}")).grow(1.0).min_w(0.0).gap(6.0).center_y()
                .transition(Transition::blur_replace())
                .child(text(MONTHS[month as usize - 1]).font(s.cfg.font(17.0, 600.0)).color(fg))
                .child(text(year.to_string()).font(s.cfg.font(17.0, 400.0).features("'tnum' 1")).color(fg.alpha(0.5 * fg.a))),
        );
    if s.calendar_offset != 0 || s.calendar_day.is_some_and(|d| d != s.today) {
        // Shaped like `tool`, so it sits quietly beside the arrows.
        header = header.child(
            el().key("calendar-today").role("button").label("Today").focusable(true)
                .h(28.0).px(12.0).center().radius(s.cfg.radius(14.0)).fill(fg.alpha(0.1 * fg.a))
                .hover(fg.alpha(0.18 * fg.a)).press(fg.alpha(0.26 * fg.a))
                .child(text("Today").font(s.cfg.font(13.0, 600.0)).color(fg).a11y_hidden())
                .on_press(|s: &mut Island| (s.calendar_offset, s.calendar_day) = (0, None)),
        );
    }
    header = header
        .child(tool(s, "calendar-prev", &icons::CHEVRON_LEFT, "Previous month").on_press(|s: &mut Island| s.calendar_offset -= 1))
        .child(tool(s, "calendar-next", &icons::CHEVRON_RIGHT, "Next month").on_press(|s: &mut Island| s.calendar_offset += 1));

    let mut names = row().key("calendar-weekdays").w_full();
    for (i, d) in ["S", "M", "T", "W", "T", "F", "S"].into_iter().enumerate() {
        names = names.child(el().key(format!("weekday-{i}")).w(0.0).grow(1.0).h(16.0).center().a11y_hidden()
            .child(text(d).font(s.cfg.font(11.0, 600.0)).color(fg.alpha(0.45 * fg.a))));
    }

    // Six weeks always, so the Island keeps one height; days of the months around are dimmed.
    let first = weekday(year, month, 1) as i32;
    let picked = s.calendar_day.unwrap_or(s.today);
    let busy_days: Vec<_> = s.cfg.events.iter().filter_map(|e| Some(event_day(&e.text)?.0)).collect();
    let (py, pm) = shift(year, month, -1);
    let (len, prev_len) = (days_in(year, month) as i32, days_in(py, pm) as i32);
    let mut grid = col().key(format!("calendar-grid-{year}-{month}")).role("group").label(format!("{} {year}", MONTHS[month as usize - 1]))
        .w_full().gap(2.0).transition(Transition::opacity());
    for week in 0..6 {
        let mut line = row().w_full();
        for col_ in 0..7 {
            let n = week * 7 + col_ - first + 1;
            // A day of the month before or after pages there as it is picked.
            let (day, page) = if n < 1 { (prev_len + n, -1) } else if n > len { (n - len, 1) } else { (n, 0) };
            let (dy, dm) = shift(year, month, page);
            let date = (dy, dm, day as u32);
            let today = date == (ty, tm, td);
            let busy = busy_days.contains(&date);
            let color = if today { ink } else if page == 0 { fg } else { fg.alpha(0.25 * fg.a) };
            let mut dot = el().size(28.0, 28.0).center().radius(s.cfg.radius(14.0))
                .child(text(day.to_string()).font(s.cfg.font(13.0, if today { 700.0 } else { 500.0 }).features("'tnum' 1")).color(color).a11y_hidden());
            if today { dot = dot.fill(fg) } else if busy { dot = dot.fill(fg.alpha(0.12 * fg.a)) }
            if date == picked && !today { dot = dot.border(1.5, fg.alpha(0.6 * fg.a)) }
            let label = format!("{}{}{}", format_date(dy, dm, day as u32), if today { ", today" } else { "" }, if busy { ", has events" } else { "" });
            line = line.child(
                el().key(format!("day-{week}-{col_}")).role("button").label(label).focusable(true).w(0.0).grow(1.0).h(28.0).center().child(dot)
                    .on_press(move |s: &mut Island| { s.calendar_day = Some(date); s.calendar_offset += page }),
            );
        }
        grid = grid.child(line);
    }

    let mut list = col().key("event-list").w_full().gap(6.0);
    if let Some(e) = &s.nextcloud[Kind::Events as usize].error {
        list = list.child(text(e).font(s.cfg.font(12.0, 400.0)).color(fg.alpha(0.55 * fg.a)).wrap_anywhere());
    }
    let events = day_events(s, picked);
    if events.is_empty() { list = list.child(empty(s, "No events")) }
    for (key, title) in events {
        list = list.child(
            row().key(format!("event-{key}")).w_full().pl(12.0).pr(4.0).py(4.0).gap(8.0).center_y()
                .radius(s.cfg.radius(12.0)).fill(fg.alpha(0.05 * fg.a))
                .transition(Transition::opacity())
                .child(text(title).font(s.cfg.font(14.0, 500.0)).color(fg).grow(1.0).min_w(0.0).wrap_anywhere())
                .child(tool(s, "delete", &icons::X, &format!("Delete {title}")).on_press(move |s: &mut Island| delete_event(s, key))),
        );
    }
    let (py_, pm_, pd_) = picked;
    let day = col().key("calendar-day").w(0.0).grow(1.0).h_full().gap(8.0)
        .child(row().key("calendar-day-title").w_full().h(28.0).center_y()
            .child(text(format_date(py_, pm_, pd_)).font(s.cfg.font(15.0, 600.0)).color(fg)))
        .child(col().key("event-scroll").w_full().grow(1.0).min_h(0.0).scroll(ScrollSpec { stick_bottom: false }).on_wheel(Island::wheel).child(list))
        .child(
            row().key("event-input").w_full().gap(8.0).center_y()
                .child(field(s, "new-event", &s.event, "New event...", |s: &mut Island| &mut s.event, add_event))
                .child(button(s, "add-event", "Add", true).on_press_tx(add_event)),
        );

    row().key("calendar").w_full().h_full().px(16.0).pt(14.0).pb(14.0).gap(16.0)
        .child(col().key("calendar-month").w(280.0).h_full().gap(8.0).child(header).child(names).child(grid))
        .child(day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn months_page_across_years() {
        assert_eq!(shift(2026, 1, -1), (2025, 12));
        assert_eq!(shift(2026, 12, 1), (2027, 1));
        assert_eq!(shift(2026, 11, -23), (2024, 12));
        assert_eq!((days_in(2024, 2), days_in(2026, 2), days_in(2000, 2), days_in(1900, 2)), (29, 28, 29, 28));
        assert_eq!(weekday(2026, 11, 18), 3);
    }
}
