//! The Island: its state, the rules that move it between Still, Quick and Large, the alerts,
//! and the compact views. The Large mode's tabs are in `tabs/`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;

use rune::time::LocalTime;
use rune::{
    AppHandle, Color, Curve, Cx, DisplayTarget, Drag, Easing, Edge, Effect, GesturePhase, El, Image, ImageCache, Key, KeyEvent, LoopSpec, OverlayHandle, OverlayPlacement,
    Phase, Shadow, Spring, SurfaceFit, TextEdit, Transition, Tx, Vec2, Waker, WheelEvent, WindowHandle, col, el, icon, row, text,
};

use crate::config::{Config, Store, Tab};
use crate::net::Weather;
use crate::platform::{self, Passthrough, Screen};
use crate::system::{self, MediaCommand, Track};
use crate::{icons, tabs};

/// The Island's window, in points: the largest Island at its hover scale, its tab dock, plus their shadows.
pub const CANVAS: (f32, f32) = (620.0, 460.0);
/// Room kept around the Island inside the window for its shadow and hover scale.
const PAD: f32 = 30.0;
/// The tab dock under the Large Island: its height, the gap above it, one button and the space between.
const DOCK: (f32, f32, f32, f32) = (44.0, 12.0, 32.0, 6.0);
/// Room between the Large Island's edge and its tab's content.
const LARGE_EDGE: f32 = 12.0;
/// Framer's spring (stiffness 400, damping 40, mass 2.5) as duration and bounce.
pub const SPRING: Spring = Spring::new(0.5, 0.3);
/// The Island grows this much under the pointer, past its layout rect.
const HOVER_SCALE: f32 = 1.05;

/// Status hues for the pill border and alerts.
pub mod status {
    use rune::Color;
    fn hex(s: &str) -> Color { crate::config::parse_color(s).unwrap_or(Color::WHITE) }
    pub fn charge() -> Color { hex("#4cc38a") }
    pub fn low() -> Color { hex("#e5484d") }
    pub fn camera() -> Color { hex("#e8b931") }
    pub fn mic() -> Color { hex("#f0883e") }
    pub fn bluetooth() -> Color { hex("#4c9aff") }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Still,
    Quick,
    Large,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Alert {
    Charging,
    Battery,
    Bluetooth,
    Camera,
    Microphone,
}

impl Alert {
    fn color(self) -> Color {
        match self {
            Alert::Charging => status::charge(),
            Alert::Battery => status::low(),
            Alert::Bluetooth => status::bluetooth(),
            Alert::Camera => status::camera(),
            Alert::Microphone => status::mic(),
        }
    }

    fn icon(self) -> &'static rune::Icon {
        match self {
            Alert::Charging | Alert::Battery => &icons::ZAP,
            Alert::Bluetooth => &icons::HEADPHONES,
            Alert::Camera => &icons::CAMERA,
            Alert::Microphone => &icons::MIC,
        }
    }

    fn key(self) -> &'static str {
        match self {
            Alert::Charging => "charging",
            Alert::Battery => "battery",
            Alert::Bluetooth => "bluetooth",
            Alert::Camera => "camera",
            Alert::Microphone => "microphone",
        }
    }
}

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

    pub fn update_if(&self, f: impl FnOnce(&mut Island, &mut Tx) -> bool + Send + 'static) {
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

    /// Lets a click give the overlay keyboard focus; a window takes it on click already.
    fn allow_keys(&self) {
        if let Host::Overlay(h, ..) = self { h.request_keys() }
    }
}

/// Where the Island's window sits on its display and where the Island is anchored inside it.
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
}

impl Placement {
    pub fn of(cfg: &Config, display: (f32, f32)) -> Placement {
        let (dw, dh) = display;
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
                _ => (0, dw * cfg.island_x / 100.0, true, cfg.island_y),
            }
        };
        // The window stays on the display; the Island moves inside it instead.
        let wx = match align { -1 => x - PAD, 0 => x - CANVAS.0 / 2.0, _ => x + PAD - CANVAS.0 }.min(dw - CANVAS.0).max(0.0);
        let wy = if top { y - PAD } else { y + PAD - CANVAS.1 }.min(dh - CANVAS.1).max(0.0);
        Placement { window: (wx.round(), wy.round()), align, x: x - wx.round(), top, y: y - wy.round() }
    }

    /// The Island's rect inside the window for a size.
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
    /// Over the Island, over the dock.
    over: [bool; 2],
    /// The pointer left both; the next poll lets the Island rest.
    left: bool,

    pub time: String,
    pub date: String,
    /// (year, month, day), for the Calendar tab.
    pub today: (i32, u32, u32),
    /// None on a machine without a battery.
    pub percent: Option<u8>,
    pub charging: bool,
    pub weather: Weather,
    pub track: Option<Track>,
    pub art: Option<Image>,
    pub art_hovered: bool,
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

    battery_alert_until: Option<f64>,
    charging_alert_until: Option<f64>,
    bluetooth_alert_until: Option<f64>,
    capture: Option<(Alert, f64)>,
    capture_queue: Vec<Alert>,
    capture_shown: [bool; 2],
    paused_until: Option<f64>,
    /// After a play/pause click: the state shown, and until when polls of the same track that
    /// still report the old state are ignored while the player catches up.
    expect_playing: Option<(bool, f64)>,

    pub search: TextEdit,
    pub ai: tabs::Assistant,
    pub clipboard: tabs::Clipboard,
    pub task: TextEdit,
    /// The Calendar tab's month, as months from this one.
    pub calendar_offset: i32,
    pub notes: tabs::Notes,
    /// Nextcloud sync of Notes and Tasks, by `nextcloud::Kind`.
    pub nextcloud: [crate::nextcloud::SyncState; 2],
    pub stremflix: tabs::Stremflix,
    /// The System tab's figures; None until the first sample.
    pub stats: Option<system::Stats>,
    /// The AI Limits tab's agents; None until the first answer.
    pub limits: Option<Vec<crate::limits::Usage>>,
    /// Bumped on every load, so an earlier answer is dropped.
    pub limits_generation: u64,
    /// A file drag is over the Island.
    pub dropping: bool,
    /// A drag started elsewhere on the desktop opened the Shelf; it rests when the drag ends.
    pub desktop_drag: bool,
    pub t3: tabs::T3,

    pub me: Option<Host>,
    pub settings: Option<WindowHandle>,
    pub images: Option<ImageCache>,
    pub screens: Vec<Screen>,
    pub placement: Placement,
    passthrough: Passthrough,
    /// The Island's window has keyboard focus.
    focused: bool,
    /// The window that had focus when the pointer reached the Island; it gets focus back on close.
    prior: Option<u64>,
    weather_kick: Option<Sender<()>>,
    wheel: f32,
    wheel_reset: f64,
    /// This wheel gesture already changed tab; the rest of it, momentum included, is ignored.
    wheel_spent: bool,
    /// A phased trackpad gesture is under the fingers; a pause in it is not a new gesture.
    wheel_phased: bool,
    /// The way the last tab change went: 1 to the next tab, -1 to the previous.
    pub tab_dir: i32,
    swipe_lock_until: f64,
    /// The tab just changed; the next poll stamps `linger_until` from it.
    tab_changed: bool,
    /// Until when leaving waits after the last tab change (`cfg.tab_linger` from the change).
    linger_until: Option<f64>,
    /// When a lingering Island closes, unless the pointer comes back first.
    close_at: Option<f64>,
    next_second: f64,
    next_clipboard: f64,
    intro_at: Option<f64>,
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
            tab: cfg.first_tab(),
            hovered: false,
            over: [false; 2],
            left: false,
            time: String::new(),
            today: (1970, 1, 1),
            date: String::new(),
            percent: None,
            charging: false,
            weather: Weather::default(),
            track: None,
            art: None,
            art_hovered: false,
            open_select: None,
            menu_hovered: false,
            background: None,
            bluetooth: false,
            bluetooth_missed: false,
            camera: false,
            microphone: false,
            battery_alert_until: None,
            charging_alert_until: None,
            bluetooth_alert_until: None,
            capture: None,
            capture_queue: Vec::new(),
            capture_shown: [false; 2],
            paused_until: None,
            expect_playing: None,
            search: TextEdit::default(),
            ai: tabs::Assistant::default(),
            clipboard: tabs::Clipboard::default(),
            task: TextEdit::default(),
            calendar_offset: 0,
            notes: tabs::Notes::new(&cfg),
            nextcloud: Default::default(),
            stremflix: tabs::Stremflix::default(),
            stats: None,
            limits: None,
            limits_generation: 0,
            dropping: false,
            desktop_drag: false,
            t3: tabs::T3::new(store.dir().as_deref()),
            me: None,
            settings: None,
            images: None,
            placement: Placement::of(&cfg, (1920.0, 1080.0)),
            screens,
            passthrough: Passthrough::default(),
            focused: false,
            prior: None,
            weather_kick: None,
            wheel: 0.0,
            wheel_reset: 0.0,
            wheel_spent: false,
            wheel_phased: false,
            tab_dir: 1,
            swipe_lock_until: 0.0,
            tab_changed: false,
            linger_until: None,
            close_at: None,
            next_second: 0.0,
            next_clipboard: 0.0,
            intro_at: None,
            store,
            cfg,
        };
        island.placement = Placement::of(&island.cfg, island.screen().size);
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
            let (x, y) = self.placement.window;
            me.place(screen.target, OverlayPlacement::Rect { x, y, width: CANVAS.0, height: CANVAS.1 });
        }
    }

    // --- What it shows ---

    pub fn playing(&self) -> bool { self.track.as_ref().is_some_and(|t| t.playing) }

    /// The compact view shows the track: while it plays, and for 3 seconds after a pause.
    pub fn shows_track(&self) -> bool { self.playing() || self.paused_until.is_some() }

    /// Hide when inactive: resting with no track to show.
    pub fn hidden(&self) -> bool { self.cfg.hide_inactive && self.mode == Mode::Still && !self.shows_track() }

    pub fn alert(&self) -> Option<Alert> {
        if self.charging_alert_until.is_some() { Some(Alert::Charging) }
        else if self.battery_alert_until.is_some() { Some(Alert::Battery) }
        else if self.bluetooth_alert_until.is_some() { Some(Alert::Bluetooth) }
        else { self.capture.map(|c| c.0) }
    }

    /// Under 20% and not charging.
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
            Mode::Large => large.0,
            _ if alerting => if notch { self.notch_glance_width() } else { 176.0 }, // fits icon + "Disconnected"
            _ if track => self.now_playing_width(),
            _ if notch => if self.mode == Mode::Quick || self.cfg.show_info_idle { self.notch_glance_width() } else { 210.0 },
            Mode::Quick | Mode::Still => 140.0, // fits "23:59" + weather/"100%"
        };
        if notch && track && !alerting && self.mode != Mode::Large { w = (w + 40.0).max(300.0) }
        let h = match self.mode {
            Mode::Large => (self.notch_inset() - LARGE_EDGE).max(0.0) + large.1,
            _ => if self.cfg.notch_mode { 34.0 } else { 40.0 }, // Mini notch keeps the notch height
        };
        (w, h)
    }

    /// The Island's rect inside its window.
    pub fn rect(&self) -> [f32; 4] { self.placement.rect(self.size()) }

    /// The tab dock's rect, the gap to the Island included: under the Large Island, or over it
    /// when the Island stands on the bottom of the display.
    pub fn dock_rect(&self) -> Option<[f32; 4]> {
        if self.mode != Mode::Large { return None }
        let (h, gap, button, space) = DOCK;
        let n = self.cfg.visible_tabs().len() as f32;
        let w = n * button + (n - 1.0) * space + 2.0 * space;
        let [x, y, iw, ih] = self.rect();
        let left = (x + iw / 2.0 - w / 2.0).clamp(4.0, CANVAS.0 - w - 4.0);
        Some([left, if self.placement.top { y + ih } else { y - gap - h }, w, gap + h])
    }

    // --- Modes ---

    /// Where the Island rests when nothing holds it open.
    pub fn rest(&self) -> Mode {
        // Standby is on by default, so the opt-in settings win over it or they could never apply.
        // Hiding wins over both, since it only works at Still.
        if self.cfg.hide_inactive { Mode::Still } else if self.cfg.large_standby { Mode::Large } else if self.cfg.standby { Mode::Quick } else { Mode::Still }
    }

    /// Every mode change but `open_large_on` goes through here; Still means wherever the Island rests.
    pub fn set_mode(&mut self, mode: Mode) {
        let was = self.mode;
        self.mode = if mode == Mode::Still { self.rest() } else { mode };
        if self.mode == Mode::Large && was != Mode::Large { self.calendar_offset = 0 }
        if self.mode == Mode::Large && was != Mode::Large {
            if self.cfg.open_default_tab { self.tab = self.cfg.first_tab() }
            self.entered_tab();
        }
        // Resting closes an open select even when the Island rests in Large.
        if mode == Mode::Still || self.mode != Mode::Large { self.open_select = None }
        if self.mode != Mode::Large { self.tab_changed = false; self.linger_until = None; self.close_at = None }
    }

    /// Opens Large straight on `tab`, as a change from the tab shown last; the starred tab doesn't apply.
    fn open_large_on(&mut self, tab: Tab) {
        self.mode = Mode::Large;
        self.open_tab(tab);
    }

    pub fn hover(&mut self, hovered: bool, tx: &mut Tx) { self.set_over(0, hovered, tx) }

    pub fn hover_dock(&mut self, hovered: bool, tx: &mut Tx) { self.set_over(1, hovered, tx) }

    /// Moving from the Island to its dock leaves one before entering the other: the Island
    /// rests only if the pointer is over neither by the next poll.
    fn set_over(&mut self, part: usize, inside: bool, tx: &mut Tx) {
        let was = self.hovered;
        self.over[part] = inside;
        self.hovered = self.over[0] || self.over[1];
        if self.hovered && !was {
            self.close_at = None;
            // Each show makes a new layer surface, so this is asked again on every entry.
            if let Some(me) = &self.me { me.allow_keys() }
            if !self.focused && matches!(self.me, Some(Host::Window(_))) { self.prior = platform::focused_window() }
            #[cfg(target_os = "linux")]
            if !self.focused && self.prior.is_none() && let Some(me @ Host::Window(_)) = self.me.clone() {
                std::thread::spawn(move || if let Some(w) = platform::gnome_focused_window() { me.update(move |s, _| s.prior = Some(w)) });
            }
            if self.mode == Mode::Still && self.cfg.show_info_idle && !self.playing() { self.set_mode(Mode::Large) }
            else if self.mode != Mode::Large { self.set_mode(Mode::Quick) }
        } else if !self.hovered {
            self.art_hovered = false;
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

    /// A click on the Island itself: Large opens and closes.
    pub fn toggle(&mut self, tx: &mut Tx) {
        tx.blur();
        let closing = self.mode == Mode::Large;
        self.set_mode(if closing { Mode::Quick } else { Mode::Large });
        self.entered_tab();
        if closing { self.give_focus_back() }
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

    /// Switches to `tab`, animating in the direction it sits from the current one.
    pub fn set_tab(&mut self, tab: Tab) {
        let visible = self.cfg.visible_tabs();
        let at = |t: Tab| visible.iter().position(|v| *v == t);
        if let (Some(from), Some(to)) = (at(self.tab), at(tab))
            && from != to
        {
            self.tab_dir = if to > from { 1 } else { -1 };
        }
        if self.tab != tab && self.mode == Mode::Large { self.tab_changed = true }
        self.tab = tab;
    }

    pub fn move_tab(&mut self, direction: i32) {
        let visible = self.cfg.visible_tabs();
        let Some(i) = visible.iter().position(|t| *t == self.tab) else { return };
        let next = i as i32 + direction;
        if next < 0 || next >= visible.len() as i32 { return }
        self.open_tab(visible[next as usize]);
    }

    /// Switches tab, sliding the way it lies, and loads what the tab needs.
    pub fn open_tab(&mut self, tab: Tab) {
        self.set_tab(tab);
        self.entered_tab();
    }

    fn entered_tab(&mut self) {
        self.open_select = None;
        if self.mode == Mode::Large { tabs::entered(self) }
    }

    // --- Shelf ---

    /// Opens the Shelf in Large without the pointer; a pending close no longer applies.
    fn open_shelf(&mut self) {
        self.close_at = None;
        // Not the user's tab change: it must not earn the linger.
        let changed = self.tab_changed;
        self.open_large_on(Tab::Shelf);
        self.tab_changed = changed;
    }

    /// A file drag over the Island opens the Shelf; one that leaves without dropping lets it rest.
    pub fn drop_hover(&mut self, over: bool, _: &mut Tx) {
        self.dropping = over;
        if over {
            self.open_shelf();
        } else if !self.hovered && !self.desktop_drag {
            self.set_mode(Mode::Still);
        }
    }

    /// A drag starting anywhere on the desktop opens the Shelf, ready for the drop. When it ends
    /// somewhere else, the Island rests.
    pub fn desktop_drag(&mut self, active: bool) {
        if active {
            if !self.cfg.visible_tabs().contains(&Tab::Shelf) { return }
            self.desktop_drag = true;
            self.open_shelf();
        } else if std::mem::take(&mut self.desktop_drag) && !self.hovered && !self.dropping {
            self.set_mode(Mode::Still);
        }
    }

    /// Dropped files go on top of the Shelf; one already there moves up.
    pub fn drop_files(&mut self, files: &[std::path::PathBuf], _: &mut Tx) {
        self.save(|c| {
            c.shelf.retain(|f| !files.contains(f));
            c.shelf.splice(0..0, files.iter().cloned());
        });
        self.dropping = false;
        self.desktop_drag = false;
        self.open_shelf();
    }

    pub fn key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        match ev.key {
            Key::Right => self.move_tab(1),
            Key::Left => self.move_tab(-1),
            Key::Command(c @ '1'..='8') => {
                let Some(&tab) = self.cfg.visible_tabs().get(c as usize - '1' as usize) else { return false };
                self.open_large_on(tab);
            }
            _ => return false,
        }
        // Keyboard tab changes land at once: they repeat too often to animate.
        tx.instant();
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
    pub fn reload(&mut self, _: &mut Tx) {
        let old = std::mem::replace(&mut self.cfg, self.store.get());
        if self.mode != Mode::Large || !self.hovered { self.set_mode(Mode::Still) }
        if !self.cfg.visible_tabs().contains(&self.tab) { self.open_tab(self.cfg.first_tab()) }
        let size = self.screen().size;
        if old.display != self.cfg.display || Placement::of(&old, size) != Placement::of(&self.cfg, size) { self.place_window() }
        if (old.location != self.cfg.location || old.weather_unit != self.cfg.weather_unit)
            && let Some(kick) = &self.weather_kick
        {
            let _ = kick.send(());
        }
        if old.bg_image != self.cfg.bg_image { self.load_background() }
        if old.hour12 != self.cfg.hour12 { self.set_clock() }
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
        std::thread::spawn(move || fetch_image(&me, url.clone(), move |s, image| if s.cfg.bg_image.trim() == url { s.background = Some(image) }));
    }

    // --- The clock ---

    fn set_clock(&mut self) {
        let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
        // A headless capture reads the same clock on every machine.
        let now = if rune::is_headless() { LocalTime::at_offset(1_767_260_460_000, 0) } else { LocalTime::at(ms) };
        let hour = if self.cfg.hour12 { (now.hour + 11) % 12 + 1 } else { now.hour };
        self.time = format!("{hour}:{:02}", now.minute);
        self.date = format_date(now.year, now.month, now.day);
        self.today = (now.year, now.month, now.day);
    }

    // --- What the pollers report ---

    pub fn set_battery(&mut self, battery: Option<(u8, bool)>, tx: &mut Tx) -> bool {
        let (percent, charging) = battery.map_or((None, false), |(p, c)| (Some(p), c));
        if (percent, charging) == (self.percent, self.charging) { return false }
        let now = tx.clock();
        if charging && !self.charging && self.cfg.battery_alerts {
            self.set_mode(Mode::Quick);
            self.charging_alert_until = Some(now + 1.5);
        }
        if percent != self.percent && percent.is_some_and(|p| [20, 15, 10, 5, 3].contains(&p)) && self.cfg.battery_alerts {
            self.set_mode(Mode::Quick);
            self.battery_alert_until = Some(now + 3.0);
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
        self.bluetooth_alert_until = Some(tx.clock() + 3.0);
        self.schedule(tx);
        true
    }

    /// Camera and microphone alerts take turns, three seconds each.
    pub fn set_capture(&mut self, camera: Option<bool>, microphone: Option<bool>, tx: &mut Tx) -> bool {
        let (camera, microphone) = (camera.unwrap_or(self.camera), microphone.unwrap_or(self.microphone));
        if (camera, microphone) == (self.camera, self.microphone) { return false }
        (self.camera, self.microphone) = (camera, microphone);
        for (i, (alert, in_use)) in [(Alert::Camera, camera), (Alert::Microphone, microphone)].into_iter().enumerate() {
            if in_use && !self.capture_shown[i] { self.capture_queue.push(alert) }
            self.capture_shown[i] = in_use;
        }
        self.capture_queue.retain(|a| if *a == Alert::Camera { camera } else { microphone });
        self.next_capture(tx);
        true
    }

    fn next_capture(&mut self, tx: &mut Tx) {
        if self.capture.is_some() || self.capture_queue.is_empty() { return }
        self.set_mode(Mode::Quick);
        self.capture = Some((self.capture_queue.remove(0), tx.clock() + 3.0));
        self.schedule(tx);
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
        let art_url = |t: &Option<Track>| t.as_ref().and_then(|t| t.artwork_url.clone());
        let (old_art, new_art) = (art_url(&self.track), art_url(&track));
        self.track = track;
        if old_art != new_art {
            self.art = None;
            if let (Some(url), Some(me)) = (new_art, self.me.clone()) {
                std::thread::spawn(move || fetch_image(&me, url.clone(), move |s, image| {
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
        let deadlines = [self.battery_alert_until, self.charging_alert_until, self.bluetooth_alert_until, self.capture.map(|c| c.1), self.paused_until, self.clipboard.copied.map(|c| c.1), self.intro_at, self.close_at];
        if let Some(due) = deadlines.into_iter().flatten().reduce(f64::min) { tx.wake_at(due) }
    }

    /// Runs before every frame: the clock, the timers, the clipboard and the input region.
    pub fn poll(&mut self, now: f64, tx: &mut Tx) -> bool {
        let mut changed = false;
        if now >= self.next_second {
            let before = (self.time.clone(), self.date.clone());
            self.set_clock();
            changed |= before != (self.time.clone(), self.date.clone());
            self.next_second = now + 1.0;
            tx.wake_at(self.next_second);
            // A note uploads once typing in it pauses.
            if self.notes.edited.is_some_and(|t| t.elapsed().as_secs() >= 2) {
                self.notes.edited = None;
                self.nextcloud_sync(crate::nextcloud::Kind::Notes);
            }
        }
        if now >= self.next_clipboard && !rune::is_headless() {
            self.next_clipboard = now + 2.0;
            tx.wake_at(self.next_clipboard);
            tx.read_clipboard(|s: &mut Island, content, _| { s.clipboard.push(content.text()); });
        }

        let due = |t: &mut Option<f64>| t.is_some_and(|d| now >= d) && { *t = None; true };
        let mut alert_ended = due(&mut self.battery_alert_until);
        alert_ended |= due(&mut self.charging_alert_until);
        alert_ended |= due(&mut self.bluetooth_alert_until);
        if self.capture.is_some_and(|c| now >= c.1) {
            self.capture = None;
            self.next_capture(tx);
            alert_ended |= self.capture.is_none();
            changed = true;
        }
        if alert_ended {
            self.set_mode(Mode::Still);
            changed = true;
        }
        if std::mem::take(&mut self.tab_changed) {
            let at = now + f64::from(self.cfg.tab_linger);
            self.linger_until = Some(at);
            // A tab change after the pointer left pushes the pending close out too.
            if self.close_at.is_some() { self.close_at = Some(at) }
        }
        changed |= self.leave(tx);
        changed |= due(&mut self.paused_until);
        if self.clipboard.copied.is_some_and(|c| now >= c.1) {
            self.clipboard.copied = None;
            changed = true;
        }
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
        let region = if self.open_select.is_some() { [0.0, 0.0, CANVAS.0, CANVAS.1] } else {
            let [x, y, w, h] = self.rect();
            let grow = (HOVER_SCALE - 1.0) / 2.0;
            let (mx, my) = (w * grow + 2.0, h * grow + 2.0);
            let [x, y, w, h] = [x - mx, y - my, w + 2.0 * mx, h + 2.0 * my];
            // The dock takes clicks too: the region spans both.
            match self.dock_rect() {
                Some([dx, dy, dw, dh]) => {
                    let (l, t) = (x.min(dx), y.min(dy));
                    [l, t, (x + w).max(dx + dw) - l, (y + h).max(dy + dh) - t]
                }
                None => [x, y, w, h],
            }
        };
        let screen = self.screen();
        self.passthrough.sync(region, self.placement.window, &screen, tx);
        changed
    }

    // --- Starting up ---

    /// Runs once the window exists: the pollers start here, after the first frame.
    pub fn start(&mut self, waker: Waker) {
        self.images = Some(ImageCache::new(waker));
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
        if self.cfg.auto_launch { std::thread::spawn(|| system::set_auto_launch(true)); }

        fn every(seconds: u64, f: impl Fn() + Send + 'static) {
            std::thread::spawn(move || loop {
                f();
                std::thread::sleep(std::time::Duration::from_secs(seconds));
            });
        }
        let h = me.clone();
        every(2, move || { let t = system::media(); h.update_if(move |s, tx| s.poll_track(t, tx)); });
        let h = me.clone();
        every(10, move || { let b = system::bluetooth_connected(); h.update_if(move |s, tx| s.set_bluetooth(b, tx)); });
        let h = me.clone();
        every(3, move || { let c = system::camera_in_use(); h.update_if(move |s, tx| s.set_capture(Some(c), None, tx)); });
        let h = me.clone();
        every(3, move || { let m = system::microphone_in_use(); h.update_if(move |s, tx| s.set_capture(None, Some(m), tx)); });
        let h = me.clone();
        std::thread::spawn(move || system::watch_drags(|active| h.update(move |s, _| s.desktop_drag(active))));
        let h = me.clone();
        every(5, move || { let b = system::battery(); h.update_if(move |s, tx| s.set_battery(b, tx)); });
        let h = me.clone();
        std::thread::spawn(move || {
            let mut monitor = system::Monitor::new();
            loop {
                std::thread::sleep(std::time::Duration::from_secs(2));
                let stats = Some(monitor.sample());
                // Redraw only while the System tab shows; otherwise just keep the latest figures.
                h.update_if(move |s, _| { let changed = s.stats != stats; s.stats = stats; changed && s.mode == Mode::Large && s.tab == Tab::System });
            }
        });

        // Weather: every 10 minutes, and at once when the location or unit changes.
        let (kick, kicked) = std::sync::mpsc::channel();
        self.weather_kick = Some(kick);
        let store = self.store.clone();
        std::thread::spawn(move || loop {
            let cfg = store.get();
            // No location: clear the weather so no icon shows and no request is sent.
            let w = if cfg.location.trim().is_empty() { Some(Weather::default()) } else { crate::net::weather(&cfg.location, &cfg.weather_unit) };
            if let Some(w) = w {
                me.update_if(move |s, _| s.weather != w && { s.weather = w; true });
            }
            if kicked.recv_timeout(std::time::Duration::from_secs(600)) == Err(std::sync::mpsc::RecvTimeoutError::Disconnected) { break }
        });
    }

    pub fn set_window(&mut self, window: WindowHandle) {
        self.passthrough.set_window(window);
    }
}

/// "Thu, Jan 1".
fn format_date(year: i32, month: u32, day: u32) -> String {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    format!("{}, {} {}", DAYS[weekday(year, month, day) as usize], MONTHS[(month as usize - 1) % 12], day)
}

/// 0 for Sunday, by Sakamoto's method.
pub fn weekday(year: i32, month: u32, day: u32) -> u32 {
    const T: [i32; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let y = if month < 3 { year - 1 } else { year };
    (y + y / 4 - y / 100 + y / 400 + T[(month as usize - 1) % 12] + day as i32).rem_euclid(7) as u32
}

/// Fetches an image (blocking) and hands it, loaded, to `put` on the UI thread.
pub fn fetch_image(me: &Host, url: String, put: impl FnOnce(&mut Island, Image) + Send + 'static) {
    let Some(bytes) = crate::net::fetch_image(&url) else { return };
    me.update(move |s, _| if let Some(image) = s.images.as_ref().map(|i| i.load(bytes)) { put(s, image) });
}

// --- Views ---

pub fn view(s: &Island, _: &mut Cx<Island>) -> El {
    let cfg = &s.cfg;
    let mut layer = Vec::new();
    let [left, top, w, h] = s.rect();
    let hidden = s.hidden();
    let (bg, fg) = if hidden { (Color::TRANSPARENT, Color::TRANSPARENT) } else { (cfg.bg(), cfg.text()) };
    let large = s.mode == Mode::Large;
    let radii = if cfg.win95() { [0.0; 4] }
        else if cfg.notch_mode { let r = if large { 44.0 } else { h / 2.0 }; [0.0, 0.0, r, r] }
        else { [if large { 44.0 } else { h / 2.0 }; 4] };

    let mut island = el().key("island").role("group").label("Ripple").hit()
        .absolute(left, top).size(w, h)
        .fill(bg).corners(radii).clip_corners(radii)
        .motion(SPRING)
        // Large is always under the pointer, so the hover lift only tells the compact Island apart.
        .scale(if s.hovered && !large { HOVER_SCALE } else { 1.0 })
        .on_hover_change(Island::hover)
        .on_drag(Island::drag)
        .on_wheel(Island::wheel);
    // A hidden Shelf takes no drops.
    let shelf = cfg.visible_tabs().contains(&Tab::Shelf);
    if shelf {
        island = island.on_drop_files(Island::drop_files).on_drop_hover(Island::drop_hover);
    }
    // iOS-style continuous corners when expanded; compact stays a true capsule.
    if large && !cfg.win95() { island = island.continuous_corners() }
    if !hidden {
        let (blur, alpha) = if s.hovered { (32.0, 0.25) } else { (24.0, 0.12) };
        island = island.box_shadow(Shadow { x: 0.0, y: 0.0, blur, spread: 0.0, color: Color::BLACK.alpha(alpha), inset: false });
    }
    if cfg.glass() && !hidden { island = island.child(glass(radii, large)) }
    if let Some(image) = s.background.as_ref().filter(|i| i.is_ready() && i.error().is_none() && !hidden) {
        island = island.child(el().key("background").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).pixels_fit(image.pixels(), SurfaceFit::Fill).a11y_hidden());
    }

    if large {
        let inset = s.notch_inset();
        // Tabs slide the way the swipe went; the old one leaves with this build's direction.
        let push = Transition::push(if s.tab_dir < 0 { Edge::Leading } else { Edge::Trailing });
        island = island.child(
            el().key("tabs").pin(Some(LARGE_EDGE), Some(inset.max(LARGE_EDGE)), Some(LARGE_EDGE), Some(LARGE_EDGE))
                .transition(Transition::blur_replace())
                .child_exit(push)
                .child(
                    el().key(format!("tab-{}", s.tab as u8)).pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
                        .transition(push)
                        .child(tabs::view(s, &mut layer)),
                ),
        );
        // The open select's menu is the layer's last element.
        if s.open_select.is_some() && let Some(menu) = layer.pop() {
            layer.push(menu.on_hover_change(|s: &mut Island, hovered, _| s.menu_hovered = hovered));
        }
    } else if s.mode == Mode::Quick || s.alert().is_some() || s.shows_track() || cfg.show_info_idle {
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
    rune_kit::root(canvas, layer)
}

/// Smoked glass lit from below: dark at the top left, grey toward the bottom right, a bright lower rim.
fn glass(radii: [f32; 4], large: bool) -> El {
    let grey = |v: u8, a: f32| crate::config::parse_color(&format!("#{v:02x}{v:02x}{v:02x}")).unwrap_or(Color::BLACK).alpha(a);
    let layer = |key: &str| el().key(key).pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).corners(radii).inert().a11y_hidden();
    let layer = |key: &str| if large { layer(key).continuous_corners() } else { layer(key) };
    // Body: a diagonal from near-black to a light smoke grey.
    layer("glass").linear_gradient(1.9, [(0.0, grey(6, 1.0)), (0.3, grey(14, 1.0)), (0.6, grey(42, 1.0)), (1.0, grey(87, 1.0))])
        // Frost pooling in the lower right, where the backdrop shows through.
        .child(layer("glass-frost").radial_gradient([0.85, 1.1], [420.0, 90.0], [(0.0, grey(200, 0.17)), (1.0, grey(200, 0.0))]))
        // The top edge falls into shade.
        .child(layer("glass-shade").gradient(grey(0, 0.33), grey(0, 0.0)))
        // Light caught along the lower edge and the left curve, with a hairline rim on top.
        .child(layer("glass-rim")
            .box_shadow(Shadow { x: 0.0, y: 6.0, blur: 14.0, spread: 0.0, color: grey(0, 0.3), inset: true })
            .box_shadow(Shadow { x: 0.0, y: -2.0, blur: 4.0, spread: 0.0, color: grey(255, 0.21), inset: true })
            .box_shadow(Shadow { x: 2.0, y: 0.0, blur: 6.0, spread: 0.0, color: grey(255, 0.07), inset: true })
            .border_gradient(1.0, grey(255, 0.06), grey(255, 0.33)))
}

/// The Large Island's tabs as a row of icons in a pill of their own.
fn dock(s: &Island, [x, y, w, h]: [f32; 4]) -> El {
    let cfg = &s.cfg;
    let (bg, fg) = (cfg.bg(), cfg.text());
    let (height, gap, button, space) = DOCK;
    let round = |r: f32| cfg.radius(r);
    let mut pill = row().key("dock").role("group").label("Tabs").w_full().h(height).px(space).gap(space).center_y()
        .fill(bg).radius(round(height / 2.0)).border(1.0, fg.alpha(0.12 * fg.a))
        .box_shadow(Shadow { x: 0.0, y: 0.0, blur: 24.0, spread: 0.0, color: Color::BLACK.alpha(0.2), inset: false });
    if cfg.glass() { pill = pill.child(glass([round(height / 2.0); 4], false)) }
    for tab in cfg.visible_tabs() {
        let on = tab == s.tab;
        pill = pill.child(
            el().key(format!("dock-{}", tab as u8)).role("button").label(tab.name()).checked(on).focusable(true)
                .size(button, button).center().radius(round(button / 2.0))
                .fill(if on { fg.alpha(0.18 * fg.a) } else { Color::TRANSPARENT })
                .hover(fg.alpha(0.1 * fg.a)).press(fg.alpha(0.2 * fg.a)).press_scale(0.97)
                .child(icon(icons::tab(tab), 17.0).color(if on { fg } else { fg.alpha(0.55 * fg.a) }).a11y_hidden())
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

fn border(s: &Island, radii: [f32; 4], hidden: bool) -> El {
    let cfg = &s.cfg;
    let ring = el().key("border").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).corners(radii).inert().a11y_hidden();
    if cfg.win95() {
        // A raised Windows 95 edge: light from the top left, shade to the bottom right.
        let grey = crate::config::parse_color("#808080").unwrap_or(Color::BLACK);
        return ring.border_sides([2.0, 0.0, 0.0, 2.0], Color::WHITE)
            .child(el().pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).border_sides([0.0, 2.0, 2.0, 0.0], grey));
    }
    if !cfg.island_border { return ring }
    let color = if s.camera { status::camera() }
        else if s.microphone { status::mic() }
        else if s.charging || s.alert() == Some(Alert::Charging) { status::charge() }
        else if s.low_battery() || s.alert() == Some(Alert::Battery) { status::low() }
        else if s.alert() == Some(Alert::Bluetooth) { status::bluetooth() }
        else if cfg.hide_inactive || hidden { return ring }
        else { cfg.text().alpha(0.3 * cfg.text().a) };
    ring.border(1.0, color)
}

/// The compact view between tracks: the time on the left, battery or weather on the right;
/// an alert takes both sides while it shows.
fn glance(s: &Island, fg: Color) -> El {
    let cfg = &s.cfg;
    let alert = s.alert();
    let color = alert.map_or(fg, Alert::color);
    let style = s.cfg.font(16.0, 600.0).lh(16.0);
    let left = match alert {
        Some(alert) => icon(alert.icon(), 20.0).color(color),
        None => text(&s.time).key("time").font(style.features("'tnum' 1")).color(color),
    };
    let battery_color = if alert.is_none() && s.low_battery() { status::low() } else { color };
    let percent = || text(s.percent_label()).key("percent").font(style).color(battery_color);
    let battery = || if s.charging {
        row().key("battery").gap(2.0).center_y().child(icon(&icons::ZAP, 14.0).color(color)).child(percent())
    } else { percent() };
    let right = match alert {
        Some(alert) => text(s.alert_label(alert)).key("label").font(style).color(color),
        None if cfg.standby && s.percent.is_some() => battery(),
        None => match s.weather.temp {
            Some(temp) => row().key("weather").gap(2.0).center_y()
                .child(icon(icons::weather(&s.weather.status), 14.0).color(color))
                .child(text(format!("{temp}º")).font(style).color(color)),
            None => battery(),
        },
    };
    let pad = if cfg.notch_mode { 12.0 } else { 16.0 };
    let row = row().key(alert.map_or("glance", Alert::key)).w_full().h_full().px(pad).center_y()
        .transition(Transition::blur_replace());
    // Mini notch centers the time when there is nothing to show beside it.
    if cfg.notch_mode && cfg.notch_mini && alert.is_none() && s.weather.temp.is_none() && s.percent.is_none() {
        return row.justify_center().child(left);
    }
    row.justify_between().child(left).child(right)
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
        if on { stack = stack.child(icon(alert.icon(), SIZE).color(alert.color()).a11y_hidden()) }
    }
    let h = if s.camera && s.microphone { 2.0 * SIZE + GAP } else { SIZE };
    Some(stack.pin(None, Some((s.size().1 - h) / 2.0), Some(4.0), None).transition(Transition::opacity()))
}

/// A clipped line; one wider than the `width` the caller gives it scrolls sideways as one
/// endless line: two copies, sliding left by one copy's width every `period` seconds.
pub fn marquee(key: &str, line: El, text_width: f32, width: f32, period: f64) -> El {
    let clip = el().key(key).clip_soft(0.0);
    if text_width <= width { return clip.child(line) }
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
    let marquee = marquee("marquee", title, width, s.size().0 - PLAYING_CHROME, 12.0).grow(1.0).min_w(0.0).h(18.0);

    let line = row().key(format!("playing-{}-{}", track.name, track.artist)).w_full().h_full().px(9.0).gap(6.0).center_y()
        .transition(Transition::blur_replace())
        .opacity(if !track.playing { 0.5 } else if s.cfg.hide_inactive { 0.6 } else { 1.0 })
        .child(cover(s, 24.0, 4.0, 14.0, 1.1))
        .child(marquee);
    let line = line.child(if s.hovered {
        el().key("play-pause").role("button").label(if track.playing { "Pause" } else { "Play" })
            .size(25.0, 24.0).center().transition(Transition::opacity())
            .child(icon(if track.playing { &icons::PAUSE } else { &icons::PLAY }, 15.0).color(Color::WHITE))
            .on_press(|s: &mut Island| control(s, MediaCommand::PlayPause))
    } else {
        visualizer(track.playing, fg)
    });
    let wrap = el().key("now-playing").w_full().h_full().child(line);
    let Some((position, length)) = track.progress else { return wrap };
    // A hairline along the bottom, inset past the capsule's curve; it glides between the 2 s polls.
    // Keyed by track, so a new track starts its own bar instead of sweeping back.
    let inset = s.size().1 / 2.0;
    let width = s.size().0 - 2.0 * inset;
    let done = width * position as f32 / length as f32;
    wrap.child(
        el().key(format!("progress-{}-{}", track.name, track.artist)).inert().a11y_hidden()
            .pin(Some(inset), None, Some(inset), Some(3.0)).h(2.0).radius(1.0).fill(fg.alpha(0.15 * fg.a))
            .child(el().key("progress-done").h_full().w(done).radius(1.0).fill(fg.alpha(0.7 * fg.a)).motion(Spring::new(2.0, 0.0))),
    )
}

/// Four bars that jump up and fall back while music plays, and rest low while it is paused.
/// shortcut: a canned motion, not the real audio level; tap the OS audio output to drive it.
fn visualizer(playing: bool, fg: Color) -> El {
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
        Some(image) => el().key("cover").role("button").label("Open player")
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
    fn dates_read_like_the_short_weekday_month_day() {
        assert_eq!(format_date(2026, 1, 1), "Thu, Jan 1");
        assert_eq!(format_date(2024, 2, 29), "Thu, Feb 29");
        assert_eq!(format_date(2026, 11, 17), "Tue, Nov 17");
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
        assert_eq!((x + w, y, window.0), (1905.0, 15.0, 1920.0 - CANVAS.0));
        let (x, y, _, h, window) = at("bottom-left");
        assert_eq!((x, y + h, window.1), (15.0, 1035.0, 1035.0 + 30.0 - CANVAS.1));
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
    fn bluetooth_alerts_on_connect_and_on_a_disconnect_seen_twice() {
        let mut s = island(Config::default());
        let mut tx = Tx::default();
        assert!(s.set_bluetooth(true, &mut tx));
        assert_eq!((s.alert(), s.alert_label(Alert::Bluetooth).as_str()), (Some(Alert::Bluetooth), "Connected"));
        s.bluetooth_alert_until = None;
        // One failed poll, then the device again: no alert.
        assert!(!s.set_bluetooth(false, &mut tx));
        assert!(!s.set_bluetooth(true, &mut tx));
        assert_eq!(s.alert(), None);
        assert!(!s.set_bluetooth(false, &mut tx));
        assert!(s.set_bluetooth(false, &mut tx));
        assert_eq!((s.alert(), s.alert_label(Alert::Bluetooth).as_str()), (Some(Alert::Bluetooth), "Disconnected"));
    }

    #[test]
    fn notch_mode_sizes_around_the_notch() {
        let mut s = island(Config { standby: false, ..Config::default() });
        s.mode = Mode::Large;
        let large = s.size();
        // Notch Mode's Large Island grows by what the notch hides.
        s.cfg.notch_mode = true;
        assert_eq!(s.size(), (large.0, large.1 + s.notch_inset() - LARGE_EDGE));
        // The compact notch glance fits its wider half on each side of the notch.
        s.mode = Mode::Quick;
        s.time = "9:41".into();
        s.percent = Some(100);
        let quick = s.size().0;
        s.capture = Some((Alert::Microphone, Default::default()));
        assert!(s.size().0 > quick, "a longer alert label widens it");
        s.cfg.notch_auto_size = false;
        let fixed = s.size().0;
        s.capture = None;
        assert_eq!(s.size().0, fixed, "without auto size the width stays put");
        // Mini notch keeps the notch height but drops the notch gap and the Large inset.
        s.cfg.notch_mini = true;
        assert_eq!(s.size().1, island(Config { notch_mode: true, ..Config::default() }).size().1);
        s.mode = Mode::Large;
        assert_eq!(s.size(), large);
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
        assert_eq!((rt.state.mode, rt.state.tab, rt.state.dropping), (Mode::Large, Tab::Shelf, true));
        rt.drop_files(vec!["/a".into(), "/b".into()], at);
        rt.frame(2.0 / 60.0);
        assert_eq!((rt.state.mode, rt.state.dropping), (Mode::Large, false));
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
        let mut s = island(Config { hidden_tabs: vec![Tab::Workflows], default_tab: Tab::Search, ..Config::default() });
        assert_eq!(s.tab, Tab::Search);
        s.move_tab(-1);
        assert_eq!(s.tab, Tab::Search);
        s.move_tab(1);
        assert_eq!(s.tab, Tab::Overview);
    }

    #[test]
    fn arrows_switch_tabs_even_while_typing_in_the_search_field() {
        let mut s = island(Config { default_tab: Tab::Search, ..Config::default() });
        s.mode = Mode::Large;
        s.search.focused = true;
        let mut rt = rune::Runtime::new(s, view, CANVAS, rune::Env { system_dark: true, scale: 1.0 });
        rt.frame(0.0);
        rt.key(KeyEvent { key: Key::Text("a".into()), mods: rune::Mods::default() });
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.search.text, "a");
        rt.key(KeyEvent { key: Key::Right, mods: rune::Mods::default() });
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.tab, Tab::Workflows);
    }

    #[test]
    fn one_long_swipe_changes_tab_once_and_a_new_one_again() {
        // Tab 1 scrolls, and a scroll view takes a wheel before the Island's handler, so skip
        // it: the swipe is about the gesture, not about which tab happens to have a scroller.
        let mut s = island(Config { hidden_tabs: vec![Tab::Workflows], default_tab: Tab::Search, ..Config::default() });
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
        assert_eq!(rt.state.tab, Tab::Overview);
        rt.frame(0.3);
        swipe(&mut rt);
        assert_eq!(rt.state.tab, Tab::Assistant);
    }

    #[test]
    fn a_pause_within_one_swipe_does_not_change_tab_again() {
        let mut s = island(Config { hidden_tabs: vec![Tab::Workflows], default_tab: Tab::Search, ..Config::default() });
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
        assert_eq!(rt.state.tab, Tab::Overview);
        // The fingers stay down through the pause, so this is still one swipe.
        for _ in 0..20 { rt.frame(1.0 / 60.0); }
        for _ in 0..6 {
            rt.wheel_event(at(rune::GesturePhase::Moved));
            rt.frame(1.0 / 60.0);
        }
        assert_eq!(rt.state.tab, Tab::Overview);
    }

    #[test]
    fn the_wheel_scrolls_a_long_task_list() {
        let mut s = island(Config { tasks: (0..30).map(|i| crate::config::Item::new(format!("task {i}"))).collect(), ..Config::default() });
        s.mode = Mode::Large;
        s.set_tab(Tab::Tasks);
        let [left, top, w, h] = s.rect();
        let mut rt = rune::Runtime::new(s, view, CANVAS, rune::Env { system_dark: true, scale: 1.0 });
        for _ in 0..60 { rt.frame(1.0 / 60.0); }
        let before = rt.find("task-task 0-0").unwrap().1;
        rt.wheel(left + w / 2.0, top + h / 2.0, -200.0);
        for _ in 0..30 { rt.frame(1.0 / 60.0); }
        let after = rt.find("task-task 0-0").unwrap().1;
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
