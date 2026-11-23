//! The Island: its state, the rules that move it between Still, Quick and Large, the alerts,
//! and the compact views. The Large mode's tabs are in `tabs/`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;

use rune::time::LocalTime;
use rune::{
    AppHandle, Color, Curve, Cx, DisplayTarget, Drag, Easing, Edge, Effect, GesturePhase, El, Image, ImageCache, Key, KeyEvent, LoopSpec, OverlayHandle, OverlayPlacement,
    Phase, Shadow, Shortcut, ShortcutError, ShortcutId, Spring, SurfaceFit, Transition, Tx, Vec2, Waker, WheelEvent, WindowHandle, col, el, icon, row, text,
};

use crate::config::{Config, Store, Tab};
use crate::net::Weather;
use crate::platform::{self, Passthrough, Screen};
use crate::system::{self, MediaCommand, Track};
use crate::tabs::{self, accent};
use crate::icons;

/// The Island's smallest window, in points: the largest Island at its hover scale, its tab dock, plus their shadows.
/// The window spans the display's width when that's wider, so a widened Overview grows to it.
/// No larger than 180% zoom fits on a 1920 display.
pub const CANVAS: (f32, f32) = (1060.0, 600.0);
/// Room kept around the Island inside the window for its shadow and hover scale.
const PAD: f32 = 30.0;
/// The tab dock under the Large Island: its height, the gap above it, one button and the space between.
// 28pt buttons 3pt apart leave room for every tab in the canvas (a test checks it).
const DOCK: (f32, f32, f32, f32) = (44.0, 12.0, 28.0, 3.0);
/// Room between the Large Island's edge and its tab's content.
pub const LARGE_EDGE: f32 = 12.0;
/// Framer's spring (stiffness 400, damping 40, mass 2.5) as duration and bounce.
pub const SPRING: Spring = Spring::new(0.5, 0.3);
/// Resting is the system's answer to the pointer leaving: it settles faster, with little overshoot.
const COLLAPSE: Spring = Spring::new(0.35, 0.1);
/// The Island grows this much under the pointer, past its layout rect.
const HOVER_SCALE: f32 = 1.02;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Still,
    Quick,
    Large,
}

/// In the order they go over each other: a level answers a key just pressed, and Caps Lock
/// may stay on for good.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Alert {
    Volume,
    Brightness,
    Charging,
    Battery,
    Bluetooth,
    /// A task fell due, an event starts, or a script's or agent's alert.
    Reminder,
    Camera,
    Microphone,
    CapsLock,
}

impl Alert {
    /// None draws in the text colour.
    fn color(self) -> Option<Color> {
        match self {
            Alert::Charging => Some(accent::charge()),
            Alert::Battery => Some(accent::low()),
            Alert::Bluetooth => Some(accent::bluetooth()),
            Alert::Camera => Some(accent::camera()),
            Alert::Microphone => Some(accent::mic()),
            Alert::Reminder => Some(accent::orange()),
            Alert::Volume | Alert::Brightness | Alert::CapsLock => None,
        }
    }

    fn icon(self) -> &'static rune::Icon {
        match self {
            Alert::Charging | Alert::Battery => &icons::ZAP,
            Alert::Bluetooth => &icons::HEADPHONES,
            Alert::Camera => &icons::CAMERA,
            Alert::Microphone => &icons::MIC,
            Alert::Volume => &icons::VOLUME,
            Alert::Brightness => &icons::SUN,
            Alert::CapsLock => &icons::CAPS_LOCK,
            Alert::Reminder => &icons::BELL,
        }
    }

    fn key(self) -> &'static str {
        match self {
            Alert::Charging => "charging",
            Alert::Battery => "battery",
            Alert::Bluetooth => "bluetooth",
            Alert::Camera => "camera",
            Alert::Microphone => "microphone",
            Alert::Volume => "volume",
            Alert::Brightness => "brightness",
            Alert::CapsLock => "caps-lock",
            Alert::Reminder => "reminder",
        }
    }
}

/// A live activity shown in a bubble beside the compact Island.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Bubble {
    Pomodoro,
    Download,
    CapsLock,
}

/// Room between the Island and a bubble, and between bubbles.
const BUBBLE_GAP: f32 = 8.0;
/// Seconds a finished download's tick shows.
const DOWNLOAD_DONE: f64 = 3.0;

/// The Island's own handle: its window, or on Wayland a layer-shell overlay, since Wayland
/// lets no ordinary window place or raise itself.
#[derive(Clone)]
pub enum Host {
    Window(AppHandle<Island>),
    /// The overlay, the windowless app that hosts it (for quitting), and whether it is shown.
    Overlay(OverlayHandle<Island>, AppHandle<()>, Arc<AtomicBool>),
}

impl Host {
    pub fn update(&self, f: impl FnOnce(&mut Island, &mut Tx) + Send + 'static) {
        match self { Host::Window(h) => h.update(f), Host::Overlay(h, ..) => h.update(f) }
    }

    fn update_if(&self, f: impl FnOnce(&mut Island, &mut Tx) -> bool + Send + 'static) {
        match self { Host::Window(h) => h.update_if(f), Host::Overlay(h, ..) => h.update_if(f) }
    }

    pub fn place(&self, display: DisplayTarget, placement: OverlayPlacement) {
        match self {
            Host::Window(h) => h.place(display, placement),
            Host::Overlay(h, ..) => { h.set_display(display); h.set_placement(Some(placement)) }
        }
    }

    pub fn is_visible(&self) -> bool {
        match self { Host::Window(h) => h.is_visible(), Host::Overlay(_, _, shown) => shown.load(Ordering::Relaxed) }
    }

    /// Shows the Island without taking focus.
    pub fn show(&self) {
        match self { Host::Window(h) => h.show(), Host::Overlay(h, _, shown) => { shown.store(true, Ordering::Relaxed); h.show() } }
    }

    pub fn hide(&self) {
        match self { Host::Window(h) => h.hide(), Host::Overlay(h, _, shown) => { shown.store(false, Ordering::Relaxed); h.hide() } }
    }

    pub fn quit(&self) {
        match self { Host::Window(h) => h.quit(), Host::Overlay(_, app, _) => app.quit() }
    }

    /// Runs blocking `work` off the UI thread, then `done` with its answer on it.
    pub fn spawn<T: Send + 'static>(&self, work: impl FnOnce() -> T + Send + 'static, done: impl FnOnce(&mut Island, T) + Send + 'static) {
        let me = self.clone();
        std::thread::spawn(move || { let answer = work(); me.update(move |s, _| done(s, answer)) });
    }

    /// Lets a click give the overlay keyboard focus; a window takes it on click already.
    fn allow_keys(&self) {
        if let Host::Overlay(h, ..) = self { h.request_keys() }
    }

    /// Shows the Island and gives it keyboard focus, with Wayland's token for a shortcut press.
    fn take_focus(&self, token: Option<String>) {
        match self { Host::Window(h) => h.activate(token.unwrap_or_default()), Host::Overlay(..) => { self.show(); self.allow_keys() } }
    }

    /// Registers the system-wide shortcut that opens Large Mode (`Island::hotkey`).
    pub fn register_hotkey(&self, shortcut: Shortcut) -> Result<ShortcutId, ShortcutError> {
        match self {
            Host::Window(h) => h.register_shortcut(shortcut, Island::hotkey),
            Host::Overlay(h, app, _) => {
                let h = h.clone();
                app.register_shortcut(shortcut, move |_, tx| {
                    let token = tx.activation_token().map(str::to_string);
                    h.update(move |s, tx| s.hotkey_with(token, tx));
                })
            }
        }
    }

    pub fn unregister_hotkey(&self, id: ShortcutId) {
        let done = match self { Host::Window(h) => h.unregister_shortcut(id), Host::Overlay(_, app, _) => app.unregister_shortcut(id) };
        if let Err(e) = done { eprintln!("Ripple: {e}") }
    }
}

/// Where the Island's window sits on its display and where the Island is anchored inside it.
/// All in the Island's own points: at a scale (`Config::zoom`) the display is that much smaller.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Placement {
    /// The window's top-left on the display, in points.
    pub window: (f32, f32),
    /// The Island's horizontal anchor inside the window: -1 its left edge, 0 its centre, 1 its right edge.
    pub align: i8,
    pub x: f32,
    /// True when the Island hangs from `y`; false when it stands on it.
    pub top: bool,
    pub y: f32,
    /// The zoom these points are drawn at.
    pub zoom: f32,
    /// How far down the window stays on the display: less than `CANVAS.1` on a short display.
    pub visible_h: f32,
    /// How far right the window stays on the display: less than `CANVAS.0` on a narrow display.
    pub visible_w: f32,
    /// The window's width: the display's, though never below `CANVAS.0`.
    pub canvas_w: f32,
}

impl Placement {
    pub fn of(cfg: &Config, display: (f32, f32)) -> Placement {
        // Only as large as fits the display, though never below 100%.
        let zoom = cfg.zoom().min((display.0 / CANVAS.0).min(display.1 / CANVAS.1).max(1.0));
        let (dw, dh) = (display.0 / zoom, display.1 / zoom);
        // A headless capture draws at `CANVAS`: no window to resize.
        let canvas_w = if rune::is_headless() { CANVAS.0 } else { dw.floor().max(CANVAS.0) };
        // (align, x on the display, top?, y on the display)
        let (align, x, top, y) = if cfg.notch_mode {
            (0, dw * 0.5, true, 0.0)
        } else {
            match cfg.position_mode.as_str() {
                "top-left" => (-1, 15.0, true, 15.0),
                "top-right" => (1, dw - 15.0, true, 15.0),
                "bottom-left" => (-1, 15.0, false, dh - 45.0),
                "bottom-right" => (1, dw - 15.0, false, dh - 45.0),
                "top-center" => (0, dw * 0.498, true, 20.0),
                "bottom-center" => (0, dw * 0.498, false, dh - 45.0),
                _ => (0, dw * cfg.island_x / 100.0, true, cfg.island_y / zoom),
            }
        };
        // The window stays on the display; the Island moves inside it instead.
        // Round before the clamp, and floor the limit, so a fractional zoom never pushes the window past the edge.
        let wx = match align { -1 => x - PAD, 0 => x - canvas_w / 2.0, _ => x + PAD - canvas_w }.round().min((dw - canvas_w).floor()).max(0.0);
        let wy = if top { y - PAD } else { y + PAD - CANVAS.1 }.round().min((dh - CANVAS.1).floor()).max(0.0);
        Placement { window: (wx, wy), align, x: x - wx, top, y: y - wy, zoom, visible_h: (dh - wy).min(CANVAS.1), visible_w: (dw - wx).min(canvas_w), canvas_w }
    }

    /// The tallest the Island gets with its dock and shadow still on the display.
    pub fn max_h(&self) -> f32 {
        (if self.top { self.visible_h - self.y } else { self.y }) - DOCK.0 - DOCK.1 - 8.0
    }

    /// The widest the Island gets around its anchor and still stays on the display and in the window.
    pub fn max_w(&self) -> f32 {
        (match self.align { -1 => self.visible_w - self.x, 0 => 2.0 * self.x.min(self.visible_w - self.x), _ => self.x }) - 8.0
    }

    /// The window's rect on the display, in the display's points.
    pub fn frame(&self) -> [f32; 4] {
        let k = self.zoom;
        [self.window.0 * k, self.window.1 * k, self.canvas_w * k, CANVAS.1 * k]
    }

    pub fn rect(&self, (w, h): (f32, f32)) -> [f32; 4] {
        let left = match self.align { -1 => self.x, 0 => self.x - w / 2.0, _ => self.x - w };
        [left, if self.top { self.y } else { self.y - h }, w, h]
    }
}

pub struct Island {
    pub store: Store,
    pub cfg: Config,
    pub mode: Mode,
    pub tab: Tab,
    /// The pointer is over the Island or its dock.
    pub hovered: bool,
    /// Large was opened on purpose (a click, Cmd+number, the Shelf), not resting there.
    large_by_hand: bool,
    /// Over the Island, over the dock, over a bubble, over the Island while a select's menu is open.
    over: [bool; 4],
    /// The pointer left both; the next poll lets the Island rest.
    left: bool,

    pub time: String,
    pub date: String,
    /// (year, month, day), for the Calendar tab.
    pub today: (i32, u32, u32),
    /// Minutes since midnight now, for Day Progress.
    pub minutes: u32,
    /// None on a machine without a battery.
    pub percent: Option<u8>,
    pub charging: bool,
    pub weather: Weather,
    pub track: Option<Track>,
    pub art: Option<Image>,
    /// The track's synced lyrics, once LRCLIB has them.
    pub lyrics: Option<Vec<crate::net::Lyric>>,
    pub art_hovered: bool,
    /// The bubble under the pointer: a bubble that goes away takes no hover-leave with it.
    bubble_hovered: Option<Bubble>,
    /// The marquee under the pointer, by key: it holds still so it can be read.
    pub marquee_hovered: Option<String>,
    /// What the last delete took, until the tab changes.
    pub undo: Option<tabs::Undo>,
    /// The select whose menu is open, by key. Its menu hangs outside the Island.
    pub open_select: Option<&'static str>,
    /// The pointer is over the open select's menu.
    menu_hovered: bool,
    pub background: Option<Image>,
    bluetooth: bool,
    /// The last Bluetooth poll found nothing connected; a disconnect needs two in a row.
    bluetooth_missed: bool,
    pub camera: bool,
    pub microphone: bool,
    /// Percent; None until the first reading, or where the OS doesn't say.
    volume: Option<u8>,
    brightness: Option<u8>,
    /// Caps Lock is on: its alert stays until it goes off.
    caps_lock: bool,
    /// The focused app is full screen.
    fullscreen: bool,
    /// The alerts showing for a while, each until when; `alert` picks the first in `Alert` order.
    timed: Vec<(Alert, f64)>,
    reminder: String,
    /// The reminder's meeting link, for its Join button.
    reminder_link: Option<String>,
    /// The reminder showing is `ripple notify`'s, standing for this many alerts.
    notified: Option<usize>,
    /// The hour of rain or snow already alerted (Unix seconds).
    rain_alerted: Option<i64>,
    /// Seconds until each break reminder (eye, hourly), counted only while no app is full screen.
    break_left: [Option<f64>; 2],
    /// The clock at the last count.
    break_tick: f64,
    /// The hour a water reminder last came, as (day, hour).
    water_mark: Option<(i64, u32)>,
    /// Camera and microphone alerts still to take their turn.
    capture_queue: Vec<Alert>,
    paused_until: Option<f64>,
    /// After a play/pause click: the state shown, and until when polls of the same track that
    /// still report the old state are ignored while the player catches up.
    expect_playing: Option<(bool, f64)>,

    pub finder: tabs::Finder,
    pub ai: tabs::Assistant,
    pub clipboard: tabs::Clipboard,
    pub pomodoro: tabs::Pomodoro,
    /// Files downloading now; None while none is.
    pub download: Option<crate::downloads::Progress>,
    /// Until when a finished download shows its tick.
    download_done: Option<f64>,
    pub calc: tabs::Calculator,
    pub shelf: tabs::Shelf,
    pub tasks: tabs::Tasks,
    pub habits: tabs::Habits,
    pub calendar: tabs::Calendar,
    pub notes: tabs::Notes,
    /// Nextcloud sync of Notes, Tasks and events, by `nextcloud::Kind`.
    pub nextcloud: tabs::Syncs,
    pub stremflix: tabs::Stremflix,
    /// The System tab's figures; None until the first sample.
    pub stats: Option<system::Stats>,
    pub limits: tabs::Limits,
    pub stocks: tabs::Stocks,
    pub contributions: tabs::Contributions,
    pub clocks: tabs::Clocks,
    pub controls: tabs::Controls,
    pub agents: tabs::Agents,
    pub t3: tabs::T3,
    pub plugins: crate::plugins::Plugins,
    pub dashboard: tabs::Dashboard,

    pub me: Option<Host>,
    pub settings: Option<WindowHandle>,
    /// The Settings window's state, told when the saved hotkey could not be registered.
    pub settings_app: Option<AppHandle<crate::settings::Settings>>,
    pub images: Option<ImageCache>,
    pub screens: Vec<Screen>,
    pub placement: Placement,
    passthrough: Passthrough,
    /// The Island's window has keyboard focus.
    focused: bool,
    /// The window that had focus when the pointer reached the Island; it gets focus back on close.
    prior: Option<u64>,
    weather_kick: Option<Sender<()>>,
    /// The registered shortcut that opens Large Mode.
    pub hotkey: Option<ShortcutId>,
    wheel: f32,
    wheel_reset: f64,
    /// This wheel gesture already changed tab; the rest of it, momentum included, is ignored.
    wheel_spent: bool,
    /// A phased trackpad gesture is under the fingers; a pause in it is not a new gesture.
    wheel_phased: bool,
    /// The way the last tab change went: 1 to the next tab, -1 to the previous.
    pub tab_dir: i32,
    swipe_lock_until: f64,
    /// What was typed into the dock to name a tab, and until when more letters add to it.
    type_ahead: (String, f64),
    /// The tab just changed; the next poll stamps `linger_until` from it.
    tab_changed: bool,
    /// Until when leaving waits after the last tab change (`cfg.tab_linger` from the change).
    linger_until: Option<f64>,
    /// When a lingering Island closes, unless the pointer comes back first.
    close_at: Option<f64>,
    next_second: f64,
    intro_at: Option<f64>,
}

/// The first of `tabs` whose name, or one of its words, starts with `typed` (lower case).
fn tab_named(tabs: &[Tab], typed: &str) -> Option<Tab> {
    let name = |t: &Tab| t.name().to_lowercase();
    tabs.iter().find(|t| name(t).starts_with(typed))
        .or_else(|| tabs.iter().find(|t| name(t).split([' ', '&']).any(|w| w.starts_with(typed))))
        .copied()
}

/// How wide a single line of text draws, roughly: the compact Island sizes itself to its
/// track title before anything is laid out.
pub fn estimate_width(text: &str, size: f32) -> f32 {
    text.chars().map(|c| match c {
        'i' | 'l' | 'j' | '.' | ',' | '\'' | '!' | ':' | ';' | '|' => 0.3,
        ' ' | 'f' | 't' | 'r' | '(' | ')' | '-' => 0.36,
        'm' | 'w' | 'M' | 'W' | '@' => 0.9,
        c if c.is_ascii_uppercase() => 0.68,
        c if c.is_ascii() => 0.56,
        _ => 1.0,
    }).sum::<f32>() * size
}

impl Island {
    pub fn new(store: Store, screens: Vec<Screen>) -> Island {
        let cfg = store.get();
        let mut island = Island {
            mode: Mode::Still,
            tab: cfg.default_tab,
            hovered: false,
            large_by_hand: false,
            over: [false; 4],
            left: false,
            time: String::new(),
            today: (1970, 1, 1),
            minutes: 0,
            date: String::new(),
            percent: None,
            charging: false,
            weather: Weather::default(),
            track: None,
            lyrics: None,
            art: None,
            art_hovered: false,
            bubble_hovered: None,
            marquee_hovered: None,
            undo: None,
            open_select: None,
            menu_hovered: false,
            background: None,
            bluetooth: false,
            bluetooth_missed: false,
            camera: false,
            microphone: false,
            volume: None,
            brightness: None,
            timed: Vec::new(),
            caps_lock: false,
            fullscreen: false,
            reminder: String::new(),
            reminder_link: None,
            notified: None,
            water_mark: None,
            rain_alerted: None,
            break_left: [None; 2],
            break_tick: 0.0,
            capture_queue: Vec::new(),
            paused_until: None,
            expect_playing: None,
            finder: tabs::Finder::default(),
            ai: tabs::Assistant::default(),
            clipboard: tabs::Clipboard::new(&cfg, store.dir().as_deref()),
            pomodoro: tabs::Pomodoro::default(),
            download: None,
            download_done: None,
            calc: tabs::Calculator::default(),
            shelf: tabs::Shelf::default(),
            tasks: tabs::Tasks::default(),
            habits: tabs::Habits::default(),
            calendar: tabs::Calendar::default(),
            notes: tabs::Notes::new(&cfg),
            nextcloud: Default::default(),
            stremflix: tabs::Stremflix::default(),
            stats: None,
            limits: tabs::Limits::default(),
            stocks: tabs::Stocks::default(),
            contributions: tabs::Contributions::default(),
            clocks: tabs::Clocks::default(),
            controls: tabs::Controls::default(),
            agents: tabs::Agents::default(),
            t3: tabs::T3::new(store.dir().as_deref()),
            plugins: crate::plugins::Plugins::new(store.dir().as_deref()),
            dashboard: tabs::Dashboard::default(),
            me: None,
            settings: None,
            settings_app: None,
            images: None,
            placement: Placement::of(&cfg, (1920.0, 1080.0)),
            screens,
            passthrough: Passthrough::default(),
            focused: false,
            prior: None,
            weather_kick: None,
            hotkey: None,
            wheel: 0.0,
            wheel_reset: 0.0,
            wheel_spent: false,
            wheel_phased: false,
            tab_dir: 1,
            swipe_lock_until: 0.0,
            type_ahead: (String::new(), 0.0),
            tab_changed: false,
            linger_until: None,
            close_at: None,
            next_second: 0.0,
            intro_at: None,
            store,
            cfg,
        };
        island.placement = Placement::of(&island.cfg, island.screen().size);
        island.tab = island.first_tab();
        island.mode = island.rest();
        island.set_clock();
        island
    }

    // --- Where it is ---

    /// The display the settings name, else the primary one.
    pub fn screen(&self) -> Screen {
        self.screens.iter().find(|s| s.name == self.cfg.display)
            .or_else(|| self.screens.iter().find(|s| s.primary))
            .or(self.screens.first())
            .cloned()
            .unwrap_or(Screen { name: String::new(), target: DisplayTarget::Primary, size: (1920.0, 1080.0), scale: 1.0, origin: (0.0, 0.0), position: (0, 0), primary: true })
    }

    /// Moves the window to where the settings put the Island.
    pub fn place_window(&mut self) {
        let screen = self.screen();
        self.placement = Placement::of(&self.cfg, screen.size);
        if let Some(me) = &self.me {
            let [x, y, width, height] = self.placement.frame();
            me.place(screen.target, OverlayPlacement::Rect { x, y, width, height });
            match me { Host::Window(_) => self.passthrough.resize((width, height)), Host::Overlay(h, ..) => h.resize((width, height)) }
        }
    }

    // --- What it shows ---

    pub fn playing(&self) -> bool { self.track.as_ref().is_some_and(|t| t.playing) }

    /// The compact view shows the track: while it plays, and for 3 seconds after a pause.
    fn shows_track(&self) -> bool { self.playing() || self.paused_until.is_some() }

    /// Hide when inactive, unless an app is full screen and its rule says otherwise.
    pub fn hiding(&self) -> bool {
        match self.fullscreen_rule() { "hidden" => true, "standby" | "still" => false, _ => self.cfg.hide_inactive }
    }

    fn fullscreen_rule(&self) -> &str { if self.fullscreen { &self.cfg.fullscreen } else { "none" } }

    /// Hide when inactive: resting with no track or live activity to show.
    pub fn hidden(&self) -> bool { self.hiding() && self.mode == Mode::Still && !self.shows_track() && self.activities().is_empty() }

    /// The live activities besides a track, first first: the glance shows the first of the
    /// track and these, and bubbles beside it the rest.
    fn activities(&self) -> Vec<Bubble> {
        [(self.pomodoro.active(), Bubble::Pomodoro), (self.download.is_some(), Bubble::Download), (self.caps_lock, Bubble::CapsLock)]
            .into_iter().filter_map(|(on, kind)| on.then_some(kind)).collect()
    }

    /// The label of the live activity the glance shows beside the time; Caps Lock shows as an alert instead.
    fn live(&self) -> Option<String> {
        if self.pomodoro.active() { Some(self.pomodoro.live_label()) } else { self.download.map(|d| d.label()) }
    }

    pub fn alert(&self) -> Option<Alert> {
        // Caps Lock is an alert when it is the only live activity; otherwise it takes a bubble.
        let caps = !self.shows_track() && self.activities() == [Bubble::CapsLock];
        self.timed.iter().map(|t| t.0).min().or(caps.then_some(Alert::CapsLock))
    }

    /// Until when `alert` shows; None when it isn't.
    fn shown_until(&self, alert: Alert) -> Option<f64> { self.timed.iter().find(|t| t.0 == alert).map(|t| t.1) }

    /// Shows `alert` until `until`, in place of `replaces` (itself included) if one shows.
    fn show_alert(&mut self, alert: Alert, until: f64, replaces: &[Alert]) {
        self.timed.retain(|t| t.0 != alert && !replaces.contains(&t.0));
        self.timed.push((alert, until));
    }

    /// A reminder shows: other alerts of its kind wait their turn.
    fn reminding(&self) -> bool { self.shown_until(Alert::Reminder).is_some() }

    /// Live activities the compact Island has no room for, each in a circle of its own beside it:
    /// away from the display edge the Island hugs, or taking turns right then left when centred.
    pub fn bubbles(&self) -> Vec<(Bubble, [f32; 4])> {
        if self.mode == Mode::Large { return Vec::new() }
        let kinds = self.activities().into_iter().skip(usize::from(!self.shows_track()));
        let [x, y, w, h] = self.rect();
        let (mut right, mut left) = (x + w, x);
        kinds.enumerate().map(|(i, kind)| {
            let want_right = match self.placement.align { -1 => true, 1 => false, _ => i % 2 == 0 };
            // A side with no room left in the window gives way to the other.
            let (fits_right, fits_left) = (right + BUBBLE_GAP + h <= self.placement.canvas_w, left - BUBBLE_GAP - h >= 0.0);
            let x = if if want_right { fits_right || !fits_left } else { fits_right && !fits_left } {
                right += BUBBLE_GAP + h;
                right - h
            } else {
                left -= BUBBLE_GAP + h;
                left
            };
            (kind, [x, y, h, h])
        }).collect()
    }

    /// The alert showing is an event's, with a meeting to join.
    fn joinable(&self) -> bool { self.alert() == Some(Alert::Reminder) && self.reminder_link.is_some() }

    /// The Island colour at the opacity set for the mode it is in.
    fn island_bg(&self) -> Color {
        let bg = self.cfg.bg();
        bg.alpha(bg.a * if self.mode == Mode::Large { self.cfg.large_opacity } else { self.cfg.compact_opacity } / 100.0)
    }

    pub fn low_battery(&self) -> bool { !self.charging && self.percent.is_some_and(|p| p < 20) }

    fn percent_label(&self) -> String { self.percent.map_or(String::new(), |p| format!("{p}%")) }

    /// The text beside an alert's icon.
    fn alert_label(&self, alert: Alert) -> String {
        match alert {
            Alert::Charging | Alert::Battery => self.percent_label(),
            // It shows on connect and on disconnect.
            Alert::Bluetooth => if self.bluetooth { "Connected" } else { "Disconnected" }.into(),
            Alert::Camera => "Camera".into(),
            Alert::Microphone => "Microphone".into(),
            Alert::Volume | Alert::Brightness => String::new(),
            Alert::CapsLock => "Caps Lock".into(),
            // Fits the alert's width.
            Alert::Reminder => {
                let title = self.reminder.as_str();
                if title.chars().count() > 13 { format!("{}…", title.chars().take(12).collect::<String>()) } else { title.to_string() }
            }
        }
    }

    fn now_playing_text(&self) -> (String, String) {
        let Some(t) = &self.track else { return Default::default() };
        (t.name.clone(), if t.artist.is_empty() { String::new() } else { format!(" • {}", t.artist) })
    }

    fn now_playing_width(&self) -> f32 {
        let (name, artist) = self.now_playing_text();
        // 2 points of slack, so a title that fits doesn't scroll.
        (estimate_width(&name, 13.0) + estimate_width(&artist, 13.0) + PLAYING_CHROME + 2.0).ceil().clamp(122.0, 300.0)
    }

    /// True when the Island must clear a physical notch (Notch Mode, not Mini).
    fn clears_notch(&self) -> bool { self.cfg.notch_mode && !self.cfg.notch_mini }

    pub fn notch_inset(&self) -> f32 { if self.clears_notch() { 34.0 } else { 0.0 } }

    /// Notch Mode's compact width: the notch, plus the wider half of the glance and its padding on
    /// each side, so neither half sits under the notch. Mirrors `glance`.
    fn notch_glance_width(&self) -> f32 {
        if !self.cfg.notch_auto_size { return 300.0 }
        const NOTCH: f32 = 200.0; // a MacBook notch is about 180-190pt wide; sized for the widest plus slack
        let label = |l: &str| estimate_width(l, 16.0);
        let percent = || label(&self.percent_label());
        let (left, right) = match self.alert() {
            Some(alert) => (20.0, label(&self.alert_label(alert))),
            None if let Some(live) = self.live() => (label(&self.time), 20.0 + label(&live)),
            None => (label(&self.time), match self.weather.temp {
                Some(temp) if !(self.cfg.standby && self.percent.is_some()) => 16.0 + label(&format!("{temp}º")),
                _ if self.charging && self.percent.is_some() => 16.0 + percent(),
                _ => percent(),
            }),
        };
        (NOTCH + 2.0 * (left.max(right) + 12.0)).ceil().max(210.0)
    }

    pub fn size(&self) -> (f32, f32) {
        let notch = self.clears_notch();
        let alerting = self.alert().is_some();
        let track = self.shows_track();
        let large = tabs::size(self.tab);
        let mut w = match self.mode {
            Mode::Large if self.tab == Tab::Overview => tabs::overview_width(self),
            Mode::Large => large.0,
            _ if matches!(self.alert(), Some(Alert::Volume | Alert::Brightness)) && !notch => 100.0, // just the ring
            _ if alerting => if notch { self.notch_glance_width() + if self.joinable() { 70.0 } else { 0.0 } } else if self.joinable() { 246.0 } else { 176.0 }, // fits icon + "Disconnected"
            _ if track => self.now_playing_width(),
            _ if notch => if self.mode == Mode::Quick || self.cfg.show_info_idle || self.live().is_some() { self.notch_glance_width() } else { 210.0 },
            _ if let Some(live) = self.live() => (110.0 + estimate_width(&live, 15.0)).ceil().max(168.0), // "23:59" + ring + "24:59"
            Mode::Quick | Mode::Still => 140.0, // fits "23:59" + weather/"100%"
        };
        if notch && track && !alerting && self.mode != Mode::Large { w = (w + 40.0).max(300.0) }
        let h = match self.mode {
            // The Overview fits its tiles.
            Mode::Large if self.tab == Tab::Overview => ((self.notch_inset() - LARGE_EDGE).max(0.0) + 2.0 * LARGE_EDGE + tabs::overview_height(self)).min(self.placement.max_h().max(large.1)),
            Mode::Large => (self.notch_inset() - LARGE_EDGE).max(0.0) + large.1,
            _ => if self.cfg.notch_mode { 34.0 } else { 40.0 }, // Mini notch keeps the notch height
        };
        (w, h)
    }

    pub fn rect(&self) -> [f32; 4] { self.placement.rect(self.size()) }

    /// The tab dock's rect, the gap to the Island included: under the Large Island, or over it
    /// when the Island stands on the bottom of the display.
    fn dock_rect(&self) -> Option<[f32; 4]> {
        if self.mode != Mode::Large { return None }
        let (h, gap, button, space) = DOCK;
        let n = self.visible_tabs().len() as f32;
        let w = n * button + (n - 1.0) * space + 2.0 * space;
        let [x, y, iw, ih] = self.rect();
        // Not clamp: a dock wider than the canvas would panic it.
        let left = (x + iw / 2.0 - w / 2.0).min(self.placement.canvas_w - w - 4.0).max(4.0);
        Some([left, if self.placement.top { y + ih } else { y - gap - h }, w, gap + h])
    }

    // --- Tabs ---

    /// The dock's tabs: the saved order without the hidden ones and those with nothing to show.
    pub fn visible_tabs(&self) -> Vec<Tab> {
        self.cfg.tab_order.iter().copied().filter(|t| !self.cfg.hidden_tabs.contains(t) && tabs::has_content(self, *t)).collect()
    }

    fn first_tab(&self) -> Tab {
        let visible = self.visible_tabs();
        if visible.contains(&self.cfg.default_tab) { self.cfg.default_tab } else { visible.first().copied().unwrap_or(Tab::Search) }
    }

    // --- Modes ---

    /// Where the Island rests when nothing holds it open.
    pub fn rest(&self) -> Mode {
        // Standby is on by default, so the opt-in settings win over it or they could never apply.
        // Hiding wins over both, since it only works at Still. A full screen rule wins over all.
        match self.fullscreen_rule() { "standby" => return Mode::Quick, "still" => return Mode::Still, _ => {} }
        if self.hiding() { Mode::Still } else if self.cfg.large_standby { Mode::Large } else if self.cfg.standby { Mode::Quick } else { Mode::Still }
    }

    /// Every mode change but a jump straight to a tab (Cmd+number, the Shelf) goes through here;
    /// Still means wherever the Island rests.
    pub fn set_mode(&mut self, mode: Mode) {
        let was = self.mode;
        self.mode = if mode == Mode::Still { self.rest() } else { mode };
        self.large_by_hand = mode == Mode::Large;
        if self.mode == Mode::Large && was != Mode::Large {
            if self.cfg.open_default_tab { self.tab = self.first_tab() }
            // An agent waiting on an answer goes over the default tab.
            if !self.agents.requests.is_empty() && self.visible_tabs().contains(&Tab::Agents) { self.tab = Tab::Agents }
            self.entered_tab();
        }
        // Resting closes an open select even when the Island rests in Large.
        if mode == Mode::Still || self.mode != Mode::Large { self.open_select = None }
        if self.mode != Mode::Large { self.tab_changed = false; self.linger_until = None; self.close_at = None }
    }

    pub fn hover(&mut self, hovered: bool, tx: &mut Tx) { self.set_over(0, hovered, tx) }

    fn hover_dock(&mut self, hovered: bool, tx: &mut Tx) { self.set_over(1, hovered, tx) }

    /// Moving from the Island to its dock leaves one before entering the other: the Island
    /// rests only if the pointer is over neither by the next poll.
    fn set_over(&mut self, part: usize, inside: bool, tx: &mut Tx) {
        let was = self.hovered;
        self.over[part] = inside;
        self.hovered = self.over.iter().any(|o| *o);
        if self.hovered && !was {
            self.close_at = None;
            // Each show makes a new layer surface, so this is asked again on every entry.
            if let Some(me) = &self.me { me.allow_keys() }
            self.remember_prior(|_| {});
            // Not from a bubble: Large has none, so the hover would vanish and close it again.
            if self.mode == Mode::Still && self.cfg.show_info_idle && !self.playing() && part != 2 { self.set_mode(Mode::Large) }
            else if self.mode != Mode::Large { self.set_mode(Mode::Quick) }
        } else if !self.hovered {
            self.art_hovered = false;
            self.marquee_hovered = None;
            self.left = true;
            tx.wake_at(tx.clock());
        }
    }

    /// The pointer stayed off the Island and its dock.
    fn leave(&mut self, tx: &mut Tx) -> bool {
        if self.hovered { return false }
        let now = tx.clock();
        let left = std::mem::take(&mut self.left);
        if left && self.mode == Mode::Large && let Some(at) = self.linger_until && now < at {
            self.close_at = Some(at);
            tx.wake_at(at);
            return false;
        }
        if !left && !self.close_at.is_some_and(|t| now >= t) { return false }
        self.close_at = None;
        // An open select's menu hangs outside the Island; leaving for it isn't leaving (poll closes both).
        if self.open_select.is_none() {
            tx.blur();
            self.set_mode(Mode::Still);
            self.give_focus_back();
        }
        true
    }

    /// A click on a bubble opens Large, on the activity's tab when it has a visible one.
    fn open_bubble(&mut self, kind: Bubble, tx: &mut Tx) {
        tx.blur();
        self.set_mode(Mode::Large);
        if kind == Bubble::Pomodoro && self.visible_tabs().contains(&Tab::Pomodoro) {
            self.set_tab(Tab::Pomodoro);
        }
        // The bubble is gone in Large, so its hover is too; the Island closes unless the pointer reaches it.
        self.bubble_hovered = None;
        self.over[2] = false;
        self.hovered = self.over.iter().any(|o| *o);
        if !self.hovered {
            let at = tx.clock() + 3.0;
            self.close_at = Some(at);
            tx.wake_at(at);
        }
    }

    /// A click on the Island itself: Large opens and closes.
    pub fn toggle(&mut self, tx: &mut Tx) {
        tx.blur();
        let closing = self.mode == Mode::Large;
        self.set_mode(if closing { Mode::Quick } else { Mode::Large });
        if closing { self.give_focus_back() }
    }

    /// The system-wide shortcut: opens Large Mode with keyboard focus, or closes it again.
    pub fn hotkey(&mut self, tx: &mut Tx) {
        let token = tx.activation_token().map(str::to_string);
        self.hotkey_with(token, tx)
    }

    fn hotkey_with(&mut self, token: Option<String>, tx: &mut Tx) {
        tx.blur();
        // Resting in Large (Large standby) is not open: the hotkey opens it by hand and takes focus.
        if self.large_open() {
            self.set_mode(if self.hovered { Mode::Quick } else { Mode::Still });
            return self.give_focus_back();
        }
        self.close_at = None;
        self.set_mode(Mode::Large);
        self.remember_prior(move |s| if s.mode == Mode::Large && let Some(me) = &s.me { me.take_focus(token) });
    }

    /// Remembers the window that has focus, to hand it back on close; `then` runs once it is
    /// known. GNOME Wayland asks Window Calls off the UI thread, as focus would name the Island.
    fn remember_prior(&mut self, then: impl FnOnce(&mut Island) + Send + 'static) {
        if !self.focused && matches!(self.me, Some(Host::Window(_))) { self.prior = platform::focused_window() }
        #[cfg(target_os = "linux")]
        if !self.focused && self.prior.is_none() && let Some(me @ Host::Window(_)) = self.me.clone() {
            return me.spawn(platform::gnome_focused_window, move |s, w| { if w.is_some() { s.prior = w } then(s) });
        }
        then(self)
    }

    /// Hands keyboard focus back to the window that had it before the Island took it.
    fn give_focus_back(&mut self) {
        if !self.focused { return }
        if let Some(window) = self.prior { platform::focus_window(window) }
        #[cfg(target_os = "linux")]
        if self.prior.is_none() { platform::focus_previous() }
        // Given away; the focus event may lag, and a second close mustn't steal it back.
        self.focused = false;
    }

    pub fn window_focus(&mut self, focused: bool, tx: &mut Tx) {
        self.focused = focused;
        if focused || self.hovered { return }
        tx.blur();
        self.set_mode(Mode::Still);
    }

    /// Switches to `tab`, sliding the way it lies from the current one, and loads what it shows.
    pub fn set_tab(&mut self, tab: Tab) {
        let visible = self.visible_tabs();
        let at = |t: Tab| visible.iter().position(|v| *v == t);
        if let (Some(from), Some(to)) = (at(self.tab), at(tab))
            && from != to
        {
            self.tab_dir = if to > from { 1 } else { -1 };
        }
        self.tab = tab;
        self.entered_tab();
    }

    pub fn move_tab(&mut self, direction: i32) {
        let visible = self.visible_tabs();
        let Some(i) = visible.iter().position(|t| *t == self.tab) else { return };
        let next = i as i32 + direction;
        if next < 0 || next >= visible.len() as i32 { return }
        self.open_tab(visible[next as usize]);
    }

    /// The user's tab change: `set_tab`, and it earns the linger.
    fn open_tab(&mut self, tab: Tab) {
        if self.tab != tab && self.mode == Mode::Large { self.tab_changed = true }
        self.set_tab(tab);
    }

    fn entered_tab(&mut self) {
        self.open_select = None;
        self.undo = None;
        if self.mode == Mode::Large { tabs::entered(self) }
    }

    // --- Shelf ---

    /// Opens Large straight on `tab`, the starred tab aside; a pending close no longer applies.
    fn open_by_hand(&mut self, tab: Tab) {
        self.close_at = None;
        self.mode = Mode::Large;
        self.large_by_hand = true;
        self.open_tab(tab);
    }

    /// A file drag over the Island opens the Shelf; one that leaves without dropping lets it rest.
    fn drop_hover(&mut self, over: bool, _: &mut Tx) {
        self.shelf.dropping = over;
        if over {
            self.open_by_hand(Tab::Shelf);
        } else if !self.hovered && !self.shelf.desktop_drag {
            self.set_mode(Mode::Still);
        }
    }

    /// A drag starting anywhere on the desktop opens the Shelf, ready for the drop. When it ends
    /// it rests, even under the pointer, unless files are over the Island: then the drop or the
    /// drag leaving decides.
    pub fn desktop_drag(&mut self, active: bool) {
        if active {
            if !self.visible_tabs().contains(&Tab::Shelf) { return }
            self.shelf.desktop_drag = true;
            self.open_by_hand(Tab::Shelf);
        } else if std::mem::take(&mut self.shelf.desktop_drag) && !self.shelf.dropping {
            self.set_mode(Mode::Still);
        }
    }

    /// Dropped files go on top of the Shelf; one already there moves up.
    pub fn drop_files(&mut self, files: &[std::path::PathBuf], _: &mut Tx) {
        self.save(|c| {
            c.shelf.retain(|f| !files.contains(f));
            c.shelf.splice(0..0, files.iter().cloned());
        });
        self.shelf_new_files(files);
        self.shelf.dropping = false;
        self.shelf.desktop_drag = false;
        self.open_by_hand(Tab::Shelf);
    }

    pub fn key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        match ev.key {
            Key::Right => self.move_tab(1),
            Key::Left => self.move_tab(-1),
            Key::Command(c @ '1'..='9') => {
                let visible = self.visible_tabs();
                // 9 is the last tab, however many there are, as in browsers.
                let tab = if c == '9' { visible.last() } else { visible.get(c as usize - '1' as usize) };
                let Some(&tab) = tab else { return false };
                self.open_by_hand(tab);
            }
            _ => return false,
        }
        // Keyboard tab changes land at once: they repeat too often to animate.
        tx.instant();
        true
    }

    /// Typing while a dock button has focus jumps to the tab of that name; letters typed within
    /// a second of each other add up ("ca" picks Calendar over Clipboard).
    fn dock_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        let Key::Text(typed) = &ev.key else { return false };
        let now = tx.clock();
        if now > self.type_ahead.1 { self.type_ahead.0.clear() }
        // A Space that starts nothing presses the focused button, as it always did.
        if ev.mods.cmd || ev.mods.ctrl || ev.mods.alt || (typed.trim().is_empty() && self.type_ahead.0.is_empty()) { return false }
        self.type_ahead = (self.type_ahead.0.clone() + &typed.to_lowercase(), now + 1.0);
        if let Some(tab) = tab_named(&self.visible_tabs(), &self.type_ahead.0) {
            self.open_tab(tab);
            tx.instant();
            tx.focus(format!("dock-{}", tab as u8), true);
        }
        true
    }

    /// A horizontal wheel or trackpad swipe over the Large Island changes tab.
    pub fn wheel(&mut self, e: WheelEvent, tx: &mut Tx) -> bool {
        let now = tx.clock();
        if self.mode != Mode::Large { return false }
        // A fresh touch starts a new gesture. A pause in a trackpad gesture does not: the
        // fingers are still down, so the rest of that swipe stays spent. A wheel that reports
        // no phases has no touch, so an idle gap starts its next run instead.
        let touch = e.phase == GesturePhase::Began && !e.momentum;
        if touch || (!self.wheel_phased && !e.momentum && now > self.wheel_reset) {
            self.wheel = 0.0;
            self.wheel_spent = false;
        }
        if touch { self.wheel_phased = true }
        if e.phase == GesturePhase::Ended { self.wheel_phased = false }
        self.wheel_reset = now + 0.2;
        if e.dy.abs() > e.dx.abs() { return false }
        if self.wheel_spent { return true }
        // Content moving left (negative dx) is a swipe to the next tab.
        self.wheel -= e.dx;
        if self.wheel.abs() >= 60.0 {
            let direction = if self.wheel > 0.0 { 1 } else { -1 };
            self.wheel = 0.0;
            self.wheel_spent = true;
            self.move_tab(direction);
        }
        true
    }

    /// A press on the Island's own surface: a short one toggles Large, a sideways drag of 60
    /// points or more changes tab.
    pub fn drag(&mut self, d: &mut Drag, tx: &mut Tx) {
        if d.phase != Phase::Up || d.cancelled() { return }
        if d.dx.abs() <= 8.0 && d.dy.abs() <= 8.0 { return self.toggle(tx) }
        if self.mode != Mode::Large || tx.clock() < self.swipe_lock_until { return }
        if d.dx.abs() < 60.0 || d.dx.abs() <= d.dy.abs() { return }
        self.swipe_lock_until = tx.clock() + 0.8;
        self.move_tab(if d.dx > 0.0 { -1 } else { 1 });
    }

    // --- Settings ---

    /// Picks up settings another window changed.
    pub fn reload(&mut self) {
        let old = std::mem::replace(&mut self.cfg, self.store.get());
        if self.mode != Mode::Large || !self.hovered { self.set_mode(Mode::Still) }
        if !self.visible_tabs().contains(&self.tab) {
            self.set_tab(self.first_tab());
        }
        let size = self.screen().size;
        if old.display != self.cfg.display || Placement::of(&old, size) != Placement::of(&self.cfg, size) { self.place_window() }
        if (old.location != self.cfg.location || old.weather_unit != self.cfg.weather_unit)
            && let Some(kick) = &self.weather_kick
        {
            let _ = kick.send(());
        }
        if old.bg_image != self.cfg.bg_image { self.load_background() }
        if old.hour12 != self.cfg.hour12 { self.set_clock() }
        if old.hide_from_capture != self.cfg.hide_from_capture && let Some(w) = self.passthrough.window() { platform::hide_from_capture(w, self.cfg.hide_from_capture) }
        if !old.lyrics && self.cfg.lyrics { self.fetch_lyrics() }
        if old.overlap_panels != self.cfg.overlap_panels && let Some(Host::Overlay(h, ..)) = &self.me { h.set_overlap_panels(self.cfg.overlap_panels) }
        tabs::reloaded(self, &old);
    }

    /// Changes the settings from the Island itself (tasks, the T3 choices).
    pub fn save(&mut self, f: impl FnOnce(&mut Config)) {
        self.cfg = self.store.update(f);
    }

    fn load_background(&mut self) {
        self.background = None;
        let url = self.cfg.bg_image.trim().to_string();
        let Some(me) = self.me.clone() else { return };
        if url.is_empty() || url == "none" { return }
        std::thread::spawn(move || fetch_image(&me, url.clone(), false, move |s, image| if s.cfg.bg_image.trim() == url { s.background = Some(image) }));
    }

    // --- The clock ---

    fn set_clock(&mut self) {
        let ms = crate::date::now_ms();
        // A headless capture reads the same clock on every machine.
        let now = if rune::is_headless() { LocalTime::at_offset(1_767_260_460_000, 0) } else { LocalTime::at(ms) };
        self.time = crate::date::clock(now.hour, now.minute, self.cfg.hour12);
        self.date = crate::date::format_date(now.year, now.month, now.day);
        self.today = (now.year, now.month, now.day);
        self.minutes = now.hour * 60 + now.minute;
    }

    // --- What the pollers report ---

    pub fn remind(&mut self, title: String, now: f64) { self.remind_for(title, None, now + 6.0) }

    fn remind_for(&mut self, title: String, link: Option<String>, until: f64) {
        self.show_alert(Alert::Reminder, until, &[]);
        (self.reminder, self.reminder_link, self.notified) = (title, link, None);
        if !self.large_open() { self.set_mode(Mode::Quick) }
    }

    /// Non-urgent alerts (water, breaks, rain, events, downloads, agents, `ripple notify`) wait:
    /// it's quiet hours, or an app is full screen and that is set to quiet too. Low battery shows.
    pub fn quiet(&self) -> bool {
        (self.cfg.quiet_fullscreen && self.fullscreen) || (self.cfg.quiet_hours && self.cfg.quiet_at(LocalTime::at(crate::date::now_ms()).hour))
    }

    /// A reminder may show now: Large isn't open and none shows, which it would replace; one
    /// held back stays due until then. Headless runs show none, so every machine renders the same.
    pub fn can_remind(&self) -> bool { !rune::is_headless() && !self.large_open() && !self.reminding() }

    /// Large is open by hand or under the pointer, not merely resting there (large_standby).
    pub(crate) fn large_open(&self) -> bool {
        self.mode == Mode::Large && (self.large_by_hand || self.hovered)
    }

    /// Once a second: the 20-20-20 eye break and the hourly break, paused while an app is full
    /// screen. One due while Large is open waits until it closes.
    fn break_reminders(&mut self, now: f64) {
        let elapsed = now - std::mem::replace(&mut self.break_tick, now);
        let on = [self.cfg.eye_breaks, self.cfg.hourly_breaks];
        let paused = self.fullscreen || !self.can_remind() || self.quiet();
        if let Some(i) = count_breaks(&mut self.break_left, on, elapsed, paused) {
            self.remind(["Look away 20s", "Take a break"][i].into(), now);
        }
    }

    /// On the hour within the set hours, while the Water widget is on the Overview and today's
    /// glasses are short of the goal.
    pub fn water_reminders(&mut self, now: f64) {
        if !self.can_remind() || self.quiet() { return }
        let t = LocalTime::at(crate::date::now_ms());
        let mark = (crate::date::days_from_civil(t.year, t.month, t.day), t.hour);
        // Track the hour even while reminders are off, so turning them on or adding the widget mid-hour stays quiet.
        if !hour_turned(&mut self.water_mark, mark) || !self.cfg.water_reminders || !self.cfg.on_dashboard(crate::config::Widget::Water) { return }
        if water_due(&self.cfg, mark) { self.remind("Drink water".into(), now) }
    }

    /// "Rain at 2:00": when the coming rain or snow starts, on the user's clock.
    pub fn precip_label(&self) -> String {
        let Some((at, snow)) = self.weather.precip else { return String::new() };
        let t = LocalTime::at(at * 1000);
        format!("{} at {}", if snow { "Snow" } else { "Rain" }, crate::date::clock(t.hour, t.minute, self.cfg.hour12))
    }

    /// Once a second: an alert once rain or snow is due within the hour; Large waits for compact.
    fn rain_alert(&mut self, now: f64) {
        // `rain_alerted` stays through a pause in the rain: `rain_due` lets only rain 3 hours away from it alert again.
        let Some((at, _)) = self.weather.precip else { return };
        if !self.cfg.rain_alerts || !self.can_remind() || self.quiet() { return }
        if !rain_due(at, crate::date::now_secs(), self.rain_alerted) { return }
        self.rain_alerted = Some(at);
        self.remind(self.precip_label(), now);
    }

    /// An event's alert: 30 seconds, so its Join button can be reached.
    pub fn remind_event(&mut self, title: String, link: Option<String>, now: f64) {
        self.remind_for(title, link, now + 30.0);
    }

    /// The alert's Join button: opens the meeting and ends the alert.
    fn join(&mut self) {
        if let Some(link) = self.reminder_link.take() { system::open_external(&link) }
        self.timed.retain(|t| t.0 != Alert::Reminder);
        self.set_mode(Mode::Still);
    }

    /// Once a second: alerts that `ripple notify` queued; several at once, or while one shows, share one.
    pub fn notifications(&mut self, now: f64) {
        let Some(dir) = self.store.dir() else { return };
        // An open Large draws no alerts: they stay queued until the Island is compact (`take` drops ones over 5 minutes old).
        // Resting in Large (Large standby) isn't open: `remind` shows the alert compact.
        if self.large_open() || self.quiet() { return }
        // An alert of ours showing counts as the alerts it merged; another reminder isn't replaced, so they wait.
        // One that ends this tick no longer shows: the poll after this second sweeps it.
        let showing = self.shown_until(Alert::Reminder).is_some_and(|until| now < until);
        let shown = match self.notified { _ if !showing => 0, Some(n) => n, None => return };
        if let Some((text, n)) = crate::notify::merge(shown, crate::notify::take(&dir)) {
            self.remind(text, now);
            self.notified = Some(n);
        }
    }

    fn set_battery(&mut self, battery: Option<(u8, bool)>, tx: &mut Tx) -> bool {
        let (percent, charging) = battery.map_or((None, false), |(p, c)| (Some(p), c));
        if (percent, charging) == (self.percent, self.charging) { return false }
        let now = tx.clock();
        if charging && !self.charging && self.cfg.battery_alerts {
            self.set_mode(Mode::Quick);
            self.show_alert(Alert::Charging, now + 1.5, &[]);
        }
        if percent != self.percent && percent.is_some_and(|p| [20, 15, 10, 5, 3].contains(&p)) && self.cfg.battery_alerts {
            self.set_mode(Mode::Quick);
            self.show_alert(Alert::Battery, now + 3.0, &[]);
        }
        (self.percent, self.charging) = (percent, charging);
        self.schedule(tx);
        true
    }

    pub fn set_bluetooth(&mut self, connected: bool, tx: &mut Tx) -> bool {
        // A failed query reads as "nothing connected": one miss alone is not a disconnect.
        let missed = std::mem::replace(&mut self.bluetooth_missed, !connected);
        if connected == self.bluetooth || (!connected && !missed) { return false }
        self.bluetooth = connected;
        self.set_mode(Mode::Quick);
        self.show_alert(Alert::Bluetooth, tx.clock() + 3.0, &[]);
        self.schedule(tx);
        true
    }

    /// A volume or brightness change shows its ring for 1.5 s, from the latest change. The first
    /// reading only sets the level. Large stays as it is: the ring shows only when compact.
    pub fn set_levels(&mut self, volume: Option<u8>, brightness: Option<u8>, tx: &mut Tx) -> bool {
        let mut moved = None;
        for (alert, new, old) in [(Alert::Volume, volume, &mut self.volume), (Alert::Brightness, brightness, &mut self.brightness)] {
            // A failed read keeps the last level, so the next change still shows.
            if new.is_none() || new == *old { continue }
            if old.is_some() && new.is_some() { moved = Some(alert) }
            *old = new;
        }
        let Some(alert) = moved else { return false };
        // A poll's commit lands at once otherwise: the Island would snap open with no ring entrance.
        tx.animated();
        if self.mode != Mode::Large { self.set_mode(Mode::Quick) }
        self.show_alert(alert, tx.clock() + 1.5, &[Alert::Volume, Alert::Brightness]);
        self.schedule(tx);
        true
    }

    /// The full screen rule applies to a resting Island; one held open stays until it rests.
    fn set_fullscreen(&mut self, on: bool) -> bool {
        // Hovered or focused, the focused-window check may read Ripple itself; keep the last reading.
        if on == self.fullscreen || self.hovered || self.focused { return false }
        let before = self.rest();
        let resting = self.mode == before && !self.large_by_hand;
        self.fullscreen = on;
        // Only a rule that moves the rest moves the Island: a Large opened by hand stays so.
        if resting && self.rest() != before { self.set_mode(Mode::Still) }
        true
    }

    /// A download whose file is there (`Progress::done`) stays a moment, full, with a tick.
    /// Quiet hours hold downloads back.
    pub fn set_download(&mut self, download: Option<crate::downloads::Progress>, tx: &mut Tx) -> bool {
        let download = download.filter(|_| !self.quiet());
        if download.is_none() && self.download_done.is_some() { return false }
        if let Some(d) = download.filter(|d| d.done) {
            // One the Island never showed running ends quietly too.
            if self.download.is_none() { return false }
            self.download = Some(d);
            let at = tx.clock() + DOWNLOAD_DONE;
            self.download_done = Some(at);
            tx.wake_at(at);
            return true
        }
        if download.is_some() { self.download_done = None }
        self.download != download && { self.download = download; true }
    }

    /// Caps Lock's alert shows from the moment it goes on until it goes off; Large stays as it is.
    pub fn set_caps_lock(&mut self, on: bool) -> bool {
        if on == self.caps_lock { return false }
        self.caps_lock = on;
        // Turning it off leaves a hovered Island or another alert as it is.
        if on && self.mode != Mode::Large { self.set_mode(Mode::Quick) }
        else if !on && self.mode == Mode::Quick && !self.hovered && self.alert().is_none() { self.set_mode(Mode::Still) }
        true
    }

    /// Camera and microphone alerts take turns, three seconds each.
    pub fn set_capture(&mut self, camera: Option<bool>, microphone: Option<bool>, tx: &mut Tx) -> bool {
        let (camera, microphone) = (camera.unwrap_or(self.camera), microphone.unwrap_or(self.microphone));
        if (camera, microphone) == (self.camera, self.microphone) { return false }
        for (alert, in_use, was) in [(Alert::Camera, camera, self.camera), (Alert::Microphone, microphone, self.microphone)] {
            if in_use && !was { self.capture_queue.push(alert) }
        }
        (self.camera, self.microphone) = (camera, microphone);
        self.capture_queue.retain(|a| if *a == Alert::Camera { camera } else { microphone });
        self.next_capture(tx);
        true
    }

    fn capturing(&self) -> bool { self.timed.iter().any(|t| matches!(t.0, Alert::Camera | Alert::Microphone)) }

    fn next_capture(&mut self, tx: &mut Tx) {
        if self.capturing() || self.capture_queue.is_empty() { return }
        self.set_mode(Mode::Quick);
        let alert = self.capture_queue.remove(0);
        self.show_alert(alert, tx.clock() + 3.0, &[]);
        self.schedule(tx);
    }

    /// Drops the lyrics and looks up the current track's, when the setting is on and the length known.
    fn fetch_lyrics(&mut self) {
        self.lyrics = None;
        let Some(t) = self.track.as_ref().filter(|t| self.cfg.lyrics && !t.name.trim().is_empty()) else { return };
        let (Some((_, length)), Some(me)) = (t.progress, self.me.clone()) else { return };
        let (name, artist) = (t.name.clone(), t.artist.clone());
        std::thread::spawn(move || {
            let Some(lines) = crate::net::lyrics(&artist, &name, length) else { return };
            // The track may have changed while the lyrics loaded.
            me.update(move |s, _| if s.track.as_ref().is_some_and(|t| t.name == name && t.artist == artist) { s.lyrics = Some(lines) });
        });
    }

    /// A polled track, unless it is the old state of a play/pause the player has not applied yet.
    fn poll_track(&mut self, track: Option<Track>, tx: &mut Tx) -> bool {
        if let (Some((want, until)), Some(old), Some(new)) = (self.expect_playing, &self.track, &track) {
            let same = old.name == new.name && old.artist == new.artist && old.source == new.source;
            if same && new.playing != want && tx.clock() < until { return false }
        }
        self.expect_playing = None;
        self.set_track(track, tx)
    }

    pub fn set_track(&mut self, track: Option<Track>, tx: &mut Tx) -> bool {
        if track == self.track { return false }
        // A poll that only played on a few seconds: the bar runs at playback speed until the next one.
        // Polls land at once (no input caused them), so this one asks for motion; seeks still jump.
        if let (Some(old), Some(new)) = (&self.track, &track)
            && let (Some((from, _)), Some((to, _))) = (old.progress, new.progress)
            && (1..=4).contains(&to.saturating_sub(from))
            && (Track { progress: new.progress, ..old.clone() }) == *new
        {
            tx.animated();
            tx.motion_curve(Curve::tween(2.0, Easing::LINEAR));
        }
        let was_paused = self.track.as_ref().is_some_and(|t| t.paused);
        let paused = track.as_ref().is_some_and(|t| t.paused);
        if paused && !was_paused {
            self.paused_until = Some(tx.clock() + 3.0);
            self.schedule(tx);
        } else if !paused {
            self.paused_until = None;
        }
        // A length that arrives after the title counts as a change: the lookup needs it.
        let song = |t: &Option<Track>| t.as_ref().map(|t| (t.name.clone(), t.artist.clone(), t.progress.is_some()));
        let new_song = song(&self.track) != song(&track);
        let art_url = |t: &Option<Track>| t.as_ref().and_then(|t| t.artwork_url.clone());
        let (old_art, new_art) = (art_url(&self.track), art_url(&track));
        self.track = track;
        if new_song { self.fetch_lyrics() }
        if old_art != new_art {
            self.art = None;
            if let (Some(url), Some(me)) = (new_art, self.me.clone()) {
                std::thread::spawn(move || fetch_image(&me, url.clone(), true, move |s, image| {
                    // The track may have changed while the cover loaded.
                    if s.track.as_ref().and_then(|t| t.artwork_url.as_deref()) == Some(url.as_str()) { s.art = Some(image) }
                }));
            }
        }
        true
    }

    // --- Timers ---

    /// Asks for a poll at the next deadline any timer holds.
    fn schedule(&self, tx: &mut Tx) {
        let deadlines = [self.paused_until, self.download_done, tabs::deadline(self), self.intro_at, self.close_at];
        if let Some(due) = deadlines.into_iter().flatten().chain(self.timed.iter().map(|t| t.1)).reduce(f64::min) { tx.wake_at(due) }
    }

    /// Runs before every frame: the clock, the timers, the tabs' own and the input region.
    pub fn poll(&mut self, now: f64, tx: &mut Tx) -> bool {
        let mut changed = false;
        if now >= self.next_second {
            let before = (self.time.clone(), self.date.clone());
            self.set_clock();
            changed |= before != (self.time.clone(), self.date.clone());
            self.next_second = now + 1.0;
            tx.wake_at(self.next_second);
            changed |= tabs::each_second(self, now);
            self.notifications(now);
            self.water_reminders(now);
            self.rain_alert(now);
            self.break_reminders(now);
            let theme = self.cfg.sun_theme(self.weather.sun, crate::date::now_secs()).filter(|t| *t != self.cfg.theme).map(str::to_string);
            if let Some(theme) = theme { self.save(|c| c.set_theme(&theme)) }
        }
        changed |= tabs::poll(self, now, tx);

        let due = |t: &mut Option<f64>| t.is_some_and(|d| now >= d) && { *t = None; true };
        let (ended, timed): (Vec<_>, Vec<_>) = std::mem::take(&mut self.timed).into_iter().partition(|t| now >= t.1);
        self.timed = timed;
        let mut alert_ended = false;
        for (alert, _) in ended {
            changed = true;
            alert_ended |= match alert {
                // Leaves Large alone, as `set_levels` did.
                Alert::Volume | Alert::Brightness => self.mode != Mode::Large,
                Alert::Camera | Alert::Microphone => { self.next_capture(tx); !self.capturing() }
                Alert::Reminder => { (self.reminder_link, self.notified) = (None, None); true }
                _ => true,
            };
        }
        // An alert ending leaves a Large the user opened meanwhile, by hand or under the pointer.
        if alert_ended && !self.large_open() {
            self.set_mode(Mode::Still);
            changed = true;
        }
        if std::mem::take(&mut self.tab_changed) {
            let at = now + f64::from(self.cfg.tab_linger);
            self.linger_until = Some(at);
            // A tab change after the pointer left pushes the pending close out too.
            if self.close_at.is_some() { self.close_at = Some(at) }
        }
        if let Some(kind) = self.bubble_hovered && !self.bubbles().iter().any(|b| b.0 == kind) {
            self.bubble_hovered = None;
            self.set_over(2, false, tx);
        }
        changed |= self.leave(tx);
        changed |= due(&mut self.paused_until);
        if due(&mut self.download_done) { self.download = None; changed = true }
        if due(&mut self.intro_at) {
            system::open_external("https://github.com/TopMyster/Ripple/blob/main/instructions.md");
            self.save(|c| c.new_user = false);
        }
        self.schedule(tx);

        // Leaving both the Island and an open select's menu closes them.
        if self.open_select.is_none() { self.menu_hovered = false }
        else if !self.hovered && !self.menu_hovered {
            tx.blur();
            self.set_mode(Mode::Still);
            changed = true;
        }

        // A select's menu opens outside the Island: the whole window takes clicks while one is open.
        let region = if self.open_select.is_some() { [0.0, 0.0, self.placement.canvas_w, CANVAS.1] } else {
            let [x, y, w, h] = self.rect();
            let grow = (HOVER_SCALE - 1.0) / 2.0;
            let (mx, my) = (w * grow + 2.0, h * grow + 2.0);
            let [x, y, w, h] = [x - mx, y - my, w + 2.0 * mx, h + 2.0 * my];
            // The dock and the bubbles take clicks too: the region spans them all.
            match self.dock_rect().or_else(|| {
                let bubbles = self.bubbles();
                let l = bubbles.iter().map(|b| b.1[0]).fold(x, f32::min);
                let r = bubbles.iter().map(|b| b.1[0] + b.1[2]).fold(x + w, f32::max);
                (!bubbles.is_empty()).then_some([l, y, r - l, h])
            }) {
                Some([dx, dy, dw, dh]) => {
                    let (l, t) = (x.min(dx), y.min(dy));
                    [l, t, (x + w).max(dx + dw) - l, (y + h).max(dy + dh) - t]
                }
                None => [x, y, w, h],
            }
        };
        let screen = self.screen();
        // The window's points are the Island's times its zoom.
        let k = self.placement.zoom;
        let (wx, wy) = self.placement.window;
        self.passthrough.sync(region.map(|v| v * k), (wx * k, wy * k), &screen, tx);
        changed
    }

    // --- Starting up ---

    /// Runs once the window exists: the pollers start here, after the first frame.
    pub fn start(&mut self, waker: Waker) {
        self.images = Some(ImageCache::new(waker));
        self.clipboard_decode();
        self.begin();
    }

    /// The pollers, once `me` is set: at `start` for a window; an overlay's handle only exists
    /// after its state does, so `main` calls this when it sets `me`.
    pub fn begin(&mut self) {
        if rune::is_headless() || self.weather_kick.is_some() { return }
        let Some(me) = self.me.clone() else { return };
        if self.cfg.new_user { self.intro_at = Some(3.0) }
        // Starting in Large: the tab showing loads now, as it would on opening.
        self.entered_tab();
        self.load_background();
        // Their tabs stay hidden until found.
        if self.cfg.t3_devices.is_empty() && !self.cfg.t3_detected { self.t3_detect() }
        // Unless entering the tab just did: two renewals at once would spend one refresh token twice.
        if !tabs::shows(self, Tab::Limits) { self.limits_load() }
        if self.cfg.auto_launch { std::thread::spawn(|| system::set_auto_launch(true)); }
        let chord = self.cfg.hotkey.trim();
        if !chord.is_empty() {
            match chord.parse().and_then(|k| me.register_hotkey(k)) {
                Ok(id) => self.hotkey = Some(id),
                Err(e) => {
                    eprintln!("Ripple: the shortcut {chord} is off: {e}");
                    // Settings shows it unset, so recording it again retries; the saved chord stays for the next launch.
                    if let Some(settings) = &self.settings_app { settings.update(|s, _| s.hotkey = rune_kit::ShortcutRecorderState::new(None)) }
                }
            }
        }

        fn every(ms: u64, f: impl Fn() + Send + 'static) {
            std::thread::spawn(move || loop {
                f();
                std::thread::sleep(std::time::Duration::from_millis(ms));
            });
        }
        let h = me.clone();
        every(2000, move || { let t = system::media(); h.update_if(move |s, tx| s.poll_track(t, tx)); });
        let h = me.clone();
        every(10000, move || { let b = system::bluetooth_connected(); h.update_if(move |s, tx| s.set_bluetooth(b, tx)); });
        let h = me.clone();
        every(3000, move || { let c = system::camera_in_use(); h.update_if(move |s, tx| s.set_capture(Some(c), None, tx)); });
        let h = me.clone();
        every(3000, move || { let m = system::microphone_in_use(); h.update_if(move |s, tx| s.set_capture(None, Some(m), tx)); });
        let h = me.clone();
        std::thread::spawn(move || system::watch_drags(|active| h.update(move |s, _| s.desktop_drag(active))));
        let h = me.clone();
        let store = self.store.clone();
        // No rule and no break reminders, no query: the default pays nothing.
        every(1000, move || { let f = { let c = store.get(); (c.fullscreen != "none" || c.eye_breaks || c.hourly_breaks || c.quiet_fullscreen) && platform::fullscreen() }; h.update_if(move |s, _| s.set_fullscreen(f)); });
        let h = me.clone();
        every(250, move || { let on = system::caps_lock().unwrap_or(false); h.update_if(move |s, _| s.set_caps_lock(on)); });
        let h = me.clone();
        std::thread::spawn(move || system::watch_levels(|v, b| h.update_if(move |s, tx| s.set_levels(v, b, tx))));
        let h = me.clone();
        std::thread::spawn(move || crate::downloads::watch(|d| h.update_if(move |s, tx| s.set_download(d, tx))));
        let h = me.clone();
        every(5000, move || { let b = system::battery(); h.update_if(move |s, tx| s.set_battery(b, tx)); });
        let h = me.clone();
        std::thread::spawn(move || {
            let mut monitor = system::Monitor::new();
            let showing = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            loop {
                std::thread::sleep(std::time::Duration::from_secs(2));
                let stats = Some(monitor.sample(showing.load(std::sync::atomic::Ordering::Relaxed)));
                let showing = showing.clone();
                // Redraw only while the System tab shows; otherwise just keep the latest figures.
                h.update_if(move |s, _| {
                    let shows = tabs::shows(s, Tab::System);
                    showing.store(shows, std::sync::atomic::Ordering::Relaxed);
                    let changed = s.stats != stats;
                    s.stats = stats;
                    changed && shows
                });
            }
        });

        // Once a day, through `ripple notify`'s queue: it waits for a closed Large like any alert.
        let (h, store) = (me.clone(), self.store.clone());
        std::thread::spawn(move || loop {
            let (cfg, t) = (store.get(), LocalTime::at(crate::date::now_ms()));
            let today = crate::date::days_from_civil(t.year, t.month, t.day);
            // A failed request tries again next hour.
            if cfg.update_check && cfg.update_day != today && let Some(tag) = crate::net::latest_release() {
                h.update(move |s, _| {
                    // Not while quiet, as the queue drops an alert that waits over 5 minutes: next hour.
                    if s.quiet() { return }
                    s.save(|c| c.update_day = today);
                    if crate::net::newer(&tag, env!("CARGO_PKG_VERSION"))
                        && let Some(dir) = s.store.dir()
                        && let Err(e) = crate::notify::queue(&dir, &format!("Ripple {tag} is out"))
                    {
                        eprintln!("Ripple: {e}");
                    }
                });
            }
            std::thread::sleep(std::time::Duration::from_secs(3600));
        });

        let (kick, kicked) = std::sync::mpsc::channel();
        self.weather_kick = Some(kick);
        let store = self.store.clone();
        // The location the weather last came for.
        let mut found = String::new();
        std::thread::spawn(move || loop {
            let cfg = store.get();
            // No location: clear the weather so no icon shows and no request is sent.
            let w = if cfg.location.trim().is_empty() { Some(Weather::default()) } else { crate::net::weather(&cfg.location, &cfg.weather_unit) };
            match w {
                Some(w) => { found = cfg.location.clone(); me.update_if(move |s, _| s.weather != w && { s.weather = w; true }) }
                // A new place not found: the sun times are the old place's, so the themes stop following them.
                // A failed refresh for the same place keeps them.
                None if cfg.location != found => me.update(|s, _| s.weather.sun = None),
                None => {}
            }
            if kicked.recv_timeout(std::time::Duration::from_secs(600)) == Err(std::sync::mpsc::RecvTimeoutError::Disconnected) { break }
        });
    }

    pub fn set_window(&mut self, window: WindowHandle) {
        platform::hide_from_capture(&window, self.cfg.hide_from_capture);
        self.passthrough.set_window(window);
    }
}

/// Moves `last` to `mark`; true only when the hour changed since an earlier mark, so the first mark after launch just seeds it.
fn hour_turned(last: &mut Option<(i64, u32)>, mark: (i64, u32)) -> bool {
    last.replace(mark).is_some_and(|prev| prev != mark)
}

/// A water reminder is due at `(day, hour)`: within the set hours and short of the goal.
fn water_due(cfg: &Config, (day, hour): (i64, u32)) -> bool {
    (cfg.water_from..cfg.water_to).contains(&hour) && cfg.water(day) < cfg.water_goal
}

/// Rain starting at `at` is due an alert at `now` (Unix seconds): within the hour before, or up to
/// 15 minutes after when Large held it back, and not the rain already alerted. A forecast often
/// moves a rain by an hour or two: a start within 3 hours of the alerted one is the same rain.
fn rain_due(at: i64, now: i64, alerted: Option<i64>) -> bool {
    (-900..=3600).contains(&(at - now)) && alerted.is_none_or(|a| (at - a).abs() > 3 * 3600)
}

/// Each break's period in seconds: 20 minutes for the eyes, an hour to get up.
const BREAKS: [f64; 2] = [20.0 * 60.0, 3600.0];

/// Counts `elapsed` off the breaks that are on, unless `paused`; returns the one that fell due,
/// which starts its period again.
fn count_breaks(left: &mut [Option<f64>; 2], on: [bool; 2], elapsed: f64, paused: bool) -> Option<usize> {
    let mut due = None;
    for i in 0..2 {
        if !on[i] { left[i] = None; continue }
        let l = left[i].get_or_insert(BREAKS[i]);
        if paused { continue }
        // A gap of over a minute (sleep, a suspended app) was a break itself: start again.
        if elapsed > 60.0 { *l = BREAKS[i]; continue }
        *l -= elapsed;
        if *l <= 0.0 && due.is_none() { *l = BREAKS[i]; due = Some(i) }
    }
    due
}

/// Fetches an image (blocking) and hands it, loaded, to `put` on the UI thread.
/// `foreign` images come from other apps or hosts: raster only, since Rune bounds neither SVGZ
/// expansion nor the local files an SVG may reference.
pub fn fetch_image(me: &Host, url: String, foreign: bool, put: impl FnOnce(&mut Island, Image) + Send + 'static) {
    let Some(bytes) = crate::net::fetch_image(&url) else { return };
    if foreign && crate::net::is_vector(&bytes) { return }
    me.update(move |s, _| if let Some(image) = s.images.as_ref().map(|i| i.load(bytes)) { put(s, image) });
}

// --- Views ---

pub fn view(s: &Island, cx: &mut Cx<Island>) -> El {
    let cfg = &s.cfg;
    cx.zoom(s.placement.zoom);
    let mut layer = Vec::new();
    let [left, top, w, h] = s.rect();
    let hidden = s.hidden();
    let large = s.mode == Mode::Large;
    let (bg, fg) = if hidden { (Color::TRANSPARENT, Color::TRANSPARENT) } else { (s.island_bg(), cfg.text()) };
    let radii = if cfg.win95() { [0.0; 4] }
        else if cfg.notch_mode { let r = if large { 44.0 } else { h / 2.0 }; [0.0, 0.0, r, r] }
        else { [if large { 44.0 } else { h / 2.0 }; 4] };

    let mut island = el().key("island").role("group").label("Ripple").hit()
        .absolute(left, top).size(w, h)
        .fill(bg).corners(radii).clip_corners(radii)
        .motion(if s.mode == Mode::Still { COLLAPSE } else { SPRING })
        // Large is always under the pointer, so the hover lift only tells the compact Island apart.
        // Hover passes are frequent: no bounce.
        .scale(if s.hovered && !large { HOVER_SCALE } else { 1.0 })
        .scale_motion(Spring::new(0.25, 0.0))
        .on_hover_change(Island::hover)
        .on_drag(Island::drag)
        .on_wheel(Island::wheel);
    // A hidden Shelf takes no drops.
    let shelf = s.visible_tabs().contains(&Tab::Shelf);
    if shelf {
        island = island.on_drop_files(Island::drop_files).on_drop_hover(Island::drop_hover);
    }
    // iOS-style continuous corners when expanded; compact stays a true capsule.
    if large && !cfg.win95() { island = island.continuous_corners() }
    if !hidden {
        let (blur, alpha) = if s.hovered { (32.0, 0.25) } else { (24.0, 0.12) };
        island = island.box_shadow(Shadow { x: 0.0, y: 0.0, blur, spread: 0.0, color: Color::BLACK.alpha(alpha), inset: false });
    }
    if let Some(image) = s.background.as_ref().filter(|i| i.is_ready() && i.error().is_none() && !hidden) {
        island = island.child(el().key("background").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).pixels_fit(image.pixels(), SurfaceFit::Fill).a11y_hidden());
    }

    if large {
        let inset = s.notch_inset();
        // Tabs slide the way the swipe went; the old one leaves with this build's direction.
        // A short shift, not the full width: a swipe changes tab in one step and doesn't track the finger.
        let shift = |e: Edge| { let d = e.direction(); Effect { offset_fraction: Vec2::new(0.2 * d.x, 0.2 * d.y), opacity: 0.0, blur: 1.5, ..Effect::IDENTITY } };
        let edge = if s.tab_dir < 0 { Edge::Leading } else { Edge::Trailing };
        let push = Transition { insertion: shift(edge), removal: shift(edge.opposite()), ..Transition::push(edge) };
        island = island.child(
            el().key("tabs").pin(Some(LARGE_EDGE), Some(inset.max(LARGE_EDGE)), Some(LARGE_EDGE), Some(LARGE_EDGE))
                .transition(Transition::blur_replace())
                .child_exit(push)
                .child(
                    el().key(format!("tab-{}", s.tab as u8)).pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
                        .transition(push)
                        // A press on a disabled control or between controls would reach the Island and close Large.
                        .on_press(|_: &mut Island| {})
                        .child(tabs::view(s, &mut layer)),
                ),
        );
        // The open select's menu is the layer's last element.
        if s.open_select.is_some() && let Some(menu) = layer.pop() {
            layer.push(menu.on_hover_change(|s: &mut Island, hovered, _| s.menu_hovered = hovered));
            // The open menu makes the Island inert, so it hears no hover: this stands in for it. It has no press handler, so presses go through.
            layer.push(el().key("island-hover").hit().absolute(left, top).size(w, h).on_hover_change(|s: &mut Island, inside, tx| s.set_over(3, inside, tx)));
        }
    } else if s.mode == Mode::Quick || s.alert().is_some() || s.shows_track() || cfg.show_info_idle || s.live().is_some() {
        island = island.child(if s.shows_track() && s.alert().is_none() { now_playing(s, fg) } else { glance(s, fg) });
    }

    if !large && let Some(dot) = capture_dot(s) { island = island.child(dot) }

    // The border draws over the content, as a CSS border does.
    island = island.child(border(s, radii, hidden));
    let mut canvas = el().key("canvas").w_full().h_full();
    // Windows lets the window take a held drag near the Island (platform::DROP_NEAR); this catches it there, behind the Island.
    if shelf && cfg!(windows) {
        let near = platform::DROP_NEAR;
        canvas = canvas.child(el().key("drop-near").absolute(left - near, top - near).size(w + 2.0 * near, h + 2.0 * near)
            .on_drop_files(Island::drop_files).on_drop_hover(Island::drop_hover));
    }
    let mut canvas = canvas.child(island);
    // A select's menu opens outside the Island: the whole overlay takes clicks while one is open.
    if s.open_select.is_some() { canvas = canvas.hit() }
    if let Some(rect) = s.dock_rect() { canvas = canvas.child(dock(s, rect)) }
    for (kind, rect) in s.bubbles() { canvas = canvas.child(bubble(s, kind, rect)) }
    rune_kit::root(canvas, layer)
}

/// The Large Island's tabs as a row of icons in a pill of their own.
fn dock(s: &Island, [x, y, w, h]: [f32; 4]) -> El {
    let cfg = &s.cfg;
    let (bg, fg) = (s.island_bg(), cfg.text());
    let (height, gap, button, space) = DOCK;
    let round = |r: f32| cfg.radius(r);
    let mut pill = row().key("dock").role("group").label("Tabs").on_key(Island::dock_key).w_full().h(height).px(space).gap(space).center_y()
        .fill(bg).radius(round(height / 2.0)).border(1.0, fg.alpha(0.12 * fg.a))
        .box_shadow(Shadow { x: 0.0, y: 0.0, blur: 24.0, spread: 0.0, color: Color::BLACK.alpha(0.2), inset: false });
    for tab in s.visible_tabs() {
        let on = tab == s.tab;
        pill = pill.child(
            el().key(format!("dock-{}", tab as u8)).role("button").label(tab.name()).checked(on).focusable(true)
                .size(button, button).center().radius(round(button / 2.0))
                .fill(if on { fg.alpha(0.18 * fg.a) } else { Color::TRANSPARENT })
                .hover(fg.alpha(0.1 * fg.a)).press(fg.alpha(0.2 * fg.a)).press_scale(0.97)
                .child(icon(tabs::icon_of(tab), 17.0).color(if on { fg } else { fg.alpha(0.55 * fg.a) }).a11y_hidden())
                .on_press(move |s: &mut Island| s.open_tab(tab)),
        );
    }
    // The gap to the Island is part of the dock, so crossing it keeps the Island open.
    // It grows out of the Island's edge, not its own centre.
    let enter = Transition::blur_replace();
    let edge = Effect { anchor: Vec2::new(0.5, if s.placement.top { 0.0 } else { 1.0 }), ..enter.insertion };
    let zone = el().key("dock-zone").hit().absolute(x, y).size(w, h).motion(SPRING)
        .transition(Transition { insertion: edge, removal: Effect { opacity: 0.0, ..edge }, ..enter })
        .on_hover_change(Island::hover_dock);
    if s.placement.top { zone.pt(gap) } else { zone.pb(gap) }.child(pill)
}

/// A live activity in a circle beside the compact Island, styled like it. A press opens Large.
fn bubble(s: &Island, kind: Bubble, [x, y, w, h]: [f32; 4]) -> El {
    let cfg = &s.cfg;
    let fg = cfg.text();
    let (key, content) = match kind {
        Bubble::Pomodoro => ("bubble-pomodoro", tabs::pomodoro_bubble(&s.pomodoro, h * 0.6)),
        Bubble::Download => ("bubble-download", s.download.map_or_else(el, |d| download_ring(s, d, h * 0.6))),
        Bubble::CapsLock => ("bubble-caps-lock", el().role("img").label("Caps Lock").child(icon(Alert::CapsLock.icon(), h * 0.45).color(fg).a11y_hidden())),
    };
    let enter = Transition::blur_replace();
    el().key(key).role("button").label(match kind { Bubble::Pomodoro => "Pomodoro: open its tab", Bubble::Download => "Downloading: open Ripple", Bubble::CapsLock => "Caps Lock is on: open Ripple" }).hit().absolute(x, y).size(w, h).center()
        .fill(cfg.bg()).radius(cfg.radius(h / 2.0)).border(1.0, fg.alpha(0.12 * fg.a))
        .box_shadow(Shadow { x: 0.0, y: 0.0, blur: 24.0, spread: 0.0, color: Color::BLACK.alpha(0.12), inset: false })
        .motion(SPRING).transition(Transition { removal: Effect { opacity: 0.0, ..enter.insertion }, ..enter })
        .hover(fg.alpha(0.1 * fg.a)).press_scale(0.95)
        .on_hover_change(move |s: &mut Island, hovered, tx| {
            s.bubble_hovered = hovered.then_some(kind);
            s.set_over(2, hovered, tx)
        })
        .on_press_tx(move |s: &mut Island, tx| s.open_bubble(kind, tx))
        .child(content)
}

fn border(s: &Island, radii: [f32; 4], hidden: bool) -> El {
    let cfg = &s.cfg;
    let ring = el().key("border").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).corners(radii).inert().a11y_hidden();
    if cfg.win95() {
        // A raised Windows 95 edge: light from the top left, shade to the bottom right.
        let grey = rune::color::hex("#808080").unwrap_or(Color::BLACK);
        return ring.border_sides([2.0, 0.0, 0.0, 2.0], Color::WHITE)
            .child(el().pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).border_sides([0.0, 2.0, 2.0, 0.0], grey));
    }
    if !cfg.island_border { return ring }
    let state = if s.camera { Some(Alert::Camera) }
        else if s.microphone { Some(Alert::Microphone) }
        else if s.charging || s.alert() == Some(Alert::Charging) { Some(Alert::Charging) }
        else if s.low_battery() || s.alert() == Some(Alert::Battery) { Some(Alert::Battery) }
        else { s.alert().filter(|a| *a == Alert::Bluetooth) };
    let color = match state.and_then(Alert::color) {
        Some(color) => color,
        None if s.hiding() || hidden => return ring,
        None => cfg.text().alpha(0.3 * cfg.text().a),
    };
    ring.border(1.0, color)
}

/// The compact view between tracks: the time on the left, battery or weather on the right;
/// an alert takes both sides while it shows.
fn glance(s: &Island, fg: Color) -> El {
    let cfg = &s.cfg;
    let alert = s.alert();
    let style = s.cfg.font(16.0, 600.0).lh(16.0);
    let pad = if cfg.notch_mode { 12.0 } else { 16.0 };
    if let Some((level, percent)) = match alert {
        Some(Alert::Volume) => Some((Alert::Volume, s.volume)),
        Some(Alert::Brightness) => Some((Alert::Brightness, s.brightness)),
        _ => None,
    } {
        let percent = percent.unwrap_or(0);
        let glyph = if level == Alert::Volume && percent == 0 { &icons::VOLUME_X } else { level.icon() };
        let label = format!("{} {percent}%", if level == Alert::Volume { "Volume" } else { "Brightness" });
        let ring = el().key("ring").role("img").label(label).child(tabs::ring(24.0, f32::from(percent), fg, icon(glyph, 12.0).color(fg)));
        return row().key(level.key()).w_full().h_full().px(pad - 4.0).center_y().justify_end()
            .transition(Transition::blur_replace()).child(ring);
    }
    let color = alert.and_then(Alert::color).unwrap_or(fg);
    let left = match alert {
        Some(alert) => icon(alert.icon(), 20.0).color(color),
        None => text(&s.time).key("time").font(style.features("'tnum' 1")).color(color),
    };
    let battery_color = if alert.is_none() && s.low_battery() { accent::low() } else { color };
    let percent = || text(s.percent_label()).key("percent").font(style.features("'tnum' 1")).color(battery_color);
    let battery = || if s.charging {
        row().key("battery").gap(2.0).center_y().child(icon(&icons::ZAP, 14.0).color(color)).child(percent())
    } else { percent() };
    let right = match alert {
        Some(alert) if s.joinable() => row().key("label").gap(8.0).center_y()
            .child(text(s.alert_label(alert)).font(style).color(color))
            // The button shows the host: a shared event's link is someone else's, so the user sees where it goes.
            .child(el().key("join").role("button").label(format!("Join {}", tabs::link_host(s.reminder_link.as_deref().unwrap_or("")))).focusable(true).h(26.0).px(12.0).center()
                .radius(cfg.radius(13.0)).fill(accent::green()).hover(accent::green().alpha(0.8)).press_scale(0.96)
                .child(text(tabs::link_host(s.reminder_link.as_deref().unwrap_or(""))).font(cfg.font(12.0, 700.0)).color(Color::BLACK).max_w(130.0).truncate().a11y_hidden())
                .on_press(Island::join)),
        Some(alert) => text(s.alert_label(alert)).key("label").font(style).color(color),
        // A running Pomodoro is a live activity: its ring and time left take the right side.
        None if s.pomodoro.active() => tabs::pomodoro_live(&s.pomodoro, style),
        None if let Some(d) = s.download => row().key("download").gap(6.0).center_y()
            .child(download_ring(s, d, 18.0))
            .child(text(d.label()).font(style.features("'tnum' 1")).color(download_color(s)).a11y_hidden()),
        None if cfg.standby && s.percent.is_some() => battery(),
        None => match s.weather.temp {
            Some(temp) => row().key("weather").gap(2.0).center_y()
                .child(icon(icons::weather(s.weather.code), 14.0).color(color))
                .child(text(format!("{temp}º")).font(style).color(color)),
            None => battery(),
        },
    };
    let row = row().key(alert.map_or("glance", Alert::key)).w_full().h_full().px(pad).center_y()
        .transition(Transition::blur_replace());
    // Mini notch centers the time when there is nothing to show beside it.
    if cfg.notch_mode && cfg.notch_mini && alert.is_none() && s.weather.temp.is_none() && s.percent.is_none() && s.live().is_none() {
        return row.justify_center().child(left);
    }
    row.justify_between().child(left).child(right)
}

/// Blue while downloading; once done, the text colour (white on the default Island, readable on a light theme).
fn download_color(s: &Island) -> Color { if s.download_done.is_some() { s.cfg.text() } else { accent::blue() } }

/// A download's ring, filled to its share done, around a download arrow; a tick once it finishes.
fn download_ring(s: &Island, d: crate::downloads::Progress, size: f32) -> El {
    let (glyph, label) = if s.download_done.is_some() { (&icons::CHECK, "Download finished".to_string()) } else { (&icons::DOWNLOAD, format!("Downloading, {}", d.label())) };
    let color = download_color(s);
    el().role("img").label(label)
        .child(tabs::ring(size, d.percent(), color, icon(glyph, size * 0.55).color(color)))
}

/// Privacy icons at the compact Island's right end while the camera or microphone is in use,
/// once their alert has had its turn; both stack when both are on. Like the border, they show even when the Island hides.
fn capture_dot(s: &Island) -> Option<El> {
    let label = match (s.camera, s.microphone) {
        (true, true) => "Camera and microphone in use",
        (true, false) => "Camera in use",
        (false, true) => "Microphone in use",
        (false, false) => return None,
    };
    if matches!(s.alert(), Some(Alert::Camera | Alert::Microphone)) { return None }
    const SIZE: f32 = 9.0;
    const GAP: f32 = 2.0;
    let mut stack = col().key("capture-dot").role("img").label(label).inert().gap(GAP);
    for (alert, on) in [(Alert::Camera, s.camera), (Alert::Microphone, s.microphone)] {
        if let (true, Some(color)) = (on, alert.color()) { stack = stack.child(icon(alert.icon(), SIZE).color(color).a11y_hidden()) }
    }
    let h = if s.camera && s.microphone { 2.0 * SIZE + GAP } else { SIZE };
    Some(stack.pin(None, Some((s.size().1 - h) / 2.0), Some(4.0), None).transition(Transition::opacity()))
}

/// A clipped line; one wider than the `width` the caller gives it scrolls sideways as one
/// endless line: two copies, sliding left by one copy's width every `period` seconds.
/// It stops at its start under the pointer (WCAG 2.2.2).
pub fn marquee(s: &Island, key: &str, line: El, text_width: f32, width: f32, period: f64) -> El {
    let owned = key.to_string();
    // Always listening, so a leave still lands after the line turns short.
    let clip = el().key(key).clip_soft(0.0).on_hover_change(move |s: &mut Island, hovered, _| {
        if hovered { s.marquee_hovered = Some(owned.clone()) }
        else if s.marquee_hovered.as_deref() == Some(owned.as_str()) { s.marquee_hovered = None }
    });
    if text_width <= width { return clip.child(line) }
    if s.marquee_hovered.as_deref() == Some(key) { return clip.clip_fade(0.0, 0.0, 0.0, 15.0).child(line) }
    let strip = row().key("strip").absolute(0.0, 0.0)
        .child(el().pr(30.0).child(line.clone()))
        .child(el().pr(30.0).child(line.a11y_hidden()))
        .looping(LoopSpec::slide(period, [0.0, 0.0], [-0.5, 0.0]).easing(Easing::LINEAR));
    clip.clip_fade(0.0, 0.0, 15.0, 15.0).child(strip)
}

/// The compact now-playing line around its title: padding 2 × 9, cover 24, gaps 2 × 6, button 25.
const PLAYING_CHROME: f32 = 79.0;

/// The compact view while music plays: cover, a title that scrolls when it doesn't fit, and
/// a visualizer that turns into play/pause under the pointer.
fn now_playing(s: &Island, fg: Color) -> El {
    let Some(track) = &s.track else { return el() };
    let (name, artist) = s.now_playing_text();
    let title = row().key("title")
        .child(text(&name).font(s.cfg.font(13.0, 600.0)).color(fg).single_line())
        .child(text(&artist).font(s.cfg.font(13.0, 400.0)).color(fg.alpha(0.7 * fg.a)).single_line());
    let width = estimate_width(&name, 13.0) + estimate_width(&artist, 13.0);
    let marquee = marquee(s, "marquee", title, width, s.size().0 - PLAYING_CHROME, 12.0).grow(1.0).min_w(0.0).h(18.0);

    let line = row().key(format!("playing-{}-{}", track.name, track.artist)).w_full().h_full().px(9.0).gap(6.0).center_y()
        .transition(Transition::blur_replace())
        .opacity(if !track.playing { 0.5 } else if s.hiding() { 0.6 } else { 1.0 })
        .child(cover(s, 24.0, 4.0, 14.0, 1.05))
        .child(marquee);
    let line = line.child(if s.hovered {
        el().key("play-pause").role("button").label(if track.playing { "Pause" } else { "Play" })
            .size(25.0, 24.0).center().transition(Transition::opacity())
            .child(icon(if track.playing { &icons::PAUSE } else { &icons::PLAY }, 15.0).color(Color::WHITE))
            .on_press(|s: &mut Island| control(s, MediaCommand::PlayPause))
    } else {
        visualizer(track.playing, accent::pink())
    });
    let wrap = el().key("now-playing").w_full().h_full().child(line);
    let Some((position, length)) = track.progress else { return wrap };
    // A hairline along the bottom, inset past the capsule's curve; it glides between the 2 s polls.
    // Keyed by track, so a new track starts its own bar instead of sweeping back.
    let inset = s.size().1 / 2.0;
    wrap.child(
        el().key(format!("progress-{}-{}", track.name, track.artist)).inert().a11y_hidden()
            .pin(Some(inset), None, Some(inset), Some(3.0)).h(2.0).radius(1.0).fill(fg.alpha(0.15 * fg.a))
            .child(el().key("progress-done").h_full().w_frac(position as f32 / length as f32).radius(1.0).fill(fg.alpha(0.7 * fg.a))),
    )
}

/// Four bars that jump up and fall back while music plays, and rest low while it is paused.
/// shortcut: a canned motion, not the real audio level; tap the OS audio output to drive it.
pub fn visualizer(playing: bool, fg: Color) -> El {
    // (period, share hidden while paused) per bar; uneven periods keep the bars out of step.
    const BARS: [(f64, f32); 4] = [(0.62, 0.55), (0.81, 0.3), (0.53, 0.45), (0.94, 0.6)];
    let mut bars = row().key("visualizer").a11y_hidden().size(25.0, 24.0).gap(2.0).center()
        .transition(Transition::opacity());
    for (i, (period, rest)) in BARS.into_iter().enumerate() {
        let mut pill = el().key("pill").size(3.0, 14.0).radius(1.5).fill(fg);
        // Each period the bar snaps full and falls 70% down its clip, the way a level meter decays.
        pill = if playing { pill.looping(LoopSpec::slide(period, [0.0, 0.0], [0.0, 0.7]).easing(Easing::EASE_OUT)) }
            else { pill.absolute(0.0, 14.0 * rest) };
        bars = bars.child(el().key(format!("bar-{i}")).size(3.0, 14.0).clip_soft(0.0).child(pill));
    }
    bars
}

/// The track's cover, or a note when it has none. A press opens the player.
pub fn cover(s: &Island, size: f32, radius: f32, glyph: f32, hover_scale: f32) -> El {
    let source = s.track.as_ref().map(|t| t.source.clone()).unwrap_or_default();
    let open = move |_: &mut Island| {
        let source = source.clone();
        std::thread::spawn(move || system::open_music_player(&source));
    };
    match s.art.as_ref().filter(|i| i.is_ready() && i.error().is_none()) {
        Some(image) => el().key("cover").role("button").label("Open player").focusable(true)
            .size(size, size).radius(radius).pixels_fit(image.pixels(), SurfaceFit::Fill)
            .scale(if s.art_hovered { hover_scale } else { 1.0 })
            .shadow(if s.art_hovered { 8.0 } else { 3.0 }, if s.art_hovered { 24.0 } else { 10.0 }, Color::BLACK.alpha(if s.art_hovered { 0.35 } else { 0.2 }))
            .on_hover_change(|s: &mut Island, hovered, _| s.art_hovered = hovered)
            .on_press(open),
        None => el().key("cover").size(size, size).radius(radius).fill(Color::WHITE.alpha(0.1)).center()
            .child(icon(&icons::MUSIC, glyph).color(s.cfg.text())),
    }
}

/// Media keys run a subprocess; never on the UI thread. Play/pause flips the shown state at once;
/// `poll_track` holds it until the player agrees, for up to 3 s.
pub fn control(s: &mut Island, command: MediaCommand) {
    if command == MediaCommand::PlayPause && let (Some(me), Some(track)) = (&s.me, &s.track) {
        let playing = !track.playing;
        let track = Track { playing, paused: !playing, ..track.clone() };
        me.update_if(move |s, tx| { s.expect_playing = Some((playing, tx.clock() + 3.0)); s.set_track(Some(track), tx) });
    }
    std::thread::spawn(move || system::media_control(command));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn island(cfg: Config) -> Island {
        Island::new(Store::memory(cfg), Vec::new())
    }

    #[test]
    fn the_hotkey_opens_large_and_closes_it_again() {
        assert!(Config::default().hotkey.parse::<Shortcut>().is_ok());
        let mut s = island(Config::default());
        let tx = &mut Tx::default();
        s.hotkey(tx);
        assert_eq!(s.mode, Mode::Large);
        s.hotkey(tx);
        assert_eq!(s.mode, s.rest());
        let mut s = island(Config { large_standby: true, ..Config::default() });
        s.hotkey(tx);
        assert!(s.large_open(), "resting in Large, the hotkey opens it by hand");
        s.hotkey(tx);
        assert!(s.mode == Mode::Large && !s.large_open(), "and lets it rest again");
    }

    #[test]
    fn water_reminders_come_within_the_set_hours_until_the_goal() {
        let cfg = Config { water_goal: 2, water_day: 100, water_count: 1, ..Config::default() };
        assert!(water_due(&cfg, (100, 9)) && water_due(&cfg, (100, 20)));
        assert!(!water_due(&cfg, (100, 8)) && !water_due(&cfg, (100, 21)), "outside 9 to 21");
        assert!(water_due(&cfg, (101, 12)), "a new day starts at 0");
        assert!(!water_due(&Config { water_count: 2, ..cfg }, (100, 12)), "the goal is met");
    }

    #[test]
    fn water_reminders_wait_for_the_hour_to_change() {
        let mut last = None;
        assert!(!hour_turned(&mut last, (100, 15)), "launching mid-hour only seeds the mark");
        assert!(!hour_turned(&mut last, (100, 15)), "the same hour stays quiet");
        assert!(hour_turned(&mut last, (100, 16)), "the next hour reminds");
        assert!(hour_turned(&mut last, (101, 16)), "a new day reminds");
        assert_eq!(last, Some((101, 16)));
    }

    #[test]
    fn rain_alerts_come_once_within_the_hour_before() {
        assert!(!rain_due(10_000, 10_000 - 3601, None), "too early");
        assert!(rain_due(10_000, 10_000 - 3600, None));
        assert!(!rain_due(10_000, 10_000 - 1800, Some(10_000)), "once");
        assert!(!rain_due(10_000 + 3600, 10_000 + 1800, Some(10_000)), "the same rain, moved an hour");
        assert!(rain_due(10_000 + 4 * 3600, 10_000 + 3 * 3600, Some(10_000)), "a later rain");
        assert!(rain_due(10_000, 10_900, None), "held back by Large, still shown up to 15 minutes after it starts");
        assert!(!rain_due(10_000, 10_901, None), "too late");
        assert!(!rain_due(10_000 + 5400, 10_000 + 1800, Some(10_000)), "a pause in the same storm");
        let mut s = island(Config { hour12: false, ..Config::default() });
        s.weather.precip = Some((0, true));
        assert!(s.precip_label().starts_with("Snow at "));
    }

    #[test]
    fn a_full_overview_fits_a_short_display() {
        for mode in ["top-center", "bottom-center"] {
            let mut s = island(Config { position_mode: mode.into(), ..Config::default() });
            s.cfg.dashboard = crate::config::Widget::OWN.into_iter().map(|w| crate::config::Tile::new(w).sized(4, 6)).collect();
            s.placement = Placement::of(&s.cfg, (853.0, 533.0));
            s.mode = Mode::Large;
            s.set_tab(Tab::Overview);
            let ([_, iy, _, ih], [_, dy, _, dh]) = (s.rect(), s.dock_rect().unwrap());
            let (top, bottom) = (iy.min(dy), (iy + ih).max(dy + dh));
            assert!(top >= 0.0 && bottom <= s.placement.visible_h, "{mode}: {top}..{bottom}");
        }
        // No room at all below the Island: it keeps to the Overview's usual size, its tiles shrunk to fit.
        let mut s = island(Config { position_mode: "free".into(), island_y: 500.0, ..Config::default() });
        s.placement = Placement::of(&s.cfg, (960.0, 540.0));
        s.mode = Mode::Large;
        s.set_tab(Tab::Overview);
        let h = s.size().1;
        assert!(h > 200.0 && h <= tabs::size(Tab::Overview).1, "{h}");
    }

    #[test]
    fn a_scaled_island_keeps_its_window_on_the_display() {
        for scale in [80.0, 110.0, 130.0, 150.0, 170.0, 180.0] {
            for mode in ["top-right", "bottom-right", "bottom-left", "free"] {
                let p = Placement::of(&Config { island_scale: scale, position_mode: mode.into(), ..Config::default() }, (1920.0, 1080.0));
                let [x, y, w, h] = p.frame();
                assert!(x >= 0.0 && y >= 0.0 && x + w <= 1920.01 && y + h <= 1080.01, "{scale}% {mode}: {:?}", p.frame());
                assert!(w > 1918.0, "{scale}%: spans the display, {w}");
            }
        }
        let small = Placement::of(&Config { island_scale: 200.0, ..Config::default() }, (1366.0, 768.0));
        let [x, y, w, h] = small.frame();
        assert!(x >= 0.0 && y >= 0.0 && x + w <= 1366.01 && y + h <= 768.01, "a small display caps the zoom: {:?}", small.frame());
        let free = |scale| { let p = Placement::of(&Config { island_scale: scale, ..Config::default() }, (1920.0, 1080.0)); (p.window.1 + p.y) * p.zoom };
        assert_eq!(free(100.0), free(200.0), "free placement keeps the Island's top on the display");
        let s = island(Config { compact_opacity: 50.0, large_opacity: 25.0, ..Config::default() });
        assert_eq!(s.island_bg().a, 0.5);
    }

    #[test]
    fn breaks_fall_due_on_their_period_and_pause_while_full_screen() {
        let mut left = [None; 2];
        // `secs` one-second ticks, as the Island counts; the first break that falls due.
        let run = |left: &mut [Option<f64>; 2], secs: u32, paused: bool| (0..secs).find_map(|_| count_breaks(left, [true, true], 1.0, paused));
        assert_eq!(count_breaks(&mut left, [true, true], 0.0, false), None);
        assert_eq!(run(&mut left, 1199, false), None);
        assert_eq!(run(&mut left, 600, true), None, "paused");
        assert_eq!(run(&mut left, 1, false), Some(0));
        assert_eq!(left[0], Some(BREAKS[0]), "starts again");
        assert_eq!(run(&mut left, 2400, false), Some(0));
        assert_eq!(run(&mut left, 1200, false), Some(0), "both fall due at the hour: the eye break first");
        assert_eq!(count_breaks(&mut left, [true, true], 0.0, false), Some(1), "the hourly one comes next");
        assert_eq!(count_breaks(&mut left, [true, true], 3600.0, false), None, "waking from sleep starts both again");
        assert_eq!(left, [Some(BREAKS[0]), Some(BREAKS[1])]);
        assert_eq!(count_breaks(&mut left, [false, false], 5000.0, false), None);
        assert_eq!(left, [None, None]);
    }

    #[test]
    fn quiet_holds_back_downloads_but_not_low_battery() {
        let mut s = island(Config { quiet_fullscreen: true, ..Config::default() });
        let mut tx = Tx::default();
        assert!(!s.quiet());
        s.set_fullscreen(true);
        assert!(s.quiet(), "an app is full screen");
        let p = Some(crate::downloads::Progress { fraction: Some(0.5), count: 1, done: false });
        assert!(!s.set_download(p, &mut tx) && s.download.is_none());
        assert!(s.set_battery(Some((15, false)), &mut tx));
        assert_eq!(s.alert(), Some(Alert::Battery));
        s.set_fullscreen(false);
        assert!(s.set_download(p, &mut tx), "shows once it's no longer quiet");
    }

    #[test]
    fn ctrl_9_and_typing_a_name_reach_every_tab() {
        let mut s = island(Config::default());
        let key = |key| KeyEvent { key, mods: rune::Mods::default() };
        assert!(s.key(&key(Key::Command('9')), &mut Tx::default()));
        assert_eq!((s.mode, s.tab), (Mode::Large, *s.visible_tabs().last().unwrap()));
        let mut tx = Tx::default();
        for c in ["c", "a"] { assert!(s.dock_key(&key(Key::Text(c.into())), &mut tx)) }
        assert_eq!(s.tab, Tab::Calendar, "letters add up while typed together");
        assert!(s.dock_key(&key(Key::Text("x".into())), &mut tx), "no tab named so: the key is still the dock's");
        assert_eq!(s.tab, Tab::Calendar);
        assert!(!island(Config::default()).dock_key(&key(Key::Text(" ".into())), &mut Tx::default()), "a lone Space still presses the button");
        let tabs = Tab::ALL;
        assert_eq!(tab_named(&tabs, "agents"), Some(Tab::Agents), "a later word counts");
        assert_eq!(tab_named(&tabs, "w"), Some(Tab::Workflows));
        assert_eq!(tab_named(&tabs, "qa"), Some(Tab::Workflows));
        assert_eq!(tab_named(&tabs, "zz"), None);
    }

    #[test]
    fn resting_in_large_is_not_open_so_reminders_show_compact() {
        let mut s = island(Config { large_standby: true, ..Config::default() });
        assert!(!s.large_open(), "resting in Large is not open by hand");
        s.remind("Look away 20s".into(), 0.0);
        assert_eq!(s.mode, Mode::Quick, "the reminder shows compact");
        s.set_mode(Mode::Still);
        assert_eq!(s.mode, Mode::Large);
        assert!(!s.large_open(), "back at rest");
        s.hovered = true;
        assert!(s.large_open(), "under the pointer");
        s.hovered = false;
        let cmd1 = KeyEvent { key: Key::Command('1'), mods: rune::Mods::default() };
        assert!(s.key(&cmd1, &mut Tx::default()));
        assert!(s.large_open(), "Cmd+1 opens it by hand with the pointer away");
        let tab = s.tab;
        s.remind("Look away 20s".into(), 0.0);
        assert_eq!((s.mode, s.tab), (Mode::Large, tab), "the reminder waits");
        s.set_mode(Mode::Still);
        assert!(!s.large_open(), "resting clears it");
        let mut s = island(Config::default());
        s.set_mode(Mode::Large);
        assert!(s.large_open());
        s.set_mode(Mode::Quick);
        assert!(!s.large_open());
        let mut s = island(Config { large_standby: true, eye_breaks: true, ..Config::default() });
        s.key(&KeyEvent { key: Key::Command('1'), mods: rune::Mods::default() }, &mut Tx::default());
        s.set_fullscreen(true);
        s.set_fullscreen(false);
        assert!(s.large_open(), "a full screen app with no rule leaves a Large opened by hand alone");
        s.cfg.fullscreen = "standby".into();
        s.set_fullscreen(true);
        assert!(s.mode == Mode::Large && s.large_open(), "nor does one with a rule");
        s.remind("Look away 20s".into(), 0.0);
        s.poll(7.0, &mut Tx::default());
        assert!(!s.reminding() && s.mode == Mode::Large, "a reminder ending leaves it open too");
    }

    #[test]
    fn the_window_stays_on_the_display_and_the_island_keeps_its_place() {
        let display = (1920.0, 1080.0);
        let at = |mode: &str| {
            let p = Placement::of(&Config { position_mode: mode.into(), ..Config::default() }, display);
            let [x, y, w, h] = p.rect((140.0, 40.0));
            // The Island's rect on the display.
            (p.window.0 + x, p.window.1 + y, w, h, p.window)
        };
        assert_eq!(at("top-left"), (15.0, 15.0, 140.0, 40.0, (0.0, 0.0)));
        let (x, y, w, _, window) = at("top-right");
        assert_eq!((x + w, y, window.0), (1905.0, 15.0, 0.0));
        let (x, y, _, h, window) = at("bottom-left");
        assert_eq!((x, y + h, window.1), (15.0, 1035.0, 1035.0 + 30.0 - CANVAS.1));
        // An Island `max_w` wide stays in its window and on the display, whatever its anchor.
        for mode in ["top-left", "top-right", "top-center", "free"] {
            let p = Placement::of(&Config { position_mode: mode.into(), ..Config::default() }, display);
            let [x, _, w, _] = p.rect((p.max_w(), 40.0));
            assert!(x >= 0.0 && x + w <= p.visible_w, "{mode}: {x}..{}", x + w);
        }
        let (x, _, w, _, _) = at("free");
        assert_eq!(x + w / 2.0, 960.0);
        let notch = Placement::of(&Config { notch_mode: true, ..Config::default() }, display);
        assert_eq!((notch.window.1, notch.rect((210.0, 34.0))[1]), (0.0, 0.0));
    }

    #[test]
    fn the_island_rests_where_the_standby_settings_say() {
        assert_eq!(island(Config::default()).mode, Mode::Quick);
        assert_eq!(island(Config { standby: false, ..Config::default() }).mode, Mode::Still);
        assert_eq!(island(Config { standby: false, large_standby: true, ..Config::default() }).mode, Mode::Large);
        assert_eq!(island(Config { large_standby: true, ..Config::default() }).mode, Mode::Large);
        assert_eq!(island(Config { hide_inactive: true, ..Config::default() }).mode, Mode::Still);
        assert_eq!(island(Config { hide_inactive: true, large_standby: true, ..Config::default() }).mode, Mode::Still);
    }

    #[test]
    fn a_full_screen_app_applies_its_rule_to_a_resting_island() {
        let full = |rule: &str, cfg: Config| { let mut s = island(Config { fullscreen: rule.into(), ..cfg }); s.set_fullscreen(true); s };
        assert_eq!(full("none", Config::default()).mode, Mode::Quick);
        assert_eq!(full("still", Config::default()).mode, Mode::Still);
        assert_eq!(full("standby", Config { standby: false, ..Config::default() }).mode, Mode::Quick);
        // The rule beats Hide when inactive, so the Island neither hides nor draws as hidden.
        let s = full("standby", Config { hide_inactive: true, ..Config::default() });
        assert!(s.mode == Mode::Quick && !s.hiding());
        let mut s = full("hidden", Config::default());
        assert!(s.mode == Mode::Still && s.hidden());
        s.set_fullscreen(false);
        assert!(s.mode == Mode::Quick && !s.hidden());
        // A hovered Island may have taken focus, so its reading is ignored.
        s.hovered = true;
        assert!(!s.set_fullscreen(true) && !s.fullscreen);
    }

    #[test]
    fn hide_when_inactive_still_shows_a_playing_track() {
        let mut s = island(Config { hide_inactive: true, ..Config::default() });
        assert!(s.hidden());
        s.set_track(Some(Track { name: "Song".into(), playing: true, ..Default::default() }), &mut Tx::default());
        assert!(s.mode == Mode::Still && !s.hidden());
    }

    #[test]
    fn a_play_pause_click_holds_until_the_player_agrees() {
        let mut s = island(Config::default());
        let mut tx = Tx::default();
        let song = |playing| Some(Track { name: "Song".into(), playing, paused: !playing, ..Default::default() });
        s.set_track(song(true), &mut tx);
        // The click shows paused; a poll that still says playing is ignored.
        s.expect_playing = Some((false, 3.0));
        s.set_track(song(false), &mut tx);
        assert!(!s.poll_track(song(true), &mut tx));
        assert!(!s.playing());
        // Another track, or the player agreeing, ends the hold.
        assert!(s.poll_track(Some(Track { name: "Other".into(), playing: true, ..Default::default() }), &mut tx));
        assert_eq!(s.expect_playing, None);
        // Past the deadline the player wins.
        s.set_track(song(false), &mut tx);
        s.expect_playing = Some((false, -1.0));
        assert!(s.poll_track(song(true), &mut tx));
        assert!(s.playing());
    }

    #[test]
    fn timed_alerts_rank_and_end_in_turn() {
        let mut s = island(Config::default());
        let mut tx = Tx::default();
        s.set_capture(Some(true), Some(true), &mut tx);
        assert_eq!(s.alert(), Some(Alert::Camera), "camera and microphone take turns");
        s.remind_event("Standup".into(), Some("https://meet.example/x".into()), 0.0);
        s.show_alert(Alert::Charging, 1.0, &[]);
        assert_eq!(s.alert(), Some(Alert::Charging), "charging goes over the reminder, the reminder over the camera");
        s.poll(3.5, &mut tx);
        assert_eq!(s.alert(), Some(Alert::Reminder), "the charging alert ended; the event's 30 s go on");
        assert!(s.capturing(), "the camera's turn ended and the microphone's began");
        s.poll(31.0, &mut tx);
        assert_eq!((s.alert(), s.reminder_link.as_deref()), (None, None), "an ended reminder takes its link with it");
    }

    #[test]
    fn bluetooth_alerts_on_connect_and_on_a_disconnect_seen_twice() {
        let mut s = island(Config::default());
        let mut tx = Tx::default();
        assert!(s.set_bluetooth(true, &mut tx));
        assert_eq!((s.alert(), s.alert_label(Alert::Bluetooth).as_str()), (Some(Alert::Bluetooth), "Connected"));
        s.poll(10.0, &mut tx);
        // One failed poll, then the device again: no alert.
        assert!(!s.set_bluetooth(false, &mut tx));
        assert!(!s.set_bluetooth(true, &mut tx));
        assert_eq!(s.alert(), None);
        assert!(!s.set_bluetooth(false, &mut tx));
        assert!(s.set_bluetooth(false, &mut tx));
        assert_eq!((s.alert(), s.alert_label(Alert::Bluetooth).as_str()), (Some(Alert::Bluetooth), "Disconnected"));
    }

    #[test]
    fn levels_ring_on_a_change_and_caps_lock_stays_until_off() {
        let mut s = island(Config { standby: false, ..Config::default() });
        let mut tx = Tx::default();
        // The first reading only sets the level.
        assert!(!s.set_levels(Some(40), Some(50), &mut tx));
        assert_eq!(s.alert(), None);
        assert!(!s.set_levels(None, None, &mut tx));
        assert!(s.set_levels(Some(45), Some(50), &mut tx));
        assert_eq!((s.alert(), s.mode), (Some(Alert::Volume), Mode::Quick));
        assert!(s.poll(10.0, &mut tx));
        assert_eq!((s.alert(), s.mode), (None, Mode::Still));
        assert!(s.set_caps_lock(true));
        assert_eq!((s.alert(), s.mode), (Some(Alert::CapsLock), Mode::Quick));
        // A playing track shows over it.
        s.set_track(Some(Track { name: "Song".into(), playing: true, ..Default::default() }), &mut tx);
        assert_eq!(s.alert(), None);
        s.set_track(None, &mut tx);
        assert!(s.set_caps_lock(false));
        assert_eq!((s.alert(), s.mode), (None, Mode::Still));
    }

    #[test]
    fn extra_live_activities_take_bubbles_beside_the_island() {
        let mut s = island(Config::default());
        let mut tx = Tx::default();
        s.set_caps_lock(true);
        assert!(s.bubbles().is_empty());
        assert_eq!(s.alert(), Some(Alert::CapsLock));
        s.set_track(Some(Track { name: "Song".into(), playing: true, ..Default::default() }), &mut tx);
        s.pomodoro.ends = Some(60.0);
        assert_eq!(s.alert(), None);
        let [x, _, w, h] = s.rect();
        let bubbles = s.bubbles();
        assert_eq!(bubbles.iter().map(|b| b.0).collect::<Vec<_>>(), [Bubble::Pomodoro, Bubble::CapsLock]);
        // Centred: the first goes right, the next left.
        assert_eq!(bubbles[0].1[0], x + w + BUBBLE_GAP);
        assert_eq!(bubbles[1].1[0], x - BUBBLE_GAP - h);
        s.open_bubble(Bubble::Pomodoro, &mut tx);
        assert_eq!((s.mode, s.tab), (Mode::Large, Tab::Pomodoro));
        assert!(s.bubbles().is_empty());

        // Against the left edge, both go right.
        let mut s = island(Config { position_mode: "top-left".into(), ..Config::default() });
        s.set_caps_lock(true);
        s.set_track(Some(Track { name: "Song".into(), playing: true, ..Default::default() }), &mut tx);
        s.pomodoro.ends = Some(60.0);
        let [x, _, w, h] = s.rect();
        assert_eq!(s.bubbles().iter().map(|b| b.1[0]).collect::<Vec<_>>(), [x + w + BUBBLE_GAP, x + w + 2.0 * BUBBLE_GAP + h]);

        // Near the window's left edge, a centred Island's second bubble goes right too.
        let mut s = island(Config { island_x: 0.0, ..Config::default() });
        s.set_caps_lock(true);
        s.set_track(Some(Track { name: "Song".into(), playing: true, ..Default::default() }), &mut tx);
        s.pomodoro.ends = Some(60.0);
        assert!(s.bubbles().iter().all(|b| b.1[0] >= 0.0 && b.1[0] + b.1[2] <= s.placement.canvas_w));

        // A bubble that goes away under the pointer lets go of the hover.
        s.bubble_hovered = Some(Bubble::CapsLock);
        s.set_over(2, true, &mut tx);
        s.set_caps_lock(false);
        s.poll(1.0, &mut tx);
        assert!(!s.hovered);

        // With info shown when idle, hovering a bubble still leaves the Island compact.
        let mut s = island(Config { show_info_idle: true, ..Config::default() });
        s.pomodoro.ends = Some(60.0);
        s.set_caps_lock(true);
        s.bubble_hovered = Some(Bubble::CapsLock);
        s.set_over(2, true, &mut tx);
        assert_ne!(s.mode, Mode::Large);
        assert_eq!(s.bubbles().len(), 1);
    }

    #[test]
    fn a_download_shows_in_the_glance_or_beside_a_track() {
        let mut s = island(Config { hide_inactive: true, ..Config::default() });
        let mut tx = Tx::default();
        assert!(s.hidden());
        let p = |fraction, done| Some(crate::downloads::Progress { fraction, count: 1, done });
        assert!(s.set_download(p(Some(0.5), false), &mut tx));
        assert!(!s.set_download(s.download, &mut tx), "no change, no redraw");
        assert!(s.set_download(None, &mut tx) && s.download.is_none(), "gone with no file: cancelled, no tick");
        assert!(!s.set_download(p(Some(1.0), true), &mut tx), "a finish the Island never saw running stays quiet");
        s.set_download(p(Some(0.95), false), &mut tx);
        assert!(!s.hidden(), "a download shows even when the Island hides");
        assert!(s.bubbles().is_empty());
        s.set_track(Some(Track { name: "Song".into(), playing: true, ..Default::default() }), &mut tx);
        assert_eq!(s.bubbles().iter().map(|b| b.0).collect::<Vec<_>>(), [Bubble::Download]);
        // Finished, its file there: it stays full, with its tick, until the moment is up.
        assert!(s.set_download(p(Some(1.0), true), &mut tx));
        assert_eq!((s.download.map(|d| d.percent()), s.download_done.is_some()), (Some(100.0), true));
        assert!(!s.set_download(None, &mut tx));
        s.poll(DOWNLOAD_DONE + 1.0, &mut tx);
        assert_eq!((s.download, s.download_done), (None, None));
        assert!(s.bubbles().is_empty());
        // A download with no known percent just goes.
        s.set_download(p(None, false), &mut tx);
        s.set_download(None, &mut tx);
        assert_eq!(s.download, None);
    }

    #[test]
    fn notch_mode_sizes_around_the_notch() {
        let mut s = island(Config { standby: false, ..Config::default() });
        s.mode = Mode::Large;
        // Short enough that the display's height doesn't cap it.
        s.cfg.dashboard.truncate(1);
        let large = s.size();
        // Notch Mode's Large Island grows by what the notch hides.
        s.cfg.notch_mode = true;
        assert!(s.size().0 == large.0 && s.size().1 > large.1);
        // The compact notch glance fits its wider half on each side of the notch.
        s.mode = Mode::Quick;
        s.time = "9:41".into();
        s.percent = Some(100);
        let quick = s.size().0;
        s.show_alert(Alert::Microphone, 1.0, &[]);
        assert!(s.size().0 > quick, "a longer alert label widens it");
        s.cfg.notch_auto_size = false;
        let fixed = s.size().0;
        s.timed.clear();
        assert_eq!(s.size().0, fixed, "without auto size the width stays put");
        // Mini notch keeps the notch height but drops the notch gap and the Large inset.
        s.cfg.notch_mini = true;
        assert_eq!(s.size().1, island(Config { notch_mode: true, ..Config::default() }).size().1);
        s.mode = Mode::Large;
        assert_eq!(s.size(), large);
    }

    #[test]
    fn the_dock_fits_the_canvas_with_every_tab_visible() {
        let t3 = crate::config::T3Device { name: "t".into(), url: "u".into(), token: "t".into() };
        let mut s = island(Config { hidden_tabs: Vec::new(), t3_devices: vec![t3], ..Config::default() });
        s.mode = Mode::Large;
        assert_eq!(s.visible_tabs().len(), Tab::ALL.len());
        let [x, _, w, _] = s.dock_rect().unwrap();
        assert!(x >= 4.0 && x + w <= s.placement.canvas_w - 4.0, "dock spans {x}..{}", x + w);
    }

    #[test]
    fn crossing_from_the_island_to_its_dock_keeps_it_large() {
        let mut s = island(Config { standby: false, ..Config::default() });
        let mut tx = Tx::default();
        s.hover(true, &mut tx);
        s.toggle(&mut tx);
        assert!(s.dock_rect().is_some());
        // Leaving the Island and entering the dock arrive together, in either order.
        s.hover(false, &mut tx);
        s.hover_dock(true, &mut tx);
        s.poll(0.0, &mut tx);
        assert_eq!(s.mode, Mode::Large);
        s.hover_dock(false, &mut tx);
        s.poll(0.1, &mut tx);
        assert_eq!(s.mode, Mode::Still);
        assert!(s.dock_rect().is_none());
        // Under the Island at the top of the display, over it at the bottom.
        s.mode = Mode::Large;
        let ([_, iy, _, ih], [_, dy, _, dh]) = (s.rect(), s.dock_rect().unwrap());
        assert_eq!(dy, iy + ih);
        s.placement = Placement::of(&Config { position_mode: "bottom-center".into(), ..Config::default() }, (1920.0, 1080.0));
        let ([_, iy, _, _], [_, dy, _, dh2]) = (s.rect(), s.dock_rect().unwrap());
        assert_eq!((dy + dh2, dh2), (iy, dh));
    }

    #[test]
    fn a_file_drag_opens_the_shelf_and_a_drop_stashes_it() {
        let s = island(Config { standby: false, shelf: vec!["/b".into()], ..Config::default() });
        let [left, top, w, h] = s.rect();
        let at = Some((left + w / 2.0, top + h / 2.0));
        let mut rt = rune::Runtime::new(s, view, CANVAS, rune::Env { system_dark: true, scale: 1.0 });
        rt.frame(0.0);
        rt.drag_files(vec!["/a".into(), "/b".into()], at);
        rt.frame(1.0 / 60.0);
        assert_eq!((rt.state.mode, rt.state.tab, rt.state.shelf.dropping), (Mode::Large, Tab::Shelf, true));
        rt.drop_files(vec!["/a".into(), "/b".into()], at);
        rt.frame(2.0 / 60.0);
        assert_eq!((rt.state.mode, rt.state.shelf.dropping), (Mode::Large, false));
        assert_eq!(rt.state.store.get().shelf, [std::path::PathBuf::from("/a"), "/b".into()]);
        // A drag that leaves without dropping lets the Island rest.
        rt.drag_files(vec!["/c".into()], at);
        rt.frame(3.0 / 60.0);
        rt.drag_files_leave();
        rt.frame(4.0 / 60.0);
        assert_eq!(rt.state.mode, Mode::Still);
    }

    #[test]
    fn a_desktop_drag_opens_the_shelf_until_it_ends_elsewhere() {
        let mut s = island(Config { standby: false, ..Config::default() });
        s.desktop_drag(true);
        assert_eq!((s.mode, s.tab), (Mode::Large, Tab::Shelf));
        // Passing over the Island and off again keeps it open while the drag goes on.
        s.drop_hover(true, &mut Tx::default());
        s.drop_hover(false, &mut Tx::default());
        assert_eq!(s.mode, Mode::Large);
        s.desktop_drag(false);
        assert_eq!(s.mode, Mode::Still);
        // Ending over the Island, even under the pointer, waits for the drop, then a leave rests it.
        s.desktop_drag(true);
        s.hover(true, &mut Tx::default());
        s.drop_hover(true, &mut Tx::default());
        s.desktop_drag(false);
        assert_eq!(s.mode, Mode::Large);
        s.hover(false, &mut Tx::default());
        s.drop_hover(false, &mut Tx::default());
        assert_eq!(s.mode, Mode::Still);
        // A drop on the Shelf keeps it open after the drag ends.
        s.desktop_drag(true);
        s.drop_files(&["/a".into()], &mut Tx::default());
        s.desktop_drag(false);
        assert_eq!(s.mode, Mode::Large);
        // A hidden Shelf doesn't open.
        let mut s = island(Config { standby: false, hidden_tabs: vec![Tab::Shelf], ..Config::default() });
        s.desktop_drag(true);
        assert_eq!(s.mode, Mode::Still);
    }

    #[test]
    fn tabs_with_nothing_to_show_stay_out_of_the_dock() {
        let mut s = island(Config { hidden_tabs: Vec::new(), default_tab: Tab::Limits, ..Config::default() });
        assert!(!s.visible_tabs().contains(&Tab::T3), "T3 Code waits for a device");
        assert_eq!(s.first_tab(), Tab::Limits, "AI Limits shows until a look finds no agent");
        s.limits.usage = Some(Vec::new());
        assert!(!s.visible_tabs().contains(&Tab::Limits));
        assert_eq!(s.first_tab(), s.cfg.tab_order[0], "a hidden starred tab falls back to the first one shown");
    }

    #[test]
    fn large_opens_on_the_starred_tab_unless_turned_off() {
        let mut s = island(Config { default_tab: Tab::Search, ..Config::default() });
        s.tab = Tab::Assistant;
        s.set_mode(Mode::Large);
        assert_eq!(s.tab, Tab::Search);
        s.cfg.open_default_tab = false;
        s.set_mode(Mode::Quick);
        s.tab = Tab::Assistant;
        s.set_mode(Mode::Large);
        assert_eq!(s.tab, Tab::Assistant);
    }

    #[test]
    fn tabs_move_within_the_visible_ones() {
        let mut s = island(Config { hidden_tabs: vec![Tab::Search], default_tab: Tab::Overview, ..Config::default() });
        assert_eq!(s.tab, Tab::Overview);
        s.move_tab(-1);
        assert_eq!(s.tab, Tab::Overview);
        s.move_tab(1);
        assert_eq!(s.tab, Tab::Workflows);
    }

    #[test]
    fn arrows_switch_tabs_even_while_typing_in_the_search_field() {
        let mut s = island(Config { default_tab: Tab::Search, ..Config::default() });
        s.mode = Mode::Large;
        s.finder.field.focused = true;
        let mut rt = rune::Runtime::new(s, view, CANVAS, rune::Env { system_dark: true, scale: 1.0 });
        rt.frame(0.0);
        rt.key(KeyEvent { key: Key::Text("a".into()), mods: rune::Mods::default() });
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.finder.field.text, "a");
        rt.key(KeyEvent { key: Key::Right, mods: rune::Mods::default() });
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.tab, Tab::Workflows);
    }

    #[test]
    fn one_long_swipe_changes_tab_once_and_a_new_one_again() {
        // Workflows scrolls, and a scroll view takes a wheel before the Island's handler, so skip
        // it: the swipe is about the gesture, not about which tab happens to have a scroller.
        let mut s = island(Config { hidden_tabs: vec![Tab::Workflows], default_tab: Tab::Overview, ..Config::default() });
        s.mode = Mode::Large;
        let [left, top, w, _] = s.rect();
        let mut rt = rune::Runtime::new(s, view, CANVAS, rune::Env { system_dark: true, scale: 1.0 });
        rt.frame(0.0);
        let swipe = |rt: &mut rune::Runtime<Island>| for i in 0..90 {
            let phase = match i { 0 => rune::GesturePhase::Began, 89 => rune::GesturePhase::Ended, _ => rune::GesturePhase::Moved };
            rt.wheel_event(WheelEvent::new(-10.0, 0.0).at(left + w / 2.0, top + 10.0).precise(true).phase(phase));
            rt.frame(1.0 / 60.0);
        };
        // 1.5 s of fingers and momentum outlasts any fixed lock.
        swipe(&mut rt);
        assert_eq!(rt.state.tab, Tab::Search);
        rt.frame(0.3);
        swipe(&mut rt);
        assert_eq!(rt.state.tab, Tab::Assistant);
    }

    #[test]
    fn a_pause_within_one_swipe_does_not_change_tab_again() {
        let mut s = island(Config { hidden_tabs: vec![Tab::Workflows], default_tab: Tab::Overview, ..Config::default() });
        s.mode = Mode::Large;
        let [left, top, w, _] = s.rect();
        let at = |phase| WheelEvent::new(-10.0, 0.0).at(left + w / 2.0, top + 10.0).precise(true).phase(phase);
        let mut rt = rune::Runtime::new(s, view, CANVAS, rune::Env { system_dark: true, scale: 1.0 });
        rt.frame(0.0);
        rt.wheel_event(at(rune::GesturePhase::Began));
        rt.frame(1.0 / 60.0);
        for _ in 0..5 {
            rt.wheel_event(at(rune::GesturePhase::Moved));
            rt.frame(1.0 / 60.0);
        }
        assert_eq!(rt.state.tab, Tab::Search);
        // The fingers stay down through the pause, so this is still one swipe.
        for _ in 0..20 { rt.frame(1.0 / 60.0); }
        for _ in 0..6 {
            rt.wheel_event(at(rune::GesturePhase::Moved));
            rt.frame(1.0 / 60.0);
        }
        assert_eq!(rt.state.tab, Tab::Search);
    }

    #[test]
    fn the_wheel_scrolls_a_long_task_list() {
        let mut s = island(Config { tasks: (0..30).map(|i| crate::config::Item::new(format!("task {i}"))).collect(), ..Config::default() });
        s.mode = Mode::Large;
        s.set_tab(Tab::Tasks);
        let [left, top, w, h] = s.rect();
        let mut rt = rune::Runtime::new(s, view, CANVAS, rune::Env { system_dark: true, scale: 1.0 });
        for _ in 0..60 { rt.frame(1.0 / 60.0); }
        let first = format!("task-{}", rt.state.cfg.tasks[0].key);
        let before = rt.find(&first).unwrap().1;
        rt.wheel(left + w / 2.0, top + h / 2.0, -200.0);
        for _ in 0..30 { rt.frame(1.0 / 60.0); }
        let after = rt.find(&first).unwrap().1;
        assert!(after < before - 50.0, "task-0 stayed at {before} -> {after}");
    }

    #[test]
    fn a_swipe_over_a_scrolling_list_changes_tab() {
        let mut s = island(Config { tasks: (0..30).map(|i| crate::config::Item::new(format!("task {i}"))).collect(), ..Config::default() });
        s.mode = Mode::Large;
        s.set_tab(Tab::Tasks);
        let before = s.tab;
        let [left, top, w, h] = s.rect();
        let mut rt = rune::Runtime::new(s, view, CANVAS, rune::Env { system_dark: true, scale: 1.0 });
        for _ in 0..60 { rt.frame(1.0 / 60.0); }
        for i in 0..10 {
            let phase = match i { 0 => rune::GesturePhase::Began, 9 => rune::GesturePhase::Ended, _ => rune::GesturePhase::Moved };
            rt.wheel_event(WheelEvent::new(-10.0, 0.0).at(left + w / 2.0, top + h / 2.0).precise(true).phase(phase));
            rt.frame(1.0 / 60.0);
        }
        assert_ne!(rt.state.tab, before);
    }

}
