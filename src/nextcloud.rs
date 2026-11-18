//! Nextcloud sync for Notes (the Notes app's REST API) and Tasks (CalDAV to-dos).
//! `sync` blocks, so it runs off the UI thread; `apply` merges its outcome on the UI thread,
//! so edits made while it ran are kept.

use std::time::Duration;

use base64::Engine;
use serde_json::{Value, json};

use crate::config::{Item, Nextcloud, Remote};
use crate::island::Island;
use crate::net::percent_encode;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Notes = 0,
    Tasks = 1,
}

/// One kind's sync: whether one runs, whether another was asked for meanwhile, and the last error.
#[derive(Default)]
pub struct SyncState {
    running: bool,
    again: bool,
    pub error: Option<String>,
}

#[derive(Debug, Default, PartialEq)]
pub struct Outcome {
    /// What each uploaded item now is on Nextcloud, by item key.
    pub pushed: Vec<(u64, Remote)>,
    /// Pending deletes (notes) or completions (tasks) that went through.
    pub cleared: Vec<String>,
    /// Everything Nextcloud has, after the uploads; None when the sync stopped before reading it.
    pub server: Option<Vec<Remote>>,
    /// Why the sync stopped early. What it did before that still counts.
    pub error: Option<String>,
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
        let (mut items, mut pending) = match kind {
            Kind::Notes => (self.cfg.notes.clone(), nc.deleted_notes.clone()),
            Kind::Tasks => (self.cfg.tasks.clone(), nc.done_tasks.clone()),
        };
        // Ids from another account mean nothing to this one: everything uploads as new. The saved
        // ids are only dropped once this account answers, so a half-typed URL loses nothing.
        let fresh = nc.synced_with != account;
        if fresh {
            for i in &mut items { i.remote = None }
            pending.clear();
        }
        std::thread::spawn(move || {
            let out = sync(&nc, kind, &items, &pending);
            me.update(move |s, _| s.nextcloud_synced(kind, &account, fresh, out));
        });
    }

    fn nextcloud_synced(&mut self, kind: Kind, account: &str, fresh: bool, out: Outcome) {
        self.nextcloud[kind as usize].running = false;
        let current = self.cfg.nextcloud.account() == account;
        self.nextcloud[kind as usize].error = out.error.clone();
        // A new account that failed at once has nothing of ours: keep the old ids.
        let answered = !fresh || out.server.is_some() || !out.pushed.is_empty();
        match out {
            out if current && answered => {
                let open = self.cfg.notes.get(self.notes.index).map(|n| (n.key, n.text.clone()));
                self.save(|c| {
                    if c.nextcloud.synced_with != account {
                        for i in c.notes.iter_mut().chain(&mut c.tasks) { i.remote = None }
                        c.nextcloud.deleted_notes.clear();
                        c.nextcloud.done_tasks.clear();
                        c.nextcloud.synced_with = account.to_string();
                    }
                    match kind {
                        Kind::Notes => apply(&mut c.notes, &mut c.nextcloud.deleted_notes, out),
                        Kind::Tasks => apply(&mut c.tasks, &mut c.nextcloud.done_tasks, out),
                    }
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
            // The account changed while it ran, or a new one never answered.
            _ => {}
        }
        if std::mem::take(&mut self.nextcloud[kind as usize].again) || !current { self.nextcloud_sync(kind) }
    }
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
    for item in items {
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
    let (status, _, res) = c.call("GET", &url, &json_body[1..], "")?;
    if !ok(status) { return Err(failed(status)) }
    let list: Vec<Value> = serde_json::from_str(&res).map_err(|_| "Nextcloud Notes isn't installed on that server".to_string())?;
    let mut notes: Vec<_> = list.iter().filter_map(|v| Some((v["modified"].as_i64().unwrap_or(0), note(v)?))).collect();
    notes.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    out.server = Some(notes.into_iter().map(|(_, n)| n).collect());
    Ok(())
}

// --- Tasks: CalDAV (RFC 4791) VTODOs in one of the user's calendars ---

const LISTS: &str = r#"<?xml version="1.0"?><d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:prop><d:displayname/><c:supported-calendar-component-set/></d:prop></d:propfind>"#;
const OPEN_TASKS: &str = r#"<?xml version="1.0"?><c:calendar-query xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:prop><d:getetag/><c:calendar-data/></d:prop><c:filter><c:comp-filter name="VCALENDAR"><c:comp-filter name="VTODO"><c:prop-filter name="COMPLETED"><c:is-not-defined/></c:prop-filter></c:comp-filter></c:comp-filter></c:filter></c:calendar-query>"#;
const XML: (&str, &str) = ("content-type", "application/xml; charset=utf-8");
const ICS: (&str, &str) = ("content-type", "text/calendar; charset=utf-8");

fn sync_tasks(c: &Client, list: &str, items: &[Item], pending: &[String], out: &mut Outcome) -> Result<(), String> {
    let home = format!("{}/remote.php/dav/calendars/{}/", c.base, percent_encode(&c.user));
    let (status, _, res) = c.call("PROPFIND", &home, &[XML, ("depth", "1")], LISTS)?;
    if status != 207 { return Err(failed(status)) }
    let calendar = pick_list(&res, list.trim()).ok_or_else(|| match list.trim() {
        "" => "No Nextcloud list holds tasks".to_string(),
        name => format!("No Nextcloud list is named {name}"),
    })?;
    let stamp = utc_stamp();
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
    for item in items.iter().filter(|i| i.remote.is_none() && !i.text.trim().is_empty()) {
        // Named by the item's key, so a retry after a lost answer finds it already there (412).
        let uid = format!("ripple-{:016x}", item.key);
        let href = format!("{}/{uid}.ics", calendar.trim_end_matches('/'));
        let (status, etag, _) = c.call("PUT", &c.at(&href), &[ICS, ("if-none-match", "*")], &new_task(&uid, &item.text, &stamp))?;
        if !ok(status) && status != 412 { return Err(failed(status)) }
        out.pushed.push((item.key, Remote { id: href, etag, text: item.text.clone() }));
    }
    let (status, _, res) = c.call("REPORT", &c.at(&calendar), &[XML, ("depth", "1")], OPEN_TASKS)?;
    if status != 207 { return Err(failed(status)) }
    let mut server = Vec::new();
    for response in elements(&res, "response") {
        let first = |name| elements(response, name).first().map(|v| unescape(v)).unwrap_or_default();
        if let (Some(id), Some(text)) = (server_path(first("href")), open_task(&first("calendar-data"))) {
            server.push(Remote { id, etag: first("getetag"), text });
        }
    }
    out.server = Some(server);
    Ok(())
}

/// The list named `want` (by name or path), else the first that holds only tasks, else the first that holds tasks.
fn pick_list(xml: &str, want: &str) -> Option<String> {
    let lists: Vec<(String, String, &str)> = elements(xml, "response").into_iter().filter_map(|r| {
        let comps = *elements(r, "supported-calendar-component-set").first()?;
        let href = server_path(unescape(elements(r, "href").first()?))?;
        comps.contains("\"VTODO\"").then(|| (href, elements(r, "displayname").first().map(|n| unescape(n)).unwrap_or_default(), comps))
    }).collect();
    if !want.is_empty() {
        let slug = |href: &str| href.trim_end_matches('/').rsplit('/').next().unwrap_or_default().to_string();
        return lists.into_iter().find(|(href, name, _)| name.eq_ignore_ascii_case(want) || slug(href).eq_ignore_ascii_case(want)).map(|l| l.0);
    }
    let only_tasks = lists.iter().position(|l| !l.2.contains("\"VEVENT\"")).unwrap_or(0);
    lists.into_iter().nth(only_tasks).map(|l| l.0)
}

/// The contents of each `<name>` element, whatever its namespace prefix. Enough for WebDAV's
/// multistatus answers; not a general XML parser.
fn elements<'a>(xml: &'a str, name: &str) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(i) = rest.find('<') {
        rest = &rest[i + 1..];
        let end = rest.find(|c: char| c.is_whitespace() || c == '>' || c == '/').unwrap_or(rest.len());
        let tag = &rest[..end];
        if tag.is_empty() || tag.rsplit(':').next() != Some(name) { continue }
        let Some(close) = rest.find('>') else { break };
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
        let entity = rest[1..].split_once(';').map(|(e, _)| e).unwrap_or_default();
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

fn new_task(uid: &str, summary: &str, stamp: &str) -> String {
    let summary = summary.replace('\\', "\\\\").replace(';', "\\;").replace(',', "\\,").replace('\n', "\\n");
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

fn utc_stamp() -> String {
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
    let t = rune::time::LocalTime::at_offset(ms, 0);
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
    }

    #[test]
    fn webdav_answers_parse() {
        let xml = r#"<d:multistatus xmlns:d="DAV:"><d:response><d:href>/dav/cal/personal/</d:href><d:propstat><d:prop><d:displayname>Personal</d:displayname><cal:supported-calendar-component-set><cal:comp name="VEVENT"/><cal:comp name="VTODO"/></cal:supported-calendar-component-set></d:prop></d:propstat></d:response><d:response><d:href>/dav/cal/tasks/</d:href><d:propstat><d:prop><d:displayname>Tasks &amp; more</d:displayname><cal:supported-calendar-component-set><cal:comp name="VTODO"/></cal:supported-calendar-component-set></d:prop></d:propstat></d:response><d:response><d:href>/dav/cal/</d:href><d:propstat><d:prop><d:displayname/></d:prop></d:propstat></d:response></d:multistatus>"#;
        assert_eq!(pick_list(xml, ""), Some("/dav/cal/tasks/".into()));
        assert_eq!(pick_list(xml, "personal"), Some("/dav/cal/personal/".into()));
        assert_eq!(pick_list(xml, "Tasks & more"), Some("/dav/cal/tasks/".into()));
        assert_eq!(pick_list(xml, "nope"), None);
        assert_eq!(unescape("a&lt;b&#13;&#x41;&bogus;"), "a<b\rA&bogus;");
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
        }, |_| String::new());
        let nc = Nextcloud { url: format!("{base}/nc/"), user: "me".into(), password: "pw".into(), ..Nextcloud::default() };
        let items = [item("Mine", 7, Some(("3", "old", "Before"))), item("Local", 8, None), item("  ", 9, None)];
        let out = sync(&nc, Kind::Notes, &items, &["2".into()]);
        assert_eq!(out.error, None);
        assert_eq!(out.cleared, ["2"]);
        assert_eq!(out.pushed, [(7, remote("12", "p", "Mine")), (8, remote("13", "p", "Local"))], "the conflicting edit became a new note");
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
        }, |_| String::new());
        let nc = Nextcloud { url: base, user: "me@x".into(), password: "pw".into(), ..Nextcloud::default() };
        let items = [item("New one", 0xab, None), item("Synced", 2, Some(("/remote.php/dav/calendars/me%40x/todo/a.ics", "\"1\"", "Synced")))];
        // An href on another host is dropped, never sent the password.
        let out = sync(&nc, Kind::Tasks, &items, &["/remote.php/dav/calendars/me%40x/todo/done.ics".into(), "https://evil.example/x.ics".into()]);
        assert_eq!(out.error, None);
        assert_eq!(out.cleared, ["/remote.php/dav/calendars/me%40x/todo/done.ics", "https://evil.example/x.ics"]);
        assert_eq!(out.pushed, [(0xab, remote("/remote.php/dav/calendars/me%40x/todo/ripple-00000000000000ab.ics", "\"t\"", "New one"))]);
        assert_eq!(out.server.unwrap(), [remote("/remote.php/dav/calendars/me%40x/todo/a.ics", "\"1\"", "Water & plants")]);
        let puts = puts.lock().unwrap();
        assert_eq!((puts[0].0.as_str(), puts[0].1.as_deref()), ("/remote.php/dav/calendars/me%40x/todo/done.ics", Some("\"t\"")));
        assert!(puts[0].2.contains("STATUS:COMPLETED"));
        assert!(puts[1].2.contains("SUMMARY:New one") && puts[1].2.contains("UID:ripple-00000000000000ab"));
    }
}
