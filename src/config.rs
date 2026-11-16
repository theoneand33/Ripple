//! Everything the user can change, saved as one JSON value in Rune's prefs file
//! (`~/.config/Ripple/prefs.json`, `%APPDATA%\Ripple`, `~/Library/Application Support/Ripple`).

use std::sync::{Arc, Mutex};

use rune::Color;
use rune::prefs::Prefs;
use serde::{Deserialize, Serialize};

pub const APP_ID: &str = "Ripple";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TabDef {
    pub id: u8,
    pub name: &'static str,
}

/// Ids are stable: saved tab orders refer to them (3 was Now Playing, 7 was Settings).
pub const TABS: [TabDef; 7] = [
    TabDef { id: 0, name: "Browser Search" },
    TabDef { id: 1, name: "Workflows & QA" },
    TabDef { id: 2, name: "Overview" },
    TabDef { id: 4, name: "AI Assistant" },
    TabDef { id: 5, name: "Clipboard" },
    TabDef { id: 6, name: "Tasks" },
    TabDef { id: 8, name: "T3 Code" },
];

pub const SEARCH_ENGINES: [(&str, &str, &str); 5] = [
    ("google", "Google", "https://www.google.com/search?q="),
    ("duckduckgo", "DuckDuckGo", "https://duckduckgo.com/?q="),
    ("brave", "Brave Search", "https://search.brave.com/search?q="),
    ("bing", "Bing", "https://www.bing.com/search?q="),
    ("startpage", "Startpage", "https://www.startpage.com/do/search?q="),
];

pub const POSITIONS: [(&str, &str); 6] = [
    ("top-left", "Top left"),
    ("top-center", "Top center"),
    ("top-right", "Top right"),
    ("bottom-left", "Bottom left"),
    ("bottom-center", "Bottom center"),
    ("bottom-right", "Bottom right"),
];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QuickApp {
    pub name: String,
    pub launch: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Workflow {
    pub name: String,
    pub urls: Vec<String>,
}

/// One T3 server (local or remote); `token` comes from `t3 auth session issue` on that machine.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct T3Device {
    pub name: String,
    pub url: String,
    pub token: String,
}

impl T3Device {
    pub fn label(&self) -> &str {
        if self.name.is_empty() { &self.url } else { &self.name }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub tab_order: Vec<u8>,
    pub hidden_tabs: Vec<u8>,
    pub default_tab: u8,
    pub battery_alerts: bool,
    pub island_border: bool,
    pub standby: bool,
    pub large_standby: bool,
    pub hide_inactive: bool,
    pub show_info_idle: bool,
    pub hour12: bool,
    pub weather_unit: String,
    pub location: String,
    pub theme: String,
    pub bg_color: String,
    pub text_color: String,
    pub bg_image: String,
    pub search_engine: String,
    pub auto_launch: bool,
    pub position_mode: String,
    pub notch_mode: bool,
    /// Free placement: the Island's centre, in percent of the display's width.
    pub island_x: f32,
    /// Free placement: points from the display's top.
    pub island_y: f32,
    /// A display's name; empty means the primary display.
    pub display: String,
    pub quick_apps: Vec<QuickApp>,
    pub workflows: Vec<Workflow>,
    pub tasks: Vec<String>,
    pub ai_provider: String,
    pub ai_model: String,
    pub api_key: String,
    pub t3_devices: Vec<T3Device>,
    pub t3_device: usize,
    /// "" = the device itself, otherwise the id of one of its linked environments.
    pub t3_env: String,
    pub t3_project: String,
    pub t3_model: String,
    pub t3_worktree: bool,
    /// The first launch opens the instructions page once.
    pub new_user: bool,
}

impl Default for Config {
    fn default() -> Self {
        let app = |n: &str| QuickApp { name: n.into(), launch: n.into() };
        Config {
            tab_order: TABS.iter().map(|t| t.id).collect(),
            hidden_tabs: Vec::new(),
            default_tab: 2,
            battery_alerts: true,
            island_border: false,
            standby: true,
            large_standby: false,
            hide_inactive: false,
            show_info_idle: false,
            hour12: true,
            weather_unit: "f".into(),
            location: String::new(),
            theme: "none".into(),
            bg_color: "#000000".into(),
            text_color: "#FFFFFF".into(),
            bg_image: "none".into(),
            search_engine: "google".into(),
            auto_launch: false,
            position_mode: "free".into(),
            notch_mode: false,
            island_x: 50.0,
            island_y: 20.0,
            display: String::new(),
            quick_apps: ["Notes", "Spotify", "Calculator", "Terminal"].map(app).into(),
            workflows: Vec::new(),
            tasks: Vec::new(),
            ai_provider: "groq".into(),
            ai_model: default_model("groq").into(),
            api_key: String::new(),
            t3_devices: Vec::new(),
            t3_device: 0,
            t3_env: String::new(),
            t3_project: String::new(),
            t3_model: String::new(),
            t3_worktree: false,
            new_user: true,
        }
    }
}

pub fn default_model(provider: &str) -> &'static str {
    if provider == "groq" { "llama-3.3-70b-versatile" } else { "meta-llama/llama-3.3-70b-instruct" }
}

impl Config {
    /// Repairs what a hand-edited or older file can get wrong.
    fn normalized(mut self) -> Self {
        let known = |id: &u8| TABS.iter().any(|t| t.id == *id);
        let mut seen = Vec::new();
        self.tab_order.retain(|id| known(id) && !seen.contains(id) && { seen.push(*id); true });
        // Tabs added after the order was saved go at the end.
        for t in TABS {
            if !self.tab_order.contains(&t.id) { self.tab_order.push(t.id) }
        }
        self.hidden_tabs.retain(known);
        if self.hidden_tabs.len() >= TABS.len() { self.hidden_tabs.clear() }
        self.island_x = self.island_x.clamp(0.0, 100.0);
        self.island_y = self.island_y.clamp(0.0, 1000.0);
        if !SEARCH_ENGINES.iter().any(|e| e.0 == self.search_engine) { self.search_engine = "google".into() }
        self
    }

    pub fn visible_tabs(&self) -> Vec<u8> {
        self.tab_order.iter().copied().filter(|id| !self.hidden_tabs.contains(id)).collect()
    }

    pub fn first_tab(&self) -> u8 {
        let visible = self.visible_tabs();
        if visible.contains(&self.default_tab) { self.default_tab } else { visible.first().copied().unwrap_or(0) }
    }

    pub fn move_tab(&mut self, from: usize, to: usize) {
        if from >= self.tab_order.len() || to >= self.tab_order.len() { return }
        let id = self.tab_order.remove(from);
        self.tab_order.insert(to, id);
    }

    /// Hiding the last visible tab is refused.
    pub fn toggle_tab(&mut self, id: u8) {
        if let Some(i) = self.hidden_tabs.iter().position(|t| *t == id) {
            self.hidden_tabs.remove(i);
        } else if self.hidden_tabs.len() + 1 < TABS.len() {
            self.hidden_tabs.push(id);
        }
    }

    /// A theme is a preset for the two colours; they stay editable afterwards.
    pub fn set_theme(&mut self, theme: &str) {
        let (bg, text) = match theme {
            "sleek-black" => ("rgba(0, 0, 0, 0.64)", "rgba(255, 255, 255)"),
            "win95" => ("rgba(195, 195, 195)", "rgba(0, 0, 0)"),
            _ => ("#000000", "#FFFFFF"),
        };
        self.theme = theme.into();
        self.bg_color = bg.into();
        self.text_color = text.into();
    }

    pub fn win95(&self) -> bool { self.theme == "win95" }
    pub fn bg(&self) -> Color { parse_color(&self.bg_color).unwrap_or(Color::BLACK) }
    pub fn text(&self) -> Color { parse_color(&self.text_color).unwrap_or(Color::WHITE) }

    pub fn search_engine(&self) -> (&'static str, &'static str) {
        let e = SEARCH_ENGINES.iter().find(|e| e.0 == self.search_engine).unwrap_or(&SEARCH_ENGINES[0]);
        (e.1, e.2)
    }
}

/// CSS colours as the settings fields take them: `#rgb`, `#rrggbb`, `#rrggbbaa`, `rgb()`, `rgba()`.
pub fn parse_color(s: &str) -> Option<Color> {
    let s = s.trim();
    let srgb = |r: f32, g: f32, b: f32, a: f32| Color {
        r: rune::color::srgb_decode(r / 255.0),
        g: rune::color::srgb_decode(g / 255.0),
        b: rune::color::srgb_decode(b / 255.0),
        a: a.clamp(0.0, 1.0),
    };
    if let Some(hex) = s.strip_prefix('#') {
        if !hex.is_ascii() { return None }
        let wide: String = match hex.len() {
            3 | 4 => hex.chars().flat_map(|c| [c, c]).collect(),
            6 | 8 => hex.into(),
            _ => return None,
        };
        let byte = |i: usize| u8::from_str_radix(&wide[i..i + 2], 16).ok().map(f32::from);
        let a = if wide.len() == 8 { byte(6)? / 255.0 } else { 1.0 };
        return Some(srgb(byte(0)?, byte(2)?, byte(4)?, a));
    }
    let lower = s.to_ascii_lowercase();
    let body = lower.strip_prefix("rgba").or_else(|| lower.strip_prefix("rgb"))?.trim().strip_prefix('(')?.strip_suffix(')')?;
    let parts: Vec<f32> = body.split([',', '/', ' ']).filter(|p| !p.is_empty()).map(|p| p.trim().parse().ok()).collect::<Option<_>>()?;
    match parts[..] {
        [r, g, b] => Some(srgb(r, g, b, 1.0)),
        [r, g, b, a] => Some(srgb(r, g, b, a)),
        _ => None,
    }
}

struct Inner {
    cfg: Config,
    prefs: Prefs,
}

/// The one copy of the settings, shared by the Island and the Settings window.
#[derive(Clone)]
pub struct Store(Arc<Mutex<Inner>>);

impl Store {
    /// Headless runs (snapshots, tests) keep nothing on disk.
    pub fn open() -> Store {
        let prefs = if rune::is_headless() { Prefs::memory() } else { Prefs::open(APP_ID) };
        let cfg = prefs.get::<Config>("settings").unwrap_or_default().normalized();
        Store(Arc::new(Mutex::new(Inner { cfg, prefs })))
    }

    #[cfg(test)]
    pub fn memory(cfg: Config) -> Store {
        Store(Arc::new(Mutex::new(Inner { cfg: cfg.normalized(), prefs: Prefs::memory() })))
    }

    pub fn get(&self) -> Config {
        self.0.lock().unwrap().cfg.clone()
    }

    /// Changes the settings and saves them. A failed save keeps the change in memory.
    pub fn update(&self, f: impl FnOnce(&mut Config)) {
        let mut inner = self.0.lock().unwrap();
        f(&mut inner.cfg);
        let Inner { cfg, prefs } = &mut *inner;
        if let Err(e) = prefs.set("settings", cfg) { eprintln!("Ripple: {e}") }
    }

    /// Where Rune keeps this app's files, for the T3 token cache and the app cache.
    pub fn dir(&self) -> Option<std::path::PathBuf> {
        self.0.lock().unwrap().prefs.path().and_then(|p| p.parent()).map(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_parse_as_css_writes_them() {
        assert_eq!(parse_color("#000"), Some(Color::BLACK));
        assert_eq!(parse_color("#FFFFFF"), Some(Color::WHITE));
        assert_eq!(parse_color("rgba(0, 0, 0, 0.64)").map(|c| c.a), Some(0.64));
        assert_eq!(parse_color("rgba(255, 255, 255)"), Some(Color::WHITE));
        assert_eq!(parse_color("#ffffff80").map(|c| (c.a * 255.0).round()), Some(128.0));
        assert_eq!(parse_color("blue"), None);
        assert_eq!(parse_color("#12"), None);
    }

    #[test]
    fn saved_orders_drop_retired_tabs_and_gain_new_ones() {
        let cfg = Config { tab_order: vec![7, 2, 3, 0], hidden_tabs: vec![3, 0], ..Config::default() }.normalized();
        assert_eq!(cfg.tab_order, [2, 0, 1, 4, 5, 6, 8]);
        assert_eq!(cfg.hidden_tabs, [0]);
        assert_eq!(cfg.visible_tabs(), [2, 1, 4, 5, 6, 8]);
    }

    #[test]
    fn the_last_visible_tab_cannot_be_hidden() {
        let mut cfg = Config::default();
        for t in TABS { cfg.toggle_tab(t.id) }
        assert_eq!(cfg.visible_tabs(), [8]);
        cfg.toggle_tab(0);
        assert_eq!(cfg.visible_tabs(), [0, 8]);
    }

    #[test]
    fn old_files_fill_missing_fields_with_defaults() {
        let cfg: Config = serde_json::from_str(r#"{"standby": false}"#).unwrap();
        assert!(!cfg.standby && cfg.battery_alerts && cfg.default_tab == 2);
    }
}
