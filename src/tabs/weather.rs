//! Weather: now, and the next five days, from Open-Meteo for the location set in Settings.

use rune::{El, col, icon, row, text};

use super::{card, empty};
use crate::date::format_date;
use crate::icons;
use crate::island::Island;

pub fn view(s: &Island) -> El {
    let fg = s.cfg.text();
    let muted = fg.alpha(0.55 * fg.a);
    let root = col().key("weather").w_full().h_full().px(16.0).pt(14.0).pb(14.0).gap(12.0);
    if s.cfg.location.trim().is_empty() { return root.child(empty(s, "Set a location in Settings → Weather")) }
    let w = &s.weather;
    let now = row().key("weather-now").w_full().gap(10.0).center_y()
        .child(icon(icons::weather(w.code), 28.0).color(fg))
        .child(text(w.temp.map_or("??º".to_string(), |t| format!("{t}º"))).font(s.cfg.font(32.0, 700.0).features("'tnum' 1")).color(fg))
        .child(text(s.cfg.location.trim()).font(s.cfg.font(14.0, 500.0)).color(muted).grow(1.0).min_w(0.0).truncate());
    if w.days.is_empty() { return root.child(now).child(empty(s, "Loading the forecast…")) }
    let mut days = row().key("forecast").role("list").label("Forecast").w_full().gap(8.0);
    for d in &w.days {
        let (y, m, day) = d.date;
        let date = format_date(y, m, day);
        let weekday = crate::date::weekday_name(y, m, day);
        days = days.child(
            col().key(format!("day-{y}-{m}-{day}")).role("listitem").label(format!("{date}: high {}º, low {}º", d.high, d.low))
                .w(0.0).grow(1.0).py(10.0).gap(6.0).items_center().radius(s.cfg.radius(16.0)).fill(card(fg))
                .child(text(weekday).font(s.cfg.font(12.0, 600.0)).color(muted).a11y_hidden())
                .child(icon(icons::weather(d.code), 22.0).color(fg).a11y_hidden())
                .child(row().gap(4.0).a11y_hidden()
                    .child(text(format!("{}º", d.high)).font(s.cfg.font(14.0, 700.0).features("'tnum' 1")).color(fg))
                    .child(text(format!("{}º", d.low)).font(s.cfg.font(13.0, 500.0).features("'tnum' 1")).color(muted))),
        );
    }
    root.child(now).child(days)
}
