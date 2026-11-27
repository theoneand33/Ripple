//! Reminders the Island holds until it may show them: while Large is open, through quiet
//! hours, and while a reminder from elsewhere shows. Everything waiting shows as one alert,
//! and more that arrive while it shows join it. Before it shows, each is asked whether it
//! still stands: a task since done, or an answered agent, no longer reminds.

use crate::config::Tab;

/// Who asked for a reminder; several from one source share its wording.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Source {
    Task,
    Event,
    Agent,
    /// `ripple notify`, a plugin, or Ripple's own update check.
    Script,
}

impl Source {
    fn several(self, n: usize) -> String {
        match self {
            Source::Task => format!("{n} tasks due"),
            Source::Event => format!("{n} events"),
            Source::Agent => format!("{n} agents need you"),
            Source::Script => format!("{n} alerts"),
        }
    }

    /// The tab Large opens on for a reminder from this source.
    fn tab(self) -> Option<Tab> { (self == Source::Agent).then_some(Tab::Agents) }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Reminder {
    pub source: Source,
    /// What the source knows it by, to tell whether it still stands.
    pub id: String,
    pub title: String,
    /// A meeting to join from the alert.
    pub link: Option<String>,
}

/// The one alert that shows for a batch, and for how many seconds.
#[derive(Debug, PartialEq)]
pub struct Alert {
    pub title: String,
    pub link: Option<String>,
    pub secs: f64,
    /// The tab Large opens on, should the user open it.
    pub tab: Option<Tab>,
    /// The reminders it shows for the first time, for their sources to record as shown.
    pub fresh: Vec<Reminder>,
}

impl Alert {
    pub fn new(title: String, link: Option<String>) -> Alert {
        // An event's alert stays long enough for its Join button to be reached.
        let secs = if link.is_some() { 30.0 } else { 6.0 };
        Alert { title, link, secs, tab: None, fresh: Vec::new() }
    }
}

#[derive(Default)]
pub struct Reminders {
    waiting: Vec<Reminder>,
    /// What the reminder on screen stands for, when it is this queue's.
    showing: Vec<Reminder>,
}

impl Reminders {
    /// Holds `reminder`; offered again while it waits or shows, it is still one.
    pub fn offer(&mut self, reminder: Reminder) {
        if !self.waiting.contains(&reminder) && !self.showing.contains(&reminder) { self.waiting.push(reminder) }
    }

    /// Forgets the reminders that no longer stand, so a batch that grows counts only those left.
    pub fn retain(&mut self, stands: impl Fn(&Reminder) -> bool) {
        self.waiting.retain(&stands);
        self.showing.retain(&stands);
    }

    /// The alert to show now for everything waiting, given whether a reminder is on screen:
    /// this queue's merges with the new ones, another's is left to end first.
    pub fn next(&mut self, on_screen: bool) -> Option<Alert> {
        if self.waiting.is_empty() || (on_screen && self.showing.is_empty()) { return None }
        if !on_screen { self.showing.clear() }
        let fresh = self.waiting.clone();
        self.showing.append(&mut self.waiting);
        let batch = self.showing.as_slice();
        let title = match batch {
            [one] => one.title.clone(),
            [first, rest @ ..] if rest.iter().all(|r| r.source == first.source) => first.source.several(batch.len()),
            _ => format!("{} alerts", batch.len()),
        };
        let link = batch.iter().find_map(|r| r.link.clone());
        Some(Alert { tab: batch.iter().find_map(|r| r.source.tab()), fresh, ..Alert::new(title, link) })
    }

    /// The reminder on screen ended, or another took its place.
    pub fn ended(&mut self) { self.showing.clear() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(source: Source, title: &str) -> Reminder { Reminder { source, id: title.into(), title: title.into(), link: None } }

    #[test]
    fn waiting_reminders_share_one_alert_worded_by_their_source() {
        let mut q = Reminders::default();
        assert_eq!(q.next(false), None);
        q.offer(r(Source::Script, "3 alerts"));
        assert_eq!(q.next(false).map(|a| a.title), Some("3 alerts".into()), "a text is a text, whatever it says");
        q.offer(r(Source::Event, "Standup"));
        q.offer(Reminder { link: Some("https://meet.example.com/a".into()), ..r(Source::Event, "Review") });
        let a = q.next(false).unwrap();
        assert_eq!((a.title.as_str(), a.link.as_deref(), a.secs), ("2 events", Some("https://meet.example.com/a"), 30.0));
        q.offer(r(Source::Task, "Ship it"));
        assert_eq!(q.next(false).map(|a| a.title), Some("Ship it".into()), "the last alert ended: a new batch");
    }

    #[test]
    fn reminders_arriving_while_one_shows_join_it_unless_it_is_anothers() {
        let mut q = Reminders::default();
        q.offer(r(Source::Script, "Build passed"));
        q.next(false);
        q.offer(r(Source::Script, "Deploy done"));
        q.offer(r(Source::Script, "Deploy done"));
        assert_eq!(q.next(true).map(|a| (a.title, a.fresh.len())), Some(("2 alerts".into(), 1)), "offered twice, still one");
        q.offer(r(Source::Task, "Ship it"));
        assert_eq!(q.next(true).map(|a| (a.title, a.secs)), Some(("3 alerts".into(), 6.0)), "mixed sources");
        q.ended();
        q.offer(r(Source::Agent, "Codex needs you"));
        q.offer(r(Source::Task, "Water plants"));
        assert_eq!(q.next(true), None, "a break reminder shows: wait for it");
        q.retain(|r| r.source != Source::Task);
        let a = q.next(false).unwrap();
        assert_eq!((a.title.as_str(), a.tab), ("Codex needs you", Some(Tab::Agents)), "the task no longer stands");
    }
}
