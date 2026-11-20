//! Tasks: a to-do list; ticking a task off removes it, or moves a recurring one to its next
//! due date. A task's text carries its priority (`!`, `!!`, `!!!`), due date (`due:2026-11-25`,
//! `due:tomorrow@17:00`) and repeat (`every:week`); a due task raises a reminder. With a
//! Nextcloud account the list syncs with Nextcloud Tasks, where these are PRIORITY, DUE and RRULE.

use rune::time::LocalTime;
use rune::{El, Transition, Tx, col, el, row, text};

use super::{Scrolls, accent, button, checkbox, empty, field};
use crate::config::Item;
use crate::date::{days_from_civil, days_in, format_date};
use crate::island::Island;
use crate::nextcloud::{self, Kind};

pub type Date = (i32, u32, u32);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Every {
    Day,
    Week,
    Month,
}

impl Every {
    pub fn name(self) -> &'static str {
        match self { Every::Day => "day", Every::Week => "week", Every::Month => "month" }
    }

    fn parse(s: &str) -> Option<Every> {
        match s.to_ascii_lowercase().as_str() {
            "day" | "daily" => Some(Every::Day),
            "week" | "weekly" => Some(Every::Week),
            "month" | "monthly" => Some(Every::Month),
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
            if i == 0 && (1..=3).contains(&word.len()) && word.bytes().all(|b| b == b'!') { task.priority = word.len() as u8; continue }
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
        let step = |(y, m, d): Date| match every {
            Every::Day => shift_days((y, m, d), 1),
            Every::Week => shift_days((y, m, d), 7),
            // The 31st repeats on each month's last day.
            Every::Month => { let (y, m) = crate::date::shift_month(y, m, 1); (y, m, d.min(days_in(y, m))) }
        };
        let (mut day, time) = self.due.unwrap_or((today, None));
        day = step(day);
        while day < today { day = step(day) }
        Some(Task { due: Some((day, time)), ..self.clone() })
    }
}

impl std::fmt::Display for Task {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.priority > 0 { write!(f, "{} ", "!".repeat(self.priority.into()))? }
        f.write_str(&self.title)?;
        if let Some(((y, m, d), time)) = self.due {
            write!(f, " due:{y:04}-{m:02}-{d:02}")?;
            if let Some((h, min)) = time { write!(f, "T{h:02}:{min:02}")? }
        }
        if let Some(every) = self.every { write!(f, " every:{}", every.name())? }
        Ok(())
    }
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

/// "Today 17:00", "Tomorrow", "Wed, Nov 25".
fn due_label(task: &Task, today: Date) -> String {
    let Some((day, time)) = task.due else { return String::new() };
    let name = if day == today { "Today".to_string() } else if day == shift_days(today, 1) { "Tomorrow".to_string() } else { format_date(day.0, day.1, day.2) };
    match time { Some((h, m)) => format!("{name} {h:02}:{m:02}"), None => name }
}

fn add_task(s: &mut Island, _: &mut Tx) {
    // Stored in the one written form, so `due:tomorrow` becomes a date.
    let task = Task::parse(&s.task.text, s.today);
    if task.title.is_empty() { return }
    s.save(|c| c.tasks.push(Item::new(task.to_string())));
    s.task.clear();
    s.nextcloud_sync(Kind::Tasks);
}

/// By key, not index: a sync may have removed a task above it since the list was drawn. A
/// recurring task comes back as a new one, due next time; Nextcloud marks this one done.
fn tick(s: &mut Island, key: u64) {
    let today = s.today;
    s.save(|c| {
        if let Some((i, item)) = nextcloud::remove(c, Kind::Tasks, key)
            && let Some(next) = Task::parse(&item.text, today).next(today) {
            c.tasks.insert(i.min(c.tasks.len()), Item::new(next.to_string()));
        }
    });
    s.nextcloud_sync(Kind::Tasks);
}

impl Island {
    /// Once a second: a reminder for each task that fell due since the last check. Tasks due
    /// before Ripple started don't all fire at once.
    pub fn task_reminders(&mut self, now: f64) {
        if rune::is_headless() { return }
        let t = LocalTime::at(crate::date::now_ms());
        let mark = format!("{:04}-{:02}-{:02}T{:02}:{:02}", t.year, t.month, t.day, t.hour, t.minute);
        let last = std::mem::replace(&mut self.task_mark, mark.clone());
        if last.is_empty() || last == mark { return }
        let due = self.cfg.tasks.iter().map(|i| Task::parse(&i.text, self.today))
            .find(|task| task.due_key().is_some_and(|k| *k > *last && k <= mark));
        if let Some(task) = due { self.remind(task.title, now) }
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
        let overdue = task.due_key().is_some_and(|k| !rune::is_headless() && k <= now);
        let mut meta = row().gap(8.0).center_y();
        if task.due.is_some() {
            meta = meta.child(text(due_label(&task, s.today)).font(s.cfg.font(12.0, 600.0)).color(if overdue { accent::red() } else { muted }));
        }
        if let Some(every) = task.every { meta = meta.child(text(format!("↻ every {}", every.name())).font(s.cfg.font(12.0, 500.0)).color(muted)) }
        let mut body = col().grow(1.0).min_w(0.0).gap(2.0)
            .child(text(&task.title).font(s.cfg.font(14.0, 500.0)).color(fg).wrap_anywhere());
        if task.due.is_some() || task.every.is_some() { body = body.child(meta) }
        let mut line = row().key(format!("task-{key}")).w_full().px(12.0).py(8.0).gap(10.0).center_y()
            .radius(s.cfg.radius(16.0)).fill(super::card(fg))
            .transition(Transition::opacity())
            .child(checkbox(s, "done", &format!("Done: {}", task.title), false).on_press(move |s: &mut Island| tick(s, key)))
            .child(body);
        if task.priority > 0 {
            let tint = priority_color(task.priority);
            line = line.child(el().key("priority").role("img").label(format!("Priority {}", ["", "low", "medium", "high"][task.priority as usize]))
                .h(22.0).px(8.0).center().radius(s.cfg.radius(11.0)).fill(tint.alpha(0.22))
                .child(text("!".repeat(task.priority.into())).font(s.cfg.font(12.0, 800.0)).color(tint).a11y_hidden()));
        }
        list = list.child(line);
    }
    col().key("tasks").w_full().h_full().px(16.0).pt(18.0).pb(14.0).gap(10.0)
        .child(col().key("task-scroll").w_full().grow(1.0).min_h(0.0).scrolls().child(list))
        .child(
            row().key("task-input").w_full().gap(8.0).center_y()
                .child(field(s, "new-task", &s.task, "New task… !! due:tomorrow every:week", |s: &mut Island| &mut s.task, add_task))
                .child(button(s, "add-task", "Add", true).on_press_tx(add_task)),
        )
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
    }

    #[test]
    fn a_recurring_task_moves_to_its_next_due_day_not_before_today() {
        let today = (2026, 11, 20);
        let next = |text: &str| Task::parse(text, today).next(today).map(|t| t.to_string());
        assert_eq!(next("Rent due:2026-01-31 every:month").as_deref(), Some("Rent due:2026-11-30 every:month"));
        assert_eq!(next("Standup due:2026-11-20T09:30 every:day").as_deref(), Some("Standup due:2026-11-21T09:30 every:day"));
        assert_eq!(next("Bins every:week").as_deref(), Some("Bins due:2026-11-27 every:week"));
        assert_eq!(next("Once due:2026-11-20"), None);
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
