//! AI Limits: how much of each signed-in coding agent's plan is left, and when it resets.

use rune::{El, col, row, text};

use super::{Scrolls, empty};
use super::monitor::gauge;
use crate::island::Island;
use crate::limits;

#[derive(Default)]
pub struct Limits {
    /// Each signed-in agent's windows; None until the first answer. Empty hides the tab.
    pub usage: Option<Vec<limits::Usage>>,
    /// Bumped on every load, so an earlier answer is dropped.
    generation: u64,
}

impl Island {
    /// Asks every signed-in agent for its limits; the last answer stays up while it refreshes.
    pub fn limits_load(&mut self) {
        let Some(me) = self.me.clone() else { return };
        let (dirs, proxy) = (self.cfg.agent_dirs.clone(), self.cfg.cliproxy_dir.clone());
        self.limits.generation += 1;
        let generation = self.limits.generation;
        me.spawn(move || limits::all(&dirs, &proxy), move |s, usage| if s.limits.generation == generation { s.limits.usage = Some(usage) });
    }
}

pub fn view(s: &Island) -> El {
    let fg = s.cfg.text();
    let muted = fg.alpha(0.55 * fg.a);
    let root = col().key("limits").w_full().h_full().px(16.0).pt(14.0).pb(14.0);
    let usage = match &s.limits.usage {
        None => return root.child(empty(s, "Loading…")),
        Some(u) if u.is_empty() => return root.child(empty(s, "Sign in to Claude Code, Codex, Copilot, OpenCode Go or Grok Build")),
        Some(u) => u,
    };
    let now = crate::date::now_secs();
    let mut list = col().key("limits-list").w_full().gap(8.0);
    for (n, agent) in usage.iter().enumerate() {
        // "Codex · me@x.com": the agent bold, the account muted.
        let (name, who) = agent.name.split_once(" · ").unwrap_or((&agent.name, ""));
        let mut card = row().key(format!("limits-{n}")).w_full().padding(10.0).gap(10.0).center_y()
            .radius(s.cfg.radius(16.0)).fill(super::card(fg))
            .child(col().w(120.0).gap(2.0)
                .child(text(name).font(s.cfg.font(13.0, 600.0)).color(fg).truncate())
                .children((!who.is_empty()).then(|| text(who).font(s.cfg.font(11.0, 400.0)).color(muted).truncate())));
        match &agent.windows {
            Err(e) => card = card.child(text(e).font(s.cfg.font(12.0, 400.0)).color(muted).grow(1.0).min_w(0.0).truncate()),
            Ok(windows) if windows.is_empty() => card = card.child(text("Unlimited").font(s.cfg.font(12.0, 400.0)).color(muted)),
            Ok(windows) => for w in windows {
                let resets = w.resets.map_or(String::new(), |r| limits::until(r, now));
                card = card.child(
                    row().key(format!("window-{n}-{}", w.label)).role("group").label(format!("{} {}: {:.0}% left{}", agent.name, w.label, 100.0 - w.used, if resets.is_empty() { String::new() } else { format!(", resets in {resets}") }))
                        .w(0.0).grow(1.0).min_w(0.0).gap(8.0).center_y()
                        .child(gauge(s, 100.0 - w.used, true))
                        .child(col().min_w(0.0).gap(2.0)
                            .child(text(&w.label).font(s.cfg.font(12.0, 500.0)).color(fg).truncate().a11y_hidden())
                            .child(text(resets).font(s.cfg.font(11.0, 400.0).features("'tnum' 1")).color(muted).truncate().a11y_hidden())),
                );
            },
        }
        list = list.child(card);
    }
    root.child(col().key("limits-scroll").w_full().grow(1.0).min_h(0.0).scrolls().scroll_edge_fade(10.0).child(list))
}
