//! Habits: things to do every day. Each one shows the last seven days to tick off and its
//! streak of days in a row.

use rune::{El, TextEdit, Tx, col, el, icon, row, text};

use super::{Scrolls, accent, button, empty, field, tool};
use crate::config::Habit;
use crate::date::{days_from_civil, weekday};
use crate::icons;
use crate::island::Island;

/// The Habits tab: the field a new habit is typed in.
#[derive(Default)]
pub struct Habits {
    pub field: TextEdit,
}

/// Days in a row done, up to today; a day not yet done today doesn't break it.
pub fn streak(done: &[i64], today: i64) -> u32 {
    let start = if done.contains(&today) { today } else { today - 1 };
    (0..).take_while(|i| done.contains(&(start - i))).count() as u32
}

fn add(s: &mut Island, _: &mut Tx) {
    let name = s.habits.field.text.trim().to_string();
    if name.is_empty() { return }
    s.save(|c| c.habits.push(Habit { name, done: Vec::new() }));
    s.habits.field.clear();
}

fn toggle(s: &mut Island, i: usize, day: i64) {
    s.save(|c| if let Some(h) = c.habits.get_mut(i) {
        match h.done.iter().position(|d| *d == day) {
            Some(p) => { h.done.remove(p); }
            None => { h.done.push(day); h.done.sort_unstable(); }
        }
    });
}

pub fn view(s: &Island) -> El {
    let fg = s.cfg.text();
    let (y, m, d) = s.today;
    let today = days_from_civil(y, m, d);
    let wd = i64::from(weekday(y, m, d));
    let mut list = col().key("habit-list").w_full().gap(6.0);
    if s.cfg.habits.is_empty() { list = list.child(empty(s, "No habits yet")) }
    let tone = accent::green();
    for (i, h) in s.cfg.habits.iter().enumerate() {
        let mut week = row().key("week").gap(4.0).center_y();
        for back in (0..7).rev() {
            let day = today - back;
            let on = h.done.contains(&day);
            let letter = crate::date::WEEKDAY_LETTERS[(wd - back).rem_euclid(7) as usize];
            week = week.child(
                el().key(format!("day-{back}")).role("checkbox").checked(on).focusable(true)
                    .label(format!("{}: {}", h.name, if back == 0 { "today".into() } else { format!("{back} days ago") }))
                    .size(24.0, 24.0).center().radius(s.cfg.radius(12.0))
                    .fill(if on { tone.alpha(0.85) } else { fg.alpha(0.08 * fg.a) }).hover(tone.alpha(0.4))
                    .border(if back == 0 { 1.5 } else { 0.0 }, tone)
                    .on_press(move |s: &mut Island| toggle(s, i, day))
                    .child(text(letter).font(s.cfg.font(11.0, 700.0)).color(if on { s.cfg.bg().alpha(1.0) } else { fg.alpha(0.55 * fg.a) }).a11y_hidden()),
            );
        }
        let n = streak(&h.done, today);
        let flame = if n > 0 { accent::orange() } else { fg.alpha(0.35 * fg.a) };
        list = list.child(
            row().key(format!("habit-{i}")).w_full().px(12.0).py(8.0).gap(10.0).center_y()
                .radius(s.cfg.radius(16.0)).fill(super::card(fg))
                .child(text(&h.name).font(s.cfg.font(14.0, 500.0)).color(fg).grow(1.0).min_w(0.0).wrap_anywhere())
                .child(week)
                .child(row().key("streak").role("img").label(format!("{n} day streak")).w(38.0).gap(3.0).center_y()
                    .child(icon(&icons::FLAME, 14.0).color(flame).a11y_hidden())
                    .child(text(n.to_string()).font(s.cfg.font(13.0, 700.0).features("'tnum' 1")).color(flame).a11y_hidden()))
                .child(tool(s, "remove", &icons::TRASH, &format!("Remove {}", h.name))
                    .on_press(move |s: &mut Island| s.save(|c| if i < c.habits.len() { c.habits.remove(i); })))
        );
    }
    col().key("habits").w_full().h_full().px(16.0).pt(18.0).pb(14.0).gap(10.0)
        .child(col().key("habit-scroll").w_full().grow(1.0).min_h(0.0).scrolls().child(list))
        .child(
            row().key("habit-input").w_full().gap(8.0).center_y()
                .child(field(s, "new-habit", &s.habits.field, "New habit…", |s: &mut Island| &mut s.habits.field, add))
                .child(button(s, "add-habit", "Add", true).on_press_tx(add)),
        )
}

#[cfg(test)]
mod tests {
    use super::streak;

    #[test]
    fn streaks_count_back_from_today_or_yesterday() {
        assert_eq!(streak(&[], 10), 0);
        assert_eq!(streak(&[8, 9, 10], 10), 3);
        // Today not done yet keeps yesterday's run.
        assert_eq!(streak(&[7, 8, 9], 10), 3);
        assert_eq!(streak(&[5, 7, 8], 10), 0);
        assert_eq!(streak(&[4, 6, 7, 8, 9, 10], 10), 5);
    }
}
