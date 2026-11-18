//! Everything the user can change, saved as one JSON value in Rune's prefs file
//! (`~/.config/Ripple/prefs.json`, `%APPDATA%\Ripple`, `~/Library/Application Support/Ripple`).

use std::sync::{Arc, Mutex};

use rune::Color;
use rune::prefs::Prefs;
use rune::tok::{Family, TextStyle};
use rune_kit::{Kit, PaletteSeed};
use serde::{Deserialize, Deserializer, Serialize};

const APP_ID: &str = "Ripple";

/// Declares `Tab` and `Tab::ALL` from one list, in the default order.
macro_rules! tabs {
    ($($tab:ident = $id:literal),* $(,)?) => {
        /// Large Mode's tabs. The numbers are what `prefs.json` stores: never reuse one (3 was
        /// Now Playing, 7 was Settings).
        #[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
        #[serde(into = "u8", try_from = "u8")]
        pub enum Tab { $($tab = $id),* }

        impl Tab {
            pub const ALL: [Tab; [$($id),*].len()] = [$(Tab::$tab),*];
        }
    };
}

tabs! {
    Search = 0,
    Workflows = 1,
    Overview = 2,
    Assistant = 4,
    Clipboard = 5,
    Tasks = 6,
    T3 = 8,
    Notes = 9,
    Stremflix = 10,
    Shelf = 11,
    System = 12,
    Calendar = 13,
    Limits = 14,
}

impl Tab {

    pub fn name(self) -> &'static str {
        match self {
            Tab::Search => "Browser Search",
            Tab::Workflows => "Workflows & QA",
            Tab::Overview => "Overview",
            Tab::Assistant => "AI Assistant",
            Tab::Clipboard => "Clipboard",
            Tab::Tasks => "Tasks",
            Tab::T3 => "T3 Code",
            Tab::Notes => "Notes",
            Tab::Stremflix => "Stremflix",
            Tab::Shelf => "Shelf",
            Tab::System => "System",
            Tab::Calendar => "Calendar",
            Tab::Limits => "AI Limits",
        }
    }
}

impl From<Tab> for u8 {
    fn from(tab: Tab) -> u8 { tab as u8 }
}

impl TryFrom<u8> for Tab {
    type Error = String;
    fn try_from(id: u8) -> Result<Tab, String> {
        Tab::ALL.into_iter().find(|t| *t as u8 == id).ok_or_else(|| format!("no tab {id}"))
    }
}

/// A saved list of tab ids, without the ones this version no longer has.
pub fn known_tabs<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Tab>, D::Error> {
    Ok(Vec::<u8>::deserialize(d)?.into_iter().filter_map(|id| Tab::try_from(id).ok()).collect())
}

fn known_tab<'de, D: Deserializer<'de>>(d: D) -> Result<Tab, D::Error> {
    Ok(Tab::try_from(u8::deserialize(d)?).unwrap_or(Config::default().default_tab))
}

/// (value, label, Island colour, text colour). A theme is a preset for the two colours; they
/// stay editable afterwards.
pub const THEMES: [(&str, &str, &str, &str); 4] = [
    ("none", "Default", "#000000", "#FFFFFF"),
    ("sleek-black", "Sleek Black", "rgba(0, 0, 0, 0.64)", "rgba(255, 255, 255)"),
    ("win95", "Windows 95", "rgba(195, 195, 195)", "rgba(0, 0, 0)"),
    ("glass", "Glass", "rgba(10, 10, 10, 0.92)", "rgba(255, 255, 255, 0.85)"),
];

/// An OpenAI-compatible chat completions service.
pub struct Provider {
    pub id: &'static str,
    pub label: &'static str,
    pub url: &'static str,
    pub model: &'static str,
    pub key_hint: &'static str,
}

pub const PROVIDERS: [Provider; 2] = [
    Provider { id: "groq", label: "Groq", url: "https://api.groq.com/openai/v1", model: "llama-3.3-70b-versatile", key_hint: "gsk_..." },
    Provider { id: "openrouter", label: "OpenRouter", url: "https://openrouter.ai/api/v1", model: "meta-llama/llama-3.3-70b-instruct", key_hint: "sk-or-..." },
];

pub const SEARCH_ENGINES: [(&str, &str, &str); 5] = [
    ("google", "Google", "https://www.google.com/search?q="),
    ("duckduckgo", "DuckDuckGo", "https://duckduckgo.com/?q="),
    ("brave", "Brave Search", "https://search.brave.com/search?q="),
    ("bing", "Bing", "https://www.bing.com/search?q="),
    ("startpage", "Startpage", "https://www.startpage.com/do/search?q="),
];

pub const POSITIONS: [(&str, &str); 7] = [
    ("top-left", "Top left"),
    ("top-center", "Top center"),
    ("top-right", "Top right"),
    ("bottom-left", "Bottom left"),
    ("bottom-center", "Bottom center"),
    ("bottom-right", "Bottom right"),
    ("free", "Free placement"),
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
    #[serde(deserialize_with = "known_tabs")]
    pub tab_order: Vec<Tab>,
    #[serde(deserialize_with = "known_tabs")]
    pub hidden_tabs: Vec<Tab>,
    #[serde(deserialize_with = "known_tab")]
    pub default_tab: Tab,
    /// Large Mode opens on the starred tab each time, not the last one used.
    pub open_default_tab: bool,
    /// Seconds Large Mode stays open after the pointer leaves, once its tab was changed.
    pub tab_linger: f32,
    pub battery_alerts: bool,
    pub island_border: bool,
    pub standby: bool,
    pub large_standby: bool,
    pub hide_inactive: bool,
    /// Wayland layer-shell: sit over panels (KDE Plasma) instead of below them.
    pub overlap_panels: bool,
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
    /// Notch Mode: shrink the compact Island to fit its content instead of a fixed width.
    pub notch_auto_size: bool,
    /// Notch Mode without a physical notch: the notch look, sized like the floating Island.
    pub notch_mini: bool,
    /// Free placement: the Island's centre, in percent of the display's width.
    pub island_x: f32,
    /// Free placement: points from the display's top.
    pub island_y: f32,
    /// A display's name; empty means the primary display.
    pub display: String,
    pub quick_apps: Vec<QuickApp>,
    pub workflows: Vec<Workflow>,
    pub tasks: Vec<String>,
    /// Each note's first line is its title.
    pub notes: Vec<String>,
    /// Files dropped on the Island, newest first.
    pub shelf: Vec<std::path::PathBuf>,
    pub ai_provider: String,
    pub ai_model: String,
    pub api_key: String,
    pub t3_devices: Vec<T3Device>,
    /// AI Limits: more Claude Code or Codex config folders, one per account.
    pub agent_dirs: Vec<String>,
    /// AI Limits: CLIProxyAPI's auth folder; empty means its default, `~/.cli-proxy-api`.
    pub cliproxy_dir: String,
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
            tab_order: Tab::ALL.into(),
            hidden_tabs: Vec::new(),
            default_tab: Tab::Overview,
            open_default_tab: true,
            tab_linger: 1.0,
            battery_alerts: true,
            island_border: false,
            standby: true,
            large_standby: false,
            hide_inactive: false,
            overlap_panels: false,
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
            notch_auto_size: true,
            notch_mini: false,
            island_x: 50.0,
            island_y: 20.0,
            display: String::new(),
            quick_apps: ["Notes", "Spotify", "Calculator", "Terminal"].map(app).into(),
            workflows: Vec::new(),
            tasks: Vec::new(),
            notes: Vec::new(),
            shelf: Vec::new(),
            ai_provider: "groq".into(),
            ai_model: PROVIDERS[0].model.into(),
            api_key: String::new(),
            t3_devices: Vec::new(),
            agent_dirs: Vec::new(),
            cliproxy_dir: String::new(),
            t3_device: 0,
            t3_env: String::new(),
            t3_project: String::new(),
            t3_model: String::new(),
            t3_worktree: false,
            new_user: true,
        }
    }
}

impl Config {
    /// Repairs what a hand-edited or older file can get wrong.
    fn normalized(mut self) -> Self {
        let mut seen = Vec::new();
        self.tab_order.retain(|t| !seen.contains(t) && { seen.push(*t); true });
        // Tabs added after the order was saved go at the end.
        for t in Tab::ALL {
            if !self.tab_order.contains(&t) { self.tab_order.push(t) }
        }
        if self.hidden_tabs.len() >= Tab::ALL.len() { self.hidden_tabs.clear() }
        self.island_x = self.island_x.clamp(0.0, 100.0);
        self.island_y = self.island_y.clamp(0.0, 1000.0);
        self.tab_linger = self.tab_linger.clamp(0.0, 10.0);
        if !SEARCH_ENGINES.iter().any(|e| e.0 == self.search_engine) { self.search_engine = "google".into() }
        if !PROVIDERS.iter().any(|p| p.id == self.ai_provider) { self.ai_provider = PROVIDERS[0].id.into() }
        self
    }

    pub fn visible_tabs(&self) -> Vec<Tab> {
        self.tab_order.iter().copied().filter(|t| !self.hidden_tabs.contains(t)).collect()
    }

    pub fn first_tab(&self) -> Tab {
        let visible = self.visible_tabs();
        if visible.contains(&self.default_tab) { self.default_tab } else { visible.first().copied().unwrap_or(Tab::Search) }
    }

    pub fn move_tab(&mut self, from: usize, to: usize) {
        if from >= self.tab_order.len() || to >= self.tab_order.len() { return }
        let id = self.tab_order.remove(from);
        self.tab_order.insert(to, id);
    }

    /// Hiding the last visible tab is refused.
    pub fn toggle_tab(&mut self, tab: Tab) {
        if let Some(i) = self.hidden_tabs.iter().position(|t| *t == tab) {
            self.hidden_tabs.remove(i);
        } else if self.hidden_tabs.len() + 1 < Tab::ALL.len() {
            self.hidden_tabs.push(tab);
        }
    }

    pub fn set_theme(&mut self, theme: &str) {
        let (_, _, bg, text) = THEMES.iter().find(|t| t.0 == theme).unwrap_or(&THEMES[0]);
        self.theme = theme.into();
        self.bg_color = (*bg).into();
        self.text_color = (*text).into();
    }

    pub fn win95(&self) -> bool { self.theme == "win95" }
    pub fn glass(&self) -> bool { self.theme == "glass" }
    pub fn bg(&self) -> Color { parse_color(&self.bg_color).unwrap_or(Color::BLACK) }
    pub fn text(&self) -> Color { parse_color(&self.text_color).unwrap_or(Color::WHITE) }

    /// A corner radius in this theme: Windows 95 has none.
    pub fn radius(&self, r: f32) -> f32 { if self.win95() { 0.0 } else { r } }

    pub fn font(&self, size: f32, weight: f32) -> TextStyle {
        let style = TextStyle::new(size, weight);
        if self.win95() { style.family(Family::Named("W95FA")) } else { style }
    }

    /// rune-kit's components (selects, fields) in the theme's two colours.
    pub fn kit(&self) -> Kit {
        let (bg, fg) = (self.bg().alpha(1.0), self.text().alpha(1.0));
        let mut kit = Kit::from_palette(PaletteSeed { bg, fg, accent: fg });
        kit.radius.md = 10.0;
        // The accent is the text colour, so an on switch's knob (and a radio's dot) needs the background to show.
        kit.c.knob_on = bg;
        kit
    }

    pub fn provider(&self) -> &'static Provider {
        PROVIDERS.iter().find(|p| p.id == self.ai_provider).unwrap_or(&PROVIDERS[0])
    }

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
        let saved = prefs.get::<Config>("settings");
        // First launch after the Electron app: its settings live in the same folder.
        let imported = saved.is_none().then(|| prefs.path().and_then(|p| p.parent()).and_then(crate::migrate::electron_settings)).flatten();
        let store = Store(Arc::new(Mutex::new(Inner { cfg: saved.or(imported.clone()).unwrap_or_default().normalized(), prefs })));
        if imported.is_some() { store.update(|_| {}); }
        store
    }

    #[cfg(test)]
    pub fn memory(cfg: Config) -> Store {
        Store(Arc::new(Mutex::new(Inner { cfg: cfg.normalized(), prefs: Prefs::memory() })))
    }

    pub fn get(&self) -> Config {
        self.0.lock().unwrap().cfg.clone()
    }

    /// Changes the settings, saves them and returns them. A failed save keeps the change in memory.
    pub fn update(&self, f: impl FnOnce(&mut Config)) -> Config {
        let mut inner = self.0.lock().unwrap();
        f(&mut inner.cfg);
        let Inner { cfg, prefs } = &mut *inner;
        if let Err(e) = prefs.set("settings", cfg) { eprintln!("Ripple: {e}") }
        // The file holds the AI key and T3 tokens: the folder is the user's alone, as Electron's was.
        #[cfg(unix)]
        if let Some(dir) = prefs.path().and_then(|p| p.parent()) {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        }
        inner.cfg.clone()
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
        assert_eq!(parse_color("rgb(0 0 0 / 0.5)").map(|c| c.a), Some(0.5));
        assert_eq!(parse_color("blue"), None);
        assert_eq!(parse_color("#12"), None);
    }

    #[test]
    fn saved_orders_drop_retired_tabs_and_gain_new_ones() {
        use Tab::*;
        let cfg = serde_json::from_str::<Config>(r#"{"tab_order": [7, 2, 3, 0], "hidden_tabs": [3, 0], "default_tab": 7}"#).unwrap().normalized();
        assert_eq!(cfg.tab_order, [Overview, Search, Workflows, Assistant, Clipboard, Tasks, T3, Notes, Stremflix, Shelf, System, Calendar, Limits]);
        assert_eq!(cfg.hidden_tabs, [Search]);
        assert_eq!(cfg.first_tab(), Overview);
    }

    #[test]
    fn the_last_visible_tab_cannot_be_hidden() {
        let mut cfg = Config::default();
        for t in Tab::ALL { cfg.toggle_tab(t) }
        assert_eq!(cfg.visible_tabs(), [Tab::Limits]);
        cfg.toggle_tab(Tab::Search);
        assert_eq!(cfg.visible_tabs(), [Tab::Search, Tab::Limits]);
    }

    #[cfg(unix)]
    #[test]
    fn the_settings_folder_is_readable_by_its_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("ripple-prefs-{}", std::process::id()));
        let store = Store(Arc::new(Mutex::new(Inner { cfg: Config::default(), prefs: Prefs::at(dir.join("prefs.json")) })));
        store.update(|c| c.api_key = "secret".into());
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(mode, 0o700);
    }

    #[test]
    fn old_files_fill_missing_fields_with_defaults() {
        let cfg: Config = serde_json::from_str(r#"{"standby": false}"#).unwrap();
        assert!(!cfg.standby && cfg.battery_alerts && cfg.default_tab == Tab::Overview);
        assert!(serde_json::to_string(&Config::default()).unwrap().contains(r#""default_tab":2"#), "tabs save as their numbers");
    }
}
