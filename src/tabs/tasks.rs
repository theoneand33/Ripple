//! Tasks: a to-do list. The controls under the field set a task's priority, due date and time,
//! and repeat; press a task to edit it. Ticking a task off removes it, or moves a recurring one
//! to its next due date; a due task raises a reminder. A task is saved as one line of text
//! (`!! Title due:2026-11-25T17:00 every:week`). With a Nextcloud account the list syncs with
//! Nextcloud Tasks, where these are PRIORITY, DUE and RRULE. A new task's title may say when it
//! is due in words ("call Bob tomorrow 9am", "pay rent friday", "standup at 9:30").

use rune::time::LocalTime;
use rune::{El, Transition, Tx, col, el, row, text};

use super::{Scrolls, accent, button, checkbox, chip_button, empty, field, tool};
use crate::reminders::{Reminder, Source};
use crate::config::Item;
use crate::date::{add_days, format_date};
use crate::task::{Date, Every, Task, is_bangs, now_key};
use crate::icons;
use crate::island::Island;
use crate::nextcloud::{self, Kind};

/// A time of day as typed: `9am`, `9:30pm`, `17:00`, `noon`. A bare number is no time.
fn clock_time(w: &str) -> Option<(u32, u32)> {
    if w == "noon" { return Some((12, 0)) }
    let (w, half) = match w.strip_suffix("am") { Some(r) => (r, Some(0)), None => match w.strip_suffix("pm") { Some(r) => (r, Some(12)), None => (w, None) } };
    let (h, m) = match w.split_once(':') { Some((h, m)) if m.len() == 2 => (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?), None if half.is_some() => (w.parse::<u32>().ok()?, 0), _ => return None };
    match half {
        Some(add) if (1..=12).contains(&h) && m < 60 => Some((h % 12 + add, m)),
        None if h < 24 && m < 60 => Some((h, m)),
        _ => None,
    }
}

/// Sunday is 0, as `date::weekday` counts. "Sun", "wed" and "sat" are words too: only spelled out.
fn weekday_named(w: &str) -> Option<i64> {
    const NAMES: [&[&str]; 7] = [&["sunday"], &["monday", "mon"], &["tuesday", "tue", "tues"], &["wednesday"], &["thursday", "thu", "thurs"], &["friday", "fri"], &["saturday"]];
    NAMES.iter().position(|n| n.contains(&w)).map(|i| i as i64)
}

/// A new task's title with the words that say when it is due taken out, and that due day and
/// time: `today`, `tonight`, `tomorrow`, a weekday (the next one, `next`/`on`/`this` before it
/// allowed), `in 3 days` or `in 2 weeks`, and a time (`9am`, `9:30pm`, `17:00`, `noon`, `at 5`).
/// A time with no day is today, or tomorrow once that time has passed (`now`, minutes since
/// midnight). Nothing found, or nothing left of the title, leaves it as it is.
fn natural_due(title: &str, today: Date, now: u32) -> (String, Option<(Date, Option<(u32, u32)>)>) {
    let words: Vec<&str> = title.split_whitespace().collect();
    let lower: Vec<String> = words.iter().map(|w| w.to_lowercase().trim_end_matches([',', '.', ';']).to_string()).collect();
    let at = |i: usize| lower.get(i).map(String::as_str);
    let (mut day, mut time, mut tonight) = (None, None, false);
    let mut keep = vec![true; words.len()];
    let mut i = 0;
    while i < words.len() {
        let w = lower[i].as_str();
        // "every monday 9am" is a repeat, which the Repeat control sets: the rest stays in the title.
        if w == "every" { break }
        let mut used = 0;
        if day.is_none() {
            let ahead = |target: i64| { let wd = i64::from(crate::date::weekday(today.0, today.1, today.2)); let n = (target - wd).rem_euclid(7); add_days(today, if n == 0 { 7 } else { n }) };
            (day, used) = match w {
                "today" => (Some(today), 1),
                "tonight" => { tonight = true; (Some(today), 1) }
                "tomorrow" | "tmrw" => (Some(add_days(today, 1)), 1),
                "next" | "on" | "this" if at(i + 1).and_then(weekday_named).is_some() => (at(i + 1).and_then(weekday_named).map(ahead), 2),
                "in" => match (at(i + 1).and_then(|n| n.parse::<i64>().ok()), at(i + 2)) {
                    (Some(n @ 0..=3650), Some("day" | "days")) => (Some(add_days(today, n)), 3),
                    (Some(n @ 0..=520), Some("week" | "weeks")) => (Some(add_days(today, n * 7)), 3),
                    _ => (None, 0),
                },
                _ => match weekday_named(w) { Some(t) => (Some(ahead(t)), 1), None => (None, 0) },
            };
        }
        if used == 0 && time.is_none() {
            // `9 pm` and `9:30 pm`: the word after it says which half of the day.
            let spaced = at(i + 1).filter(|h| matches!(*h, "am" | "pm")).and_then(|half| clock_time(&format!("{w}{half}")));
            (time, used) = match (spaced, clock_time(w), w, at(i + 1)) {
                (Some(t), ..) => (Some(t), 2),
                (None, Some(t), ..) => (Some(t), 1),
                (None, None, "at" | "@", Some(next)) => match at(i + 2).filter(|h| ["am", "pm"].contains(h)).and_then(|half| clock_time(&format!("{next}{half}"))) {
                    Some(t) => (Some(t), 3),
                    None => match clock_time(next) {
                        Some(t) => (Some(t), 2),
                        // `at 5` means 5pm: the working day, not before dawn. Only as the title's last
                        // words, so "look at 3 options" stays as it is.
                        None if i + 2 < words.len() => (None, 0),
                        None => match next.parse::<u32>() { Ok(h @ 1..=7) => (Some((h + 12, 0)), 2), Ok(h @ 8..=23) => (Some((h, 0)), 2), _ => (None, 0) },
                    },
                },
                _ => (None, 0),
            };
        }
        for k in &mut keep[i..i + used] { *k = false }
        // The word that brought it in goes with it: "by friday", "due tomorrow", "on 9am".
        if used > 0 && i > 0 && keep[i - 1] && matches!(lower[i - 1].as_str(), "by" | "due" | "on") { keep[i - 1] = false }
        i += used.max(1);
    }
    // Tonight is 20:00, or no time at all once that has passed.
    if tonight && time.is_none() && now < 20 * 60 { time = Some((20, 0)) }
    let rest: Vec<&str> = words.iter().zip(&keep).filter(|(_, k)| **k).map(|(w, _)| *w).collect();
    if (day.is_none() && time.is_none()) || rest.is_empty() { return (title.to_string(), None) }
    let day = day.unwrap_or_else(|| if time.is_some_and(|(h, m)| h * 60 + m <= now) { add_days(today, 1) } else { today });
    (rest.join(" "), Some((day, time)))
}

/// "Today 17:00", "Tomorrow", "Wed, Nov 25".
fn due_label(task: &Task, today: Date) -> String {
    let Some((day, time)) = task.due else { return String::new() };
    let name = if day == today { "Today".to_string() } else if day == add_days(today, 1) { "Tomorrow".to_string() } else { format_date(day.0, day.1, day.2) };
    match time { Some((h, m)) => format!("{name} {h:02}:{m:02}"), None => name }
}

/// The Tasks tab: the field a task is typed in, its controls, and the last reminder check.
#[derive(Default)]
pub struct Tasks {
    pub field: rune::TextEdit,
    pub draft: TaskDraft,
    /// The minute task reminders were last checked, as `YYYY-MM-DDTHH:MM`; empty until the first.
    mark: String,
    /// The task just ticked, by key, and when it goes: its box shows the tick until then.
    ticked: Option<(u64, f64)>,
}

/// How long a ticked task shows its tick before it goes.
const TICK_HOLD: f64 = 0.18;

/// What the controls under the field hold: the next task's, or the edited task's.
#[derive(Default)]
pub struct TaskDraft {
    pub priority: u8,
    pub due: Option<Date>,
    pub time: Option<(u32, u32)>,
    pub every: Option<Every>,
    /// The task being edited, by key; None adds a new one.
    pub editing: Option<u64>,
}

/// Adds the typed task with the controls' settings, or saves the one being edited in place, so
/// sync writes the change over it on Nextcloud.
fn add_task(s: &mut Island, _: &mut Tx) {
    let d = &s.tasks.draft;
    // Typed `due:tomorrow` and the like still work: read as of today, then saved as dates.
    // Priority comes from its control only, so leading bangs stay in the title.
    let typed = s.tasks.field.text.trim();
    let typed = if typed.split_whitespace().next().is_some_and(is_bangs) { format!("\\{typed}") } else { typed.to_string() };
    // An edit's field holds the title as it is, which may itself contain such words.
    let mut task = if d.editing.is_some() { Task { title: s.tasks.field.text.split_whitespace().collect::<Vec<_>>().join(" "), ..Task::default() } } else { Task::parse(&typed, s.today) };
    if d.editing.is_none() && d.due.is_none() && task.due.is_none() {
        let t = LocalTime::at(crate::date::now_ms());
        let (title, due) = natural_due(&task.title, s.today, t.hour * 60 + t.minute);
        if due.is_some() { (task.title, task.due) = (title, due) }
    }
    if d.priority > 0 { task.priority = d.priority }
    if let Some(day) = d.due { task.due = Some((day, d.time)) }
    if d.every.is_some() { task.every = d.every }
    if task.title.is_empty() { return }
    let text = task.to_string();
    let editing = s.tasks.draft.editing;
    reset_draft(s);
    s.save_item(Kind::Tasks, editing, text);
}

/// As the tab opens: an edit left open is dropped, and the list syncs.
pub(super) fn entered(s: &mut Island) {
    if s.tasks.draft.editing.is_some() { reset_draft(s) }
    s.nextcloud_sync(Kind::Tasks);
}

fn reset_draft(s: &mut Island) {
    s.tasks.draft = TaskDraft::default();
    s.tasks.field.clear();
}

fn start_edit(s: &mut Island, key: u64) {
    let Some(task) = s.cfg.tasks.iter().find(|t| t.key == key).map(|t| Task::parse(&t.text, s.today)) else { return };
    s.tasks.field.replace_text(task.title.clone());
    s.tasks.field.focused = true;
    s.tasks.draft = TaskDraft { priority: task.priority, due: task.due.map(|d| d.0), time: task.due.and_then(|d| d.1), every: task.every, editing: Some(key) };
}

/// The time 30 minutes either way, round the clock.
fn step_time((h, m): (u32, u32), by: i32) -> (u32, u32) {
    let t = (h as i32 * 60 + m as i32 + by * 30).rem_euclid(1440);
    ((t / 60) as u32, (t % 60) as u32)
}

/// By key, not index: a sync may have removed a task above it since the list was drawn. A
/// recurring task comes back as a new one, due next time; Nextcloud marks this one done.
/// A press on a task's box fills it; the task goes `TICK_HOLD` later, in `tick_due`.
fn tick(s: &mut Island, key: u64, tx: &mut Tx) {
    // Done here, a rule only Nextcloud follows would end or come back wrong.
    let custom = s.cfg.tasks.iter().find(|t| t.key == key).is_some_and(|t| Task::parse(&t.text, s.today).every == Some(Every::Custom));
    if custom && s.cfg.nextcloud.enabled() {
        s.nextcloud[Kind::Tasks].error = Some("This task repeats by a rule only Nextcloud follows: tick it there".into());
        return;
    }
    // A second tick lands the first at once.
    match s.tasks.ticked.take() {
        Some((held, at)) if held == key => { s.tasks.ticked = Some((held, at)); return }
        Some((held, _)) => finish(s, held),
        None => {}
    }
    let at = tx.clock() + TICK_HOLD;
    s.tasks.ticked = Some((key, at));
    tx.wake_at(at);
}

/// The next time a ticked task goes.
pub(super) fn tick_deadline(s: &Island) -> Option<f64> { s.tasks.ticked.map(|t| t.1) }

/// Removes the ticked task once its tick has shown. True if it did.
pub(super) fn tick_due(s: &mut Island, now: f64, tx: &mut Tx) -> bool {
    let Some((key, _)) = s.tasks.ticked.filter(|t| now >= t.1) else { return false };
    s.tasks.ticked = None;
    // A wake's commit lands at once: this one lets the row fade out, as the press would have.
    tx.animated();
    finish(s, key);
    true
}

/// Ticks the task off: removes it, or puts its next repeat in its place.
fn finish(s: &mut Island, key: u64) {
    if s.tasks.draft.editing == Some(key) { reset_draft(s) }
    let today = s.today;
    s.save(|c| {
        if let Some((i, item)) = nextcloud::remove(c, Kind::Tasks, key)
            && let Some(next) = Task::parse(&item.text, today).next(today) {
            c.tasks.insert(i.min(c.tasks.len()), Item::new(next.to_string()));
        }
    });
    s.nextcloud_sync(Kind::Tasks);
}

/// The tasks due by `now` (`YYYY-MM-DDTHH:MM`) that `reminded` doesn't hold yet, as their
/// reminder id (`key@due`, so a task moved to a new time reminds again) and title.
fn due_reminders(tasks: &[Item], reminded: &[String], now: &str, today: Date) -> Vec<(String, String)> {
    tasks.iter().filter_map(|item| {
        let task = Task::parse(&item.text, today);
        let due = task.due_key().filter(|k| k.as_str() <= now)?;
        let id = format!("{}@{due}", item.key);
        (!reminded.contains(&id)).then_some((id, task.title))
    }).collect()
}

impl Island {
    /// Once a minute: a reminder for the tasks that fell due and haven't had one, including those
    /// that fell due while Ripple wasn't running.
    pub fn task_reminders(&mut self) {
        // A headless capture draws its scene's tasks; recording them as reminded would save
        // the stored settings over the scene.
        if rune::is_headless() { return }
        let mark = now_key();
        if std::mem::replace(&mut self.tasks.mark, mark.clone()) == mark { return }
        let due = due_reminders(&self.cfg.tasks, &self.cfg.reminded_tasks, &mark, self.today);
        if due.is_empty() { return }
        self.save(|c| {
            // Ids of tasks gone or moved since are dropped.
            let live: Vec<String> = c.tasks.iter().map(|i| format!("{}@", i.key)).collect();
            c.reminded_tasks.retain(|id| live.iter().any(|k| id.starts_with(k)));
            c.reminded_tasks.extend(due.iter().map(|d| d.0.clone()));
        });
        for (_, title) in due { self.offer(Reminder { source: Source::Task, title, link: None }) }
    }
}

fn priority_color(p: u8) -> rune::Color {
    match p { 3 => accent::red(), 2 => accent::orange(), _ => accent::blue() }
}

pub fn view(s: &Island) -> El {
    let fg = s.cfg.text();
    let muted = fg.alpha(0.55 * fg.a);
    let mut list = col().key("task-list").w_full().gap(6.0);
    if let Some(e) = &s.nextcloud[Kind::Tasks].error {
        list = list.child(text(e).font(s.cfg.font(12.0, 400.0)).color(muted).wrap_anywhere());
    }
    if s.cfg.tasks.is_empty() { list = list.child(empty(s, "No tasks yet")) }
    let now = now_key();
    let mut tasks: Vec<_> = s.cfg.tasks.iter().map(|i| (i.key, Task::parse(&i.text, s.today))).collect();
    tasks.sort_by_key(|(_, t)| (std::cmp::Reverse(t.priority), t.due_key().unwrap_or_else(|| "~".into())));
    for (key, task) in tasks {
        let editing = s.tasks.draft.editing == Some(key);
        let overdue = task.due_key().is_some_and(|k| !rune::is_headless() && k <= now);
        let mut meta = row().gap(8.0).center_y();
        if task.due.is_some() {
            meta = meta.child(text(due_label(&task, s.today)).font(s.cfg.font(12.0, 600.0)).color(if overdue { accent::red() } else { muted }));
        }
        if let Some(every) = task.every {
            let label = if every == Every::Custom { "↻ repeats (set in Nextcloud)".to_string() } else { format!("↻ every {}", every.name()) };
            meta = meta.child(text(label).font(s.cfg.font(12.0, 500.0)).color(muted));
        }
        let mut body = col().key("edit").role("button").label(format!("Edit {}", task.title)).focusable(true).grow(1.0).min_w(0.0).gap(2.0)
            .on_press(move |s: &mut Island| start_edit(s, key))
            .child(text(&task.title).font(s.cfg.font(14.0, 500.0)).color(fg).wrap_anywhere());
        if task.due.is_some() || task.every.is_some() { body = body.child(meta) }
        let mut line = row().key(format!("task-{key}")).w_full().px(12.0).py(8.0).gap(10.0).center_y()
            .radius(s.cfg.radius(16.0)).fill(super::card(fg))
            .transition(Transition::opacity().combined(Transition::scale(0.97)))
            .child(checkbox(s, "done", &format!("Done: {}", task.title), s.tasks.ticked.is_some_and(|t| t.0 == key)).on_press_tx(move |s: &mut Island, tx| tick(s, key, tx)))
            .child(body);
        if editing { line = line.border(1.5, fg.alpha(0.6 * fg.a)) }
        if task.priority > 0 {
            let tint = priority_color(task.priority);
            line = line.child(el().key("priority").role("img").label(format!("Priority {}", ["", "low", "medium", "high"][task.priority as usize]))
                .h(22.0).px(8.0).center().radius(s.cfg.radius(11.0)).fill(tint.alpha(0.22))
                .child(text("!".repeat(task.priority.into())).font(s.cfg.font(12.0, 800.0)).color(tint).a11y_hidden()));
        }
        list = list.child(line);
    }
    let editing = s.tasks.draft.editing.is_some();
    let mut input = row().key("task-input").w_full().gap(8.0).center_y()
        .child(field(s, "new-task", &s.tasks.field, if editing { "Task title" } else { "New task, like call Bob tomorrow 9am" }, |s: &mut Island| &mut s.tasks.field, add_task));
    if editing { input = input.child(tool(s, "cancel-edit", &icons::X, "Cancel edit").on_press(reset_draft)) }
    col().key("tasks").w_full().h_full().px(16.0).pt(18.0).pb(14.0).gap(10.0)
        .child(col().key("task-scroll").w_full().grow(1.0).min_h(0.0).scrolls().child(list))
        .child(input.child(button(s, "add-task", if editing { "Save" } else { "Add" }, true).on_press_tx(add_task)))
        .child(controls(s))
}

fn toggle(s: &Island, key: &str, label: &str, tint: rune::Color, on: bool, press: impl Fn(&mut Island) + 'static) -> El {
    chip_button(s, key, label, tint, on).role("button").label(label.to_string()).on_press(press)
}

/// A value with arrows either side, shown while it is set.
fn stepper(s: &Island, key: &'static str, middle: El, set: bool, step: fn(&mut Island, i32)) -> El {
    let mut r = row().key(key).gap(2.0).center_y();
    if set { r = r.child(tool(s, "earlier", &icons::CHEVRON_LEFT, "Earlier").on_press(move |s: &mut Island| step(s, -1))) }
    r = r.child(middle);
    if set { r = r.child(tool(s, "later", &icons::CHEVRON_RIGHT, "Later").on_press(move |s: &mut Island| step(s, 1))) }
    r
}

/// Priority, due date and time, and repeat, for the task being written or edited.
fn controls(s: &Island) -> El {
    let d = &s.tasks.draft;
    let priority = ["Priority", "Low", "Medium", "High"][usize::from(d.priority)];
    let mut row_ = row().key("task-controls").w_full().gap(6.0).center_y()
        .child(toggle(s, "task-priority", priority, priority_color(d.priority), d.priority > 0, |s| s.tasks.draft.priority = (s.tasks.draft.priority + 1) % 4));
    let date = d.due.map_or("Date".to_string(), |day| due_label(&Task { due: Some((day, None)), ..Task::default() }, s.today));
    row_ = row_.child(stepper(s, "task-date",
        toggle(s, "task-due", &date, accent::teal(), d.due.is_some(), |s| {
            let d = &mut s.tasks.draft;
            (d.due, d.time) = if d.due.is_some() { (None, None) } else { (Some(s.today), None) };
        }),
        d.due.is_some(), |s, by| if let Some(day) = s.tasks.draft.due { s.tasks.draft.due = Some(add_days(day, by.into())) }));
    if d.due.is_some() {
        let time = d.time.map_or("Time".to_string(), |(h, m)| format!("{h:02}:{m:02}"));
        row_ = row_.child(stepper(s, "task-time",
            toggle(s, "task-at", &time, accent::teal(), d.time.is_some(), |s| s.tasks.draft.time = if s.tasks.draft.time.is_some() { None } else { Some((9, 0)) }),
            d.time.is_some(), |s, by| if let Some(t) = s.tasks.draft.time { s.tasks.draft.time = Some(step_time(t, by)) }));
    }
    let every = match d.every { None => "Repeat", Some(Every::Day) => "Daily", Some(Every::Week) => "Weekly", Some(Every::Month) => "Monthly", Some(Every::Custom) => "Custom" };
    row_.child(toggle(s, "task-repeat", every, accent::indigo(), d.every.is_some(), |s| {
        s.tasks.draft.every = match s.tasks.draft.every { None => Some(Every::Day), Some(Every::Day) => Some(Every::Week), Some(Every::Week) => Some(Every::Month), Some(Every::Month) | Some(Every::Custom) => None };
    }))
}

#[cfg(test)]
mod tests {
    use super::{Every, Task};

    #[test]
    fn a_ticked_task_shows_its_tick_then_goes() {
        let cfg = crate::config::Config { tasks: vec![super::Item::new("a"), super::Item::new("b")], ..Default::default() };
        let mut s = super::Island::new(crate::config::Store::memory(cfg), Vec::new());
        let (a, b) = (s.cfg.tasks[0].key, s.cfg.tasks[1].key);
        let mut tx = rune::Tx::default();
        super::tick(&mut s, a, &mut tx);
        assert_eq!(s.cfg.tasks.len(), 2, "it stays while its tick shows");
        assert!(!super::tick_due(&mut s, tx.clock(), &mut tx), "not yet");
        super::tick(&mut s, b, &mut tx);
        assert_eq!(s.cfg.tasks.iter().map(|t| t.key).collect::<Vec<_>>(), [b], "a second tick lands the first");
        assert!(super::tick_due(&mut s, tx.clock() + 1.0, &mut tx));
        assert!(s.cfg.tasks.is_empty());
    }

    #[test]
    fn a_title_says_when_a_task_is_due_in_words() {
        // 2026-11-20 is a Friday; it is 10:00.
        let due = |t: &str| super::natural_due(t, (2026, 11, 20), 600);
        assert_eq!(due("call Bob tomorrow 9am"), ("call Bob".into(), Some(((2026, 11, 21), Some((9, 0))))));
        assert_eq!(due("Pay rent on monday"), ("Pay rent".into(), Some(((2026, 11, 23), None))));
        assert_eq!(due("Review friday"), ("Review".into(), Some(((2026, 11, 27), None))), "today's weekday is next week's");
        assert_eq!(due("standup at 9:30"), ("standup".into(), Some(((2026, 11, 21), Some((9, 30))))), "passed today, so tomorrow");
        assert_eq!(due("Call mom at 5"), ("Call mom".into(), Some(((2026, 11, 20), Some((17, 0))))));
        assert_eq!(due("Call Bob at 9 am"), ("Call Bob".into(), Some(((2026, 11, 21), Some((9, 0))))));
        assert_eq!(due("Call mom at 9 pm"), ("Call mom".into(), Some(((2026, 11, 20), Some((21, 0))))));
        assert_eq!(due("Dinner tonight"), ("Dinner".into(), Some(((2026, 11, 20), Some((20, 0))))));
        assert_eq!(due("Gym 6:30 pm"), ("Gym".into(), Some(((2026, 11, 20), Some((18, 30))))));
        assert_eq!(due("Lunch 12 pm today"), ("Lunch".into(), Some(((2026, 11, 20), Some((12, 0))))));
        assert_eq!(due("Renew passport in 2 weeks"), ("Renew passport".into(), Some(((2026, 12, 4), None))));
        assert_eq!(due("Midnight snack 12am tomorrow").1, Some(((2026, 11, 21), Some((0, 0)))));
        assert_eq!(super::natural_due("Dinner tonight", (2026, 11, 20), 21 * 60), ("Dinner".into(), Some(((2026, 11, 20), None))), "no time already passed");
        assert_eq!(due("Call Bob in -200000000000 days"), ("Call Bob in -200000000000 days".into(), None));
        assert_eq!(due("Water plants every monday"), ("Water plants every monday".into(), None));
        assert_eq!(due("Water plants every monday 9am"), ("Water plants every monday 9am".into(), None));
        assert_eq!(due("Submit report by friday"), ("Submit report".into(), Some(((2026, 11, 27), None))));
        assert_eq!(due("Pay rent due tomorrow"), ("Pay rent".into(), Some(((2026, 11, 21), None))));
        assert_eq!(due("Look at 3 options"), ("Look at 3 options".into(), None));
        for plain in ["Read Monday's paper", "Buy 3 apples", "Tomorrow", "Fix bug 1:5", "Buy sun cream", "Sat exam prep"] {
            assert_eq!(due(plain), (plain.into(), None), "{plain}");
        }
    }

    #[test]
    fn the_controls_write_a_task_and_an_edit_keeps_its_key() {
        use crate::config::{Config, Store};
        use crate::island::Island;
        let mut s = Island::new(Store::memory(Config::default()), Vec::new());
        s.today = (2026, 1, 1);
        s.tasks.field.replace_text("  Pay   rent ");
        s.tasks.draft = super::TaskDraft { priority: 2, due: Some((2026, 1, 31)), time: Some(super::step_time((0, 0), -1)), every: Some(Every::Month), editing: None };
        super::add_task(&mut s, &mut rune::Tx::default());
        assert_eq!(s.cfg.tasks[0].text, "!! Pay rent due:2026-01-31T23:30 every:month");
        assert!(s.tasks.field.text.is_empty() && s.tasks.draft.priority == 0, "the controls reset");
        let key = s.cfg.tasks[0].key;
        super::start_edit(&mut s, key);
        assert_eq!((s.tasks.field.text.as_str(), s.tasks.draft.time, s.tasks.draft.editing), ("Pay rent", Some((23, 30)), Some(key)));
        s.tasks.draft.every = None;
        super::add_task(&mut s, &mut rune::Tx::default());
        assert_eq!((s.cfg.tasks.len(), s.cfg.tasks[0].key, s.cfg.tasks[0].text.as_str()), (1, key, "!! Pay rent due:2026-01-31T23:30"));
        s.tasks.field.replace_text("Call Bob due:tomorrow");
        super::add_task(&mut s, &mut rune::Tx::default());
        assert_eq!(s.cfg.tasks[1].text, "Call Bob due:2026-01-02", "a typed relative day is saved as its date");
        s.tasks.field.replace_text("Call Ann tomorrow 9am");
        super::add_task(&mut s, &mut rune::Tx::default());
        assert_eq!(s.cfg.tasks[2].text, "Call Ann due:2026-01-02T09:00", "a day said in words too");
        s.save(|c| { c.tasks.pop(); });
        let bob = s.cfg.tasks[1].key;
        super::start_edit(&mut s, bob);
        s.save(|c| c.tasks.retain(|t| t.key != bob));
        s.tasks.field.replace_text("Call Bob back");
        super::add_task(&mut s, &mut rune::Tx::default());
        assert_eq!(s.cfg.tasks[1].text, "Call Bob back due:2026-01-02", "an edit of a task gone meanwhile is kept as a new one");
        let read = s.cfg.tasks[1].key;
        super::start_edit(&mut s, read);
        s.tasks.field.replace_text("Read every:week article");
        super::add_task(&mut s, &mut rune::Tx::default());
        assert_eq!(Task::parse(&s.cfg.tasks[1].text, s.today).title, "Read every:week article", "an edit's title is taken as typed");
        s.tasks.field.replace_text("! call mom");
        super::add_task(&mut s, &mut rune::Tx::default());
        assert_eq!(Task::parse(&s.cfg.tasks.last().unwrap().text, s.today), Task { title: "! call mom".into(), ..Task::default() });
    }

    #[test]
    fn reminders_come_once_for_each_due_time_even_if_missed() {
        use crate::config::Item;
        let tasks = vec![Item::new("Missed yesterday due:2026-01-01"), Item::new("Later due:2026-01-02T18:00"), Item::new("No date")];
        let due = super::due_reminders(&tasks, &[], "2026-01-02T12:00", (2026, 1, 2));
        assert_eq!(due.iter().map(|d| d.1.as_str()).collect::<Vec<_>>(), ["Missed yesterday"]);
        let reminded = vec![due[0].0.clone()];
        assert!(super::due_reminders(&tasks, &reminded, "2026-01-02T12:01", (2026, 1, 2)).is_empty(), "once only");
        assert_eq!(super::due_reminders(&tasks, &reminded, "2026-01-02T18:00", (2026, 1, 2)).len(), 1);
        let moved = vec![Item { text: "Missed yesterday due:2026-01-02T11:00".into(), ..tasks[0].clone() }];
        assert_eq!(super::due_reminders(&moved, &reminded, "2026-01-02T12:00", (2026, 1, 2)).len(), 1, "a new due time reminds again");
    }

    #[test]
    fn a_custom_repeat_is_ticked_in_nextcloud_not_here() {
        use crate::config::{Config, Item, Nextcloud, Store};
        use crate::island::Island;
        let nextcloud = Nextcloud { url: "https://cloud.example.com".into(), user: "me".into(), ..Nextcloud::default() };
        let mut s = Island::new(Store::memory(Config { tasks: vec![Item::new("Bins due:2026-01-01 every:custom")], nextcloud, ..Config::default() }), Vec::new());
        let key = s.cfg.tasks[0].key;
        super::tick(&mut s, key, &mut rune::Tx::default());
        assert_eq!(s.cfg.tasks.len(), 1);
        assert!(s.nextcloud[crate::nextcloud::Kind::Tasks].error.is_some());
    }

    #[test]
    fn ticking_a_recurring_task_brings_it_back_due_next_time() {
        use crate::config::{Config, Item, Store};
        use crate::island::Island;
        let mut s = Island::new(Store::memory(Config { tasks: vec![Item::new("Water plants due:2026-01-01 every:week"), Item::new("Once")], ..Config::default() }), Vec::new());
        s.today = (2026, 1, 1);
        let (weekly, once) = (s.cfg.tasks[0].key, s.cfg.tasks[1].key);
        super::finish(&mut s, weekly);
        super::finish(&mut s, once);
        let texts: Vec<_> = s.cfg.tasks.iter().map(|t| t.text.as_str()).collect();
        assert_eq!(texts, ["Water plants due:2026-01-08 every:week"]);
    }
}
