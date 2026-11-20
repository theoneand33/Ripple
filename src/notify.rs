//! `ripple notify "text"`: a script's alert. The command drops the text as a file in a
//! `notify` folder beside prefs.json, and the running Island shows each one it finds there.
//! No socket or port, so it works the same on every OS and over a synced or mounted folder.

use std::path::Path;

/// The longest text kept; an alert shows far less.
const LIMIT: usize = 500;

/// Queues `text` for the running Island: written under a temporary name, then renamed, so the
/// Island never reads half a file.
pub fn queue(dir: &Path, text: &str) -> std::io::Result<()> {
    let dir = dir.join("notify");
    std::fs::create_dir_all(&dir)?;
    let name = format!("{}-{}", crate::date::now_ms(), std::process::id());
    let tmp = dir.join(format!("{name}.tmp"));
    std::fs::write(&tmp, text.chars().take(LIMIT).collect::<String>())?;
    std::fs::rename(tmp, dir.join(format!("{name}.txt")))
}

/// The queued texts, oldest first, each removed as it is read.
pub fn take(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir.join("notify")) else { return Vec::new() };
    let mut files: Vec<_> = entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "txt")).collect();
    files.sort();
    files.into_iter().filter_map(|p| {
        let text = std::fs::read_to_string(&p).ok();
        let _ = std::fs::remove_file(&p);
        Some(text?.trim().chars().take(LIMIT).collect::<String>()).filter(|t| !t.is_empty())
    }).collect()
}

/// The command itself: queues its arguments as one text. Returns the exit code.
pub fn cli(args: &[String]) -> i32 {
    let text = args.join(" ");
    if text.trim().is_empty() { eprintln!("Usage: ripple notify \"text\""); return 2 }
    let Some(dir) = crate::config::Store::open().dir() else { eprintln!("Ripple: no settings folder"); return 1 };
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
        std::thread::sleep(std::time::Duration::from_millis(5));
        super::queue(&dir, &"x".repeat(900)).unwrap();
        std::fs::write(dir.join("notify/half.tmp"), "not yet").unwrap();
        let texts = super::take(&dir);
        let again = super::take(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(texts.len(), 2);
        assert_eq!((texts[0].as_str(), texts[1].len()), ("Build passed", 500));
        assert!(again.is_empty(), "taken once");
    }
}
