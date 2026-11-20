//! Clipboard: the last 50 texts and 10 images copied anywhere, newest first, each one copyable
//! again. Pinned items stay on top and are kept across restarts; pinned images are saved as PNG
//! files in a `clipboard` folder beside prefs.json. Text tools turn the newest copied text into
//! another form (case, Base64, JSON, URL encoding, a JWT's claims, a Unix time) and copy that.

use rune::{ClipboardContent, ClipboardImage, Color, El, Image, SurfaceFit, TextEdit, Transition, Tx, col, el, row, text};

use super::{Scrolls, empty, field, pill, tool};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::config::{Config, Pin};
use crate::icons;
use crate::island::Island;

const TEXTS: usize = 50;
const IMAGES: usize = 10;

#[derive(Clone)]
pub enum Content {
    Text(String),
    /// The copied PNG, and its decoded pixels for the row (None until the window has an image cache).
    Image(ClipboardImage, Option<Image>),
}

impl PartialEq for Content {
    fn eq(&self, other: &Content) -> bool {
        match (self, other) {
            (Content::Text(a), Content::Text(b)) => a == b,
            (Content::Image(a, _), Content::Image(b, _)) => a == b,
            _ => false,
        }
    }
}

#[derive(Clone)]
pub struct Clip {
    pub content: Content,
    pub pinned: bool,
}

#[derive(Default)]
pub struct Clipboard {
    /// Pinned items first, in the order pinned; then the rest, newest first.
    pub items: Vec<Clip>,
    /// The row whose button reads "Copied", and until when.
    pub copied: Option<(usize, f64)>,
    pub search: TextEdit,
    next_read: f64,
    /// What the last read found: an unchanged clipboard is not pushed again, so a copy holding
    /// both text and an image doesn't reshuffle the list every poll.
    last_read: Option<ClipboardContent>,
    /// Where pinned images are saved; None keeps them only this run (headless).
    dir: Option<PathBuf>,
    /// The text tools show under the search.
    pub tools: bool,
    /// Why the last text tool couldn't run.
    pub tool_error: Option<String>,
}

type TextTool = fn(&str) -> Result<String, String>;

/// Each text tool's label and what it makes of a text.
pub const TOOLS: [(&str, TextTool); 10] = [
    ("UPPER", |t| Ok(t.to_uppercase())),
    ("lower", |t| Ok(t.to_lowercase())),
    ("Title", |t| Ok(title_case(t))),
    ("Base64", |t| { use base64::Engine; Ok(base64::engine::general_purpose::STANDARD.encode(t)) }),
    ("From Base64", |t| from_base64(t.trim(), false)),
    ("JSON", pretty_json),
    ("URL encode", |t| Ok(crate::net::percent_encode(t))),
    ("URL decode", |t| String::from_utf8(crate::net::percent_decode_bytes(&t.replace('+', " "))).map_err(|_| "it doesn't decode to UTF-8 text".into())),
    ("JWT", jwt),
    ("Unix time", unix_time),
];

/// Each word's first letter upper case, the rest lower.
fn title_case(t: &str) -> String {
    let mut start = true;
    t.chars().flat_map(|c| {
        let out: Vec<char> = if start { c.to_uppercase().collect() } else { c.to_lowercase().collect() };
        start = !c.is_alphanumeric() && c != '\'';
        out
    }).collect()
}

/// Base64, standard or URL-safe, padded or not, as UTF-8 text.
fn from_base64(t: &str, url: bool) -> Result<String, String> {
    use base64::Engine;
    use base64::engine::general_purpose::{STANDARD_NO_PAD, URL_SAFE_NO_PAD};
    let t = t.trim_end_matches('=');
    let bytes = if url || t.contains(['-', '_']) { URL_SAFE_NO_PAD.decode(t) } else { STANDARD_NO_PAD.decode(t) };
    String::from_utf8(bytes.map_err(|_| "Not Base64")?).map_err(|_| "Not text once decoded".into())
}

/// A JWT's header and claims, as formatted JSON; the signature isn't checked.
fn jwt(t: &str) -> Result<String, String> {
    let parts: Vec<&str> = t.trim().split('.').collect();
    let [header, claims, _] = parts[..] else { return Err("Not a JWT".into()) };
    let (header, claims) = (from_base64(header, true)?, from_base64(claims, true)?);
    // Both are JSON objects; `pretty_json` checks the rest.
    if ![&header, &claims].iter().all(|p| p.trim_start().starts_with('{')) { return Err("Not a JWT".into()) }
    pretty_json(&format!(r#"{{"header":{header},"claims":{claims}}}"#)).map_err(|_| "Not a JWT".into())
}

/// JSON indented by two spaces, its keys and numbers exactly as written: checked, not re-encoded,
/// which would sort the keys and round big numbers.
fn pretty_json(t: &str) -> Result<String, String> {
    serde_json::from_str::<serde::de::IgnoredAny>(t).map_err(|_| "Not JSON")?;
    let (mut out, mut depth, mut in_str, mut escaped) = (String::new(), 0, false, false);
    let line = |out: &mut String, depth: usize| { out.push('\n'); out.push_str(&"  ".repeat(depth)) };
    let mut chars = t.trim().chars().peekable();
    while let Some(c) = chars.next() {
        if in_str {
            out.push(c);
            (escaped, in_str) = (!escaped && c == '\\', escaped || c != '"');
            continue;
        }
        match c {
            '"' => { in_str = true; out.push(c) }
            '{' | '[' => {
                while chars.next_if(|c| c.is_whitespace()).is_some() {}
                out.push(c);
                // An empty one stays on its line.
                match chars.next_if(|c| matches!(c, '}' | ']')) { Some(end) => out.push(end), None => { depth += 1; line(&mut out, depth) } }
            }
            '}' | ']' => { depth -= 1; line(&mut out, depth); out.push(c) }
            ',' => { out.push(c); line(&mut out, depth) }
            ':' => out.push_str(": "),
            c if c.is_whitespace() => {}
            c => out.push(c),
        }
    }
    Ok(out)
}

/// Unix seconds (or milliseconds, past the year 5138) as the date and time on this clock.
fn unix_time(t: &str) -> Result<String, String> {
    let n: i64 = t.trim().parse().map_err(|_| "Not a Unix time")?;
    let ms = if n.unsigned_abs() >= 100_000_000_000 { n } else { n.saturating_mul(1000) };
    // 0001-01-01 to 9999-12-31 UTC, so the local offset never leaves four-digit years.
    if !(-62_135_596_800_000..=253_402_214_400_000).contains(&ms) { return Err("Not a Unix time".into()) }
    let l = rune::time::LocalTime::at(ms);
    let off = l.offset / 60;
    Ok(format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02} {}{:02}:{:02}", l.year, l.month, l.day, l.hour, l.minute, l.second, if off < 0 { '-' } else { '+' }, off.abs() / 60, off.abs() % 60))
}

/// A pinned image's file name: its PNG's hash, so the same image is one file.
fn file_name(image: &ClipboardImage) -> String {
    let hash: String = Sha256::digest(image.png()).iter().take(16).map(|b| format!("{b:02x}")).collect();
    format!("{hash}.png")
}

/// Only names `file_name` makes are read: the prefs file can be edited by hand.
fn load_image(dir: &Path, name: &str) -> Option<ClipboardImage> {
    let stem = name.strip_suffix(".png")?;
    if stem.is_empty() || !stem.bytes().all(|b| b.is_ascii_hexdigit()) { return None }
    ClipboardImage::from_png(std::fs::read(dir.join(name)).ok()?)
}

impl Island {
    /// Reads the clipboard every 2 s, and ends the "Copied" label.
    pub fn clipboard_poll(&mut self, now: f64, tx: &mut Tx) -> bool {
        if now >= self.clipboard.next_read && !rune::is_headless() {
            self.clipboard.next_read = now + 2.0;
            tx.wake_at(self.clipboard.next_read);
            // A copy from a spreadsheet or browser can carry both; the text goes on top.
            tx.read_clipboard(|s: &mut Island, content, _| {
                if s.clipboard.last_read.as_ref() == Some(content) { return }
                s.clipboard.last_read = Some(content.clone());
                if let Some(image) = content.image() { s.clipboard_image(image.clone()); }
                s.clipboard.push(Content::Text(content.text().to_string()));
            });
        }
        let ended = self.clipboard.copied.is_some_and(|c| now >= c.1);
        if ended { self.clipboard.copied = None }
        ended
    }

    /// Adds a copied image, decoding it only when it is new.
    pub fn clipboard_image(&mut self, image: ClipboardImage) -> bool {
        let known = self.clipboard.items.iter().any(|c| matches!(&c.content, Content::Image(i, _) if *i == image));
        let pixels = if known { None } else { self.images.as_ref().map(|cache| cache.load(image.png().to_vec())) };
        self.clipboard.push(Content::Image(image, pixels))
    }

    /// Runs a text tool on the newest copied text (`Clipboard::tool_source`), then copies its answer.
    fn run_tool(&mut self, tool: usize, tx: &mut Tx) {
        let c = &mut self.clipboard;
        let Some(source) = c.tool_source().map(str::to_string) else { c.tool_error = Some("Copy some text first".into()); return };
        match (TOOLS[tool].1)(&source) {
            Ok(out) if out.is_empty() => c.tool_error = Some(format!("{}: the result is empty", TOOLS[tool].0)),
            Ok(out) => {
                tx.copy(out.clone());
                c.push(Content::Text(out.clone()));
                // Where the copy now sits: below the pins, or a pinned text it matches.
                let at = c.items.iter().position(|i| matches!(&i.content, Content::Text(t) if *t == out)).unwrap_or(0);
                (c.tool_error, c.copied) = (None, Some((at, tx.clock() + 2.0)));
            }
            Err(e) => c.tool_error = Some(format!("{}: {e}", TOOLS[tool].0)),
        }
    }

    pub fn toggle_clip_pin(&mut self, i: usize) {
        self.clipboard.toggle_pin(i);
        let pins = self.clipboard.pins();
        if pins == self.cfg.clipboard_pins { return }
        // An image no longer pinned loses its file.
        if let Some(dir) = &self.clipboard.dir {
            for old in &self.cfg.clipboard_pins {
                if let Pin::Image { image } = old && !pins.contains(old) && load_image(dir, image).is_some() { let _ = std::fs::remove_file(dir.join(image)); }
            }
        }
        self.save(|c| c.clipboard_pins = pins);
    }

    /// Decodes the pinned images loaded at start, once the window has an image cache.
    pub fn clipboard_decode(&mut self) {
        let Some(cache) = &self.images else { return };
        for clip in &mut self.clipboard.items {
            if let Content::Image(image, pixels @ None) = &mut clip.content { *pixels = Some(cache.load(image.png().to_vec())) }
        }
    }
}

impl Clipboard {
    /// The items pinned last time; `dir` is the settings folder. An image whose file is gone is left out.
    pub fn new(cfg: &Config, dir: Option<&Path>) -> Clipboard {
        let dir = dir.map(|d| d.join("clipboard"));
        let items = cfg.clipboard_pins.iter().filter_map(|pin| Some(Clip {
            content: match pin {
                Pin::Text(t) => Content::Text(t.clone()),
                Pin::Image { image } => Content::Image(load_image(dir.as_deref()?, image)?, None),
            },
            pinned: true,
        })).collect();
        Clipboard { items, dir, ..Clipboard::default() }
    }

    fn pinned(&self) -> usize { self.items.iter().take_while(|c| c.pinned).count() }

    /// What the text tools run on: the newest copied text, the first below the pins; with none
    /// there, the first pinned text. The tools row shows it, so it is never a guess.
    pub fn tool_source(&self) -> Option<&str> {
        self.items[self.pinned()..].iter().chain(&self.items).find_map(|c| match &c.content { Content::Text(t) => Some(t.as_str()), Content::Image(..) => None })
    }

    /// Adds a copy below the pinned items; one already listed moves up there, a pinned one stays.
    /// False when nothing changed.
    pub fn push(&mut self, mut content: Content) -> bool {
        if matches!(&content, Content::Text(t) if t.is_empty()) { return false }
        let top = self.pinned();
        match self.items.iter().position(|c| c.content == content) {
            Some(i) if i <= top => return false,
            // A known image keeps the pixels it already decoded.
            Some(i) => if let (Content::Image(_, pixels @ None), Content::Image(_, Some(old))) = (&mut content, self.items.remove(i).content) { *pixels = Some(old) },
            None => {}
        }
        self.items.insert(top, Clip { content, pinned: false });
        // Unpinned texts and images each keep their newest few.
        let (mut texts, mut images) = (0, 0);
        self.items.retain(|c| {
            if c.pinned { return true }
            let n = if matches!(c.content, Content::Text(_)) { &mut texts } else { &mut images };
            *n += 1;
            *n <= if matches!(c.content, Content::Text(_)) { TEXTS } else { IMAGES }
        });
        // A tool's error was about the text before.
        (self.copied, self.tool_error) = (None, None);
        true
    }

    /// Pins an item at the end of the pinned ones, or unpins it to the top of the rest.
    pub fn toggle_pin(&mut self, i: usize) {
        if i >= self.items.len() { return }
        let mut clip = self.items.remove(i);
        clip.pinned = !clip.pinned;
        // Unpinned, it goes first below the pins, which no longer count it.
        let at = self.pinned();
        self.items.insert(at, clip);
        self.copied = None;
    }

    /// The pinned items to save, writing each pinned image's file if it isn't there yet. An
    /// image that can't be written stays pinned only this run.
    pub fn pins(&self) -> Vec<Pin> {
        self.items.iter().filter(|c| c.pinned).filter_map(|c| match &c.content {
            Content::Text(t) => Some(Pin::Text(t.clone())),
            Content::Image(image, _) => {
                let (dir, name) = (self.dir.as_ref()?, file_name(image));
                let path = dir.join(&name);
                if !path.exists() {
                    std::fs::create_dir_all(dir).and_then(|_| std::fs::write(&path, image.png())).map_err(|e| eprintln!("Ripple: could not save a pinned image: {e}")).ok()?;
                }
                Some(Pin::Image { image: name })
            }
        }).collect()
    }

    /// The items the search matches, by index; images only show with no search.
    pub fn listed(&self) -> Vec<usize> {
        let query = self.search.text.trim().to_lowercase();
        (0..self.items.len()).filter(|i| match &self.items[*i].content {
            Content::Text(t) => query.is_empty() || t.to_lowercase().contains(&query),
            Content::Image(..) => query.is_empty(),
        }).collect()
    }
}

pub fn view(s: &Island) -> El {
    let fg = s.cfg.text();
    let mut list = col().key("clipboard-list").w_full().gap(8.0);
    let listed = s.clipboard.listed();
    if s.clipboard.items.is_empty() { list = list.child(empty(s, "Clipboard is empty")) }
    else if listed.is_empty() { list = list.child(empty(s, "No matches")) }
    for i in listed {
        let clip = &s.clipboard.items[i];
        let copied = s.clipboard.copied.is_some_and(|c| c.0 == i);
        let content = clip.content.clone();
        let copy = el().key("copy").role("button").label("Copy").focusable(true)
            .px(10.0).h(24.0).center().radius(s.cfg.radius(12.0))
            .fill(if copied { Color { r: 0.03, g: 0.57, b: 0.1, a: 0.4 } } else { Color::WHITE.alpha(0.15) })
            .hover(fg.alpha(0.1 * fg.a)).press(fg.alpha(0.2 * fg.a))
            .child(text(if copied { "Copied" } else { "Copy" }).key(if copied { "copied" } else { "copy-label" }).font(s.cfg.font(11.0, 500.0)).color(fg).a11y_hidden()
                .transition(Transition::blur_replace()))
            .on_press_tx(move |s: &mut Island, tx| {
                match &content {
                    Content::Text(t) => tx.copy(t.clone()),
                    Content::Image(image, _) => tx.copy_rich(ClipboardContent::default().with_image(image.clone())),
                }
                s.clipboard.copied = Some((i, tx.clock() + 2.0));
            });
        let pin = tool(s, "pin", &icons::STAR, if clip.pinned { "Unpin" } else { "Pin" }).checked(clip.pinned)
            .on_press(move |s: &mut Island| s.toggle_clip_pin(i));
        let pin = if clip.pinned { pin.fill(super::accent::orange().alpha(0.3)) } else { pin };
        let body = match &clip.content {
            Content::Text(t) => text(t).font(s.cfg.font(13.0, 400.0).lh(18.0)).color(fg).grow(1.0).min_w(0.0).wrap_anywhere().max_lines(4),
            Content::Image(image, pixels) => {
                let (w, h) = image.size();
                let mut thumb = el().key("thumb").role("img").label(format!("Image, {w} × {h}")).h(64.0).w(96.0).radius(s.cfg.radius(8.0)).fill(fg.alpha(0.06 * fg.a));
                if let Some(p) = pixels.as_ref().filter(|p| p.is_ready() && p.error().is_none()) { thumb = thumb.pixels_fit(p.pixels(), SurfaceFit::Fit) }
                row().grow(1.0).min_w(0.0).gap(10.0).center_y().child(thumb)
                    .child(text(format!("{w} × {h}")).font(s.cfg.font(12.0, 500.0).features("'tnum' 1")).color(fg.alpha(0.55 * fg.a)).a11y_hidden())
            }
        };
        list = list.child(
            row().key(format!("clip-{i}")).w_full().px(14.0).py(10.0).gap(8.0).items_start()
                .radius(s.cfg.radius(16.0))
                .fill(super::card(fg))
                .child(body)
                .child(pin)
                .child(copy),
        );
    }
    let tools = tool(s, "text-tools", &icons::TYPE, "Text tools").checked(s.clipboard.tools).on_press(|s: &mut Island| s.clipboard.tools = !s.clipboard.tools);
    let mut root = col().key("clipboard").w_full().h_full().px(14.0).pt(14.0).pb(14.0).gap(10.0)
        .child(row().key("clipboard-search-row").w_full().gap(8.0).center_y()
            .child(field(s, "clipboard-search", &s.clipboard.search, "Search clipboard", |s: &mut Island| &mut s.clipboard.search, |_, _| {}))
            .child(if s.clipboard.tools { tools.fill(fg.alpha(0.26 * fg.a)) } else { tools }));
    if s.clipboard.tools {
        let source = s.clipboard.tool_source().map(|t| t.split_whitespace().collect::<Vec<_>>().join(" "));
        root = root.child(text(source.as_deref().map_or("Copy some text to use the tools".into(), |t| format!("On: {t}"))).key("tool-source")
            .font(s.cfg.font(12.0, 500.0)).color(fg.alpha(0.55 * fg.a)).w_full().single_line().truncate());
        let mut chips = row().key("text-tool-chips").role("group").label("Text tools: run on the newest copied text").w_full().gap(6.0).flex_wrap();
        for (i, (label, _)) in TOOLS.iter().enumerate() {
            chips = chips.child(pill(s, &format!("tool-{i}"), label).on_press_tx(move |s: &mut Island, tx| s.run_tool(i, tx)));
        }
        root = root.child(chips);
        if let Some(e) = &s.clipboard.tool_error {
            root = root.child(text(e).key("tool-error").role("alert").font(s.cfg.font(12.0, 500.0)).color(super::accent::red()));
        }
    }
    root
        .child(col().key("clipboard-scroll").w_full().grow(1.0).min_h(0.0).scrolls().scroll_edge_fade(12.0).child(list))
}

#[cfg(test)]
mod tests {
    use super::{Clipboard, Content};

    fn texts(c: &Clipboard) -> Vec<String> {
        c.items.iter().map(|i| match &i.content { Content::Text(t) => t.clone(), Content::Image(..) => "<image>".into() }).collect()
    }

    #[test]
    fn text_tools_convert_and_refuse_what_they_cannot_read() {
        let run = |label: &str, t: &str| (super::TOOLS.iter().find(|x| x.0 == label).unwrap().1)(t);
        assert_eq!(run("UPPER", "Hi there").unwrap(), "HI THERE");
        assert_eq!(run("Title", "hello wORLD, it's me").unwrap(), "Hello World, It's Me");
        assert_eq!(run("Base64", "hi?").unwrap(), "aGk/");
        assert_eq!(run("From Base64", " aGk/ ").unwrap(), "hi?");
        assert!(run("From Base64", "@@").is_err());
        assert_eq!(run("JSON", r#"{"a":[1]}"#).unwrap(), "{\n  \"a\": [\n    1\n  ]\n}");
        assert!(run("JSON", "{").is_err());
        assert_eq!(run("JSON", r#" {"b":1,"a":12345678901234567890123,"s":"x,\\\"{y}","e":{},"l":[ ]} "#).unwrap(),
            "{\n  \"b\": 1,\n  \"a\": 12345678901234567890123,\n  \"s\": \"x,\\\\\\\"{y}\",\n  \"e\": {},\n  \"l\": []\n}", "keys, numbers and strings as written");
        assert_eq!(run("URL encode", "a b&c").unwrap(), "a%20b%26c");
        assert_eq!(run("URL decode", "a+b%26c").unwrap(), "a b&c");
        assert!(run("URL decode", "caf%E9").is_err(), "Latin-1 isn't UTF-8");
        // {"alg":"none"}.{"sub":"me"}.
        let claims = run("JWT", "eyJhbGciOiJub25lIn0.eyJzdWIiOiJtZSJ9.").unwrap();
        assert!(claims.contains(r#""sub": "me""#) && claims.contains(r#""alg": "none""#), "{claims}");
        assert!(run("JWT", "a.b").is_err());
        assert!(run("JWT", "MQ.Mg.").is_err(), "1 and 2 are JSON, not a header and claims");
        let t = run("Unix time", "0").unwrap();
        assert!(t.starts_with("1970-01-01") || t.starts_with("1969-12-31"), "{t}");
        assert_eq!(run("Unix time", "1767225600000").unwrap()[..7], run("Unix time", "1767225600").unwrap()[..7], "milliseconds too");
        assert!(run("Unix time", "soon").is_err());
        assert!(run("Unix time", "-9223372036854775808").is_err());
        assert!(run("Unix time", "9223372036854775807").is_err());
        assert!(run("Unix time", "-62135596801").is_err());
    }

    #[test]
    fn text_tools_run_on_the_newest_copy_then_a_pinned_one() {
        let t = |s: &str| Content::Text(s.into());
        let mut c = Clipboard::default();
        assert_eq!(c.tool_source(), None);
        c.push(t("pinned"));
        c.toggle_pin(0);
        assert_eq!(c.tool_source(), Some("pinned"), "only pins: the first one");
        c.push(t("newest"));
        assert_eq!(c.tool_source(), Some("newest"), "the newest copy, below the pins");
    }

    #[test]
    fn history_keeps_the_newest_first_without_repeats() {
        let t = |s: &str| Content::Text(s.into());
        let mut c = Clipboard::default();
        assert!(c.push(t("a")) && c.push(t("b")));
        assert!(!c.push(t("b")) && !c.push(t("")));
        assert!(c.push(t("a")));
        assert_eq!(texts(&c), ["a", "b"]);
        for i in 0..60 { c.push(t(&i.to_string())); }
        assert_eq!(c.items.len(), 50);
    }

    #[test]
    fn pinned_items_stay_on_top_past_the_limit_and_search_filters() {
        let t = |s: &str| Content::Text(s.into());
        let mut c = Clipboard::default();
        c.push(t("keep"));
        c.push(t("other"));
        c.toggle_pin(1);
        assert_eq!((texts(&c), c.pins()), (vec!["keep".to_string(), "other".into()], vec![crate::config::Pin::Text("keep".into())]));
        for i in 0..60 { c.push(t(&format!("n{i}"))); }
        assert_eq!((c.items.len(), texts(&c)[0].as_str(), texts(&c)[1].as_str()), (51, "keep", "n59"));
        assert!(!c.push(t("keep")), "copying a pinned text again leaves it in place");
        c.search.replace_text("KEE");
        assert_eq!(c.listed(), [0]);
        c.toggle_pin(0);
        assert!(c.pins().is_empty() && texts(&c)[0] == "keep");
    }

    #[test]
    fn a_known_image_copied_again_keeps_its_pixels() {
        let png = [&b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR"[..], &[0, 0, 0, 2, 0, 0, 0, 3]].concat();
        let image = rune::ClipboardImage::from_png(png).unwrap();
        let cache = rune::ImageCache::new(std::sync::Arc::new(|| {}));
        let mut c = Clipboard::default();
        c.push(Content::Image(image.clone(), Some(cache.load(image.png().to_vec()))));
        c.push(Content::Text("after".into()));
        assert!(c.push(Content::Image(image, None)));
        assert!(matches!(&c.items[0].content, Content::Image(_, Some(_))));
    }

    #[test]
    fn pinned_images_are_saved_and_load_next_time() {
        use crate::config::{Config, Pin};
        let dir = std::env::temp_dir().join(format!("ripple-clips-{}", std::process::id()));
        let png = [&b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR"[..], &[0, 0, 0, 2, 0, 0, 0, 3]].concat();
        let image = rune::ClipboardImage::from_png(png).unwrap();
        let mut c = Clipboard::new(&Config::default(), Some(&dir));
        c.push(Content::Image(image.clone(), None));
        c.toggle_pin(0);
        let pins = c.pins();
        let cfg = Config { clipboard_pins: [pins.clone(), vec![Pin::Image { image: "../prefs.json".into() }, Pin::Image { image: "ff.png".into() }]].concat(), ..Config::default() };
        let again = Clipboard::new(&cfg, Some(&dir));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(matches!(&pins[..], [Pin::Image { image }] if image.len() == 36));
        assert_eq!(again.items.len(), 1, "files outside the folder or gone are left out");
        assert!(matches!(&again.items[0].content, Content::Image(i, None) if *i == image) && again.items[0].pinned);
    }
}
