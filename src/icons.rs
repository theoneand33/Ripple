//! Lucide icons (ISC licence) from `lucide-static-svg` as Rune icon bodies: 24-unit view box, drawn in the element's colour.

use lucide_static_svg::Icon as Lucide;
use rune::Icon;

macro_rules! lucide {
    ($name:ident, $icon:ident) => {
        pub static $name: Icon = Icon { name: Lucide::$icon.name(), view: 24.0, body: Lucide::$icon.svg_str() };
    };
}

lucide!(ACTIVITY, Activity);
lucide!(CPU, Cpu);
lucide!(MEMORY, MemoryStick);
lucide!(HARD_DRIVE, HardDrive);
lucide!(GLOBE, Globe);
lucide!(BOT, Bot);
lucide!(GAUGE, Gauge);
lucide!(PUZZLE, Puzzle);
lucide!(ROTATE_CCW, RotateCcw);
lucide!(TIMER, Timer);
lucide!(CALCULATOR, Calculator);
lucide!(COFFEE, Coffee);
lucide!(FLAME, Flame);
lucide!(PENCIL, Pencil);
lucide!(MAXIMIZE, Maximize2);
lucide!(TRENDING_UP, TrendingUp);
lucide!(TYPE, TypeIcon);
lucide!(DROPLET, Droplet);
lucide!(MINUS, Minus);
lucide!(PIPETTE, Pipette);
lucide!(COPY, Copy);
lucide!(CLOCK, Clock);
lucide!(MOON, Moon);
lucide!(SUNRISE, Sunrise);
lucide!(SUNSET, Sunset);
lucide!(MIC_OFF, MicOff);
lucide!(HEADPHONE_OFF, HeadphoneOff);
lucide!(PHONE_OFF, PhoneOff);
lucide!(WIFI, Wifi);
lucide!(BLUETOOTH, Bluetooth);
lucide!(LOCK, Lock);
lucide!(POWER, Power);
lucide!(BATTERY, Battery);

/// Filled, as the playback controls and the default-tab star draw: Lucide has no filled variants.
macro_rules! solid {
    ($name:ident, $id:literal, $body:literal) => {
        pub static $name: Icon = Icon {
            name: $id,
            view: 24.0,
            body: concat!(r#"<g fill="currentColor" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">"#, $body, "</g>"),
        };
    };
}

lucide!(SEARCH, Search);
lucide!(ZAP, Zap);
lucide!(SUN, Sun);
lucide!(MIC, Mic);
lucide!(LIST, List);
lucide!(CHECK, Check);
lucide!(STICKY_NOTE, StickyNote);
lucide!(TV, Tv);
lucide!(TERMINAL, Terminal);
lucide!(CAMERA, Camera);
lucide!(HEADPHONES, Headphones);
lucide!(MUSIC, EighthNotes);
lucide!(SETTINGS, Settings);
lucide!(CLOUD, Cloud);
lucide!(CLOUD_RAIN, CloudRain);
lucide!(CLOUD_SNOW, CloudSnow);
lucide!(CLOUD_LIGHTNING, CloudLightning);
lucide!(CLOUD_SUN, CloudSun);
lucide!(TRASH, Trash);
lucide!(CHEVRON_UP, ChevronUp);
lucide!(CHEVRON_RIGHT, ChevronRight);
lucide!(CALENDAR, Calendar);
lucide!(CHEVRON_LEFT, ChevronLeft);
lucide!(CHEVRON_DOWN, ChevronDown);
lucide!(CLIPBOARD_PASTE, ClipboardPaste);
lucide!(INBOX, Inbox);
lucide!(DOWNLOAD, Download);
lucide!(FILE, File);
lucide!(X, X);
lucide!(PLUS, Plus);
lucide!(EYE, Eye);
lucide!(EYE_OFF, EyeOff);
lucide!(GRIP, GripVertical);
lucide!(MONITOR, Monitor);
lucide!(STAR, Star);
solid!(STAR_FILLED, "star-filled", r#"<path d="M11.525 2.295a.53.53 0 0 1 .95 0l2.31 4.679a2.123 2.123 0 0 0 1.595 1.16l5.166.756a.53.53 0 0 1 .294.904l-3.736 3.638a2.123 2.123 0 0 0-.611 1.878l.882 5.14a.53.53 0 0 1-.771.56l-4.618-2.428a2.122 2.122 0 0 0-1.973 0L6.396 21.01a.53.53 0 0 1-.77-.56l.881-5.139a2.122 2.122 0 0 0-.611-1.879L2.16 9.795a.53.53 0 0 1 .294-.906l5.165-.755a2.122 2.122 0 0 0 1.597-1.16z"/>"#);
solid!(PLAY, "play", r#"<polygon points="6 3 20 12 6 21 6 3"/>"#);
solid!(PAUSE, "pause", r#"<rect x="14" y="4" width="4" height="16" rx="1"/><rect x="6" y="4" width="4" height="16" rx="1"/>"#);
solid!(SKIP_BACK, "skip-back", r#"<polygon points="19 20 9 12 19 4 19 20"/><line x1="5" x2="5" y1="19" y2="5"/>"#);
solid!(SKIP_FORWARD, "skip-forward", r#"<polygon points="5 4 15 12 5 20 5 4"/><line x1="19" x2="19" y1="5" y2="19"/>"#);

/// The icon for a WMO weather code; the sun while there is none.
pub fn weather(code: Option<u64>) -> &'static Icon {
    match code {
        Some(1 | 2) => &CLOUD_SUN,
        Some(3 | 45 | 48) => &CLOUD,
        Some(51..=57 | 61..=67 | 80..=82) => &CLOUD_RAIN,
        Some(71..=77 | 85 | 86) => &CLOUD_SNOW,
        Some(95..=99) => &CLOUD_LIGHTNING,
        _ => &SUN,
    }
}

lucide!(VOLUME, Volume2);
lucide!(VOLUME_X, VolumeX);

lucide!(BELL, Bell);
lucide!(CAPS_LOCK, ArrowBigUpDash);
lucide!(GAMEPAD, Gamepad2);

/// A ring filled clockwise from the top to `percent`, over a faint full track.
pub fn ring(percent: u8) -> &'static Icon {
    // Rune caches icons by address: one static per percent, built once.
    static RINGS: std::sync::OnceLock<Vec<Icon>> = std::sync::OnceLock::new();
    let rings = RINGS.get_or_init(|| (0..=100u8).map(|p| {
        let track = r#"<circle cx="12" cy="12" r="10.5" stroke-opacity="0.25"/>"#;
        let arc = match p {
            0 => String::new(),
            100 => r#"<circle cx="12" cy="12" r="10.5"/>"#.into(),
            _ => {
                let a = f32::from(p) / 100.0 * std::f32::consts::TAU;
                format!(r#"<path d="M12 1.5A10.5 10.5 0 {} 1 {:.3} {:.3}"/>"#, u8::from(p > 50), 12.0 + 10.5 * a.sin(), 12.0 - 10.5 * a.cos())
            }
        };
        let body = format!(r#"<g fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round">{track}{arc}</g>"#);
        Icon { name: "ring", view: 24.0, body: Box::leak(body.into_boxed_str()) }
    }).collect());
    &rings[usize::from(percent.min(100))]
}

/// Every icon above, so a plugin can name one (`ui.icon("timer")`). An icon added above belongs here too.
static ALL: &[&Icon] = &[&ACTIVITY, &CPU, &MEMORY, &HARD_DRIVE, &GLOBE, &BOT, &GAUGE, &PUZZLE, &ROTATE_CCW, &TIMER, &CALCULATOR, &COFFEE, &FLAME, &PENCIL, &MAXIMIZE, &TRENDING_UP, &TYPE, &DROPLET, &MINUS, &PIPETTE, &COPY, &CLOCK, &MOON, &SUNRISE, &SUNSET, &MIC_OFF, &HEADPHONE_OFF, &PHONE_OFF, &WIFI, &BLUETOOTH, &LOCK, &POWER, &BATTERY, &SEARCH, &ZAP, &SUN, &MIC, &LIST, &CHECK, &STICKY_NOTE, &TV, &TERMINAL, &CAMERA, &HEADPHONES, &MUSIC, &SETTINGS, &CLOUD, &CLOUD_RAIN, &CLOUD_SNOW, &CLOUD_LIGHTNING, &CLOUD_SUN, &TRASH, &CHEVRON_UP, &CHEVRON_RIGHT, &CALENDAR, &CHEVRON_LEFT, &CHEVRON_DOWN, &CLIPBOARD_PASTE, &INBOX, &DOWNLOAD, &FILE, &X, &PLUS, &EYE, &EYE_OFF, &GRIP, &MONITOR, &STAR, &STAR_FILLED, &PLAY, &PAUSE, &SKIP_BACK, &SKIP_FORWARD, &VOLUME, &VOLUME_X, &BELL, &CAPS_LOCK];

/// The icon with this Lucide name.
pub fn named(name: &str) -> Option<&'static Icon> {
    ALL.iter().copied().find(|i| i.name == name)
}
