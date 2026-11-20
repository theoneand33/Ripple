//! Tasks: a to-do list. The controls under the field set a task's priority, due date and time,
//! and repeat; press a task to edit it. Ticking a task off removes it, or moves a recurring one
//! to its next due date; a due task raises a reminder. A task is saved as one line of text
//! (`!! Title due:2026-11-25T17:00 every:week`). With a Nextcloud account the list syncs with
//! Nextcloud Tasks, where these are PRIORITY, DUE and RRULE.

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
        };
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
        // Large mode draws no alerts: the reminder waits until the Island is compact again.
        if rune::is_headless() || self.mode == crate::island::Mode::Large { return }
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
        if let Some(every) = task.every { meta = meta.child(text(format!("↻ every {}", every.name())).font(s.cfg.font(12.0, 500.0)).color(muted)) }
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
        .child(field(s, "new-task", &s.task, if editing { "Task title" } else { "New task…" }, |s: &mut Island| &mut s.task, add_task));
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
    let every = match d.every { None => "Repeat", Some(Every::Day) => "Daily", Some(Every::Week) => "Weekly", Some(Every::Month) => "Monthly" };
    row_.child(toggle(s, "task-repeat", every, accent::indigo(), d.every.is_some(), |s| {
        s.task_draft.every = match s.task_draft.every { None => Some(Every::Day), Some(Every::Day) => Some(Every::Week), Some(Every::Week) => Some(Every::Month), Some(Every::Month) => None };
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
