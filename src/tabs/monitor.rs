//! System: CPU, memory, storage, network and battery, as gauges, and the apps using the most
//! CPU and memory.

use rune::{El, col, el, icon, row, text};

use super::{accent, card, ring};
use crate::icons;
use crate::island::Island;

pub fn view(s: &Island) -> El {
    let fg = s.cfg.text();
    let muted = fg.alpha(0.55 * fg.a);
    let st = s.stats.clone().unwrap_or_default();
    let ready = s.stats.is_some();
    let pct = |used: u64, total: u64| if total == 0 { 0.0 } else { used as f32 / total as f32 * 100.0 };
    // (key, title, icon, accent, ring percent or None, value, detail, high is good)
    let mut cards: Vec<(&str, &str, &'static rune::Icon, rune::Color, Option<f32>, String, String, bool)> = vec![
        ("cpu", "CPU", &icons::CPU, accent::indigo(), Some(st.cpu), format!("{:.0}%", st.cpu), "Usage".into(), false),
        ("memory", "Memory", &icons::MEMORY, accent::teal(), Some(pct(st.mem_used, st.mem_total)), format!("{:.0}%", pct(st.mem_used, st.mem_total)), of_total(st.mem_used, st.mem_total), false),
        ("storage", "Storage", &icons::HARD_DRIVE, accent::orange(), Some(pct(st.disk_total.saturating_sub(st.disk_free), st.disk_total)), format!("{:.0}%", pct(st.disk_total.saturating_sub(st.disk_free), st.disk_total)), format!("{} free", bytes(st.disk_free)), false),
        ("network", "Network", &icons::GLOBE, accent::pink(), None, format!("{}/s", bytes(st.down)), format!("↑ {}/s", bytes(st.up)), false),
    ];
    if let Some(p) = s.percent {
        cards.push(("battery", "Battery", &icons::BATTERY, accent::green(), Some(p as f32), format!("{p}%"), if s.charging { "Charging".into() } else { "On battery".into() }, true));
    }
    let mut tiles = Vec::new();
    for (key, title, glyph, tint, percent, value, detail, high_good) in cards {
        let (value, detail) = if ready || key == "battery" { (value, detail) } else { ("—".into(), String::new()) };
        // Each figure keeps its own colour, as Droppy's widgets do, until it runs high (or a battery low).
        let tint = match percent { Some(p) if (if high_good { 100.0 - p } else { p }) >= 85.0 => accent::red(), _ => tint };
        let mut body = row().gap(10.0).center_y();
        body = body.child(match percent.filter(|_| ready || key == "battery") {
            Some(p) => ring(44.0, p, tint, icon(glyph, 18.0).color(tint)),
            None => el().size(44.0, 44.0).center().radius(s.cfg.radius(22.0)).fill(tint.alpha(0.18)).a11y_hidden().child(icon(glyph, 18.0).color(tint)),
        });
        tiles.push(
            col().key(format!("monitor-{key}")).role("group").label(format!("{title}: {value} {detail}")).w(0.0).grow(1.0).min_w(0.0).h(92.0).padding(10.0).gap(8.0)
                .radius(s.cfg.radius(16.0)).fill(card(fg))
                .child(text(title).font(s.cfg.font(13.0, 700.0)).color(tint).a11y_hidden())
                .child(body.child(col().min_w(0.0).grow(1.0).gap(1.0)
                    .child(text(value).font(s.cfg.font(20.0, 700.0).features("'tnum' 1")).color(fg).single_line().a11y_hidden())
                    .child(text(detail).font(s.cfg.font(11.0, 400.0).features("'tnum' 1")).color(muted).truncate().a11y_hidden()))),
        );
    }
    let mut root = col().key("monitor").w_full().h_full().px(16.0).pt(14.0).pb(14.0).gap(10.0);
    let mut tiles = tiles.into_iter().peekable();
    while tiles.peek().is_some() {
        // Three to a row; a short last row keeps the same tile width.
        let mut line = row().w_full().gap(10.0);
        for i in 0..3 { line = line.child(tiles.next().unwrap_or_else(|| el().key(format!("monitor-gap-{i}")).w(0.0).grow(1.0).padding(10.0))) }
        root = root.child(line);
    }
    let list = |key: &str, title: &str, tint: rune::Color, rows: Vec<(String, String)>| {
        let mut list = col().key(key.to_string()).role("list").label(title.to_string()).w(0.0).grow(1.0).min_w(0.0).padding(10.0).gap(4.0)
            .radius(s.cfg.radius(16.0)).fill(card(fg))
            .child(text(title).font(s.cfg.font(13.0, 700.0)).color(tint));
        if rows.is_empty() { list = list.child(text("—").font(s.cfg.font(12.0, 400.0)).color(muted)) }
        for (i, (name, value)) in rows.into_iter().enumerate() {
            list = list.child(row().key(format!("{key}-{i}")).role("listitem").label(format!("{name}: {value}")).w_full().gap(8.0).center_y()
                .child(text(name).font(s.cfg.font(12.0, 500.0)).color(fg).grow(1.0).min_w(0.0).truncate().a11y_hidden())
                .child(text(value).font(s.cfg.font(12.0, 600.0).features("'tnum' 1")).color(muted).a11y_hidden()));
        }
        list
    };
    root.child(row().key("monitor-top").w_full().gap(10.0)
        .child(list("top-cpu", "Top CPU", accent::indigo(), st.top_cpu.iter().map(|(n, v)| (n.clone(), format!("{v:.1}%"))).collect()))
        .child(list("top-memory", "Top memory", accent::teal(), st.top_mem.iter().map(|(n, v)| (n.clone(), bytes(*v))).collect())))
}

/// A ring lit clockwise from the top up to `percent`: green, orange, then red as load rises.
pub(super) fn gauge(s: &Island, percent: f32, high_good: bool) -> El {
    let fg = s.cfg.text();
    let p = percent.clamp(0.0, 100.0);
    let load = if high_good { 100.0 - p } else { p };
    let lit = if load < 60.0 { accent::green() } else if load < 85.0 { accent::orange() } else { accent::red() };
    ring(44.0, p, lit, text(format!("{p:.0}%")).font(s.cfg.font(10.0, 700.0).features("'tnum' 1")).color(fg))
}

/// "29 / 48 GB", the unit said once when both share it.
fn of_total(used: u64, total: u64) -> String {
    let (u, t) = (bytes(used), bytes(total));
    match (u.split_once(' '), t.split_once(' ')) {
        (Some((n, a)), Some((_, b))) if a == b => format!("{n} / {t}"),
        _ => format!("{u} / {t}"),
    }
}

/// 1024-based, a decimal only under 10: "4.2 GB", "29 GB".
fn bytes(n: u64) -> String {
    let mut v = n as f64;
    for unit in ["B", "KB", "MB", "GB"] {
        if v < 1024.0 { return if unit == "B" || v >= 10.0 { format!("{v:.0} {unit}") } else { format!("{v:.1} {unit}") } }
        v /= 1024.0;
    }
    format!("{v:.1} TB")
}

#[cfg(test)]
mod tests {
    #[test]
    fn sizes_read_short() {
        assert_eq!(super::bytes(512), "512 B");
        assert_eq!(super::bytes(4_500_000_000), "4.2 GB");
        assert_eq!(super::of_total(29 << 30, 48 << 30), "29 / 48 GB");
        assert_eq!(super::of_total(900 << 20, 2 << 30), "900 MB / 2.0 GB");
    }
}
