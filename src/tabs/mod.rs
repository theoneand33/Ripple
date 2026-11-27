//! What the Island shows in Large mode: one module per tab, each with its state, its actions
//! and its view.

mod agents;
mod assistant;
mod calculator;
mod calendar;
mod clipboard;
mod clocks;
mod discord;
mod controls;
mod colors;
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
mod sync;
mod t3;
mod tasks;
mod weather;
mod workflows;

pub use agents::{Agents, live as agents_live};
pub use assistant::Assistant;
pub use calculator::Calculator;
pub use calendar::Calendar;
pub use clipboard::{Clipboard, Content};
pub use overview::{Contributions, Dashboard, height as overview_height, width as overview_width};
pub use clocks::Clocks;
pub use discord::{Discord, live as discord_live, widget as discord_widget};
pub use controls::Controls;
pub use colors::Colors;
pub use habits::Habits;
pub use limits::Limits;
pub use notes::Notes;
pub use shelf::Shelf;
pub use pomodoro::{Clock, Pomodoro, bubble as pomodoro_bubble, live as pomodoro_live};
pub use search::Finder;
pub use stocks::Stocks;
pub use stremflix::Stremflix;
pub use sync::{SyncState, Syncs};
pub use t3::T3;
pub use tasks::{TaskDraft, Tasks};

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
        Tab::Discord => discord::view(s),
        Tab::Controls => controls::view(s),
        Tab::Colors => colors::view(s),
    }
}

/// The Large Island's size on each tab, before any notch inset.
pub const fn size(tab: Tab) -> (f32, f32) {
    match tab {
        Tab::Search => (480.0, 330.0),
        Tab::Workflows => (520.0, 250.0),
        Tab::Notes | Tab::Stremflix | Tab::Limits | Tab::Discord => (520.0, 290.0),
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
        Tab::Controls => (440.0, 230.0),
        Tab::Colors => (520.0, 240.0),
    }
}

/// The tab's icon in the dock and in Settings.
pub fn icon_of(tab: Tab) -> &'static rune::Icon {
    match tab {
        Tab::Search => &icons::SEARCH,
        Tab::Workflows => &icons::ZAP,
        Tab::Overview => &icons::SUN,
        Tab::Weather => &icons::CLOUD_SUN,
        Tab::Assistant => &icons::MIC,
        Tab::Clipboard => &icons::LIST,
        Tab::Tasks => &icons::CHECK,
        Tab::T3 => &icons::TERMINAL,
        Tab::Agents => &icons::BOT,
        Tab::Notes => &icons::STICKY_NOTE,
        Tab::Stremflix => &icons::TV,
        Tab::Shelf => &icons::INBOX,
        Tab::System => &icons::ACTIVITY,
        Tab::Limits => &icons::GAUGE,
        Tab::Calendar => &icons::CALENDAR,
        Tab::Plugins => &icons::PUZZLE,
        Tab::Pomodoro => &icons::TIMER,
        Tab::Calculator => &icons::CALCULATOR,
        Tab::Habits => &icons::FLAME,
        Tab::Stocks => &icons::TRENDING_UP,
        Tab::Clocks => &icons::CLOCK,
        Tab::Discord => &icons::HEADPHONES,
        Tab::Controls => &icons::POWER,
        Tab::Colors => &icons::PIPETTE,
    }
}

/// Whether a tab has anything to show: T3 Code needs a device, AI Limits a signed-in agent once
/// looked for, Discord an app set up in Settings.
pub fn has_content(s: &Island, tab: Tab) -> bool {
    match tab {
        Tab::T3 => !s.cfg.t3_devices.is_empty(),
        Tab::Discord => s.discord_app().is_some(),
        Tab::Limits => !s.limits.usage.as_ref().is_some_and(Vec::is_empty),
        _ => true,
    }
}

/// Loads what a tab shows, as it opens: the Overview's panels with it.
pub fn entered(s: &mut Island) {
    s.dashboard.drag = None;
    s.dashboard.add = None;
    load(s, s.tab);
    if s.tab == Tab::Overview {
        for tile in s.cfg.dashboard.clone() { load_widget(s, tile.widget) }
    }
}

/// Loads what an Overview widget shows, as it appears.
fn load_widget(s: &mut Island, widget: Widget) {
    match widget {
        Widget::Panel(tab) => load(s, tab),
        Widget::Contributions => s.contributions_load(),
        Widget::Today | Widget::Day => s.calendar_feeds_load(),
        _ => {}
    }
}

/// Whether Large mode shows a tab: open, or as a panel on the Overview.
pub fn shows(s: &Island, tab: Tab) -> bool { shows_widget(s, Widget::Panel(tab)) }

/// Whether Large mode shows a widget: on the open Overview, or a panel's tab open itself.
fn shows_widget(s: &Island, widget: Widget) -> bool {
    s.mode == Mode::Large && (widget == Widget::Panel(s.tab) || s.tab == Tab::Overview && s.cfg.on_dashboard(widget))
}

/// Counts a reload's wait down a second; true once it is over.
fn waited(countdown: &mut Option<u8>) -> bool {
    match countdown.take() {
        Some(n) if n > 1 => { *countdown = Some(n - 1); false }
        Some(_) => true,
        None => false,
    }
}

fn load(s: &mut Island, tab: Tab) {
    match tab {
        Tab::Search => s.search_load(),
        Tab::T3 => s.t3_load(),
        Tab::Stremflix => s.stremflix_load(),
        Tab::Limits => s.limits_load(),
        Tab::Calculator => s.rates_load(),
        Tab::Stocks => s.stocks_load(),
        Tab::Clocks => s.clocks_load(),
        Tab::Controls => s.controls_load(),
        Tab::Notes => s.nextcloud_sync(Kind::Notes),
        Tab::Tasks => tasks::entered(s),
        Tab::Calendar => calendar::entered(s),
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
    s.task_reminders();
    s.event_alerts();
    let agents = s.agents_poll(now);
    if waited(&mut s.stocks.reload_in) && shows(s, Tab::Stocks) { s.stocks_load() }
    if waited(&mut s.contributions.reload_in) && shows_widget(s, Widget::Contributions) { s.contributions_load() }
    s.pomodoro.tick_all(crate::date::wall()) | plugins | agents
}

/// Before every frame, from `Island::poll`.
pub fn poll(s: &mut Island, now: f64, tx: &mut Tx) -> bool {
    if shows(s, Tab::Search) { s.search_poll() }
    plugins::poll(s, tx) | s.clipboard_poll(now, tx) | tasks::tick_due(s, now, tx)
}

/// The next time a tab needs a poll.
pub fn deadline(s: &Island) -> Option<f64> { [s.clipboard.copied.map(|c| c.1), tasks::tick_deadline(s)].into_iter().flatten().reduce(f64::min) }

/// Picks up a settings change another window made, for the tab showing.
pub fn reloaded(s: &mut Island, old: &Config) {
    if shows(s, Tab::T3) && old.t3_devices != s.cfg.t3_devices { s.t3_load() }
    // Settings saves each keystroke: wait until typing stops, and drop answers for what was typed over.
    if (old.sure_url != s.cfg.sure_url || old.sure_key != s.cfg.sure_key || old.stocks_currency != s.cfg.stocks_currency) && !s.cfg.sure_key.is_empty() {
        (s.stocks.portfolio, s.stocks.generation, s.stocks.reload_in) = (None, s.stocks.generation + 1, Some(2));
    }
    if old.github_user != s.cfg.github_user || old.leetcode_user != s.cfg.leetcode_user { s.contributions.reload_in = Some(2) }
}

impl Island {
    /// Loads exchange rates for Calculator, Search and Stocks, unless they are under a day old.
    pub fn rates_load(&mut self) {
        if self.rates.asked || self.rates.fresh() { return }
        let Some(me) = self.me.clone() else { return };
        self.rates.asked = true;
        me.spawn(crate::net::rates, |s, rates| { s.rates.asked = false; s.rates.got(rates) });
    }
}

/// What a delete took: a note or an event and where it sat, or the cleared Shelf.
pub enum Undo {
    Item(Kind, usize, Item),
    Shelf(Vec<std::path::PathBuf>),
}

impl Undo {
    /// The tab whose panel offers it.
    fn tab(&self) -> Tab {
        match self {
            Undo::Item(Kind::Notes, ..) => Tab::Notes,
            Undo::Item(Kind::Tasks, ..) => Tab::Tasks,
            Undo::Item(Kind::Events, ..) => Tab::Calendar,
            Undo::Shelf(_) => Tab::Shelf,
        }
    }
}

impl Island {
    pub fn undo(&mut self) {
        match self.undo.take() {
            Some(Undo::Item(kind, i, item)) => {
                let (key, syncing) = (item.key, self.nextcloud[kind].running);
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

/// A `chip` the user presses: it lightens under the pointer and dips when pressed. The caller gives its role.
fn chip_button(s: &Island, key: &str, label: &str, color: Color, on: bool) -> El {
    let fg = s.cfg.text();
    chip(s, key, label, color, on).focusable(true)
        .hover(if on { color.alpha(0.3) } else { fg.alpha(0.18 * fg.a) }).press(if on { color.alpha(0.38) } else { fg.alpha(0.26 * fg.a) }).press_scale(0.97)
}

/// A live activity the compact Island shows as an icon and a short label, and its bubble as the icon alone.
pub struct Live {
    pub key: &'static str,
    pub glyph: &'static rune::Icon,
    pub tint: Color,
    pub label: String,
    /// What a screen reader hears for it.
    pub spoken: String,
}

impl Live {
    /// The glance's right side.
    pub fn view(&self, style: rune::tok::TextStyle) -> El {
        row().key(self.key).role("img").label(self.spoken.as_str()).gap(6.0).center_y()
            .child(icon(self.glyph, 16.0).color(self.tint))
            .child(text(&self.label).font(style).color(self.tint).max_w(120.0).truncate().a11y_hidden())
    }

    /// The bubble's content.
    pub fn bubble(&self, size: f32) -> El {
        el().key(self.key).role("img").label(self.spoken.as_str()).child(icon(self.glyph, size * 0.45).color(self.tint).a11y_hidden())
    }
}

/// `text` cut to `max` characters, an ellipsis ending a cut one: the compact Island sizes to its label.
pub fn clip(text: &str, max: usize) -> String {
    if text.chars().count() > max { format!("{}…", text.chars().take(max.saturating_sub(1)).collect::<String>()) } else { text.to_string() }
}

/// A progress ring in one colour, its track the same colour faint, with `inner` in its middle.
pub fn ring(size: f32, percent: f32, color: Color, inner: El) -> El {
    el().size(size, size).center().a11y_hidden()
        .child(icon(icons::ring(percent.clamp(0.0, 100.0).round() as u8), size).color(color).pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)))
        .child(inner)
}

/// A ring lit clockwise from the top up to `percent`: green, orange, then red as load rises.
fn gauge(s: &Island, percent: f32, high_good: bool) -> El {
    let fg = s.cfg.text();
    let p = percent.clamp(0.0, 100.0);
    let load = if high_good { 100.0 - p } else { p };
    let lit = if load < 60.0 { accent::green() } else if load < 85.0 { accent::orange() } else { accent::red() };
    ring(44.0, p, lit, text(format!("{p:.0}%")).font(s.cfg.font(10.0, 700.0).features("'tnum' 1")).color(fg))
}

/// The fill of a list row: a raised grey card, no outline.
fn card(fg: Color) -> Color { fg.alpha(0.09 * fg.a) }

/// A text button shaped like `tool`, so it sits quietly in a tab's header.
fn pill(s: &Island, key: &str, label: &str) -> El {
    let fg = s.cfg.text();
    el().key(key).role("button").label(label).focusable(true)
        .h(28.0).px(12.0).center().radius(s.cfg.radius(14.0)).fill(fg.alpha(0.1 * fg.a))
        .hover(fg.alpha(0.18 * fg.a)).press(fg.alpha(0.26 * fg.a)).press_scale(0.97)
        .child(text(label).font(s.cfg.font(13.0, 600.0)).color(fg).a11y_hidden())
}

/// Undo for the last delete, on the panel it happened on, while its tab stays open.
fn undo_button(s: &Island, tab: Tab) -> Option<El> {
    s.undo.as_ref().is_some_and(|u| u.tab() == tab).then(|| pill(s, "undo", "Undo").on_press(Island::undo))
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
fn field(s: &Island, key: &str, edit: &TextEdit, placeholder: &str, access: impl for<'a> Fn(&'a mut Island) -> &'a mut TextEdit + 'static, submit: impl Fn(&mut Island, &mut Tx) + 'static) -> El {
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
        .hover(fg.alpha(0.18 * fg.a)).press(fg.alpha(0.26 * fg.a)).press_scale(0.97)
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
        .hover(fg.alpha(0.15 * fg.a)).press(fg.alpha(0.25 * fg.a)).press_scale(0.97);
    if checked { b = b.child(icon(&icons::CHECK, 12.0).color(bg)) }
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Store;

    #[test]
    fn undo_shows_only_on_the_panel_whose_item_went() {
        let cfg = Config { events: vec![Item::new("2026-01-01 Standup")], notes: vec![Item::new("Old")], ..Config::default() };
        let mut s = Island::new(Store::memory(cfg), Vec::new());
        let key = s.cfg.events[0].key;
        s.delete_item(Kind::Events, key);
        assert!(s.cfg.events.is_empty());
        assert!(undo_button(&s, Tab::Calendar).is_some());
        assert!(undo_button(&s, Tab::Notes).is_none(), "an Overview with both panels offers it once");
        s.undo();
        assert_eq!(s.cfg.events.iter().map(|e| e.key).collect::<Vec<_>>(), [key], "back where it was");
    }
}
