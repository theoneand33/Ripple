//! Pomodoro: 25 minutes of focus, then a 5 minute break, round after round; beside it a timer
//! and a stopwatch. While one runs, the compact Island shows it beside a ring, as a live activity.

use rune::tok::TextStyle;
use rune::{El, Tx, col, el, icon, row, text};

use super::{accent, chip, ring, tinted, tool};
use crate::icons;
use crate::island::Island;

pub const FOCUS: f64 = 25.0 * 60.0;
pub const BREAK: f64 = 5.0 * 60.0;

/// Which of the tab's clocks shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Clock {
    #[default]
    Pomodoro,
    Timer,
    Stopwatch,
}

pub struct Pomodoro {
    pub clock: Clock,
    pub timer: Timer,
    pub stopwatch: Stopwatch,
    pub focus: bool,
    /// Seconds left, as of the last tick.
    pub left: f64,
    /// When the phase ends, on the app clock; None while paused.
    pub ends: Option<f64>,
    /// Focus phases finished.
    pub rounds: u32,
}

impl Default for Pomodoro {
    fn default() -> Self { Pomodoro { clock: Clock::Pomodoro, timer: Timer::default(), stopwatch: Stopwatch::default(), focus: true, left: FOCUS, ends: None, rounds: 0 } }
}

/// "4:05", or "1:02:03" past an hour.
fn clock_label(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

/// A countdown of `length` seconds. Once it runs out it has `rung` until reset or seen.
pub struct Timer {
    pub length: f64,
    pub left: f64,
    pub ends: Option<f64>,
    pub rung: bool,
}

impl Default for Timer {
    fn default() -> Self { Timer { length: 5.0 * 60.0, left: 5.0 * 60.0, ends: None, rung: false } }
}

impl Timer {
    pub fn label(&self) -> String { if self.rung { "Done".into() } else { clock_label(self.left.ceil()) } }

    pub fn percent(&self) -> f32 { ((1.0 - self.left / self.length) * 100.0).clamp(0.0, 100.0) as f32 }

    pub fn toggle(&mut self, now: f64) {
        if self.left <= 0.0 { self.reset() }
        self.ends = match self.ends { Some(ends) => { self.left = (ends - now).max(0.0); None } None => Some(now + self.left) };
    }

    /// Picks a length; a running timer starts over on it.
    pub fn set(&mut self, minutes: f64, now: f64) {
        (self.length, self.left, self.rung) = (minutes * 60.0, minutes * 60.0, false);
        if self.ends.is_some() { self.ends = Some(now + self.left) }
    }

    pub fn reset(&mut self) { (self.left, self.ends, self.rung) = (self.length, None, false) }

    fn tick(&mut self, now: f64) -> bool {
        let Some(ends) = self.ends.filter(|_| now.is_finite()) else { return false };
        if now >= ends { (self.left, self.ends, self.rung) = (0.0, None, true) } else { self.left = ends - now }
        true
    }
}

/// Counts up from when it started, less its pauses.
#[derive(Default)]
pub struct Stopwatch {
    pub elapsed: f64,
    /// The app clock when it would have started, had it never paused; None while paused.
    pub since: Option<f64>,
}

impl Stopwatch {
    pub fn label(&self) -> String { clock_label(self.elapsed) }

    /// The seconds hand: the ring goes round once a minute.
    pub fn percent(&self) -> f32 { (self.elapsed % 60.0 / 60.0 * 100.0) as f32 }

    pub fn toggle(&mut self, now: f64) {
        self.since = match self.since { Some(since) => { self.elapsed = now - since; None } None => Some(now - self.elapsed) };
    }

    pub fn reset(&mut self) { *self = Stopwatch::default() }

    fn tick(&mut self, now: f64) -> bool {
        let Some(since) = self.since.filter(|_| now.is_finite()) else { return false };
        self.elapsed = now - since;
        true
    }
}

impl Pomodoro {
    pub fn running(&self) -> bool { self.ends.is_some() }

    /// A clock the compact Island shows: one running, or a timer that has rung.
    pub fn active(&self) -> bool { self.running() || self.timer.ends.is_some() || self.timer.rung || self.stopwatch.since.is_some() }

    /// The time the live activity shows.
    pub fn live_label(&self) -> String {
        if self.running() { self.label() } else if self.timer.ends.is_some() || self.timer.rung { self.timer.label() } else { self.stopwatch.label() }
    }

    /// Ticks every clock; false while none runs.
    pub fn tick_all(&mut self, now: f64) -> bool { self.tick(now) | self.timer.tick(now) | self.stopwatch.tick(now) }

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

/// The compact Island's live activity: the Pomodoro, else the timer, else the stopwatch, as a
/// ring and its time.
pub fn live(p: &Pomodoro, style: TextStyle) -> El {
    let (key, glyph, tint, percent, label) = shown(p);
    row().key(key).role("img").label(label).gap(6.0).center_y()
        .child(ring(18.0, percent, tint, icon(glyph, 10.0).color(tint)))
        .child(text(p.live_label()).font(style.features("'tnum' 1")).color(tint).a11y_hidden())
}

/// The clock the Island shows: the Pomodoro, else the timer, else the stopwatch, as its key,
/// icon, colour, ring percent and spoken label.
fn shown(p: &Pomodoro) -> (&'static str, &'static rune::Icon, rune::Color, f32, String) {
    if p.running() {
        ("pomodoro", if p.focus { &icons::TIMER } else { &icons::COFFEE }, color(p.focus), f32::from(p.percent()), format!("{}, {} left", if p.focus { "Focus" } else { "Break" }, p.label()))
    } else if p.timer.ends.is_some() || p.timer.rung {
        ("timer", &icons::TIMER, if p.timer.rung { accent::red() } else { accent::teal() }, p.timer.percent(), if p.timer.rung { "Timer done".into() } else { format!("Timer, {} left", p.timer.label()) })
    } else {
        ("stopwatch", &icons::TIMER, accent::indigo(), p.stopwatch.percent(), format!("Stopwatch, {}", p.stopwatch.label()))
    }
}

/// A chip that picks one of the tab's clocks.
fn clock_chip(s: &Island, clock: Clock, label: &str, tint: rune::Color) -> El {
    chip(s, &format!("clock-{label}"), label, tint, s.pomodoro.clock == clock).role("button").label(label).focusable(true)
        .checked(s.pomodoro.clock == clock).on_press(move |s: &mut Island| s.pomodoro.clock = clock)
}

/// The timer or the stopwatch: the live activity's ring, large, with its time inside.
fn dial(s: &Island, key: &str, percent: f32, tint: rune::Color, label: String) -> El {
    ring(104.0, percent, tint, text(label).key(format!("{key}-time")).font(s.cfg.font(22.0, 700.0).features("'tnum' 1")).color(tint))
}

fn timer_view(s: &Island) -> El {
    let t = &s.pomodoro.timer;
    let tint = if t.rung { accent::red() } else { accent::teal() };
    let mut presets = row().key("timer-presets").gap(6.0);
    for m in [1.0, 5.0, 10.0, 15.0, 30.0] {
        presets = presets.child(chip(s, &format!("timer-{m}"), &format!("{m}m"), tint, t.length == m * 60.0)
            .role("button").label(format!("{m} minutes")).focusable(true)
            .on_press_tx(move |s: &mut Island, tx: &mut Tx| s.pomodoro.timer.set(m, tx.clock())));
    }
    row().key("timer").w_full().gap(18.0).center_y()
        .child(dial(s, "timer", t.percent(), tint, t.label()))
        .child(col().gap(12.0).child(presets).child(row().gap(10.0).center_y()
            .child(tinted(s, "timer-start", if t.ends.is_some() { "Pause" } else { "Start" }, tint)
                .on_press_tx(|s: &mut Island, tx: &mut Tx| s.pomodoro.timer.toggle(tx.clock())))
            .child(tool(s, "timer-reset", &icons::ROTATE_CCW, "Reset").on_press(|s: &mut Island| s.pomodoro.timer.reset()))))
}

fn stopwatch_view(s: &Island) -> El {
    let w = &s.pomodoro.stopwatch;
    let tint = accent::indigo();
    row().key("stopwatch").w_full().gap(18.0).center_y()
        .child(dial(s, "stopwatch", w.percent(), tint, w.label()))
        .child(row().gap(10.0).center_y()
            .child(tinted(s, "stopwatch-start", if w.since.is_some() { "Pause" } else { "Start" }, tint)
                .on_press_tx(|s: &mut Island, tx: &mut Tx| s.pomodoro.stopwatch.toggle(tx.clock())))
            .child(tool(s, "stopwatch-reset", &icons::ROTATE_CCW, "Reset").on_press(|s: &mut Island| s.pomodoro.stopwatch.reset())))
}

/// The ring alone, for the bubble beside the compact Island while music has the Island.
pub fn bubble(p: &Pomodoro, size: f32) -> El {
    let (_, glyph, tint, percent, label) = shown(p);
    el().role("img").label(label).child(ring(size, percent, tint, icon(glyph, size / 2.0).color(tint)))
}

pub fn view(s: &Island) -> El {
    let clocks = row().key("clocks").w_full().gap(6.0)
        .child(clock_chip(s, Clock::Pomodoro, "Pomodoro", accent::orange()))
        .child(clock_chip(s, Clock::Timer, "Timer", accent::teal()))
        .child(clock_chip(s, Clock::Stopwatch, "Stopwatch", accent::indigo()));
    let root = col().key("clock").w_full().h_full().px(20.0).pt(14.0).pb(14.0).gap(14.0).child(clocks);
    match s.pomodoro.clock {
        Clock::Pomodoro => root.child(pomodoro_view(s)),
        Clock::Timer => root.child(timer_view(s)),
        Clock::Stopwatch => root.child(stopwatch_view(s)),
    }
}

fn pomodoro_view(s: &Island) -> El {
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
    col().key("pomodoro").w_full().gap(14.0).child(header).child(ruler).child(controls)
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

    #[test]
    fn the_timer_rings_once_and_the_stopwatch_keeps_its_pauses() {
        let mut p = Pomodoro::default();
        assert!(!p.active() && !p.tick_all(1.0));
        p.timer.set(1.0, 0.0);
        p.timer.toggle(0.0);
        assert!(p.tick_all(30.0) && p.active() && p.live_label() == "0:30");
        p.tick_all(61.0);
        assert!(p.timer.rung && p.timer.ends.is_none() && p.live_label() == "Done" && p.active());
        // Starting a rung timer starts it afresh.
        p.timer.toggle(100.0);
        assert!(!p.timer.rung && p.timer.left == 60.0);
        p.timer.reset();
        p.stopwatch.toggle(10.0);
        p.tick_all(20.0);
        p.stopwatch.toggle(25.0);
        p.stopwatch.toggle(100.0);
        p.tick_all(105.0);
        assert_eq!((p.stopwatch.elapsed, p.live_label()), (20.0, "0:20".to_string()));
        p.stopwatch.elapsed = 3723.0;
        assert_eq!(p.stopwatch.label(), "1:02:03");
    }
}
