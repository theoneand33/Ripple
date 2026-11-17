//! Notes: plain-text notes, each titled by its first line, saved as they are typed.

use rune::{El, ScrollSpec, TextEdit, Transition, col, el, row, text, text_input};

use super::{empty, tool};
use crate::config::Config;
use crate::icons;
use crate::island::Island;

pub struct Notes {
    /// The open note's editor.
    pub editor: TextEdit,
    /// Which note the editor edits.
    pub index: usize,
}

impl Notes {
    pub fn new(cfg: &Config) -> Notes {
        Notes { editor: TextEdit::new(cfg.notes.first().map_or("", String::as_str)), index: 0 }
    }
}

impl Island {
    /// Edits note `i`, or the last one when `i` is past the end.
    pub fn open_note(&mut self, i: usize) {
        self.notes.index = i.min(self.cfg.notes.len().saturating_sub(1));
        self.notes.editor = TextEdit::new(self.cfg.notes.get(self.notes.index).map_or("", String::as_str));
    }

    pub fn add_note(&mut self) {
        // An empty note on top is reused rather than stacking blanks.
        if self.cfg.notes.first().is_none_or(|n| !n.trim().is_empty()) { self.save(|c| c.notes.insert(0, String::new())) }
        self.open_note(0);
        self.notes.editor.focused = true;
    }

    pub fn delete_note(&mut self) {
        let i = self.notes.index;
        self.save(|c| { if i < c.notes.len() { c.notes.remove(i); } });
        self.open_note(i);
    }

    pub fn note_edited(&mut self) {
        let (i, text) = (self.notes.index, self.notes.editor.text.clone());
        if self.cfg.notes.get(i).is_some_and(|n| *n != text) { self.save(|c| c.notes[i] = text) }
    }
}

/// A note's title is its first line; the lines after it, run together, preview it.
fn note_heading(note: &str) -> (&str, String) {
    let mut lines = note.lines().map(str::trim).filter(|l| !l.is_empty());
    (lines.next().unwrap_or("New Note"), lines.collect::<Vec<_>>().join(" "))
}

pub fn view(s: &Island) -> El {
    let fg = s.cfg.text();
    let muted = fg.alpha(0.55 * fg.a);
    let accent = crate::config::parse_color("#4c9aff").unwrap_or(fg);
    let mut header = row().key("notes-header").w_full().h(28.0).gap(8.0).center_y()
        .child(text("Notes").font(s.cfg.font(17.0, 700.0)).color(fg))
        .child(text(s.cfg.notes.len().to_string()).font(s.cfg.font(15.0, 500.0).features("'tnum' 1")).color(muted))
        .child(el().grow(1.0));
    if !s.cfg.notes.is_empty() {
        header = header.child(tool(s, "delete-note", &icons::TRASH, "Delete note").on_press(Island::delete_note));
    }
    header = header.child(tool(s, "new-note", &icons::PLUS, "New note").on_press(Island::add_note));
    let root = col().key("notes").w_full().h_full().px(16.0).pt(14.0).pb(14.0).gap(10.0).child(header);
    if s.cfg.notes.is_empty() { return root.child(empty(s, "No notes yet, press + to write one")) }

    let mut list = col().key("note-list").w_full().gap(2.0);
    for (i, note) in s.cfg.notes.iter().enumerate() {
        let on = i == s.notes.index;
        let (title, preview) = note_heading(note);
        list = list.child(
            row().key(format!("note-{i}")).role("button").label(title.to_string()).checked(on).focusable(true)
                .w_full().px(6.0).py(6.0).gap(8.0).items_start().radius(s.cfg.radius(10.0))
                .hover(fg.alpha(0.06 * fg.a)).press(fg.alpha(0.12 * fg.a))
                .on_press(move |s: &mut Island| s.open_note(i))
                .child(el().mt(6.0).size(7.0, 7.0).radius(3.5).fill(if on { accent } else { fg.alpha(0.3 * fg.a) }).a11y_hidden())
                .child(
                    col().grow(1.0).min_w(0.0).gap(2.0).a11y_hidden()
                        .child(text(title).font(s.cfg.font(14.0, 600.0)).color(if on { accent } else { fg }).truncate())
                        .child(text(if preview.is_empty() { "No additional text" } else { &preview }).font(s.cfg.font(12.0, 400.0)).color(muted).truncate()),
                ),
        );
    }
    let editor = text_input(&s.notes.editor, s.cfg.font(14.0, 400.0).lh(20.0)).key("note-editor").label("Note").w_full()
        .color(fg).placeholder("Title, then your note", fg.alpha(0.4 * fg.a))
        .on_edit(|s: &mut Island| &mut s.notes.editor)
        // Return breaks the line.
        .on_submit(|s: &mut Island, _| { s.notes.editor.insert("\n"); s.note_edited() })
        .on_input_change(|s: &mut Island, _| s.note_edited());
    root.child(
        row().key("notes-body").w_full().grow(1.0).min_h(0.0).gap(12.0)
            .child(col().key("note-scroll").w(160.0).h_full().scroll(ScrollSpec { stick_bottom: false }).scroll_edge_fade(8.0).child(list))
            .child(
                col().key(format!("note-card-{}", s.notes.index)).grow(1.0).min_w(0.0).h_full().px(14.0).py(10.0)
                    .radius(s.cfg.radius(18.0)).fill(fg.alpha(0.08 * fg.a)).border(1.0, fg.alpha(0.06 * fg.a))
                    .transition(Transition::opacity())
                    // The editor is only as tall as its text: a press on the rest of the card types into it too.
                    .on_press(|s: &mut Island| s.notes.editor.focused = true)
                    .scroll(ScrollSpec { stick_bottom: false }).scroll_edge_fade(10.0).child(editor),
            ),
    )
}

#[cfg(test)]
mod tests {
    use super::note_heading;
    use crate::config::{Config, Store};
    use crate::island::Island;

    #[test]
    fn a_note_is_titled_by_its_first_line() {
        assert_eq!(note_heading("PR queue\n\nGitHub\n• PR #482"), ("PR queue", "GitHub • PR #482".to_string()));
        assert_eq!(note_heading("  \n"), ("New Note", String::new()));
    }

    #[test]
    fn notes_are_added_edited_and_deleted() {
        let mut s = Island::new(Store::memory(Config { notes: vec!["Old".into()], ..Config::default() }), Vec::new());
        s.add_note();
        assert_eq!((s.notes.index, s.cfg.notes.clone()), (0, vec![String::new(), "Old".into()]));
        s.add_note();
        assert_eq!(s.cfg.notes.len(), 2);
        s.notes.editor.insert("Groceries");
        s.note_edited();
        assert_eq!(s.store.get().notes, ["Groceries", "Old"]);
        s.open_note(5);
        assert_eq!((s.notes.index, s.notes.editor.text.as_str()), (1, "Old"));
        s.delete_note();
        assert_eq!((s.cfg.notes.clone(), s.notes.editor.text.clone()), (vec!["Groceries".to_string()], "Groceries".to_string()));
    }
}
