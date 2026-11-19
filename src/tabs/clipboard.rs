//! Clipboard: the last 50 texts copied anywhere, newest first, each one copyable again.

use rune::{Color, El, ScrollSpec, Transition, col, el, row, text};

use super::empty;
use crate::island::Island;

#[derive(Default)]
pub struct Clipboard {
    pub items: Vec<String>,
    /// The row whose button reads "Copied", and until when.
    pub copied: Option<(usize, f64)>,
}

impl Clipboard {
    /// Adds a copied text on top; one already listed moves up. False when nothing changed.
    pub fn push(&mut self, text: &str) -> bool {
        if text.is_empty() || self.items.first().is_some_and(|t| t == text) { return false }
        self.items.retain(|t| t != text);
        self.items.insert(0, text.to_string());
        self.items.truncate(50);
        self.copied = None;
        true
    }
}

pub fn view(s: &Island) -> El {
    let fg = s.cfg.text();
    let mut list = col().key("clipboard-list").w_full().px(14.0).py(14.0).gap(8.0);
    if s.clipboard.items.is_empty() { list = list.child(empty(s, "Clipboard is empty")) }
    for (i, item) in s.clipboard.items.iter().enumerate() {
        let copied = s.clipboard.copied.is_some_and(|c| c.0 == i);
        let value = item.clone();
        let copy = el().key("copy").role("button").label("Copy").focusable(true)
            .px(8.0).h(24.0).center().radius(s.cfg.radius(6.0))
            .fill(if copied { Color { r: 0.03, g: 0.57, b: 0.1, a: 0.4 } } else { Color::WHITE.alpha(0.15) })
            .hover(fg.alpha(0.1 * fg.a)).press(fg.alpha(0.2 * fg.a))
            .child(text(if copied { "Copied" } else { "Copy" }).key(if copied { "copied" } else { "copy-label" }).font(s.cfg.font(11.0, 500.0)).color(fg).a11y_hidden()
                .transition(Transition::blur_replace()))
            .on_press_tx(move |s: &mut Island, tx| {
                tx.copy(value.clone());
                s.clipboard.copied = Some((i, tx.clock() + 2.0));
            });
        list = list.child(
            row().key(format!("clip-{i}")).w_full().px(12.0).py(8.0).gap(8.0).items_start()
                .radius(s.cfg.radius(12.0))
                .fill(fg.alpha(0.05 * fg.a)).border(1.0, fg.alpha(0.08 * fg.a))
                .child(text(item).font(s.cfg.font(13.0, 400.0).lh(18.0)).color(fg).grow(1.0).min_w(0.0).wrap_anywhere().max_lines(4))
                .child(copy),
        );
    }
    col().key("clipboard").w_full().h_full().scroll(ScrollSpec { stick_bottom: false }).on_wheel(Island::wheel).scroll_edge_fade(12.0).child(list)
}

#[cfg(test)]
mod tests {
    use super::Clipboard;

    #[test]
    fn history_keeps_the_newest_first_without_repeats() {
        let mut c = Clipboard::default();
        assert!(c.push("a") && c.push("b"));
        assert!(!c.push("b") && !c.push(""));
        assert!(c.push("a"));
        assert_eq!(c.items, ["a", "b"]);
        for i in 0..60 { c.push(&i.to_string()); }
        assert_eq!(c.items.len(), 50);
    }
}
