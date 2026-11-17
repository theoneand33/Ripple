//! Browser Search: a query or a URL, opened in the default browser.

use rune::{El, Tx, icon, row, text_input};

use super::tab_arrows;
use crate::icons;
use crate::island::{Island, Mode};
use crate::system;

impl Island {
    pub fn search_browser(&mut self, tx: &mut Tx) {
        let query = self.search.text.trim().to_string();
        if query.is_empty() { return }
        let lower = query.to_ascii_lowercase();
        let url = if !query.contains('.') { format!("{}{}", self.cfg.search_engine().1, crate::net::percent_encode(&query)) }
            else if lower.starts_with("http://") || lower.starts_with("https://") { query }
            else { format!("https://{query}") };
        system::open_external(&url);
        self.search.clear();
        tx.blur();
        self.set_mode(Mode::Still);
    }
}

pub fn view(s: &Island) -> El {
    let fg = s.cfg.text();
    let input = text_input(&s.search, s.cfg.font(16.0, 400.0).lh(22.0)).key("browser-searchbar").label("Search").single_line().grow(1.0).min_w(0.0)
        .color(fg).placeholder(format!("Search {} or enter URL", s.cfg.search_engine().0), fg.alpha(0.5 * fg.a))
        .on_edit(|s: &mut Island| &mut s.search)
        .on_submit(Island::search_browser)
        .on_input_key(tab_arrows);
    row().key("search").w_full().h_full().center().child(
        row().key("searchbar").w(360.0).h(46.0).px(16.0).gap(10.0).center_y()
            .radius(s.cfg.radius(16.0))
            .fill(fg.alpha(0.06 * fg.a)).border(1.0, fg.alpha(0.1 * fg.a))
            .child(icon(&icons::SEARCH, 16.0).color(fg.alpha(0.5 * fg.a)).a11y_hidden())
            .child(input),
    )
}
