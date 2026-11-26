//! `ripple notify "text"`: a script's alert. The command drops the text as a file in a
//! `notify` folder beside prefs.json, and the running Island shows each one it finds there.
//! No socket or port, so it works the same on every OS and over a synced or mounted folder.

use std::path::Path;

/// The longest text kept; an alert shows far less.
const LIMIT: usize = 500;
/// Older alerts are dropped unseen: they were queued while Ripple was closed.
const STALE_MS: i64 = 5 * 60 * 1000;

/// Queues `text` for the running Island.
pub fn queue(dir: &Path, text: &str) -> std::io::Result<()> {
    // The count keeps two alerts queued in one millisecond by one process apart.
    static COUNT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let name = format!("{}-{}-{n:06}.txt", crate::date::now_ms(), std::process::id());
    crate::paths::write_atomic(&dir.join("notify").join(name), &text.chars().take(LIMIT).collect::<String>())
}

/// The queued texts, oldest first, each removed as it is read.
pub fn take(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir.join("notify")) else { return Vec::new() };
    let now = crate::date::now_ms();
    let mut files = Vec::new();
    for p in entries.flatten().map(|e| e.path()) {
        // The name starts with the unix ms it was queued at; stale `.txt` and stray `.tmp` files go.
        let stamp = p.file_name().and_then(|n| n.to_str()).and_then(|n| n.split('-').next()?.parse::<i64>().ok());
        if stamp.is_some_and(|ms| now - ms > STALE_MS) { let _ = std::fs::remove_file(&p); }
        else if p.extension().is_some_and(|e| e == "txt") { files.push(p) }
    }
    files.sort();
    files.into_iter().filter_map(|p| {
        let text = std::fs::read_to_string(&p).ok();
        let _ = std::fs::remove_file(&p);
        Some(text?.trim().chars().take(LIMIT).collect::<String>()).filter(|t| !t.is_empty())
    }).collect()
}

pub fn cli(args: &[String]) -> i32 {
    let text = args.join(" ");
    if text.trim().is_empty() { eprintln!("Usage: ripple notify \"text\""); return 2 }
    let Some(dir) = crate::paths::app_dir() else { eprintln!("Ripple: no settings folder"); return 1 };
    match queue(&dir, &text) {
        Ok(()) => 0,
        Err(e) => { eprintln!("Ripple: could not queue the alert: {e}"); 1 }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn queued_texts_are_taken_once_in_order() {
        let dir = std::env::temp_dir().join(format!("ripple-notify-{}", std::process::id()));
        super::queue(&dir, "Build passed").unwrap();
        super::queue(&dir, &"x".repeat(900)).unwrap();
        std::fs::write(dir.join("notify/half.tmp"), "not yet").unwrap();
        std::fs::write(dir.join("notify/1000-1.txt"), "From before launch").unwrap();
        std::fs::write(dir.join("notify/1000-2.tmp"), "Stray").unwrap();
        let texts = super::take(&dir);
        let again = super::take(&dir);
        let stale_gone = !dir.join("notify/1000-1.txt").exists() && !dir.join("notify/1000-2.tmp").exists();
        let half_kept = dir.join("notify/half.tmp").exists();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(stale_gone && half_kept, "stale files deleted, a fresh .tmp kept");
        assert_eq!(texts.len(), 2);
        assert_eq!((texts[0].as_str(), texts[1].len()), ("Build passed", 500));
        assert!(again.is_empty(), "taken once");
    }
}
