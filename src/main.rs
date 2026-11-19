//! Ripple: a Dynamic Island-style bar at the top of the screen, drawn with Rune.

// Release builds on Windows open no console window beside the Island; `attach_console` keeps CLI output.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod command;
mod config;
mod icons;
mod island;
mod limits;
#[cfg(target_os = "macos")]
mod mac;
mod net;
mod nextcloud;
mod platform;
mod plugins;
mod settings;
mod stremflix;
mod system;
mod t3;
mod tabs;
#[cfg(test)]
mod testserver;

use rune::{App, AppHandle, AppSpec, MacWindowOptions, RunOptions, Tray, TrayEvent, TrayIcon, TrayItem, TraySpec, WindowHandle, WindowOptions, WindowSpec};

use config::{Store, Tab};
use island::{CANVAS, Host, Island, Mode};
use platform::Screen;
use settings::Settings;

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
            ("search", |s| { s.mode = Mode::Large; s.set_tab(Tab::Search) }),
            ("workflows", |s| { s.mode = Mode::Large; s.set_tab(Tab::Workflows) }),
            ("assistant", |s| { s.mode = Mode::Large; s.set_tab(Tab::Assistant); s.ai.asked = true; s.ai.answer = Some("**Rust** is a systems language.\n\n```rust\nfn main() {}\n```".into()) }),
            ("clipboard", |s| { s.mode = Mode::Large; s.set_tab(Tab::Clipboard); s.clipboard.push("Hello from Ripple"); }),
            ("tasks", |s| { s.mode = Mode::Large; s.set_tab(Tab::Tasks) }),
            ("tasks-sync-error", |s| {
                s.mode = Mode::Large;
                s.set_tab(Tab::Tasks);
                s.cfg.tasks = vec![config::Item::new("Water the plants")];
                s.nextcloud[nextcloud::Kind::Tasks as usize].error = Some("Nextcloud refused the user or app password".into());
            }),
            ("calendar", |s| { s.mode = Mode::Large; s.set_tab(Tab::Calendar) }),
            ("calendar-paged", |s| { s.mode = Mode::Large; s.set_tab(Tab::Calendar); s.calendar_offset = 1 }),
            ("calendar-events", |s| {
                s.mode = Mode::Large;
                s.set_tab(Tab::Calendar);
                s.cfg.events = ["2026-01-01 New Year's brunch", "2026-01-01 09:30 Standup", "2026-01-15 Dentist"].map(config::Item::new).into();
            }),
            ("t3", |s| { s.mode = Mode::Large; s.set_tab(Tab::T3) }),
            ("notes", |s| {
                s.mode = Mode::Large;
                s.set_tab(Tab::Notes);
                s.cfg.notes = [
                    "PR review queue\nGitHub\n• PR #482 — notch profile switcher polish\n• PR #491 — Screen Time category colors\n\nLeave blocking comments only; merge green builds.",
                    "Sprint board\nIn progress • MN-214 — Code hosting",
                    "Merge checklist\nBefore merge: CI green, changelog",
                ].map(config::Item::new).into();
                s.open_note(0);
            }),
            ("stremflix", |s| {
                s.mode = Mode::Large;
                s.set_tab(Tab::Stremflix);
                let entry = |id: &str, title: &str, position| stremflix::Entry { kind: "movie".into(), id: id.into(), title: title.into(), poster: None, position, duration: 100.0 };
                s.stremflix.entries = Some(Ok(vec![entry("a", "Big Buck Bunny", 40.0), entry("b", "Sintel", 80.0), entry("c", "A Very Long Title That Truncates", 10.0), entry("d", "Tears of Steel", 60.0), entry("e", "Elephants Dream", 5.0)]));
            }),
            ("system", |s| {
                s.mode = Mode::Large;
                s.set_tab(Tab::System);
                s.percent = Some(57);
                const GB: u64 = 1 << 30;
                s.stats = Some(system::Stats { cpu: 7.0, mem_used: 29 * GB, mem_total: 48 * GB, disk_free: 810 * GB, disk_total: 990 * GB, down: 15_200, up: 3_100 });
            }),
            ("limits", |s| {
                s.mode = Mode::Large;
                s.set_tab(Tab::Limits);
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
                let w = |label: &str, used, hours: i64| limits::Window { label: label.into(), used, resets: Some(now + hours * 3600) };
                s.limits = Some(vec![
                    limits::Usage { name: "Claude Code".into(), windows: Ok(vec![w("5 hours", 42.0, 3), w("Week", 71.0, 100)]) },
                    limits::Usage { name: "Codex · me@example.com".into(), windows: Ok(vec![w("5 hours", 8.0, 4), w("Week", 93.0, 30)]) },
                    limits::Usage { name: "OpenCode Go".into(), windows: Ok(vec![w("5 hours", 12.0, 2), w("Week", 55.0, 50), w("Month", 30.0, 400)]) },
                    limits::Usage { name: "Grok Build".into(), windows: Err("Token expired: run its CLI once to refresh".into()) },
                ]);
            }),
            ("plugins", |s| {
                s.mode = Mode::Large;
                s.set_tab(Tab::Plugins);
                // RIPPLE_PLUGINS=FILE shows that plugin; otherwise the example.
                if s.plugins.list.is_empty() { s.plugins.list.push(plugins::Plugin::load("example.lua".into(), None, include_str!("../plugins/example.lua"))) }
            }),
            ("plugins-empty", |s| { s.mode = Mode::Large; s.set_tab(Tab::Plugins); s.plugins = Default::default() }),
            ("shelf-drop", |s| { s.mode = Mode::Large; s.set_tab(Tab::Shelf); s.dropping = true }),
            ("shelf", |s| { s.mode = Mode::Large; s.set_tab(Tab::Shelf); s.cfg.shelf = vec!["/home/me/Report final.pdf".into(), "/home/me/photo.png".into()] }),
            ("t3-device", |s| {
                s.mode = Mode::Large;
                s.set_tab(Tab::T3);
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
            ("playing-progress", |s| {
                let mut tx = rune::Tx::default();
                s.set_track(Some(system::Track { name: "Song".into(), artist: "Artist".into(), playing: true, progress: Some((80, 200)), ..Default::default() }), &mut tx);
                s.set_capture(None, Some(true), &mut tx);
                s.poll(4.0, &mut tx); // the mic alert ends; its dot stays
            }),
            ("overview-playing", |s| {
                s.mode = Mode::Large;
                s.set_tab(Tab::Overview);
                let mut tx = rune::Tx::default();
                s.set_track(Some(system::Track { name: "Song".into(), artist: "Artist".into(), playing: true, ..Default::default() }), &mut tx);
            }),
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
            ("win95", |s| { s.cfg.set_theme("win95"); s.mode = Mode::Large }),
        ],
        ..AppSpec::new("Ripple", CANVAS, || unreachable!("state comes from App::init"), island::view)
    }
}

fn settings_spec() -> AppSpec<Settings> {
    AppSpec {
        min_size: Some((420.0, 400.0)),
        fonts: FONTS,
        families: FAMILIES,
        scenes: &[("panel-ai", |s| s.page = Some(Tab::Assistant)), ("panel-limits", |s| s.page = Some(Tab::Limits)), ("panel-tasks", |s| s.page = Some(Tab::Tasks))],
        ..AppSpec::new("Ripple Settings", (520.0, 720.0), || unreachable!("state comes from WindowSpec::state"), settings::view)
    }
}

/// The hidden Settings window, on whichever app hosts the Island.
fn settings_window<S: 'static>(app: &mut App<S>, state: Settings) -> (AppHandle<Settings>, WindowHandle) {
    app.window(
        WindowSpec::new("settings", settings_spec())
            .state(state)
            .options(WindowOptions { visible: false, min_size: Some((420.0, 400.0)), ..Default::default() }),
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
    let (x, y) = state.placement.window;
    let options = OverlayOptions {
        placement: Some(OverlayPlacement::Rect { x, y, width: CANVAS.0, height: CANVAS.1 }),
        display: screen.target,
        // Only the Island, its dock and an open select's menu (`.hit()`) take the pointer.
        input: OverlayInput::Regions,
        keys: KeyPolicy::OnDemand,
        xwayland_input: true,
        overlap_panels: state.cfg.overlap_panels,
        ..Default::default()
    };
    let spec = OverlaySpec { name: "ripple", size: CANVAS, edge: ScreenEdge::Top, margin: 0.0, align: Align::Start, inset: 0.0, interactive: true, view: island::view };
    let overlay = host.overlay(
        spec.options(options).setup(Island::start).poll(Island::poll).on_key(Island::key).on_focus(Island::window_focus),
        state,
    );
    let me = Host::Overlay(overlay.clone(), host.handle(), Arc::new(AtomicBool::new(false)));
    let (_, settings) = settings_window(&mut host, Settings::new(store, Some(me.clone()), &screens));
    let island = me.clone();
    overlay.update(move |s, _| { s.me = Some(island); s.settings = Some(settings); s.begin() });
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

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "--test-plugin") {
        // Only here: an attached GUI process dies with its terminal, and this path exits at once.
        #[cfg(windows)]
        attach_console();
        std::process::exit(plugins::test_cli(&args[1..]))
    }
    #[cfg(target_os = "linux")]
    let wayland = {
        let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
        // Headless tools (scenes, snapshots) still drive the Island as a window.
        let overlay = wayland && !rune::is_headless() && has_layer_shell();
        // SAFETY: single-threaded here, before the event loop or any thread starts.
        unsafe { system::note_wayland(!overlay && std::env::var_os("DISPLAY").is_some()) };
        overlay
    };
    rune::set_app_id("ripple");

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
        std::thread::spawn(move || {
            let apps = system::discover_apps();
            settings_app.update(move |s, _| s.installed = apps);
        });
        #[cfg(not(windows))]
        let _ = settings_app;

        let mut state = Island::new(store, screens);
        state.me = Some(island.clone());
        state.settings = Some(settings_window);
        let screen = state.screen();
        let position = (!state.screens.is_empty()).then(|| {
            let (x, y) = state.placement.window;
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
