//! Search, Spotlight-style: one field, results in order of apps, then files, then the web.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use rune::{El, Key, KeyEvent, Tx, col, icon, row, text, text_input};

use super::{Scrolls, tab_arrows};
use crate::config::QuickApp;
use crate::icons;
use crate::island::{Island, Mode};
use crate::system;

/// Most rows each kind shows, so files never push the web row out of sight.
const PER_KIND: usize = 3;

#[derive(Default)]
pub struct Finder {
    pub field: rune::TextEdit,
    /// Installed apps, found once.
    pub apps: Option<Vec<QuickApp>>,
    /// The OS index's answer for `query`.
    pub files: Vec<PathBuf>,
    /// The highlighted result.
    pub pick: usize,
    /// The query last sent to the index, and a count of queries sent so a stale one can tell.
    query: String,
    sent: Arc<AtomicU64>,
    finding_apps: bool,
}

pub enum Hit<'a> {
    App(&'a QuickApp),
    File(&'a Path),
    Web,
}

impl Island {
    /// Finds the installed apps once, off the UI thread.
    pub fn search_load(&mut self) {
        let Some(me) = self.me.clone() else { return };
        if self.finder.apps.is_some() || self.finder.finding_apps { return }
        self.finder.finding_apps = true;
        std::thread::spawn(move || {
            let apps = system::discover_apps();
            me.update(move |s, _| { s.finder.apps = Some(apps); s.finder.finding_apps = false });
        });
    }

    /// Asks the OS's file index about the query once typing pauses, off the UI thread.
    pub fn search_poll(&mut self) {
        let Some(me) = self.me.clone() else { return };
        let q = self.finder.field.text.trim().to_string();
        if q == self.finder.query { return }
        self.finder.query = q.clone();
        let id = self.finder.sent.fetch_add(1, Ordering::SeqCst) + 1;
        // The last query's files would be wrong for this one, and Enter could open them.
        self.finder.files.clear();
        if q.is_empty() { return }
        let sent = self.finder.sent.clone();
        std::thread::spawn(move || {
            // Each key would otherwise start an index query; only the last one in a burst runs.
            std::thread::sleep(std::time::Duration::from_millis(150));
            if sent.load(Ordering::SeqCst) != id { return }
            let files = rank_files(system::find_files(&q, 100), &q);
            me.update(move |s, _| if s.finder.query == q { s.finder.files = files });
        });
    }

    fn search_hits(&self) -> Vec<Hit<'_>> {
        let q = self.finder.field.text.trim().to_lowercase();
        if q.is_empty() { return Vec::new() }
        let apps = self.finder.apps.iter().flatten().map(|a| (a.name.to_lowercase(), a));
        let mut hits: Vec<Hit> = ranked(apps, &q).map(Hit::App).collect();
        hits.extend(self.finder.files.iter().map(|p| Hit::File(p)));
        hits.push(Hit::Web);
        hits
    }

    fn search_open(&mut self, tx: &mut Tx) {
        let hits = self.search_hits();
        let Some(hit) = hits.get(self.finder.pick.min(hits.len().saturating_sub(1))) else { return };
        match hit {
            Hit::App(app) => { let launch = app.launch.clone(); std::thread::spawn(move || system::launch_app(&launch)); }
            Hit::File(path) => system::open_external(&file_url(path)),
            Hit::Web => system::open_external(&self.web_url()),
        }
        self.finder.field.clear();
        (self.finder.pick, self.finder.query) = (0, String::new());
        self.finder.files.clear();
        tx.blur();
        self.set_mode(Mode::Still);
    }

    /// A URL as typed, or a search for the text.
    fn web_url(&self) -> String {
        let query = self.finder.field.text.trim();
        let lower = query.to_ascii_lowercase();
        if !query.contains('.') || query.contains(' ') { format!("{}{}", self.cfg.search_engine().1, crate::net::percent_encode(query)) }
        else if lower.starts_with("http://") || lower.starts_with("https://") { query.to_string() }
        else { format!("https://{query}") }
    }
}

/// Names that start with the query first, then names that contain it, at most `PER_KIND`.
fn ranked<'a, T: 'a>(items: impl Iterator<Item = (String, T)> + Clone + 'a, q: &'a str) -> impl Iterator<Item = T> + 'a {
    let starts = items.clone().filter(move |(n, _)| n.starts_with(q));
    let contains = items.filter(move |(n, _)| !n.starts_with(q) && n.contains(q));
    starts.chain(contains).map(|(_, t)| t).take(PER_KIND)
}

/// The index's hits with the best name matches first, as `ranked` orders apps; content-only
/// matches last. At most `PER_KIND`.
fn rank_files(mut files: Vec<PathBuf>, q: &str) -> Vec<PathBuf> {
    let q = q.to_lowercase();
    // Programs and bundles: `open_external` refuses them, so Enter on one would do nothing.
    files.retain(|p| system::openable(&file_url(p)));
    files.sort_by_key(|p| {
        let name = p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
        if name.starts_with(&q) { 0 } else if name.contains(&q) { 1 } else { 2 }
    });
    files.truncate(PER_KIND);
    files
}

/// `file://` URL of a path, each segment percent-encoded; a Windows drive keeps its colon.
fn file_url(path: &Path) -> String {
    let p = path.to_string_lossy();
    // Only Windows separates with `\`; elsewhere it's part of a name and is encoded.
    let p = if cfg!(windows) { p.replace('\\', "/") } else { p.into_owned() };
    let enc: Vec<String> = p.split('/').map(|seg| if seg.len() == 2 && seg.ends_with(':') { seg.into() } else { crate::net::percent_encode(seg) }).collect();
    let joined = enc.join("/");
    // A Windows UNC path `\\server\share` names its server as the URL's host.
    if cfg!(windows) && joined.starts_with("//") { format!("file:{joined}") }
    else if joined.starts_with('/') { format!("file://{joined}") } else { format!("file:///{joined}") }
}

fn keys(s: &mut Island, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let last = s.search_hits().len().saturating_sub(1);
    match ev.key {
        Key::Down => { s.finder.pick = (s.finder.pick + 1).min(last); true }
        Key::Up => { s.finder.pick = s.finder.pick.min(last).saturating_sub(1); true }
        _ => tab_arrows(s, ev, tx),
    }
}

pub fn view(s: &Island) -> El {
    let fg = s.cfg.text();
    let input = text_input(&s.finder.field, s.cfg.font(16.0, 400.0).lh(22.0)).key("browser-searchbar").label("Search").single_line().grow(1.0).min_w(0.0)
        .color(fg).placeholder("Search apps, files and the web", fg.alpha(0.5 * fg.a))
        // Any edit can change the results, so the first one is highlighted again.
        .on_edit(|s: &mut Island| { s.finder.pick = 0; &mut s.finder.field })
        .on_submit(Island::search_open)
        .on_input_key(keys);
    let bar = row().key("searchbar").w_full().h(46.0).px(16.0).gap(10.0).center_y()
        .radius(s.cfg.radius(23.0))
        .fill(fg.alpha(0.06 * fg.a)).border(1.0, fg.alpha(0.1 * fg.a))
        .child(icon(&icons::SEARCH, 16.0).color(fg.alpha(0.5 * fg.a)).a11y_hidden())
        .child(input);

    let hits = s.search_hits();
    let pick = s.finder.pick.min(hits.len().saturating_sub(1));
    let mut list = col().key("search-results").w_full().gap(2.0);
    for (i, hit) in hits.iter().enumerate() {
        let (glyph, name, detail) = match hit {
            Hit::App(app) => (&icons::MONITOR, app.name.clone(), "App".to_string()),
            Hit::File(path) => (&icons::FILE, path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                path.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()),
            Hit::Web => (&icons::GLOBE, s.finder.field.text.trim().to_string(), format!("Search {}", s.cfg.search_engine().0)),
        };
        list = list.child(
            row().key(format!("search-hit-{i}")).role("button").label(name.clone()).focusable(true)
                .w_full().h(34.0).px(12.0).gap(10.0).center_y().radius(s.cfg.radius(10.0))
                .fill(fg.alpha(if i == pick { 0.12 } else { 0.0 } * fg.a)).hover(fg.alpha(0.08 * fg.a))
                .child(icon(glyph, 15.0).color(fg.alpha(0.7 * fg.a)).a11y_hidden())
                .child(text(name).font(s.cfg.font(14.0, 600.0)).color(fg).single_line().truncate().max_w(220.0).a11y_hidden())
                .child(text(detail).font(s.cfg.font(12.0, 400.0)).color(fg.alpha(0.5 * fg.a)).single_line().truncate().grow(1.0).min_w(0.0).a11y_hidden())
                .on_press_tx(move |s: &mut Island, tx| { s.finder.pick = i; s.search_open(tx) }),
        );
    }
    col().key("search").w_full().h_full().px(14.0).py(14.0).gap(8.0)
        .child(bar)
        .child(col().key("search-scroll").w_full().grow(1.0).min_h(0.0).scrolls().child(list))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_matches_come_first_and_each_kind_is_capped() {
        let names = ["notepad", "gnome-notes", "notes", "anote", "notion"].map(|n| (n.to_string(), n));
        let got: Vec<_> = ranked(names.into_iter(), "no").collect();
        assert_eq!(got, ["notepad", "notes", "notion"]);
    }

    #[test]
    fn file_hits_put_name_matches_before_content_matches() {
        let files = ["/a/report.txt", "/a/my-notes.md", "/a/notes.sh", "/a/notes.md", "/a/x/todo"].map(PathBuf::from).to_vec();
        assert_eq!(rank_files(files, "Notes"), ["/a/notes.md", "/a/my-notes.md", "/a/report.txt"].map(PathBuf::from));
    }

    #[test]
    fn file_urls_encode_each_segment() {
        assert_eq!(file_url(Path::new("/home/me/My Notes/a#1.txt")), "file:///home/me/My%20Notes/a%231.txt");
        #[cfg(windows)]
        assert_eq!(file_url(Path::new(r"C:\Users\me\a b.pdf")), "file:///C:/Users/me/a%20b.pdf");
        #[cfg(windows)]
        assert_eq!(file_url(Path::new(r"\\server\share\a.pdf")), "file://server/share/a.pdf");
        #[cfg(not(windows))]
        assert_eq!(file_url(Path::new(r"/home/me/foo\bar.txt")), "file:///home/me/foo%5Cbar.txt");
    }
}
