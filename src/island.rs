//! The Island: its state, the rules that move it between Still, Quick and Large, the alerts,
//! and the compact views. The Large mode's tabs are in `tabs.rs`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;

use rune::time::LocalTime;
use rune::tok::{Family, TextStyle};
use rune::{
    AppHandle, Color, Cx, Display, DisplayTarget, Drag, Easing, Edge, GesturePhase, El, Image, ImageCache, Key, KeyEvent, LoopSpec, OverlayHandle, OverlayPlacement,
    Phase, Shadow, Spring, SurfaceFit, TextEdit, Transition, Tx, Waker, WheelEvent, WindowHandle, el, icon, row, text,
};
use rune_kit::{Kit, PaletteSeed};

use crate::config::{Config, Store};
use crate::net::Weather;
use crate::platform::Passthrough;
use crate::system::{self, MediaCommand, Track};
use crate::{icons, stremflix, t3, tabs};

/// The Island's window, in points: the largest Island at its hover scale, its tab dock, plus their shadows.
pub const CANVAS: (f32, f32) = (560.0, 420.0);
/// Room kept around the Island inside the window for its shadow and hover scale.
const PAD: f32 = 30.0;
/// The tab dock under the Large Island: its height, the gap above it, one button and the space between.
const DOCK: (f32, f32, f32, f32) = (44.0, 12.0, 32.0, 6.0);
/// Framer's spring (stiffness 400, damping 40, mass 2.5) as duration and bounce.
pub const SPRING: Spring = Spring::new(0.5, 0.3);

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

/// One display as the window factory saw it at startup.
#[derive(Clone, Debug)]
pub struct Screen {
    pub name: String,
    pub target: DisplayTarget,
    /// Size in points.
    pub size: (f32, f32),
    pub scale: f32,
    /// Top-left in the OS's global space: points on macOS, pixels elsewhere.
    #[cfg_attr(target_os = "linux", allow(dead_code))]
    pub origin: (f64, f64),
    pub primary: bool,
}

impl Screen {
    pub fn all(displays: &[Display]) -> Vec<Screen> {
        displays.iter().enumerate().map(|(i, d)| {
            let scale = d.scale.max(0.1) as f32;
            let bounds = d.native_bounds();
            Screen {
                name: d.name.clone().filter(|n| !n.is_empty()).unwrap_or_else(|| format!("Display {}", i + 1)),
                target: d.id.clone().map_or(DisplayTarget::Primary, DisplayTarget::Id),
                size: (d.size.0 as f32 / scale, d.size.1 as f32 / scale),
                scale,
                origin: (bounds[0], bounds[1]),
                primary: d.primary,
            }
        }).collect()
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

#[derive(Default)]
pub struct StremflixState {
    /// Continue Watching, or why it couldn't load; None while loading.
    pub entries: Option<Result<Vec<stremflix::Entry>, String>>,
    /// Loaded posters by URL.
    pub posters: std::collections::HashMap<String, Image>,
    /// A play request that failed.
    pub status: String,
    /// Bumped on every load, so an earlier answer is dropped.
    generation: u64,
}

#[derive(Default)]
pub struct T3State {
    pub data: Option<t3::Loaded>,
    pub projects: Option<Vec<t3::Project>>,
    pub status: String,
    pub message: TextEdit,
    /// The select that is open: "device", "folder" or "model".
    pub open: Option<&'static str>,
    /// Bumped on every load, so an answer for an earlier device or environment is dropped.
    pub generation: u64,
    /// A thread launch is in flight.
    pub starting: bool,
}

pub struct Island {
    pub store: Store,
    pub cfg: Config,
    pub kit: Kit,
    pub mode: Mode,
    pub tab: u8,
    /// The pointer is over the Island or its dock.
    pub hovered: bool,
    /// Over the Island, over the dock.
    over: [bool; 2],
    /// The pointer left both; the next poll lets the Island rest.
    left: bool,

    pub time: String,
    pub date: String,
    pub percent: u8,
    pub charging: bool,
    pub weather: Weather,
    pub track: Option<Track>,
    pub art: Option<Image>,
    pub art_hovered: bool,
    /// The pointer is over an open select's menu.
    pub menu_hovered: bool,
    pub background: Option<Image>,
    bluetooth: bool,
    pub camera: bool,
    pub microphone: bool,

    battery_alert_until: Option<f64>,
    charging_alert_until: Option<f64>,
    bluetooth_alert_until: Option<f64>,
    capture: Option<(Alert, f64)>,
    capture_queue: Vec<Alert>,
    capture_shown: [bool; 2],
    paused_until: Option<f64>,

    pub search: TextEdit,
    pub ask: TextEdit,
    pub asked: bool,
    pub answer: Option<String>,
    ai_generation: u64,
    pub clipboard: Vec<String>,
    /// The clipboard row whose button reads "Copied", and until when.
    pub copied: Option<(usize, f64)>,
    pub task: TextEdit,
    /// The open note's editor and which note it edits.
    pub note: TextEdit,
    pub note_index: usize,
    pub stremflix: StremflixState,
    pub t3: T3State,
    pub t3_client: Arc<t3::Client>,

    pub me: Option<Host>,
    pub settings: Option<WindowHandle>,
    pub images: Option<ImageCache>,
    pub screens: Vec<Screen>,
    pub placement: Placement,
    passthrough: Passthrough,
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
    next_second: f64,
    next_clipboard: f64,
    intro_at: Option<f64>,
}

/// How wide a single line of text draws, roughly: the compact Island sizes itself to its
/// track title before anything is laid out.
fn estimate_width(text: &str, size: f32) -> f32 {
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
        let t3_client = Arc::new(t3::Client::new(store.dir().map(|d| d.join("t3-mcp-tokens.json"))));
        let mut island = Island {
            kit: kit(&cfg),
            mode: Mode::Still,
            tab: cfg.first_tab(),
            hovered: false,
            over: [false; 2],
            left: false,
            time: String::new(),
            date: String::new(),
            percent: 100,
            charging: false,
            weather: Weather::default(),
            track: None,
            art: None,
            art_hovered: false,
            menu_hovered: false,
            background: None,
            bluetooth: false,
            camera: false,
            microphone: false,
            battery_alert_until: None,
            charging_alert_until: None,
            bluetooth_alert_until: None,
            capture: None,
            capture_queue: Vec::new(),
            capture_shown: [false; 2],
            paused_until: None,
            search: TextEdit::new(""),
            ask: TextEdit::new(""),
            asked: false,
            answer: None,
            ai_generation: 0,
            clipboard: Vec::new(),
            copied: None,
            task: TextEdit::new(""),
            note: TextEdit::new(cfg.notes.first().map_or("", String::as_str)),
            note_index: 0,
            stremflix: StremflixState::default(),
            t3: T3State::default(),
            t3_client,
            me: None,
            settings: None,
            images: None,
            placement: Placement::of(&cfg, (1920.0, 1080.0)),
            screens,
            passthrough: Passthrough::default(),
            weather_kick: None,
            wheel: 0.0,
            wheel_reset: 0.0,
            wheel_spent: false,
            wheel_phased: false,
            tab_dir: 1,
            swipe_lock_until: 0.0,
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
            .unwrap_or(Screen { name: String::new(), target: DisplayTarget::Primary, size: (1920.0, 1080.0), scale: 1.0, origin: (0.0, 0.0), primary: true })
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

    pub fn alert(&self) -> Option<Alert> {
        if self.charging_alert_until.is_some() { Some(Alert::Charging) }
        else if self.battery_alert_until.is_some() { Some(Alert::Battery) }
        else if self.bluetooth_alert_until.is_some() { Some(Alert::Bluetooth) }
        else { self.capture.map(|c| c.0) }
    }

    fn now_playing_text(&self) -> (String, String) {
        let Some(t) = &self.track else { return Default::default() };
        (t.name.clone(), if t.artist.is_empty() { String::new() } else { format!(" • {}", t.artist) })
    }

    fn now_playing_width(&self) -> f32 {
        let (name, artist) = self.now_playing_text();
        (estimate_width(&name, 13.0) + estimate_width(&artist, 13.0) + 24.0 + 6.0 + 20.0).ceil().clamp(122.0, 300.0)
    }

    pub fn notch_inset(&self) -> f32 { if self.cfg.notch_mode { 34.0 } else { 0.0 } }

    pub fn size(&self) -> (f32, f32) {
        let notch = self.cfg.notch_mode;
        let alerting = self.alert().is_some();
        let track = self.shows_track();
        let mut w = match self.mode {
            Mode::Large => match self.tab { 1 | 9 | 10 => 480.0, 0 => 405.0, _ => 380.0 },
            _ if alerting => if notch { 260.0 } else { 160.0 }, // fits icon + "Microphone"
            _ if track => self.now_playing_width(),
            Mode::Quick => if notch { 260.0 } else { 140.0 }, // fits "23:59" + weather/"100%"
            Mode::Still => if notch { 170.0 } else { 140.0 },
        };
        if notch && self.mode != Mode::Large { w = (w + 40.0).max(if track { 300.0 } else { 0.0 }) }
        let h = match self.mode {
            Mode::Large => self.notch_inset() + match self.tab { 6 | 8 | 9 | 10 => 250.0, 0 => 120.0, 1 => 210.0, _ => 190.0 },
            _ => if notch { 34.0 } else { 40.0 },
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

    pub fn font(&self, size: f32, weight: f32) -> TextStyle { font(&self.cfg, size, weight) }

    // --- Modes ---

    /// Where the Island rests when nothing holds it open.
    pub fn rest(&self) -> Mode {
        if self.cfg.standby { Mode::Quick } else if self.cfg.large_standby { Mode::Large } else { Mode::Still }
    }

    pub fn input_focused(&self) -> bool {
        self.search.focused || self.ask.focused || self.task.focused || self.note.focused || self.t3.message.focused
    }

    fn set_mode(&mut self, mode: Mode) {
        self.mode = if mode == Mode::Still { self.rest() } else { mode };
        if self.mode != Mode::Large { self.t3.open = None }
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
            // Each show makes a new layer surface, so this is asked again on every entry.
            if let Some(me) = &self.me { me.allow_keys() }
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
        if !std::mem::take(&mut self.left) || self.hovered { return false }
        // An open select's menu hangs outside the Island; leaving for it isn't leaving (poll closes both).
        if self.t3.open.is_none() {
            tx.blur();
            self.set_mode(Mode::Still);
        }
        true
    }

    /// A click on the Island itself: Large opens and closes.
    pub fn toggle(&mut self, tx: &mut Tx) {
        tx.blur();
        self.set_mode(if self.mode == Mode::Large { Mode::Quick } else { Mode::Large });
        self.entered_tab();
    }

    pub fn window_focus(&mut self, focused: bool, tx: &mut Tx) {
        if focused || self.hovered { return }
        tx.blur();
        self.t3.open = None;
        self.set_mode(Mode::Still);
    }

    /// Switches to `tab`, animating in the direction it sits from the current one.
    pub fn set_tab(&mut self, tab: u8) {
        let visible = self.cfg.visible_tabs();
        let at = |t: u8| visible.iter().position(|v| *v == t);
        if let (Some(from), Some(to)) = (at(self.tab), at(tab))
            && from != to
        {
            self.tab_dir = if to > from { 1 } else { -1 };
        }
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
    pub fn open_tab(&mut self, id: u8) {
        self.set_tab(id);
        self.entered_tab();
    }

    fn entered_tab(&mut self) {
        self.t3.open = None;
        if self.mode == Mode::Large && self.tab == 8 { self.t3_load() }
        if self.mode == Mode::Large && self.tab == 10 { self.stremflix_load() }
    }

    // --- Stremflix ---

    /// Asks Stremflix-rs for Continue Watching, then fetches the posters not yet loaded.
    pub fn stremflix_load(&mut self) {
        let Some(me) = self.me.clone() else { return };
        let Some(base) = stremflix::base() else {
            self.stremflix.entries = Some(Err("Stremflix-rs's API is off (STREMFLIX_RS_API=off)".into()));
            return;
        };
        // The last list stays up while it refreshes.
        if self.stremflix.entries.as_ref().is_some_and(Result::is_err) { self.stremflix.entries = None }
        self.stremflix.status.clear();
        self.stremflix.generation += 1;
        let generation = self.stremflix.generation;
        let cached: Vec<String> = self.stremflix.posters.keys().cloned().collect();
        std::thread::spawn(move || {
            let result = stremflix::continue_watching(&base);
            // Posters come from addons: only the web, never file: or data: URLs.
            let posters: Vec<String> = result.iter().flatten().filter_map(|e| e.poster.clone())
                .filter(|p| (p.starts_with("https://") || p.starts_with("http://")) && !cached.contains(p)).collect();
            me.update(move |s, _| if s.stremflix.generation == generation { s.stremflix.entries = Some(result) });
            for url in posters {
                let Some(bytes) = crate::net::fetch_image(&url) else { continue };
                me.update(move |s, _| if let Some(image) = s.images.as_ref().map(|i| i.load(bytes)) { s.stremflix.posters.insert(url, image); });
            }
        });
    }

    /// Resumes an entry in Stremflix-rs's window.
    pub fn stremflix_play(&mut self, entry: stremflix::Entry) {
        let (Some(base), Some(me)) = (stremflix::base(), self.me.clone()) else { return };
        self.stremflix.status = format!("Opening {}...", entry.title);
        std::thread::spawn(move || {
            let result = stremflix::play(&base, &entry);
            me.update(move |s, _| s.stremflix.status = result.err().unwrap_or_default());
        });
    }

    // --- Notes ---

    /// Edits note `i`, or the last one when `i` is past the end.
    pub fn open_note(&mut self, i: usize) {
        self.note_index = i.min(self.cfg.notes.len().saturating_sub(1));
        self.note = TextEdit::new(self.cfg.notes.get(self.note_index).map_or("", String::as_str));
    }

    pub fn add_note(&mut self) {
        // An empty note on top is reused rather than stacking blanks.
        if self.cfg.notes.first().is_none_or(|n| !n.trim().is_empty()) { self.save(|c| c.notes.insert(0, String::new())) }
        self.open_note(0);
        self.note.focused = true;
    }

    pub fn delete_note(&mut self) {
        let i = self.note_index;
        self.save(|c| { if i < c.notes.len() { c.notes.remove(i); } });
        self.open_note(i);
    }

    /// Saves the editor's text into its note as it changes.
    pub fn note_edited(&mut self) {
        let (i, text) = (self.note_index, self.note.text.clone());
        if self.cfg.notes.get(i).is_some_and(|n| *n != text) { self.save(|c| c.notes[i] = text) }
    }

    pub fn key(&mut self, ev: &KeyEvent, _: &mut Tx) -> bool {
        match ev.key {
            Key::Right => self.move_tab(1),
            Key::Left => self.move_tab(-1),
            Key::Command(c @ '1'..='8') => {
                let Some(&id) = self.cfg.visible_tabs().get(c as usize - '1' as usize) else { return false };
                self.mode = Mode::Large;
                self.open_tab(id);
            }
            _ => return false,
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
    pub fn reload(&mut self, _: &mut Tx) {
        let old = std::mem::replace(&mut self.cfg, self.store.get());
        self.kit = kit(&self.cfg);
        if !self.cfg.visible_tabs().contains(&self.tab) { self.set_tab(self.cfg.first_tab()) }
        if self.mode != Mode::Large || !self.hovered { self.mode = self.rest() }
        let moved = (old.position_mode != self.cfg.position_mode, old.notch_mode != self.cfg.notch_mode, old.island_x != self.cfg.island_x, old.island_y != self.cfg.island_y, old.display != self.cfg.display);
        if moved != (false, false, false, false, false) { self.place_window() }
        if (old.location != self.cfg.location || old.weather_unit != self.cfg.weather_unit)
            && let Some(kick) = &self.weather_kick
        {
            let _ = kick.send(());
        }
        if old.bg_image != self.cfg.bg_image { self.load_background() }
        if old.hour12 != self.cfg.hour12 { self.set_clock() }
        if old.t3_devices != self.cfg.t3_devices && self.tab == 8 && self.mode == Mode::Large { self.t3_load() }
    }

    /// Changes the settings from the Island itself (tasks, the T3 choices).
    pub fn save(&mut self, f: impl FnOnce(&mut Config)) {
        self.store.update(f);
        self.cfg = self.store.get();
    }

    fn load_background(&mut self) {
        self.background = None;
        let url = self.cfg.bg_image.trim().to_string();
        let Some(me) = self.me.clone() else { return };
        if url.is_empty() || url == "none" { return }
        std::thread::spawn(move || {
            let Some(bytes) = crate::net::fetch_image(&url) else { return };
            me.update(move |s, _| {
                if s.cfg.bg_image.trim() == url { s.background = s.images.as_ref().map(|i| i.load(bytes)) }
            });
        });
    }

    // --- The clock ---

    fn set_clock(&mut self) {
        let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
        // A headless capture reads the same clock on every machine.
        let now = if rune::is_headless() { LocalTime::at_offset(1_767_260_460_000, 0) } else { LocalTime::at(ms) };
        let hour = if self.cfg.hour12 { (now.hour + 11) % 12 + 1 } else { now.hour };
        self.time = format!("{hour}:{:02}", now.minute);
        self.date = format_date(now.year, now.month, now.day);
    }

    // --- What the pollers report ---

    pub fn set_battery(&mut self, percent: u8, charging: bool, tx: &mut Tx) -> bool {
        if (percent, charging) == (self.percent, self.charging) { return false }
        let now = tx.clock();
        if charging && !self.charging && self.cfg.battery_alerts {
            self.mode = Mode::Quick;
            self.charging_alert_until = Some(now + 1.5);
        }
        if percent != self.percent && [20, 15, 10, 5, 3].contains(&percent) && self.cfg.battery_alerts {
            self.mode = Mode::Quick;
            self.battery_alert_until = Some(now + 3.0);
        }
        (self.percent, self.charging) = (percent, charging);
        self.schedule(tx);
        true
    }

    pub fn set_bluetooth(&mut self, connected: bool, tx: &mut Tx) -> bool {
        if connected == self.bluetooth { return false }
        self.bluetooth = connected;
        if connected {
            self.mode = Mode::Quick;
            self.bluetooth_alert_until = Some(tx.clock() + 3.0);
            self.schedule(tx);
        }
        connected
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
        self.mode = Mode::Quick;
        self.capture = Some((self.capture_queue.remove(0), tx.clock() + 3.0));
        self.schedule(tx);
    }

    pub fn set_track(&mut self, track: Option<Track>, tx: &mut Tx) -> bool {
        if track == self.track { return false }
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
                std::thread::spawn(move || {
                    let Some(bytes) = crate::net::fetch_image(&url) else { return };
                    me.update(move |s, _| {
                        // The track may have changed while the cover loaded.
                        if s.track.as_ref().and_then(|t| t.artwork_url.as_deref()) == Some(url.as_str()) {
                            s.art = s.images.as_ref().map(|i| i.load(bytes));
                        }
                    });
                });
            }
        }
        true
    }

    pub fn push_clipboard(&mut self, text: &str) -> bool {
        if text.is_empty() || self.clipboard.first().is_some_and(|t| t == text) { return false }
        self.clipboard.retain(|t| t != text);
        self.clipboard.insert(0, text.to_string());
        self.clipboard.truncate(50);
        self.copied = None;
        true
    }

    // --- Timers ---

    /// Asks for a poll at the next deadline any timer holds.
    fn schedule(&self, tx: &mut Tx) {
        let deadlines = [self.battery_alert_until, self.charging_alert_until, self.bluetooth_alert_until, self.capture.map(|c| c.1), self.paused_until, self.copied.map(|c| c.1), self.intro_at];
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
        }
        if now >= self.next_clipboard && !rune::is_headless() {
            self.next_clipboard = now + 2.0;
            tx.wake_at(self.next_clipboard);
            tx.read_clipboard(|s: &mut Island, content, _| { s.push_clipboard(content.text()); });
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
        changed |= self.leave(tx);
        changed |= due(&mut self.paused_until);
        if self.copied.is_some_and(|c| now >= c.1) {
            self.copied = None;
            changed = true;
        }
        if due(&mut self.intro_at) {
            system::open_external("https://github.com/TopMyster/Ripple/blob/main/instructions.md");
            self.save(|c| c.new_user = false);
        }
        self.schedule(tx);

        // Leaving both the Island and an open select's menu closes them.
        if self.t3.open.is_none() { self.menu_hovered = false }
        else if !self.hovered && !self.menu_hovered {
            tx.blur();
            self.t3.open = None;
            self.set_mode(Mode::Still);
            changed = true;
        }

        // A select's menu opens outside the Island: the whole window takes clicks while one is open.
        // The hover scale (1.05) draws the Island slightly past its layout rect.
        let region = if self.t3.open.is_some() { [0.0, 0.0, CANVAS.0, CANVAS.1] } else {
            let [x, y, w, h] = self.rect();
            let (mx, my) = (w * 0.025 + 2.0, h * 0.025 + 2.0);
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
        self.load_background();
        if self.cfg.auto_launch { std::thread::spawn(|| system::set_auto_launch(true)); }

        fn every(seconds: u64, f: impl Fn() + Send + 'static) {
            std::thread::spawn(move || loop {
                f();
                std::thread::sleep(std::time::Duration::from_secs(seconds));
            });
        }
        let h = me.clone();
        every(2, move || { let t = system::media(); h.update_if(move |s, tx| s.set_track(t, tx)); });
        let h = me.clone();
        every(10, move || { let b = system::bluetooth_connected(); h.update_if(move |s, tx| s.set_bluetooth(b, tx)); });
        let h = me.clone();
        every(3, move || { let c = system::camera_in_use(); h.update_if(move |s, tx| s.set_capture(Some(c), None, tx)); });
        let h = me.clone();
        every(3, move || { let m = system::microphone_in_use(); h.update_if(move |s, tx| s.set_capture(None, Some(m), tx)); });
        let h = me.clone();
        every(5, move || { let (p, c) = system::battery(); h.update_if(move |s, tx| s.set_battery(p, c, tx)); });

        // Weather: every 10 minutes, and at once when the location or unit changes.
        let (kick, kicked) = std::sync::mpsc::channel();
        self.weather_kick = Some(kick);
        let store = self.store.clone();
        std::thread::spawn(move || loop {
            let cfg = store.get();
            if let Some(w) = crate::net::weather(&cfg.location, &cfg.weather_unit) {
                me.update_if(move |s, _| s.weather != w && { s.weather = w; true });
            }
            if kicked.recv_timeout(std::time::Duration::from_secs(600)) == Err(std::sync::mpsc::RecvTimeoutError::Disconnected) { break }
        });
    }

    pub fn set_window(&mut self, window: WindowHandle) {
        self.passthrough.set_window(window);
    }

    // --- The AI assistant ---

    pub fn ask_ai(&mut self) {
        self.asked = true;
        self.ai_generation += 1;
        let generation = self.ai_generation;
        let key = self.cfg.api_key.trim().to_string();
        if key.is_empty() {
            self.answer = Some("Enter your API key in settings".into());
            return;
        }
        self.answer = None;
        let Some(me) = self.me.clone() else { return };
        let (provider, model, question) = (self.cfg.ai_provider.clone(), self.cfg.ai_model.clone(), self.ask.text.clone());
        std::thread::spawn(move || {
            let model = if model.trim().is_empty() { crate::config::default_model(&provider).to_string() } else { model };
            let stream = me.clone();
            let result = crate::net::ask_ai(&provider, &model, &key, &question, |delta| {
                let delta = delta.to_string();
                stream.update(move |s, _| if s.ai_generation == generation { s.answer.get_or_insert_default().push_str(&delta) });
            });
            let ending = match result {
                Ok(true) => return,
                Ok(false) => "No response received. Check your settings.".to_string(),
                Err(e) => format!("Error: {e}"),
            };
            me.update(move |s, _| if s.ai_generation == generation { s.answer = Some(ending) });
        });
    }

    pub fn ask_another(&mut self) {
        self.ai_generation += 1;
        self.asked = false;
        self.answer = None;
        self.ask.clear();
    }

    // --- Browser search ---

    pub fn search_browser(&mut self, tx: &mut Tx) {
        let query = self.search.text.trim().to_string();
        if query.is_empty() { return }
        let lower = query.to_ascii_lowercase();
        let url = if !query.contains('.') { format!("{}{}", self.cfg.search_engine().1, crate::net::percent_encode(&query)) }
            else if lower.starts_with("http://") || lower.starts_with("https://") { query }
            else { format!("https://{query}") };
        system::open_external(&url);
        self.search.clear();
        tx.blur();
        self.set_mode(Mode::Still);
    }

    // --- T3 Code ---

    pub fn t3_device(&self) -> Option<(usize, &crate::config::T3Device)> {
        let devices = &self.cfg.t3_devices;
        devices.get(self.cfg.t3_device).map(|d| (self.cfg.t3_device, d)).or(devices.first().map(|d| (0, d)))
    }

    /// The chosen environment, if the device still links it; "" is the device itself.
    pub fn t3_env(&self) -> &str {
        let linked = self.t3.data.as_ref().is_some_and(|d| d.links.iter().any(|l| l.id == self.cfg.t3_env));
        if linked { &self.cfg.t3_env } else { "" }
    }

    pub fn t3_project(&self) -> Option<&t3::Project> {
        let projects = self.t3.projects.as_ref()?;
        projects.iter().find(|p| p.id == self.cfg.t3_project).or(projects.first())
    }

    pub fn t3_model(&self) -> Option<&t3::Favorite> {
        let favorites = &self.t3.data.as_ref()?.favorites;
        favorites.iter().find(|f| f.key() == self.cfg.t3_model).or(favorites.first())
    }

    /// Asks the device for its models and linked environments, then for its folders.
    pub fn t3_load(&mut self) {
        self.t3.generation += 1;
        self.t3.data = None;
        self.t3.projects = None;
        let (Some(device), Some(me)) = (self.t3_device().map(|d| d.1.clone()), self.me.clone()) else { return };
        self.t3.status = "Loading...".into();
        let (client, generation) = (self.t3_client.clone(), self.t3.generation);
        std::thread::spawn(move || {
            let result = client.load(&device);
            me.update(move |s, _| {
                if s.t3.generation != generation { return }
                match result {
                    Ok(data) => {
                        s.t3.data = Some(data);
                        s.t3_load_projects();
                    }
                    Err(e) => s.t3.status = e,
                }
            });
        });
    }

    pub fn t3_load_projects(&mut self) {
        self.t3.generation += 1;
        self.t3.projects = None;
        let (Some(device), Some(me)) = (self.t3_device().map(|d| d.1.clone()), self.me.clone()) else { return };
        self.t3.status = "Loading folders...".into();
        let (client, generation, env) = (self.t3_client.clone(), self.t3.generation, self.t3_env().to_string());
        std::thread::spawn(move || {
            let result = client.projects(&device, &env);
            me.update(move |s, _| {
                if s.t3.generation != generation { return }
                match result {
                    Ok(list) => {
                        s.t3.projects = Some(list);
                        s.t3.status.clear();
                    }
                    Err(e) => s.t3.status = e,
                }
            });
        });
    }

    pub fn t3_can_start(&self) -> bool {
        !self.t3.starting && self.t3_project().is_some() && self.t3_model().is_some() && !self.t3.message.text.trim().is_empty()
    }

    pub fn t3_start(&mut self) {
        let (Some((_, device)), Some(project), Some(model), Some(me)) = (self.t3_device(), self.t3_project(), self.t3_model(), self.me.clone()) else { return };
        // One launch at a time: a second Ctrl+Return or click while starting does nothing.
        if self.t3.message.text.trim().is_empty() || self.t3.starting { return }
        let launch = t3::Launch {
            environment_id: self.t3_env().to_string(),
            project_id: project.id.clone(),
            instance_id: model.instance_id.clone(),
            model: model.model.clone(),
            message: self.t3.message.text.clone(),
            worktree: self.cfg.t3_worktree,
            runtime_mode: self.t3.data.as_ref().map(|d| d.runtime_mode.clone()).unwrap_or_default(),
        };
        let (device, client, title) = (device.clone(), self.t3_client.clone(), project.title.clone());
        self.t3.status = "Starting...".into();
        self.t3.starting = true;
        std::thread::spawn(move || {
            let result = client.start_thread(&device, &launch);
            me.update(move |s, _| { s.t3.starting = false; match result {
                Ok(_) => {
                    s.t3.message.clear();
                    s.t3.status = format!("Thread started in {title}");
                }
                Err(e) => s.t3.status = e,
            } });
        });
    }
}

/// "Thu, Jan 1".
fn format_date(year: i32, month: u32, day: u32) -> String {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    // Sakamoto's day-of-week.
    const T: [i32; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let y = if month < 3 { year - 1 } else { year };
    let weekday = (y + y / 4 - y / 100 + y / 400 + T[(month as usize - 1) % 12] + day as i32).rem_euclid(7);
    format!("{}, {} {}", DAYS[weekday as usize], MONTHS[(month as usize - 1) % 12], day)
}

pub fn font(cfg: &Config, size: f32, weight: f32) -> TextStyle {
    let style = TextStyle::new(size, weight);
    if cfg.win95() { style.family(Family::Named("W95FA")) } else { style }
}

/// rune-kit's components (selects, fields) in the Island's own two colours.
pub fn kit(cfg: &Config) -> Kit {
    let (bg, fg) = (cfg.bg().alpha(1.0), cfg.text().alpha(1.0));
    let mut kit = Kit::from_palette(PaletteSeed { bg, fg, accent: fg });
    kit.radius.md = 10.0;
    kit
}

// --- Views ---

pub fn view(s: &Island, _: &mut Cx<Island>) -> El {
    let cfg = &s.cfg;
    let mut layer = Vec::new();
    let [left, top, w, h] = s.rect();
    let hidden = cfg.hide_inactive && s.mode == Mode::Still;
    let (bg, fg) = if hidden { (Color::TRANSPARENT, Color::TRANSPARENT) } else { (cfg.bg(), cfg.text()) };
    let large = s.mode == Mode::Large;
    let radii = if cfg.win95() { [0.0; 4] }
        else if cfg.notch_mode { let r = if large { 44.0 } else { h / 2.0 }; [0.0, 0.0, r, r] }
        else { [if large { 44.0 } else { h / 2.0 }; 4] };

    let mut island = el().key("island").role("group").label("Ripple").hit()
        .absolute(left, top).size(w, h)
        .fill(bg).corners(radii).clip_corners(radii)
        .motion(SPRING)
        .scale(if s.hovered { 1.05 } else { 1.0 })
        .on_hover_change(Island::hover)
        .on_drag(Island::drag)
        .on_wheel(Island::wheel);
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
        let push = Transition::push(if s.tab_dir < 0 { Edge::Leading } else { Edge::Trailing });
        island = island.child(
            el().key("tabs").pin(Some(0.0), Some(inset), Some(0.0), Some(0.0))
                .transition(Transition::blur_replace())
                .child_exit(push)
                .child(
                    el().key(format!("tab-{}", s.tab)).pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
                        .transition(push)
                        .child(tabs::view(s, &mut layer)),
                ),
        );
        // The open select's menu is the layer's last element.
        if s.t3.open.is_some() && let Some(menu) = layer.pop() {
            layer.push(menu.on_hover_change(|s: &mut Island, hovered, _| s.menu_hovered = hovered));
        }
    } else if s.mode == Mode::Quick || s.alert().is_some() || s.shows_track() || cfg.show_info_idle {
        island = island.child(if s.shows_track() && s.alert().is_none() { now_playing(s, fg) } else { glance(s, fg) });
    }

    // The border draws over the content, as a CSS border does.
    island = island.child(border(s, radii, hidden));
    let mut canvas = el().key("canvas").w_full().h_full().child(island);
    // A select's menu opens outside the Island: the whole overlay takes clicks while one is open.
    if s.t3.open.is_some() { canvas = canvas.hit() }
    if let Some(rect) = s.dock_rect() { canvas = canvas.child(dock(s, rect)) }
    rune_kit::root(canvas, layer)
}

/// The Large Island's tabs as a row of icons in a pill of their own.
fn dock(s: &Island, [x, y, w, h]: [f32; 4]) -> El {
    let cfg = &s.cfg;
    let (bg, fg) = (cfg.bg(), cfg.text());
    let (height, gap, button, space) = DOCK;
    let round = |r: f32| if cfg.win95() { 0.0 } else { r };
    let mut pill = row().key("dock").role("group").label("Tabs").w_full().h(height).px(space).gap(space).center_y()
        .fill(bg).radius(round(height / 2.0)).border(1.0, fg.alpha(0.12 * fg.a))
        .box_shadow(Shadow { x: 0.0, y: 0.0, blur: 24.0, spread: 0.0, color: Color::BLACK.alpha(0.2), inset: false });
    for id in cfg.visible_tabs() {
        let name = crate::config::TABS.iter().find(|t| t.id == id).map_or("", |t| t.name);
        let on = id == s.tab;
        pill = pill.child(
            el().key(format!("dock-{id}")).role("button").label(name).checked(on).focusable(true)
                .size(button, button).center().radius(round(button / 2.0))
                .fill(if on { fg.alpha(0.18 * fg.a) } else { Color::TRANSPARENT })
                .hover(fg.alpha(0.1 * fg.a)).press(fg.alpha(0.2 * fg.a))
                .child(icon(icons::tab(id), 17.0).color(if on { fg } else { fg.alpha(0.55 * fg.a) }).a11y_hidden())
                .on_press(move |s: &mut Island| s.open_tab(id)),
        );
    }
    // The gap to the Island is part of the dock, so crossing it keeps the Island open.
    let zone = el().key("dock-zone").hit().absolute(x, y).size(w, h).motion(SPRING)
        .transition(Transition::blur_replace())
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
        else if s.percent <= 20 || s.alert() == Some(Alert::Battery) { status::low() }
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
    let color = match alert {
        Some(Alert::Charging) => status::charge(),
        Some(Alert::Battery) => status::low(),
        Some(Alert::Camera) => status::camera(),
        Some(Alert::Microphone) => status::mic(),
        _ => fg,
    };
    let style = s.font(16.0, 600.0).lh(16.0);
    let left = match alert {
        Some(Alert::Charging | Alert::Battery) => icon(&icons::ZAP, 20.0).color(color),
        Some(Alert::Camera) => icon(&icons::CAMERA, 20.0).color(color),
        Some(Alert::Microphone) => icon(&icons::MIC, 20.0).color(color),
        Some(Alert::Bluetooth) => icon(&icons::HEADPHONES, 20.0).color(color),
        None => text(&s.time).key("time").font(style.features("'tnum' 1")).color(color),
    };
    let percent = || text(format!("{}%", s.percent)).key("percent").font(style).color(color);
    let label = |l: &str| text(l).key("label").font(style).color(color);
    let right = match alert {
        Some(Alert::Charging | Alert::Battery) => percent(),
        Some(Alert::Camera) => label("Camera"),
        Some(Alert::Microphone) => label("Microphone"),
        Some(Alert::Bluetooth) => label("Connected"),
        None if cfg.standby => percent(),
        None => match s.weather.temp {
            Some(temp) => row().key("weather").gap(2.0).center_y()
                .child(icon(icons::weather(&s.weather.status), 14.0).color(color))
                .child(text(format!("{temp}º")).font(style).color(color)),
            None => percent(),
        },
    };
    let pad = if cfg.notch_mode { 12.0 } else { 16.0 };
    let key = match alert {
        Some(Alert::Charging) => "charging",
        Some(Alert::Battery) => "battery",
        Some(Alert::Bluetooth) => "bluetooth",
        Some(Alert::Camera) => "camera",
        Some(Alert::Microphone) => "microphone",
        None => "glance",
    };
    row().key(key).w_full().h_full().px(pad).center_y().justify_between()
        .transition(Transition::blur_replace())
        .child(left).child(right)
}

/// The compact view while music plays: cover, a title that scrolls when it doesn't fit, and
/// play/pause under the pointer.
fn now_playing(s: &Island, fg: Color) -> El {
    let Some(track) = &s.track else { return el() };
    let (name, artist) = s.now_playing_text();
    let width = estimate_width(&name, 13.0) + estimate_width(&artist, 13.0);
    let viewport = s.size().0 - 18.0 - 24.0 - 6.0 - if s.hovered { 31.0 } else { 0.0 };
    let title = |key: &str| row().key(key).pr(30.0)
        .child(text(&name).font(s.font(13.0, 600.0)).color(fg).single_line())
        .child(text(&artist).font(s.font(13.0, 400.0)).color(fg.alpha(0.7 * fg.a)).single_line());
    let scrolls = width > viewport;
    let mut strip = row().key("strip").child(title("title"));
    if scrolls {
        // Two copies, sliding left by one copy's width every 12 seconds, read as one endless line.
        strip = strip.child(title("title-again").a11y_hidden())
            .looping(LoopSpec::slide(12.0, [0.0, 0.0], [-0.5, 0.0]).easing(Easing::LINEAR));
    }
    let mut marquee = el().key("marquee").grow(1.0).min_w(0.0).h(18.0).clip_soft(0.0).child(strip.absolute(0.0, 0.0));
    if scrolls { marquee = marquee.clip_fade(0.0, 0.0, 15.0, 15.0) }

    let mut line = row().key(format!("playing-{}-{}", track.name, track.artist)).w_full().h_full().px(9.0).gap(6.0).center_y()
        .transition(Transition::blur_replace())
        .opacity(if !track.playing { 0.5 } else if s.cfg.hide_inactive { 0.6 } else { 1.0 })
        .child(cover(s, 24.0, 4.0, 14.0, 1.25))
        .child(marquee);
    if s.hovered {
        line = line.child(
            el().key("play-pause").role("button").label(if track.playing { "Pause" } else { "Play" })
                .size(25.0, 24.0).center().transition(Transition::opacity())
                .child(icon(if track.playing { &icons::PAUSE } else { &icons::PLAY }, 15.0).color(Color::WHITE))
                .on_press(|_: &mut Island| control(MediaCommand::PlayPause)),
        );
    }
    line
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

/// Media keys run a subprocess; never on the UI thread.
pub fn control(command: MediaCommand) {
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
    }

    #[test]
    fn sizes_follow_the_mode_and_the_tab() {
        let mut s = island(Config { standby: false, ..Config::default() });
        assert_eq!(s.size(), (140.0, 40.0));
        s.mode = Mode::Large;
        for (tab, size) in [(0, (405.0, 120.0)), (1, (480.0, 210.0)), (2, (380.0, 190.0)), (6, (380.0, 250.0)), (8, (380.0, 250.0)), (9, (480.0, 250.0))] {
            s.tab = tab;
            assert_eq!(s.size(), size);
        }
        s.cfg.notch_mode = true;
        assert_eq!(s.size(), (480.0, 284.0));
        s.mode = Mode::Still;
        assert_eq!(s.size(), (210.0, 34.0));
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
    fn notes_are_added_edited_and_deleted() {
        let mut s = island(Config { notes: vec!["Old".into()], ..Config::default() });
        s.add_note();
        assert_eq!((s.note_index, s.cfg.notes.clone()), (0, vec![String::new(), "Old".into()]));
        s.add_note();
        assert_eq!(s.cfg.notes.len(), 2);
        s.note.insert("Groceries");
        s.note_edited();
        assert_eq!(s.store.get().notes, ["Groceries", "Old"]);
        s.open_note(5);
        assert_eq!((s.note_index, s.note.text.as_str()), (1, "Old"));
        s.delete_note();
        assert_eq!((s.cfg.notes.clone(), s.note.text.clone()), (vec!["Groceries".to_string()], "Groceries".to_string()));
    }

    #[test]
    fn clipboard_history_keeps_the_newest_first_without_repeats() {
        let mut s = island(Config::default());
        assert!(s.push_clipboard("a") && s.push_clipboard("b"));
        assert!(!s.push_clipboard("b") && !s.push_clipboard(""));
        assert!(s.push_clipboard("a"));
        assert_eq!(s.clipboard, ["a", "b"]);
        for i in 0..60 { s.push_clipboard(&i.to_string()); }
        assert_eq!(s.clipboard.len(), 50);
    }

    #[test]
    fn tabs_move_within_the_visible_ones() {
        let mut s = island(Config { hidden_tabs: vec![1], default_tab: 0, ..Config::default() });
        assert_eq!(s.tab, 0);
        s.move_tab(-1);
        assert_eq!(s.tab, 0);
        s.move_tab(1);
        assert_eq!(s.tab, 2);
    }

    #[test]
    fn arrows_switch_tabs_even_while_typing_in_the_search_field() {
        let mut s = island(Config { default_tab: 0, ..Config::default() });
        s.mode = Mode::Large;
        s.search.focused = true;
        let mut rt = rune::Runtime::new(s, view, CANVAS, rune::Env { system_dark: true, scale: 1.0 });
        rt.frame(0.0);
        rt.key(KeyEvent { key: Key::Text("a".into()), mods: rune::Mods::default() });
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.search.text, "a");
        rt.key(KeyEvent { key: Key::Right, mods: rune::Mods::default() });
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.tab, 1);
    }

    #[test]
    fn one_long_swipe_changes_tab_once_and_a_new_one_again() {
        // Tab 1 scrolls, and a scroll view takes a wheel before the Island's handler, so skip
        // it: the swipe is about the gesture, not about which tab happens to have a scroller.
        let mut s = island(Config { hidden_tabs: vec![1], default_tab: 0, ..Config::default() });
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
        assert_eq!(rt.state.tab, 2);
        rt.frame(0.3);
        swipe(&mut rt);
        assert_eq!(rt.state.tab, 4);
    }

    #[test]
    fn a_pause_within_one_swipe_does_not_change_tab_again() {
        let mut s = island(Config { hidden_tabs: vec![1], default_tab: 0, ..Config::default() });
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
        assert_eq!(rt.state.tab, 2);
        // The fingers stay down through the pause, so this is still one swipe.
        for _ in 0..20 { rt.frame(1.0 / 60.0); }
        for _ in 0..6 {
            rt.wheel_event(at(rune::GesturePhase::Moved));
            rt.frame(1.0 / 60.0);
        }
        assert_eq!(rt.state.tab, 2);
    }

}
