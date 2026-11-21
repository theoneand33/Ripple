//! A task as one line of text: `!! Title due:2026-11-25T17:00 every:week`. The Tasks tab shows
//! and edits it; Nextcloud sync maps it to PRIORITY, DUE and RRULE.

use crate::date::{add_days, days_in};

pub type Date = (i32, u32, u32);

/// A minute as due times compare: `YYYY-MM-DDTHH:MM`, which sorts as text.
fn minute_key((y, m, d): Date, (h, min): (u32, u32)) -> String { format!("{y:04}-{m:02}-{d:02}T{h:02}:{min:02}") }

/// Now on this clock, as a `minute_key`.
pub fn now_key() -> String {
    let t = rune::time::LocalTime::at(crate::date::now_ms());
    minute_key((t.year, t.month, t.day), (t.hour, t.minute))
}

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
    pub fn due_at(&self) -> Option<(Date, (u32, u32))> { self.due.map(|(day, time)| (day, time.unwrap_or((9, 0)))) }

    /// `due_at` as a `minute_key`.
    pub fn due_key(&self) -> Option<String> { self.due_at().map(|(day, time)| minute_key(day, time)) }

    /// The next one of a recurring task, due the first repeat after its due day that is not
    /// before today; one with no due day is due a repeat from today.
    pub fn next(&self, today: Date) -> Option<Task> {
        let every = self.every?;
        let (from, time) = self.due.unwrap_or((today, None));
        // The nth repeat counts from the due day itself, so the 31st repeats on each month's last day without drifting.
        let nth = |n: i32| match every {
            Every::Day => Some(add_days(from, n.into())),
            Every::Week => Some(add_days(from, i64::from(n) * 7)),
            Every::Month => { let (y, m) = crate::date::shift_month(from.0, from.1, n); Some((y, m, from.2.min(days_in(y, m)))) }
            Every::Custom => None,
        };
        let day = (1..).map_while(nth).find(|day| *day >= today)?;
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
pub fn is_bangs(word: &str) -> bool { (1..=3).contains(&word.len()) && word.bytes().all(|b| b == b'!') }

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
        "tomorrow" => add_days(today, 1),
        _ => crate::date::parse_ymd(day)?,
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
        assert_eq!(next("Bins every:custom"), None, "Nextcloud keeps a rule Ripple can't follow");
    }
}
