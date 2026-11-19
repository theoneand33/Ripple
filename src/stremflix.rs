//! Stremflix-rs's local API: its Continue Watching, and playback in its window.
//! Blocking; callers run them off the UI thread.

use serde::Deserialize;

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Entry {
    /// `movie`, `series`, `file`, `btih`, …; with `id`, what `/api/play` takes.
    #[serde(rename = "type")]
    pub kind: String,
    pub id: String,
    pub title: String,
    pub poster: Option<String>,
    #[serde(default)]
    pub position: f64,
    #[serde(default)]
    pub duration: f64,
}

impl Entry {
    /// How far in, 0 to 1.
    pub fn progress(&self) -> f32 {
        if self.duration > 0.0 { (self.position / self.duration).clamp(0.0, 1.0) as f32 } else { 0.0 }
    }
}

/// `http://<host:port>/api`, from `STREMFLIX_RS_API` as Stremflix-rs reads it; None when it is `off`.
pub fn base() -> Option<String> {
    let addr = std::env::var("STREMFLIX_RS_API").ok().filter(|a| !a.trim().is_empty()).unwrap_or_else(|| "127.0.0.1:11471".into());
    (addr.trim() != "off").then(|| format!("http://{}/api", addr.trim()))
}

fn error(res: &mut ureq::http::Response<ureq::Body>) -> String {
    let body: serde_json::Value = res.body_mut().read_to_string().ok().and_then(|b| serde_json::from_str(&b).ok()).unwrap_or_default();
    body["error"].as_str().map_or_else(|| format!("Stremflix-rs answered {}", res.status().as_u16()), str::to_string)
}

const OFFLINE: &str = "Open Stremflix-rs to see what you're watching";

pub fn continue_watching(base: &str) -> Result<Vec<Entry>, String> {
    let mut res = crate::net::agent().get(format!("{base}/continue")).call().map_err(|_| OFFLINE.to_string())?;
    if !res.status().is_success() { return Err(error(&mut res)) }
    let body = res.body_mut().read_to_string().map_err(|e| e.to_string())?;
    serde_json::from_str(&body).map_err(|e| e.to_string())
}

pub fn play(base: &str, entry: &Entry) -> Result<(), String> {
    let mut res = crate::net::agent().post(format!("{base}/play"))
        .query("type", &entry.kind).query("id", &entry.id).query("name", &entry.title)
        .send_empty().map_err(|_| OFFLINE.to_string())?;
    if !res.status().is_success() { return Err(error(&mut res)) }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testserver::{json, serve};

    #[test]
    fn lists_continue_watching_and_plays_an_entry() {
        let base = serve(|r| match (r.method.as_str(), r.path.as_str()) {
            ("GET", "/api/continue") => json(200, serde_json::json!([
                {"type": "series", "id": "tt0903747:1:2", "title": "Cat's in the Bag...", "poster": "https://x/p.jpg", "position": 720.0, "duration": 2880.0},
                {"type": "file", "id": "/home/me/a b.mkv", "title": "a b", "poster": null, "position": 0.0, "duration": 0.0},
            ])),
            ("POST", "/api/play?type=file&id=%2Fhome%2Fme%2Fa+b.mkv&name=a+b" | "/api/play?type=file&id=%2Fhome%2Fme%2Fa%20b.mkv&name=a%20b") => json(200, serde_json::json!({"ok": true})),
            _ => json(404, serde_json::json!({"error": format!("no {}", r.path)})),
        });
        let base = format!("{base}/api");
        let list = continue_watching(&base).unwrap();
        assert_eq!((list.len(), list[0].progress(), list[1].poster.clone(), list[1].progress()), (2, 0.25, None, 0.0));
        assert_eq!(play(&base, &list[1]), Ok(()));
        assert!(play(&base, &list[0]).unwrap_err().starts_with("no /api/play"));
        assert_eq!(continue_watching("http://127.0.0.1:1/api"), Err(OFFLINE.to_string()));
    }
}
