//! What the Island shows in Large mode: one module per tab, each with its state, its actions
//! and its view.

mod agents;
mod assistant;
mod calculator;
mod calendar;
mod clipboard;
mod clocks;
mod habits;
mod limits;
mod monitor;
mod notes;
mod overview;
mod plugins;
mod pomodoro;
mod search;
mod shelf;
mod stocks;
mod stremflix;
mod t3;
mod tasks;
mod weather;
mod workflows;

pub use agents::Agents;
pub use assistant::Assistant;
pub use calculator::Calculator;
pub use calendar::{link_host, Calendar};
pub use clipboard::{Clipboard, Content};
pub use overview::Contributions;
pub use clocks::Clocks;
pub use limits::Limits;
pub use notes::Notes;
pub use shelf::Shelf;
pub use pomodoro::{Clock, Pomodoro, bubble as pomodoro_bubble, live as pomodoro_live};
pub use search::Finder;
pub use stocks::Stocks;
pub use stremflix::Stremflix;
pub use t3::T3;
pub use tasks::{Every, Task, TaskDraft};

use rune::{Color, El, Key, KeyEvent, ScrollSpec, TextEdit, Tx, el, icon, row, text, text_input};

use crate::config::{Config, Item, Tab, Widget};
use crate::nextcloud::{self, Kind};
use crate::icons;
use crate::island::{Island, Mode};

pub fn view(s: &Island, layer: &mut Vec<El>) -> El { panel(s, s.tab, layer) }

/// A tab's view, on its own or as a tile on the Overview.
fn panel(s: &Island, tab: Tab, layer: &mut Vec<El>) -> El {
    match tab {
        Tab::Search => search::view(s),
        Tab::Workflows => workflows::view(s),
        Tab::Overview => overview::view(s, layer),
        Tab::Weather => weather::view(s),
        Tab::Assistant => assistant::view(s),
        Tab::Clipboard => clipboard::view(s),
        Tab::Tasks => tasks::view(s),
        Tab::T3 => t3::view(s, layer),
        Tab::Agents => agents::view(s),
        Tab::Notes => notes::view(s),
        Tab::Stremflix => stremflix::view(s),
        Tab::Shelf => shelf::view(s, layer),
        Tab::System => monitor::view(s),
        Tab::Limits => limits::view(s),
        Tab::Calendar => calendar::view(s),
        Tab::Plugins => plugins::view(s),
        Tab::Pomodoro => pomodoro::view(s),
        Tab::Calculator => calculator::view(s),
        Tab::Habits => habits::view(s),
        Tab::Stocks => stocks::view(s),
        Tab::Clocks => clocks::view(s),
    }
}

/// The Large Island's size on each tab, before any notch inset.
pub const fn size(tab: Tab) -> (f32, f32) {
    match tab {
        Tab::Search => (480.0, 330.0),
        Tab::Workflows => (520.0, 250.0),
        Tab::Notes | Tab::Stremflix | Tab::Limits => (520.0, 290.0),
        Tab::Shelf => (520.0, 230.0),
        Tab::System => (520.0, 342.0),
        Tab::T3 | Tab::Plugins | Tab::Calculator => (420.0, 290.0),
        Tab::Tasks | Tab::Agents => (480.0, 320.0),
        Tab::Habits => (520.0, 290.0),
        Tab::Pomodoro => (440.0, 236.0),
        Tab::Calendar => (560.0, 280.0),
        Tab::Overview => (560.0, 320.0),
        Tab::Stocks => (560.0, 350.0),
        Tab::Assistant => (420.0, 230.0),
        Tab::Clipboard => (420.0, 290.0),
        Tab::Weather => (420.0, 190.0),
        Tab::Clocks => (480.0, 300.0),
    }
}

/// Loads what a tab shows, as it opens: the Overview's panels with it.
pub fn entered(s: &mut Island) {
    s.dashboard_drag = None;
    load(s, s.tab);
    if s.tab == Tab::Overview {
        for tile in s.cfg.dashboard.clone() { if let Widget::Panel(tab) = tile.widget { load(s, tab) } }
        if s.cfg.dashboard.iter().any(|t| t.widget == Widget::Contributions) { s.contributions_load() }
    }
}

/// Whether Large mode shows a tab: open, or as a panel on the Overview.
pub fn shows(s: &Island, tab: Tab) -> bool {
    s.mode == Mode::Large && (s.tab == tab || s.tab == Tab::Overview && s.cfg.dashboard.iter().any(|t| t.widget == Widget::Panel(tab)))
}

fn load(s: &mut Island, tab: Tab) {
    match tab {
        Tab::Search => s.search_load(),
        Tab::T3 => s.t3_load(),
        Tab::Stremflix => s.stremflix_load(),
        Tab::Limits => s.limits_load(),
        Tab::Calculator => s.calc_load(),
        Tab::Stocks => s.stocks_load(),
        Tab::Clocks => s.clocks_load(),
        Tab::Notes => s.nextcloud_sync(Kind::Notes),
        Tab::Tasks => {
            if s.task_draft.editing.is_some() { tasks::reset_draft(s) }
            s.nextcloud_sync(Kind::Tasks);
        }
        Tab::Calendar => {
            (s.calendar.offset, s.calendar.day) = (0, None);
            if s.calendar.editing.take().is_some() { s.calendar.event.clear() }
            s.nextcloud_sync(Kind::Events);
            s.calendar_feeds_load();
        }
        Tab::Shelf => s.nextcloud_shelf_sync(),
        Tab::Plugins => s.plugins.scan(),
        // Seen: a rung timer leaves the compact Island.
        Tab::Pomodoro => s.pomodoro.timer.rung = false,
        _ => {}
    }
}

/// Once a second, from `Island::poll`.
pub fn each_second(s: &mut Island, now: f64) -> bool {
    let plugins = shows(s, Tab::Plugins) && s.plugins.tick();
    s.notes_tick();
    s.task_reminders(now);
    s.event_alerts(now);
    s.notifications(now);
    let agents = s.agents_poll(now);
    let theme = s.cfg.sun_theme(s.weather.sun, crate::date::now_secs()).filter(|t| *t != s.cfg.theme).map(str::to_string);
    if let Some(theme) = theme { s.save(|c| c.set_theme(&theme)) }
    s.water_reminders(now);
    s.rain_alert(now);
    s.break_reminders(now);
    crate::system::WANT_PROCESSES.store(s.mode == Mode::Large && s.tab == Tab::System, std::sync::atomic::Ordering::Relaxed);
    match s.stocks.reload_in.take() {
        Some(n) if n > 1 => s.stocks.reload_in = Some(n - 1),
        Some(_) if shows(s, Tab::Stocks) => s.stocks_load(),
        _ => {}
    }
    match s.contributions.reload_in.take() {
        Some(n) if n > 1 => s.contributions.reload_in = Some(n - 1),
        Some(_) if s.mode == Mode::Large && s.tab == Tab::Overview && s.cfg.dashboard.iter().any(|t| t.widget == Widget::Contributions) => s.contributions_load(),
        _ => {}
    }
    s.pomodoro.tick_all(now) | plugins | agents
}

/// Before every frame, from `Island::poll`.
pub fn poll(s: &mut Island, now: f64, tx: &mut Tx) -> bool {
    if shows(s, Tab::Search) { s.search_poll() }
    s.clipboard_poll(now, tx)
}

/// The next time a tab needs a poll.
pub fn deadline(s: &Island) -> Option<f64> { s.clipboard.copied.map(|c| c.1) }

/// Picks up a settings change another window made, for the tab showing.
pub fn reloaded(s: &mut Island, old: &Config) {
    if shows(s, Tab::T3) && old.t3_devices != s.cfg.t3_devices { s.t3_load() }
    // Settings saves each keystroke: wait until typing stops, and drop answers for what was typed over.
    if (old.sure_url != s.cfg.sure_url || old.sure_key != s.cfg.sure_key) && !s.cfg.sure_key.is_empty() {
        (s.stocks.portfolio, s.stocks.generation, s.stocks.reload_in) = (None, s.stocks.generation + 1, Some(2));
    }
    if old.github_user != s.cfg.github_user || old.leetcode_user != s.cfg.leetcode_user { s.contributions.reload_in = Some(2) }
}

/// What a delete took: a note or an event and where it sat, or the cleared Shelf.
pub enum Undo {
    Item(Kind, usize, Item),
    Shelf(Vec<std::path::PathBuf>),
}

impl Island {
    pub fn undo(&mut self) {
        match self.undo.take() {
            Some(Undo::Item(kind, i, item)) => {
                let (key, syncing) = (item.key, self.nextcloud[kind as usize].running);
                self.save(|c| nextcloud::restore(c, kind, i, item, syncing));
                if kind == Kind::Notes { self.open_note(self.cfg.notes.iter().position(|n| n.key == key).unwrap_or(0)) }
                self.nextcloud_sync(kind);
            }
            // Files dropped since stay in front.
            Some(Undo::Shelf(files)) => {
                self.save(|c| { let back: Vec<_> = files.into_iter().filter(|f| !c.shelf.contains(f)).collect(); c.shelf.extend(back) });
                self.nextcloud_shelf_sync();
            }
            None => {}
        }
    }
}

/// A tab's vertical scroll. A scroll view takes the wheel before the Island does, so it passes
/// sideways swipes on to `Island::wheel` to keep them changing tab.
pub trait Scrolls {
    fn scrolls(self) -> Self;
}

impl Scrolls for El {
    fn scrolls(self) -> El { self.scroll(ScrollSpec { stick_bottom: false }).on_wheel(Island::wheel) }
}

/// Droppy-style accents: each widget has one colour for its rings, chips and figures; and the
/// status hues of the pill border and alerts.
pub mod accent {
    use rune::Color;
    // Colours are linear: decode the sRGB hex, as `config::parse_color` does.
    fn hex(s: &str) -> Color { rune::color::hex(s).unwrap_or(Color::WHITE) }
    pub fn charge() -> Color { hex("#4cc38a") }
    pub fn low() -> Color { hex("#e5484d") }
    pub fn camera() -> Color { hex("#e8b931") }
    pub fn mic() -> Color { hex("#f0883e") }
    pub fn blue() -> Color { hex("#4c9aff") }
    pub fn bluetooth() -> Color { blue() }
    pub fn orange() -> Color { hex("#FF9F0A") }
    pub fn red() -> Color { hex("#FF453A") }
    pub fn indigo() -> Color { hex("#7D86F0") }
    pub fn green() -> Color { hex("#30D158") }
    pub fn teal() -> Color { hex("#40C8E0") }
    pub fn pink() -> Color { hex("#FA586A") }
}

/// A button tinted with an accent: the accent faint as fill, full as ink.
fn tinted(s: &Island, key: &str, label: &str, color: Color) -> El {
    el().key(key).role("button").label(label).focusable(true)
        .px(20.0).h(40.0).center().radius(s.cfg.radius(20.0))
        .fill(color.alpha(0.22)).hover(color.alpha(0.3)).press(color.alpha(0.38)).press_scale(0.97)
        .child(text(label).font(s.cfg.font(15.0, 700.0)).color(color).a11y_hidden())
}

/// A label pill: tinted with its accent while on, grey while off.
fn chip(s: &Island, key: &str, label: &str, color: Color, on: bool) -> El {
    let fg = s.cfg.text();
    el().key(key).h(28.0).px(12.0).center().radius(s.cfg.radius(14.0))
        .fill(if on { color.alpha(0.22) } else { fg.alpha(0.1 * fg.a) })
        .child(text(label).font(s.cfg.font(13.0, 700.0)).color(if on { color } else { fg.alpha(0.5 * fg.a) }))
}

/// A progress ring in one colour, its track the same colour faint, with `inner` in its middle.
pub fn ring(size: f32, percent: f32, color: Color, inner: El) -> El {
    el().size(size, size).center().a11y_hidden()
        .child(icon(icons::ring(percent.clamp(0.0, 100.0).round() as u8), size).color(color).pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)))
        .child(inner)
}

/// The fill of a list row: a raised grey card, no outline.
fn card(fg: Color) -> Color { fg.alpha(0.09 * fg.a) }

/// A text button shaped like `tool`, so it sits quietly in a tab's header.
fn pill(s: &Island, key: &str, label: &str) -> El {
    let fg = s.cfg.text();
    el().key(key).role("button").label(label).focusable(true)
        .h(28.0).px(12.0).center().radius(s.cfg.radius(14.0)).fill(fg.alpha(0.1 * fg.a))
        .hover(fg.alpha(0.18 * fg.a)).press(fg.alpha(0.26 * fg.a))
        .child(text(label).font(s.cfg.font(13.0, 600.0)).color(fg).a11y_hidden())
}

/// Undo for the last delete, while the tab it happened on stays open.
fn undo_button(s: &Island) -> Option<El> {
    s.undo.is_some().then(|| pill(s, "undo", "Undo").on_press(Island::undo))
}

/// Left and Right switch tabs even while typing, as the Electron app's window-wide key
/// handler did. The T3 message box keeps them for its caret.
fn tab_arrows(s: &mut Island, ev: &KeyEvent, tx: &mut Tx) -> bool {
    matches!(ev.key, Key::Left | Key::Right) && s.key(ev, tx)
}

/// The Island's filled button: the text colour as fill, the Island colour as ink.
fn button(s: &Island, key: &str, label: &str, enabled: bool) -> El {
    let (bg, fg) = (s.cfg.text(), s.cfg.bg().alpha(1.0));
    // Round ends, as every one-line control in the Island has.
    el().key(key).role("button").label(label).focusable(enabled).disabled(!enabled)
        .px(18.0).h(34.0).center().radius(s.cfg.radius(17.0))
        .fill(bg.alpha(if enabled { bg.a } else { 0.4 * bg.a }))
        .hover(fg.alpha(0.1)).press(fg.alpha(0.2)).press_scale(0.97)
        .child(text(label).font(s.cfg.font(14.0, 600.0)).color(fg).a11y_hidden())
}

/// A one-line field in the Island's colours.
fn field(s: &Island, key: &str, edit: &TextEdit, placeholder: &str, access: fn(&mut Island) -> &mut TextEdit, submit: fn(&mut Island, &mut Tx)) -> El {
    let fg = s.cfg.text();
    let input = text_input(edit, s.cfg.font(14.0, 400.0).lh(20.0)).key(key).label(placeholder).single_line().grow(1.0).min_w(0.0)
        .color(fg).placeholder(placeholder, fg.alpha(0.5 * fg.a))
        .on_edit(access).on_submit(submit).on_input_key(tab_arrows);
    row().key(format!("{key}-field")).grow(1.0).min_w(0.0).h(36.0).px(14.0).center_y()
        .radius(s.cfg.radius(18.0))
        .fill(fg.alpha(0.05 * fg.a)).border(1.0, fg.alpha(0.1 * fg.a))
        .child(input)
}

/// A round icon button for a tab's header.
fn tool(s: &Island, key: &str, glyph: &'static rune::Icon, label: &str) -> El {
    let fg = s.cfg.text();
    el().key(key).role("button").label(label).focusable(true)
        .size(28.0, 28.0).center().radius(s.cfg.radius(14.0)).fill(fg.alpha(0.1 * fg.a))
        .hover(fg.alpha(0.18 * fg.a)).press(fg.alpha(0.26 * fg.a))
        .child(icon(glyph, 15.0).color(fg).a11y_hidden())
}

fn empty(s: &Island, message: &str) -> El {
    let fg = s.cfg.text();
    el().key("empty").w_full().pt(30.0).center().child(text(message).font(s.cfg.font(13.0, 400.0)).color(fg.alpha(0.5 * fg.a)))
}

fn checkbox(s: &Island, key: &str, label: &str, checked: bool) -> El {
    let (bg, fg) = (s.cfg.bg().alpha(1.0), s.cfg.text());
    let mut b = el().key(key).role("checkbox").label(label).checked(checked).focusable(true)
        .size(18.0, 18.0).center().radius(s.cfg.radius(6.0))
        .fill(if checked { fg } else { Color::TRANSPARENT }).border(1.5, fg.alpha(if checked { fg.a } else { 0.5 * fg.a }))
        .hover(fg.alpha(0.15 * fg.a));
    if checked { b = b.child(icon(&icons::CHECK, 12.0).color(bg)) }
    b
}
