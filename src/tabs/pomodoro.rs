//! Pomodoro: 25 minutes of focus, then a 5 minute break, round after round. While it runs, the
//! compact Island shows the time left beside a ring, as a live activity.

use rune::tok::TextStyle;
use rune::{El, Tx, col, el, icon, row, text};

use super::{accent, chip, ring, tinted, tool};
use crate::icons;
use crate::island::Island;

pub const FOCUS: f64 = 25.0 * 60.0;
pub const BREAK: f64 = 5.0 * 60.0;

pub struct Pomodoro {
    pub focus: bool,
    /// Seconds left, as of the last tick.
    pub left: f64,
    /// When the phase ends, on the app clock; None while paused.
    pub ends: Option<f64>,
    /// Focus phases finished.
    pub rounds: u32,
}

impl Default for Pomodoro {
    fn default() -> Self { Pomodoro { focus: true, left: FOCUS, ends: None, rounds: 0 } }
}

impl Pomodoro {
    pub fn running(&self) -> bool { self.ends.is_some() }

    fn length(&self) -> f64 { if self.focus { FOCUS } else { BREAK } }

    /// Elapsed share of the phase, for the ring.
    pub fn percent(&self) -> u8 { ((1.0 - self.left / self.length()) * 100.0).clamp(0.0, 100.0) as u8 }

    pub fn label(&self) -> String {
        let s = self.left.ceil() as u64;
        format!("{}:{:02}", s / 60, s % 60)
    }

    pub fn toggle(&mut self, now: f64) {
        // Pausing keeps the time since the last tick off the clock.
        self.ends = match self.ends { Some(ends) => { self.left = (ends - now).max(0.0); None } None => Some(now + self.left) };
    }

    pub fn reset(&mut self) { *self = Pomodoro { rounds: self.rounds, ..Pomodoro::default() } }

    /// A skipped focus phase is not a finished round.
    pub fn skip(&mut self, now: f64) {
        self.focus = !self.focus;
        self.left = self.length();
        if self.ends.is_some() { self.ends = Some(now + self.left) }
    }

    /// Counts down; a phase that ends starts the next one. False while paused.
    pub fn tick(&mut self, now: f64) -> bool {
        // shortcut: the app clock may stop while the computer sleeps, so the timer falls behind; use wall time if that matters.
        let Some(mut ends) = self.ends.filter(|_| now.is_finite()) else { return false };
        while now >= ends {
            if self.focus { self.rounds += 1 }
            self.focus = !self.focus;
            ends += self.length();
        }
        (self.ends, self.left) = (Some(ends), ends - now);
        true
    }
}

fn color(focus: bool) -> rune::Color { if focus { accent::orange() } else { accent::green() } }

/// The compact Island's live activity while it runs: a ring and the time left.
pub fn live(p: &Pomodoro, style: TextStyle) -> El {
    let (glyph, tint) = (if p.focus { &icons::TIMER } else { &icons::COFFEE }, color(p.focus));
    row().key("pomodoro").role("img").label(format!("{}, {} left", if p.focus { "Focus" } else { "Break" }, p.label())).gap(6.0).center_y()
        .child(ring(18.0, f32::from(p.percent()), tint, icon(glyph, 10.0).color(tint)))
        .child(text(p.label()).font(style.features("'tnum' 1")).color(tint).a11y_hidden())
}

/// The ring alone, for the bubble beside the compact Island while music has the Island.
pub fn bubble(p: &Pomodoro, size: f32) -> El {
    let (glyph, tint) = (if p.focus { &icons::TIMER } else { &icons::COFFEE }, color(p.focus));
    el().role("img").label(format!("{}, {} left", if p.focus { "Focus" } else { "Break" }, p.label()))
        .child(ring(size, f32::from(p.percent()), tint, icon(glyph, size / 2.0).color(tint)))
}

pub fn view(s: &Island) -> El {
    let fg = s.cfg.text();
    let p = &s.pomodoro;
    let color = color(p.focus);
    let phase = if p.focus { "Focus" } else { "Break" };
    let header = row().key("pomodoro-phases").w_full().center_y().justify_between()
        .child(chip(s, "pomodoro-focus", "Focus", accent::orange(), p.focus))
        .child(text(format!("{} {} done", p.rounds, if p.rounds == 1 { "round" } else { "rounds" }))
            .font(s.cfg.font(12.0, 500.0)).color(fg.alpha(0.5 * fg.a)))
        .child(chip(s, "pomodoro-break", "Break", accent::green(), !p.focus));
    // A ruler of the phase: lit ticks are the time left, the brightest one is now.
    const TICKS: usize = 36;
    let lit = ((p.left / p.length()) * TICKS as f64).ceil() as usize;
    let mut ruler = row().key("pomodoro-ruler").role("img").label(format!("{phase}, {} left", p.label()))
        .w_full().h(26.0).center_y().justify_between();
    for i in 0..TICKS {
        let tone = if i + 1 == lit { fg } else if i < lit { color.alpha(0.35 + 0.65 * (i + 1) as f32 / lit as f32) } else { color.alpha(0.18) };
        ruler = ruler.child(el().w(3.0).h(if i % 6 == 0 { 26.0 } else { 20.0 }).radius(1.5).fill(tone).a11y_hidden());
    }
    let controls = row().key("pomodoro-controls").w_full().gap(10.0).center_y()
        .child(tinted(s, "pomodoro-start", if p.running() { "Pause" } else { "Start Timer" }, color)
            .on_press_tx(|s: &mut Island, tx: &mut Tx| s.pomodoro.toggle(tx.clock())))
        .child(tool(s, "pomodoro-reset", &icons::ROTATE_CCW, "Reset").on_press(|s: &mut Island| s.pomodoro.reset()))
        .child(tool(s, "pomodoro-skip", &icons::SKIP_FORWARD, "Skip").on_press_tx(|s: &mut Island, tx: &mut Tx| s.pomodoro.skip(tx.clock())))
        .child(el().grow(1.0))
        .child(text(p.label()).key("pomodoro-time").font(s.cfg.font(40.0, 700.0).lh(44.0).features("'tnum' 1")).color(color).a11y_hidden());
    col().key("pomodoro").w_full().h_full().px(20.0).pt(16.0).pb(14.0).gap(14.0)
        .child(header).child(ruler).child(controls)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phases_alternate_and_count_rounds() {
        let mut p = Pomodoro::default();
        assert!(!p.tick(10.0));
        p.toggle(0.0);
        assert!(p.tick(60.0));
        assert_eq!(p.label(), "24:00");
        // Past focus and into the break.
        p.tick(FOCUS + 60.0);
        assert!(!p.focus && p.rounds == 1 && p.label() == "4:00");
        // A pause between ticks keeps the part second already gone.
        let mut q = Pomodoro::default();
        q.toggle(0.0);
        q.toggle(0.5);
        assert_eq!(q.left, FOCUS - 0.5);
        // Paused, the time holds.
        p.toggle(FOCUS + 60.0);
        assert!(!p.tick(FOCUS + 600.0) && p.label() == "4:00");
        p.skip(0.0);
        assert!(p.focus && p.left == FOCUS && p.rounds == 1);
        p.skip(0.0);
        assert!(!p.focus && p.rounds == 1);
        p.skip(0.0);
        p.reset();
        assert!(p.rounds == 1 && p.focus && !p.running());
    }
}
