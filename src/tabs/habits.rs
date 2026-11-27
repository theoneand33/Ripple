//! Habits: things to do every day. Only today can be ticked off; the past week shows as
//! history beside the streak of days in a row.

use rune::{El, TextEdit, Tx, col, el, icon, row, text};

use super::{Scrolls, accent, button, empty, field, tool};
use crate::config::Habit;
use crate::date::{days_from_civil, streak, weekday};
use crate::icons;
use crate::island::Island;

/// The Habits tab: the field a new habit is typed in.
#[derive(Default)]
pub struct Habits {
    pub field: TextEdit,
}

fn add(s: &mut Island, _: &mut Tx) {
    let name = s.habits.field.text.trim().to_string();
    if name.is_empty() { return }
    s.save(|c| c.habits.push(Habit { name, done: Vec::new() }));
    s.habits.field.clear();
}

/// Ticks or unticks today; past days are history and can't be changed.
fn toggle_today(s: &mut Island, i: usize) {
    let (y, m, d) = s.today;
    let today = days_from_civil(y, m, d);
    s.save(|c| if let Some(h) = c.habits.get_mut(i) {
        match h.done.iter().position(|d| *d == today) {
            Some(p) => { h.done.remove(p); }
            None => { h.done.push(today); h.done.sort_unstable(); }
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
        let done = h.done.contains(&today);
        let check = el().key("today").role("checkbox").checked(done).focusable(true)
            .label(format!("{}: done today", h.name))
            .size(30.0, 30.0).center().radius(s.cfg.radius(15.0))
            .fill(if done { tone } else { fg.alpha(0.06 * fg.a) }).hover(tone.alpha(if done { 0.8 } else { 0.3 }))
            .border(if done { 0.0 } else { 1.5 }, fg.alpha(0.25 * fg.a))
            .on_press(move |s: &mut Island| toggle_today(s, i))
            .child(icon(&icons::CHECK, 16.0).color(if done { s.cfg.bg().alpha(1.0) } else { fg.alpha(0.0) }).a11y_hidden());

        let n = streak(&h.done, today);
        let flame = if n > 0 { accent::orange() } else { fg.alpha(0.4 * fg.a) };
        let sub = match n { 0 => "No streak".to_string(), 1 => "1 day".to_string(), n => format!("{n} days") };
        let title = col().key("title").grow(1.0).min_w(0.0).gap(2.0)
            .child(text(&h.name).font(s.cfg.font(14.0, 500.0)).color(if done { fg.alpha(0.6 * fg.a) } else { fg }).wrap_anywhere())
            .child(row().key("streak").gap(3.0).center_y()
                .child(icon(&icons::FLAME, 12.0).color(flame).a11y_hidden())
                .child(text(sub).font(s.cfg.font(12.0, 600.0).features("'tnum' 1")).color(flame)));

        // The six days before today, read-only.
        let past = (1..7).filter(|b| h.done.contains(&(today - b))).count();
        let mut week = row().key("week").role("img").label(format!("Done {past} of the last 6 days")).gap(5.0).center_y();
        for back in (1..7).rev() {
            let on = h.done.contains(&(today - back));
            let letter = crate::date::WEEKDAY_LETTERS[(wd - back).rem_euclid(7) as usize];
            week = week.child(col().key(format!("day-{back}")).gap(3.0).center().a11y_hidden()
                .child(text(letter).font(s.cfg.font(10.0, 600.0)).color(fg.alpha(0.4 * fg.a)))
                .child(el().size(8.0, 8.0).radius(4.0).fill(if on { tone.alpha(0.85) } else { fg.alpha(0.12 * fg.a) })));
        }

        list = list.child(
            row().key(format!("habit-{i}")).w_full().px(10.0).py(8.0).gap(12.0).center_y()
                .radius(s.cfg.radius(16.0)).fill(super::card(fg))
                .child(check)
                .child(title)
                .child(week)
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
