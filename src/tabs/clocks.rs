//! World Clocks: the time in a few cities, with today's sunrise and sunset there, and the moon's
//! phase. Open-Meteo looks each city up; it is asked again once an hour while the tab opens.

use std::collections::HashMap;

use rune::time::LocalTime;
use rune::{El, TextEdit, Tx, col, icon, row, text};

use super::{Scrolls, accent, button, card, empty, field, tool};
use crate::icons;
use crate::island::Island;
use crate::net::City;

#[derive(Default)]
pub struct Clocks {
    /// Each city's lookup by the name typed; None while it loads or when Open-Meteo didn't know it.
    pub cities: HashMap<String, Option<City>>,
    pub add: TextEdit,
    /// When the cities were last looked up (Unix seconds).
    loaded_at: i64,
}

/// The moon's age as a share of its 29.53-day cycle, from the new moon of 2000-01-06 18:14 UTC.
fn moon_phase(unix: i64) -> f64 { ((unix - 947_182_440) as f64 / 86_400.0 / 29.530_588_853).rem_euclid(1.0) }

/// The phase's name and the share of the disc lit.
fn moon_name(phase: f64) -> (&'static str, f64) {
    const NAMES: [&str; 8] = ["New moon", "Waxing crescent", "First quarter", "Waxing gibbous", "Full moon", "Waning gibbous", "Last quarter", "Waning crescent"];
    (NAMES[((phase * 8.0).round() as usize) % 8], (1.0 - (phase * std::f64::consts::TAU).cos()) / 2.0)
}

impl Island {
    pub fn clocks_load(&mut self) {
        let now = crate::date::now_secs();
        let missing = self.cfg.world_clocks.iter().any(|c| !self.clocks.cities.contains_key(c));
        if !missing && now - self.clocks.loaded_at < 3600 { return }
        let Some(me) = self.me.clone() else { return };
        self.clocks.loaded_at = now;
        for name in self.cfg.world_clocks.clone() {
            self.clocks.cities.entry(name.clone()).or_insert(None);
            me.spawn({ let name = name.clone(); move || crate::net::city(&name) }, move |s, city| { s.clocks.cities.insert(name, city); });
        }
    }

    fn clocks_add(&mut self, _: &mut Tx) {
        let name = self.clocks.add.text.trim().to_string();
        if name.is_empty() { return }
        self.clocks.add.clear();
        if !self.cfg.world_clocks.contains(&name) { self.save(|c| c.world_clocks.push(name)) }
        self.clocks_load();
    }
}

/// "9:41" on the user's clock format.
fn clock(s: &Island, t: &LocalTime) -> String {
    crate::date::clock(t.hour, t.minute, s.cfg.hour12)
}

pub fn view(s: &Island) -> El {
    let fg = s.cfg.text();
    let muted = fg.alpha(0.55 * fg.a);
    let now = crate::date::now_secs();
    let local = LocalTime::at(now * 1000);
    let here = crate::date::days_from_civil(local.year, local.month, local.day);
    let (phase, lit) = moon_name(moon_phase(now));
    let moon = row().key("moon").role("img").label(format!("{phase}, {:.0}% lit", lit * 100.0)).w_full().gap(8.0).center_y()
        .child(icon(&icons::MOON, 16.0).color(accent::indigo()).a11y_hidden())
        .child(text(format!("{phase} · {:.0}% lit", lit * 100.0)).font(s.cfg.font(13.0, 600.0)).color(muted).a11y_hidden());
    let mut list = col().key("clock-list").w_full().gap(6.0);
    if s.cfg.world_clocks.is_empty() { list = list.child(empty(s, "No cities yet")) }
    for (i, name) in s.cfg.world_clocks.iter().enumerate() {
        let city = s.clocks.cities.get(name).cloned().flatten();
        let mut details = col().grow(1.0).min_w(0.0).gap(2.0)
            .child(text(city.as_ref().map_or(name.as_str(), |c| c.name.as_str())).font(s.cfg.font(14.0, 600.0)).color(fg).truncate());
        let mut right = row().gap(10.0).center_y();
        match &city {
            None => details = details.child(text(if s.clocks.cities.contains_key(name) { "Not found or loading…" } else { "" }).font(s.cfg.font(12.0, 500.0)).color(muted)),
            Some(c) => {
                let t = LocalTime::at_offset((now + c.offset) * 1000, 0);
                let day = match crate::date::days_from_civil(t.year, t.month, t.day) - here { 0 => "Today", 1 => "Tomorrow", -1 => "Yesterday", _ => "" };
                let hours = c.offset as f64 / 3600.0 - f64::from(local.offset) / 3600.0;
                let diff = if hours == 0.0 { String::new() } else { format!(", {hours:+}h") };
                details = details.child(text(format!("{day}{diff}")).font(s.cfg.font(12.0, 500.0)).color(muted));
                if let Some((rise, set)) = c.sun {
                    let at = |u: i64| clock(s, &LocalTime::at_offset((u + c.offset) * 1000, 0));
                    for (glyph, when, label, tint) in [(&icons::SUNRISE, rise, "Sunrise", accent::orange()), (&icons::SUNSET, set, "Sunset", accent::pink())] {
                        right = right.child(row().key(label).role("img").label(format!("{label} {}", at(when))).gap(3.0).center_y()
                            .child(icon(glyph, 13.0).color(tint).a11y_hidden())
                            .child(text(at(when)).font(s.cfg.font(12.0, 600.0).features("'tnum' 1")).color(muted).a11y_hidden()));
                    }
                }
                right = right.child(text(clock(s, &t)).key("time").font(s.cfg.font(22.0, 600.0).features("'tnum' 1")).color(fg));
            }
        }
        let name = name.clone();
        list = list.child(
            row().key(format!("city-{i}")).w_full().px(12.0).py(8.0).gap(10.0).center_y().radius(s.cfg.radius(16.0)).fill(card(fg))
                .child(details).child(right)
                .child(tool(s, "remove", &icons::TRASH, &format!("Remove {name}")).on_press(move |s: &mut Island| s.save(|c| c.world_clocks.retain(|n| *n != name)))),
        );
    }
    col().key("clocks").w_full().h_full().px(16.0).pt(14.0).pb(14.0).gap(10.0)
        .child(moon)
        .child(col().key("clock-scroll").w_full().grow(1.0).min_h(0.0).scrolls().child(list))
        .child(row().key("clock-input").w_full().gap(8.0).center_y()
            .child(field(s, "new-city", &s.clocks.add, "Add a city…", |s: &mut Island| &mut s.clocks.add, Island::clocks_add))
            .child(button(s, "add-city", "Add", true).on_press_tx(Island::clocks_add)))
}

#[cfg(test)]
mod tests {
    use super::{moon_name, moon_phase};

    #[test]
    fn the_moon_phase_follows_its_cycle() {
        // 2000-01-06 18:14 UTC was new; half a cycle on, 300 cycles later, it is full.
        assert_eq!(moon_name(moon_phase(947_182_440)).0, "New moon");
        let (name, lit) = moon_name(moon_phase(947_182_440 + (300.5 * 29.530_588_853 * 86_400.0) as i64));
        assert_eq!(name, "Full moon");
        assert!(lit > 0.98, "{lit}");
        assert_eq!(moon_name(0.25).0, "First quarter");
    }
}
