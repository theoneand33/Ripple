//! AI coding agents' plan limits, read with the credentials each agent's CLI already saved.
//! Blocking; callers run them off the UI thread. The endpoints are the ones each CLI calls
//! itself and are undocumented, so an answer of another shape shows as an error, never as 0%.

use std::path::{Path, PathBuf};

use serde_json::Value;

#[derive(Clone, Debug, PartialEq)]
pub struct Window {
    pub label: String,
    /// Percent of the window used, 0 to 100.
    pub used: f32,
    /// Unix seconds.
    pub resets: Option<i64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Usage {
    /// The agent, and the account when there is a name for it: "Codex · me@x.com".
    pub name: String,
    pub windows: Result<Vec<Window>, String>,
}

type Job = Box<dyn FnOnce() -> Result<Vec<Window>, String> + Send>;

/// One signed-in account: its card name, its token (to drop duplicates) and its fetch.
struct Account {
    name: String,
    token: String,
    fetch: Job,
}

/// Every account signed in on this machine: each CLI's own sign-in, the Claude Code and Codex
/// config folders in `extra_dirs`, and CLIProxyAPI's auth files in `cliproxy_dir`.
pub fn all(extra_dirs: &[String], cliproxy_dir: &str) -> Vec<Usage> {
    let Some(home) = rune::dirs::home() else { return Vec::new() };
    let handles: Vec<_> = accounts(&home, extra_dirs, cliproxy_dir).into_iter()
        .map(|a| (a.name, std::thread::spawn(a.fetch))).collect();
    handles.into_iter().map(|(name, h)| Usage { name, windows: h.join().unwrap_or_else(|_| Err("Couldn't read its limits".into())) }).collect()
}

fn accounts(home: &Path, extra_dirs: &[String], cliproxy_dir: &str) -> Vec<Account> {
    let mut out = Vec::new();
    let extra: Vec<PathBuf> = extra_dirs.iter().map(|d| d.trim()).filter(|d| !d.is_empty()).map(|d| expand(home, d)).collect();
    let claude_dirs = std::iter::once(dir("CLAUDE_CONFIG_DIR", home.join(".claude"))).chain(extra.iter().cloned());
    for (i, d) in claude_dirs.enumerate() {
        let Some(creds) = read_json(d.join(".credentials.json")).or_else(|| if i == 0 { claude_keychain() } else { None }) else { continue };
        let Some(token) = creds["claudeAiOauth"]["accessToken"].as_str() else { continue };
        // Claude Code keeps the account beside its folder by default, and inside it otherwise.
        let profile = if d == home.join(".claude") { home.join(".claude.json") } else { d.join(".claude.json") };
        let email = read_json(profile).and_then(|v| v["oauthAccount"]["emailAddress"].as_str().map(str::to_string));
        out.push(claude(named("Claude Code", email.as_deref()), token));
    }
    for d in std::iter::once(dir("CODEX_HOME", home.join(".codex"))).chain(extra.iter().cloned()) {
        let Some(auth) = read_json(d.join("auth.json")) else { continue };
        let t = &auth["tokens"];
        let Some(token) = t["access_token"].as_str() else { continue };
        let email = t["id_token"].as_str().and_then(jwt_email);
        out.push(codex(named("Codex", email.as_deref()), token, t["account_id"].as_str().unwrap_or_default()));
    }
    let proxy = expand(home, if cliproxy_dir.trim().is_empty() { "~/.cli-proxy-api" } else { cliproxy_dir.trim() });
    let mut files: Vec<PathBuf> = std::fs::read_dir(proxy).into_iter().flatten().flatten().map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json")).collect();
    files.sort();
    for v in files.into_iter().filter_map(read_json) {
        let (Some(token), email) = (v["access_token"].as_str(), v["email"].as_str()) else { continue };
        match v["type"].as_str().or(v["provider"].as_str()) {
            Some("claude") => out.push(claude(named("Claude Code", email), token)),
            Some("codex") => out.push(codex(named("Codex", email), token, v["account_id"].as_str().unwrap_or_default())),
            _ => {}
        }
    }
    let copilot_files = [".config/github-copilot", "AppData/Local/github-copilot"].iter().flat_map(|d| ["apps.json", "hosts.json"].map(|f| home.join(d).join(f)));
    for v in copilot_files.filter_map(read_json) {
        for e in v.as_object().into_iter().flat_map(|o| o.values()) {
            if let Some(token) = e["oauth_token"].as_str() { out.push(copilot(named("GitHub Copilot", e["user"].as_str()), token)) }
        }
    }
    if let Some(key) = read_json(home.join(".local/share/opencode/auth.json")).and_then(|v| v["opencode-go"]["key"].as_str().map(str::to_string)) {
        out.push(opencode_go(key));
    }
    if let Some(auth) = read_json(dir("GROK_HOME", home.join(".grok")).join("auth.json")) {
        let keys: Vec<&str> = auth.as_object().into_iter().flat_map(|o| o.values()).filter_map(|e| e["key"].as_str()).collect();
        for (i, key) in keys.iter().enumerate() {
            let name = if keys.len() > 1 { format!("Grok Build {}", i + 1) } else { "Grok Build".into() };
            out.push(grok(name, key));
        }
    }
    // The same account found twice (an extra folder that is the default one, a token copied into CLIProxyAPI).
    let mut seen = std::collections::HashSet::new();
    out.retain(|a| seen.insert(a.token.clone()));
    out
}

fn read_json(path: PathBuf) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

fn dir(var: &str, fallback: PathBuf) -> PathBuf {
    std::env::var_os(var).filter(|v| !v.is_empty()).map_or(fallback, PathBuf::from)
}

/// A path typed by the user, with `~` for the home folder.
fn expand(home: &Path, p: &str) -> PathBuf {
    match p.strip_prefix("~/").or_else(|| p.strip_prefix("~\\")) {
        Some(rest) => home.join(rest),
        None if p == "~" => home.into(),
        None => p.into(),
    }
}

fn named(agent: &str, who: Option<&str>) -> String {
    match who.map(str::trim).filter(|w| !w.is_empty()) {
        Some(w) => format!("{agent} · {w}"),
        None => agent.into(),
    }
}

/// The `email` claim of a JWT, unverified: only a label.
fn jwt_email(jwt: &str) -> Option<String> {
    use base64::Engine;
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(jwt.split('.').nth(1)?.trim_end_matches('=')).ok()?;
    serde_json::from_slice::<Value>(&payload).ok()?["email"].as_str().map(str::to_string)
}

fn get(url: &str, headers: &[(&str, &str)]) -> Result<Value, String> {
    let mut req = crate::net::agent().get(url).header("accept", "application/json").header("user-agent", "Ripple");
    for (k, v) in headers { req = req.header(*k, *v) }
    let mut res = req.call().map_err(|_| "Can't reach the server".to_string())?;
    match res.status().as_u16() {
        200..=299 => {}
        401 | 403 => return Err("Token expired: run its CLI once to refresh".into()),
        code => return Err(format!("The server answered {code}")),
    }
    let body = res.body_mut().read_to_string().map_err(|e| e.to_string())?;
    serde_json::from_str(&body).map_err(|e| e.to_string())
}

/// The windows, or an error when the answer had none.
fn found(windows: Vec<Window>) -> Result<Vec<Window>, String> {
    if windows.is_empty() { Err("Unknown answer; Ripple may need an update".into()) } else { Ok(windows) }
}

fn window(label: &str, used: Option<f64>, resets: Option<i64>) -> Option<Window> {
    Some(Window { label: label.into(), used: used?.clamp(0.0, 100.0) as f32, resets })
}

fn claude(name: String, token: &str) -> Account {
    let auth = format!("Bearer {token}");
    Account { name, token: token.into(), fetch: Box::new(move || {
        get("https://api.anthropic.com/api/oauth/usage", &[("authorization", &auth), ("anthropic-beta", "oauth-2025-04-20")]).and_then(|v| found(claude_windows(&v)))
    }) }
}

/// macOS keeps Claude Code's sign-in in the login Keychain instead of a file.
fn claude_keychain() -> Option<Value> {
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("security").args(["find-generic-password", "-s", "Claude Code-credentials", "-w"]).output().ok()?;
        return serde_json::from_slice(&out.stdout).ok();
    }
    #[allow(unreachable_code)]
    None
}

fn claude_windows(v: &Value) -> Vec<Window> {
    [("five_hour", "5 hours"), ("seven_day", "Week"), ("seven_day_opus", "Opus week"), ("seven_day_sonnet", "Sonnet week")].iter()
        .filter_map(|(key, label)| window(label, v[key]["utilization"].as_f64(), v[key]["resets_at"].as_str().and_then(iso)))
        .collect()
}

fn codex(name: String, token: &str, account: &str) -> Account {
    let (auth, account) = (format!("Bearer {token}"), account.to_string());
    Account { name, token: token.into(), fetch: Box::new(move || {
        get("https://chatgpt.com/backend-api/wham/usage", &[("authorization", &auth), ("chatgpt-account-id", &account)]).and_then(|v| found(codex_windows(&v)))
    }) }
}

fn codex_windows(v: &Value) -> Vec<Window> {
    ["primary_window", "secondary_window"].iter().filter_map(|key| {
        let w = &v["rate_limit"][key];
        window(&span(w["limit_window_seconds"].as_i64()?), w["used_percent"].as_f64(), w["reset_at"].as_i64())
    }).collect()
}

/// "5 hours", "Week", "30 days".
fn span(secs: i64) -> String {
    match secs {
        604_800 => "Week".into(),
        s if s < 86_400 => format!("{} hours", (s + 1800) / 3600),
        s => format!("{} days", (s + 43_200) / 86_400),
    }
}

fn copilot(name: String, token: &str) -> Account {
    let auth = format!("token {token}");
    Account { name, token: token.into(), fetch: Box::new(move || {
        get("https://api.github.com/copilot_internal/user", &[
            ("authorization", &auth),
            ("editor-version", "vscode/1.96.2"),
            ("editor-plugin-version", "copilot-chat/0.26.7"),
            ("x-github-api-version", "2025-04-01"),
        ]).and_then(|v| found(copilot_windows(&v)))
    }) }
}

fn copilot_windows(v: &Value) -> Vec<Window> {
    let resets = v["quota_reset_date"].as_str().and_then(iso);
    [("premium_interactions", "Premium requests"), ("chat", "Chat")].iter().filter_map(|(key, label)| {
        let q = &v["quota_snapshots"][key];
        if q["unlimited"].as_bool() == Some(true) { return None }
        window(label, q["percent_remaining"].as_f64().map(|p| 100.0 - p), resets)
    }).collect()
}

fn opencode_go(key: String) -> Account {
    let auth = format!("Bearer {key}");
    Account { name: "OpenCode Go".into(), token: key, fetch: Box::new(move || {
        get("https://opencode.ai/zen/go/v1/usage", &[("authorization", &auth)]).and_then(|v| found(opencode_windows(&v)))
    }) }
}

fn opencode_windows(v: &Value) -> Vec<Window> {
    [("rolling", "5 hours"), ("weekly", "Week"), ("monthly", "Month")].iter().filter_map(|(key, label)| {
        let w = &v["usage"][key];
        window(label, w["percent"].as_f64(), w["resetsAt"].as_str().and_then(iso))
    }).collect()
}

fn grok(name: String, token: &str) -> Account {
    let auth = format!("Bearer {token}");
    Account { name, token: token.into(), fetch: Box::new(move || {
        get("https://cli-chat-proxy.grok.com/v1/billing?format=credits", &[("authorization", &auth), ("x-xai-token-auth", "xai-grok-cli")]).and_then(|v| found(grok_windows(&v)))
    }) }
}

fn grok_windows(v: &Value) -> Vec<Window> {
    let c = &v["config"];
    let label = if c["currentPeriod"]["type"].as_str() == Some("USAGE_PERIOD_TYPE_MONTHLY") { "Month" } else { "Week" };
    let resets = c["currentPeriod"]["end"].as_str().or(c["billingPeriodEnd"].as_str()).and_then(iso);
    window(label, c["creditUsagePercent"].as_f64(), resets).into_iter().collect()
}

/// Unix seconds from "2026-08-13", "2026-08-13T16:27:38.287Z" or "...+02:00".
fn iso(s: &str) -> Option<i64> {
    let n = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, m, d) = (n(0..4)?, n(5..7)?, n(8..10)?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) { return None }
    let secs = if s.len() > 10 { n(11..13)? * 3600 + n(14..16)? * 60 + n(17..19)? } else { 0 };
    let tz = s[10..].rfind(['+', '-']).filter(|_| s.len() > 19).map(|i| i + 10);
    let offset = tz.and_then(|i| Some((n(i + 1..i + 3)? * 3600 + n(i + 4..i + 6)? * 60) * if &s[i..=i] == "-" { -1 } else { 1 })).unwrap_or(0);
    // Days from 1970-01-01 to y-m-d (Howard Hinnant's days_from_civil).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    let days = era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468;
    Some(days * 86_400 + secs - offset)
}

/// "3h 20m", "2d 4h", "now".
pub fn until(resets: i64, now: i64) -> String {
    let s = resets - now;
    if s < 60 { "now".into() }
    else if s < 3600 { format!("{}m", s / 60) }
    else if s < 86_400 { format!("{}h {}m", s / 3600, s % 3600 / 60) }
    else { format!("{}d {}h", s / 86_400, s % 86_400 / 3600) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn w(label: &str, used: f32, resets: Option<i64>) -> Window { Window { label: label.into(), used, resets } }

    #[test]
    fn reads_iso_dates() {
        assert_eq!(iso("1970-01-02"), Some(86_400));
        assert_eq!(iso("2026-08-13T16:27:38.287Z"), Some(1_786_638_458));
        assert_eq!(iso("2026-08-13T18:27:38+02:00"), Some(1_786_638_458));
        assert_eq!(iso("2026-08-13T16:27:38.930537+00:00"), Some(1_786_638_458));
        assert_eq!((iso("soon"), iso("2026-13-40")), (None, None));
        assert_eq!((until(100, 90), until(4000, 0), until(90_000, 0)), ("now".into(), "1h 6m".into(), "1d 1h".into()));
    }

    #[test]
    fn reads_each_agents_answer() {
        assert_eq!(claude_windows(&json!({"five_hour": {"utilization": 12.0, "resets_at": "1970-01-02T00:00:00Z"}, "seven_day": {"utilization": 40.5, "resets_at": null}, "seven_day_opus": null})),
            [w("5 hours", 12.0, Some(86_400)), w("Week", 40.5, None)]);
        assert_eq!(codex_windows(&json!({"rate_limit": {"primary_window": {"used_percent": 5, "limit_window_seconds": 18000, "reset_at": 99}, "secondary_window": {"used_percent": 120, "limit_window_seconds": 604800, "reset_at": 100}}})),
            [w("5 hours", 5.0, Some(99)), w("Week", 100.0, Some(100))]);
        assert_eq!(copilot_windows(&json!({"quota_reset_date": "1970-01-02", "quota_snapshots": {"premium_interactions": {"percent_remaining": 75.0, "unlimited": false}, "chat": {"unlimited": true}}})),
            [w("Premium requests", 25.0, Some(86_400))]);
        assert_eq!(opencode_windows(&json!({"usage": {"rolling": {"status": "ok", "percent": 4, "resetsAt": "1970-01-02T00:00:00.000Z"}, "weekly": {"percent": 3}}})),
            [w("5 hours", 4.0, Some(86_400)), w("Week", 3.0, None)]);
        assert_eq!(grok_windows(&json!({"config": {"currentPeriod": {"type": "USAGE_PERIOD_TYPE_WEEKLY", "end": "1970-01-02T00:00:00+00:00"}, "creditUsagePercent": 75.0}})),
            [w("Week", 75.0, Some(86_400))]);
        assert_eq!(found(grok_windows(&json!({"error": "x"}))), Err("Unknown answer; Ripple may need an update".into()));
        assert_eq!(span(2_592_000), "30 days");
    }

    #[test]
    fn finds_every_account_once() {
        use base64::Engine;
        let home = std::env::temp_dir().join(format!("ripple-limits-{}", std::process::id()));
        let put = |path: &str, v: serde_json::Value| {
            let p = home.join(path);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, v.to_string()).unwrap();
        };
        let id_token = format!("x.{}.y", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"email":"b@x.com"}"#));
        put(".claude/.credentials.json", json!({"claudeAiOauth": {"accessToken": "c1"}}));
        put(".claude.json", json!({"oauthAccount": {"emailAddress": "a@x.com"}}));
        put("work/.credentials.json", json!({"claudeAiOauth": {"accessToken": "c2"}}));
        put("work/auth.json", json!({"tokens": {"access_token": "x1", "account_id": "acc", "id_token": id_token}}));
        put("proxy/claude-a.json", json!({"type": "claude", "email": "a@x.com", "access_token": "c1"}));
        put("proxy/codex-d.json", json!({"type": "codex", "email": "d@x.com", "access_token": "x2"}));
        put("proxy/gemini.json", json!({"type": "gemini", "access_token": "g"}));
        put(".config/github-copilot/apps.json", json!({"github.com:1": {"user": "octo", "oauth_token": "gh1"}, "github.com:2": {"user": "cat", "oauth_token": "gh2"}}));
        let names: Vec<String> = accounts(&home, &["~/work".into(), " ".into()], &home.join("proxy").to_string_lossy()).into_iter().map(|a| a.name).collect();
        let _ = std::fs::remove_dir_all(&home);
        assert_eq!(names, ["Claude Code · a@x.com", "Claude Code", "Codex · b@x.com", "Codex · d@x.com", "GitHub Copilot · octo", "GitHub Copilot · cat"]);
    }
}
