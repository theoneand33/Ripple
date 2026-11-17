//! A one-time import of the Electron app's settings. Electron kept them in Chromium's
//! localStorage: a LevelDB in `Local Storage/leveldb` inside the same folder the Rust app's
//! `prefs.json` lives in. This reads just enough LevelDB for that: the write-ahead logs and the
//! (snappy-compressed) tables, newest write per key winning.

use std::collections::HashMap;
use std::path::Path;

use serde::de::DeserializeOwned;

use crate::config::{Config, QuickApp, Tab, known_tabs};

/// The Electron app's settings, if its localStorage is in `dir` (the app's data folder).
pub fn electron_settings(dir: &Path) -> Option<Config> {
    let storage = local_storage(&dir.join("Local Storage").join("leveldb"));
    from_local_storage(&storage)
}

// --- LevelDB ---

/// (sequence number, value or None for a deletion) per key.
type Entries = HashMap<Vec<u8>, (u64, Option<Vec<u8>>)>;

fn record(entries: &mut Entries, key: Vec<u8>, seq: u64, value: Option<Vec<u8>>) {
    let newer = entries.get(&key).is_none_or(|(old, _)| seq >= *old);
    if newer { entries.insert(key, (seq, value)); }
}

fn varint(data: &[u8], pos: &mut usize) -> Option<u64> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *data.get(*pos)?;
        *pos += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 { return Some(value) }
    }
    None
}

fn take<'a>(data: &'a [u8], pos: &mut usize, len: usize) -> Option<&'a [u8]> {
    let bytes = data.get(*pos..pos.checked_add(len)?)?;
    *pos += len;
    Some(bytes)
}

/// Snappy's raw format, as LevelDB compresses table blocks.
fn snappy(input: &[u8]) -> Option<Vec<u8>> {
    let mut pos = 0;
    let len = usize::try_from(varint(input, &mut pos)?).ok()?;
    // A block is at most a few KB; anything wildly larger is corruption.
    if len > 64 << 20 { return None }
    let mut out = Vec::with_capacity(len);
    while pos < input.len() {
        let tag = input[pos];
        pos += 1;
        let (copy_len, offset) = match tag & 3 {
            0 => {
                let mut n = usize::from(tag >> 2);
                if n >= 60 {
                    let bytes = take(input, &mut pos, n - 59)?;
                    n = bytes.iter().rev().fold(0, |acc, b| acc << 8 | usize::from(*b));
                }
                out.extend_from_slice(take(input, &mut pos, n + 1)?);
                continue;
            }
            1 => (4 + usize::from((tag >> 2) & 7), usize::from(tag >> 5) << 8 | usize::from(*take(input, &mut pos, 1)?.first()?)),
            2 => (usize::from(tag >> 2) + 1, usize::from(u16::from_le_bytes(take(input, &mut pos, 2)?.try_into().ok()?))),
            _ => (usize::from(tag >> 2) + 1, usize::try_from(u32::from_le_bytes(take(input, &mut pos, 4)?.try_into().ok()?)).ok()?),
        };
        if offset == 0 || offset > out.len() { return None }
        // Byte by byte: a copy may overlap what it is producing.
        for _ in 0..copy_len { out.push(out[out.len() - offset]) }
    }
    (out.len() == len).then_some(out)
}

/// One block of a table, decompressed, given its handle.
fn table_block(file: &[u8], handle: &[u8]) -> Option<Vec<u8>> {
    let mut pos = 0;
    let offset = usize::try_from(varint(handle, &mut pos)?).ok()?;
    let size = usize::try_from(varint(handle, &mut pos)?).ok()?;
    let data = file.get(offset..offset.checked_add(size)?)?;
    match file.get(offset + size)? {
        0 => Some(data.to_vec()),
        1 => snappy(data),
        _ => None,
    }
}

/// A block's (key, value) entries, undoing its key prefix compression.
fn block_entries(block: &[u8]) -> Vec<(Vec<u8>, Vec<u8>)> {
    let mut out = Vec::new();
    let Some(count) = block.len().checked_sub(4).and_then(|i| block.get(i..)).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize) else { return out };
    let Some(end) = count.checked_mul(4).and_then(|n| block.len().checked_sub(4 + n)) else { return out };
    let data = &block[..end];
    let (mut pos, mut key) = (0, Vec::new());
    while pos < data.len() {
        let entry = (|| {
            let shared = usize::try_from(varint(data, &mut pos)?).ok()?;
            let unshared = usize::try_from(varint(data, &mut pos)?).ok()?;
            let value_len = usize::try_from(varint(data, &mut pos)?).ok()?;
            key.truncate(shared);
            key.extend_from_slice(take(data, &mut pos, unshared)?);
            Some((key.clone(), take(data, &mut pos, value_len)?.to_vec()))
        })();
        match entry { Some(e) => out.push(e), None => break }
    }
    out
}

fn read_table(file: &[u8], entries: &mut Entries) {
    const MAGIC: u64 = 0xdb47_7524_8b80_fb57;
    let Some(footer) = file.len().checked_sub(48).map(|i| &file[i..]) else { return };
    if u64::from_le_bytes(footer[40..48].try_into().unwrap_or_default()) != MAGIC { return }
    // The footer holds the metaindex handle (skipped), then the index handle.
    let mut pos = 0;
    for _ in 0..2 { if varint(footer, &mut pos).is_none() { return } }
    let Some(index) = footer.get(pos..40).and_then(|handle| table_block(file, handle)) else { return };
    for (_, handle) in block_entries(&index) {
        let Some(block) = table_block(file, &handle) else { continue };
        for (internal, value) in block_entries(&block) {
            // An internal key is the user key and 8 bytes of (sequence << 8 | type).
            let Some(split) = internal.len().checked_sub(8) else { continue };
            let tag = u64::from_le_bytes(internal[split..].try_into().unwrap_or_default());
            let value = (tag & 0xff == 1).then_some(value);
            record(entries, internal[..split].to_vec(), tag >> 8, value);
        }
    }
}

/// A log's write batches: sequence, count, then puts (1) and deletes (0).
fn read_batch(batch: &[u8], entries: &mut Entries) {
    let Some(seq) = batch.get(..8).map(|b| u64::from_le_bytes(b.try_into().unwrap_or_default())) else { return };
    let mut pos = 12;
    for i in 0.. {
        let Some(&kind) = batch.get(pos) else { break };
        pos += 1;
        let read = |pos: &mut usize| -> Option<Vec<u8>> {
            let len = usize::try_from(varint(batch, pos)?).ok()?;
            take(batch, pos, len).map(<[u8]>::to_vec)
        };
        let Some(key) = read(&mut pos) else { break };
        let value = match kind {
            1 => match read(&mut pos) { Some(v) => Some(v), None => break },
            0 => None,
            _ => break,
        };
        record(entries, key, seq + i, value);
    }
}

fn read_log(file: &[u8], entries: &mut Entries) {
    const BLOCK: usize = 32 * 1024;
    let mut pending = Vec::new();
    for block in file.chunks(BLOCK) {
        let mut pos = 0;
        while pos + 7 <= block.len() {
            let len = usize::from(u16::from_le_bytes([block[pos + 4], block[pos + 5]]));
            let kind = block[pos + 6];
            let Some(data) = block.get(pos + 7..pos + 7 + len) else { break };
            pos += 7 + len;
            match kind {
                1 => read_batch(data, entries),
                2 => pending = data.to_vec(),
                3 => pending.extend_from_slice(data),
                4 => { pending.extend_from_slice(data); read_batch(&std::mem::take(&mut pending), entries) }
                // 0: the zero padding at a block's end.
                _ => break,
            }
        }
    }
}

/// localStorage keys and values from a Chromium localStorage LevelDB, per origin.
fn local_storage(dir: &Path) -> HashMap<String, HashMap<String, String>> {
    let mut entries = Entries::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        // Settings are a few KB; a file far bigger than that isn't Ripple's.
        if entry.metadata().map_or(true, |m| m.len() > 64 << 20) { continue }
        let Ok(file) = std::fs::read(&path) else { continue };
        match path.extension().and_then(|e| e.to_str()) {
            Some("ldb" | "sst") => read_table(&file, &mut entries),
            Some("log") => read_log(&file, &mut entries),
            _ => {}
        }
    }
    let mut origins: HashMap<String, HashMap<String, String>> = HashMap::new();
    for (key, (_, value)) in entries {
        // "_" origin "\0" then a string; strings start with 1 (Latin-1) or 0 (UTF-16LE).
        let (Some(value), Some(rest)) = (value, key.strip_prefix(b"_")) else { continue };
        let Some(split) = rest.iter().position(|b| *b == 0) else { continue };
        let (Some(name), Some(value)) = (chromium_string(&rest[split + 1..]), chromium_string(&value)) else { continue };
        let origin = String::from_utf8_lossy(&rest[..split]).into_owned();
        origins.entry(origin).or_default().insert(name, value);
    }
    origins
}

fn chromium_string(bytes: &[u8]) -> Option<String> {
    match bytes.split_first()? {
        (1, latin1) => Some(latin1.iter().map(|b| char::from(*b)).collect()),
        (0, utf16) => {
            let units: Vec<u16> = utf16.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
            String::from_utf16(&units).ok()
        }
        _ => None,
    }
}

// --- Electron's keys ---

/// The packaged app's origin wins; the dev server's fills in what it lacks.
fn from_local_storage(origins: &HashMap<String, HashMap<String, String>>) -> Option<Config> {
    let mut storage: HashMap<&str, &str> = HashMap::new();
    let mut ordered: Vec<_> = origins.iter().collect();
    ordered.sort_by_key(|(origin, _)| origin.as_str() == "file://");
    for (_, values) in ordered {
        for (k, v) in values { storage.insert(k, v); }
    }
    let get = |key: &str| storage.get(key).copied();
    const KNOWN: [&str; 6] = ["tab-order", "standby-mode", "theme", "tasks", "quick-apps", "newuser"];
    if !KNOWN.iter().any(|k| get(k).is_some()) { return None }

    let json = |key: &str| -> Option<serde_json::Value> { serde_json::from_str(get(key)?).ok() };
    fn parse<T: DeserializeOwned>(v: serde_json::Value) -> Option<T> { serde_json::from_value(v).ok() }
    let mut c = Config::default();
    let flag = |key: &str, default: bool| get(key).map_or(default, |v| v == "true");
    c.battery_alerts = flag("battery-alerts", c.battery_alerts);
    c.island_border = flag("island-border", c.island_border);
    // Standby was on unless set to "false".
    c.standby = get("standby-mode") != Some("false");
    c.large_standby = flag("large-standby-mode", c.large_standby);
    c.hide_inactive = flag("hide-island-notactive", c.hide_inactive);
    c.show_info_idle = flag("show-info-when-idle", c.show_info_idle);
    c.auto_launch = flag("auto-launch", c.auto_launch);
    c.notch_mode = flag("notch-mode", c.notch_mode);
    c.t3_worktree = flag("t3-worktree", c.t3_worktree);
    c.hour12 = get("hour-format") != Some("24-hr");
    c.new_user = get("newuser") != Some("false");
    let text = |key: &str, into: &mut String| if let Some(v) = get(key) { *into = v.to_string() };
    text("weather-unit", &mut c.weather_unit);
    text("location", &mut c.location);
    text("theme", &mut c.theme);
    text("bg-color", &mut c.bg_color);
    text("text-color", &mut c.text_color);
    text("bg-image", &mut c.bg_image);
    text("search-engine", &mut c.search_engine);
    text("side-mode", &mut c.position_mode);
    text("position-mode", &mut c.position_mode);
    text("ai-provider", &mut c.ai_provider);
    text("ai-model", &mut c.ai_model);
    text("api-key", &mut c.api_key);
    text("t3-env", &mut c.t3_env);
    text("t3-project", &mut c.t3_project);
    text("t3-model", &mut c.t3_model);
    // The Electron app re-applied a preset theme's colours on every launch.
    if ["sleek-black", "win95", "none"].contains(&c.theme.as_str()) { let theme = c.theme.clone(); c.set_theme(&theme) }
    if let Some(v) = get("default-tab").and_then(|v| v.parse().ok()).and_then(|v: u8| Tab::try_from(v).ok()) { c.default_tab = v }
    if let Some(v) = get("island-x").and_then(|v| v.parse().ok()) { c.island_x = v }
    if let Some(v) = get("island-y").and_then(|v| v.parse().ok()) { c.island_y = v }
    if let Some(v) = get("t3-device").and_then(|v| v.parse().ok()) { c.t3_device = v }
    if let Some(v) = json("tab-order").and_then(|v| known_tabs(v).ok()) { c.tab_order = v }
    if let Some(v) = json("hidden-tabs").and_then(|v| known_tabs(v).ok()) { c.hidden_tabs = v }
    if let Some(v) = json("workflows").and_then(parse) { c.workflows = v }
    if let Some(v) = json("tasks").and_then(parse) { c.tasks = v }
    if let Some(v) = json("t3-devices").and_then(parse) { c.t3_devices = v }
    // Quick apps were once plain names, later {name, launch}.
    if let Some(serde_json::Value::Array(apps)) = json("quick-apps") {
        c.quick_apps = apps.into_iter().filter_map(|a| match a {
            serde_json::Value::String(name) => Some(QuickApp { launch: name.clone(), name }),
            other => parse(other),
        }).collect();
    }
    Some(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A LevelDB written by the real library (classic-level): a compacted, snappy table
    /// plus newer puts and a delete still in the log.
    fn fixture() -> HashMap<String, HashMap<String, String>> {
        local_storage(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/electron-leveldb"))
    }

    #[test]
    fn localstorage_reads_newest_values_from_tables_and_logs() {
        let file = &fixture()["file://"];
        assert_eq!(file["filler-7"].len(), 2000, "a value from the compressed table");
        assert_eq!(file["hour-format"], "24-hr");
        assert_eq!(file["standby-mode"], "false", "the log's newer write wins");
        assert_eq!(file["tasks"], r#"["Ship Rïpple ✓"]"#, "UTF-16 values decode");
        assert!(!file.contains_key("location"), "a deleted key stays deleted");
    }

    #[test]
    fn electron_settings_become_a_config() {
        let c = from_local_storage(&fixture()).unwrap();
        assert!(!c.standby && !c.hour12 && !c.new_user);
        assert_eq!(c.tab_order, [Tab::T3, Tab::Search, Tab::Workflows, Tab::Overview, Tab::Assistant, Tab::Clipboard, Tab::Tasks]);
        assert_eq!(c.api_key, "gsk_new");
        assert_eq!(c.tasks, ["Ship Rïpple ✓"]);
        assert_eq!(c.t3_devices[0].token, "tok");
        assert_eq!(c.island_x, 25.5);
        assert_eq!(c.location, "");
        assert_eq!(c.theme, "win95", "the dev server's value fills a key the packaged app lacks");
        assert_eq!((c.bg_color.as_str(), c.text_color.as_str()), ("rgba(195, 195, 195)", "rgba(0, 0, 0)"), "a preset theme brings its colours");
    }

    #[test]
    fn old_quick_app_names_and_missing_storage() {
        let mut origin = HashMap::new();
        origin.insert("quick-apps".to_string(), r#"["Notes",{"name":"Code","launch":"code ."}]"#.to_string());
        let c = from_local_storage(&HashMap::from([("file://".to_string(), origin)])).unwrap();
        assert_eq!(c.quick_apps, [QuickApp { name: "Notes".into(), launch: "Notes".into() }, QuickApp { name: "Code".into(), launch: "code .".into() }]);
        assert!(electron_settings(Path::new("/nonexistent")).is_none());
    }

    #[test]
    fn snappy_decodes_literals_and_overlapping_copies() {
        // "abcabcabcabc": a 3-byte literal, then a 9-byte copy at offset 3.
        assert_eq!(snappy(&[12, 0x08, b'a', b'b', b'c', 0x15, 0x03]).as_deref(), Some(&b"abcabcabcabc"[..]));
        assert_eq!(snappy(&[5, 0x08, b'a', b'b', b'c']), None, "a short output is corrupt");
        assert_eq!(snappy(&[3, 0x09, 0x05]), None, "a copy before any output is corrupt");
    }
}
