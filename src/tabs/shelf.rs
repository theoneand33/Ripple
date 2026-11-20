//! Shelf: files dropped on the Island, kept until they are dragged out or removed.

use rune::{DragItem, DragOutcome, El, Transition, col, el, icon, row, text};

use super::{Undo, tool, undo_button};
use crate::icons;
use crate::island::Island;

pub fn view(s: &Island) -> El {
    let fg = s.cfg.text();
    let muted = fg.alpha(0.55 * fg.a);
    let mut header = row().key("shelf-header").w_full().h(28.0).gap(8.0).center_y()
        .child(text("Shelf").font(s.cfg.font(17.0, 700.0)).color(fg))
        .child(icon(&icons::INBOX, 16.0).color(muted).a11y_hidden())
        .child(el().grow(1.0));
    if let Some(e) = &s.shelf_sync.error {
        header = header.child(text(e).font(s.cfg.font(12.0, 400.0)).color(muted).shrink().min_w(0.0).truncate());
    }
    if let Some(undo) = undo_button(s) { header = header.child(undo) }
    if !s.cfg.shelf.is_empty() {
        header = header.child(tool(s, "clear-shelf", &icons::TRASH, "Clear shelf").on_press(|s: &mut Island| {
            let mut gone = Vec::new();
            s.save(|c| gone = std::mem::take(&mut c.shelf));
            s.undo = Some(Undo::Shelf(gone));
            s.nextcloud_shelf_sync();
        }));
    }
    let root = col().key("shelf").w_full().h_full().px(16.0).pt(14.0).pb(14.0).gap(10.0).child(header);

    // The drop zone, while a drag is over the Island or nothing is stashed.
    if s.dropping || s.cfg.shelf.is_empty() {
        let line = fg.alpha(if s.dropping { 0.6 } else { 0.3 } * fg.a);
        return root.child(
            col().key("shelf-drop").w_full().grow(1.0).min_h(0.0).gap(4.0).center().radius(s.cfg.radius(16.0))
                .fill(fg.alpha(if s.dropping { 0.08 } else { 0.0 } * fg.a)).border(1.5, line).border_dash(6.0, 5.0)
                .child(icon(&icons::DOWNLOAD, 26.0).color(muted).a11y_hidden())
                .child(text("Drop files here").font(s.cfg.font(15.0, 600.0)).color(fg).mt(4.0))
                .child(text("Stash files here. Drag them out when you need them.").font(s.cfg.font(12.0, 400.0)).color(muted)),
        );
    }

    let mut files = row().key("shelf-files").gap(10.0).items_start();
    for path in &s.cfg.shelf {
        let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
        let (out, moved) = (path.clone(), path.clone());
        let remove = path.clone();
        files = files.child(
            col().key(format!("shelf-{}", path.display())).role("button").label(name.clone()).focusable(true)
                .w(84.0).padding(8.0).gap(6.0).items_center().radius(s.cfg.radius(12.0))
                .hover(fg.alpha(0.08 * fg.a)).press(fg.alpha(0.14 * fg.a))
                .transition(Transition::opacity())
                .drag_out(move |_: &Island| Some(DragItem::new().file(&out)))
                .on_drag_end(move |s: &mut Island, outcome, _| if outcome == DragOutcome::Moved { s.save(|c| c.shelf.retain(|f| *f != moved)); s.nextcloud_shelf_sync() })
                .child(
                    el().size(52.0, 52.0).center().radius(s.cfg.radius(12.0)).fill(fg.alpha(0.08 * fg.a))
                        .child(icon(&icons::FILE, 24.0).color(fg).a11y_hidden())
                        .child(
                            el().key("remove").role("button").label(format!("Remove {name}")).focusable(true)
                                .pin(None, Some(-4.0), Some(-4.0), None).size(18.0, 18.0).center().radius(9.0)
                                .fill(fg.alpha(0.25 * fg.a)).hover(fg.alpha(0.4 * fg.a))
                                .child(icon(&icons::X, 11.0).color(fg).a11y_hidden())
                                .on_press(move |s: &mut Island| { s.save(|c| c.shelf.retain(|f| *f != remove)); s.nextcloud_shelf_sync() }),
                        ),
                )
                .child(text(&name).font(s.cfg.font(11.0, 500.0)).color(fg).w_full().align(rune::Align::Center).max_lines(2).wrap_anywhere().a11y_hidden()),
        );
    }
    root.child(el().key("shelf-scroll").w_full().grow(1.0).min_h(0.0).scroll_x().child(files))
}
