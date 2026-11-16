//! Ripple: a Dynamic Island-style bar at the top of the screen, drawn with Rune.

mod command;
mod config;
mod icons;
mod island;
#[cfg(target_os = "macos")]
mod mac;
mod migrate;
mod net;
mod platform;
mod settings;
mod system;
mod t3;
mod tabs;

use rune::{App, AppHandle, AppSpec, MacWindowOptions, RunOptions, Tray, TrayEvent, TrayIcon, TrayItem, TraySpec, WindowOptions, WindowSpec};

use config::Store;
use island::{CANVAS, Island, Screen};
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

/// The tray lives as long as the app; it is a process-wide singleton.
fn start_tray(island: AppHandle<Island>) {
    #[cfg(target_os = "macos")]
    {
        mac::tray(include_bytes!("../assets/icons/tray.png"), move |id| match id {
            "toggle" => if island.is_visible() { island.hide() } else { island.open() },
            "settings" => island.update(|s, _| if let Some(w) = &s.settings { w.open() }),
            _ => island.quit(),
        });
        return;
    }
    #[cfg_attr(target_os = "macos", allow(unreachable_code))]
    let Some(icon) = tray_icon() else { return };
    let menu = vec![
        TrayItem::action("toggle", "Show/Hide Ripple"),
        TrayItem::action("settings", "Settings"),
        TrayItem::Separator,
        TrayItem::action("quit", "Quit"),
    ];
    let handle = island.clone();
    let toggle = move || if handle.is_visible() { handle.hide() } else { handle.open() };
    let spec = TraySpec::new("Ripple", "Ripple", icon).tooltip("Ripple").icon_name("ripple").menu(menu);
    match Tray::new(spec, move |event| match event {
        TrayEvent::Activate { .. } => toggle(),
        TrayEvent::Menu(id) if id == "toggle" => toggle(),
        TrayEvent::Menu(id) if id == "settings" => island.update(|s, _| if let Some(w) = &s.settings { w.open() }),
        TrayEvent::Menu(id) if id == "quit" => island.quit(),
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
            ("quick", |s| { s.cfg.standby = true; s.mode = island::Mode::Quick }),
            ("large", |s| s.mode = island::Mode::Large),
            ("search", |s| { s.mode = island::Mode::Large; s.tab = 0 }),
            ("workflows", |s| { s.mode = island::Mode::Large; s.tab = 1 }),
            ("assistant", |s| { s.mode = island::Mode::Large; s.tab = 4; s.asked = true; s.answer = Some("**Rust** is a systems language.\n\n```rust\nfn main() {}\n```".into()) }),
            ("clipboard", |s| { s.mode = island::Mode::Large; s.tab = 5; s.push_clipboard("Hello from Ripple"); }),
            ("tasks", |s| { s.mode = island::Mode::Large; s.tab = 6 }),
            ("t3", |s| { s.mode = island::Mode::Large; s.tab = 8 }),
            ("t3-device", |s| {
                s.mode = island::Mode::Large;
                s.tab = 8;
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
                s.mode = island::Mode::Quick;
                let mut tx = rune::Tx::default();
                s.set_track(Some(system::Track { name: "A Very Long Song Title That Scrolls".into(), artist: "Artist".into(), playing: true, ..Default::default() }), &mut tx);
            }),
            ("overview-playing", |s| {
                s.mode = island::Mode::Large;
                s.tab = 2;
                let mut tx = rune::Tx::default();
                s.set_track(Some(system::Track { name: "Song".into(), artist: "Artist".into(), playing: true, ..Default::default() }), &mut tx);
            }),
            ("camera", |s| { let mut tx = rune::Tx::default(); s.set_capture(Some(true), None, &mut tx); }),
            ("notch", |s| { s.cfg.notch_mode = true; s.placement = island::Placement::of(&s.cfg, (1920.0, 1080.0)) }),
            ("win95", |s| { s.cfg.set_theme("win95"); s.kit = island::kit(&s.cfg); s.mode = island::Mode::Large }),
        ],
        ..AppSpec::new("Ripple", CANVAS, || unreachable!("state comes from App::init"), island::view)
    }
}

fn settings_spec() -> AppSpec<Settings> {
    AppSpec {
        min_size: Some((420.0, 400.0)),
        fonts: FONTS,
        families: FAMILIES,
        ..AppSpec::new("Ripple Settings", (520.0, 720.0), || unreachable!("state comes from WindowSpec::state"), settings::view)
    }
}

fn main() {
    // The Island needs to place and raise its own window, which Wayland doesn't allow: run on
    // XWayland, as Electron did. Apps Ripple starts get Wayland back.
    #[cfg(target_os = "linux")]
    {
        let wayland = std::env::var_os("WAYLAND_DISPLAY");
        let _ = system::WAYLAND_DISPLAY.set(wayland.clone());
        if wayland.is_some() && std::env::var_os("DISPLAY").is_some() {
            // SAFETY: single-threaded here, before the event loop or any thread starts.
            unsafe { std::env::remove_var("WAYLAND_DISPLAY") };
        }
    }
    rune::set_app_id("ripple");

    rune::run_windows_with(RunOptions { accessory: true }, |displays| {
        let screens = Screen::all(displays);
        let store = Store::open();
        let mut app = App::new(island_spec()).keep_running().on_focus(Island::window_focus);
        let island = app.handle();

        let settings_state = Settings::new(store.clone(), Some(island.clone()), &screens);
        let (settings_app, settings_window) = app.window(
            WindowSpec::new("settings", settings_spec())
                .state(settings_state)
                .options(WindowOptions { visible: false, min_size: Some((420.0, 400.0)), ..Default::default() }),
        );
        #[cfg(windows)]
        std::thread::spawn(move || {
            let apps = system::discover_apps();
            settings_app.update(move |s, _| s.installed = apps);
        });
        #[cfg(not(windows))]
        let _ = settings_app;

        // The window starts where the settings put the Island.
        let mut state = Island::new(store, screens);
        state.me = Some(island.clone());
        state.settings = Some(settings_window);
        let screen = state.screen();
        let display = displays.iter().find(|d| d.name.as_deref() == Some(screen.name.as_str())).or_else(|| displays.iter().find(|d| d.primary)).or(displays.first());
        let position = display.map(|d| {
            let (x, y) = state.placement.window;
            (d.position.0 + (x * screen.scale).round() as i32, d.position.1 + (y * screen.scale).round() as i32)
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
        vec![WindowSpec::from_app("island", app).configured_options(options)]
    });
}
