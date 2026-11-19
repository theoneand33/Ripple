//! What the Island shows in Large mode: one module per tab, each with its state, its actions
//! and its view.

mod assistant;
mod calendar;
mod clipboard;
mod limits;
mod monitor;
mod notes;
mod overview;
mod plugins;
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

use crate::config::{Config, Item, Tab};
use crate::nextcloud::Kind;
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
        Tab::Limits => limits::view(s),
        Tab::Calendar => calendar::view(s),
        Tab::Plugins => plugins::view(s),
    }
}

/// The Large Island's size on each tab, before any notch inset.
pub fn size(tab: Tab) -> (f32, f32) {
    match tab {
        Tab::Search => (445.0, 160.0),
        Tab::Workflows => (520.0, 250.0),
        Tab::Notes | Tab::Stremflix => (520.0, 290.0),
        Tab::Shelf | Tab::System => (520.0, 230.0),
        Tab::Limits => (520.0, 290.0),
        Tab::Tasks | Tab::T3 | Tab::Plugins => (420.0, 290.0),
        Tab::Calendar => (560.0, 280.0),
        Tab::Overview | Tab::Assistant | Tab::Clipboard => (420.0, 230.0),
    }
}

/// Loads what a tab shows, as it opens.
pub fn entered(s: &mut Island) {
    match s.tab {
        Tab::T3 => s.t3_load(),
        Tab::Stremflix => s.stremflix_load(),
        Tab::Limits => s.limits_load(),
        Tab::Notes => s.nextcloud_sync(crate::nextcloud::Kind::Notes),
        Tab::Tasks => s.nextcloud_sync(crate::nextcloud::Kind::Tasks),
        Tab::Calendar => {
            (s.calendar_offset, s.calendar_day) = (0, None);
            s.nextcloud_sync(crate::nextcloud::Kind::Events);
        }
        Tab::Plugins => s.plugins.scan(),
        _ => {}
    }
}

/// Picks up a settings change another window made, for the tab showing.
pub fn reloaded(s: &mut Island, old: &Config) {
    if s.mode == Mode::Large && s.tab == Tab::T3 && old.t3_devices != s.cfg.t3_devices { s.t3_load() }
}

/// What a delete took: a note and where it sat, an event, or the cleared Shelf.
pub enum Undo {
    Note(usize, Item),
    Event(Item),
    Shelf(Vec<std::path::PathBuf>),
}

/// An item back from the bin. A delete not yet sent is called off and the item keeps its
/// Nextcloud link. One sent, or maybe in the sync running now, goes through, and the item
/// uploads afresh: a kept link to a deleted copy would make `nextcloud::apply` drop it.
fn restore(deleted: &mut Vec<String>, item: Item, syncing: bool) -> Item {
    let Some(r) = &item.remote else { return item };
    if !syncing && deleted.contains(&r.id) {
        deleted.retain(|id| *id != r.id);
        return item;
    }
    Item { remote: None, ..item }
}

impl Island {
    pub fn undo(&mut self) {
        match self.undo.take() {
            Some(Undo::Note(i, item)) => {
                let (key, syncing) = (item.key, self.nextcloud[Kind::Notes as usize].running);
                self.save(|c| { let item = restore(&mut c.nextcloud.deleted_notes, item, syncing); c.notes.insert(i.min(c.notes.len()), item) });
                self.open_note(self.cfg.notes.iter().position(|n| n.key == key).unwrap_or(0));
                self.nextcloud_sync(Kind::Notes);
            }
            Some(Undo::Event(item)) => {
                let syncing = self.nextcloud[Kind::Events as usize].running;
                self.save(|c| { let item = restore(&mut c.nextcloud.deleted_events, item, syncing); c.events.push(item) });
                self.nextcloud_sync(Kind::Events);
            }
            // Files dropped since stay in front.
            Some(Undo::Shelf(files)) => self.save(|c| { let back: Vec<_> = files.into_iter().filter(|f| !c.shelf.contains(f)).collect(); c.shelf.extend(back) }),
            None => {}
        }
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Remote;

    #[test]
    fn restore_calls_off_an_unsent_delete_and_lets_a_sent_one_go() {
        let linked = || Item { remote: Some(Remote { id: "n1".into(), ..Default::default() }), ..Item::new("a") };
        // Not sent: the delete is dropped and the link kept, so no copy is made.
        let mut deleted = vec!["n0".to_string(), "n1".to_string()];
        assert!(restore(&mut deleted, linked(), false).remote.is_some());
        assert_eq!(deleted, ["n0"]);
        // Maybe in the running sync: the delete stays and the item uploads afresh.
        let mut deleted = vec!["n1".to_string()];
        assert_eq!(restore(&mut deleted, linked(), true).remote, None);
        assert_eq!(deleted, ["n1"]);
        // Already sent: nothing is pending, the item uploads afresh.
        assert_eq!(restore(&mut Vec::new(), linked(), false).remote, None);
    }
}
