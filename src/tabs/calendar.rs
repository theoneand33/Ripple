//! Calendar: a month at a glance, today marked; the arrows page through months.

use rune::{El, Transition, col, el, row, text};

use super::tool;
use crate::icons;
use crate::island::{Island, weekday};

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
    if s.calendar_offset != 0 {
        // Shaped like `tool`, so it sits quietly beside the arrows.
        header = header.child(
            el().key("calendar-today").role("button").label("Today").focusable(true)
                .h(28.0).px(12.0).center().radius(s.cfg.radius(14.0)).fill(fg.alpha(0.1 * fg.a))
                .hover(fg.alpha(0.18 * fg.a)).press(fg.alpha(0.26 * fg.a))
                .child(text("Today").font(s.cfg.font(13.0, 600.0)).color(fg).a11y_hidden())
                .on_press(|s: &mut Island| s.calendar_offset = 0),
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
    let (py, pm) = shift(year, month, -1);
    let (len, prev_len) = (days_in(year, month) as i32, days_in(py, pm) as i32);
    let mut grid = col().key(format!("calendar-grid-{year}-{month}")).role("group").label(format!("{} {year}", MONTHS[month as usize - 1]))
        .w_full().gap(2.0).transition(Transition::opacity());
    for week in 0..6 {
        let mut line = row().w_full();
        for col_ in 0..7 {
            let n = week * 7 + col_ - first + 1;
            let (day, inside) = if n < 1 { (prev_len + n, false) } else if n > len { (n - len, false) } else { (n, true) };
            let today = inside && (year, month, day as u32) == (ty, tm, td);
            let color = if today { ink } else if inside { fg } else { fg.alpha(0.25 * fg.a) };
            let mut dot = el().size(28.0, 28.0).center().radius(s.cfg.radius(14.0))
                .child(text(day.to_string()).font(s.cfg.font(13.0, if today { 700.0 } else { 500.0 }).features("'tnum' 1")).color(color));
            if today { dot = dot.fill(fg).label("Today") }
            line = line.child(el().key(format!("day-{week}-{col_}")).w(0.0).grow(1.0).h(28.0).center().child(dot));
        }
        grid = grid.child(line);
    }

    col().key("calendar").w_full().h_full().px(16.0).pt(14.0).pb(14.0).gap(8.0)
        .child(header)
        .child(names)
        .child(grid)
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
