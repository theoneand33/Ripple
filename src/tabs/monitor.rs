//! System: CPU, memory, storage, network and battery, as gauges.

use rune::{El, col, el, icon, row, text};

use crate::icons;
use crate::island::Island;

pub fn view(s: &Island) -> El {
    let fg = s.cfg.text();
    let muted = fg.alpha(0.55 * fg.a);
    let st = s.stats.unwrap_or_default();
    let ready = s.stats.is_some();
    let pct = |used: u64, total: u64| if total == 0 { 0.0 } else { used as f32 / total as f32 * 100.0 };
    // (key, title, icon, ring percent or None, value, detail, high is good)
    let mut cards: Vec<(&str, &str, &'static rune::Icon, Option<f32>, String, String, bool)> = vec![
        ("cpu", "CPU", &icons::CPU, Some(st.cpu), format!("{:.0}%", st.cpu), "Usage".into(), false),
        ("memory", "Memory", &icons::MEMORY, Some(pct(st.mem_used, st.mem_total)), format!("{:.0}%", pct(st.mem_used, st.mem_total)), of_total(st.mem_used, st.mem_total), false),
        ("storage", "Storage", &icons::HARD_DRIVE, Some(pct(st.disk_total.saturating_sub(st.disk_free), st.disk_total)), format!("{:.0}%", pct(st.disk_total.saturating_sub(st.disk_free), st.disk_total)), format!("{} free", bytes(st.disk_free)), false),
        ("network", "Network", &icons::GLOBE, None, format!("{}/s", bytes(st.down)), format!("↑ {}/s", bytes(st.up)), false),
    ];
    if let Some(p) = s.percent {
        cards.push(("battery", "Battery", &icons::BATTERY, Some(p as f32), format!("{p}%"), if s.charging { "Charging".into() } else { "On battery".into() }, true));
    }
    let mut tiles = Vec::new();
    for (key, title, glyph, ring, value, detail, high_good) in cards {
        let (value, detail) = if ready || key == "battery" { (value, detail) } else { ("—".into(), String::new()) };
        let mut body = row().gap(10.0).center_y();
        if let Some(p) = ring.filter(|_| ready || key == "battery") { body = body.child(gauge(s, p, high_good)) }
        tiles.push(
            col().key(format!("monitor-{key}")).role("group").label(format!("{title}: {value} {detail}")).w(0.0).grow(1.0).min_w(0.0).h(92.0).padding(10.0).gap(8.0)
                .radius(s.cfg.radius(14.0)).fill(fg.alpha(0.06 * fg.a))
                .child(row().gap(6.0).center_y()
                    .child(icon(glyph, 14.0).color(muted).a11y_hidden())
                    .child(text(title).font(s.cfg.font(13.0, 600.0)).color(fg).a11y_hidden()))
                .child(body.child(col().min_w(0.0).grow(1.0).gap(2.0)
                    .child(text(value).font(s.cfg.font(17.0, 600.0).features("'tnum' 1")).color(fg).single_line().a11y_hidden())
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
    root
}

/// A ring of dashes, lit clockwise from the top up to `percent`.
fn gauge(s: &Island, percent: f32, high_good: bool) -> El {
    const SIZE: f32 = 44.0;
    const DASHES: usize = 24;
    let fg = s.cfg.text();
    let p = percent.clamp(0.0, 100.0);
    let load = if high_good { 100.0 - p } else { p };
    let lit = if load < 60.0 { rune::oklch(0.75, 0.17, 150.0) } else if load < 85.0 { rune::oklch(0.80, 0.15, 70.0) } else { rune::oklch(0.66, 0.20, 25.0) };
    let on = (p / 100.0 * DASHES as f32).round() as usize;
    let (c, r) = (SIZE / 2.0, SIZE / 2.0 - 3.0);
    let step = std::f32::consts::TAU / DASHES as f32;
    let mut shape = rune::Shape::new();
    for i in 0..DASHES {
        let a = -std::f32::consts::FRAC_PI_2 + i as f32 * step;
        let arc: Vec<[f32; 2]> = [-0.25, 0.0, 0.25].iter().map(|t| { let b = a + t * step; [c + r * b.cos(), c + r * b.sin()] }).collect();
        shape = shape.stroke(arc, 3.0, if i < on { lit } else { fg.alpha(0.15 * fg.a) });
    }
    el().size(SIZE, SIZE).center().a11y_hidden()
        .child(rune::canvas(shape).size(SIZE, SIZE).absolute(0.0, 0.0))
        .child(text(format!("{p:.0}%")).font(s.cfg.font(10.0, 600.0).features("'tnum' 1")).color(fg))
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
