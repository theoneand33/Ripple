//! Lucide icons (ISC licence) from `lucide-static-svg` as Rune icon bodies: 24-unit view box, drawn in the element's colour.

use lucide_static_svg::Icon as Lucide;
use rune::Icon;

/// Declares every icon and `ALL` from one list, so each can be named by a plugin. Solid ones
/// are filled, as the playback controls and the default-tab star draw: Lucide has no filled variants.
macro_rules! icons {
    (lucide { $($name:ident = $icon:ident),* $(,)? } solid { $($solid:ident = $id:literal, $body:literal),* $(,)? }) => {
        $(pub static $name: Icon = Icon { name: Lucide::$icon.name(), view: 24.0, body: Lucide::$icon.svg_str() };)*
        $(pub static $solid: Icon = Icon {
            name: $id,
            view: 24.0,
            body: concat!(r#"<g fill="currentColor" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">"#, $body, "</g>"),
        };)*
        /// Every icon, so a plugin can name one (`ui.icon("timer")`).
        static ALL: &[&Icon] = &[$(&$name,)* $(&$solid,)*];
    };
}

icons! {
    lucide {
        ACTIVITY = Activity,
        CPU = Cpu,
        MEMORY = MemoryStick,
        HARD_DRIVE = HardDrive,
        GLOBE = Globe,
        BOT = Bot,
        GAUGE = Gauge,
        PUZZLE = Puzzle,
        ROTATE_CCW = RotateCcw,
        TIMER = Timer,
        CALCULATOR = Calculator,
        COFFEE = Coffee,
        FLAME = Flame,
        PENCIL = Pencil,
        MAXIMIZE = Maximize2,
        TRENDING_UP = TrendingUp,
        TYPE = TypeIcon,
        DROPLET = Droplet,
        MINUS = Minus,
        PIPETTE = Pipette,
        COPY = Copy,
        CLOCK = Clock,
        MOON = Moon,
        SUNRISE = Sunrise,
        SUNSET = Sunset,
        MIC_OFF = MicOff,
        HEADPHONE_OFF = HeadphoneOff,
        PHONE_OFF = PhoneOff,
        WIFI = Wifi,
        BLUETOOTH = Bluetooth,
        LOCK = Lock,
        POWER = Power,
        BATTERY = Battery,
        SEARCH = Search,
        ZAP = Zap,
        SUN = Sun,
        MIC = Mic,
        LIST = List,
        CHECK = Check,
        STICKY_NOTE = StickyNote,
        TV = Tv,
        TERMINAL = Terminal,
        CAMERA = Camera,
        HEADPHONES = Headphones,
        MUSIC = EighthNotes,
        SETTINGS = Settings,
        CLOUD = Cloud,
        CLOUD_RAIN = CloudRain,
        CLOUD_SNOW = CloudSnow,
        CLOUD_LIGHTNING = CloudLightning,
        CLOUD_SUN = CloudSun,
        TRASH = Trash,
        CHEVRON_UP = ChevronUp,
        CHEVRON_RIGHT = ChevronRight,
        CALENDAR = Calendar,
        CHEVRON_LEFT = ChevronLeft,
        CHEVRON_DOWN = ChevronDown,
        CLIPBOARD_PASTE = ClipboardPaste,
        INBOX = Inbox,
        DOWNLOAD = Download,
        FILE = File,
        X = X,
        PLUS = Plus,
        EYE = Eye,
        EYE_OFF = EyeOff,
        GRIP = GripVertical,
        MONITOR = Monitor,
        STAR = Star,
        VOLUME = Volume2,
        VOLUME_X = VolumeX,
        BELL = Bell,
        CAPS_LOCK = ArrowBigUpDash,
        GAMEPAD = Gamepad2,
    }
    solid {
        STAR_FILLED = "star-filled", r#"<path d="M11.525 2.295a.53.53 0 0 1 .95 0l2.31 4.679a2.123 2.123 0 0 0 1.595 1.16l5.166.756a.53.53 0 0 1 .294.904l-3.736 3.638a2.123 2.123 0 0 0-.611 1.878l.882 5.14a.53.53 0 0 1-.771.56l-4.618-2.428a2.122 2.122 0 0 0-1.973 0L6.396 21.01a.53.53 0 0 1-.77-.56l.881-5.139a2.122 2.122 0 0 0-.611-1.879L2.16 9.795a.53.53 0 0 1 .294-.906l5.165-.755a2.122 2.122 0 0 0 1.597-1.16z"/>"#,
        PLAY = "play", r#"<polygon points="6 3 20 12 6 21 6 3"/>"#,
        PAUSE = "pause", r#"<rect x="14" y="4" width="4" height="16" rx="1"/><rect x="6" y="4" width="4" height="16" rx="1"/>"#,
        SKIP_BACK = "skip-back", r#"<polygon points="19 20 9 12 19 4 19 20"/><line x1="5" x2="5" y1="19" y2="5"/>"#,
        SKIP_FORWARD = "skip-forward", r#"<polygon points="5 4 15 12 5 20 5 4"/><line x1="19" x2="19" y1="5" y2="19"/>"#,
    }
}

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


/// The icon with this Lucide name.
pub fn named(name: &str) -> Option<&'static Icon> {
    ALL.iter().copied().find(|i| i.name == name)
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_plugin_can_name_every_icon() {
        assert!(super::named("gamepad-2").is_some());
        assert!(super::named("star-filled").is_some());
        let mut names: Vec<_> = super::ALL.iter().map(|i| i.name).collect();
        names.sort();
        let before = names.len();
        names.dedup();
        assert_eq!(names.len(), before, "one name, one icon");
    }
}
