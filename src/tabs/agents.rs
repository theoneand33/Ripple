//! Coding Agents: each running agent session, and its permission requests and questions to
//! allow, deny or answer. `crate::agents` holds the hook side.

use std::collections::{BTreeMap, HashMap, HashSet};

use rune::{El, col, row, text};

use super::{Scrolls, accent, button, card, chip, empty, tinted};
use crate::agents::{self, Answer, Request, Session};
use crate::config::Tab;
use crate::island::Island;

#[derive(Default)]
pub struct Agents {
    pub sessions: Vec<Session>,
    pub requests: Vec<Request>,
    /// The labels picked so far, by request key and question.
    pub picks: HashMap<(String, usize), Vec<String>>,
    /// Answered, but the hook may not have taken the answer yet: hidden so it doesn't come back.
    answered: HashSet<String>,
    /// A new request that came in quiet hours: its alert shows once they end.
    held: Option<String>,
}

impl Island {
    /// Once a second: rereads the agents folder. A new request brings the Island up on this tab.
    pub fn agents_poll(&mut self, now: f64) -> bool {
        // A hidden tab can't be answered: without `alive`, hooks leave agents to ask in their terminals.
        if rune::is_headless() || !self.visible_tabs().contains(&Tab::Agents) { return false }
        let Some(dir) = self.store.dir().map(|d| d.join("agents")) else { return false };
        let (sessions, mut requests) = agents::scan(&dir);
        self.agents.answered.retain(|key| requests.iter().any(|r| &r.key == key));
        requests.retain(|r| !self.agents.answered.contains(&r.key));
        let changed = (&sessions, &requests) != (&self.agents.sessions, &self.agents.requests);
        let fresh = requests.iter().find(|r| !self.agents.requests.iter().any(|o| o.key == r.key)).map(|r| r.key.clone());
        self.agents.picks.retain(|(key, _), _| requests.iter().any(|r| &r.key == key));
        (self.agents.sessions, self.agents.requests) = (sessions, requests);
        // Quiet hours: the request waits on this tab, and its alert until they end.
        let key = fresh.or_else(|| self.agents.held.take()).filter(|k| self.agents.requests.iter().any(|r| &r.key == k));
        if key.is_some() && self.quiet() { self.agents.held = key; return changed }
        if let Some(r) = key.and_then(|k| self.agents.requests.iter().find(|r| r.key == k).cloned()) && !self.large_open() {
            let agent = self.agents.sessions.iter().find(|s| s.key == r.session).map_or("An agent", |s| agents::name(&s.agent)).to_string();
            if self.visible_tabs().contains(&Tab::Agents) { self.set_tab(Tab::Agents) }
            self.remind(format!("{agent} needs you"), now);
            return true
        }
        changed
    }

    pub fn agents_answer(&mut self, key: &str, answer: Answer) {
        let Some(dir) = self.store.dir().map(|d| d.join("agents")) else { return };
        // Left up on failure: the hook still waits, and the next scan shows it again.
        if let Err(e) = agents::answer(&dir, key, &answer) { return eprintln!("Ripple: could not answer the agent: {e}") }
        self.agents.requests.retain(|r| r.key != key);
        self.agents.answered.insert(key.to_string());
    }

    fn agents_pick(&mut self, key: &str, question: usize, label: &str, multi: bool) {
        let picked = self.agents.picks.entry((key.to_string(), question)).or_default();
        match picked.iter().position(|l| l == label) {
            Some(i) => { picked.remove(i); }
            None if multi => picked.push(label.to_string()),
            None => *picked = vec![label.to_string()],
        }
    }

    /// Every question's picks, once each has one.
    fn agents_answers(&self, r: &Request) -> Option<BTreeMap<String, String>> {
        r.questions.iter().enumerate().map(|(i, q)| {
            let picked = self.agents.picks.get(&(r.key.clone(), i)).filter(|p| !p.is_empty())?;
            Some((q.question.clone(), picked.join(", ")))
        }).collect()
    }
}

pub fn view(s: &Island) -> El {
    let fg = s.cfg.text();
    let muted = fg.alpha(0.55 * fg.a);
    let root = col().key("agents").w_full().h_full().px(16.0).pt(14.0).pb(14.0);
    if s.agents.sessions.is_empty() { return root.child(empty(s, "No agent sessions yet. Add Ripple's hook to your agents (see the README).")) }
    let mut list = col().key("agents-list").w_full().gap(8.0);
    // Those waiting on you first, so a request is never scrolled away.
    let mut sessions: Vec<&Session> = s.agents.sessions.iter().collect();
    sessions.sort_by_key(|session| !s.agents.requests.iter().any(|r| r.session == session.key));
    for (n, session) in sessions.into_iter().enumerate() {
        let requests: Vec<&Request> = s.agents.requests.iter().filter(|r| r.session == session.key).collect();
        let (state, color) = if !requests.is_empty() { ("Needs you", accent::orange()) }
            else if session.working { ("Working", accent::blue()) } else { ("Idle", muted) };
        let folder = session.cwd.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().unwrap_or_default();
        let mut c = col().key(format!("agent-{n}")).w_full().padding(10.0).gap(6.0).radius(s.cfg.radius(16.0)).fill(card(fg))
            .child(row().w_full().gap(8.0).center_y()
                .child(text(agents::name(&session.agent)).font(s.cfg.font(13.0, 600.0)).color(fg).single_line())
                .child(text(folder).font(s.cfg.font(12.0, 400.0)).color(muted).grow(1.0).min_w(0.0).truncate())
                .child(chip(s, &format!("agent-{n}-state"), state, color, state != "Idle")));
        if requests.is_empty() && !session.detail.is_empty() {
            c = c.child(text(&session.detail).font(s.cfg.font(12.0, 400.0)).color(muted).w_full().max_lines(2).wrap_anywhere());
        }
        for r in requests { c = c.child(request(s, r)) }
        list = list.child(c);
    }
    root.child(col().key("agents-scroll").w_full().grow(1.0).min_h(0.0).scrolls().scroll_edge_fade(10.0).child(list))
}

/// A permission's Deny and Allow, or a question's choices and Send.
fn request(s: &Island, r: &Request) -> El {
    let fg = s.cfg.text();
    let key = r.key.clone();
    if r.questions.is_empty() {
        let (deny, allow) = (key.clone(), key.clone());
        return col().key(format!("request-{key}")).w_full().gap(8.0)
            .child(text(format!("{}: {}", r.tool, r.detail)).font(s.cfg.font(12.0, 400.0)).color(fg).w_full().wrap_anywhere())
            .child(row().w_full().gap(8.0)
                .child(tinted(s, &format!("deny-{key}"), "Deny", accent::red()).on_press(move |s: &mut Island| s.agents_answer(&deny, Answer::Deny)))
                .child(tinted(s, &format!("allow-{key}"), "Allow", accent::green()).on_press(move |s: &mut Island| s.agents_answer(&allow, Answer::Allow))));
    }
    let mut c = col().key(format!("request-{key}")).w_full().gap(6.0);
    for (i, q) in r.questions.iter().enumerate() {
        let picked = s.agents.picks.get(&(key.clone(), i));
        let mut chips = row().key(format!("choices-{key}-{i}")).role("group").label(q.question.as_str()).w_full().gap(6.0).flex_wrap();
        for (j, choice) in q.options.iter().enumerate() {
            let on = picked.is_some_and(|p| p.contains(&choice.label));
            let (k, label, multi) = (key.clone(), choice.label.clone(), q.multi_select);
            chips = chips.child(chip(s, &format!("choice-{key}-{i}-{j}"), &choice.label, accent::indigo(), on)
                .role(if multi { "checkbox" } else { "radio" }).label(choice.label.as_str()).checked(on).focusable(true)
                .on_press(move |s: &mut Island| s.agents_pick(&k, i, &label, multi)));
        }
        c = c.child(text(&q.question).font(s.cfg.font(12.0, 500.0)).color(fg).w_full().wrap_anywhere()).child(chips);
    }
    let ready = s.agents_answers(r).is_some();
    let send = key.clone();
    c.child(row().w_full().child(button(s, &format!("send-{key}"), "Send", ready).on_press(move |s: &mut Island| {
        let Some(r) = s.agents.requests.iter().find(|r| r.key == send).cloned() else { return };
        if let Some(answers) = s.agents_answers(&r) { s.agents_answer(&send, Answer::Answers(answers)) }
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::{Choice, Question};
    use crate::config::{Config, Store};
    use crate::island::Mode;

    fn question(text: &str, multi: bool) -> Question {
        let options = ["A", "B"].map(|l| Choice { label: l.into(), description: String::new() }).into();
        Question { question: text.into(), header: String::new(), options, multi_select: multi }
    }

    #[test]
    fn picks_answer_every_question_before_send() {
        let mut s = Island::new(Store::memory(Config::default()), Vec::new());
        let r = Request { key: "r".into(), questions: vec![question("One?", false), question("Many?", true)], ..Request::default() };
        s.agents_pick("r", 0, "A", false);
        s.agents_pick("r", 0, "B", false);
        assert_eq!(s.agents_answers(&r), None, "the second question has no pick yet");
        s.agents_pick("r", 1, "A", true);
        s.agents_pick("r", 1, "B", true);
        let answers = s.agents_answers(&r).unwrap();
        assert_eq!((answers["One?"].as_str(), answers["Many?"].as_str()), ("B", "A, B"), "one replaces, many add");
        s.agents_pick("r", 1, "A", true);
        s.agents_pick("r", 1, "B", true);
        assert_eq!(s.agents_answers(&r), None, "picking again unpicks");
    }

    #[test]
    fn large_opens_on_a_waiting_request() {
        let mut s = Island::new(Store::memory(Config { default_tab: Tab::Search, ..Config::default() }), Vec::new());
        s.agents.requests = vec![Request { key: "r".into(), ..Request::default() }];
        s.set_mode(Mode::Large);
        assert_eq!(s.tab, Tab::Agents);
        s.set_mode(Mode::Still);
        s.agents.requests.clear();
        s.set_mode(Mode::Large);
        assert_eq!(s.tab, Tab::Search);
    }
}
