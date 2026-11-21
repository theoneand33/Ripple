//! Overview: a dashboard of tiles the user arranges. A tile is a widget made for it (clock,
//! now playing, weather, battery, today) or another tab's panel. The Island grows to fit the
//! tiles. The pencil edits the layout and opens a drawer of widgets to drag onto it.

use rune::{Color, Drag, El, Key, KeyEvent, Phase, Transition, Tx, col, el, icon, row, text};

use super::{Scrolls, accent, card, empty, pill, ring, tool};
use crate::config::{Tab, Tile, Widget};
use crate::icons;
use crate::island::{Island, SPRING, control, cover, estimate_width, marquee, visualizer};
use crate::ics::{event_day, leading_time};
use crate::net::Graph;
use crate::task::Task;
use crate::system::MediaCommand;

const GAP: f32 = 10.0;
/// The Overview's content width: the Island's, less its edges.
const WIDTH: f32 = super::size(Tab::Overview).0 - 2.0 * crate::island::LARGE_EDGE;
/// One grid cell: a quarter of the width, and a row.
const CELL: (f32, f32) = ((WIDTH - 3.0 * GAP) / Tile::COLS as f32, 52.0);
const HEADER: f32 = 28.0;
/// The edit drawer's width: a column beside the grid, so it hides none of it.
const DRAWER_W: f32 = 140.0;
/// One drawer column of chips, inside its padding.
const CHIP_COL: f32 = DRAWER_W - 16.0;
/// A chip and the gap under it.
const CHIP_ROW: f32 = 28.0 + 6.0;
/// The drawer's padding, its hint and the gap under it.
const DRAWER_CHROME: f32 = 16.0 + 16.0 + 6.0;
/// The tallest the Overview's content grows before its tiles scroll: room left for the dock in `CANVAS`.
const MAX_H: f32 = 496.0;

/// `n` cells and the gaps between them.
fn span(n: u8, cell: f32) -> f32 { n as f32 * cell + (n as f32 - 1.0) * GAP }

/// Each tile's rect `[x, y, w, h]`: tiles fill a row left to right in order, and one that
/// doesn't fit starts the next row, under the row's tallest tile.
fn layout(tiles: &[Tile], cols: u8) -> Vec<[f32; 4]> {
    let (mut x, mut y, mut row_h, mut used) = (0.0, 0.0, 0.0_f32, 0);
    tiles.iter().map(|t| {
        if used > 0 && used + t.w > cols { (x, y, row_h, used) = (0.0, y + row_h + GAP, 0.0, 0) }
        let r = [x, y, span(t.w, CELL.0), span(t.h, CELL.1)];
        (x, used, row_h) = (x + r[2] + GAP, used + t.w, row_h.max(r[3]));
        r
    }).collect()
}

/// A tile after its corner was dragged by `(dx, dy)`, snapped to whole cells.
fn resized(t: Tile, dx: f32, dy: f32) -> Tile {
    let cells = |n: u8, cell: f32, d: f32| ((span(n, cell) + d + GAP) / (cell + GAP)).round() as i32;
    t.sized(cells(t.w, CELL.0, dx), cells(t.h, CELL.1, dy))
}

/// Where tile `i`, dragged by `(dx, dy)`, lands: the place of the tile under its centre, the
/// end when it's past them all, or where it was.
fn drop_at(rects: &[[f32; 4]], i: usize, dx: f32, dy: f32) -> usize {
    let [x, y, w, h] = rects[i];
    let (cx, cy) = (x + w / 2.0 + dx, y + h / 2.0 + dy);
    if let Some(j) = rects.iter().position(|r| cx >= r[0] && cx < r[0] + r[2] && cy >= r[1] && cy < r[1] + r[3]) { return j }
    if rects.iter().all(|r| cy >= r[1] + r[3]) { rects.len() - 1 } else { i }
}

/// The tiles to draw: the dashboard as a dragged corner would leave it, with a widget being
/// dragged in from the drawer at its slot (the index returned).
fn shown(s: &Island) -> (Vec<Tile>, Option<usize>) {
    let mut tiles = s.cfg.dashboard.clone();
    if let Some((i, dx, dy, true)) = s.dashboard.drag.filter(|d| d.0 < tiles.len()) { tiles[i] = resized(tiles[i], dx, dy) }
    let new = s.dashboard.add.filter(|_| s.dashboard.edit).and_then(|(w, at)| at.map(|at| { let at = at.min(tiles.len()); tiles.insert(at, Tile::new(w)); at }));
    (tiles, new)
}

/// The grid's height: its tiles', and while editing room for two rows to drop on.
fn grid_height(edit: bool, rects: &[[f32; 4]]) -> f32 {
    let h = rects.iter().map(|r| r[1] + r[3]).fold(0.0, f32::max);
    if edit || rects.is_empty() { h.max(span(2, CELL.1)) } else { h }
}

/// The tallest the content gets: `MAX_H`, less on a short display (12 is `Island::size`'s `LARGE_EDGE`).
fn limit(s: &Island) -> f32 {
    MAX_H.min(s.placement.max_h().max(super::size(Tab::Overview).1) - (s.notch_inset() - 12.0).max(0.0) - 24.0)
}

/// The grid's columns: the fewest from `Tile::COLS` whose layout fits `MAX_H`, so a dashboard
/// that won't fit grows wider before it scrolls; else the ones that leave it shortest. As many
/// as the display holds beside the drawer, so editing doesn't reflow the grid. From the saved
/// tiles, so the Island holds still under a drag.
fn cols(s: &Island) -> u8 {
    let room = s.placement.max_w() - super::size(Tab::Overview).0 - GAP - DRAWER_W;
    let max = Tile::COLS + (room.max(0.0) / (CELL.0 + GAP)) as u8;
    let h = |n| grid_height(s.dashboard.edit, &layout(&s.cfg.dashboard, n));
    (Tile::COLS..=max).find(|&n| HEADER + 10.0 + h(n) <= limit(s))
        .unwrap_or_else(|| (Tile::COLS..=max).min_by(|a, b| h(*a).total_cmp(&h(*b))).unwrap_or(Tile::COLS))
}

/// The grid's width at `cols` columns.
fn grid_w(cols: u8) -> f32 { span(cols, CELL.0) }

/// The Overview's content height: the header and the saved tiles, and while editing the
/// drawer at its widest, so the Island grows taller only once the drawer can't grow wider.
pub fn height(s: &Island) -> f32 {
    let grid = HEADER + 10.0 + grid_height(s.dashboard.edit, &layout(&s.cfg.dashboard, cols(s)));
    let drawer = if s.dashboard.edit { drawer_h(addable(s).len(), max_drawer_cols(s)) } else { 0.0 };
    grid.max(drawer).min(limit(s))
}

/// The Large Island's width on the Overview: wider for extra columns, and while editing for the drawer.
pub fn width(s: &Island) -> f32 {
    super::size(Tab::Overview).0 + grid_w(cols(s)) - WIDTH + if s.dashboard.edit { GAP + drawer_w(drawer_cols(s)) } else { 0.0 }
}

/// The drawer's width at `cols` columns of chips.
fn drawer_w(cols: usize) -> f32 { 16.0 + cols as f32 * (CHIP_COL + 6.0) - 6.0 }

/// The drawer's height with `n` chips in `cols` columns.
fn drawer_h(n: usize, cols: usize) -> f32 { DRAWER_CHROME + n.div_ceil(cols.max(1)) as f32 * CHIP_ROW - 6.0 }

/// The most chip columns the display holds beside the grid.
fn max_drawer_cols(s: &Island) -> usize {
    let room = s.placement.max_w() - (super::size(Tab::Overview).0 + grid_w(cols(s)) - WIDTH + GAP + DRAWER_W);
    1 + (room.max(0.0) / (CHIP_COL + 6.0)) as usize
}

/// The drawer's chip columns: the fewest that fit the Island's height, so the drawer grows
/// wider before it scrolls.
fn drawer_cols(s: &Island) -> usize {
    let (n, max) = (addable(s).len(), max_drawer_cols(s));
    let h = height(s);
    (1..=max).find(|&c| drawer_h(n, c) <= h).unwrap_or(max)
}

/// The widgets the drawer offers: those not on the dashboard.
fn addable(s: &Island) -> Vec<Widget> {
    all().filter(|w| !s.cfg.on_dashboard(*w)).collect()
}

/// The slot a widget dropped at `(x, y)` on the grid takes: after every tile before it in
/// reading order (rows above, and the left halves of the row it's on).
fn slot_at(rects: &[[f32; 4]], x: f32, y: f32) -> usize {
    rects.iter().filter(|r| r[1] + r[3] <= y || r[1] <= y && r[0] + r[2] / 2.0 < x).count()
}

/// The slot under the pointer, or `None` off the grid.
fn slot(s: &Island, d: &Drag) -> Option<usize> {
    let [gx, gy, gw, gh] = s.dashboard.grid.get();
    if gw <= 0.0 { return None }
    // The drawn rect is in window points: zoomed, and scaled with the Island.
    let n = cols(s);
    let k = gw / grid_w(n);
    let [px, py] = d.window_point();
    let (x, y) = ((px - gx) / k, (py - gy) / k);
    if !(0.0..=grid_w(n)).contains(&x) || !(-GAP..=gh / k + GAP).contains(&y) { return None }
    Some(slot_at(&layout(&s.cfg.dashboard, n), x, y))
}

/// Dragging a widget out of the drawer shows where it would go; releasing on the grid adds it there.
fn drag_in(s: &mut Island, widget: Widget, d: &mut Drag) {
    match d.phase {
        Phase::Down => s.dashboard.add = None,
        Phase::Move => s.dashboard.add = Some((widget, slot(s, d))),
        Phase::Up => {
            let at = s.dashboard.add.take().and_then(|(_, at)| at);
            if let Some(at) = at.filter(|_| !d.cancelled()) { add(s, widget, at) }
        }
    }
}

/// Every widget the dashboard could hold: its own, then each tab but this one.
fn all() -> impl Iterator<Item = Widget> {
    Widget::OWN.into_iter().chain(Tab::ALL.into_iter().filter(|t| *t != Tab::Overview).map(Widget::Panel))
}

/// Puts `widget` at `at` (the end past it), unless it's on the dashboard already: so a press
/// that follows a drop changes nothing.
fn add(s: &mut Island, widget: Widget, at: usize) {
    s.save(|c| if !c.on_dashboard(widget) { c.dashboard.insert(at.min(c.dashboard.len()), Tile::new(widget)) });
    super::load_widget(s, widget);
}

fn move_tile(s: &mut Island, from: usize, to: usize) {
    s.save(|c| if from < c.dashboard.len() && to < c.dashboard.len() { let t = c.dashboard.remove(from); c.dashboard.insert(to, t) });
}

fn resize(s: &mut Island, i: usize, dw: i32, dh: i32) {
    s.save(|c| if let Some(t) = c.dashboard.get_mut(i) { *t = t.sized(t.w as i32 + dw, t.h as i32 + dh) });
}

/// Dragging a tile moves it; dragging its corner resizes it. Both land on release.
fn drag(s: &mut Island, i: usize, corner: bool, d: &mut Drag, tx: &mut Tx) {
    match d.phase {
        Phase::Down => { tx.instant(); s.dashboard.drag = None }
        Phase::Move => { tx.instant(); s.dashboard.drag = Some((i, d.dx, d.dy, corner)) }
        Phase::Up => {
            s.dashboard.drag = None;
            if d.cancelled() || i >= s.cfg.dashboard.len() { return }
            if corner {
                let t = resized(s.cfg.dashboard[i], d.dx, d.dy);
                s.save(|c| c.dashboard[i] = t);
            } else {
                let to = drop_at(&layout(&s.cfg.dashboard, cols(s)), i, d.dx, d.dy);
                if to != i { move_tile(s, i, to) }
            }
        }
    }
}

/// The keyboard's way to edit: arrows move a tile, Shift and arrows resize it.
fn edit_key(s: &mut Island, i: usize, ev: &KeyEvent) -> bool {
    let len = s.cfg.dashboard.len();
    match (&ev.key, ev.mods.shift) {
        // At the ends too: an arrow the tile let through would change tab.
        (Key::Left, false) => if i > 0 { move_tile(s, i, i - 1) },
        (Key::Right, false) => if i + 1 < len { move_tile(s, i, i + 1) },
        (Key::Left, true) => resize(s, i, -1, 0),
        (Key::Right, true) => resize(s, i, 1, 0),
        (Key::Up, true) => resize(s, i, 0, -1),
        (Key::Down, true) => resize(s, i, 0, 1),
        _ => return false,
    }
    true
}

pub fn view(s: &Island, layer: &mut Vec<El>) -> El {
    let fg = s.cfg.text();
    let muted = fg.alpha(0.6 * fg.a);

    let mut header = row().key("overview-header").w_full().h(HEADER).gap(8.0).center_y();
    // A Clock tile already shows them.
    header = if s.cfg.on_dashboard(Widget::Clock) { header.child(el().grow(1.0)) } else {
        header.child(text(&s.time).key("time").font(s.cfg.font(15.0, 600.0).features("'tnum' 1")).color(fg))
            .child(text(&s.date).font(s.cfg.font(13.0, 500.0)).color(muted).grow(1.0).min_w(0.0).truncate())
    };
    header = if s.dashboard.edit {
        header.child(pill(s, "edit-done", "Done").on_press(|s: &mut Island| (s.dashboard.edit, s.dashboard.add) = (false, None)))
    } else {
        header.child(tool(s, "edit-dashboard", &icons::PENCIL, "Edit dashboard").on_press(|s: &mut Island| s.dashboard.edit = true))
            .child(tool(s, "open-settings", &icons::SETTINGS, "Open settings").on_press(|s: &mut Island| if let Some(settings) = &s.settings { settings.open() }))
    };

    // A corner being dragged reflows the grid around the size it would land on.
    let (tiles, new) = shown(s);
    let drag = s.dashboard.drag.filter(|d| d.0 < s.cfg.dashboard.len());
    let n = cols(s);
    let rects = layout(&tiles, n);
    let target = drag.filter(|d| !d.3).map(|(i, dx, dy, _)| drop_at(&rects, i, dx, dy));

    let mut grid = el().key("dashboard").w_full().h(grid_height(s.dashboard.edit, &rects)).drawn_rect_into(&s.dashboard.grid);
    for (j, (t, r)) in tiles.iter().zip(&rects).enumerate() {
        if new == Some(j) { grid = grid.child(placeholder(s, *t, *r)); continue }
        // The dashboard's own index, past the widget being dragged in.
        let i = if new.is_some_and(|n| j > n) { j - 1 } else { j };
        let mut tile = tile(s, i, *t, *r, layer);
        match drag {
            Some((j, dx, dy, false)) if j == i => tile = tile.offset(dx, dy).raise().opacity(0.85),
            _ if target == Some(i) => tile = tile.border(2.0, fg.alpha(0.6 * fg.a)),
            _ => {}
        }
        grid = grid.child(tile);
    }
    let content = if tiles.is_empty() && !s.dashboard.edit { empty(s, "No widgets yet: press the pencil to add some") } else { grid };

    let overview = col().key("overview").w(grid_w(n)).h_full().gap(10.0)
        .child(header)
        .child(col().key("dashboard-scroll").w_full().grow(1.0).min_h(0.0).scrolls().child(content));
    if !s.dashboard.edit { return overview }
    row().key("overview-edit").w_full().h_full().gap(GAP).child(overview).child(drawer(s))
}

/// Where a widget dragged from the drawer would land.
fn placeholder(s: &Island, t: Tile, [x, y, w, h]: [f32; 4]) -> El {
    let fg = s.cfg.text();
    let radius = s.cfg.radius(18.0);
    el().key("tile-new").absolute(x, y).size(w, h).center().radius(radius)
        .fill(fg.alpha(0.05 * fg.a)).border(2.0, fg.alpha(0.5 * fg.a))
        .motion(SPRING).transition(Transition::opacity()).a11y_hidden()
        .child(text(t.widget.name()).font(s.cfg.font(13.0, 700.0)).color(fg.alpha(0.7 * fg.a)).truncate())
}

fn tile(s: &Island, i: usize, t: Tile, [x, y, w, h]: [f32; 4], layer: &mut Vec<El>) -> El {
    let (bg, fg) = (s.cfg.bg().alpha(1.0), s.cfg.text());
    let radius = s.cfg.radius(18.0);
    let body = match t.widget {
        Widget::Clock => clock(s, w, h),
        Widget::Media => media(s, w),
        Widget::Weather => weather(s, h),
        Widget::Battery => battery(s, h),
        Widget::Today => today(s),
        Widget::Water => water(s, h),
        Widget::Contributions => contributions(s, w),
        Widget::Quote => quote(s),
        Widget::Day => day(s, w),
        Widget::Panel(tab) => super::panel(s, tab, layer),
    };
    let name = t.widget.name();
    let mut tile = el().key(format!("tile-{:?}", t.widget)).role("group").label(name).absolute(x, y).size(w, h)
        .radius(radius).fill(card(fg)).clip_rounded(radius)
        .motion(SPRING)
        .transition(Transition::opacity())
        .child(body);
    if s.dashboard.edit {
        let grip = el().key("resize").role("button").label(format!("Resize {name}")).pin(None, None, Some(4.0), Some(4.0))
            .size(28.0, 28.0).center().radius(s.cfg.radius(14.0)).hover(fg.alpha(0.18 * fg.a))
            .child(icon(&icons::MAXIMIZE, 14.0).color(fg.alpha(0.7 * fg.a)).a11y_hidden())
            .on_drag(move |s: &mut Island, d: &mut Drag, tx| drag(s, i, true, d, tx));
        // Covers the tile, so a press or drag edits the layout instead of reaching the panel under it.
        tile = tile.child(el().key("tile-edit").role("button").label(format!("{name}: drag to move, arrows move, Shift and arrows resize"))
            .focusable(true).pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).center()
            .fill(bg.alpha(0.85)).on_press(|_: &mut Island| {})
            .on_drag(move |s: &mut Island, d: &mut Drag, tx| drag(s, i, false, d, tx))
            .on_key(move |s: &mut Island, ev: &KeyEvent, _: &mut Tx| edit_key(s, i, ev))
            .child(text(name).font(s.cfg.font(13.0, 700.0)).color(fg).truncate().a11y_hidden())
            .child(tool(s, "remove", &icons::X, &format!("Remove {name}")).pin(Some(4.0), Some(4.0), None, None)
                .on_press(move |s: &mut Island| s.save(|c| if i < c.dashboard.len() { c.dashboard.remove(i); })))
            .child(grip));
    }
    tile
}

/// The edit drawer: the widgets not on the dashboard, to drag onto it or press to add at the end.
fn drawer(s: &Island) -> El {
    let fg = s.cfg.text();
    let dragging = s.dashboard.add.map(|a| a.0);
    let widgets = addable(s);
    let cols = drawer_cols(s);
    // Columns read top to bottom, like the single column they widen from.
    let mut chips = row().key("drawer-chips").w_full().gap(6.0);
    for column in widgets.chunks(widgets.len().div_ceil(cols).max(1)) {
        let mut c = col().w(CHIP_COL).gap(6.0);
        for &widget in column {
            let mut chip = pill(s, &format!("add-{widget:?}"), widget.name())
                .label(format!("Add {}: press, or drag onto the dashboard", widget.name()))
                .on_press(move |s: &mut Island| add(s, widget, usize::MAX))
                .on_drag(move |s: &mut Island, d: &mut Drag, _: &mut Tx| drag_in(s, widget, d));
            if dragging == Some(widget) { chip = chip.opacity(0.4) }
            c = c.child(chip);
        }
        chips = chips.child(c);
    }
    let hint = if widgets.is_empty() { "All added" } else { "Drag to add" };
    col().key("drawer").w(drawer_w(cols)).h_full().padding(8.0).gap(6.0).radius(s.cfg.radius(18.0)).fill(card(fg))
        .transition(Transition::opacity())
        .child(text(hint).font(s.cfg.font(12.0, 700.0)).color(fg.alpha(0.6 * fg.a)))
        .child(col().key("drawer-scroll").w_full().grow(1.0).min_h(0.0).scrolls().child(chips))
}

fn clock(s: &Island, w: f32, h: f32) -> El {
    let fg = s.cfg.text();
    let c = col().key("clock").w_full().h_full().center().gap(6.0)
        .child(text(&s.time).font(s.cfg.font((h * 0.4).min(w * 0.2).clamp(18.0, 64.0), 600.0).features("'tnum' 1")).color(fg).single_line());
    if h < CELL.1 * 2.0 { return c }
    c.child(text(&s.date).font(s.cfg.font(13.0, 500.0)).color(fg.alpha(0.6 * fg.a)).single_line())
}

/// A one-row tile lays its widget out in a line.
fn stack(key: &str, h: f32) -> El {
    let c = if h < CELL.1 * 2.0 { row() } else { col() };
    c.key(key).w_full().h_full().center().gap(6.0)
}

fn weather(s: &Island, h: f32) -> El {
    let fg = s.cfg.text();
    let place = s.cfg.location.trim();
    let mut c = stack("weather", h);
    if place.is_empty() {
        return c.child(icon(&icons::CLOUD_SUN, 28.0).color(fg.alpha(0.5 * fg.a)))
            .child(text("Set a location in Settings").font(s.cfg.font(12.0, 500.0)).color(fg.alpha(0.6 * fg.a)));
    }
    c = c.child(row().gap(8.0).center_y()
        .child(icon(icons::weather(s.weather.code), 28.0).color(fg))
        .child(text(s.weather.temp.map_or("??º".to_string(), |t| format!("{t}º"))).font(s.cfg.font(30.0, 600.0)).color(fg)));
    c.child(text(place).font(s.cfg.font(12.0, 500.0)).color(fg.alpha(0.6 * fg.a)).truncate().max_w(220.0))
}

fn battery(s: &Island, h: f32) -> El {
    let fg = s.cfg.text();
    let c = stack("battery", h);
    let Some(p) = s.percent else {
        return c.child(icon(&icons::BATTERY, 28.0).color(fg.alpha(0.5 * fg.a)))
            .child(text("No battery").font(s.cfg.font(12.0, 500.0)).color(fg.alpha(0.6 * fg.a)));
    };
    let tint = if s.low_battery() { accent::low() } else { accent::green() };
    let glyph = if s.charging { &icons::ZAP } else { &icons::BATTERY };
    c.child(ring(if h < CELL.1 * 2.0 { 32.0 } else { 52.0 }, p as f32, tint, icon(glyph, 18.0).color(tint)))
        .child(text(format!("{p}%{}", if s.charging { " · Charging" } else { "" })).font(s.cfg.font(13.0, 600.0).features("'tnum' 1")).color(fg))
}

fn today(s: &Island) -> El {
    let fg = s.cfg.text();
    let muted = fg.alpha(0.6 * fg.a);
    let events: Vec<&str> = s.cfg.events.iter().filter_map(|e| event_day(&e.text).filter(|(d, _)| *d == s.today).map(|(_, t)| t)).collect();
    let mut c = col().key("today").w_full().h_full().padding(12.0).gap(4.0)
        .child(text("Today").font(s.cfg.font(13.0, 700.0)).color(accent::red()));
    if events.is_empty() { c = c.child(text("No events").font(s.cfg.font(13.0, 500.0)).color(muted)) }
    for (i, e) in events.iter().take(2).enumerate() {
        c = c.child(text(*e).key(format!("event-{i}")).font(s.cfg.font(13.0, 500.0)).color(fg).truncate());
    }
    if events.len() > 2 { c = c.child(text(format!("+{} more", events.len() - 2)).font(s.cfg.font(12.0, 500.0)).color(muted)) }
    let tasks = s.cfg.tasks.len();
    c.child(el().grow(1.0))
        .child(text(match tasks { 0 => "No tasks left".to_string(), 1 => "1 task left".to_string(), n => format!("{n} tasks left") })
            .font(s.cfg.font(12.0, 500.0)).color(muted))
}

/// Adds or takes away a glass of today's water.
fn drink(s: &mut Island, by: i32) {
    let today = crate::date::days_from_civil(s.today.0, s.today.1, s.today.2);
    s.save(|c| { c.water_count = c.water(today).saturating_add_signed(by); c.water_day = today });
}

fn water(s: &Island, h: f32) -> El {
    let fg = s.cfg.text();
    let tint = accent::blue();
    let n = s.cfg.water(crate::date::days_from_civil(s.today.0, s.today.1, s.today.2));
    let goal = s.cfg.water_goal.max(1);
    let gauge = ring(if h < CELL.1 * 2.0 { 32.0 } else { 52.0 }, n as f32 * 100.0 / goal as f32, tint, icon(&icons::DROPLET, 16.0).color(tint));
    stack("water", h)
        .child(row().gap(8.0).center_y()
            .child(tool(s, "water-less", &icons::MINUS, "One glass less").on_press(|s: &mut Island| drink(s, -1)))
            .child(gauge)
            .child(tool(s, "water-more", &icons::PLUS, "One more glass").on_press(|s: &mut Island| drink(s, 1))))
        .child(text(format!("{n} of {goal} glasses")).font(s.cfg.font(13.0, 600.0).features("'tnum' 1")).color(fg))
}

/// The Overview's layout editing.
pub struct Dashboard {
    /// The layout is being edited.
    pub edit: bool,
    /// The tile being dragged: its index, the pointer's travel, and whether its corner (a resize).
    pub drag: Option<(usize, f32, f32, bool)>,
    /// A widget being dragged out of the edit drawer, and the slot on the grid it would take.
    pub add: Option<(Widget, Option<usize>)>,
    /// Where the grid draws, in window points, to place a widget dropped on it.
    pub grid: rune::Live<[f32; 4]>,
}

impl Default for Dashboard {
    fn default() -> Dashboard { Dashboard { edit: false, drag: None, add: None, grid: rune::Live::new([0.0; 4]) } }
}

/// The GitHub and LeetCode graphs: for whom, and the graph or why it failed; None while loading.
#[derive(Default)]
pub struct Contributions {
    pub github: Option<(String, Result<Graph, String>)>,
    pub leetcode: Option<(String, Result<Graph, String>)>,
    /// When they were last asked for (Unix seconds).
    loaded_at: i64,
    /// Seconds left before a changed user name loads, so typing in Settings doesn't fetch each keystroke.
    pub(super) reload_in: Option<u8>,
}

impl Island {
    /// Asks again once an hour, or at once for a changed user name.
    pub fn contributions_load(&mut self) {
        let now = crate::date::now_secs();
        let c = &self.contributions;
        let same = |had: &Option<(String, Result<Graph, String>)>, user: &str| user.trim().is_empty() || had.as_ref().is_some_and(|h| h.0 == user.trim());
        if same(&c.github, &self.cfg.github_user) && same(&c.leetcode, &self.cfg.leetcode_user) && now - c.loaded_at < 3600 { return }
        let Some(me) = self.me.clone() else { return };
        self.contributions.loaded_at = now;
        let (gh, lc) = (self.cfg.github_user.trim().to_string(), self.cfg.leetcode_user.trim().to_string());
        // A late answer for a name since changed would replace the newer one's graph.
        if !gh.is_empty() { me.spawn({ let u = gh.clone(); move || crate::net::github_graph(&u) }, move |s, g| if s.cfg.github_user.trim() == gh { s.contributions.github = Some((gh, g)) }) }
        if !lc.is_empty() { me.spawn({ let u = lc.clone(); move || crate::net::leetcode_graph(&u) }, move |s, g| if s.cfg.leetcode_user.trim() == lc { s.contributions.leetcode = Some((lc, g)) }) }
    }
}

/// One graph: its name and streak, then a column per week up to this one, Sunday on top.
fn graph(s: &Island, key: &str, name: &str, user: &str, data: Option<&Result<Graph, String>>, weeks: i64) -> El {
    let fg = s.cfg.text();
    let muted = fg.alpha(0.6 * fg.a);
    let today = crate::date::days_from_civil(s.today.0, s.today.1, s.today.2);
    let head = row().w_full().gap(6.0).center_y().child(text(format!("{name} · {user}")).font(s.cfg.font(13.0, 700.0)).color(fg).grow(1.0).min_w(0.0).truncate());
    let days = match data {
        None => return col().key(key.to_string()).w_full().gap(6.0).child(head).child(text("Loading…").font(s.cfg.font(12.0, 500.0)).color(muted)),
        Some(Err(e)) => return col().key(key.to_string()).w_full().gap(6.0).child(head).child(text(e).font(s.cfg.font(12.0, 500.0)).color(muted).truncate()),
        Some(Ok(days)) => days,
    };
    let active: Vec<i64> = days.iter().map(|d| d.0).collect();
    let streak = super::habits::streak(&active, today);
    let flame = if streak > 0 { accent::orange() } else { fg.alpha(0.35 * fg.a) };
    let head = head.child(row().key("streak").role("img").label(format!("{streak} day streak")).gap(3.0).center_y()
        .child(icon(&icons::FLAME, 13.0).color(flame).a11y_hidden())
        .child(text(format!("{streak}")).font(s.cfg.font(12.0, 700.0).features("'tnum' 1")).color(flame).a11y_hidden()));
    // 1970-01-01 was a Thursday: day 0 sits in row 4.
    let sunday = today - (today + 4).rem_euclid(7);
    let mut grid = row().key("cells").gap(2.0).a11y_hidden();
    for week in (0..weeks).rev() {
        let mut column = col().gap(2.0);
        for wd in 0..7 {
            let day = sunday - week * 7 + wd;
            let level = days.iter().find(|d| d.0 == day).map_or(0, |d| d.1);
            let fill = if day > today { Color::TRANSPARENT } else if level == 0 { fg.alpha(0.08 * fg.a) } else { accent::green().alpha([0.3, 0.5, 0.75, 1.0][usize::from(level - 1)]) };
            column = column.child(el().size(8.0, 8.0).radius(2.0).fill(fill));
        }
        grid = grid.child(column);
    }
    col().key(key.to_string()).role("group").label(format!("{name} contributions of {user}: {} active days, {streak} day streak", active.len())).w_full().gap(6.0).child(head).child(grid)
}

fn contributions(s: &Island, w: f32) -> El {
    let fg = s.cfg.text();
    let c = &s.contributions;
    // 8pt cells 2pt apart, inside the tile's padding.
    let weeks = (((w - 24.0 + 2.0) / 10.0).floor() as i64).clamp(1, 53);
    let mut body = col().key("contributions").w_full().h_full().padding(12.0).gap(12.0);
    let (gh, lc) = (s.cfg.github_user.trim(), s.cfg.leetcode_user.trim());
    if gh.is_empty() && lc.is_empty() {
        return body.center().child(text("Set a GitHub or LeetCode user in Settings → Overview").font(s.cfg.font(12.0, 500.0)).color(fg.alpha(0.6 * fg.a)));
    }
    if !gh.is_empty() { body = body.child(graph(s, "github", "GitHub", gh, c.github.as_ref().filter(|g| g.0 == gh).map(|g| &g.1), weeks)) }
    if !lc.is_empty() { body = body.child(graph(s, "leetcode", "LeetCode", lc, c.leetcode.as_ref().filter(|g| g.0 == lc).map(|g| &g.1), weeks)) }
    body
}

/// (quote, author); all from authors long in the public domain.
const QUOTES: [(&str, &str); 30] = [
    ("The secret of getting ahead is getting started.", "Mark Twain"),
    ("Well begun is half done.", "Aristotle"),
    ("It does not matter how slowly you go as long as you do not stop.", "Confucius"),
    ("What we think, we become.", "The Buddha"),
    ("Simplicity is the ultimate sophistication.", "Leonardo da Vinci"),
    ("The best way out is always through.", "Robert Frost"),
    ("Energy and persistence conquer all things.", "Benjamin Franklin"),
    ("Do what you can, with what you have, where you are.", "Theodore Roosevelt"),
    ("Nothing will work unless you do.", "Maya Angelou"),
    ("He who has a why to live can bear almost any how.", "Friedrich Nietzsche"),
    ("The only way to have a friend is to be one.", "Ralph Waldo Emerson"),
    ("Knowing is not enough; we must apply.", "Johann Wolfgang von Goethe"),
    ("Waste no more time arguing what a good man should be. Be one.", "Marcus Aurelius"),
    ("We suffer more often in imagination than in reality.", "Seneca"),
    ("No man ever steps in the same river twice.", "Heraclitus"),
    ("Little by little, one travels far.", "J. R. R. Tolkien"),
    ("Quality is not an act, it is a habit.", "Aristotle"),
    ("Go confidently in the direction of your dreams.", "Henry David Thoreau"),
    ("A journey of a thousand miles begins with a single step.", "Lao Tzu"),
    ("Imagination is more important than knowledge.", "Albert Einstein"),
    ("Act as if what you do makes a difference. It does.", "William James"),
    ("The unexamined life is not worth living.", "Socrates"),
    ("Whatever you are, be a good one.", "Abraham Lincoln"),
    ("Not all those who wander are lost.", "J. R. R. Tolkien"),
    ("Be kind, for everyone you meet is fighting a hard battle.", "Ian Maclaren"),
    ("Hope is the thing with feathers that perches in the soul.", "Emily Dickinson"),
    ("To be yourself in a world trying to make you something else is the greatest accomplishment.", "Ralph Waldo Emerson"),
    ("The future depends on what you do today.", "Mahatma Gandhi"),
    ("Luck is what happens when preparation meets opportunity.", "Seneca"),
    ("I think, therefore I am.", "René Descartes"),
];

/// The day's quote: the same all day, the next one tomorrow.
fn quote_of(day: i64) -> (&'static str, &'static str) { QUOTES[day.rem_euclid(QUOTES.len() as i64) as usize] }

fn quote(s: &Island) -> El {
    let fg = s.cfg.text();
    let (words, author) = quote_of(crate::date::days_from_civil(s.today.0, s.today.1, s.today.2));
    col().key("quote").role("group").label(format!("Quote of the day: {words} {author}")).w_full().h_full().padding(14.0).gap(6.0).justify_center()
        .child(text(format!("“{words}”")).font(s.cfg.font(15.0, 600.0).lh(20.0)).color(fg).wrap().max_lines(3).a11y_hidden())
        .child(text(format!("— {author}")).font(s.cfg.font(12.0, 500.0)).color(fg.alpha(0.6 * fg.a)).truncate().a11y_hidden())
}

/// Today's timed events and the tasks due today, as (minutes since midnight, title, a task?),
/// in time order. A task due without a time counts at 9:00, as its reminder does.
fn timeline(s: &Island) -> Vec<(u32, String, bool)> {
    let events = s.cfg.events.iter().filter_map(|e| {
        let (_, title) = event_day(&e.text).filter(|(d, _)| *d == s.today)?;
        let (Some((h, m)), rest) = leading_time(title) else { return None };
        Some((h * 60 + m, rest.to_string(), false))
    });
    let tasks = s.cfg.tasks.iter().filter_map(|t| {
        let task = Task::parse(&t.text, s.today);
        let (_, (h, m)) = task.due_at().filter(|(d, _)| *d == s.today)?;
        Some((h * 60 + m, task.title, true))
    });
    let mut items: Vec<_> = events.chain(tasks).collect();
    items.sort_by_key(|i| i.0);
    items
}

/// One line for the whole day: the share gone, a mark for each event and task, and what is next.
fn day(s: &Island, w: f32) -> El {
    let fg = s.cfg.text();
    let muted = fg.alpha(0.6 * fg.a);
    let items = timeline(s);
    let gone = s.minutes as f32 / 1440.0;
    let bar_w = (w - 24.0).max(0.0);
    let tint = |task: bool| if task { accent::teal() } else { accent::indigo() };
    let mut bar = el().key("day-bar").role("progressbar").label(format!("{:.0}% of the day gone", gone * 100.0)).w(bar_w).h(10.0)
        .radius(5.0).fill(fg.alpha(0.12 * fg.a))
        .child(el().h_full().w_frac(gone).radius(5.0).fill(accent::orange().alpha(0.7)).a11y_hidden());
    for (i, (at, _, task)) in items.iter().enumerate() {
        bar = bar.child(el().key(format!("mark-{i}")).a11y_hidden().absolute(*at as f32 / 1440.0 * bar_w - 3.0, 2.0).size(6.0, 6.0).radius(3.0).fill(tint(*task)));
    }
    let hours = row().key("day-hours").w(bar_w).justify_between().a11y_hidden()
        .children([0, 6, 12, 18, 24].map(|h| text(format!("{h}")).font(s.cfg.font(10.0, 500.0).features("'tnum' 1")).color(muted)));
    let next: Vec<_> = items.iter().filter(|i| i.0 >= s.minutes).take(2).collect();
    let mut c = col().key("day").w_full().h_full().padding(12.0).gap(6.0)
        .child(row().w_full().justify_between()
            .child(text("Day Progress").font(s.cfg.font(13.0, 700.0)).color(accent::orange()))
            .child(text(format!("{:.0}%", gone * 100.0)).font(s.cfg.font(13.0, 700.0).features("'tnum' 1")).color(fg)))
        .child(bar).child(hours);
    if next.is_empty() { c = c.child(text("Nothing left today").font(s.cfg.font(12.0, 500.0)).color(muted)) }
    for (i, (at, title, task)) in next.into_iter().enumerate() {
        c = c.child(row().key(format!("next-{i}")).w_full().gap(6.0).center_y()
            .child(el().size(6.0, 6.0).radius(3.0).fill(tint(*task)).a11y_hidden())
            .child(text(format!("{:02}:{:02}", at / 60, at % 60)).font(s.cfg.font(12.0, 600.0).features("'tnum' 1")).color(muted))
            .child(text(title).font(s.cfg.font(12.0, 500.0)).color(fg).grow(1.0).min_w(0.0).truncate()));
    }
    c
}

/// Droppy's player: cover and title on top, the progress bar with both times, then the controls.
fn media(s: &Island, w: f32) -> El {
    let fg = s.cfg.text();
    let muted = fg.alpha(0.6 * fg.a);
    let Some(track) = &s.track else {
        return col().key("media").w_full().h_full().center().gap(6.0)
            .child(icon(&icons::MUSIC, 28.0).color(fg.alpha(0.5 * fg.a)))
            .child(text("Nothing playing").font(s.cfg.font(12.0, 500.0)).color(muted));
    };
    let name = if track.name.is_empty() { "Unknown Title" } else { &track.name };
    let artist = if track.artist.is_empty() { "Unknown Artist" } else { &track.artist };
    let button = |key: &'static str, glyph: &'static rune::Icon, size: f32, command: MediaCommand, label: &str| {
        el().key(key).role("button").label(label).focusable(true)
            .padding(4.0).radius(s.cfg.radius(8.0)).hover(fg.alpha(0.1 * fg.a)).press(fg.alpha(0.2 * fg.a)).press_scale(0.97)
            .child(icon(glyph, size).color(fg))
            .on_press(move |s: &mut Island| control(s, command))
    };
    // The tile less its padding, the cover, the visualizer and the gaps between them.
    let title_w = (w - 24.0 - 56.0 - 25.0 - 24.0).max(0.0);
    let line = |key: &str, text: El, text_width: f32, height: f32| marquee(s, key, text, text_width, title_w, 10.0).w(title_w).h(height);
    let head = row().key("track-head").w_full().gap(12.0).center_y()
        .child(cover(s, 56.0, 12.0, 24.0, 1.06))
        .child(col().key("details").grow(1.0).min_w(0.0)
            .child(line("track-name", text(name).font(s.cfg.font(17.0, 700.0)).color(fg).single_line(), estimate_width(name, 17.0), 22.0))
            .child(line("track-artist", text(artist).font(s.cfg.font(14.0, 500.0)).color(muted).single_line(), estimate_width(artist, 14.0), 18.0)))
        .child(visualizer(track.playing, accent::pink()));
    let mut body = col().key(format!("track-{}-{}", track.name, track.artist)).w_full().h_full().padding(12.0).gap(10.0)
        .opacity(if track.playing { 1.0 } else { 0.5 })
        .transition(Transition::blur_replace())
        .child(head);
    if let Some((position, length)) = track.progress {
        let clock = |t: u32| format!("{}:{:02}", t / 60, t % 60);
        let time = |t: String| text(t).font(s.cfg.font(12.0, 600.0).features("'tnum' 1")).color(muted);
        body = body.child(row().key("track-progress").role("progressbar").label(format!("{} of {}", clock(position), clock(length))).w_full().gap(10.0).center_y()
            .child(time(clock(position)).a11y_hidden())
            .child(el().grow(1.0).h(6.0).radius(3.0).fill(fg.alpha(0.15 * fg.a)).a11y_hidden()
                .child(el().h_full().w_frac(position.min(length) as f32 / length.max(1) as f32).radius(3.0).fill(fg.alpha(0.55 * fg.a))))
            .child(time(format!("−{}", clock(length.saturating_sub(position)))).a11y_hidden()));
        // shortcut: whole-second position from the 2 s poll, so a line can show up to 2 s late; interpolate if that shows.
        if let Some(line) = s.lyrics.as_deref().filter(|_| s.cfg.lyrics).and_then(|l| crate::net::lyric_at(l, position * 1000)).filter(|l| !l.is_empty()) {
            body = body.child(text(line).key(format!("lyric-{line}")).font(s.cfg.font(14.0, 600.0)).color(fg).w_full().truncate()
                .transition(Transition::blur_replace()));
        }
    }
    body.child(el().grow(1.0)).child(row().key("controls").w_full().gap(36.0).center()
        .child(button("previous", &icons::SKIP_BACK, 22.0, MediaCommand::Previous, "Previous"))
        .child(button("play-pause", if track.playing { &icons::PAUSE } else { &icons::PLAY }, 28.0, MediaCommand::PlayPause, if track.playing { "Pause" } else { "Play" }))
        .child(button("next", &icons::SKIP_FORWARD, 22.0, MediaCommand::Next, "Next")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile(w: u8, h: u8) -> Tile { Tile { widget: Widget::Clock, w, h } }

    #[test]
    fn the_quote_changes_once_a_day_and_wraps_around() {
        assert_ne!(quote_of(10), quote_of(11));
        assert_eq!(quote_of(0), quote_of(QUOTES.len() as i64));
        assert_eq!(quote_of(-1), QUOTES[QUOTES.len() - 1]);
    }

    #[test]
    fn the_day_lines_up_timed_events_and_tasks_due_today() {
        use crate::config::{Config, Item, Store};
        let mut cfg = Config::default();
        cfg.events = ["2026-01-01 13:00 Lunch", "2026-01-01 All day", "2026-01-02 09:00 Tomorrow"].map(Item::new).into();
        cfg.tasks = ["Ship due:2026-01-01T17:30", "Plan due:2026-01-01", "Later due:2026-01-03"].map(Item::new).into();
        let mut s = Island::new(Store::memory(cfg), Vec::new());
        s.today = (2026, 1, 1);
        assert_eq!(timeline(&s), [(540, "Plan".into(), true), (780, "Lunch".into(), false), (1050, "Ship".into(), true)]);
    }

    #[test]
    fn tiles_fill_rows_in_order() {
        let r = layout(&[tile(2, 2), tile(1, 1), tile(2, 1), tile(4, 3)], Tile::COLS);
        assert_eq!((r[0][0], r[0][1]), (0.0, 0.0));
        // Four columns: a two-wide tile ends halfway, less half a gap.
        assert_eq!((r[1][0], r[1][1]), ((WIDTH + GAP) / 2.0, 0.0));
        // Three columns used: the two-wide tile starts a row under the tallest one.
        assert_eq!((r[2][0], r[2][1]), (0.0, 124.0));
        assert_eq!(r[3][1], 186.0);
        assert_eq!(r[3][2], WIDTH);
    }

    #[test]
    fn a_dragged_corner_snaps_to_cells_within_bounds() {
        let t = tile(2, 2);
        assert_eq!(resized(t, CELL.0 + GAP, 0.0), tile(3, 2));
        assert_eq!(resized(t, 10.0, -20.0), t);
        assert_eq!(resized(t, -1000.0, 1000.0), tile(1, Tile::ROWS));
    }

    #[test]
    fn a_dragged_tile_lands_on_the_tile_under_its_centre() {
        let r = layout(&[tile(2, 2), tile(2, 2), tile(4, 2)], Tile::COLS);
        assert_eq!(drop_at(&r, 0, r[1][0], 0.0), 1);
        assert_eq!(drop_at(&r, 0, 0.0, r[2][1]), 2);
        assert_eq!(drop_at(&r, 2, 0.0, 1000.0), 2);
        assert_eq!(drop_at(&r, 1, 2.0, 2.0), 1);
    }

    #[test]
    fn a_widget_dragged_in_takes_the_slot_under_the_pointer() {
        let r = layout(&[tile(2, 2), tile(2, 2), tile(4, 2)], Tile::COLS);
        assert_eq!(slot_at(&r, 0.0, 0.0), 0);
        // Right of the first tile's middle: after it.
        assert_eq!(slot_at(&r, r[0][2] * 0.75, 10.0), 1);
        assert_eq!(slot_at(&r, 0.0, r[2][1] + 1.0), 2);
        assert_eq!(slot_at(&r, 0.0, 1000.0), 3);
        assert_eq!(slot_at(&[], 50.0, 50.0), 0);
    }

    #[test]
    fn a_dashboard_too_tall_to_fit_grows_columns() {
        use crate::config::{Config, Store};
        let mut s = Island::new(Store::memory(Config::default()), Vec::new());
        s.cfg.dashboard = vec![tile(2, 2), tile(2, 2)];
        assert_eq!(cols(&s), Tile::COLS);
        // Three rows of 3×3 don't fit; two side by side need 6 columns.
        s.cfg.dashboard = vec![tile(3, 3); 3];
        assert_eq!(cols(&s), 6);
        assert!(height(&s) < MAX_H);
        assert_eq!(width(&s), super::super::size(Tab::Overview).0 + grid_w(6) - WIDTH);
        s.dashboard.edit = true;
        assert_eq!(cols(&s), 6, "editing keeps the columns");
        assert!(width(&s) <= s.placement.max_w());
        // A narrow display keeps it to what fits.
        s.placement = crate::island::Placement::of(&s.cfg, (960.0, 540.0));
        assert!(cols(&s) < 6 && width(&s) <= s.placement.max_w());
    }

    #[test]
    fn the_drawer_widens_before_the_island_grows_taller() {
        use crate::config::{Config, Store};
        let mut s = Island::new(Store::memory(Config::default()), Vec::new());
        s.dashboard.edit = true;
        s.cfg.dashboard = vec![tile(2, 2)];
        let n = addable(&s).len();
        assert!(drawer_cols(&s) > 1, "a short dashboard spreads the drawer out");
        assert!(drawer_h(n, drawer_cols(&s)) <= height(&s), "every chip shows");
        assert!(width(&s) <= s.placement.max_w());
        // Only the columns it needs.
        assert!(drawer_h(n, drawer_cols(&s) - 1) > height(&s));
    }
}
