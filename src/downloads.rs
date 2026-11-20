//! Download progress, shown as a live activity while files download.
//!
//! - Linux: each app's Unity LauncherEntry progress on the session bus (Firefox, Chromium and
//!   its forks, KDE apps), the signal Plasma's task manager and GNOME's docks draw. It is D-Bus,
//!   so it works the same on Wayland and X11.
//! - macOS: the NSProgress browsers publish for a file in Downloads, which Finder draws.
//! - Everywhere, and the only source on Windows: partial files in the Downloads folder, with no
//!   percent.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Progress {
    /// The mean share done, 0 to 1; None while no source knows it.
    pub fraction: Option<f32>,
    pub count: usize,
    /// The downloads just ended and their files are there: the Island shows its tick.
    pub done: bool,
}

impl Progress {
    pub fn percent(&self) -> f32 { self.fraction.map_or(0.0, |f| (f * 100.0).clamp(0.0, 100.0)) }

    pub fn label(&self) -> String {
        match self.fraction {
            Some(_) => format!("{}%", self.percent().floor()),
            None if self.count > 1 => format!("{} files", self.count),
            None => "Downloading".into(),
        }
    }
}

/// Partial-download suffixes: Chromium and its forks, Firefox, Safari, Opera, old Edge.
const PARTIAL: [&str; 5] = ["crdownload", "part", "download", "opdownload", "partial"];
/// A partial file untouched this long is a download left behind, not one running.
const IDLE: Duration = Duration::from_secs(10);
/// An app that stops reporting without hiding its progress (it quit) is dropped after this.
const STALE: Duration = Duration::from_secs(60);

/// Each source's share done (None: not known) and when it last reported, by source.
type Reports = Arc<Mutex<HashMap<String, (Option<f64>, Instant)>>>;

fn lock(reports: &Reports) -> std::sync::MutexGuard<'_, HashMap<String, (Option<f64>, Instant)>> {
    reports.lock().unwrap_or_else(|e| e.into_inner())
}

/// Calls `on` once a second with the downloads running now. Runs for good: call it on a thread of its own.
pub fn watch(on: impl Fn(Option<Progress>)) {
    let reports: Reports = Arc::default();
    // Not on macOS: reading Downloads there asks for permission, and NSProgress covers its browsers.
    #[cfg(not(target_os = "macos"))]
    let scan = folder();
    #[cfg(target_os = "macos")]
    let scan: Option<PathBuf> = None;
    #[cfg(target_os = "linux")]
    { let r = reports.clone(); std::thread::spawn(move || launcher_entries(&r)); }
    #[cfg(target_os = "macos")]
    if let Some(dir) = folder() {
        let r = reports.clone();
        std::thread::spawn(move || crate::mac::watch_progress(&dir, |fractions| {
            let mut r = lock(&r);
            r.retain(|k, _| !k.starts_with("mac:"));
            let now = Instant::now();
            r.extend(fractions.into_iter().enumerate().map(|(i, f)| (format!("mac:{i}"), (f, now))));
        }));
    }
    // While downloads run: when they started, how many, and whether a source last called one complete.
    let mut running: Option<(SystemTime, usize, bool)> = None;
    loop {
        let partial = scan.as_deref().map_or(0, |d| partial_files(d, SystemTime::now()));
        let reported: Vec<Option<f64>> = {
            let mut r = lock(&reports);
            r.retain(|_, (_, at)| at.elapsed() < STALE);
            r.values().map(|v| v.0).collect()
        };
        let now = merge(&reported, partial);
        let ended = match (now, running) {
            (Some(p), _) => { running = Some((running.map_or_else(SystemTime::now, |r| r.0), p.count, reported.contains(&Some(1.0)))); None }
            (None, Some(r)) => { running = None; Some(r) }
            (None, None) => None,
        };
        // Not on macOS, which reads no folder: there NSProgress says it finished, not cancelled.
        let done = ended.filter(|&(since, _, full)| scan.as_deref().map_or(full, |d| finished_file(d, since)));
        on(done.map(|(_, count, _)| Progress { fraction: Some(1.0), count, done: true }).or(now));
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// The Downloads folder: `xdg-user-dir`'s on Linux, as its name follows the language.
fn folder() -> Option<PathBuf> {
    // With none set it answers the home folder, which is not one to scan.
    #[cfg(target_os = "linux")]
    if let Some(dir) = crate::system::output("xdg-user-dir", &["DOWNLOAD"]).map(PathBuf::from).filter(|d| !d.as_os_str().is_empty() && Some(d) != rune::dirs::home().as_ref()) { return Some(dir) }
    rune::dirs::AppDirs::new("ripple")?.downloads().map(Path::to_path_buf)
}

/// Partial downloads in `dir` written to within `IDLE` of `now`.
fn partial_files(dir: &Path, now: SystemTime) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    entries.flatten().filter(|e| {
        let p = e.path();
        let partial = p.extension().and_then(|x| x.to_str()).is_some_and(|x| PARTIAL.iter().any(|s| x.eq_ignore_ascii_case(s)));
        partial && e.metadata().and_then(|m| m.modified()).is_ok_and(|t| now.duration_since(t).unwrap_or_default() < IDLE)
    }).count()
}

/// A whole file in `dir` written since `since`: a download that finished, not one cancelled
/// (its partial file is deleted and nothing takes its name).
// shortcut: only the Downloads folder is checked, so a file saved elsewhere ends without a tick.
fn finished_file(dir: &Path, since: SystemTime) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else { return false };
    // Filesystem times can be a moment coarse.
    let since = since - Duration::from_secs(2);
    entries.flatten().any(|e| {
        let p = e.path();
        let partial = p.extension().and_then(|x| x.to_str()).is_some_and(|x| PARTIAL.iter().any(|s| x.eq_ignore_ascii_case(s)));
        let hidden = e.file_name().to_string_lossy().starts_with('.');
        !partial && !hidden && e.metadata().is_ok_and(|m| m.is_file() && m.modified().is_ok_and(|t| t >= since))
    })
}

/// One activity for every source. An app's report covers its partial files, so they are not
/// counted twice; partial files alone have no percent.
fn merge(reported: &[Option<f64>], partial: usize) -> Option<Progress> {
    if reported.is_empty() { return (partial > 0).then_some(Progress { fraction: None, count: partial, done: false }) }
    let known: Vec<f64> = reported.iter().flatten().copied().collect();
    let fraction = (!known.is_empty()).then(|| (known.iter().sum::<f64>() / known.len() as f64).clamp(0.0, 1.0) as f32);
    Some(Progress { fraction, count: reported.len().max(partial), done: false })
}

/// Follows `busctl monitor` for LauncherEntry updates, restarting it if it exits.
#[cfg(target_os = "linux")]
fn launcher_entries(reports: &Reports) {
    use std::io::BufRead;
    use std::process::{Command, Stdio};
    // Apps whose bar is visible; unlike reports, this never goes stale, so a download that pauses past `STALE` comes back with its next progress.
    let mut shown = std::collections::HashSet::new();
    loop {
        let child = Command::new("busctl")
            .args(["--user", "monitor", "--json=short", "--match", "type='signal',interface='com.canonical.Unity.LauncherEntry',member='Update'"])
            .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn();
        if let Ok(mut child) = child {
            for line in child.stdout.take().into_iter().flat_map(|o| std::io::BufReader::new(o).lines().map_while(Result::ok)) {
                if let Some(update) = parse_launcher(&line) { apply(&mut lock(reports), &mut shown, update, Instant::now()) }
            }
            let _ = child.kill();
            let _ = child.wait();
        }
        // It died or is missing: try again later without spinning.
        std::thread::sleep(Duration::from_secs(10));
    }
}

/// Applies one LauncherEntry update: hidden drops the app; a progress without `progress-visible`
/// updates an app whose bar is visible and keeps its last share when it has none.
#[cfg(any(target_os = "linux", test))]
fn apply(r: &mut HashMap<String, (Option<f64>, Instant)>, shown: &mut std::collections::HashSet<String>, (app, visible, progress): (String, Option<bool>, Option<f64>), now: Instant) {
    match visible {
        Some(false) => { shown.remove(&app); r.remove(&app); }
        _ if visible == Some(true) || shown.contains(&app) => {
            shown.insert(app.clone());
            let known = progress.or_else(|| r.get(&app).and_then(|e| e.0));
            r.insert(app, (known, now));
        }
        _ => {}
    }
}

/// A LauncherEntry `Update` from `busctl --json=short`: the app, `progress-visible` and `progress`.
#[cfg(any(target_os = "linux", test))]
fn parse_launcher(line: &str) -> Option<(String, Option<bool>, Option<f64>)> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let data = v["payload"]["data"].as_array()?;
    let app = data.first()?.as_str()?.to_string();
    let props = data.get(1)?;
    let visible = props["progress-visible"]["data"].as_bool();
    let progress = props["progress"]["data"].as_f64();
    (visible.is_some() || progress.is_some()).then_some((app, visible, progress))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launcher_updates_parse_from_busctl() {
        let line = r#"{"type":"signal","member":"Update","payload":{"type":"sa{sv}","data":["application://firefox.desktop",{"progress":{"type":"d","data":4.2e-01},"progress-visible":{"type":"b","data":true}}]}}"#;
        assert_eq!(parse_launcher(line), Some(("application://firefox.desktop".into(), Some(true), Some(0.42))));
        let hidden = r#"{"payload":{"data":["application://chromium.desktop",{"progress-visible":{"type":"b","data":false}}]}}"#;
        assert_eq!(parse_launcher(hidden), Some(("application://chromium.desktop".into(), Some(false), None)));
        let badge = r#"{"payload":{"data":["application://mail.desktop",{"count":{"type":"x","data":3}}]}}"#;
        assert_eq!(parse_launcher(badge), None, "a badge count is not a download");
        assert_eq!(parse_launcher("Monitoring bus message stream."), None);
    }

    #[test]
    fn launcher_updates_show_and_hide_an_apps_bar() {
        let now = Instant::now();
        let (mut r, mut shown) = (HashMap::new(), std::collections::HashSet::new());
        let up = |v: Option<bool>, p: Option<f64>| ("app".to_string(), v, p);
        apply(&mut r, &mut shown, up(None, Some(0.3)), now);
        assert!(r.is_empty(), "progress alone shows no bar");
        apply(&mut r, &mut shown, up(Some(true), Some(0.3)), now);
        apply(&mut r, &mut shown, up(None, Some(0.6)), now);
        assert_eq!(r["app"].0, Some(0.6));
        apply(&mut r, &mut shown, up(Some(true), None), now);
        assert_eq!(r["app"].0, Some(0.6), "a share once known stays");
        // Paused past STALE: `watch` drops the report; the next progress alone brings it back.
        r.clear();
        apply(&mut r, &mut shown, up(None, Some(0.7)), now);
        assert_eq!(r["app"].0, Some(0.7));
        apply(&mut r, &mut shown, up(Some(false), None), now);
        assert!(r.is_empty());
        apply(&mut r, &mut shown, up(None, Some(0.8)), now);
        assert!(r.is_empty(), "hidden stays hidden until shown again");
    }

    #[test]
    fn sources_merge_into_one_activity() {
        assert_eq!(merge(&[], 0), None);
        let p = |fraction, count| Progress { fraction, count, done: false };
        assert_eq!(merge(&[], 2), Some(p(None, 2)));
        assert_eq!(merge(&[Some(0.2), Some(0.6)], 1), Some(p(Some(0.4), 2)));
        assert_eq!(merge(&[None, Some(0.5)], 3), Some(p(Some(0.5), 3)));
        assert_eq!(p(Some(0.426), 1).label(), "42%");
        assert_eq!(p(None, 2).label(), "2 files");
    }

    #[test]
    fn only_fresh_partial_files_count() {
        let dir = std::env::temp_dir().join(format!("ripple-downloads-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["a.iso.crdownload", "b.zip.part", "c.PART", "done.zip"] { std::fs::write(dir.join(name), "x").unwrap() }
        let now = SystemTime::now();
        let fresh = partial_files(&dir, now);
        let later = partial_files(&dir, now + IDLE + Duration::from_secs(1));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!((fresh, later), (3, 0));
    }

    #[test]
    fn only_a_new_whole_file_finishes_a_download() {
        let dir = std::env::temp_dir().join(format!("ripple-finished-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let start = SystemTime::now() + Duration::from_secs(60);
        for name in ["movie.mkv.crdownload", ".hidden"] { std::fs::write(dir.join(name), "x").unwrap() }
        let cancelled = finished_file(&dir, start - Duration::from_secs(120));
        std::fs::write(dir.join("old.zip"), "x").unwrap();
        let old = finished_file(&dir, start);
        let fresh = finished_file(&dir, start - Duration::from_secs(120));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!cancelled, "a partial or hidden file isn't a finished one");
        assert!(!old, "a file from before the download started doesn't count");
        assert!(fresh);
    }
}
