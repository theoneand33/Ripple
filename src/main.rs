//! Ripple: a Dynamic Island-style bar at the top of the screen, drawn with Rune.

// Release builds on Windows open no console window beside the Island; `attach_console` keeps CLI output.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod agents;
mod command;
mod config;
mod date;
mod discord;
mod downloads;
mod icons;
mod ics;
mod island;
mod limits;
#[cfg(target_os = "macos")]
mod mac;
mod net;
mod nextcloud;
mod notify;
mod paths;
mod pick;
mod platform;
mod plugins;
mod settings;
mod stocks;
mod stremflix;
mod system;
mod t3;
mod task;
mod tabs;
#[cfg(test)]
mod testserver;

use rune::{App, AppHandle, AppSpec, MacWindowOptions, RunOptions, Tray, TrayEvent, TrayIcon, TrayItem, TraySpec, WindowHandle, WindowOptions, WindowSpec};

use config::{Store, Tab};
use island::{CANVAS, Host, Island, Mode};
use platform::Screen;
use settings::{Page, Settings};

static FONTS: &[&[u8]] = &[
    include_bytes!("../assets/fonts/OpenRunde-Regular.otf"),
    include_bytes!("../assets/fonts/OpenRunde-Medium.otf"),
    include_bytes!("../assets/fonts/OpenRunde-Semibold.otf"),
    include_bytes!("../assets/fonts/OpenRunde-Bold.otf"),
    include_bytes!("../assets/fonts/w95fa.otf"),
];
const FAMILIES: (&str, &str) = ("Open Runde", "ui-monospace");

fn tray_icon() -> Option<TrayIcon> {
    let decoder = png::Decoder::new(std::io::Cursor::new(include_bytes!("../assets/icons/tray.png").as_slice()));
    let mut reader = decoder.read_info().ok()?;
    let mut rgba = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut rgba).ok()?;
    (info.color_type == png::ColorType::Rgba).then(|| TrayIcon::rgba(info.width, info.height, rgba[..info.buffer_size()].to_vec()))
}

/// (id, title) of the tray menu's items; a separator goes before the last.
const TRAY_MENU: [(&str, &str); 3] = [("toggle", "Show/Hide Ripple"), ("settings", "Settings"), ("quit", "Quit")];

fn tray_pick(island: &Host, id: &str) {
    match id {
        // show, not open: bringing the Island back mustn't take focus from the app in use.
        "toggle" => if island.is_visible() { island.hide() } else { island.show() },
        "settings" => island.update(|s, _| if let Some(w) = &s.settings { w.open() }),
        // Saves a note typed in the last 2 s first; on the UI thread, so the save lands before the quit.
        _ => { let h = island.clone(); island.update(move |s, _| { s.store.flush(); h.quit() }) }
    }
}

/// The tray lives as long as the app; it is a process-wide singleton.
fn start_tray(island: Host) {
    #[cfg(target_os = "macos")]
    {
        mac::tray(include_bytes!("../assets/icons/tray.png"), &TRAY_MENU, move |id| tray_pick(&island, id));
        return;
    }
    #[cfg_attr(target_os = "macos", allow(unreachable_code))]
    let Some(icon) = tray_icon() else { return };
    let mut menu: Vec<TrayItem> = TRAY_MENU.iter().map(|(id, title)| TrayItem::action(*id, *title)).collect();
    menu.insert(TRAY_MENU.len() - 1, TrayItem::Separator);
    let spec = TraySpec::new("Ripple", "Ripple", icon).tooltip("Ripple").icon_name("ripple").menu(menu);
    match Tray::new(spec, move |event| match event {
        TrayEvent::Activate { .. } => tray_pick(&island, "toggle"),
        TrayEvent::Menu(id) => tray_pick(&island, &id),
        _ => {}
    }) {
        Ok(tray) => std::mem::forget(tray),
        Err(e) => eprintln!("Ripple: no tray icon: {e}"),
    }
}

fn island_spec() -> AppSpec<Island> {
    AppSpec {
        min_size: Some((0.0, 0.0)),
        fonts: FONTS,
        families: FAMILIES,
        setup: Some(Island::start),
        poll: Some(Island::poll),
        on_key: Some(Island::key),
        scenes: &[
            ("quick", |s| { s.cfg.standby = true; s.mode = Mode::Quick }),
            ("charging", |s| { s.cfg.standby = true; s.mode = Mode::Quick; (s.percent, s.charging) = (Some(80), true) }),
            ("volume", |s| { let tx = &mut rune::Tx::default(); s.set_levels(Some(40), None, tx); s.set_levels(Some(70), None, tx); }),
            ("brightness", |s| { let tx = &mut rune::Tx::default(); s.set_levels(None, Some(40), tx); s.set_levels(None, Some(85), tx); }),
            ("caps-lock", |s| { s.set_caps_lock(true); }),
            ("large", |s| s.mode = Mode::Large),
            ("weather-empty", |s| { s.mode = Mode::Large; s.tab = Tab::Weather }),
            ("weather", |s| {
                s.mode = Mode::Large;
                s.tab = Tab::Weather;
                s.cfg.location = "Paris".into();
                let day = |date, code, high, low| net::Day { date, code: Some(code), high, low };
                s.weather = net::Weather { temp: Some(12), code: Some(3), days: vec![day((2026, 1, 2), 3, 13, 4), day((2026, 1, 3), 61, 9, 5), day((2026, 1, 4), 0, 15, 6), day((2026, 1, 5), 71, 2, -3), day((2026, 1, 6), 95, 11, 7)], sun: None, precip: None };
            }),
            ("search", |s| { s.mode = Mode::Large; s.tab = Tab::Search }),
            ("search-results", |s| {
                s.mode = Mode::Large;
                s.tab = Tab::Search;
                s.finder.apps = Some(vec![config::QuickApp { name: "Notes".into(), launch: "notes".into() }]);
                s.finder.files = ["~/Documents/notes.md", "~/Projects/ripple-notes"].map(Into::into).into();
                s.finder.field.replace_text("notes");
            }),
            ("search-sum", |s| { s.mode = Mode::Large; s.tab = Tab::Search; s.finder.field.replace_text("(12+30)*1.5") }),
            ("workflows", |s| { s.mode = Mode::Large; s.tab = Tab::Workflows }),
            ("assistant", |s| { s.mode = Mode::Large; s.tab = Tab::Assistant; s.ai.asked = true; s.ai.answer = Some("**Rust** is a systems language.\n\n```rust\nfn main() {}\n```".into()) }),
            ("clipboard", |s| { s.mode = Mode::Large; s.tab = Tab::Clipboard; s.clipboard.push(tabs::Content::Text("Hello from Ripple".into())); }),
            ("clipboard-tools", |s| {
                s.mode = Mode::Large;
                s.tab = Tab::Clipboard;
                s.clipboard.push(tabs::Content::Text("1767260460".into()));
                s.clipboard.tools = true;
                s.clipboard.tool_error = Some("JSON: Not JSON".into());
            }),
            ("clipboard-pinned", |s| {
                s.mode = Mode::Large;
                s.tab = Tab::Clipboard;
                // A 2×1 PNG header is enough for the row's size; its pixels stay blank headless.
                let png = [&b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR"[..], &[0, 0, 7, 128, 0, 0, 4, 56]].concat();
                for t in ["ssh deploy@10.0.0.4", "https://github.com/TopMyster/Ripple"] { s.clipboard.push(tabs::Content::Text(t.into())); }
                s.clipboard.toggle_pin(0);
                if let Some(image) = rune::ClipboardImage::from_png(png) { s.clipboard_image(image); }
            }),
            ("tasks", |s| { s.mode = Mode::Large; s.tab = Tab::Tasks }),
            ("tasks-due", |s| {
                s.mode = Mode::Large;
                s.tab = Tab::Tasks;
                s.cfg.tasks = ["Water the plants due:2026-01-01 every:week", "!!! Ship v0.5 due:2026-01-02T17:00", "Read the RFC", "! Book flights due:2026-01-09"].map(config::Item::new).into();
            }),
            ("tasks-edit", |s| {
                s.mode = Mode::Large;
                s.tab = Tab::Tasks;
                s.cfg.tasks = ["!!! Ship v0.5 due:2026-01-02T17:00", "Water the plants due:2026-01-01 every:week"].map(config::Item::new).into();
                let key = s.cfg.tasks[0].key;
                s.tasks.draft = tabs::TaskDraft { priority: 3, due: Some((2026, 1, 2)), time: Some((17, 0)), every: Some(crate::task::Every::Week), editing: Some(key) };
                s.tasks.field.replace_text("Ship v0.5");
            }),
            ("reminder", |s| s.remind("Ship v0.5 release notes".into(), 0.0)),
            ("event-alert", |s| s.remind_event("Standup".into(), Some("https://meet.example.com/abc".into()), 0.0)),
            ("notify", |s| s.remind("Build passed".into(), 0.0)),
            ("rain-alert", |s| { s.weather.precip = Some((date::now_secs() + 3000, false)); s.remind(s.precip_label(), 0.0) }),
            ("break-reminder", |s| s.remind("Look away 20s".into(), 0.0)),
            ("update-alert", |s| s.remind("Ripple v0.7.0 is out".into(), 0.0)),
            ("tasks-sync-error", |s| {
                s.mode = Mode::Large;
                s.tab = Tab::Tasks;
                s.cfg.tasks = vec![config::Item::new("Water the plants")];
                s.nextcloud[nextcloud::Kind::Tasks].error = Some("Nextcloud refused the user or app password".into());
            }),
            ("calendar", |s| { s.mode = Mode::Large; s.tab = Tab::Calendar }),
            ("calendar-paged", |s| { s.mode = Mode::Large; s.tab = Tab::Calendar; s.calendar.offset = 1 }),
            ("calendar-events", |s| {
                s.mode = Mode::Large;
                s.tab = Tab::Calendar;
                s.cfg.events = ["2026-01-01 New Year's brunch", "2026-01-01 09:30 Standup", "2026-01-15 Dentist"].map(config::Item::new).into();
            }),
            ("calendar-feed", |s| {
                s.mode = Mode::Large;
                s.tab = Tab::Calendar;
                s.cfg.events = vec![config::Item::new("2026-01-01 09:30 Standup")];
                s.calendar.feed_events = vec!["2026-01-01 12:00 Lunch (Google)".into(), "2026-01-01 Public holiday".into()];
            }),
            ("calendar-edit", |s| {
                s.mode = Mode::Large;
                s.tab = Tab::Calendar;
                s.cfg.events = ["2026-01-01 New Year's brunch", "2026-01-01 09:30 Standup"].map(config::Item::new).into();
                let key = s.cfg.events[1].key;
                s.calendar.editing = Some(key);
                s.calendar.event.replace_text("09:30 Standup");
            }),
            ("habits", |s| {
                s.mode = Mode::Large;
                s.tab = Tab::Habits;
                let today = date::days_from_civil(s.today.0, s.today.1, s.today.2);
                s.cfg.habits = vec![
                    config::Habit { name: "Read 20 pages".into(), done: (0..5).map(|i| today - i).collect() },
                    config::Habit { name: "Stretch".into(), done: vec![today - 6, today - 3, today - 1] },
                    config::Habit { name: "No sugar".into(), done: Vec::new() },
                ];
            }),
            ("t3", |s| { s.mode = Mode::Large; s.tab = Tab::T3 }),
            ("stocks", |s| {
                s.mode = Mode::Large;
                s.tab = Tab::Stocks;
                s.cfg.sure_key = "demo".into();
                let account = |name: &str, balance, cash, positions, cost, value| stocks::Account { name: name.into(), balance, cash, positions, cost, value };
                s.stocks.portfolio = Some(Ok(stocks::Portfolio {
                    currency: "EUR".into(),
                    accounts: vec![account("Trading 212", 26_693.55, 612.45, 5, 21_457.15, 26_081.10), account("Kraken", 8_028.00, 230.50, 2, 5_750.00, 7_797.50)],
                    history: vec![34_409.06, 34_380.0, 34_450.2, 34_430.0, 34_520.8, 34_610.0, 34_590.4, 34_680.0, 34_721.55],
                    market: vec![0.0, -29.06, 70.2, -20.2, 90.8, 89.2, -19.6, 89.6, 41.55],
                    skipped: vec!["Wise (AED)".into()],
                }));
            }),
            ("stocks-watchlist", |s| {
                s.mode = Mode::Large;
                s.tab = Tab::Stocks;
                s.cfg.stocks = ["AAPL", "NVDA", "BTC-USD"].map(Into::into).into();
                let quote = |name: &str, currency: &str, price: f64, points: Vec<f64>| stocks::Quote { name: name.into(), currency: currency.into(), price, change: price - points[0], points };
                s.stocks.quotes = [
                    ("AAPL".to_string(), Ok(quote("Apple Inc.", "USD", 228.52, vec![226.0, 227.1, 226.4, 228.0, 228.52]))),
                    ("NVDA".to_string(), Ok(quote("NVIDIA Corporation", "USD", 131.04, vec![134.2, 133.0, 132.5, 131.8, 131.04]))),
                    ("BTC-USD".to_string(), Err("Couldn't reach Yahoo Finance".to_string())),
                ].into();
            }),
            ("clocks", |s| {
                s.mode = Mode::Large;
                s.tab = Tab::Clocks;
                let now = date::now_secs();
                let city = |name: &str, offset: i64| Some(net::City { name: name.into(), offset, sun: Some((now - 4 * 3600, now + 5 * 3600)) });
                s.clocks.cities = [("London", city("London", 0)), ("New York", city("New York", -5 * 3600)), ("Tokyo", city("Tokyo", 9 * 3600))].map(|(n, c)| (n.to_string(), c)).into();
            }),
            ("controls", |s| { s.mode = Mode::Large; s.tab = Tab::Controls; s.controls.toggles = [Some(true), Some(false), None]; s.controls.armed = Some(system::Power::Restart) }),
            ("overview-controls", |s| {
                s.mode = Mode::Large;
                s.tab = Tab::Overview;
                s.cfg.dashboard = [config::Widget::Clock, config::Widget::Panel(Tab::Controls)].map(config::Tile::new).into();
                s.controls.toggles = [Some(true), Some(true), Some(false)];
            }),
            ("colors", |s| { s.mode = Mode::Large; s.tab = Tab::Colors; s.colors.color = [0xe8, 0x5d, 0x75] }),
            ("pomodoro", |s| { s.mode = Mode::Large; s.tab = Tab::Pomodoro; s.pomodoro.left = 18.0 * 60.0 + 42.0; s.pomodoro.rounds = 2 }),
            // Ends on the app clock, which starts near zero in a headless render.
            ("timer", |s| { s.mode = Mode::Large; s.tab = Tab::Pomodoro; s.pomodoro.clock = tabs::Clock::Timer; s.pomodoro.timer.set(10.0, 0.0); s.pomodoro.timer.left = 4.0 * 60.0 + 12.0 }),
            ("stopwatch", |s| { s.mode = Mode::Large; s.tab = Tab::Pomodoro; s.pomodoro.clock = tabs::Clock::Stopwatch; s.pomodoro.stopwatch.elapsed = 83.0 }),
            ("timer-done", |s| { s.pomodoro.timer.rung = true; s.pomodoro.timer.left = 0.0 }),
            ("stopwatch-live", |s| { s.pomodoro.stopwatch.elapsed = 3723.0; s.pomodoro.stopwatch.since = Some(crate::date::wall() - 3723.0) }),
            ("pomodoro-live", |s| { let now = crate::date::wall(); s.pomodoro.ends = Some(now + 15.0 * 60.0 + 23.0); s.pomodoro.tick(now); }),
            ("download", |s| { s.set_download(Some(downloads::Progress { fraction: Some(0.42), count: 1, done: false }), &mut rune::Tx::default()); }),
            ("download-done", |s| { let mut tx = rune::Tx::default(); s.set_download(Some(downloads::Progress { fraction: Some(0.9), count: 1, done: false }), &mut tx); s.set_download(Some(downloads::Progress { fraction: Some(1.0), count: 1, done: true }), &mut tx); }),
            ("calculator", |s| {
                s.mode = Mode::Large;
                s.tab = Tab::Calculator;
                s.cfg.calc_tape = vec![("5 km to mi".into(), "3.106856 mi".into()), ("1299 * 0.8".into(), "1039.2".into()), ("(12 + 30) / 7".into(), "6".into())];
                s.calc.input.replace_text("1039.2 + 15%4");
            }),
            ("notes", |s| {
                s.mode = Mode::Large;
                s.tab = Tab::Notes;
                s.cfg.notes = [
                    "PR review queue\nGitHub\n• PR #482 — notch profile switcher polish\n• PR #491 — Screen Time category colors\n\nLeave blocking comments only; merge green builds.",
                    "Sprint board\nIn progress • MN-214 — Code hosting",
                    "Merge checklist\nBefore merge: CI green, changelog",
                ].map(config::Item::new).into();
                s.open_note(0);
            }),
            ("notes-pinned-search", |s| {
                s.mode = Mode::Large;
                s.tab = Tab::Notes;
                s.cfg.notes = ["Groceries\nMilk, eggs", "Sprint board\nMN-214", "Merge checklist\nCI green", "Board games\nCatan"].map(config::Item::new).into();
                s.cfg.pinned_notes = vec![s.cfg.notes[2].key];
                s.notes.search.replace_text("c");
                s.open_note(2);
            }),
            ("stremflix", |s| {
                s.mode = Mode::Large;
                s.tab = Tab::Stremflix;
                let entry = |id: &str, title: &str, position| stremflix::Entry { kind: "movie".into(), id: id.into(), title: title.into(), poster: None, position, duration: 100.0 };
                s.stremflix.entries = Some(Ok(vec![entry("a", "Big Buck Bunny", 40.0), entry("b", "Sintel", 80.0), entry("c", "A Very Long Title That Truncates", 10.0), entry("d", "Tears of Steel", 60.0), entry("e", "Elephants Dream", 5.0)]));
            }),
            ("system", |s| {
                s.mode = Mode::Large;
                s.tab = Tab::System;
                s.percent = Some(57);
                const GB: u64 = 1 << 30;
                s.stats = Some(system::Stats { cpu: 7.0, mem_used: 29 * GB, mem_total: 48 * GB, disk_free: 810 * GB, disk_total: 990 * GB, down: 15_200, up: 3_100,
                    top_cpu: vec![("firefox".into(), 12.4), ("rust-analyzer".into(), 6.1), ("ripple".into(), 0.8)],
                    top_mem: vec![("firefox".into(), 3 * GB + GB / 5), ("rust-analyzer".into(), 2 * GB), ("Slack".into(), 900 << 20)] });
            }),
            ("agents", |s| {
                s.mode = Mode::Large;
                s.tab = Tab::Agents;
                let session = |key: &str, agent: &str, cwd: &str, working, detail: &str| agents::Session { key: key.into(), agent: agent.into(), cwd: cwd.into(), working, detail: detail.into(), ..Default::default() };
                s.agents.sessions = vec![
                    session("claude-1", "claude", "/home/me/ripple", true, ""),
                    session("opencode-2", "opencode", "/home/me/site", false, ""),
                    session("codex-3", "codex", "/home/me/api", true, "Add rate limiting to the login endpoint"),
                    session("claude-4", "claude", "/home/me/notes", false, "Done: the README now covers the hook setup."),
                ];
                let choice = |label: &str| agents::Choice { label: label.into(), description: String::new() };
                s.agents.requests = vec![
                    agents::Request { key: "r1".into(), session: "claude-1".into(), tool: "Bash".into(), detail: "cargo build --release".into(), questions: Vec::new() },
                    agents::Request { key: "r2".into(), session: "opencode-2".into(), tool: "AskUserQuestion".into(), detail: String::new(), questions: vec![agents::Question {
                        question: "Which framework should the site use?".into(), header: "Framework".into(), options: vec![choice("Astro"), choice("Next.js"), choice("Plain HTML")], multi_select: false,
                    }] },
                ];
                s.agents.picks.insert(("r2".into(), 0), vec!["Astro".into()]);
            }),
            ("limits", |s| {
                s.mode = Mode::Large;
                s.tab = Tab::Limits;
                let now = date::now_secs();
                let w = |label: &str, used, hours: i64| limits::Window { label: label.into(), used, resets: Some(now + hours * 3600) };
                s.limits.usage = Some(vec![
                    limits::Usage { name: "Claude Code".into(), windows: Ok(vec![w("5 hours", 42.0, 3), w("Week", 71.0, 100)]) },
                    limits::Usage { name: "Codex · me@example.com".into(), windows: Ok(vec![w("5 hours", 8.0, 4), w("Week", 93.0, 30)]) },
                    limits::Usage { name: "OpenCode Go".into(), windows: Ok(vec![w("5 hours", 12.0, 2), w("Week", 55.0, 50), w("Month", 30.0, 400)]) },
                    limits::Usage { name: "Grok Build".into(), windows: Err("Token expired: run its CLI once to refresh".into()) },
                ]);
            }),
            ("plugins", |s| {
                s.mode = Mode::Large;
                s.tab = Tab::Plugins;
                // RIPPLE_PLUGINS=FILE shows that plugin; otherwise the example.
                if s.plugins.list.is_empty() { s.plugins.list.push(plugins::Plugin::load("example.lua".into(), None, include_str!("../plugins/example.lua"), None)) }
            }),
            ("plugins-todo", |s| {
                s.mode = Mode::Large;
                s.tab = Tab::Plugins;
                let mut p = plugins::Plugin::load("todo.lua".into(), None, include_str!("../plugins/todo.lua"), None);
                for task in ["Buy milk", "Ship the plugin API"] { p.type_key("draft", task); p.submit("draft") }
                p.type_key("draft", "Write the dra");
                p.press_key("item-1");
                s.plugins.list = vec![p];
            }),
            ("plugins-empty", |s| { s.mode = Mode::Large; s.tab = Tab::Plugins; s.plugins = Default::default() }),
            ("shelf-drop", |s| { s.mode = Mode::Large; s.tab = Tab::Shelf; s.shelf.dropping = true }),
            ("shelf", |s| { s.mode = Mode::Large; s.tab = Tab::Shelf; s.cfg.shelf = vec!["/home/me/Report final.pdf".into(), "/home/me/photo.png".into()] }),
            ("shelf-sync-error", |s| { s.mode = Mode::Large; s.tab = Tab::Shelf; s.cfg.shelf = vec!["/home/me/photo.png".into()]; s.shelf.sync.error = Some("Nextcloud refused the user or app password".into()) }),
            ("shelf-menu", |s| {
                s.mode = Mode::Large;
                s.tab = Tab::Shelf;
                s.cfg.shelf = vec!["/home/me/Report final.pdf".into(), "/home/me/photo.png".into()];
                s.shelf.menu = Some(("/home/me/photo.png".into(), rune::PointerPress { x: 500.0, y: 90.0 }));
                (s.open_select, s.hovered) = (Some("shelf"), true);
            }),
            ("shelf-rename", |s| {
                s.mode = Mode::Large;
                s.tab = Tab::Shelf;
                s.cfg.shelf = vec!["/home/me/Report final.pdf".into()];
                s.shelf.renaming = Some("/home/me/Report final.pdf".into());
                s.shelf.name = rune::TextEdit::new("Report final.pdf");
            }),
            ("shelf-undo", |s| { s.mode = Mode::Large; s.tab = Tab::Shelf; s.undo = Some(tabs::Undo::Shelf(vec!["/home/me/photo.png".into()])) }),
            ("t3-device", |s| {
                s.mode = Mode::Large;
                s.tab = Tab::T3;
                s.cfg.t3_devices = vec![config::T3Device { name: "Laptop".into(), url: "http://127.0.0.1:3773".into(), token: "t".into() }];
                s.t3.data = Some(t3::Loaded {
                    favorites: vec![t3::Favorite { instance_id: "claude".into(), model: "opus".into(), label: "Claude Opus".into() }],
                    runtime_mode: "full-access".into(),
                    links: vec![t3::Link { id: "env".into(), label: "Desktop".into() }],
                });
                s.t3.projects = Some(vec![t3::Project { id: "p".into(), title: "Ripple".into(), path: "/src/ripple".into() }]);
                s.t3.status = "Thread started in Ripple".into();
            }),
            ("playing", |s| {
                s.mode = Mode::Quick;
                let mut tx = rune::Tx::default();
                s.set_track(Some(system::Track { name: "A Very Long Song Title That Scrolls".into(), artist: "Artist".into(), playing: true, ..Default::default() }), &mut tx);
            }),
            ("playing-bubbles", |s| {
                let mut tx = rune::Tx::default();
                s.set_track(Some(system::Track { name: "Song".into(), artist: "Artist".into(), playing: true, ..Default::default() }), &mut tx);
                let now = crate::date::wall();
                s.pomodoro.ends = Some(now + 15.0 * 60.0 + 23.0);
                s.pomodoro.tick(now);
                s.set_download(Some(downloads::Progress { fraction: None, count: 2, done: false }), &mut tx);
                s.set_caps_lock(true);
            }),
            ("playing-progress", |s| {
                let mut tx = rune::Tx::default();
                s.set_track(Some(system::Track { name: "Song".into(), artist: "Artist".into(), playing: true, progress: Some((80, 200)), ..Default::default() }), &mut tx);
                s.set_capture(None, Some(true), &mut tx);
                s.poll(4.0, &mut tx); // the mic alert ends; its dot stays
            }),
            ("overview-playing", |s| {
                s.mode = Mode::Large;
                s.tab = Tab::Overview;
                let mut tx = rune::Tx::default();
                s.set_track(Some(system::Track { name: "Song".into(), artist: "Artist".into(), playing: true, progress: Some((89, 200)), ..Default::default() }), &mut tx);
            }),
            ("overview-lyrics", |s| {
                s.cfg.lyrics = true;
                s.mode = Mode::Large;
                s.tab = Tab::Overview;
                let mut tx = rune::Tx::default();
                s.set_track(Some(system::Track { name: "Song".into(), artist: "Artist".into(), playing: true, progress: Some((89, 200)), ..Default::default() }), &mut tx);
                s.lyrics = Some(net::parse_lrc("[01:20.00]The line before\n[01:28.50]And this is the line sung now"));
            }),
            ("overview", |s| { s.mode = Mode::Large; s.tab = Tab::Overview; s.cfg.location = "Lisbon".into(); s.weather.temp = Some(21); s.percent = Some(64) }),
            ("overview-panels", |s| {
                s.mode = Mode::Large;
                s.tab = Tab::Overview;
                s.cfg.dashboard = [config::Widget::Clock, config::Widget::Battery, config::Widget::Panel(Tab::Tasks), config::Widget::Panel(Tab::System)].map(config::Tile::new).into();
                s.cfg.tasks = vec![config::Item::new("Ship the dashboard"), config::Item::new("Water the plants")];
            }),
            ("overview-water", |s| {
                s.mode = Mode::Large;
                s.tab = Tab::Overview;
                s.cfg.dashboard = [config::Widget::Clock, config::Widget::Water].map(config::Tile::new).into();
                (s.cfg.water_day, s.cfg.water_count) = (date::days_from_civil(s.today.0, s.today.1, s.today.2), 3);
            }),
            ("overview-contributions", |s| {
                s.mode = Mode::Large;
                s.tab = Tab::Overview;
                s.cfg.dashboard = [config::Widget::Contributions].map(config::Tile::new).into();
                (s.cfg.github_user, s.cfg.leetcode_user) = ("octocat".into(), "ana".into());
                let today = date::days_from_civil(s.today.0, s.today.1, s.today.2);
                let graph = |seed: i64| (0..365).filter(|d| (d * seed) % 7 < 3 || *d < 6).map(|d| (today - d, ((d * seed) % 4 + 1) as u8)).collect::<Vec<_>>();
                s.contributions.github = Some(("octocat".into(), Ok(graph(5))));
                s.contributions.leetcode = Some(("ana".into(), Ok(graph(3))));
            }),
            ("discord", |s| { s.mode = Mode::Large; s.tab = Tab::Discord; discord_scene(s) }),
            ("overview-discord", |s| {
                s.mode = Mode::Large;
                s.tab = Tab::Overview;
                s.cfg.dashboard = [config::Tile::new(config::Widget::Voice), config::Tile::new(config::Widget::Clock), config::Tile::new(config::Widget::Voice).sized(4, 1)].into();
                discord_scene(s);
            }),
            ("overview-quote", |s| { s.mode = Mode::Large; s.tab = Tab::Overview; s.cfg.dashboard = [config::Widget::Quote, config::Widget::Clock].map(config::Tile::new).into() }),
            ("overview-day", |s| {
                s.mode = Mode::Large;
                s.tab = Tab::Overview;
                s.cfg.dashboard = [config::Widget::Day].map(config::Tile::new).into();
                s.cfg.events = ["2026-01-01 09:30 Standup", "2026-01-01 13:00 Lunch with Sam"].map(config::Item::new).into();
                s.cfg.tasks = ["Ship v0.5 due:2026-01-01T17:00"].map(config::Item::new).into();
            }),
            ("overview-wide", |s| {
                s.mode = Mode::Large; s.tab = Tab::Overview;
                // Too tall for four columns: side by side in five.
                s.cfg.dashboard = [(config::Widget::Media, 3), (config::Widget::Clock, 2), (config::Widget::Quote, 3), (config::Widget::Battery, 2)].map(|(w, n)| config::Tile::new(w).sized(n, 3)).into();
            }),
            ("overview-full", |s| {
                s.cfg.notch_mode = true; s.placement = island::Placement::of(&s.cfg, (1920.0, 1080.0)); s.mode = Mode::Large; s.tab = Tab::Overview;
                // Too tall at any width: every tile shrinks to show at a glance.
                s.cfg.dashboard = config::Widget::OWN.into_iter().map(|w| config::Tile::new(w).sized(4, 3)).collect();
            }),
            ("overview-small", |s| { sized_widgets(s, 2, 1) }),
            ("overview-tiny", |s| { sized_widgets(s, 1, 1) }),
            ("overview-roomy", |s| { sized_widgets(s, 4, 3); s.cfg.dashboard.truncate(4) }),
            ("overview-narrow", |s| { sized_widgets(s, 1, 2) }),
            ("overview-edit", |s| { s.mode = Mode::Large; s.tab = Tab::Overview; s.dashboard.edit = true }),
            ("overview-drag-in", |s| { s.mode = Mode::Large; s.tab = Tab::Overview; s.dashboard.edit = true; s.dashboard.add = Some((config::Widget::Quote, Some(2))) }),
            ("overview-resize", |s| { s.mode = Mode::Large; s.tab = Tab::Overview; s.dashboard.edit = true; s.dashboard.drag = Some((0, -130.0, -60.0, true)) }),
            ("camera", |s| { let mut tx = rune::Tx::default(); s.set_capture(Some(true), None, &mut tx); }),
            ("capture-both", |s| {
                let mut tx = rune::Tx::default();
                s.set_capture(Some(true), Some(true), &mut tx);
                s.poll(4.0, &mut tx); // the camera alert ends; the mic alert takes its turn
                s.poll(8.0, &mut tx); // both icons stay
            }),
            ("bluetooth-off", |s| { let mut tx = rune::Tx::default(); s.set_bluetooth(true, &mut tx); s.set_bluetooth(false, &mut tx); s.set_bluetooth(false, &mut tx); }),
            ("notch", |s| { s.cfg.notch_mode = true; s.placement = island::Placement::of(&s.cfg, (1920.0, 1080.0)) }),
            ("notch-large", |s| { s.cfg.notch_mode = true; s.placement = island::Placement::of(&s.cfg, (1920.0, 1080.0)); s.mode = Mode::Large }),
            ("mini-notch", |s| { s.cfg.notch_mode = true; s.cfg.notch_mini = true; s.placement = island::Placement::of(&s.cfg, (1920.0, 1080.0)) }),
            ("mini-notch-single", |s| { s.cfg.notch_mode = true; s.cfg.notch_mini = true; s.cfg.show_info_idle = true; s.weather.temp = None; s.percent = None; s.placement = island::Placement::of(&s.cfg, (1920.0, 1080.0)) }),
            ("island-scale", |s| { s.cfg.island_scale = 80.0; s.cfg.compact_opacity = 60.0; s.cfg.standby = true; s.mode = Mode::Quick; s.placement = island::Placement::of(&s.cfg, (1920.0, 1080.0)) }),
            ("win95", |s| { s.cfg.set_theme("win95"); s.mode = Mode::Large }),
        ],
        ..AppSpec::new("Ripple", CANVAS, || unreachable!("state comes from App::init"), island::view)
    }
}

/// A scene: every Overview widget at `w`×`h` cells, with something in each to show.
fn sized_widgets(s: &mut Island, w: i32, h: i32) {
    s.mode = Mode::Large;
    s.tab = Tab::Overview;
    s.cfg.dashboard = config::Widget::OWN.into_iter().map(|x| config::Tile::new(x).sized(w, h)).collect();
    (s.cfg.location, s.weather.temp, s.percent) = ("Lisbon".into(), Some(21), Some(64));
    s.cfg.events = ["2026-01-01 09:30 Standup", "2026-01-01 13:00 Lunch with Sam", "2026-01-01 16:00 Review", "2026-01-01 18:00 Gym"].map(config::Item::new).into();
    s.cfg.tasks = ["Ship v0.5 due:2026-01-01T17:00", "Water the plants"].map(config::Item::new).into();
    (s.cfg.water_day, s.cfg.water_count) = (date::days_from_civil(s.today.0, s.today.1, s.today.2), 3);
    s.cfg.github_user = "octocat".into();
    let today = date::days_from_civil(s.today.0, s.today.1, s.today.2);
    s.contributions.github = Some(("octocat".into(), Ok((0..365).filter(|d| d % 3 == 0).map(|d| (today - d, (d % 4 + 1) as u8)).collect())));
    let mut tx = rune::Tx::default();
    s.set_track(Some(system::Track { name: "A Song With a Long Title".into(), artist: "Artist".into(), playing: true, progress: Some((89, 200)), ..Default::default() }), &mut tx);
}

fn settings_spec() -> AppSpec<Settings> {
    AppSpec {
        min_size: Some((600.0, 400.0)),
        fonts: FONTS,
        families: FAMILIES,
        scenes: &[
            ("tabs", |s| s.page = Page::Tabs),
            ("appearance", |s| s.page = Page::Appearance),
            ("appearance-sun", |s| { s.page = Page::Appearance; s.cfg.auto_theme = true }),
            ("features", |s| s.page = Page::Features),
            ("features-quiet", |s| { s.page = Page::Features; s.cfg.quiet_hours = true; s.cfg.quiet_fullscreen = true }),
            ("search", |s| s.query.replace_text("alert")),
            ("panel-stocks", |s| s.page = Page::Tab(Tab::Stocks)),
            ("panel-discord", |s| s.page = Page::Tab(Tab::Discord)),
            ("panel-ai", |s| s.page = Page::Tab(Tab::Assistant)),
            ("panel-limits", |s| s.page = Page::Tab(Tab::Limits)),
            ("panel-tasks", |s| s.page = Page::Tab(Tab::Tasks)),
        ],
        ..AppSpec::new("Ripple Settings", (720.0, 720.0), || unreachable!("state comes from WindowSpec::state"), settings::view)
    }
}

/// The hidden Settings window, on whichever app hosts the Island.
fn settings_window<S: 'static>(app: &mut App<S>, state: Settings) -> (AppHandle<Settings>, WindowHandle) {
    app.window(
        WindowSpec::new("settings", settings_spec())
            .state(state)
            .options(WindowOptions { visible: false, min_size: Some((600.0, 400.0)), ..Default::default() }),
    )
}

/// Whether the Wayland compositor offers `zwlr_layer_shell_v1` (KDE, wlroots; not GNOME).
#[cfg(target_os = "linux")]
fn has_layer_shell() -> bool {
    use wayland_client::{Connection, Dispatch, QueueHandle, globals::{GlobalListContents, registry_queue_init}, protocol::wl_registry::WlRegistry};
    struct Probe;
    impl Dispatch<WlRegistry, GlobalListContents> for Probe {
        fn event(_: &mut Self, _: &WlRegistry, _: <WlRegistry as wayland_client::Proxy>::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
    }
    let Ok(conn) = Connection::connect_to_env() else { return false };
    let Ok((globals, _)) = registry_queue_init::<Probe>(&conn) else { return false };
    globals.contents().with_list(|list| list.iter().any(|g| g.interface == "zwlr_layer_shell_v1"))
}

/// Wayland lets no ordinary window place or raise itself, so there the Island is a layer-shell
/// overlay on a windowless app. Without layer-shell (GNOME) it stays an XWayland window: Rune's
/// XWayland fallback refuses overlays that take keys.
#[cfg(target_os = "linux")]
fn overlay_island(screens: Vec<Screen>, store: Store) -> Vec<rune::AnyWindowSpec> {
    use rune::{Align, KeyPolicy, OverlayInput, OverlayOptions, OverlayPlacement, OverlaySpec, ScreenEdge};
    use std::sync::{Arc, atomic::AtomicBool};
    let spec = AppSpec { fonts: FONTS, families: FAMILIES, ..AppSpec::new("Ripple", (1.0, 1.0), || (), |_, _| rune::el()) };
    let mut host = App::new(spec).windowless();
    let state = Island::new(store.clone(), screens.clone());
    let screen = state.screen();
    let [x, y, width, height] = state.placement.frame();
    let options = OverlayOptions {
        placement: Some(OverlayPlacement::Rect { x, y, width, height }),
        display: screen.target,
        // Only the Island, its dock and an open select's menu (`.hit()`) take the pointer.
        input: OverlayInput::Regions,
        keys: KeyPolicy::OnDemand,
        xwayland_input: true,
        overlap_panels: state.cfg.overlap_panels,
        ..Default::default()
    };
    let spec = OverlaySpec { name: "ripple", size: (width, height), edge: ScreenEdge::Top, margin: 0.0, align: Align::Start, inset: 0.0, interactive: true, view: island::view };
    let overlay = host.overlay(
        spec.options(options).setup(Island::start).poll(Island::poll).on_key(Island::key).on_focus(Island::window_focus),
        state,
    );
    let me = Host::Overlay(overlay.clone(), host.handle(), Arc::new(AtomicBool::new(false)));
    let (settings_app, settings) = settings_window(&mut host, Settings::new(store, Some(me.clone()), &screens));
    let island = me.clone();
    overlay.update(move |s, _| { s.me = Some(island); s.settings = Some(settings); s.settings_app = Some(settings_app); s.begin() });
    me.show();
    start_tray(me);
    vec![WindowSpec::from_app("host", host).into()]
}

/// Writes stdout and stderr to the terminal that ran Ripple, unless they are already redirected.
#[cfg(windows)]
fn attach_console() {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetStdHandle(which: u32) -> isize;
        fn AttachConsole(pid: u32) -> i32;
    }
    const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
    // STD_OUTPUT_HANDLE and STD_ERROR_HANDLE; unset reads as NULL or INVALID_HANDLE_VALUE.
    // SAFETY: plain Win32 calls; attaching fails harmlessly when there is no parent console or one is attached.
    unsafe {
        if [-11i32, -12].into_iter().any(|h| matches!(GetStdHandle(h as u32), 0 | -1)) { AttachConsole(ATTACH_PARENT_PROCESS); }
    }
}

/// Reverse-DNS: GNOME's shortcuts portal refuses other app ids.
const APP_ID: &str = "io.github.theoneand33.Ripple";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "--test-plugin") {
        // Only here: an attached GUI process dies with its terminal, and this path exits at once.
        #[cfg(windows)]
        attach_console();
        std::process::exit(plugins::test_cli(&args[1..]))
    }
    if args.first().is_some_and(|a| a == "notify") {
        #[cfg(windows)]
        attach_console();
        std::process::exit(notify::cli(&args[1..]))
    }
    // No console: the agent reads the hook's answer from the stdout it piped.
    if args.first().is_some_and(|a| a == "hook") { std::process::exit(agents::cli(&args[1..])) }
    #[cfg(target_os = "linux")]
    let wayland = {
        let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
        // Headless tools (scenes, snapshots) still drive the Island as a window.
        let overlay = wayland && !rune::is_headless() && has_layer_shell();
        // SAFETY: single-threaded here, before the event loop or any thread starts.
        unsafe { system::note_wayland(!overlay && std::env::var_os("DISPLAY").is_some()) };
        if wayland && !rune::is_headless() { system::install_desktop_entry(APP_ID) }
        overlay
    };
    rune::set_app_id(APP_ID);

    rune::run_windows_with(RunOptions { accessory: true }, move |displays| {
        let screens = Screen::all(displays);
        let store = Store::open();
        #[cfg(target_os = "linux")]
        if wayland { return overlay_island(screens, store) }
        // The Island only takes focus when clicked, never on launch (macOS and Windows).
        // Saves a note typed since the last per-second save: macOS Quit and logout, Windows logoff and shutdown.
        let mut app = App::new(island_spec()).keep_running().open_inactive().on_focus(Island::window_focus).on_quit(|s, _| s.store.flush());
        let island = Host::Window(app.handle());

        let settings_state = Settings::new(store.clone(), Some(island.clone()), &screens);
        let (settings_app, settings_window) = settings_window(&mut app, settings_state);
        #[cfg(windows)]
        std::thread::spawn({
            let settings_app = settings_app.clone();
            move || {
                let apps = system::discover_apps();
                settings_app.update(move |s, _| s.installed = apps);
            }
        });

        let mut state = Island::new(store, screens);
        state.me = Some(island.clone());
        state.settings = Some(settings_window);
        state.settings_app = Some(settings_app);
        let screen = state.screen();
        let position = (!state.screens.is_empty()).then(|| {
            let [x, y, ..] = state.placement.frame();
            (screen.position.0 + (x * screen.scale).round() as i32, screen.position.1 + (y * screen.scale).round() as i32)
        });

        let handle = island.clone();
        let app = app.init(move || state).on_window(move |window| {
            platform::adopt(window);
            let window = window.clone();
            handle.update(move |s, _| { s.set_window(window); s.place_window() });
        });
        if !rune::is_headless() { start_tray(island.clone()) }

        let options = WindowOptions {
            transparent: true,
            borderless: true,
            always_on_top: true,
            resizable: false,
            position,
            min_size: Some((0.0, 0.0)),
            ..Default::default()
        };
        // CanJoinAllSpaces | Stationary | IgnoresCycle | FullScreenAuxiliary, at the pop-up menu level.
        let options = options.macos(MacWindowOptions::default().shadow(false).level(101).collection_behavior(1 | 16 | 64 | 256));
        vec![rune::AnyWindowSpec::from(WindowSpec::from_app("island", app).configured_options(options))]
    });
}

/// A Discord call for the scenes: four in General, one speaking, one muted and deafened.
fn discord_scene(s: &mut Island) {
    (s.cfg.discord_id, s.cfg.discord_secret) = ("1".into(), "s".into());
    let m = |id: &str, name: &str, speaking, off| discord::Member { id: id.into(), name: name.into(), avatar: None, speaking, muted: off, deafened: off };
    let members = vec![m("1", "Ada", true, false), m("2", "Bob", false, true), m("3", "Carol with a very long display name", false, false), m("4", "Dee", false, false)];
    s.discord_set(Ok(discord::Voice { channel: Some(discord::Channel { id: "c".into(), name: "General".into(), members }), mute: true, deaf: false }));
}
