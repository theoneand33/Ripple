//! Notes: plain-text notes, each titled by its first line, saved as they are typed and synced
//! with Nextcloud Notes when an account is set. Pinned notes list first; search filters the list.
//! The editor previews Markdown live, as Obsidian does: styled, its markup hidden except on the line being edited.

use std::ops::Range;

use rune::{Color, El, Link, Span, TextEdit, Transition, Tx, col, el, icon, row, text, text_input};

use super::{Scrolls, Undo, empty, field, tool, undo_button};
use crate::config::{Config, Item};
use crate::icons;
use crate::island::Island;
use crate::nextcloud::{self, Kind};

pub struct Notes {
    pub editor: TextEdit,
    /// Which note the editor edits.
    pub index: usize,
    /// When the open note was last typed in, until Nextcloud gets the change.
    pub edited: Option<std::time::Instant>,
    pub search: TextEdit,
}

impl Notes {
    pub fn new(cfg: &Config) -> Notes {
        Notes { editor: TextEdit::new(cfg.notes.first().map_or("", |n| n.text.as_str())), index: 0, edited: None, search: TextEdit::default() }
    }
}

impl Island {
    /// Edits note `i`, or the last one when `i` is past the end.
    /// After a sync: keeps the open note (`open`: its key and text before) open where it went,
    /// and shows Nextcloud's text if it changed.
    pub(super) fn notes_synced(&mut self, open: Option<(u64, String)>) {
        let at = open.as_ref().and_then(|(key, _)| self.cfg.notes.iter().position(|n| n.key == *key));
        match at {
            Some(i) if open.as_ref().is_some_and(|(_, text)| self.cfg.notes[i].text == *text) => self.notes.index = i,
            Some(i) => { let focused = self.notes.editor.focused; self.open_note(i); self.notes.editor.focused = focused }
            None => self.open_note(self.notes.index),
        }
    }

    pub fn open_note(&mut self, i: usize) {
        self.notes.index = i.min(self.cfg.notes.len().saturating_sub(1));
        self.notes.editor = TextEdit::new(self.cfg.notes.get(self.notes.index).map_or("", |n| n.text.as_str()));
    }

    fn add_note(&mut self) {
        // An empty note on top is reused rather than stacking blanks.
        if self.cfg.notes.first().is_none_or(|n| !n.text.trim().is_empty()) { self.save(|c| c.notes.insert(0, Item::new(""))) }
        self.open_note(0);
        self.notes.editor.focused = true;
    }

    pub fn toggle_pin(&mut self) {
        let Some(key) = self.cfg.notes.get(self.notes.index).map(|n| n.key) else { return };
        self.save(|c| if c.pinned_notes.contains(&key) { c.pinned_notes.retain(|k| *k != key) } else { c.pinned_notes.push(key) });
    }

    /// The notes the list shows, by index: pinned first, each matching the search.
    fn listed_notes(&self) -> Vec<usize> {
        let query = self.notes.search.text.trim().to_lowercase();
        let mut shown: Vec<usize> = (0..self.cfg.notes.len()).filter(|i| query.is_empty() || self.cfg.notes[*i].text.to_lowercase().contains(&query)).collect();
        shown.sort_by_key(|i| !self.cfg.pinned_notes.contains(&self.cfg.notes[*i].key));
        shown
    }

    fn delete_note(&mut self) {
        let mut gone = None;
        if let Some(key) = self.cfg.notes.get(self.notes.index).map(|n| n.key) { self.save(|c| { gone = nextcloud::remove(c, Kind::Notes, key); c.pinned_notes.retain(|k| *k != key) }) }
        if let Some((i, item)) = gone { self.undo = Some(Undo::Item(Kind::Notes, i, item)) }
        self.open_note(self.notes.index);
        self.nextcloud_sync(Kind::Notes);
    }

    /// A note being typed saves once a second, and uploads once typing pauses.
    pub fn notes_tick(&mut self) {
        let Some(edited) = self.notes.edited else { return };
        self.store.flush();
        if edited.elapsed().as_secs() >= 2 {
            self.notes.edited = None;
            self.nextcloud_sync(Kind::Notes);
        }
    }

    /// Ticks or unticks the `[ ]` checkbox starting at byte `at` of the open note.
    fn toggle_task(&mut self, at: usize) {
        let mark = match self.notes.editor.text.get(at..at + 3) { Some("[ ]") => "x", Some("[x]" | "[X]") => " ", _ => return };
        self.notes.editor.set_selection(at + 1, at + 2);
        self.notes.editor.insert(mark);
        self.note_edited();
    }

    fn note_edited(&mut self) {
        let (i, text) = (self.notes.index, self.notes.editor.text.clone());
        if self.cfg.notes.get(i).is_some_and(|n| n.text != text) {
            // Saved to disk once a second by `notes_tick`, not on every key.
            self.cfg = self.store.update_unsaved(|c| c.notes[i].text = text);
            self.notes.edited = Some(std::time::Instant::now());
        }
    }
}

/// Enter in the search opens the first note it lists.
fn open_first_match(s: &mut Island, _: &mut Tx) {
    if let Some(&i) = s.listed_notes().first() { s.open_note(i) }
}

/// A note's title is its first line; the lines after it, run together, preview it.
fn note_heading(note: &str) -> (&str, String) {
    let mut lines = note.lines().map(str::trim).filter(|l| !l.is_empty());
    (lines.next().unwrap_or("New Note"), lines.collect::<Vec<_>>().join(" "))
}

/// How the editor paints Markdown: `spans` style it and hide its markup, `strikes` cross out ~~text~~,
/// and `tasks` are the `[ ]` checkboxes (their range, and whether ticked) a click toggles.
#[derive(Debug, Default, PartialEq)]
struct Preview { spans: Vec<Span>, strikes: Vec<Range<usize>>, tasks: Vec<(Range<usize>, bool)> }

/// Markdown in `text` styled in place. Its markup (`**`, `# `, `` ` ``, `[`…`](url)`) is hidden,
/// except on the lines `reveal` (the selection, while editing) touches, which show their source.
fn live_markdown(text: &str, reveal: Option<Range<usize>>, accent: Color, muted: Color, code: Color) -> Preview {
    use pulldown_cmark::{Event, HeadingLevel, Options, Parser, TagEnd};
    let mut out = Preview::default();
    let mut hidden: Vec<Range<usize>> = Vec::new();
    // Each open element: its whole range and the range its children cover.
    let mut open: Vec<(Range<usize>, Option<usize>, usize)> = Vec::new();
    // The current table's cells, so the pipes and the `---` row between them can be muted.
    let mut cells: Vec<Range<usize>> = Vec::new();
    let mut tables: Vec<Range<usize>> = Vec::new();
    for (event, r) in Parser::new_ext(text, Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS | Options::ENABLE_TABLES).into_offset_iter() {
        if !matches!(event, Event::End(_)) && let Some((_, start, end)) = open.last_mut() {
            start.get_or_insert(r.start);
            *end = (*end).max(r.end);
        }
        match event {
            Event::Start(_) => open.push((r, None, 0)),
            Event::End(tag) => {
                let Some((outer, start, end)) = open.pop() else { continue };
                let inner = start.unwrap_or(outer.end)..end.max(start.unwrap_or(outer.end));
                // Hide the markup around `inner`: never a line break (a setext heading's underline
                // stays), nor all of an element with no text (`[](url)`), which would vanish.
                let mut wrap = |out: &mut Preview, span: Option<Span>| {
                    if let Some(span) = span { out.spans.push(span) }
                    if inner.is_empty() { return }
                    let tail = inner.end..outer.start + text[outer.clone()].trim_end().len();
                    hidden.push(outer.start..inner.start);
                    if !text[tail.clone()].contains('\n') { hidden.push(tail) }
                };
                match tag {
                    TagEnd::Heading(level) => {
                        let size = match level { HeadingLevel::H1 => 22.0, HeadingLevel::H2 => 19.0, HeadingLevel::H3 => 17.0, _ => 15.0 };
                        wrap(&mut out, Some(Span::new(outer.start, outer.end).size(size).weight(700.0)));
                    }
                    TagEnd::Strong => wrap(&mut out, Some(Span::new(inner.start, inner.end).bold())),
                    TagEnd::Emphasis => wrap(&mut out, Some(Span::new(inner.start, inner.end).italic())),
                    TagEnd::Strikethrough => { out.strikes.push(inner.clone()); wrap(&mut out, None) }
                    TagEnd::Link => wrap(&mut out, Some(Span::new(inner.start, inner.end).color(accent).underline(accent))),
                    TagEnd::BlockQuote(_) => out.spans.push(Span::new(outer.start, outer.end).color(muted).italic()),
                    TagEnd::Item => out.spans.push(Span::new(outer.start, inner.start).color(accent)),
                    TagEnd::TableCell => cells.push(outer),
                    TagEnd::TableHead => out.spans.push(Span::new(outer.start, outer.end).bold()),
                    // Monospaced, so a table whose pipes line up in its source shows as columns.
                    TagEnd::Table => {
                        out.spans.push(Span::new(outer.start, outer.end).mono());
                        tables.push(outer.clone());
                        let mut at = outer.start;
                        for c in cells.drain(..).chain([outer.end..outer.end]) {
                            if c.start > at { out.spans.push(Span::new(at, c.start).color(muted)) }
                            at = c.end;
                        }
                    }
                    _ => {}
                }
            }
            Event::Code(_) => {
                let ticks = text[r.clone()].bytes().take_while(|b| *b == b'`').count();
                out.spans.push(Span::new(r.start + ticks, r.end - ticks).mono().bg(code, 2.0));
                hidden.extend([r.start..r.start + ticks, r.end - ticks..r.end]);
            }
            Event::TaskListMarker(done) => out.tasks.push((r, done)),
            Event::Rule => out.spans.push(Span::new(r.start, r.end).color(muted)),
            _ => {}
        }
    }
    // The lines the selection touches, from the start of its first to the end of its last.
    let shown = reveal.map(|r| {
        let (a, b) = (r.start.min(text.len()), r.end.min(text.len()));
        text[..a].rfind('\n').map_or(0, |i| i + 1)..text[b..].find('\n').map_or(text.len(), |i| b + i)
    });
    let revealed = |h: &Range<usize>| shown.as_ref().is_some_and(|l| h.start <= l.end && h.end >= l.start);
    // A task on a revealed line is plain text: a click there places the caret, not ticks it.
    let (shown_tasks, tasks): (Vec<_>, Vec<_>) = std::mem::take(&mut out.tasks).into_iter().partition(|(r, _)| revealed(r));
    for (r, _) in shown_tasks { out.spans.push(Span::new(r.start, r.end).color(muted)) }
    out.tasks = tasks;
    for (r, done) in &out.tasks {
        // A box: the brackets hidden, the mark between them on a rounded fill, the accent when ticked.
        // The list marker before it too (`-`, `1.`), keeping the indent and any `>` before it.
        let line = text[..r.start].rfind('\n').map_or(0, |i| i + 1);
        let before = text[line..r.start].trim_end();
        let bullet = line + before.rfind(|c: char| c.is_whitespace() || c == '>').map_or(0, |i| i + 1);
        hidden.extend([bullet..r.start, r.start..r.start + 1, r.end - 1..r.end]);
        let mark = Span::new(r.start + 1, r.end - 1).mono().bold();
        out.spans.push(if *done { mark.bg(accent, 2.0).color(Color::WHITE) } else { mark.bg(muted.alpha(0.35 * muted.a), 2.0).color(Color::TRANSPARENT) });
    }
    for h in hidden {
        if h.start >= h.end || revealed(&h) { continue }
        // In a table, markup is muted, not hidden: hiding it would shift the columns after it.
        if tables.iter().any(|t| t.start <= h.start && h.end <= t.end) { out.spans.push(Span::new(h.start, h.end).color(muted)); continue }
        // Hidden: taking next to no room and drawing nothing, its bytes still there to edit and copy.
        out.spans.push(Span::new(h.start, h.end).size(0.01).color(Color::TRANSPARENT));
    }
    out
}

pub fn view(s: &Island) -> El {
    let fg = s.cfg.text();
    let muted = fg.alpha(0.55 * fg.a);
    let accent = super::accent::blue();
    let mut header = row().key("notes-header").w_full().h(28.0).gap(8.0).center_y()
        .child(text("Notes").font(s.cfg.font(17.0, 700.0)).color(fg))
        .child(text(s.cfg.notes.len().to_string()).font(s.cfg.font(15.0, 500.0).features("'tnum' 1")).color(muted))
        .child(el().grow(1.0));
    if let Some(e) = &s.nextcloud[Kind::Notes].error {
        header = header.child(text(e).font(s.cfg.font(12.0, 400.0)).color(muted).shrink().min_w(0.0).truncate());
    }
    if let Some(undo) = undo_button(s) { header = header.child(undo) }
    if let Some(open) = s.cfg.notes.get(s.notes.index) {
        let pinned = s.cfg.pinned_notes.contains(&open.key);
        let pin = tool(s, "pin-note", &icons::STAR, if pinned { "Unpin note" } else { "Pin note" }).checked(pinned).on_press(Island::toggle_pin);
        header = header.child(if pinned { pin.fill(accent.alpha(0.25)) } else { pin });
        header = header.child(tool(s, "delete-note", &icons::TRASH, "Delete note").on_press(Island::delete_note));
    }
    header = header.child(tool(s, "new-note", &icons::PLUS, "New note").on_press(Island::add_note));
    let root = col().key("notes").w_full().h_full().px(16.0).pt(14.0).pb(14.0).gap(10.0).child(header);
    if s.cfg.notes.is_empty() { return root.child(empty(s, "No notes yet, press + to write one")) }

    let mut list = col().key("note-list").w_full().gap(2.0);
    let shown = s.listed_notes();
    if shown.is_empty() { list = list.child(text("No matches").key("no-matches").font(s.cfg.font(12.0, 400.0)).color(muted).px(6.0).py(6.0)) }
    for i in shown {
        let note = &s.cfg.notes[i];
        let on = i == s.notes.index;
        let pinned = s.cfg.pinned_notes.contains(&note.key);
        let (title, preview) = note_heading(&note.text);
        list = list.child(
            row().key(format!("note-{i}")).role("button").label(title.to_string()).checked(on).focusable(true)
                .w_full().px(6.0).py(6.0).gap(8.0).items_start().radius(s.cfg.radius(10.0))
                // Selection shows in the fill too, not only the accent (WCAG 1.4.1).
                .fill(if on { fg.alpha(0.1 * fg.a) } else { Color::TRANSPARENT })
                .hover(fg.alpha(0.06 * fg.a)).press(fg.alpha(0.12 * fg.a))
                .on_press(move |s: &mut Island| s.open_note(i))
                .child(if pinned { icon(&icons::STAR, 11.0).mt(4.0).color(accent).a11y_hidden() }
                    else { el().mt(6.0).mx(2.0).size(7.0, 7.0).radius(3.5).fill(if on { accent } else { fg.alpha(0.3 * fg.a) }).a11y_hidden() })
                .child(
                    col().grow(1.0).min_w(0.0).gap(2.0).a11y_hidden()
                        .child(text(title).font(s.cfg.font(14.0, 600.0)).color(if on { accent } else { fg }).truncate())
                        .child(text(if preview.is_empty() { "No additional text" } else { &preview }).font(s.cfg.font(12.0, 400.0)).color(muted).truncate()),
                ),
        );
    }
    let ed = &s.notes.editor;
    let preview = live_markdown(&ed.text, ed.focused.then(|| ed.anchor.min(ed.focus)..ed.anchor.max(ed.focus)), accent, muted, fg.alpha(0.1 * fg.a));
    let editor = text_input(ed, s.cfg.font(14.0, 400.0).lh(20.0)).key("note-editor").label("Note").w_full()
        .spans(preview.spans).strikes(preview.strikes.into_iter().map(|r| (r.start, r.end, None)))
        .on_link(|s: &mut Island, id: &str, _: &mut Tx| if let Ok(at) = id.parse() { s.toggle_task(at) })
        .color(fg).placeholder("Title, then your note", fg.alpha(0.5 * fg.a))
        .on_edit(|s: &mut Island| &mut s.notes.editor)
        .on_submit(|s: &mut Island, _| { s.notes.editor.insert("\n"); s.note_edited() })
        .on_input_change(|s: &mut Island, _| s.note_edited());
    let editor = preview.tasks.into_iter().fold(editor, |e, (r, done)| e.link(Link::new(r.clone(), r.start.to_string()).role("checkbox").label(if done { "Done" } else { "To do" })));
    root.child(
        row().key("notes-body").w_full().grow(1.0).min_h(0.0).gap(12.0)
            .child(col().key("note-side").w(160.0).h_full().gap(8.0)
                .child(row().key("note-search-row").w_full().child(field(s, "note-search", &s.notes.search, "Search", |s: &mut Island| &mut s.notes.search, open_first_match)))
                .child(col().key("note-scroll").w_full().grow(1.0).min_h(0.0).scrolls().scroll_edge_fade(8.0).child(list)))
            .child(
                col().key(format!("note-card-{}", s.notes.index)).grow(1.0).min_w(0.0).h_full().px(14.0).py(10.0)
                    .radius(s.cfg.radius(18.0)).fill(super::card(fg))
                    .transition(Transition::opacity())
                    // The editor is only as tall as its text: a press on the rest of the card types into it too.
                    .on_press(|s: &mut Island| s.notes.editor.focused = true)
                    .scrolls().scroll_edge_fade(10.0).child(editor),
            ),
    )
}

#[cfg(test)]
mod tests {
    use super::{live_markdown, note_heading};
    use rune::Color;
    use crate::config::{Config, Store};
    use crate::island::Island;

    #[test]
    fn a_note_is_titled_by_its_first_line() {
        assert_eq!(note_heading("PR queue\n\nGitHub\n• PR #482"), ("PR queue", "GitHub • PR #482".to_string()));
        assert_eq!(note_heading("  \n"), ("New Note", String::new()));
    }

    #[test]
    fn markdown_hides_its_markup_except_on_the_line_being_edited() {
        fn hidden(text: &str, reveal: Option<std::ops::Range<usize>>) -> Vec<&str> { live_markdown(text, reveal, Color::WHITE, Color::WHITE, Color::WHITE).spans.iter()
            .filter(|s| s.color == Some(Color::TRANSPARENT)).map(|s| &text[s.start as usize..s.end as usize]).collect() }
        let note = "# Plan\nShip **this** and `that`\n[docs](https://x.y) ~~old~~";
        assert_eq!(hidden(note, None), ["# ", "**", "**", "`", "`", "[", "](https://x.y)", "~~", "~~"]);
        // The caret on line 2 shows that line's source; the others stay rendered.
        assert_eq!(hidden(note, Some(12..12)), ["# ", "[", "](https://x.y)", "~~", "~~"]);
        let p = live_markdown(note, None, Color::WHITE, Color::WHITE, Color::WHITE);
        assert_eq!(p.strikes.iter().map(|r| &note[r.clone()]).collect::<Vec<_>>(), ["old"]);
        assert!(p.spans.iter().any(|s| &note[s.start as usize..s.end as usize] == "this" && s.weight == Some(600.0)));
    }

    #[test]
    fn markdown_edge_cases_keep_their_text() {
        fn hidden(text: &str, reveal: Option<std::ops::Range<usize>>) -> Vec<&str> { live_markdown(text, reveal, Color::WHITE, Color::WHITE, Color::WHITE).spans.iter()
            .filter(|s| s.color == Some(Color::TRANSPARENT)).map(|s| &text[s.start as usize..s.end as usize]).collect() }
        assert!(hidden("Title\n---\nbody", None).is_empty(), "a setext underline and its line break stay");
        assert!(hidden("see [](https://x.y)", None).is_empty(), "an empty link stays visible");
        assert_eq!(hidden("ünï **bö** 🎉 `ç`", None), ["**", "**", "`", "`"]);
        assert_eq!(hidden("> - [x] quoted\n1. [x] numbered", None), ["- ", "[", "]", "1. ", "[", "]"]);
        // The task on the edited line is neither a box nor clickable.
        let p = live_markdown("- [ ] a\n- [ ] b", Some(0..0), Color::WHITE, Color::WHITE, Color::WHITE);
        assert_eq!(p.tasks, [(10..13, false)]);
    }

    #[test]
    fn a_table_mutes_its_pipes_and_bolds_its_header() {
        let note = "| A | **B** |\n|---|:-:|\n| 1 | 2 |\n\nafter";
        let p = live_markdown(note, None, Color::WHITE, Color::BLACK, Color::WHITE);
        let styled = |f: &dyn Fn(&rune::Span) -> bool| { let mut v: Vec<_> = p.spans.iter().filter(|s| f(s)).collect(); v.sort_by_key(|s| s.start); v }.into_iter().map(|s| &note[s.start as usize..s.end as usize]).collect::<Vec<_>>();
        assert_eq!(styled(&|s| s.color == Some(Color::BLACK)).concat().replace(char::is_whitespace, ""), "||****||---|:-:||||");
        assert!(styled(&|s| s.weight == Some(600.0)).iter().any(|t| t.contains("| A |")), "the header row is bold");
        assert!(!styled(&|s| s.color == Some(Color::BLACK)).concat().contains("after"));
        assert!(styled(&|s| s.color == Some(Color::TRANSPARENT)).is_empty(), "nothing in a table is hidden, so its columns stay put");
    }

    #[test]
    fn a_click_on_a_checkbox_ticks_it() {
        let mut s = Island::new(Store::memory(Config { notes: vec![crate::config::Item::new("List\n- [ ] Milk\n- [x] Eggs")], ..Config::default() }), Vec::new());
        let tasks = |s: &Island| live_markdown(&s.notes.editor.text, None, Color::WHITE, Color::WHITE, Color::WHITE).tasks;
        assert_eq!(tasks(&s), [(7..10, false), (18..21, true)]);
        s.toggle_task(7);
        s.toggle_task(18);
        s.toggle_task(0); // not a checkbox
        assert_eq!(s.store.get().notes[0].text, "List\n- [x] Milk\n- [ ] Eggs");
    }

    #[test]
    fn notes_are_added_edited_and_deleted() {
        let texts = |notes: &[crate::config::Item]| notes.iter().map(|n| n.text.clone()).collect::<Vec<_>>();
        let mut s = Island::new(Store::memory(Config { notes: vec![crate::config::Item::new("Old")], ..Config::default() }), Vec::new());
        s.add_note();
        assert_eq!((s.notes.index, texts(&s.cfg.notes)), (0, vec![String::new(), "Old".into()]));
        s.add_note();
        assert_eq!(s.cfg.notes.len(), 2);
        s.notes.editor.insert("Groceries");
        s.note_edited();
        assert_eq!(texts(&s.store.get().notes), ["Groceries", "Old"]);
        s.open_note(5);
        assert_eq!((s.notes.index, s.notes.editor.text.as_str()), (1, "Old"));
        s.delete_note();
        assert_eq!((texts(&s.cfg.notes), s.notes.editor.text.clone()), (vec!["Groceries".to_string()], "Groceries".to_string()));
    }

    #[test]
    fn pinned_notes_list_first_and_search_filters() {
        let notes = ["Alpha", "Beta list", "Gamma list"].map(crate::config::Item::new).into();
        let mut s = Island::new(Store::memory(Config { notes, ..Config::default() }), Vec::new());
        s.open_note(2);
        s.toggle_pin();
        assert_eq!(s.listed_notes(), [2, 0, 1]);
        s.notes.search.replace_text("LIST");
        assert_eq!(s.listed_notes(), [2, 1]);
        s.toggle_pin();
        assert_eq!((s.listed_notes(), s.cfg.pinned_notes.len()), (vec![1, 2], 0));
        s.toggle_pin();
        s.delete_note();
        assert!(s.cfg.pinned_notes.is_empty(), "a deleted note's pin goes with it");
    }
}
