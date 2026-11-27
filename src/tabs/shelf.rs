//! Shelf: files dropped on the Island, kept until they are dragged out or removed. A right-click
//! on a file opens its menu: open, show in folder, rename, compress to .zip, extract a .zip.

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

use rune::{DragItem, DragOutcome, El, Key, KeyEvent, PointerPress, TextEdit, Transition, Tx, col, el, icon, row, text, text_input};
use rune_kit::{MenuItem, context_menu};

use super::{Undo, tool, undo_button};
use crate::config::Tab;
use crate::icons;
use crate::island::{Island, Mode};
use crate::system;

/// The `open_select` name while a file's menu is open: its menu hangs outside the Island.
const MENU: &str = "shelf";

#[derive(Default)]
pub struct Shelf {
    /// The file whose menu is open, and where the right-click landed.
    pub menu: Option<(PathBuf, PointerPress)>,
    /// The file being renamed, and the name being typed.
    pub renaming: Option<PathBuf>,
    pub name: TextEdit,
    /// The last action's error, or the work in progress.
    status: String,
    pub sync: super::SyncState,
    /// A file drag is over the Island.
    pub dropping: bool,
    /// A drag started elsewhere on the desktop opened the Shelf; it rests when the drag ends.
    pub desktop_drag: bool,
}

fn close_menu(s: &mut Island) {
    s.shelf.menu = None;
    s.open_select = None;
}

/// Swaps `old` for `new` in place on the Shelf, or puts `new` on top when `old` stays.
fn shelve(s: &mut Island, old: Option<&Path>, new: PathBuf) {
    s.save(|c| {
        c.shelf.retain(|f| *f != new);
        match old.and_then(|o| c.shelf.iter().position(|f| f == o)) {
            Some(i) => c.shelf[i] = new.clone(),
            None => c.shelf.insert(0, new.clone()),
        }
    });
    s.shelf_new_files(&[new]);
}

/// Runs a blocking file job off the UI thread; its result goes on top of the Shelf.
fn job(s: &mut Island, busy: &str, work: impl FnOnce() -> io::Result<PathBuf> + Send + 'static) {
    let Some(me) = s.me.clone() else { return };
    s.shelf.status = busy.into();
    me.spawn(work, |s, out| match out {
        Ok(path) => { s.shelf.status.clear(); shelve(s, None, path) }
        Err(e) => s.shelf.status = e.to_string(),
    });
}

/// Puts the clipboard on the Shelf: copied files as they are, an image or a text as a new file
/// in a `Pasted` folder beside prefs.json.
fn paste(_: &mut Island, tx: &mut Tx) {
    tx.read_clipboard(|s: &mut Island, content, tx| {
        if !content.files().is_empty() { return s.drop_files(content.files(), tx) }
        let Some(dir) = s.store.dir().map(|d| d.join("Pasted")) else { s.shelf.status = "Nowhere to save the paste".into(); return };
        let (stem, ext, bytes) = match content.image() {
            Some(image) => ("Pasted image", ".png", image.png().to_vec()),
            None if !content.text().is_empty() => ("Pasted text", ".txt", content.text().as_bytes().to_vec()),
            None => { s.shelf.status = "The clipboard is empty".into(); return }
        };
        job(s, "Pasting…", move || save_new(&dir, stem, ext, &bytes));
    });
}

/// Writes `bytes` to a new file named from `stem` in `dir`; never over an existing one.
fn save_new(dir: &Path, stem: &str, ext: &str, bytes: &[u8]) -> io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let out = free(dir, stem, ext);
    io::Write::write_all(&mut std::fs::OpenOptions::new().write(true).create_new(true).open(&out)?, bytes)?;
    Ok(out)
}

fn start_rename(s: &mut Island, path: PathBuf, tx: &mut Tx) {
    let name = file_name(&path);
    // Select the stem, as a file manager does, so typing keeps the extension.
    let stem = Path::new(&name).file_stem().map_or(name.len(), |s| s.len());
    s.shelf.name = TextEdit::new(name);
    (s.shelf.name.anchor, s.shelf.name.focus) = (0, stem);
    s.shelf.renaming = Some(path);
    tx.focus("shelf-rename", false);
}

fn finish_rename(s: &mut Island, _: &mut Tx) {
    let Some(path) = s.shelf.renaming.take() else { return };
    match rename(&path, s.shelf.name.text.trim()) {
        Ok(new) if new == path => {}
        Ok(new) => { s.shelf.status.clear(); shelve(s, Some(&path), new) }
        Err(e) => s.shelf.status = e.to_string(),
    }
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

/// Renames within the same folder; never over an existing file.
fn rename(path: &Path, name: &str) -> io::Result<PathBuf> {
    let bad = |m: &str| io::Error::new(io::ErrorKind::InvalidInput, m.to_string());
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\', ':']) { return Err(bad("That name isn't valid")) }
    let new = path.with_file_name(name);
    if new == path { return Ok(new) }
    // A case-only rename on a case-insensitive disk finds the file itself.
    if new.symlink_metadata().is_ok() && new.canonicalize().ok() != path.canonicalize().ok() { return Err(bad("A file with that name already exists")) }
    std::fs::rename(path, &new)?;
    Ok(new)
}

/// `dir/stem.ext`, or `dir/stem 2.ext` and up when that is taken.
fn free(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let at = |n: u32| dir.join(format!("{stem}{}{ext}", if n < 2 { String::new() } else { format!(" {n}") }));
    (1..).map(at).find(|p| p.symlink_metadata().is_err()).expect("some name is free")
}

fn is_zip(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("zip"))
}

/// Zips a file or a folder next to itself.
fn compress(src: &Path) -> io::Result<PathBuf> {
    if src.symlink_metadata()?.file_type().is_symlink() { return Err(io::Error::other("Can't compress a link")) }
    let dir = src.parent().unwrap_or(Path::new("."));
    let out = free(dir, &file_name(src), ".zip");
    let mut zip = zip::ZipWriter::new(std::fs::OpenOptions::new().write(true).create_new(true).open(&out)?);
    let opts = zip::write::SimpleFileOptions::default();
    let result = (|| {
        // Entries are named from the folder that holds `src`, with `/` on every OS.
        let mut stack = vec![src.to_path_buf()];
        while let Some(p) = stack.pop() {
            // Links are skipped: following one could loop forever or archive files from elsewhere.
            let kind = p.symlink_metadata()?.file_type();
            if kind.is_symlink() { continue }
            let name = p.strip_prefix(dir).unwrap_or(&p).components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/");
            if kind.is_dir() {
                zip.add_directory(name, opts)?;
                for entry in std::fs::read_dir(&p)? { stack.push(entry?.path()) }
            } else if kind.is_file() {
                zip.start_file(name, opts)?;
                io::copy(&mut File::open(&p)?, &mut zip)?;
            }
        }
        zip.finish()?;
        Ok(())
    })();
    // A half-written archive is worse than none.
    result.map(|()| out.clone()).map_err(|e: zip::result::ZipError| { let _ = std::fs::remove_file(&out); io::Error::other(e) })
}

/// Extracts a .zip into a new folder beside it, named after it.
fn extract(src: &Path) -> io::Result<PathBuf> {
    let dir = src.parent().unwrap_or(Path::new("."));
    let stem = src.file_stem().map_or_else(|| "Archive".into(), |s| s.to_string_lossy().into_owned());
    let out = free(dir, &stem, "");
    // `extract` keeps every entry inside `out` (no `../` or absolute paths, no escaping links).
    let done = zip::ZipArchive::new(File::open(src)?).and_then(|mut z| z.extract(&out));
    // A half-filled folder is worse than none.
    if let Err(e) = done { let _ = std::fs::remove_dir_all(&out); return Err(io::Error::other(e)) }
    Ok(out)
}

/// The file's right-click menu around its tile.
fn menu(s: &Island, path: &Path, tile: El, layer: &mut Vec<El>) -> El {
    let point = s.shelf.menu.as_ref().filter(|(p, _)| p == path && s.open_select == Some(MENU)).map(|m| m.1);
    let act = |f: fn(&mut Island, PathBuf, &mut Tx)| {
        let path = path.to_path_buf();
        move |s: &mut Island, tx: &mut Tx| { close_menu(s); f(s, path.clone(), tx) }
    };
    let mut m = context_menu(format!("shelf-menu-{}", path.display()), tile, point)
        .item(MenuItem::new("Open").on_select_tx(act(|_, p, _| system::open_file(&p))))
        .item(MenuItem::new(if cfg!(target_os = "macos") { "Show in Finder" } else { "Show in Folder" }).on_select_tx(act(|_, p, _| system::reveal(&p))))
        .separator()
        .item(MenuItem::new("Rename…").on_select_tx(act(start_rename)))
        .item(MenuItem::new("Compress").on_select_tx(act(|s, p, _| job(s, "Compressing…", move || compress(&p)))));
    if is_zip(path) {
        m = m.item(MenuItem::new("Extract Here").on_select_tx(act(|s, p, _| job(s, "Extracting…", move || extract(&p)))));
    }
    m.item(MenuItem::new("Copy Path").on_select_tx(act(|_, p, tx| tx.copy(p.display().to_string()))))
        .separator()
        .item(MenuItem::new("Remove from Shelf").danger(true).on_select_tx(act(|s, p, _| { s.save(|c| c.shelf.retain(|f| *f != p)); s.nextcloud_shelf_sync() })))
        .on_close(close_menu)
        .build(&s.cfg.kit(), layer)
}

pub fn view(s: &Island, layer: &mut Vec<El>) -> El {
    let fg = s.cfg.text();
    let muted = fg.alpha(0.55 * fg.a);
    let mut header = row().key("shelf-header").w_full().h(28.0).gap(8.0).center_y()
        .child(text("Shelf").font(s.cfg.font(17.0, 700.0)).color(fg))
        .child(icon(&icons::INBOX, 16.0).color(muted).a11y_hidden());
    let status = if s.shelf.status.is_empty() { s.shelf.sync.error.as_deref().unwrap_or_default() } else { &s.shelf.status };
    if !status.is_empty() {
        header = header.child(text(status).font(s.cfg.font(12.0, 400.0)).color(muted).shrink().min_w(0.0).max_lines(1));
    }
    header = header.child(el().grow(1.0));
    if let Some(undo) = undo_button(s, Tab::Shelf) { header = header.child(undo) }
    header = header.child(tool(s, "paste-shelf", &icons::CLIPBOARD_PASTE, "Add from clipboard").on_press_tx(paste));
    if !s.cfg.shelf.is_empty() {
        header = header.child(tool(s, "clear-shelf", &icons::TRASH, "Clear shelf").on_press(|s: &mut Island| {
            let mut gone = Vec::new();
            s.save(|c| gone = std::mem::take(&mut c.shelf));
            s.undo = Some(Undo::Shelf(gone));
            s.nextcloud_shelf_sync();
        }));
    }
    let root = col().key("shelf").w_full().h_full().px(16.0).pt(14.0).pb(14.0).gap(10.0).child(header);

    // The drop zone, while a drag is over the Island or nothing is stashed.
    if s.shelf.dropping || s.cfg.shelf.is_empty() {
        let line = fg.alpha(if s.shelf.dropping { 0.6 } else { 0.3 } * fg.a);
        return root.child(
            col().key("shelf-drop").w_full().grow(1.0).min_h(0.0).gap(4.0).center().radius(s.cfg.radius(16.0))
                .fill(fg.alpha(if s.shelf.dropping { 0.08 } else { 0.0 } * fg.a)).border(1.5, line).border_dash(6.0, 5.0)
                .child(icon(&icons::DOWNLOAD, 26.0).color(muted).a11y_hidden())
                .child(text("Drop files here").font(s.cfg.font(15.0, 600.0)).color(fg).mt(4.0))
                .child(text("Stash files here. Drag them out when you need them.").font(s.cfg.font(12.0, 400.0)).color(muted)),
        );
    }

    let mut files = row().key("shelf-files").gap(10.0).items_start();
    for path in &s.cfg.shelf {
        let name = file_name(path);
        let (out, moved) = (path.clone(), path.clone());
        let (remove, target) = (path.clone(), path.clone());
        let label = if s.shelf.renaming.as_ref() == Some(path) {
            text_input(&s.shelf.name, s.cfg.font(11.0, 500.0).lh(14.0)).key("shelf-rename").label("Name").single_line().w_full()
                .color(fg).fill(fg.alpha(0.1 * fg.a)).radius(4.0)
                .on_edit(|s: &mut Island| &mut s.shelf.name).on_submit(finish_rename)
                .on_input_key(|s: &mut Island, ev: &KeyEvent, _| ev.key == Key::Escape && s.shelf.renaming.take().is_some())
        } else {
            text(&name).font(s.cfg.font(11.0, 500.0)).color(fg).w_full().align(rune::Align::Center).max_lines(2).wrap_anywhere().a11y_hidden()
        };
        let tile = col().key(format!("shelf-{}", path.display())).role("button").label(name.clone()).focusable(true)
            .w(84.0).padding(8.0).gap(6.0).items_center().radius(s.cfg.radius(12.0))
            .hover(fg.alpha(0.08 * fg.a)).press(fg.alpha(0.14 * fg.a))
            .transition(Transition::opacity())
            .drag_out(move |_: &Island| Some(DragItem::new().file(&out)))
            .on_drag_end(move |s: &mut Island, outcome, _| {
                if outcome == DragOutcome::Moved { s.save(|c| c.shelf.retain(|f| *f != moved)); s.nextcloud_shelf_sync() }
                // Back over the Island, the drop decides.
                if !s.shelf.dropping { s.set_mode(Mode::Still) }
            })
            .on_secondary_press(move |s: &mut Island, at, _| { s.shelf.menu = Some((target.clone(), at)); s.open_select = Some(MENU) })
            .child(
                el().size(52.0, 52.0).center().radius(s.cfg.radius(12.0)).fill(fg.alpha(0.08 * fg.a))
                    .child(icon(&icons::FILE, 24.0).color(fg).a11y_hidden())
                    .child(
                        el().key("remove").role("button").label(format!("Remove {name}")).focusable(true)
                            .pin(None, Some(-4.0), Some(-4.0), None).size(18.0, 18.0).center().radius(9.0)
                            .fill(fg.alpha(0.25 * fg.a)).hover(fg.alpha(0.4 * fg.a))
                            .child(icon(&icons::X, 11.0).color(fg).a11y_hidden())
                            .on_press(move |s: &mut Island| { s.save(|c| c.shelf.retain(|f| *f != remove)); s.nextcloud_shelf_sync() }),
                    ),
            )
            .child(label);
        files = files.child(menu(s, path, tile, layer));
    }
    root.child(el().key("shelf-scroll").w_full().grow(1.0).min_h(0.0).scroll_x().child(files))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compress_extract_and_rename_round_trip() {
        let dir = std::env::temp_dir().join(format!("ripple-shelf-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("docs/sub")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&dir, dir.join("docs/loop")).unwrap();
        std::fs::write(dir.join("docs/a.txt"), "hello").unwrap();
        std::fs::write(dir.join("docs/sub/b.txt"), "world").unwrap();

        let pasted = save_new(&dir.join("Pasted"), "Pasted text", ".txt", b"hi").unwrap();
        assert_eq!(save_new(&dir.join("Pasted"), "Pasted text", ".txt", b"yo").unwrap(), dir.join("Pasted/Pasted text 2.txt"));
        assert_eq!(std::fs::read_to_string(pasted).unwrap(), "hi");

        let zip = compress(&dir.join("docs")).unwrap();
        assert_eq!(zip, dir.join("docs.zip"));
        assert!(is_zip(&zip));
        // A second archive doesn't overwrite the first.
        assert_eq!(compress(&dir.join("docs")).unwrap(), dir.join("docs 2.zip"));

        let bad = dir.join("bad.zip");
        std::fs::write(&bad, "not a zip").unwrap();
        assert!(extract(&bad).is_err());
        assert!(!dir.join("bad").exists(), "a failed extract leaves nothing");

        let out = extract(&zip).unwrap();
        assert_eq!(out, dir.join("docs 2"), "the folder `docs` is taken");
        assert_eq!(std::fs::read_to_string(out.join("docs/sub/b.txt")).unwrap(), "world");

        let a = dir.join("docs/a.txt");
        assert!(rename(&a, "b/c.txt").is_err());
        assert!(rename(&a, "").is_err() && rename(&a, "C:x").is_err());
        assert!(rename(&a, "sub").is_err(), "never over an existing entry");
        assert_eq!(rename(&a, "a.txt").unwrap(), a);
        assert_eq!(rename(&a, "c.txt").unwrap(), dir.join("docs/c.txt"));
        assert_eq!(std::fs::read_to_string(dir.join("docs/c.txt")).unwrap(), "hello");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
