//! Ripple's files on disk: its folder beside prefs.json, user-typed paths, and writes a reader
//! never sees half done.

use std::path::{Path, PathBuf};

/// The folder name Rune keeps prefs.json in.
pub const APP_ID: &str = "Ripple";

/// The folder that holds prefs.json, for commands that need it without loading the settings.
pub fn app_dir() -> Option<PathBuf> {
    rune::prefs::Prefs::open(APP_ID).path().and_then(Path::parent).map(Into::into)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_paths_expand_home_and_writes_replace_whole_files() {
        let home = Path::new("/home/me");
        assert_eq!((expand_home(home, "~/a"), expand_home(home, "~"), expand_home(home, "/x/~")), (home.join("a"), home.into(), "/x/~".into()));
        let file = std::env::temp_dir().join(format!("ripple-paths-{}", std::process::id())).join("sub/f.txt");
        write_atomic(&file, "one").unwrap();
        write_atomic(&file, "two").unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "two");
        assert_eq!(std::fs::read_dir(file.parent().unwrap()).unwrap().count(), 1, "no temporary file is left");
        std::fs::remove_dir_all(file.parent().unwrap().parent().unwrap()).unwrap();
    }
}
