//! Stocks: a portfolio from Sure, and a watchlist of tickers.

use std::collections::HashMap;

use rune::{Color, El, Shape, TextEdit, Tx, canvas, col, el, row, text};

use super::{Scrolls, accent, card, empty, field, tool};
use crate::icons;
use crate::island::Island;
use crate::stocks::{self as api, Portfolio, Quote, Range, format_money, percent, signed_money};

#[derive(Default)]
pub struct Stocks {
    /// The watchlist shows instead of the portfolio; None picks the watchlist while Sure isn't set up.
    pub watchlist: Option<bool>,
    pub range: Range,
    /// The portfolio, or why it couldn't load; None while loading.
    pub portfolio: Option<Result<Portfolio, String>>,
    /// Each ticker's quote over `range`, or why it couldn't load.
    pub quotes: HashMap<String, Result<Quote, String>>,
    pub add: TextEdit,
    /// Bumped on every load and Sure account change, so an earlier answer is dropped.
    pub(super) generation: u64,
    /// Seconds until the portfolio reloads after a Sure setting changed; each change restarts it.
    pub(super) reload_in: Option<u8>,
}

impl Island {
    fn stocks_watchlist(&self) -> bool { self.stocks.watchlist.unwrap_or(self.cfg.sure_key.is_empty()) }

    /// Fetches the portfolio and every quote over the range; the last answers stay up meanwhile.
    pub fn stocks_load(&mut self) {
        let Some(me) = self.me.clone() else { return };
        self.stocks.generation += 1;
        let (generation, range) = (self.stocks.generation, self.stocks.range);
        if !self.cfg.sure_key.is_empty() {
            let (url, key) = (self.cfg.sure_url.clone(), self.cfg.sure_key.clone());
            if matches!(self.stocks.portfolio, Some(Err(_))) { self.stocks.portfolio = None }
            me.spawn(move || api::portfolio(&url, &key, range), move |s, p| if s.stocks.generation == generation { s.stocks.portfolio = Some(p) });
        }
        // One job per ticker, so a slow one holds up no other; an answer for another range or a removed ticker is dropped.
        for t in self.cfg.stocks.clone() {
            me.spawn({ let t = t.clone(); move || api::quote(&t, range) },
                move |s, q| if s.stocks.range == range && s.cfg.stocks.contains(&t) { s.stocks.quotes.insert(t, q); });
        }
    }

    fn stocks_range(&mut self, range: Range) {
        self.stocks.range = range;
        self.stocks_load();
    }

    /// Adds the typed ticker to the watchlist; one that can't be a ticker stays in the field.
    fn stocks_add(&mut self) {
        let Some(t) = api::ticker(&self.stocks.add.text) else { return };
        self.stocks.add.clear();
        if !self.cfg.stocks.contains(&t) { self.save(|c| c.stocks.push(t)) }
        self.stocks.watchlist = Some(true);
        self.stocks_load();
    }

    fn stocks_remove(&mut self, t: &str) {
        self.save(|c| c.stocks.retain(|x| x != t));
        self.stocks.quotes.remove(t);
    }
}

fn tint(v: f64) -> Color { if v < 0.0 { accent::red() } else { accent::green() } }

pub fn view(s: &Island) -> El {
    let fg = s.cfg.text();
    let watch = s.stocks_watchlist();
    let title = |key: &str, label: &str, on: bool, watchlist: bool| {
        text(label).key(key).role("button").label(label).focusable(true).font(s.cfg.font(if on { 20.0 } else { 15.0 }, 700.0))
            .color(if on { fg } else { fg.alpha(0.45 * fg.a) }).single_line()
            .on_press(move |s: &mut Island| s.stocks.watchlist = Some(watchlist))
    };
    let ranges = Range::ALL.into_iter().fold(row().key("stocks-range").padding(2.0).gap(2.0).radius(s.cfg.radius(14.0)).fill(card(fg)), |r, range| {
        let on = s.stocks.range == range;
        r.child(el().key(format!("range-{}", range.label())).role("button").label(range.label()).focusable(true)
            .h(24.0).px(10.0).center().radius(s.cfg.radius(12.0))
            .fill(if on { fg.alpha(0.16 * fg.a) } else { Color::TRANSPARENT }).hover(fg.alpha(0.12 * fg.a))
            .on_press(move |s: &mut Island| s.stocks_range(range))
            .child(text(range.label()).font(s.cfg.font(12.0, 600.0)).color(if on { fg } else { fg.alpha(0.55 * fg.a) }).a11y_hidden()))
    });
    let header = row().key("stocks-header").w_full().h(28.0).gap(14.0).center_y()
        .child(title("stocks-portfolio", "Portfolio", !watch, false))
        .child(title("stocks-watchlist", "Watchlist", watch, true))
        .child(el().grow(1.0))
        .child(ranges)
        .child(tool(s, "stocks-refresh", &icons::ROTATE_CCW, "Refresh").on_press(Island::stocks_load));
    let root = col().key("stocks").w_full().h_full().px(16.0).pt(14.0).pb(14.0).gap(12.0).child(header);
    if watch { return watchlist(s, root) }
    match &s.stocks.portfolio {
        _ if s.cfg.sure_key.is_empty() => root.child(empty(s, "Add a Sure API key in Settings → Stocks")),
        None => root.child(empty(s, "Loading…")),
        Some(Err(e)) => root.child(empty(s, e)),
        Some(Ok(p)) => portfolio(s, root, p),
    }
}

fn portfolio(s: &Island, root: El, p: &Portfolio) -> El {
    let fg = s.cfg.text();
    let muted = fg.alpha(0.55 * fg.a);
    let cur = p.currency.as_str();
    let tnum = |size: f32, weight: f32| s.cfg.font(size, weight).features("'tnum' 1");
    let (today, over) = p.changes();
    // Against the last close: today's deposits aren't part of what moved.
    let yesterday = p.history.len().checked_sub(2).map_or(p.total() - today, |i| p.history[i]);
    let summary = row().key("stocks-summary").w_full().gap(12.0)
        .child(col().grow(1.0).min_w(0.0).gap(4.0)
            .child(text(format_money(p.total(), cur)).font(tnum(34.0, 700.0)).color(fg).single_line())
            .child(row().gap(6.0)
                .child(text(format!("{} ({:+.2}%)", signed_money(today, cur), percent(today, yesterday))).font(tnum(13.0, 500.0)).color(tint(today)))
                .child(text("today").font(s.cfg.font(13.0, 400.0)).color(muted))))
        .child(col().gap(4.0).align(rune::Align::End)
            .child(sparkline(&p.history, tint(over), 220.0, 56.0))
            .child(text(format!("P/L {} · {}", signed_money(over, cur), s.stocks.range.label())).font(tnum(11.0, 400.0)).color(muted)));
    let stat = |key: &str, label: &str, value: String, extra: Option<(String, Color)>| {
        col().key(key).role("group").label(format!("{label}: {value}")).w(0.0).grow(1.0).min_w(0.0).px(12.0).py(8.0).gap(2.0)
            .radius(s.cfg.radius(12.0)).fill(card(fg))
            .child(text(label).font(s.cfg.font(12.0, 500.0)).color(muted).a11y_hidden())
            .child(row().gap(6.0)
                .child(text(&value).font(tnum(15.0, 700.0)).color(extra.as_ref().map_or(fg, |e| e.1)).single_line().a11y_hidden())
                .children(extra.map(|(t, c)| text(t).font(tnum(12.0, 600.0)).color(c).single_line().a11y_hidden())))
    };
    let open = p.open();
    let stats = row().key("stocks-stats").w_full().gap(8.0)
        .child(stat("stat-cash", "Cash", format_money(p.cash(), cur), None))
        .child(stat("stat-invested", "Invested", format_money(p.cost(), cur), None))
        .child(stat("stat-open", "Open", signed_money(open, cur), Some((format!("{:+.2}%", percent(open, p.cost())), tint(open)))));
    let mut list = col().key("stocks-accounts").w_full();
    for (i, a) in p.accounts.iter().enumerate() {
        let detail = format!("{} position{} · Cash {}", a.positions, if a.positions == 1 { "" } else { "s" }, format_money(a.cash, cur));
        list = list.child(row().key(format!("account-{i}")).w_full().py(8.0).gap(12.0).center_y()
            .border_sides([0.0, 0.0, if i + 1 < p.accounts.len() { 1.0 } else { 0.0 }, 0.0], fg.alpha(0.08 * fg.a))
            .child(col().grow(1.0).min_w(0.0).gap(2.0)
                .child(text(&a.name).font(s.cfg.font(14.0, 600.0)).color(fg).truncate())
                .child(text(detail).font(tnum(12.0, 400.0)).color(muted).truncate()))
            .child(col().gap(2.0).align(rune::Align::End)
                .child(text(format_money(a.balance, cur)).font(tnum(14.0, 600.0)).color(fg).single_line())
                .child(text(signed_money(a.open(), cur)).font(tnum(12.0, 500.0)).color(tint(a.open())).single_line()))
            .child(change(s, percent(a.open(), a.cost))));
    }
    root.child(summary).child(stats)
        .child(col().key("stocks-scroll").w_full().grow(1.0).min_h(0.0).scrolls().scroll_edge_fade(10.0).child(list))
}

fn watchlist(s: &Island, root: El) -> El {
    let fg = s.cfg.text();
    let muted = fg.alpha(0.55 * fg.a);
    let tnum = |size: f32, weight: f32| s.cfg.font(size, weight).features("'tnum' 1");
    let mut list = col().key("stocks-watch").w_full();
    for (i, t) in s.cfg.stocks.iter().enumerate() {
        let quote = s.stocks.quotes.get(t);
        let name = match quote { Some(Ok(q)) => q.name.clone(), Some(Err(e)) => e.clone(), None => String::new() };
        let mut line = row().key(format!("stock-{t}")).w_full().py(6.0).gap(12.0).center_y()
            .border_sides([0.0, 0.0, if i + 1 < s.cfg.stocks.len() { 1.0 } else { 0.0 }, 0.0], fg.alpha(0.08 * fg.a))
            .child(col().grow(1.0).min_w(0.0).gap(2.0)
                .child(text(t).font(s.cfg.font(14.0, 700.0)).color(fg).single_line())
                .child(text(name).font(s.cfg.font(12.0, 400.0)).color(muted).truncate()));
        if let Some(Ok(q)) = quote {
            line = line.child(sparkline(&q.points, tint(q.change), 90.0, 26.0))
                .child(col().w(96.0).gap(2.0).align(rune::Align::End)
                    .child(text(format_money(q.price, &q.currency)).font(tnum(14.0, 600.0)).color(fg).single_line())
                    .child(text(signed_money(q.change, &q.currency)).font(tnum(12.0, 500.0)).color(tint(q.change)).single_line()))
                .child(change(s, q.percent()));
        }
        let t = t.clone();
        list = list.child(line.child(tool(s, &format!("remove-{t}"), &icons::X, &format!("Remove {t}")).on_press(move |s: &mut Island| s.stocks_remove(&t))));
    }
    let body = if s.cfg.stocks.is_empty() { empty(s, "Add a ticker, like AAPL, VWCE.DE or BTC-USD") } else {
        col().key("stocks-scroll").w_full().grow(1.0).min_h(0.0).scrolls().scroll_edge_fade(10.0).child(list)
    };
    root.child(body).child(row().key("stocks-add").w_full()
        .child(field(s, "stocks-ticker", &s.stocks.add, "Add ticker…", |s: &mut Island| &mut s.stocks.add, |s: &mut Island, _: &mut Tx| s.stocks_add())))
}

/// A percent in its up or down colour, on a faint pill of it.
fn change(s: &Island, pct: f64) -> El {
    let c = tint(pct);
    let label = format!("{pct:+.2}%");
    el().w(78.0).h(26.0).center().radius(s.cfg.radius(8.0)).fill(c.alpha(0.18))
        .child(text(label).font(s.cfg.font(13.0, 700.0).features("'tnum' 1")).color(c).single_line())
}

/// A line through `points`, lowest at the bottom, over a faint fill of its colour.
fn sparkline(points: &[f64], color: Color, w: f32, h: f32) -> El {
    if points.len() < 2 { return el().size(w, h).a11y_hidden() }
    // At most ~200 points: more than the line has pixels for.
    let step = points.len().div_ceil(200).max(1);
    let mut pts: Vec<f64> = points.iter().step_by(step).copied().collect();
    if (points.len() - 1) % step != 0 { pts.push(points[points.len() - 1]) }
    let (lo, hi) = pts.iter().fold((f64::MAX, f64::MIN), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
    let span = (hi - lo).max(f64::EPSILON);
    let last = (pts.len() - 1) as f32;
    let line: Vec<[f32; 2]> = pts.iter().enumerate().map(|(i, v)| [i as f32 / last * w, h - 2.0 - ((v - lo) / span) as f32 * (h - 4.0)]).collect();
    let mut area = line.clone();
    area.extend([[w, h], [0.0, h]]);
    canvas(Shape::new().view_box(w, h).fill(area, color.alpha(0.12)).stroke(line, 2.0, color)).size(w, h).a11y_hidden()
}
