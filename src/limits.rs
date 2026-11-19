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

/// Fetches the windows; the flag allows renewing an expired token.
type Job = Box<dyn FnOnce(bool) -> Result<Vec<Window>, String> + Send>;

/// One signed-in account: its card name, its token (to drop duplicates), its account id when it can renew, and its fetch.
struct Account {
    name: String,
    token: String,
    renews: Option<String>,
    fetch: Job,
}

/// Every account signed in on this machine: each CLI's own sign-in, the Claude Code and Codex
/// config folders in `extra_dirs`, and CLIProxyAPI's auth files in `cliproxy_dir`.
pub fn all(extra_dirs: &[String], cliproxy_dir: &str) -> Vec<Usage> {
    let Some(home) = rune::dirs::home() else { return Vec::new() };
    let accounts = accounts(&home, extra_dirs, cliproxy_dir);
    // Two files with one account: one may hold an older, already-used refresh token, and spending it can revoke the sign-in.
    let renew: Vec<bool> = accounts.iter().map(|a| a.renews.as_ref().is_some_and(|id| accounts.iter().filter(|b| b.renews.as_ref() == Some(id)).count() == 1)).collect();
    let handles: Vec<_> = accounts.into_iter().zip(renew)
        .map(|(a, renew)| (a.name, std::thread::spawn(move || (a.fetch)(renew)))).collect();
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
        let account = read_json(profile).map(|v| v["oauthAccount"].clone()).unwrap_or_default();
        let email = account["emailAddress"].as_str();
        let saved = d.join(".credentials.json").exists().then(|| Saved::claude(d.join(".credentials.json")));
        // Renew only a known account: an unknown one may be a stale copy of another folder.
        let id = account["accountUuid"].as_str().or(email).filter(|_| saved.is_some()).map(|id| format!("claude:{id}"));
        out.push(Account { renews: id, ..claude(named("Claude Code", email), token, saved) });
    }
    for d in std::iter::once(dir("CODEX_HOME", home.join(".codex"))).chain(extra.iter().cloned()) {
        let Some(auth) = read_json(d.join("auth.json")) else { continue };
        let t = &auth["tokens"];
        let Some(token) = t["access_token"].as_str() else { continue };
        let email = t["id_token"].as_str().and_then(jwt_email);
        let saved = Saved { path: d.join("auth.json"), at: "/tokens", access: "access_token", refresh: "refresh_token", oauth: &CODEX };
        let id = t["account_id"].as_str().filter(|a| !a.is_empty()).map(|a| format!("codex:{a}"));
        out.push(Account { renews: id, ..codex(named("Codex", email.as_deref()), token, t["account_id"].as_str().unwrap_or_default(), Some(saved)) });
    }
    let proxy = expand(home, if cliproxy_dir.trim().is_empty() { "~/.cli-proxy-api" } else { cliproxy_dir.trim() });
    let mut files: Vec<PathBuf> = std::fs::read_dir(proxy).into_iter().flatten().flatten().map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json")).collect();
    files.sort();
    for v in files.into_iter().filter_map(read_json) {
        let (Some(token), email) = (v["access_token"].as_str(), v["email"].as_str()) else { continue };
        // CLIProxyAPI renews its own tokens.
        match v["type"].as_str().or(v["provider"].as_str()) {
            Some("claude") => out.push(claude(named("Claude Code", email), token, None)),
            Some("codex") => out.push(codex(named("Codex", email), token, v["account_id"].as_str().unwrap_or_default(), None)),
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
pub fn expand(home: &Path, p: &str) -> PathBuf {
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

const EXPIRED: &str = "Token expired: run its CLI once to refresh";
const LOST: &str = "Couldn't save the renewed sign-in: sign in to its CLI again";

/// A CLI's OAuth app, to renew its expired tokens the way the CLI does.
struct Oauth { url: &'static str, client: &'static str, scope: Option<&'static str> }

const CLAUDE: Oauth = Oauth { url: "https://platform.claude.com/v1/oauth/token", client: "9d1c250a-e61b-44d9-88ed-5944d1962f5e", scope: None };
const CODEX: Oauth = Oauth { url: "https://auth.openai.com/oauth/token", client: "app_EMoamEEZ73f0CkXaXp7hrann", scope: Some("openid profile email") };

/// Where a sign-in is saved: the file, the JSON pointer to its tokens, their key names, and its app.
struct Saved { path: PathBuf, at: &'static str, access: &'static str, refresh: &'static str, oauth: &'static Oauth }

impl Saved {
    fn claude(path: PathBuf) -> Self { Saved { path, at: "/claudeAiOauth", access: "accessToken", refresh: "refreshToken", oauth: &CLAUDE } }
}

/// Fetches with the token and, when it has expired, once more with a renewed one.
fn renewing(token: &str, saved: Option<&Saved>, fetch: impl Fn(&str) -> Result<Value, String>) -> Result<Value, String> {
    match (fetch(token), saved) {
        (Err(e), Some(s)) if e == EXPIRED => fetch(&renew(s, token)?),
        (r, _) => r,
    }
}

/// Trades the saved refresh token for new tokens and saves them back in the CLI's file:
/// refresh tokens are single-use, so the CLI would be signed out with the old one.
fn renew(s: &Saved, expired: &str) -> Result<String, String> {
    // Overlapping loads must not spend the same refresh token twice: OpenAI may revoke the whole sign-in.
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _held = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut file = read_json(s.path.clone()).ok_or(EXPIRED)?;
    let creds = file.pointer_mut(s.at).filter(|c| c.is_object()).ok_or(EXPIRED)?;
    // Another load (or the CLI) renewed it already.
    if let Some(t) = creds[s.access].as_str().filter(|t| *t != expired) { return Ok(t.to_string()) }
    // A copy beside the file keeps its permissions and proves it can be saved before the old token is spent.
    let tmp = s.path.with_extension("json.ripple-tmp");
    std::fs::copy(&s.path, &tmp).map_err(|_| EXPIRED.to_string())?;
    let saved = renew_into(s, creds).and_then(|access| {
        let text = serde_json::to_string_pretty(&file).map_err(|e| e.to_string())?;
        // The old refresh token is spent now, so a failed save falls back to writing the file in place.
        let ok = std::fs::write(&tmp, &text).and_then(|_| std::fs::rename(&tmp, &s.path)).or_else(|_| std::fs::write(&s.path, &text));
        ok.map(|_| access).map_err(|_| LOST.to_string())
    });
    let _ = std::fs::remove_file(&tmp);
    saved
}

/// Posts the refresh token and puts the answer's tokens into `creds`; returns the access token.
fn renew_into(s: &Saved, creds: &mut Value) -> Result<String, String> {
    let refresh = creds[s.refresh].as_str().ok_or(EXPIRED)?.to_string();
    let mut body = serde_json::json!({"grant_type": "refresh_token", "refresh_token": refresh, "client_id": s.oauth.client});
    if let Some(scope) = s.oauth.scope { body["scope"] = scope.into() }
    let mut res = crate::net::agent().post(s.oauth.url).header("content-type", "application/json").header("user-agent", "Ripple")
        .send(body.to_string()).map_err(|e| {
            use ureq::{Error as E, Timeout as T};
            match e {
                // The request never left, so the refresh token is unspent.
                E::HostNotFound | E::ConnectionFailed | E::Timeout(T::Resolve | T::Connect) => "Can't reach the server".to_string(),
                _ => "Renewal didn't finish: if it stays expired, sign in to its CLI again".to_string(),
            }
        })?;
    if !res.status().is_success() { return Err(EXPIRED.into()) }
    // From here the old refresh token is spent.
    let v: Value = res.body_mut().read_to_string().ok().and_then(|b| serde_json::from_str(&b).ok()).ok_or(LOST)?;
    let access = v["access_token"].as_str().ok_or(LOST)?.to_string();
    creds[s.access] = access.clone().into();
    if let Some(r) = v["refresh_token"].as_str() { creds[s.refresh] = r.into() }
    if let (Some(id), true) = (v["id_token"].as_str(), creds.get("id_token").is_some()) { creds["id_token"] = id.into() }
    // Claude Code's expiry in ms; without it the CLI would renew again at once.
    if let (Some(secs), true) = (v["expires_in"].as_i64(), creds.get("expiresAt").is_some()) {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as i64;
        creds["expiresAt"] = (now + secs * 1000).into();
    }
    Ok(access)
}

fn get(url: &str, headers: &[(&str, &str)]) -> Result<Value, String> {
    let mut req = crate::net::agent().get(url).header("accept", "application/json").header("user-agent", "Ripple");
    for (k, v) in headers { req = req.header(*k, *v) }
    let mut res = req.call().map_err(|_| "Can't reach the server".to_string())?;
    match res.status().as_u16() {
        200..=299 => {}
        401 => return Err(EXPIRED.into()),
        // Refused for another reason: renewing would only spend the refresh token.
        403 => return Err("Token refused: run its CLI once to refresh".into()),
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

fn claude(name: String, token: &str, saved: Option<Saved>) -> Account {
    let token = token.to_string();
    Account { name, token: token.clone(), renews: None, fetch: Box::new(move |renew| {
        renewing(&token, saved.as_ref().filter(|_| renew), |t| get("https://api.anthropic.com/api/oauth/usage", &[("authorization", &format!("Bearer {t}")), ("anthropic-beta", "oauth-2025-04-20")]))
            .and_then(|v| found(claude_windows(&v)))
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

fn codex(name: String, token: &str, account: &str, saved: Option<Saved>) -> Account {
    let (token, account) = (token.to_string(), account.to_string());
    Account { name, token: token.clone(), renews: None, fetch: Box::new(move |renew| {
        renewing(&token, saved.as_ref().filter(|_| renew), |t| get("https://chatgpt.com/backend-api/wham/usage", &[("authorization", &format!("Bearer {t}")), ("chatgpt-account-id", &account)]))
            .and_then(|v| found(codex_windows(&v)))
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
    Account { name, token: token.into(), renews: None, fetch: Box::new(move |_| {
        get("https://api.github.com/copilot_internal/user", &[
            ("authorization", &auth),
            ("editor-version", "vscode/1.96.2"),
            ("editor-plugin-version", "copilot-chat/0.26.7"),
            ("x-github-api-version", "2025-04-01"),
        ]).and_then(|v| match copilot_windows(&v) {
            // A known quota marked unlimited and none limited: recognised, with nothing to show.
            w if w.is_empty() && ["premium_interactions", "chat"].iter().any(|k| v["quota_snapshots"][k]["unlimited"].as_bool() == Some(true)) => Ok(w),
            w => found(w),
        })
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
    Account { name: "OpenCode Go".into(), token: key, renews: None, fetch: Box::new(move |_| {
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
    Account { name, token: token.into(), renews: None, fetch: Box::new(move |_| {
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
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let last = match m { 2 => 28 + leap as i64, 4 | 6 | 9 | 11 => 30, _ => 31 };
    if !(1..=12).contains(&m) || !(1..=last).contains(&d) { return None }
    let secs = if s.len() > 10 { n(11..13)? * 3600 + n(14..16)? * 60 + n(17..19)? } else { 0 };
    let tz = s[10..].rfind(['+', '-']).filter(|_| s.len() > 19).map(|i| i + 10);
    let offset = tz.and_then(|i| {
        // "+02:00" or "+0200".
        let m = if s.get(i + 3..i + 4) == Some(":") { i + 4 } else { i + 3 };
        Some((n(i + 1..i + 3)? * 3600 + n(m..m + 2)? * 60) * if &s[i..=i] == "-" { -1 } else { 1 })
    }).unwrap_or(0);
    let days = crate::nextcloud::days_from_civil(i32::try_from(y).ok()?, m as u32, d as u32);
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
        assert_eq!((iso("2026-08-13T18:27:38+0200"), iso("2026-08-13T14:27:38-0200")), (Some(1_786_638_458), Some(1_786_638_458)));
        assert_eq!(iso("2026-08-13T16:27:38.930537+00:00"), Some(1_786_638_458));
        assert_eq!((iso("soon"), iso("2026-13-40"), iso("2026-02-30"), iso("2025-02-29")), (None, None, None, None));
        assert_eq!(iso("2028-02-29"), Some(1_835_395_200));
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
    fn renews_and_saves_an_expired_token() {
        use crate::testserver::{json as reply, serve};
        let url = serve(|r| match r.body.contains("\"refresh_token\":\"r1\"") {
            true => reply(200, json!({"access_token": "a2", "refresh_token": "r2", "expires_in": 60})),
            false => reply(400, json!({"error": "invalid_grant"})),
        }, |_| String::new());
        let path = std::env::temp_dir().join(format!("ripple-renew-{}.json", std::process::id()));
        std::fs::write(&path, json!({"claudeAiOauth": {"accessToken": "a1", "refreshToken": "r1", "expiresAt": 0, "scopes": ["x"]}}).to_string()).unwrap();
        let oauth: &'static Oauth = Box::leak(Box::new(Oauth { url: Box::leak(format!("{url}/token").into_boxed_str()), client: "c", scope: None }));
        let saved = Saved { oauth, ..Saved::claude(path.clone()) };
        let fetch = |t: &str| if t == "a2" { Ok(json!({"ok": true})) } else { Err(EXPIRED.to_string()) };
        assert_eq!(renewing("a1", Some(&saved), fetch), Ok(json!({"ok": true})));
        let file = read_json(path.clone()).unwrap();
        assert_eq!((&file["claudeAiOauth"]["accessToken"], &file["claudeAiOauth"]["refreshToken"], &file["claudeAiOauth"]["scopes"]), (&json!("a2"), &json!("r2"), &json!(["x"])));
        assert!(file["claudeAiOauth"]["expiresAt"].as_i64().unwrap() > 0);
        // A load that still had a1 takes the saved a2 instead of spending r2 (the server would refuse it).
        assert_eq!(renewing("a1", Some(&saved), fetch), Ok(json!({"ok": true})));
        // A refused refresh token keeps the error and leaves the file as it was.
        assert_eq!(renewing("a2", Some(&saved), |_| Err(EXPIRED.to_string())), Err(EXPIRED.to_string()));
        assert!(!path.with_extension("json.ripple-tmp").exists());
        assert_eq!(read_json(path.clone()).unwrap(), file);
        let _ = std::fs::remove_file(&path);
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
        let found = accounts(&home, &["~/work".into(), " ".into()], &home.join("proxy").to_string_lossy());
        let _ = std::fs::remove_dir_all(&home);
        // Only a CLI's own file with a known account renews: not the folder without a profile, not CLIProxyAPI or Copilot.
        let ids: Vec<Option<&str>> = found.iter().map(|a| a.renews.as_deref()).collect();
        assert_eq!(ids, [Some("claude:a@x.com"), None, Some("codex:acc"), None, None, None]);
        let names: Vec<String> = found.into_iter().map(|a| a.name).collect();
        assert_eq!(names, ["Claude Code · a@x.com", "Claude Code", "Codex · b@x.com", "Codex · d@x.com", "GitHub Copilot · octo", "GitHub Copilot · cat"]);
    }
}
