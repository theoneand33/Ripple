//! Tasks: a to-do list. The controls under the field set a task's priority, due date and time,
//! and repeat; press a task to edit it. Ticking a task off removes it, or moves a recurring one
//! to its next due date; a due task raises a reminder. A task is saved as one line of text
//! (`!! Title due:2026-11-25T17:00 every:week`). With a Nextcloud account the list syncs with
//! Nextcloud Tasks, where these are PRIORITY, DUE and RRULE. A new task's title may say when it
//! is due in words ("call Bob tomorrow 9am", "pay rent friday", "standup at 9:30").

use rune::time::LocalTime;
use rune::{El, Transition, Tx, col, el, row, text};

use super::{Scrolls, accent, button, checkbox, chip, empty, field, tool};
use crate::config::Item;
use crate::date::{days_from_civil, days_in, format_date};
use crate::icons;
use crate::island::Island;
use crate::nextcloud::{self, Kind};

pub type Date = (i32, u32, u32);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Every {
    Day,
    Week,
    Month,
    /// A Nextcloud rule Ripple can't follow (an interval, set days, yearly): left as it is there.
    Custom,
}

impl Every {
    pub fn name(self) -> &'static str {
        match self { Every::Day => "day", Every::Week => "week", Every::Month => "month", Every::Custom => "custom" }
    }

    fn parse(s: &str) -> Option<Every> {
        match s.to_ascii_lowercase().as_str() {
            "day" | "daily" => Some(Every::Day),
            "week" | "weekly" => Some(Every::Week),
            "month" | "monthly" => Some(Every::Month),
            "custom" => Some(Every::Custom),
            _ => None,
        }
    }
}

/// A task's text, read.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Task {
    /// 0 for none, up to 3 for `!!!`.
    pub priority: u8,
    pub title: String,
    /// The day, and the time on this clock if it has one.
    pub due: Option<(Date, Option<(u32, u32)>)>,
    pub every: Option<Every>,
}

fn shift_days((y, m, d): Date, days: i64) -> Date {
    let t = LocalTime::at_offset((days_from_civil(y, m, d) + days) * 86_400_000, 0);
    (t.year, t.month, t.day)
}

impl Task {
    /// Reads a task's text; `today` is what `due:today` and `due:tomorrow` mean.
    pub fn parse(text: &str, today: Date) -> Task {
        let mut task = Task::default();
        let mut title = Vec::new();
        for (i, word) in text.split_whitespace().enumerate() {
            if i == 0 && is_bangs(word) { task.priority = word.len() as u8; continue }
            // `Display` escapes title words that would read as settings.
            if let Some(word) = word.strip_prefix('\\').filter(|w| setting(w, title.is_empty())) { title.push(word); continue }
            if let Some(due) = word.strip_prefix("due:").and_then(|v| parse_due(v, today)) { task.due = Some(due); continue }
            if let Some(every) = word.strip_prefix("every:").and_then(Every::parse) { task.every = Some(every); continue }
            title.push(word);
        }
        task.title = title.join(" ");
        task
    }

    /// The due time reminders and order go by: a day without a time is due at 9:00.
    pub fn due_key(&self) -> Option<String> {
        let ((y, m, d), time) = self.due?;
        let (h, min) = time.unwrap_or((9, 0));
        Some(format!("{y:04}-{m:02}-{d:02}T{h:02}:{min:02}"))
    }

    /// The next one of a recurring task, due the first repeat after its due day that is not
    /// before today; one with no due day is due a repeat from today.
    pub fn next(&self, today: Date) -> Option<Task> {
        let every = self.every?;
        let (from, time) = self.due.unwrap_or((today, None));
        // The nth repeat counts from the due day itself, so the 31st repeats on each month's last day without drifting.
        let nth = |n: i32| match every {
            Every::Day => shift_days(from, n.into()),
            Every::Week => shift_days(from, i64::from(n) * 7),
            Every::Month => { let (y, m) = crate::date::shift_month(from.0, from.1, n); (y, m, from.2.min(days_in(y, m))) }
            Every::Custom => from,
        };
        if every == Every::Custom { return None }
        let day = (1..).map(nth).find(|day| *day >= today)?;
        Some(Task { due: Some((day, time)), ..self.clone() })
    }
}

impl std::fmt::Display for Task {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.priority > 0 { write!(f, "{} ", "!".repeat(self.priority.into()))? }
        // Title words that would read back as settings (`!`, `due:…`, `every:…`) are escaped.
        for (i, word) in self.title.split_whitespace().enumerate() {
            if i > 0 { f.write_str(" ")? }
            if setting(word, i == 0) { f.write_str("\\")? }
            f.write_str(word)?;
        }
        if let Some(((y, m, d), time)) = self.due {
            write!(f, " due:{y:04}-{m:02}-{d:02}")?;
            if let Some((h, min)) = time { write!(f, "T{h:02}:{min:02}")? }
        }
        if let Some(every) = self.every { write!(f, " every:{}", every.name())? }
        Ok(())
    }
}

/// `!`, `!!` or `!!!`: a priority, when a task's text starts with it.
fn is_bangs(word: &str) -> bool { (1..=3).contains(&word.len()) && word.bytes().all(|b| b == b'!') }

/// A word `parse` reads as a setting rather than title: bangs (first only), a due day, a repeat.
fn setting(word: &str, first: bool) -> bool {
    (first && is_bangs(word))
        || word.strip_prefix("due:").is_some_and(|v| parse_due(v, (1970, 1, 1)).is_some())
        || word.strip_prefix("every:").is_some_and(|v| Every::parse(v).is_some())
}

/// `2026-11-25`, `today` or `tomorrow`, then optionally `T17:00` or `@17:00`.
fn parse_due(v: &str, today: Date) -> Option<(Date, Option<(u32, u32)>)> {
    let (day, time) = match v.split_once(['T', '@']) { Some((d, t)) => (d, Some(t)), None => (v, None) };
    let day = match day.to_ascii_lowercase().as_str() {
        "today" => today,
        "tomorrow" => shift_days(today, 1),
        _ => {
            let mut p = day.splitn(3, '-').map(str::parse::<u32>);
            let (y, m, d) = (p.next()?.ok()?, p.next()?.ok()?, p.next()?.ok()?);
            if !(1..=12).contains(&m) || d == 0 || d > days_in(y as i32, m) { return None }
            (y as i32, m, d)
        }
    };
    let time = match time {
        Some(t) => {
            let (h, m) = t.split_once(':')?;
            let (h, m) = (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?);
            if h > 23 || m > 59 { return None }
            Some((h, m))
        }
        None => None,
    };
    Some((day, time))
}

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
pub fn natural_due(title: &str, today: Date, now: u32) -> (String, Option<(Date, Option<(u32, u32)>)>) {
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
            let ahead = |target: i64| { let wd = i64::from(crate::date::weekday(today.0, today.1, today.2)); let n = (target - wd).rem_euclid(7); shift_days(today, if n == 0 { 7 } else { n }) };
            (day, used) = match w {
                "today" => (Some(today), 1),
                "tonight" => { tonight = true; (Some(today), 1) }
                "tomorrow" | "tmrw" => (Some(shift_days(today, 1)), 1),
                "next" | "on" | "this" if at(i + 1).and_then(weekday_named).is_some() => (at(i + 1).and_then(weekday_named).map(ahead), 2),
                "in" => match (at(i + 1).and_then(|n| n.parse::<i64>().ok()), at(i + 2)) {
                    (Some(n @ 0..=3650), Some("day" | "days")) => (Some(shift_days(today, n)), 3),
                    (Some(n @ 0..=520), Some("week" | "weeks")) => (Some(shift_days(today, n * 7)), 3),
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
    let day = day.unwrap_or_else(|| if time.is_some_and(|(h, m)| h * 60 + m <= now) { shift_days(today, 1) } else { today });
    (rest.join(" "), Some((day, time)))
}

/// "Today 17:00", "Tomorrow", "Wed, Nov 25".
fn due_label(task: &Task, today: Date) -> String {
    let Some((day, time)) = task.due else { return String::new() };
    let name = if day == today { "Today".to_string() } else if day == shift_days(today, 1) { "Tomorrow".to_string() } else { format_date(day.0, day.1, day.2) };
    match time { Some((h, m)) => format!("{name} {h:02}:{m:02}"), None => name }
}

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
    let d = &s.task_draft;
    // Typed `due:tomorrow` and the like still work: read as of today, then saved as dates.
    // Priority comes from its control only, so leading bangs stay in the title.
    let typed = s.task.text.trim();
    let typed = if typed.split_whitespace().next().is_some_and(is_bangs) { format!("\\{typed}") } else { typed.to_string() };
    // An edit's field holds the title as it is, which may itself contain such words.
    let mut task = if d.editing.is_some() { Task { title: s.task.text.split_whitespace().collect::<Vec<_>>().join(" "), ..Task::default() } } else { Task::parse(&typed, s.today) };
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
    match s.task_draft.editing {
        // Gone since the edit began (done elsewhere, removed by a sync): saved as a new task, not lost.
        Some(key) => s.save(|c| match c.tasks.iter_mut().find(|t| t.key == key) { Some(t) => t.text = text, None => c.tasks.push(Item::new(text)) }),
        None => s.save(|c| c.tasks.push(Item::new(text))),
    }
    reset_draft(s);
    s.nextcloud_sync(Kind::Tasks);
}

pub fn reset_draft(s: &mut Island) {
    s.task_draft = TaskDraft::default();
    s.task.clear();
}

fn start_edit(s: &mut Island, key: u64) {
    let Some(task) = s.cfg.tasks.iter().find(|t| t.key == key).map(|t| Task::parse(&t.text, s.today)) else { return };
    s.task.replace_text(task.title.clone());
    s.task.focused = true;
    s.task_draft = TaskDraft { priority: task.priority, due: task.due.map(|d| d.0), time: task.due.and_then(|d| d.1), every: task.every, editing: Some(key) };
}

/// The time 30 minutes either way, round the clock.
fn step_time((h, m): (u32, u32), by: i32) -> (u32, u32) {
    let t = (h as i32 * 60 + m as i32 + by * 30).rem_euclid(1440);
    ((t / 60) as u32, (t % 60) as u32)
}

/// By key, not index: a sync may have removed a task above it since the list was drawn. A
/// recurring task comes back as a new one, due next time; Nextcloud marks this one done.
fn tick(s: &mut Island, key: u64) {
    // Done here, a rule only Nextcloud follows would end or come back wrong.
    let custom = s.cfg.tasks.iter().find(|t| t.key == key).is_some_and(|t| Task::parse(&t.text, s.today).every == Some(Every::Custom));
    if custom && s.cfg.nextcloud.enabled() {
        s.nextcloud[Kind::Tasks as usize].error = Some("This task repeats by a rule only Nextcloud follows: tick it there".into());
        return;
    }
    if s.task_draft.editing == Some(key) { reset_draft(s) }
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
pub fn due_reminders(tasks: &[Item], reminded: &[String], now: &str, today: Date) -> Vec<(String, String)> {
    tasks.iter().filter_map(|item| {
        let task = Task::parse(&item.text, today);
        let due = task.due_key().filter(|k| k.as_str() <= now)?;
        let id = format!("{}@{due}", item.key);
        (!reminded.contains(&id)).then_some((id, task.title))
    }).collect()
}

impl Island {
    /// Once a minute: a reminder for the tasks that fell due and haven't had one, including those
    /// that fell due while Ripple wasn't running. Several at once share one reminder.
    pub fn task_reminders(&mut self, now: f64) {
        // An open Large draws no alerts and another reminder isn't replaced: the reminder waits. Resting in Large (Large standby) doesn't count.
        if rune::is_headless() || self.large_open() || self.reminder.is_some() { return }
        let t = LocalTime::at(crate::date::now_ms());
        let mark = format!("{:04}-{:02}-{:02}T{:02}:{:02}", t.year, t.month, t.day, t.hour, t.minute);
        if std::mem::replace(&mut self.task_mark, mark.clone()) == mark { return }
        let due = due_reminders(&self.cfg.tasks, &self.cfg.reminded_tasks, &mark, self.today);
        if due.is_empty() { return }
        let title = if due.len() == 1 { due[0].1.clone() } else { format!("{} tasks due", due.len()) };
        self.save(|c| {
            // Ids of tasks gone or moved since are dropped.
            let live: Vec<String> = c.tasks.iter().map(|i| format!("{}@", i.key)).collect();
            c.reminded_tasks.retain(|id| live.iter().any(|k| id.starts_with(k)));
            c.reminded_tasks.extend(due.into_iter().map(|d| d.0));
        });
        self.remind(title, now);
    }
}

fn priority_color(p: u8) -> rune::Color {
    match p { 3 => accent::red(), 2 => accent::orange(), _ => accent::blue() }
}

pub fn view(s: &Island) -> El {
    let fg = s.cfg.text();
    let muted = fg.alpha(0.55 * fg.a);
    let mut list = col().key("task-list").w_full().gap(6.0);
    if let Some(e) = &s.nextcloud[Kind::Tasks as usize].error {
        list = list.child(text(e).font(s.cfg.font(12.0, 400.0)).color(muted).wrap_anywhere());
    }
    if s.cfg.tasks.is_empty() { list = list.child(empty(s, "No tasks yet")) }
    let now = { let t = LocalTime::at(crate::date::now_ms()); format!("{:04}-{:02}-{:02}T{:02}:{:02}", t.year, t.month, t.day, t.hour, t.minute) };
    // Highest priority first, then soonest due; the rest keep their order.
    let mut tasks: Vec<_> = s.cfg.tasks.iter().map(|i| (i.key, Task::parse(&i.text, s.today))).collect();
    tasks.sort_by_key(|(_, t)| (std::cmp::Reverse(t.priority), t.due_key().unwrap_or_else(|| "~".into())));
    for (key, task) in tasks {
        let editing = s.task_draft.editing == Some(key);
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
            .transition(Transition::opacity())
            .child(checkbox(s, "done", &format!("Done: {}", task.title), false).on_press(move |s: &mut Island| tick(s, key)))
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
    let editing = s.task_draft.editing.is_some();
    let mut input = row().key("task-input").w_full().gap(8.0).center_y()
        .child(field(s, "new-task", &s.task, if editing { "Task title" } else { "New task, like call Bob tomorrow 9am" }, |s: &mut Island| &mut s.task, add_task));
    if editing { input = input.child(tool(s, "cancel-edit", &icons::X, "Cancel edit").on_press(reset_draft)) }
    col().key("tasks").w_full().h_full().px(16.0).pt(18.0).pb(14.0).gap(10.0)
        .child(col().key("task-scroll").w_full().grow(1.0).min_h(0.0).scrolls().child(list))
        .child(input.child(button(s, "add-task", if editing { "Save" } else { "Add" }, true).on_press_tx(add_task)))
        .child(controls(s))
}

/// A chip that does something when pressed.
fn toggle(s: &Island, key: &str, label: &str, tint: rune::Color, on: bool, press: impl Fn(&mut Island) + 'static) -> El {
    chip(s, key, label, tint, on).role("button").label(label.to_string()).focusable(true).on_press(press)
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
    let d = &s.task_draft;
    let priority = ["Priority", "Low", "Medium", "High"][usize::from(d.priority)];
    let mut row_ = row().key("task-controls").w_full().gap(6.0).center_y()
        .child(toggle(s, "task-priority", priority, priority_color(d.priority), d.priority > 0, |s| s.task_draft.priority = (s.task_draft.priority + 1) % 4));
    let date = d.due.map_or("Date".to_string(), |day| due_label(&Task { due: Some((day, None)), ..Task::default() }, s.today));
    row_ = row_.child(stepper(s, "task-date",
        toggle(s, "task-due", &date, accent::teal(), d.due.is_some(), |s| {
            let d = &mut s.task_draft;
            (d.due, d.time) = if d.due.is_some() { (None, None) } else { (Some(s.today), None) };
        }),
        d.due.is_some(), |s, by| if let Some(day) = s.task_draft.due { s.task_draft.due = Some(shift_days(day, by.into())) }));
    if d.due.is_some() {
        let time = d.time.map_or("Time".to_string(), |(h, m)| format!("{h:02}:{m:02}"));
        row_ = row_.child(stepper(s, "task-time",
            toggle(s, "task-at", &time, accent::teal(), d.time.is_some(), |s| s.task_draft.time = if s.task_draft.time.is_some() { None } else { Some((9, 0)) }),
            d.time.is_some(), |s, by| if let Some(t) = s.task_draft.time { s.task_draft.time = Some(step_time(t, by)) }));
    }
    let every = match d.every { None => "Repeat", Some(Every::Day) => "Daily", Some(Every::Week) => "Weekly", Some(Every::Month) => "Monthly", Some(Every::Custom) => "Custom" };
    row_.child(toggle(s, "task-repeat", every, accent::indigo(), d.every.is_some(), |s| {
        s.task_draft.every = match s.task_draft.every { None => Some(Every::Day), Some(Every::Day) => Some(Every::Week), Some(Every::Week) => Some(Every::Month), Some(Every::Month) | Some(Every::Custom) => None };
    }))
}

#[cfg(test)]
mod tests {
    use super::{Every, Task};

    #[test]
    fn task_text_reads_and_writes_priority_due_and_repeat() {
        let today = (2026, 11, 20);
        let t = Task::parse("!! Pay  rent due:tomorrow@7:05 every:monthly", today);
        assert_eq!(t, Task { priority: 2, title: "Pay rent".into(), due: Some(((2026, 11, 21), Some((7, 5)))), every: Some(Every::Month) });
        assert_eq!(t.to_string(), "!! Pay rent due:2026-11-21T07:05 every:month");
        assert_eq!(Task::parse(&t.to_string(), today), t, "the written form reads back the same");
        assert_eq!(Task::parse("!!!! loud due:2026-02-30 every:year", today).title, "!!!! loud due:2026-02-30 every:year", "what isn't valid stays in the title");
        assert_eq!(Task::parse("Buy milk due:2026-12-01", today).due_key().as_deref(), Some("2026-12-01T09:00"));
        for title in ["! call mom", "!", "!!! now", "Read every:week article", "Call due:2026-01-05 mum", "due:today"] {
            for priority in [0, 2] {
                let t = Task { priority, title: title.into(), ..Task::default() };
                assert_eq!(Task::parse(&t.to_string(), today), t, "{t}");
            }
        }
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
    fn a_recurring_task_moves_to_its_next_due_day_not_before_today() {
        let today = (2026, 11, 20);
        let next = |text: &str| Task::parse(text, today).next(today).map(|t| t.to_string());
        assert_eq!(next("Rent due:2026-01-31 every:month").as_deref(), Some("Rent due:2026-11-30 every:month"));
        assert_eq!(Task::parse("Rent due:2026-01-31 every:month", (2026, 2, 1)).next((2026, 2, 1)).unwrap().due, Some(((2026, 2, 28), None)));
        assert_eq!(next("Standup due:2026-11-20T09:30 every:day").as_deref(), Some("Standup due:2026-11-21T09:30 every:day"));
        assert_eq!(next("Bins every:week").as_deref(), Some("Bins due:2026-11-27 every:week"));
        assert_eq!(next("Once due:2026-11-20"), None);
    }

    #[test]
    fn the_controls_write_a_task_and_an_edit_keeps_its_key() {
        use crate::config::{Config, Store};
        use crate::island::Island;
        let mut s = Island::new(Store::memory(Config::default()), Vec::new());
        s.today = (2026, 1, 1);
        s.task.replace_text("  Pay   rent ");
        s.task_draft = super::TaskDraft { priority: 2, due: Some((2026, 1, 31)), time: Some(super::step_time((0, 0), -1)), every: Some(Every::Month), editing: None };
        super::add_task(&mut s, &mut rune::Tx::default());
        assert_eq!(s.cfg.tasks[0].text, "!! Pay rent due:2026-01-31T23:30 every:month");
        assert!(s.task.text.is_empty() && s.task_draft.priority == 0, "the controls reset");
        let key = s.cfg.tasks[0].key;
        super::start_edit(&mut s, key);
        assert_eq!((s.task.text.as_str(), s.task_draft.time, s.task_draft.editing), ("Pay rent", Some((23, 30)), Some(key)));
        s.task_draft.every = None;
        super::add_task(&mut s, &mut rune::Tx::default());
        assert_eq!((s.cfg.tasks.len(), s.cfg.tasks[0].key, s.cfg.tasks[0].text.as_str()), (1, key, "!! Pay rent due:2026-01-31T23:30"));
        s.task.replace_text("Call Bob due:tomorrow");
        super::add_task(&mut s, &mut rune::Tx::default());
        assert_eq!(s.cfg.tasks[1].text, "Call Bob due:2026-01-02", "a typed relative day is saved as its date");
        s.task.replace_text("Call Ann tomorrow 9am");
        super::add_task(&mut s, &mut rune::Tx::default());
        assert_eq!(s.cfg.tasks[2].text, "Call Ann due:2026-01-02T09:00", "a day said in words too");
        s.save(|c| { c.tasks.pop(); });
        let bob = s.cfg.tasks[1].key;
        super::start_edit(&mut s, bob);
        s.save(|c| c.tasks.retain(|t| t.key != bob));
        s.task.replace_text("Call Bob back");
        super::add_task(&mut s, &mut rune::Tx::default());
        assert_eq!(s.cfg.tasks[1].text, "Call Bob back due:2026-01-02", "an edit of a task gone meanwhile is kept as a new one");
        let read = s.cfg.tasks[1].key;
        super::start_edit(&mut s, read);
        s.task.replace_text("Read every:week article");
        super::add_task(&mut s, &mut rune::Tx::default());
        assert_eq!(Task::parse(&s.cfg.tasks[1].text, s.today).title, "Read every:week article", "an edit's title is taken as typed");
        s.task.replace_text("! call mom");
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
        super::tick(&mut s, key);
        assert_eq!(s.cfg.tasks.len(), 1);
        assert!(s.nextcloud[crate::nextcloud::Kind::Tasks as usize].error.is_some());
        assert_eq!(Task::parse("Bins every:custom", (2026, 1, 1)).next((2026, 1, 1)), None);
    }

    #[test]
    fn ticking_a_recurring_task_brings_it_back_due_next_time() {
        use crate::config::{Config, Item, Store};
        use crate::island::Island;
        let mut s = Island::new(Store::memory(Config { tasks: vec![Item::new("Water plants due:2026-01-01 every:week"), Item::new("Once")], ..Config::default() }), Vec::new());
        s.today = (2026, 1, 1);
        let (weekly, once) = (s.cfg.tasks[0].key, s.cfg.tasks[1].key);
        super::tick(&mut s, weekly);
        super::tick(&mut s, once);
        let texts: Vec<_> = s.cfg.tasks.iter().map(|t| t.text.as_str()).collect();
        assert_eq!(texts, ["Water plants due:2026-01-08 every:week"]);
    }
}
