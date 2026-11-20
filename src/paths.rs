//! Ripple's files on disk: its folder beside prefs.json, user-typed paths, and writes a reader
//! never sees half done.

use std::path::{Path, PathBuf};

/// The folder that holds prefs.json, for commands that need it without loading the settings.
pub fn app_dir() -> Option<PathBuf> {
    rune::prefs::Prefs::open(crate::config::APP_ID).path().and_then(Path::parent).map(Into::into)
}

/// A path typed by the user, with `~` for the home folder.
pub fn expand_home(home: &Path, p: &str) -> PathBuf {
    match p.strip_prefix("~/").or_else(|| p.strip_prefix("~\\")) {
        Some(rest) => home.join(rest),
        None if p == "~" => home.into(),
        None => p.into(),
    }
}

/// Writes under a temporary name, then renames, so a reader never sees half a file.
pub fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)? }
    // One name per process: writers of the same file can run at once.
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&tmp, text)?;
    std::fs::rename(tmp, path)
}
