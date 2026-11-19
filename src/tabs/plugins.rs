//! Plugins: one panel per Lua file in the plugins folder (src/plugins.rs runs them).

use rune::{El, ScrollSpec, col, el, row, text};

use super::empty;
use crate::island::Island;
use crate::plugins::Node;

pub fn view(s: &Island) -> El {
    let fg = s.cfg.text();
    let plugins = &s.plugins;
    let Some(open) = plugins.list.get(plugins.open) else {
        let dir = plugins.dir.as_ref().map_or("the plugins folder".into(), |d| d.display().to_string());
        return col().key("plugins").w_full().h_full().px(16.0)
            .child(empty(s, "No plugins yet"))
            .child(el().w_full().pt(4.0).center().child(text(format!("Add .lua files to {dir}")).font(s.cfg.font(12.0, 400.0)).color(fg.alpha(0.5 * fg.a)).wrap_anywhere()));
    };
    let mut root = col().key("plugins").w_full().h_full().px(16.0).pt(14.0).pb(14.0).gap(10.0);
    if plugins.list.len() > 1 {
        let mut chips = row().key("plugin-chips").gap(6.0);
        for (i, p) in plugins.list.iter().enumerate() {
            let on = i == plugins.open;
            chips = chips.child(
                el().key(format!("plugin-{i}")).role("tab").label(p.name.clone()).checked(on).focusable(true)
                    .h(26.0).px(10.0).center().radius(s.cfg.radius(13.0))
                    .fill(fg.alpha(if on { 0.18 } else { 0.06 } * fg.a)).hover(fg.alpha(0.12 * fg.a))
                    .child(text(&p.name).font(s.cfg.font(12.0, 600.0)).color(fg).single_line().a11y_hidden())
                    .on_press(move |s: &mut Island| s.plugins.open = i),
            );
        }
        root = root.child(el().key("plugin-chip-strip").w_full().scroll_x().child(chips));
    }
    let mut body = col().key(format!("plugin-body-{}", plugins.open)).w_full().gap(6.0);
    if let Some(e) = &open.error {
        body = body.child(text(e).key("plugin-error").font(s.cfg.font(12.0, 400.0)).color(fg.alpha(0.6 * fg.a)).wrap_anywhere());
    }
    if let Some(v) = &open.view { body = body.child(node(s, v, "n".into())) }
    root.child(col().key("plugin-scroll").w_full().grow(1.0).min_h(0.0).scroll(ScrollSpec { stick_bottom: false }).on_wheel(Island::wheel).child(body))
}

/// A plugin's node as Island elements; `key` is its path, so siblings stay apart.
fn node(s: &Island, n: &Node, key: String) -> El {
    let fg = s.cfg.text();
    let kids = |children: &[Node]| children.iter().enumerate().map(|(i, c)| node(s, c, format!("{key}.{i}"))).collect::<Vec<_>>();
    match n {
        Node::Text { text: t, size, weight, dim } => text(t).key(key).font(s.cfg.font(*size, *weight)).color(if *dim { fg.alpha(0.55 * fg.a) } else { fg }),
        Node::Row { gap, children } => row().children(kids(children)).key(key).gap(*gap).center_y(),
        Node::Col { gap, children } => col().children(kids(children)).key(key).w_full().gap(*gap).items_start(),
        Node::Button { key: k, label, action } => {
            let action = *action;
            super::button(s, k, label, true).on_press(move |s: &mut Island| s.plugins.press(action))
        }
        Node::Bar { value } => row().key(key).role("progressbar").w_full().h(6.0).radius(3.0).fill(fg.alpha(0.12 * fg.a))
            .child(el().w(0.0).grow(*value).h_full().radius(3.0).fill(fg))
            .child(el().w(0.0).grow(1.0 - value)),
    }
}
