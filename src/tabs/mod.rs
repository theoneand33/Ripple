//! What the Island shows in Large mode: one module per tab, each with its state, its actions
//! and its view.

mod assistant;
mod calendar;
mod clipboard;
mod monitor;
mod notes;
mod overview;
mod search;
mod shelf;
mod stremflix;
mod t3;
mod tasks;
mod workflows;

pub use assistant::Assistant;
pub use clipboard::Clipboard;
pub use notes::Notes;
pub use stremflix::Stremflix;
pub use t3::T3;

use rune::{Color, El, Key, KeyEvent, TextEdit, Tx, el, icon, row, text, text_input};

use crate::config::{Config, Tab};
use crate::icons;
use crate::island::{Island, Mode};

pub fn view(s: &Island, layer: &mut Vec<El>) -> El {
    match s.tab {
        Tab::Search => search::view(s),
        Tab::Workflows => workflows::view(s),
        Tab::Overview => overview::view(s),
        Tab::Assistant => assistant::view(s),
        Tab::Clipboard => clipboard::view(s),
        Tab::Tasks => tasks::view(s),
        Tab::T3 => t3::view(s, layer),
        Tab::Notes => notes::view(s),
        Tab::Stremflix => stremflix::view(s),
        Tab::Shelf => shelf::view(s),
        Tab::System => monitor::view(s),
        Tab::Calendar => calendar::view(s),
    }
}

/// The Large Island's size on each tab, before any notch inset.
pub fn size(tab: Tab) -> (f32, f32) {
    match tab {
        Tab::Search => (445.0, 160.0),
        Tab::Workflows => (520.0, 250.0),
        Tab::Notes | Tab::Stremflix => (520.0, 290.0),
        Tab::Shelf | Tab::System => (520.0, 230.0),
        Tab::Tasks | Tab::T3 => (420.0, 290.0),
        Tab::Calendar => (420.0, 280.0),
        Tab::Overview | Tab::Assistant | Tab::Clipboard => (420.0, 230.0),
    }
}

/// Loads what a tab shows, as it opens.
pub fn entered(s: &mut Island) {
    match s.tab {
        Tab::T3 => s.t3_load(),
        Tab::Stremflix => s.stremflix_load(),
        Tab::Calendar => s.calendar_offset = 0,
        _ => {}
    }
}

/// Picks up a settings change another window made, for the tab showing.
pub fn reloaded(s: &mut Island, old: &Config) {
    if s.mode == Mode::Large && s.tab == Tab::T3 && old.t3_devices != s.cfg.t3_devices { s.t3_load() }
}

/// Left and Right switch tabs even while typing, as the Electron app's window-wide key
/// handler did. The T3 message box keeps them for its caret.
fn tab_arrows(s: &mut Island, ev: &KeyEvent, tx: &mut Tx) -> bool {
    matches!(ev.key, Key::Left | Key::Right) && s.key(ev, tx)
}

/// The Island's filled button: the text colour as fill, the Island colour as ink.
fn button(s: &Island, key: &str, label: &str, enabled: bool) -> El {
    let (bg, fg) = (s.cfg.text(), s.cfg.bg().alpha(1.0));
    el().key(key).role("button").label(label).focusable(enabled).disabled(!enabled)
        .px(16.0).h(34.0).center().radius(s.cfg.radius(12.0))
        .fill(bg.alpha(if enabled { bg.a } else { 0.4 * bg.a }))
        .hover(fg.alpha(0.1)).press(fg.alpha(0.2)).press_scale(0.97)
        .child(text(label).font(s.cfg.font(14.0, 600.0)).color(fg).a11y_hidden())
}

/// A one-line field in the Island's colours.
fn field(s: &Island, key: &str, edit: &TextEdit, placeholder: &str, access: fn(&mut Island) -> &mut TextEdit, submit: fn(&mut Island, &mut Tx)) -> El {
    let fg = s.cfg.text();
    let input = text_input(edit, s.cfg.font(14.0, 400.0).lh(20.0)).key(key).label(placeholder).single_line().grow(1.0).min_w(0.0)
        .color(fg).placeholder(placeholder, fg.alpha(0.4 * fg.a))
        .on_edit(access).on_submit(submit).on_input_key(tab_arrows);
    row().key(format!("{key}-field")).grow(1.0).min_w(0.0).h(36.0).px(12.0).center_y()
        .radius(s.cfg.radius(12.0))
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
