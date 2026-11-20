//! Nextcloud sync for Notes (the Notes app's REST API), Tasks (CalDAV to-dos), Calendar
//! events (CalDAV events) and the Shelf (WebDAV files in one folder).
//! `sync` blocks, so it runs off the UI thread; `apply` merges its outcome on the UI thread,
//! so edits made while it ran are kept.

use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine;
use serde_json::{Value, json};

use crate::config::{Config, Item, Nextcloud, Remote};
use crate::date::days_from_civil;
use crate::island::Island;
use crate::net::{percent_decode, percent_encode};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Notes = 0,
    Tasks = 1,
    Events = 2,
}

impl Kind {
    const ALL: [Kind; 3] = [Kind::Notes, Kind::Tasks, Kind::Events];

    /// The kind's items, and what Nextcloud is still to hear of: deletes (notes, events) or
    /// completions (tasks), by remote id.
    pub fn lists(self, c: &mut Config) -> (&mut Vec<Item>, &mut Vec<String>) {
        match self {
            Kind::Notes => (&mut c.notes, &mut c.nextcloud.deleted_notes),
            Kind::Tasks => (&mut c.tasks, &mut c.nextcloud.done_tasks),
            Kind::Events => (&mut c.events, &mut c.nextcloud.deleted_events),
        }
    }
}

/// Takes an item out, and where it sat; one Nextcloud has goes there too on the next sync
/// (a task is marked done).
pub fn remove(c: &mut Config, kind: Kind, key: u64) -> Option<(usize, Item)> {
    let (items, pending) = kind.lists(c);
    let i = items.iter().position(|item| item.key == key)?;
    let item = items.remove(i);
    if let Some(r) = &item.remote { pending.push(r.id.clone()) }
    Some((i, item))
}

/// Puts a removed item back at `at`. A removal not yet sent is called off and the item keeps its
/// Nextcloud link. One sent, or maybe in the sync running now, goes through, and the item
/// uploads afresh: a kept link to a deleted copy would make `apply` drop it.
pub fn restore(c: &mut Config, kind: Kind, at: usize, item: Item, syncing: bool) {
    let (items, pending) = kind.lists(c);
    let item = match &item.remote {
        Some(r) if !syncing && pending.contains(&r.id) => {
            pending.retain(|id| *id != r.id);
            item
        }
        Some(_) => Item { remote: None, ..item },
        None => item,
    };
    items.insert(at.min(items.len()), item);
}

/// One kind's sync: whether one runs, whether another was asked for meanwhile, and the last error.
#[derive(Default)]
pub struct SyncState {
    pub running: bool,
    again: bool,
    pub error: Option<String>,
}

#[derive(Debug, Default, PartialEq)]
pub struct Outcome {
    /// What each uploaded item now is on Nextcloud, by item key.
    pub pushed: Vec<(u64, Remote)>,
    /// Pending deletes (notes, events) or completions (tasks) that went through.
    pub cleared: Vec<String>,
    /// Everything Nextcloud has, after the uploads; None when the sync stopped before reading it.
    pub server: Option<Vec<Remote>>,
    /// Why the sync stopped early. What it did before that still counts.
    pub error: Option<String>,
    /// The path `server` lists (a Tasks list or calendar); items synced elsewhere are left alone.
    pub scope: String,
}

/// Merges a sync's outcome into the items as they are now.
pub fn apply(items: &mut Vec<Item>, pending: &mut Vec<String>, out: Outcome) {
    for (key, remote) in out.pushed {
        match items.iter_mut().find(|i| i.key == key) {
            Some(i) => i.remote = Some(remote),
            // Deleted or ticked off while it uploaded: do the same there.
            None => pending.push(remote.id),
        }
    }
    pending.retain(|p| !out.cleared.contains(p));
    let Some(server) = out.server else { return };
    items.retain_mut(|i| {
        let Some(r) = &i.remote else { return true };
        // In another list (the Tasks list was changed): still there, just not listed.
        if !r.id.starts_with(&out.scope) { return true }
        // Changed here since the last sync: the next sync uploads it, so Nextcloud's copy doesn't win.
        let clean = i.text == r.text;
        match server.iter().find(|s| s.id == r.id) {
            Some(s) => {
                if clean { (i.text, i.remote) = (s.text.clone(), Some(s.clone())) }
                true
            }
            // Gone there: an unchanged copy goes too, a changed one uploads afresh.
            None => {
                i.remote = None;
                !clean
            }
        }
    });
    for s in server {
        let known = pending.contains(&s.id) || items.iter().any(|i| i.remote.as_ref().is_some_and(|r| r.id == s.id));
        if !known { items.push(Item { remote: Some(s.clone()), ..Item::new(s.text) }) }
    }
}

impl Island {
    /// Syncs one kind with Nextcloud in the background; a request while one runs queues one more.
    pub fn nextcloud_sync(&mut self, kind: Kind) {
        if !self.cfg.nextcloud.enabled() { return }
        let Some(me) = self.me.clone() else { return };
        let state = &mut self.nextcloud[kind as usize];
        if state.running { state.again = true; return }
        state.running = true;
        let account = self.cfg.nextcloud.account();
        let nc = self.cfg.nextcloud.clone();
        let (items, pending) = kind.lists(&mut self.cfg);
        let (mut items, mut pending) = (items.clone(), pending.clone());
        // Ids from another account mean nothing to this one: everything uploads as new. The saved
        // ids are only dropped once this account answers, so a half-typed URL loses nothing.
        let fresh = nc.synced_with != account;
        if fresh {
            for i in &mut items { i.remote = None }
            pending.clear();
        }
        me.spawn(move || sync(&nc, kind, &items, &pending), move |s, out| s.nextcloud_synced(kind, &account, fresh, out));
    }

    fn nextcloud_synced(&mut self, kind: Kind, account: &str, fresh: bool, out: Outcome) {
        self.nextcloud[kind as usize].running = false;
        let current = self.cfg.nextcloud.account() == account;
        self.nextcloud[kind as usize].error = out.error.clone();
        // A new account that failed at once has nothing of ours: keep the old ids.
        let answered = !fresh || out.server.is_some() || !out.pushed.is_empty();
        // Otherwise the account changed while it ran, or a new one never answered.
        if current && answered {
            let open = self.cfg.notes.get(self.notes.index).map(|n| (n.key, n.text.clone()));
            self.save(|c| {
                adopt(c, account);
                let (items, pending) = kind.lists(c);
                apply(items, pending, out);
            });
            // Keep the open note open where it went, and show Nextcloud's text if it changed.
            if kind == Kind::Notes {
                let at = open.as_ref().and_then(|(key, _)| self.cfg.notes.iter().position(|n| n.key == *key));
                match at {
                    Some(i) if open.as_ref().is_some_and(|(_, text)| self.cfg.notes[i].text == *text) => self.notes.index = i,
                    Some(i) => { let focused = self.notes.editor.focused; self.open_note(i); self.notes.editor.focused = focused }
                    None => self.open_note(self.notes.index),
                }
            }
        }
        // A changed account waits for the next sync asked of it, so no resync chases a URL that is still being typed.
        if std::mem::take(&mut self.nextcloud[kind as usize].again) && current { self.nextcloud_sync(kind) }
    }

    /// Syncs the Shelf with its Nextcloud folder in the background, as `nextcloud_sync` does a kind.
    pub fn nextcloud_shelf_sync(&mut self) {
        if !self.cfg.nextcloud.enabled() { self.shelf_sync.error = None; return }
        let (Some(me), Some(dir)) = (self.me.clone(), self.store.dir()) else { return };
        if self.shelf_sync.running { self.shelf_sync.again = true; return }
        self.shelf_sync.running = true;
        let account = self.cfg.nextcloud.account();
        let nc = self.cfg.nextcloud.clone();
        let folder = nc.shelf_folder();
        // Another account's folder says nothing of this one's: nothing counts as removed.
        let base: Vec<String> = if nc.synced_with == account { nc.shelf_synced.iter().filter_map(|e| in_folder(e, &folder)).map(str::to_string).collect() } else { Vec::new() };
        let files = self.cfg.shelf.clone();
        let at = folder.clone();
        me.spawn(move || sync_shelf(&nc, &at, &files, &base, &dir.join("Shelf")), move |s, out| s.nextcloud_shelf_synced(&account, &folder, out));
    }

    fn nextcloud_shelf_synced(&mut self, account: &str, folder: &str, out: ShelfOutcome) {
        self.shelf_sync.running = false;
        let current = self.cfg.nextcloud.account() == account;
        if current { self.shelf_sync.error = out.error }
        let changed = self.shelf_sync.again;
        if current && let Some(mut both) = out.both {
            self.save(|c| {
                adopt(c, account);
                if changed {
                    // The Shelf changed while it ran, so a file it found gone may have been dropped
                    // again: it stays on, and the rerun decides. Its name stays known unless the drop forgot it.
                    for f in &out.gone {
                        let Some(name) = f.file_name().map(|n| n.to_string_lossy().into_owned()) else { continue };
                        if c.nextcloud.shelf_synced.contains(&format!("{folder}/{name}")) { both.push(name) }
                    }
                } else {
                    c.shelf.retain(|f| !out.gone.contains(f));
                }
                for f in out.downloaded { if !c.shelf.contains(&f) { c.shelf.push(f) } }
                c.nextcloud.shelf_synced.retain(|e| in_folder(e, folder).is_none());
                c.nextcloud.shelf_synced.extend(both.into_iter().map(|n| format!("{folder}/{n}")));
            });
        }
        if std::mem::take(&mut self.shelf_sync.again) && current { self.nextcloud_shelf_sync() }
    }
}

/// Starts afresh with another account: the old one's ids mean nothing to it.
fn adopt(c: &mut Config, account: &str) {
    if c.nextcloud.synced_with == account { return }
    for k in Kind::ALL {
        let (items, pending) = k.lists(c);
        for i in items { i.remote = None }
        pending.clear();
    }
    c.nextcloud.shelf_synced.clear();
    c.nextcloud.synced_with = account.to_string();
}

/// The file name a `shelf_synced` entry has in `folder`, if it is in that folder.
fn in_folder<'a>(entry: &'a str, folder: &str) -> Option<&'a str> {
    entry.strip_prefix(folder)?.strip_prefix('/').filter(|n| !n.contains('/'))
}

// --- The network half ---

struct Client {
    base: String,
    /// `scheme://host[:port]`, which WebDAV paths are relative to.
    origin: String,
    user: String,
    auth: String,
    agent: ureq::Agent,
}

impl Client {
    fn new(nc: &Nextcloud) -> Client {
        let base = nc.url.trim().trim_end_matches('/');
        let base = if base.contains("://") { base.to_string() } else { format!("https://{base}") };
        let host = base.find("://").map_or(0, |i| i + 3);
        let origin = base[host..].find('/').map_or(base.clone(), |end| base[..host + end].to_string());
        let auth = base64::engine::general_purpose::STANDARD.encode(format!("{}:{}", nc.user.trim(), nc.password));
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .http_status_as_error(false)
            .allow_non_standard_methods(true)
            .build()
            .into();
        Client { base, origin, user: nc.user.trim().into(), auth: format!("Basic {auth}"), agent }
    }

    /// A WebDAV path (`/remote.php/dav/...`) as a URL on this server; `server_path` keeps out any other host.
    fn at(&self, href: &str) -> String { format!("{}{href}", self.origin) }

    /// One request: its status, ETag header and body.
    fn call(&self, method: &str, url: &str, headers: &[(&str, &str)], body: &str) -> Result<(u16, String, String), String> {
        let mut req = ureq::http::Request::builder().method(method).uri(url).header("authorization", &self.auth);
        for (k, v) in headers { req = req.header(*k, *v) }
        let req = req.body(body.to_string()).map_err(|e| e.to_string())?;
        let mut res = self.agent.run(req).map_err(|e| format!("Nextcloud: {e}"))?;
        let etag = res.headers().get("etag").and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
        let text = res.body_mut().with_config().limit(32 << 20).read_to_string().map_err(|e| e.to_string())?;
        Ok((res.status().as_u16(), etag, text))
    }
}

impl Client {
    /// A file transfer: an hour, not 30 seconds, as a big file takes long; a stalled one still ends.
    fn transfer(&self, method: &str, url: &str, body: impl ureq::AsSendBody) -> Result<ureq::http::Response<ureq::Body>, String> {
        let req = ureq::http::Request::builder().method(method).uri(url).header("authorization", &self.auth).body(body).map_err(|e| e.to_string())?;
        let wait = Some(Duration::from_secs(30));
        let req = self.agent.configure_request(req).timeout_global(Some(Duration::from_secs(3600))).timeout_connect(wait).timeout_recv_response(wait).build();
        self.agent.run(req).map_err(|e| format!("Nextcloud: {e}"))
    }
}

fn ok(status: u16) -> bool { (200..300).contains(&status) }

fn failed(status: u16) -> String {
    match status {
        401 | 403 => "Nextcloud refused the user or app password".into(),
        s => format!("Nextcloud answered {s}"),
    }
}

/// Uploads what changed here, then reads back everything Nextcloud has.
pub fn sync(nc: &Nextcloud, kind: Kind, items: &[Item], pending: &[String]) -> Outcome {
    let c = Client::new(nc);
    let mut out = Outcome::default();
    let result = match kind {
        Kind::Notes => sync_notes(&c, items, pending, &mut out),
        Kind::Tasks => sync_tasks(&c, &nc.list, items, pending, &mut out),
        Kind::Events => sync_events(&c, &nc.calendar, items, pending, &mut out),
    };
    out.error = result.err();
    out
}

/// A WebDAV href from the server, if it is a path on that server: credentials go nowhere else.
fn server_path(href: String) -> Option<String> { (href.starts_with('/') && !href.starts_with("//")).then_some(href) }

// --- Notes: https://github.com/nextcloud/notes/blob/main/docs/api/v1.md ---

fn note(v: &Value) -> Option<Remote> {
    Some(Remote { id: v["id"].as_u64()?.to_string(), etag: v["etag"].as_str().unwrap_or_default().into(), text: v["content"].as_str()?.into() })
}

fn sync_notes(c: &Client, items: &[Item], pending: &[String], out: &mut Outcome) -> Result<(), String> {
    let url = format!("{}/index.php/apps/notes/api/v1/notes", c.base);
    let json_body = [("content-type", "application/json"), ("accept", "application/json")];
    for id in pending {
        let (status, ..) = c.call("DELETE", &format!("{url}/{id}"), &json_body, "")?;
        if !ok(status) && status != 404 { return Err(failed(status)) }
        out.cleared.push(id.clone());
    }
    // A new note whose text Nextcloud already has, unclaimed, is that note: an upload whose
    // answer was lost, or the same note on both sides. Taking it saves a duplicate.
    let listed = list_notes(c, &url)?;
    let mut claimed: Vec<&str> = items.iter().filter_map(|i| i.remote.as_ref().map(|r| r.id.as_str())).collect();
    for item in items {
        if item.remote.is_none() && let Some(n) = listed.iter().find(|n| n.text == item.text && !claimed.contains(&n.id.as_str())) {
            claimed.push(&n.id);
            out.pushed.push((item.key, n.clone()));
            continue;
        }
        let title = item.text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("New Note");
        let body = json!({ "content": item.text, "title": title }).to_string();
        let updated = match &item.remote {
            None if item.text.trim().is_empty() => continue,
            None => None,
            Some(r) if r.text == item.text => continue,
            Some(r) => {
                let if_match = format!("\"{}\"", r.etag);
                let (status, _, res) = c.call("PUT", &format!("{url}/{}", r.id), &[json_body[0], json_body[1], ("if-match", &if_match)], &body)?;
                match status {
                    s if ok(s) => Some(res),
                    // Deleted or changed there too: this copy becomes a new note, so neither is lost.
                    404 | 412 => None,
                    s => return Err(failed(s)),
                }
            }
        };
        let res = match updated {
            Some(res) => res,
            None => {
                let (status, _, res) = c.call("POST", &url, &json_body, &body)?;
                if !ok(status) { return Err(failed(status)) }
                res
            }
        };
        let remote = serde_json::from_str(&res).ok().as_ref().and_then(note).ok_or("Nextcloud sent an unreadable note")?;
        // The text sent, so a note Nextcloud reformats isn't seen as changed here.
        out.pushed.push((item.key, Remote { text: item.text.clone(), ..remote }));
    }
    out.server = Some(list_notes(c, &url)?);
    Ok(())
}

/// Every note on Nextcloud, the latest changed first.
fn list_notes(c: &Client, url: &str) -> Result<Vec<Remote>, String> {
    let (status, _, res) = c.call("GET", url, &[("accept", "application/json")], "")?;
    if !ok(status) { return Err(failed(status)) }
    let list: Vec<Value> = serde_json::from_str(&res).map_err(|_| "Nextcloud Notes isn't installed on that server".to_string())?;
    let mut notes: Vec<_> = list.iter().filter_map(|v| Some((v["modified"].as_i64().unwrap_or(0), note(v)?))).collect();
    notes.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    Ok(notes.into_iter().map(|(_, n)| n).collect())
}

// --- Tasks: CalDAV (RFC 4791) VTODOs in one of the user's calendars ---

const LISTS: &str = r#"<?xml version="1.0"?><d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:prop><d:displayname/><c:supported-calendar-component-set/></d:prop></d:propfind>"#;
const OPEN_TASKS: &str = r#"<?xml version="1.0"?><c:calendar-query xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:prop><d:getetag/><c:calendar-data/></d:prop><c:filter><c:comp-filter name="VCALENDAR"><c:comp-filter name="VTODO"><c:prop-filter name="COMPLETED"><c:is-not-defined/></c:prop-filter></c:comp-filter></c:comp-filter></c:filter></c:calendar-query>"#;
const XML: (&str, &str) = ("content-type", "application/xml; charset=utf-8");
const ICS: (&str, &str) = ("content-type", "text/calendar; charset=utf-8");

fn sync_tasks(c: &Client, list: &str, items: &[Item], pending: &[String], out: &mut Outcome) -> Result<(), String> {
    let calendar = find_list(c, list, "VTODO", out)?;
    let stamp = utc_stamp(0);
    for href in pending {
        let Some(path) = server_path(href.clone()) else { out.cleared.push(href.clone()); continue };
        let url = c.at(&path);
        let (status, etag, ics) = c.call("GET", &url, &[], "")?;
        if ok(status) {
            let mut headers = vec![ICS];
            if !etag.is_empty() { headers.push(("if-match", &etag)) }
            let (status, ..) = c.call("PUT", &url, &headers, &completed(&ics, &stamp))?;
            if !ok(status) && status != 404 { return Err(failed(status)) }
        } else if status != 404 {
            return Err(failed(status));
        }
        out.cleared.push(href.clone());
    }
    let listed = list_objects(c, &calendar, OPEN_TASKS, open_task)?;
    upload_new(c, items, &listed, out, |uid, text| Some(new_task(uid, text, &stamp)))?;
    out.server = Some(list_objects(c, &calendar, OPEN_TASKS, open_task)?);
    Ok(())
}

/// Uploads each new item as a calendar object, `ics(uid, text)`; None skips it. A listed object
/// with the same text, unclaimed, is that item instead (as with notes).
fn upload_new(c: &Client, items: &[Item], listed: &[Remote], out: &mut Outcome, ics: impl Fn(&str, &str) -> Option<String>) -> Result<(), String> {
    let mut claimed: Vec<&str> = items.iter().filter_map(|i| i.remote.as_ref().map(|r| r.id.as_str())).collect();
    for item in items.iter().filter(|i| i.remote.is_none() && !i.text.trim().is_empty()) {
        if let Some(t) = listed.iter().find(|t| t.text == item.text && !claimed.contains(&t.id.as_str())) {
            claimed.push(&t.id);
            out.pushed.push((item.key, t.clone()));
            continue;
        }
        // Named by the item's key, so a retry after a lost answer finds it already there (412).
        let uid = format!("ripple-{:016x}", item.key);
        let Some(body) = ics(&uid, &item.text) else { continue };
        let href = format!("{}{uid}.ics", out.scope);
        let (status, etag, _) = c.call("PUT", &c.at(&href), &[ICS, ("if-none-match", "*")], &body)?;
        if !ok(status) && status != 412 { return Err(failed(status)) }
        out.pushed.push((item.key, Remote { id: href, etag, text: item.text.clone() }));
    }
    Ok(())
}

/// The user's calendar or Tasks list that `want` names and that holds `comp`, which `out` then scopes to.
fn find_list(c: &Client, want: &str, comp: &str, out: &mut Outcome) -> Result<String, String> {
    let home = format!("{}/remote.php/dav/calendars/{}/", c.base, percent_encode(&c.user));
    let (status, _, res) = c.call("PROPFIND", &home, &[XML, ("depth", "1")], LISTS)?;
    if status != 207 { return Err(failed(status)) }
    let calendar = pick_list(&res, want.trim(), comp).ok_or_else(|| match (want.trim(), comp) {
        ("", "VTODO") => "No Nextcloud list holds tasks".to_string(),
        ("", _) => "No Nextcloud calendar holds events".to_string(),
        (name, _) => format!("No Nextcloud list is named {name}"),
    })?;
    out.scope = format!("{}/", calendar.trim_end_matches('/'));
    Ok(calendar)
}

/// Every calendar object `query` matches, as `text` reads it; objects it reads as None are left out.
fn list_objects(c: &Client, calendar: &str, query: &str, text: fn(&str) -> Option<String>) -> Result<Vec<Remote>, String> {
    let (status, _, res) = c.call("REPORT", &c.at(calendar), &[XML, ("depth", "1")], query)?;
    if status != 207 { return Err(failed(status)) }
    Ok(elements(&res, "response").into_iter().filter_map(|response| {
        let first = |name| elements(response, name).first().map(|v| unescape(v)).unwrap_or_default();
        Some(Remote { id: server_path(first("href"))?, etag: first("getetag"), text: text(&first("calendar-data"))? })
    }).collect())
}

/// The list named `want` (by name or path) that holds `comp`. Else, for tasks, the first that holds
/// only tasks, else the first that holds tasks; for events, the first that holds events, past the
/// read-only birthdays calendar.
fn pick_list(xml: &str, want: &str, comp: &str) -> Option<String> {
    let lists: Vec<(String, String, &str)> = elements(xml, "response").into_iter().filter_map(|r| {
        let comps = *elements(r, "supported-calendar-component-set").first()?;
        let href = server_path(unescape(elements(r, "href").first()?))?;
        comps.contains(&format!("\"{comp}\"")).then(|| (href, elements(r, "displayname").first().map(|n| unescape(n)).unwrap_or_default(), comps))
    }).collect();
    let slug = |href: &str| href.trim_end_matches('/').rsplit('/').next().unwrap_or_default().to_string();
    if !want.is_empty() {
        return lists.into_iter().find(|(href, name, _)| name.eq_ignore_ascii_case(want) || slug(href).eq_ignore_ascii_case(want)).map(|l| l.0);
    }
    let first = match comp {
        "VTODO" => lists.iter().position(|l| !l.2.contains("\"VEVENT\"")).unwrap_or(0),
        _ => lists.iter().position(|l| slug(&l.0) != "contact_birthdays")?,
    };
    lists.into_iter().nth(first).map(|l| l.0)
}

/// The contents of each `<name>` element, whatever its namespace prefix. Enough for WebDAV's
/// multistatus answers; not a general XML parser.
fn elements<'a>(xml: &'a str, name: &str) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(i) = rest.find('<') {
        rest = &rest[i + 1..];
        // Every scan stops at the next '<', so a hostile answer parses in linear time.
        let end = rest.find(|c: char| c.is_whitespace() || matches!(c, '>' | '/' | '<')).unwrap_or(rest.len());
        let tag = &rest[..end];
        if tag.is_empty() || tag.rsplit(':').next() != Some(name) { continue }
        let Some(close) = rest.find(['>', '<']) else { break };
        if rest[close..].starts_with('<') { continue }
        if rest[..close].ends_with('/') {
            out.push("");
            continue;
        }
        let body = &rest[close + 1..];
        let Some(stop) = body.find(&format!("</{tag}>")) else { break };
        out.push(&body[..stop]);
        rest = &body[stop..];
    }
    out
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        // Look no further than the next '&': a run of them must not rescan the rest each time.
        let entity = rest[1..].find([';', '&']).filter(|&j| rest[1 + j..].starts_with(';')).map(|j| &rest[1..1 + j]).unwrap_or_default();
        let c = match entity {
            "lt" => Some('<'),
            "gt" => Some('>'),
            "amp" => Some('&'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            e => match e.strip_prefix('#') {
                Some(n) => match n.strip_prefix(['x', 'X']) { Some(hex) => u32::from_str_radix(hex, 16).ok(), None => n.parse().ok() }.and_then(char::from_u32),
                None => None,
            },
        };
        match c {
            Some(c) => { out.push(c); rest = &rest[entity.len() + 2..] }
            None => { out.push('&'); rest = &rest[1..] }
        }
    }
    out + rest
}

/// iCalendar lines with their folds joined.
fn unfold(ics: &str) -> String { ics.replace("\r\n", "\n").replace("\n ", "").replace("\n\t", "") }

/// The summary of the calendar object's to-do, unless it is done.
fn open_task(ics: &str) -> Option<String> {
    let (mut inside, mut summary, mut open) = (false, None, true);
    for line in unfold(ics).lines() {
        let upper = line.to_ascii_uppercase();
        // An alarm inside the to-do has a SUMMARY of its own.
        if upper == "BEGIN:VTODO" || upper == "END:VALARM" { inside = true; continue }
        if upper == "BEGIN:VALARM" { inside = false; continue }
        if upper == "END:VTODO" { break }
        let Some((name, value)) = line.split_once(':').filter(|_| inside) else { continue };
        match name.split(';').next().unwrap_or_default().to_ascii_uppercase().as_str() {
            "SUMMARY" => summary = Some(unescape_text(value)),
            "STATUS" => open &= !matches!(value.trim().to_ascii_uppercase().as_str(), "COMPLETED" | "CANCELLED"),
            "COMPLETED" => open = false,
            _ => {}
        }
    }
    summary.filter(|_| open)
}

/// An iCalendar TEXT value; a line break becomes a space, as tasks are one line here.
fn unescape_text(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        out.push(match c {
            '\\' => match chars.next() { Some('n' | 'N') => ' ', Some(c) => c, None => '\\' },
            c => c,
        });
    }
    out
}

/// A TEXT value for iCalendar.
fn escape_text(value: &str) -> String { value.replace('\\', "\\\\").replace(';', "\\;").replace(',', "\\,").replace('\n', "\\n").replace('\r', "") }

fn new_task(uid: &str, summary: &str, stamp: &str) -> String {
    let summary = escape_text(summary);
    format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Ripple//EN\r\nBEGIN:VTODO\r\nUID:{uid}\r\nDTSTAMP:{stamp}\r\nCREATED:{stamp}\r\nSUMMARY:{summary}\r\nSTATUS:NEEDS-ACTION\r\nEND:VTODO\r\nEND:VCALENDAR\r\n")
}

/// The calendar object with its to-dos marked done, everything else kept.
fn completed(ics: &str, stamp: &str) -> String {
    let mut out = String::new();
    let mut inside = false;
    for line in unfold(ics).lines() {
        let upper = line.to_ascii_uppercase();
        let name = upper.split([':', ';']).next().unwrap_or_default();
        if upper == "BEGIN:VTODO" { inside = true }
        if inside && ["STATUS", "COMPLETED", "PERCENT-COMPLETE"].contains(&name) { continue }
        if inside && upper == "END:VTODO" {
            out.push_str(&format!("STATUS:COMPLETED\r\nCOMPLETED:{stamp}\r\nPERCENT-COMPLETE:100\r\n"));
            inside = false;
        }
        out.push_str(line);
        out.push_str("\r\n");
    }
    out
}

// --- Shelf: files in one Nextcloud Files folder, over WebDAV ---

const FOLDER: &str = r#"<?xml version="1.0"?><d:propfind xmlns:d="DAV:"><d:prop><d:resourcetype/><d:getcontentlength/></d:prop></d:propfind>"#;

#[derive(Debug, Default, PartialEq)]
pub struct ShelfOutcome {
    /// The names both sides have now; None when the folder couldn't be read.
    both: Option<Vec<String>>,
    /// Shelf files removed on Nextcloud, so off the Shelf here too. The files themselves stay.
    gone: Vec<PathBuf>,
    /// Files new on Nextcloud, downloaded into the cache folder.
    downloaded: Vec<PathBuf>,
    /// Names deleted on Nextcloud, as they left the Shelf here.
    removed: Vec<String>,
    error: Option<String>,
}

/// Two-way sync of the Shelf with `folder`, by file name, against `base`, what both had at the
/// last sync: a name one side lacks is new on the other, unless it is in `base`, when it was removed.
/// shortcut: a file changed on one side after it synced isn't sent again; compare ETags if people ask.
pub fn sync_shelf(nc: &Nextcloud, folder: &str, files: &[PathBuf], base: &[String], cache: &Path) -> ShelfOutcome {
    let c = Client::new(nc);
    let mut out = ShelfOutcome::default();
    out.error = match shelf(&c, folder, files, base, cache, &mut out) {
        Ok(skipped) => skipped,
        // Stopped partway: what wasn't reached keeps its place in `base`, so the next sync still
        // knows it was removed on one side.
        Err(e) => {
            let gone: Vec<_> = out.gone.iter().filter_map(|f| f.file_name().map(|n| n.to_string_lossy().into_owned())).collect();
            if let Some(both) = &mut out.both {
                for b in base { if !both.contains(b) && !out.removed.contains(b) && !gone.contains(b) { both.push(b.clone()) } }
            }
            Some(e)
        }
    };
    out
}

/// Ok with a file that was skipped, or Err when the sync stopped partway.
fn shelf(c: &Client, folder: &str, files: &[PathBuf], base: &[String], cache: &Path, out: &mut ShelfOutcome) -> Result<Option<String>, String> {
    let mut url = format!("{}/remote.php/dav/files/{}/", c.base, percent_encode(&c.user));
    let mut made = Vec::new();
    for part in folder.split('/').filter(|p| !matches!(*p, "" | "." | "..")) {
        url = format!("{url}{}/", percent_encode(part));
        made.push(url.clone());
    }
    let (status, _, res) = c.call("PROPFIND", &url, &[XML, ("depth", "1")], FOLDER)?;
    let sizes = match status {
        207 => folder_files(&res),
        // Not made yet: each missing level is (405 is one already there).
        404 => {
            for level in made {
                let (status, ..) = c.call("MKCOL", &level, &[], "")?;
                if !ok(status) && status != 405 { return Err(failed(status)) }
            }
            Vec::new()
        }
        s => return Err(failed(s)),
    };
    let listed: Vec<String> = sizes.iter().map(|(n, _)| n.clone()).collect();
    // shortcut: of two Shelf files with one name, the first syncs; the other stays here only.
    let mut local: Vec<(String, &PathBuf)> = Vec::new();
    for f in files {
        let Some(name) = f.file_name().map(|n| n.to_string_lossy().into_owned()) else { continue };
        if !local.iter().any(|(n, _)| *n == name) { local.push((name, f)) }
    }
    let here = |name: &String| local.iter().any(|(n, _)| n == name);
    // A name met for the first time on both sides is one file only if the bytes match. Otherwise
    // it is two: neither is taken for the other, so removing one never deletes the other.
    let mut clashes = Vec::new();
    for (name, path) in &local {
        let Some((_, theirs)) = sizes.iter().find(|(n, _)| n == name).filter(|_| !base.contains(name)) else { continue };
        let mine = std::fs::metadata(path).ok().filter(|m| m.is_file()).map(|m| m.len());
        let same = theirs.is_some() && *theirs == mine && {
            let mut res = c.transfer("GET", &format!("{url}{}", percent_encode(name)), ())?;
            if matches!(res.status().as_u16(), 401 | 403) { return Err(failed(res.status().as_u16())) }
            ok(res.status().as_u16()) && std::fs::File::open(path).is_ok_and(|f| same_bytes(res.body_mut().as_reader(), f))
        };
        if !same { clashes.push(name.clone()) }
    }
    let clash = |name: &String| clashes.contains(name);
    let removed_here: Vec<&String> = listed.iter().filter(|n| !here(n) && base.contains(n)).collect();
    // One file that fails (too big, a name this OS can't save) doesn't hold up the rest; its error still shows.
    let mut skipped = None;
    let both = out.both.insert(listed.iter().filter(|n| here(n) && !clash(n)).chain(removed_here.iter().copied()).cloned().collect());
    for name in removed_here {
        let (status, ..) = c.call("DELETE", &format!("{url}{}", percent_encode(name)), &[], "")?;
        if matches!(status, 401 | 403) { return Err(failed(status)) }
        // Still there, so still in `both`: the next sync tries again.
        if !ok(status) && status != 404 { skipped.get_or_insert(format!("Couldn't remove {name}: {}", failed(status))); continue }
        both.retain(|n| n != name);
        out.removed.push(name.clone());
    }
    for (name, path) in &local {
        if listed.contains(name) {
            if clash(name) { skipped.get_or_insert(format!("Nextcloud already has another {name}; rename one to sync it")); }
            continue;
        }
        if base.contains(name) { out.gone.push((*path).clone()); continue }
        // shortcut: folders and files that are missing or unreadable stay here only.
        if !path.is_file() { continue }
        let Ok(file) = std::fs::File::open(path) else { continue };
        let status = c.transfer("PUT", &format!("{url}{}", percent_encode(name)), file)?.status().as_u16();
        if matches!(status, 401 | 403) { return Err(failed(status)) }
        if !ok(status) { skipped.get_or_insert(format!("Couldn't upload {name}: {}", failed(status))); continue }
        both.push(name.clone());
    }
    for name in listed.iter().filter(|n| !here(n) && !base.contains(n)) {
        let mut res = c.transfer("GET", &format!("{url}{}", percent_encode(name)), ())?;
        let status = res.status().as_u16();
        if matches!(status, 401 | 403) { return Err(failed(status)) }
        if !ok(status) { skipped.get_or_insert(format!("Couldn't download {name}: {}", failed(status))); continue }
        // Written beside, then renamed, so a broken download never sits on the Shelf.
        // shortcut: a downloaded file stays in the cache after it leaves the Shelf; prune it if the folder grows.
        let (path, part) = (cache.join(name), cache.join(format!(".{name}.ripple-download")));
        let copied = std::fs::create_dir_all(cache).and_then(|_| std::fs::File::create(&part)).and_then(|mut f| std::io::copy(&mut res.body_mut().as_reader(), &mut f)).and_then(|_| std::fs::rename(&part, &path));
        if let Err(e) = copied { let _ = std::fs::remove_file(&part); skipped.get_or_insert(format!("Couldn't save {name}: {e}")); continue }
        out.downloaded.push(path);
        both.push(name.clone());
    }
    Ok(skipped)
}

/// Whether two readers hold the same bytes, read a piece at a time.
fn same_bytes(mut a: impl std::io::Read, mut b: impl std::io::Read) -> bool {
    let (mut x, mut y) = ([0u8; 8192], [0u8; 8192]);
    loop {
        let Ok(n) = a.read(&mut x) else { return false };
        if n == 0 { return b.read(&mut y[..1]).is_ok_and(|m| m == 0) }
        if b.read_exact(&mut y[..n]).is_err() || x[..n] != y[..n] { return false }
    }
}

/// The files (not folders) a PROPFIND of a folder lists, by name, with their sizes. A name that
/// could leave the cache folder, or that Windows takes as a device or a stream, is left out.
fn folder_files(xml: &str) -> Vec<(String, Option<u64>)> {
    elements(xml, "response").into_iter().filter_map(|r| {
        let href = unescape(elements(r, "href").first()?);
        if href.ends_with('/') { return None }
        let name = percent_decode(href.rsplit('/').next()?);
        let size = elements(r, "getcontentlength").first().and_then(|s| s.trim().parse().ok());
        let stem = name.split('.').next().unwrap_or_default().trim_end().to_ascii_uppercase();
        let device = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL") || (stem.len() == 4 && (stem.starts_with("COM") || stem.starts_with("LPT")) && stem.as_bytes()[3].is_ascii_digit());
        (Path::new(&name).file_name() == Some(name.as_ref()) && !name.contains(['/', '\\', ':']) && !device).then_some((name, size))
    }).collect()
}

// --- Calendar events: CalDAV VEVENTs in one of the user's calendars ---

/// Events that end after `since` (a UTC stamp). Older ones drop out of the Calendar tab.
fn events_query(since: &str) -> String {
    format!(r#"<?xml version="1.0"?><c:calendar-query xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:prop><d:getetag/><c:calendar-data/></d:prop><c:filter><c:comp-filter name="VCALENDAR"><c:comp-filter name="VEVENT"><c:time-range start="{since}"/></c:comp-filter></c:comp-filter></c:filter></c:calendar-query>"#)
}

fn sync_events(c: &Client, want: &str, items: &[Item], pending: &[String], out: &mut Outcome) -> Result<(), String> {
    let calendar = find_list(c, want, "VEVENT", out)?;
    for href in pending {
        if let Some(path) = server_path(href.clone()) {
            let (status, ..) = c.call("DELETE", &c.at(&path), &[], "")?;
            if !ok(status) && status != 404 { return Err(failed(status)) }
        }
        out.cleared.push(href.clone());
    }
    let (stamp, since) = (utc_stamp(0), utc_stamp(365));
    let query = events_query(&since);
    let listed = list_objects(c, &calendar, &query, event)?;
    // An event before the listed window stays here only: uploaded, the next listing would miss it and drop it.
    let listable = |(y, m, d): (i32, u32, u32)| format!("{y:04}{m:02}{d:02}").as_str() >= &since[..8];
    upload_new(c, items, &listed, out, |uid, text| event_day(text).filter(|(day, _)| listable(*day)).map(|(day, title)| new_event(uid, day, title, &stamp)))?;
    out.server = Some(list_objects(c, &calendar, &query, event)?);
    Ok(())
}

/// An event's day and title: its text is `YYYY-MM-DD title`, where the title starts with the
/// time (`HH:MM`) when the event has one.
pub fn event_day(text: &str) -> Option<((i32, u32, u32), &str)> {
    let (date, title) = text.split_once(' ')?;
    let mut parts = date.splitn(3, '-').map(str::parse::<u32>);
    let (y, m, d) = (parts.next()?.ok()?, parts.next()?.ok()?, parts.next()?.ok()?);
    ((1..=12).contains(&m) && (1..=31).contains(&d)).then_some(((y as i32, m, d), title))
}

/// The calendar object's first event as an event's text, unless it was cancelled.
/// shortcut: a recurring event shows on its first day only; expand RRULEs if people ask.
fn event(ics: &str) -> Option<String> {
    let (mut inside, mut summary, mut start, mut open) = (false, String::new(), None, true);
    for line in unfold(ics).lines() {
        let upper = line.to_ascii_uppercase();
        if upper == "BEGIN:VEVENT" || upper == "END:VALARM" { inside = true; continue }
        if upper == "BEGIN:VALARM" { inside = false; continue }
        if upper == "END:VEVENT" { break }
        let Some((name, value)) = line.split_once(':').filter(|_| inside) else { continue };
        match name.split(';').next().unwrap_or_default().to_ascii_uppercase().as_str() {
            "SUMMARY" => summary = unescape_text(value),
            "DTSTART" => start = Some(value.trim().to_string()),
            "STATUS" => open = !value.trim().eq_ignore_ascii_case("CANCELLED"),
            _ => {}
        }
    }
    let start = start?;
    let num = |from: usize, to: usize| start.get(from..to)?.parse::<u32>().ok();
    let (y, m, d) = (num(0, 4)? as i32, num(4, 6)?, num(6, 8)?);
    let summary = if summary.trim().is_empty() { "Untitled".to_string() } else { summary };
    let text = match start.get(8..)? {
        "" => format!("{y:04}-{m:02}-{d:02} {summary}"),
        t if t.starts_with('T') => {
            let (h, min) = (num(9, 11)?, num(11, 13)?);
            // UTC moves to this clock; a TZID time is taken as written, as it is usually this zone.
            let (y, m, d, h, min) = if t.ends_with('Z') {
                let l = rune::time::LocalTime::at((days_from_civil(y, m, d) * 86_400 + i64::from(h * 3600 + min * 60)) * 1000);
                (l.year, l.month, l.day, l.hour, l.minute)
            } else { (y, m, d, h, min) };
            format!("{y:04}-{m:02}-{d:02} {h:02}:{min:02} {summary}")
        }
        _ => return None,
    };
    open.then_some(text)
}

/// An event on `day`: at the title's leading `HH:MM`, on this clock, else all day. `event` reads
/// either back as the same text.
fn new_event(uid: &str, (y, m, d): (i32, u32, u32), title: &str, stamp: &str) -> String {
    let time = title.split_once(' ').filter(|(t, _)| t.len() == 5 && t.as_bytes()[2] == b':')
        .and_then(|(t, rest)| Some((t[..2].parse::<u32>().ok().filter(|h| *h < 24)?, t[3..].parse::<u32>().ok().filter(|m| *m < 60)?, rest)));
    let (when, summary) = match time {
        Some((h, min, rest)) => (format!("DTSTART:{y:04}{m:02}{d:02}T{h:02}{min:02}00"), rest),
        None => {
            let next = rune::time::LocalTime::at_offset((days_from_civil(y, m, d) + 1) * 86_400_000, 0);
            (format!("DTSTART;VALUE=DATE:{y:04}{m:02}{d:02}\r\nDTEND;VALUE=DATE:{:04}{:02}{:02}", next.year, next.month, next.day), title)
        }
    };
    format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Ripple//EN\r\nBEGIN:VEVENT\r\nUID:{uid}\r\nDTSTAMP:{stamp}\r\nCREATED:{stamp}\r\n{when}\r\nSUMMARY:{}\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n", escape_text(summary))
}

/// Now, or `days_ago` days back, in UTC.
fn utc_stamp(days_ago: i64) -> String {
    let t = rune::time::LocalTime::at_offset(crate::date::now_ms() - days_ago * 86_400_000, 0);
    format!("{:04}{:02}{:02}T{:02}{:02}{:02}Z", t.year, t.month, t.day, t.hour, t.minute, t.second)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::testserver::{Response, json as reply, serve};

    fn item(text: &str, key: u64, remote: Option<(&str, &str, &str)>) -> Item {
        Item { text: text.into(), key, remote: remote.map(|(id, etag, text)| Remote { id: id.into(), etag: etag.into(), text: text.into() }) }
    }
    fn remote(id: &str, etag: &str, text: &str) -> Remote { Remote { id: id.into(), etag: etag.into(), text: text.into() } }

    #[test]
    fn undoing_a_removal_calls_off_an_unsent_delete_and_lets_a_sent_one_go() {
        let linked = |c: &mut Config| { c.notes = vec![item("a", 1, Some(("n1", "", "a")))]; c.nextcloud.deleted_notes = vec!["n0".into()] };
        let mut c = Config::default();
        linked(&mut c);
        // Not sent: the delete is dropped and the link kept, so no copy is made.
        let (at, gone) = remove(&mut c, Kind::Notes, 1).unwrap();
        assert_eq!(c.nextcloud.deleted_notes, ["n0", "n1"]);
        restore(&mut c, Kind::Notes, at, gone, false);
        assert_eq!((c.notes[0].remote.is_some(), c.nextcloud.deleted_notes.as_slice()), (true, ["n0".to_string()].as_slice()));
        // Maybe in the running sync: the delete stays and the item uploads afresh.
        let (at, gone) = remove(&mut c, Kind::Notes, 1).unwrap();
        restore(&mut c, Kind::Notes, at, gone, true);
        assert_eq!((c.notes[0].remote.is_none(), c.nextcloud.deleted_notes.len()), (true, 2));
        // Already sent: nothing is pending, the item uploads afresh.
        let mut c = Config::default();
        linked(&mut c);
        let (at, gone) = remove(&mut c, Kind::Notes, 1).unwrap();
        c.nextcloud.deleted_notes.clear();
        restore(&mut c, Kind::Notes, at, gone, false);
        assert!(c.notes[0].remote.is_none());
    }

    #[test]
    fn a_sync_merges_without_losing_edits() {
        let mut items = vec![
            item("unchanged", 1, Some(("a", "1", "unchanged"))),
            item("edited here", 2, Some(("b", "1", "old"))),
            item("gone there", 3, Some(("c", "1", "gone there"))),
            item("edited, gone there", 4, Some(("d", "1", "old"))),
            item("typed during sync", 5, None),
        ];
        let mut pending = vec!["x".to_string(), "y".to_string()];
        let out = Outcome {
            // Item 5 uploaded as it was; item 9 was deleted while it uploaded.
            pushed: vec![(5, remote("e", "1", "typed")), (9, remote("f", "1", "lost"))],
            cleared: vec!["x".into()],
            server: Some(vec![remote("a", "2", "changed there"), remote("b", "2", "also there"), remote("e", "1", "typed"), remote("g", "1", "new there"), remote("y", "1", "deleting")]),
            error: None,
            scope: String::new(),
        };
        apply(&mut items, &mut pending, out);
        let texts: Vec<_> = items.iter().map(|i| (i.text.as_str(), i.remote.as_ref().map(|r| (r.id.as_str(), r.etag.as_str())))).collect();
        assert_eq!(texts, [
            ("changed there", Some(("a", "2"))),
            ("edited here", Some(("b", "1"))),
            ("edited, gone there", None),
            ("typed during sync", Some(("e", "1"))),
            ("new there", Some(("g", "1"))),
        ]);
        assert_eq!(pending, ["y", "f"]);

        // A sync that stopped before reading the server keeps its uploads and removes nothing.
        let mut items = vec![item("kept", 1, Some(("a", "1", "kept"))), item("up", 2, None)];
        apply(&mut items, &mut Vec::new(), Outcome { pushed: vec![(2, remote("b", "1", "up"))], error: Some("x".into()), ..Outcome::default() });
        assert_eq!(items.iter().map(|i| i.remote.as_ref().map(|r| r.id.as_str())).collect::<Vec<_>>(), [Some("a"), Some("b")]);

        // A task synced to another list isn't taken as deleted when this list lacks it.
        let mut items = vec![item("old list", 1, Some(("/cal/tasks/a.ics", "1", "old list"))), item("this list", 2, Some(("/cal/personal/b.ics", "1", "this list")))];
        apply(&mut items, &mut Vec::new(), Outcome { server: Some(Vec::new()), scope: "/cal/personal/".into(), ..Outcome::default() });
        assert_eq!(items.iter().map(|i| i.text.as_str()).collect::<Vec<_>>(), ["old list"]);
    }

    #[test]
    fn webdav_answers_parse() {
        let xml = r#"<d:multistatus xmlns:d="DAV:"><d:response><d:href>/dav/cal/personal/</d:href><d:propstat><d:prop><d:displayname>Personal</d:displayname><cal:supported-calendar-component-set><cal:comp name="VEVENT"/><cal:comp name="VTODO"/></cal:supported-calendar-component-set></d:prop></d:propstat></d:response><d:response><d:href>/dav/cal/tasks/</d:href><d:propstat><d:prop><d:displayname>Tasks &amp; more</d:displayname><cal:supported-calendar-component-set><cal:comp name="VTODO"/></cal:supported-calendar-component-set></d:prop></d:propstat></d:response><d:response><d:href>/dav/cal/</d:href><d:propstat><d:prop><d:displayname/></d:prop></d:propstat></d:response></d:multistatus>"#;
        assert_eq!(pick_list(xml, "", "VTODO"), Some("/dav/cal/tasks/".into()));
        assert_eq!(pick_list(xml, "personal", "VTODO"), Some("/dav/cal/personal/".into()));
        assert_eq!(pick_list(xml, "Tasks & more", "VTODO"), Some("/dav/cal/tasks/".into()));
        assert_eq!(pick_list(xml, "nope", "VTODO"), None);
        assert_eq!(pick_list(xml, "", "VEVENT"), Some("/dav/cal/personal/".into()));
        assert_eq!(pick_list(xml, "Tasks & more", "VEVENT"), None, "that list holds no events");
        let birthdays = xml.replace("/dav/cal/personal/", "/dav/cal/contact_birthdays/");
        assert_eq!(pick_list(&birthdays, "", "VEVENT"), None);
        assert_eq!(unescape("a&lt;b&#13;&#x41;&bogus;"), "a<b\rA&bogus;");
        assert_eq!(unescape("&&amp;;&"), "&&;&");
        // Hostile answers that once took quadratic time.
        let start = std::time::Instant::now();
        assert!(elements(&"<x".repeat(100_000), "response").is_empty());
        assert_eq!(elements(&format!("{}/>", "<response ".repeat(20_000)), "response").len(), 1);
        assert_eq!(unescape(&"&".repeat(200_000)).len(), 200_000);
        // Linear takes milliseconds; the old quadratic scan took minutes on these inputs.
        assert!(start.elapsed() < std::time::Duration::from_secs(10), "linear parse took {:?}", start.elapsed());
    }

    #[test]
    fn todos_read_and_complete() {
        let ics = "BEGIN:VCALENDAR\r\nBEGIN:VTODO\r\nUID:1\r\nSUMMARY;LANGUAGE=en:Buy milk\\, eggs and a very long\r\n  line\r\nSTATUS:NEEDS-ACTION\r\nDUE:20260101\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";
        assert_eq!(open_task(ics).as_deref(), Some("Buy milk, eggs and a very long line"));
        let done = completed(ics, "20261118T000000Z");
        assert_eq!(open_task(&done), None);
        assert!(done.contains("DUE:20260101\r\nSTATUS:COMPLETED\r\nCOMPLETED:20261118T000000Z\r\nPERCENT-COMPLETE:100\r\nEND:VTODO") && !done.contains("NEEDS-ACTION"));
        assert_eq!(open_task(&new_task("u", "a, b; c \\n", "20260101T000000Z")).as_deref(), Some("a, b; c \\n"));
        assert_eq!(open_task("BEGIN:VTODO\r\nSUMMARY:Task\r\nBEGIN:VALARM\r\nSUMMARY:Alarm\r\nEND:VALARM\r\nEND:VTODO").as_deref(), Some("Task"));
    }

    #[test]
    fn events_read_and_write() {
        let ics = "BEGIN:VCALENDAR\r\nBEGIN:VTIMEZONE\r\nBEGIN:STANDARD\r\nDTSTART:19701025T030000\r\nEND:STANDARD\r\nEND:VTIMEZONE\r\nBEGIN:VEVENT\r\nDTSTART;TZID=Europe/Berlin:20261119T150000\r\nSUMMARY:Dentist\\, upstairs\r\nBEGIN:VALARM\r\nSUMMARY:Alarm\r\nEND:VALARM\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        assert_eq!(event(ics).as_deref(), Some("2026-11-19 15:00 Dentist, upstairs"));
        assert_eq!(event("BEGIN:VEVENT\r\nDTSTART;VALUE=DATE:20261231\r\nEND:VEVENT").as_deref(), Some("2026-12-31 Untitled"));
        assert_eq!(event("BEGIN:VEVENT\r\nDTSTART:20261231\r\nSTATUS:CANCELLED\r\nEND:VEVENT"), None);
        let new = new_event("u", (2026, 12, 31), "Party; late", "20261118T000000Z");
        assert!(new.contains("DTSTART;VALUE=DATE:20261231\r\nDTEND;VALUE=DATE:20270101\r\n"));
        assert_eq!(event(&new).as_deref(), Some("2026-12-31 Party; late"), "an event round-trips to the same text");
        let timed = new_event("u", (2026, 11, 19), "15:00 Dentist", "20261118T000000Z");
        assert!(timed.contains("DTSTART:20261119T150000\r\nSUMMARY:Dentist\r\n"), "a leading time is the event's time, not its title");
        assert_eq!(event(&timed).as_deref(), Some("2026-11-19 15:00 Dentist"));
        assert_eq!(event(&new_event("u", (2026, 11, 19), "25:00 Odd", "s")).as_deref(), Some("2026-11-19 25:00 Odd"));
        assert_eq!(event_day("2026-02-03 9:00 Standup"), Some(((2026, 2, 3), "9:00 Standup")));
        assert_eq!((event_day("2026-13-01 x"), event_day("Standup")), (None, None));
    }

    #[test]
    fn events_sync_over_caldav() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        let base = serve(move |r| {
            log.lock().unwrap().push((r.method.clone(), r.path.clone(), r.body.clone()));
            let ok = |status, body: &str| Response { status, headers: vec![("etag".into(), "\"e\"".into())], body: body.into() };
            match (r.method.as_str(), r.path.as_str()) {
                ("PROPFIND", "/remote.php/dav/calendars/me/") => ok(207, r#"<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:response><d:href>/remote.php/dav/calendars/me/personal/</d:href><d:propstat><d:prop><c:supported-calendar-component-set><c:comp name="VEVENT"/></c:supported-calendar-component-set></d:prop></d:propstat></d:response></d:multistatus>"#),
                ("DELETE", "/remote.php/dav/calendars/me/personal/old.ics") => ok(204, ""),
                ("PUT", _) => ok(201, ""),
                ("REPORT", "/remote.php/dav/calendars/me/personal/") => ok(207, "<d:multistatus xmlns:d=\"DAV:\" xmlns:cal=\"urn:ietf:params:xml:ns:caldav\"><d:response><d:href>/remote.php/dav/calendars/me/personal/a.ics</d:href><d:propstat><d:prop><d:getetag>&quot;1&quot;</d:getetag><cal:calendar-data>BEGIN:VCALENDAR\nBEGIN:VEVENT\nDTSTART;VALUE=DATE:20261120\nSUMMARY:Review\nEND:VEVENT\nEND:VCALENDAR\n</cal:calendar-data></d:prop></d:propstat></d:response></d:multistatus>"),
                _ => ok(500, ""),
            }
        });
        let nc = Nextcloud { url: base, user: "me".into(), password: "pw".into(), ..Nextcloud::default() };
        let items = [item("2026-11-19 Launch", 0xcd, None), item("2026-11-20 Review", 2, None), item("2001-01-01 Long ago", 3, None)];
        let out = sync(&nc, Kind::Events, &items, &["/remote.php/dav/calendars/me/personal/old.ics".into()]);
        assert_eq!(out.error, None);
        assert_eq!(out.cleared, ["/remote.php/dav/calendars/me/personal/old.ics"]);
        assert_eq!(out.pushed, [
            (0xcd, remote("/remote.php/dav/calendars/me/personal/ripple-00000000000000cd.ics", "\"e\"", "2026-11-19 Launch")),
            (2, remote("/remote.php/dav/calendars/me/personal/a.ics", "\"1\"", "2026-11-20 Review")),
        ]);
        let seen = seen.lock().unwrap();
        let puts: Vec<_> = seen.iter().filter(|r| r.0 == "PUT").collect();
        assert_eq!(puts.len(), 1, "the matching event was claimed, not uploaded again, and the old one stays local");
        assert!(puts[0].2.contains("DTSTART;VALUE=DATE:20261119") && puts[0].2.contains("SUMMARY:Launch"));
        assert!(seen.iter().any(|r| r.0 == "REPORT" && r.2.contains("<c:time-range start=\"")));
    }

    #[test]
    fn shelf_syncs_with_a_files_folder() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        let dir = "/remote.php/dav/files/me/Ripple%20Shelf/";
        let base = serve(move |r| {
            log.lock().unwrap().push((r.method.clone(), r.path.clone(), r.body.clone()));
            let res = |status, body: &str| Response { status, headers: vec![], body: body.into() };
            let file = |name: &str, size: u64| format!("<d:response><d:href>{dir}{name}</d:href><d:propstat><d:prop><d:getcontentlength>{size}</d:getcontentlength></d:prop></d:propstat></d:response>");
            match (r.method.as_str(), r.path.as_str()) {
                ("PROPFIND", p) if p == dir => res(207, &format!("<d:multistatus xmlns:d=\"DAV:\"><d:response><d:href>{dir}</d:href></d:response>{}{}{}{}{}{}{}{}{}{}<d:response><d:href>{dir}sub/</d:href></d:response></d:multistatus>", file("kept.txt", 8), file("their%20file.txt", 5), file("dropped.txt", 1), file("%2E%2E%2Fevil", 1), file("clash.txt", 99), file("a%3Ab", 1), file("nul.txt", 1), file("COM1", 1), file("twin.txt", 8), file("same.txt", 8))),
                ("PROPFIND", _) => res(404, ""),
                ("GET", p) if p == format!("{dir}their%20file.txt") => res(200, "hello"),
                ("GET", p) if p == format!("{dir}twin.txt") => res(200, "twin.txt"),
                ("GET", p) if p == format!("{dir}same.txt") => res(200, "SAME.TXT"),
                ("PUT", p) if p.ends_with("big.txt") => res(507, ""),
                ("PUT" | "DELETE" | "MKCOL", _) => res(201, ""),
                _ => res(500, ""),
            }
        });
        let nc = Nextcloud { url: base, user: "me".into(), password: "pw".into(), ..Nextcloud::default() };
        let tmp = std::env::temp_dir().join(format!("ripple-shelf-{}", std::process::id()));
        let cache = tmp.join("cache");
        std::fs::create_dir_all(&tmp).unwrap();
        let names = ["big.txt", "kept.txt", "mine.txt", "old.txt", "clash.txt", "twin.txt", "same.txt"];
        for name in names { std::fs::write(tmp.join(name), name).unwrap() }
        // The server has no room for big.txt: it's skipped, and the rest still syncs. Its clash.txt
        // and same.txt (same size, other bytes) are other files: neither is uploaded nor in `both`.
        // twin.txt has the same bytes there, so it is that file.
        let files = names.map(|n| tmp.join(n));
        // dropped.txt left the Shelf here and old.txt left Nextcloud since the last sync.
        let out = sync_shelf(&nc, "Ripple Shelf", &files, &["dropped.txt".into(), "old.txt".into(), "kept.txt".into()], &cache);
        assert_eq!(out.error.as_deref(), Some("Couldn't upload big.txt: Nextcloud answered 507"));
        assert_eq!(out.both.unwrap(), ["kept.txt", "twin.txt", "mine.txt", "their file.txt"]);
        assert_eq!(out.gone, [tmp.join("old.txt")]);
        assert_eq!(out.downloaded, [cache.join("their file.txt")]);
        assert_eq!(std::fs::read_to_string(cache.join("their file.txt")).unwrap(), "hello");
        let first = std::mem::take(&mut *seen.lock().unwrap());
        let sent: Vec<_> = first.iter().filter(|r| r.0 == "PUT" || r.0 == "DELETE").map(|r| (r.0.as_str(), r.1.as_str(), r.2.as_str())).collect();
        assert_eq!(sent, [("DELETE", &*format!("{dir}dropped.txt"), ""), ("PUT", &*format!("{dir}big.txt"), "big.txt"), ("PUT", &*format!("{dir}mine.txt"), "mine.txt")]);
        // A folder not made yet is made, level by level, and counts as empty.
        let out = sync_shelf(&nc, "/A/../B/", &[], &[], &cache);
        assert_eq!((out.error, out.both), (None, Some(Vec::new())));
        let made: Vec<_> = seen.lock().unwrap().iter().filter(|r| r.0 == "MKCOL").map(|r| r.1.clone()).collect();
        assert_eq!(made, ["/remote.php/dav/files/me/A/", "/remote.php/dav/files/me/A/B/"], "'..' is dropped, not followed");
        // A delete refused partway stops the sync; old.txt, not yet reached, stays known as synced.
        let base = serve(move |r| match r.method.as_str() {
            "PROPFIND" => Response { status: 207, headers: vec![], body: format!("<d:multistatus xmlns:d=\"DAV:\"><d:response><d:href>{dir}x.txt</d:href></d:response></d:multistatus>") },
            _ => Response { status: 401, headers: vec![], body: String::new() },
        });
        let nc = Nextcloud { url: base, user: "me".into(), password: "pw".into(), ..Nextcloud::default() };
        let out = sync_shelf(&nc, "Ripple Shelf", &[tmp.join("old.txt")], &["old.txt".into(), "x.txt".into()], &cache);
        assert_eq!(out.error.as_deref(), Some("Nextcloud refused the user or app password"));
        assert_eq!((out.both.unwrap(), out.gone), (vec!["x.txt".to_string(), "old.txt".into()], Vec::new()));
        let names = ["a%3Ab", "nul.txt", "COM1", "con", "COM", "console.log", "%2E%2E"].map(|n| format!("<d:response><d:href>/d/{n}</d:href></d:response>")).concat();
        assert_eq!(folder_files(&names), [("COM".to_string(), None), ("console.log".into(), None)], "devices, streams and '..' are left out");
        let folder = |f: &str| Nextcloud { shelf_folder: f.into(), ..Nextcloud::default() }.shelf_folder();
        assert_eq!([folder(""), folder(".."), folder(" /./ "), folder("/A/../ B /")], ["Ripple Shelf", "Ripple Shelf", "Ripple Shelf", "A/B"]);
        assert_eq!([in_folder("A/x", "A"), in_folder("A/B/x", "A"), in_folder("AB/x", "A")], [Some("x"), None, None]);
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn notes_sync_with_the_notes_api() {
        // Note 1 is on the server; 2 was deleted here; 3 changed there since the last sync.
        let notes = Arc::new(Mutex::new(vec![json!({"id": 1, "etag": "e1", "content": "Server", "modified": 5}), json!({"id": 3, "etag": "new", "content": "Theirs", "modified": 9})]));
        let state = notes.clone();
        let base = serve(move |r| {
            if r.header("authorization") != Some("Basic bWU6cHc=") { return reply(401, json!({})) }
            let mut notes = state.lock().unwrap();
            let body: Value = serde_json::from_str(&r.body).unwrap_or_default();
            match (r.method.as_str(), r.path.trim_start_matches("/nc/index.php/apps/notes/api/v1/notes")) {
                ("GET", "") => reply(200, Value::Array(notes.clone())),
                ("POST", "") => {
                    let note = json!({"id": 10 + notes.len(), "etag": "p", "content": body["content"], "modified": 1});
                    notes.push(note.clone());
                    reply(200, note)
                }
                ("DELETE", "/2") => reply(200, json!({})),
                ("PUT", "/3") if r.header("if-match") != Some("\"new\"") => reply(412, json!({})),
                _ => Response { status: 500, headers: vec![], body: String::new() },
            }
        });
        let nc = Nextcloud { url: format!("{base}/nc/"), user: "me".into(), password: "pw".into(), ..Nextcloud::default() };
        // Item 6 matches note 1, unclaimed: it takes that note rather than uploading a copy.
        let items = [item("Mine", 7, Some(("3", "old", "Before"))), item("Local", 8, None), item("  ", 9, None), item("Server", 6, None)];
        let out = sync(&nc, Kind::Notes, &items, &["2".into()]);
        assert_eq!(out.error, None);
        assert_eq!(out.cleared, ["2"]);
        assert_eq!(out.pushed, [(7, remote("12", "p", "Mine")), (8, remote("13", "p", "Local")), (6, remote("1", "e1", "Server"))], "the conflicting edit became a new note");
        assert_eq!(out.server.unwrap().iter().map(|n| n.text.as_str()).collect::<Vec<_>>(), ["Theirs", "Server", "Mine", "Local"]);
        // A failure partway keeps what was uploaded, so the next sync doesn't upload it twice.
        let partial = sync(&nc, Kind::Notes, &[item("Uploads", 1, None), item("Fails", 2, Some(("99", "e", "old")))], &[]);
        assert_eq!((partial.pushed.len(), partial.server, partial.error.as_deref()), (1, None, Some("Nextcloud answered 500")));
        let wrong = Nextcloud { password: "no".into(), ..nc };
        assert_eq!(sync(&wrong, Kind::Notes, &[], &[]).error.as_deref(), Some("Nextcloud refused the user or app password"));
    }

    #[test]
    fn tasks_sync_over_caldav() {
        let puts = Arc::new(Mutex::new(Vec::new()));
        let seen = puts.clone();
        let base = serve(move |r| {
            let ok = |status, body: &str| Response { status, headers: vec![("etag".into(), "\"t\"".into())], body: body.into() };
            match (r.method.as_str(), r.path.as_str()) {
                ("PROPFIND", "/remote.php/dav/calendars/me%40x/") if r.header("depth") == Some("1") => ok(207, r#"<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:response><d:href>/remote.php/dav/calendars/me%40x/todo/</d:href><d:propstat><d:prop><c:supported-calendar-component-set><c:comp name="VTODO"/></c:supported-calendar-component-set></d:prop></d:propstat></d:response></d:multistatus>"#),
                ("GET", "/remote.php/dav/calendars/me%40x/todo/done.ics") => ok(200, "BEGIN:VCALENDAR\r\nBEGIN:VTODO\r\nSUMMARY:Done\r\nEND:VTODO\r\nEND:VCALENDAR\r\n"),
                ("PUT", path) => { seen.lock().unwrap().push((path.to_string(), r.header("if-match").map(str::to_string), r.body.clone())); ok(201, "") }
                ("REPORT", "/remote.php/dav/calendars/me%40x/todo/") => ok(207, "<d:multistatus xmlns:d=\"DAV:\" xmlns:cal=\"urn:ietf:params:xml:ns:caldav\"><d:response><d:href>/remote.php/dav/calendars/me%40x/todo/a.ics</d:href><d:propstat><d:prop><d:getetag>&quot;1&quot;</d:getetag><cal:calendar-data>BEGIN:VCALENDAR&#13;\nBEGIN:VTODO&#13;\nSUMMARY:Water &amp; plants&#13;\nEND:VTODO&#13;\nEND:VCALENDAR&#13;\n</cal:calendar-data></d:prop></d:propstat></d:response></d:multistatus>"),
                _ => ok(500, ""),
            }
        });
        let nc = Nextcloud { url: base, user: "me@x".into(), password: "pw".into(), ..Nextcloud::default() };
        // "Water & plants" is already there, unclaimed: it takes that task rather than uploading a copy.
        let items = [item("New one", 0xab, None), item("Synced", 2, Some(("/remote.php/dav/calendars/me%40x/todo/b.ics", "\"1\"", "Synced"))), item("Water & plants", 3, None)];
        // An href on another host is dropped, never sent the password.
        let out = sync(&nc, Kind::Tasks, &items, &["/remote.php/dav/calendars/me%40x/todo/done.ics".into(), "https://evil.example/x.ics".into()]);
        assert_eq!(out.error, None);
        assert_eq!(out.cleared, ["/remote.php/dav/calendars/me%40x/todo/done.ics", "https://evil.example/x.ics"]);
        assert_eq!(out.pushed, [
            (0xab, remote("/remote.php/dav/calendars/me%40x/todo/ripple-00000000000000ab.ics", "\"t\"", "New one")),
            (3, remote("/remote.php/dav/calendars/me%40x/todo/a.ics", "\"1\"", "Water & plants")),
        ]);
        assert_eq!(out.server.unwrap(), [remote("/remote.php/dav/calendars/me%40x/todo/a.ics", "\"1\"", "Water & plants")]);
        let puts = puts.lock().unwrap();
        assert_eq!((puts[0].0.as_str(), puts[0].1.as_deref()), ("/remote.php/dav/calendars/me%40x/todo/done.ics", Some("\"t\"")));
        assert!(puts[0].2.contains("STATUS:COMPLETED"));
        assert!(puts[1].2.contains("SUMMARY:New one") && puts[1].2.contains("UID:ripple-00000000000000ab"));
        assert_eq!(puts.len(), 2, "the claimed task was not uploaded");
    }
}
