//! Minimal T3 Code client. Each device is { url, token } where token comes from
//! `t3 auth session issue` on that machine, so any reachable T3 server works.
//! Linked environments (T3's peer links) are only reachable through /mcp tools
//! that take `environmentId`, so for those we sign in to /mcp as an MCP client.
//! Blocking; callers run it off the UI thread.

use std::collections::HashMap;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::config::T3Device;
use crate::net::{percent_decode, percent_encode};

const TIMEOUT: Duration = Duration::from_secs(15);
/// Never opened: the authorization code is read straight from the redirect URL,
/// the same way T3's own peer links sign in.
const REDIRECT_URI: &str = "http://127.0.0.1/ripple";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Favorite {
    pub instance_id: String,
    pub model: String,
    pub label: String,
}

impl Favorite {
    pub fn key(&self) -> String { format!("{}/{}", self.instance_id, self.model) }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub id: String,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Loaded {
    pub favorites: Vec<Favorite>,
    pub runtime_mode: String,
    pub links: Vec<Link>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Project {
    pub id: String,
    pub title: String,
    pub path: String,
}

#[derive(Clone, Debug, Default)]
pub struct Launch {
    /// Empty = the device itself; otherwise one of its linked environments.
    pub environment_id: String,
    pub project_id: String,
    pub instance_id: String,
    pub model: String,
    pub message: String,
    pub worktree: bool,
    pub runtime_mode: String,
}

#[derive(Clone, Serialize, Deserialize)]
struct CachedToken {
    token: String,
    #[serde(rename = "expiresAt")]
    expires_at: u64,
}

/// MCP tokens last 30 days; they are kept on disk so each app start doesn't add another
/// "Ripple" client to the device's Sharing & access list.
pub struct Client {
    file: Option<PathBuf>,
    tokens: Mutex<HashMap<String, CachedToken>>,
}

fn base(device: &T3Device) -> String {
    device.url.trim().trim_end_matches('/').to_string()
}

fn bearer(device: &T3Device) -> String {
    format!("Bearer {}", device.token.trim())
}

fn origin_of(url: &str) -> &str {
    let after_scheme = url.find("://").map_or(0, |i| i + 3);
    &url[..url[after_scheme..].find('/').map_or(url.len(), |i| after_scheme + i)]
}

fn path_of(url: &str) -> &str {
    let path = &url[origin_of(url).len()..];
    path.split('?').next().unwrap_or(path)
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

fn random_bytes<const N: usize>() -> Result<[u8; N], String> {
    let mut bytes = [0u8; N];
    getrandom::fill(&mut bytes).map_err(|e| e.to_string())?;
    Ok(bytes)
}

fn uuid_v4() -> Result<String, String> {
    let mut b = random_bytes::<16>()?;
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    Ok(format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..]))
}

fn query_param(url: &str, name: &str) -> Option<String> {
    let query = url.split_once('?')?.1.split('#').next()?;
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        (k == name).then(|| percent_decode(&v.replace('+', " ")))
    })
}

/// One HTTP exchange. A failed status is an error carrying the server's own message.
fn request(method: &str, url: &str, headers: &[(&str, &str)], body: Option<&str>) -> Result<Value, String> {
    let agent = crate::net::agent();
    let unreachable = |_| format!("Could not reach {}", origin_of(url));
    let mut res = if method == "GET" {
        let mut req = agent.get(url);
        for (k, v) in headers { req = req.header(*k, *v) }
        req.call().map_err(unreachable)?
    } else {
        let mut req = agent.post(url);
        for (k, v) in headers { req = req.header(*k, *v) }
        match body {
            Some(body) => req.send(body),
            None => req.send_empty(),
        }
        .map_err(unreachable)?
    };
    let text = res.body_mut().read_to_string().unwrap_or_default();
    let data: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if !res.status().is_success() {
        let message = data["error_description"].as_str().or(data["message"].as_str());
        return Err(message.map_or_else(|| format!("T3 {} failed ({})", path_of(url), res.status().as_u16()), Into::into));
    }
    Ok(data)
}

/// Loopback ports are open to every user on the machine: before a token goes to one, make sure
/// the program listening there is ours.
fn verify_peer(device: &T3Device) -> Result<(), String> {
    let base = base(device);
    let Some(port) = loopback_port(origin_of(&base)) else { return Ok(()) };
    #[cfg(target_os = "linux")]
    {
        // The real uid from status: /proc/self itself belongs to root in non-dumpable processes.
        let status = std::fs::read_to_string("/proc/self/status").map_err(|e| e.to_string())?;
        let uid = status.lines().find_map(|l| l.strip_prefix("Uid:")?.split_whitespace().next()?.parse().ok()).ok_or("Unknown user")?;
        let tables: Vec<String> = ["/proc/net/tcp", "/proc/net/tcp6"].iter().filter_map(|p| std::fs::read_to_string(p).ok()).collect();
        match listeners_ours(&tables, port, uid) {
            Some(true) => Ok(()),
            Some(false) => Err("Another user's program holds T3's port".into()),
            None => Err(format!("Could not reach {}", origin_of(&base))),
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        // shortcut: only the detected device is re-checked, against a public id; upgrade to a peer-owner check (lsof, GetExtendedTcpTable) per OS.
        let _ = port;
        if device.name != LOCAL_NAME { return Ok(()) }
        let id = t3_home().and_then(|h| std::fs::read_to_string(h.join("userdata").join("environment-id")).ok()).unwrap_or_default();
        let answer = request("GET", &format!("{base}/.well-known/t3/environment"), &[], None)?;
        if id.trim().is_empty() || answer["environmentId"].as_str() != Some(id.trim()) { return Err("Another program holds T3's port".into()) }
        Ok(())
    }
}

/// Whether every TCP listener on `port` in /proc/net/tcp{,6} belongs to `uid` (or root, which
/// owns Docker's port proxy and outranks us anyway). None when nothing listens there.
#[cfg(target_os = "linux")]
fn listeners_ours(tables: &[String], port: u16, uid: u32) -> Option<bool> {
    let owners: Vec<u32> = tables.iter().flat_map(|t| t.lines().skip(1)).filter_map(|line| {
        let f: Vec<&str> = line.split_whitespace().collect();
        let local_port = u16::from_str_radix(f.get(1)?.rsplit(':').next()?, 16).ok()?;
        // State 0A is LISTEN.
        (local_port == port && *f.get(3)? == "0A").then(|| f.get(7)?.parse().ok()).flatten()
    }).collect();
    (!owners.is_empty()).then(|| owners.iter().all(|&o| o == uid || o == 0))
}

fn authed(device: &T3Device, method: &str, path: &str, body: Option<&Value>) -> Result<Value, String> {
    verify_peer(device)?;
    let auth = bearer(device);
    let mut headers = vec![("authorization", auth.as_str())];
    if body.is_some() { headers.push(("content-type", "application/json")) }
    request(method, &(base(device) + path), &headers, body.map(Value::to_string).as_deref())
}

/// One-shot Effect RPC call over T3's /ws endpoint.
fn rpc(device: &T3Device, tag: &str, payload: Value) -> Result<Value, String> {
    let ticket = authed(device, "POST", "/api/auth/websocket-ticket", None)?["ticket"].as_str().unwrap_or_default().to_string();
    let origin = base(device);
    let ws_url = format!("{}/ws?orchestrationProtocol=2&wsTicket={}", origin.replacen("http", "ws", 1), percent_encode(&ticket));
    let failed = || "Could not connect to T3".to_string();
    let deadline = Instant::now() + TIMEOUT;

    let authority = origin_of(&ws_url).split("://").nth(1).unwrap_or_default();
    let default_port = if ws_url.starts_with("wss") { 443 } else { 80 };
    let addr = match authority.rsplit_once(':') {
        Some((host, port)) if port.bytes().all(|b| b.is_ascii_digit()) && !port.is_empty() => (host.trim_matches(['[', ']']), port.parse().unwrap_or(default_port)).to_socket_addrs(),
        _ => (authority.trim_matches(['[', ']']), default_port).to_socket_addrs(),
    }
    .ok()
    .and_then(|mut a| a.next())
    .ok_or_else(failed)?;
    let tcp = TcpStream::connect_timeout(&addr, TIMEOUT).map_err(|_| failed())?;
    // A read that outlasts the deadline fails rather than hanging the caller.
    tcp.set_read_timeout(Some(TIMEOUT)).and_then(|_| tcp.set_write_timeout(Some(TIMEOUT))).map_err(|_| failed())?;
    let (mut ws, _) = tungstenite::client_tls(ws_url.as_str(), tcp).map_err(|_| failed())?;

    let call = json!({ "_tag": "Request", "id": "1", "tag": tag, "payload": payload, "headers": [] });
    ws.send(tungstenite::Message::text(call.to_string())).map_err(|_| failed())?;
    let result = loop {
        if Instant::now() >= deadline { break Err("T3 request timed out".to_string()) }
        let msg = match ws.read() {
            Ok(tungstenite::Message::Text(text)) => text,
            Ok(tungstenite::Message::Close(_)) => break Err(failed()),
            Ok(_) => continue,
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => break Err("T3 request timed out".to_string()),
            Err(_) => break Err(failed()),
        };
        let Ok(msg) = serde_json::from_str::<Value>(msg.as_str()) else { continue };
        if msg["_tag"] != "Exit" { continue }
        if msg["exit"]["_tag"] == "Success" { break Ok(msg["exit"]["value"].clone()) }
        let err = &msg["exit"]["cause"][0]["error"];
        let message = err["cause"]["cause"].as_str().or(err["cause"]["message"].as_str()).or(err["message"].as_str());
        break Err(message.unwrap_or("T3 request failed").to_string());
    };
    let _ = ws.close(None);
    result
}

impl Client {
    pub fn new(file: Option<PathBuf>) -> Client {
        let tokens = file.as_ref()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Client { file, tokens: Mutex::new(tokens) }
    }

    fn save(&self, tokens: &HashMap<String, CachedToken>) {
        let Some(file) = &self.file else { return };
        let Ok(json) = serde_json::to_string(tokens) else { return };
        if let Some(dir) = file.parent() { let _ = std::fs::create_dir_all(dir); }
        // Written beside the cache and renamed over it, so a crash never leaves half a file.
        let tmp = file.with_extension("json.tmp");
        #[cfg(unix)]
        let written = {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            // Bearer tokens: readable by the user only.
            std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp).and_then(|mut f| f.write_all(json.as_bytes()))
        };
        #[cfg(not(unix))]
        let written = std::fs::write(&tmp, json);
        let written = written.and_then(|_| std::fs::rename(&tmp, file));
        if let Err(e) = written { eprintln!("Ripple: could not save T3 tokens: {e}") }
    }

    fn token_key(device: &T3Device) -> String {
        Sha256::digest(format!("{}\n{}", base(device), device.token.trim())).iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Mints a one-time pairing code with the device's bearer token, then spends it
    /// on /mcp's OAuth flow: register, decide, exchange.
    fn sign_in_mcp(&self, device: &T3Device) -> Result<String, String> {
        let origin = base(device);
        let resource = format!("{origin}/mcp");
        let json_headers = [("content-type", "application/json")];
        let pairing = authed(device, "POST", "/api/auth/pairing-token", Some(&json!({ "label": "Ripple", "access": "full-access" })))?;
        let client = request("POST", &format!("{origin}/oauth/mcp/register"), &json_headers,
            Some(&json!({ "client_name": "Ripple", "redirect_uris": [REDIRECT_URI] }).to_string()))?;
        let client_id = client["client_id"].as_str().unwrap_or_default();
        let verifier = URL_SAFE_NO_PAD.encode(random_bytes::<32>()?);
        let state = URL_SAFE_NO_PAD.encode(random_bytes::<16>()?);
        // The pairing code and, below, the code and verifier are credentials too: check the peer before each.
        verify_peer(device)?;
        let decided = request("POST", &format!("{origin}/oauth/mcp/decision"), &json_headers, Some(&json!({
            "authorization": {
                "response_type": "code",
                "client_id": client_id,
                "redirect_uri": REDIRECT_URI,
                "code_challenge": URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())),
                "code_challenge_method": "S256",
                "state": state,
                "resource": resource,
            },
            "decision": { "_tag": "pairing-code", "access": "full-access", "code": pairing["credential"] },
        }).to_string()))?;
        let redirect = decided["redirectTo"].as_str().unwrap_or_default();
        let code = query_param(redirect, "code").filter(|c| !c.is_empty());
        let (Some(code), true) = (code, query_param(redirect, "state").as_deref() == Some(state.as_str())) else {
            return Err(query_param(redirect, "error_description").unwrap_or_else(|| "T3 did not approve Ripple".into()));
        };
        let form: String = [("grant_type", "authorization_code"), ("code", &code), ("redirect_uri", REDIRECT_URI), ("client_id", client_id), ("code_verifier", &verifier), ("resource", &resource)]
            .iter().map(|(k, v)| format!("{k}={}", percent_encode(v))).collect::<Vec<_>>().join("&");
        verify_peer(device)?;
        let token = request("POST", &format!("{origin}/oauth/mcp/token"), &[("content-type", "application/x-www-form-urlencoded")], Some(&form))?;
        let access = token["access_token"].as_str().ok_or("T3 gave no MCP token")?.to_string();
        // T3's tokens last 30 days; assume that when the answer doesn't say.
        let expires_in = token["expires_in"].as_f64().unwrap_or(30.0 * 86_400.0);
        // Renew a day early (or at half-life for short tokens) so a call never races the expiry.
        let lifetime = (expires_in / 2.0).max(expires_in - 86_400.0).max(0.0);
        let mut tokens = self.tokens.lock().unwrap();
        tokens.insert(Self::token_key(device), CachedToken { token: access.clone(), expires_at: now_ms() + (lifetime * 1000.0) as u64 });
        self.save(&tokens);
        Ok(access)
    }

    fn mcp_token(&self, device: &T3Device) -> Result<String, String> {
        let cached = self.tokens.lock().unwrap().get(&Self::token_key(device)).cloned();
        match cached {
            Some(c) if c.expires_at >= now_ms() => Ok(c.token),
            _ => self.sign_in_mcp(device),
        }
    }

    /// One tools/call over MCP streamable HTTP.
    fn mcp(&self, device: &T3Device, name: &str, args: Value) -> Result<Value, String> {
        for attempt in 0..2 {
            let token = self.mcp_token(device)?;
            match mcp_call(device, &token, name, &args)? {
                Some(result) => return Ok(result),
                // Revoked in T3's Sharing & access: sign in again once.
                None if attempt == 0 => { self.tokens.lock().unwrap().remove(&Self::token_key(device)); }
                None => break,
            }
        }
        Err("T3 rejected Ripple's MCP sign-in".into())
    }

    pub fn load(&self, device: &T3Device) -> Result<Loaded, String> {
        let config = rpc(device, "server.getConfig", json!({}))?;
        let providers = config["providers"].as_array().cloned().unwrap_or_default();
        let settings = &config["settings"];
        let links = if settings["linkedEnvironmentsEnabled"].as_bool().unwrap_or(false) {
            self.mcp(device, "t3_environment_links", json!({})).map(|r| r["links"].as_array().cloned().unwrap_or_default()).unwrap_or_default()
        } else {
            Vec::new()
        };
        let text = |v: &Value| v.as_str().unwrap_or_default().to_string();
        Ok(Loaded {
            favorites: settings["favorites"].as_array().into_iter().flatten().map(|f| {
                let (provider, model) = (text(&f["provider"]), text(&f["model"]));
                let label = providers.iter()
                    .find(|p| p["instanceId"] == f["provider"])
                    .and_then(|p| p["models"].as_array()?.iter().find(|m| m["slug"] == f["model"])?["name"].as_str().map(String::from))
                    .unwrap_or_else(|| model.clone());
                Favorite { instance_id: provider, model, label }
            }).collect(),
            runtime_mode: settings["defaultRuntimeMode"].as_str().unwrap_or("full-access").into(),
            links: links.iter().map(|l| Link { id: text(&l["environmentId"]), label: text(&l["label"]) }).collect(),
        })
    }

    /// `environment_id` empty = the device itself; otherwise one of its linked environments.
    pub fn projects(&self, device: &T3Device, environment_id: &str) -> Result<Vec<Project>, String> {
        let mut projects = Vec::new();
        if environment_id.is_empty() {
            projects = authed(device, "GET", "/api/projects", None)?["projects"].as_array().cloned().unwrap_or_default();
        } else {
            let mut cursor = json!(0);
            // 1,000 pages bounds a server that never ends its paging.
            for _ in 0..1000 {
                let page = self.mcp(device, "t3_project_list", json!({ "environmentId": environment_id, "cursor": cursor, "limit": 100 }))?;
                projects.extend(page["projects"].as_array().cloned().unwrap_or_default());
                cursor = page["nextCursor"].clone();
                if cursor.is_null() { break }
            }
        }
        let text = |v: &Value| v.as_str().unwrap_or_default().to_string();
        let gone = |v: &Value| !v.is_null() && v != &json!(false) && v != &json!("");
        Ok(projects.iter()
            .filter(|p| !gone(&p["archivedAt"]) && !gone(&p["deletedAt"]))
            .map(|p| Project { id: text(&p["id"]), title: text(&p["title"]), path: text(&p["workspaceRoot"]) })
            .collect())
    }

    /// Starts an agent thread; returns its id.
    pub fn start_thread(&self, device: &T3Device, launch: &Launch) -> Result<String, String> {
        let text = launch.message.trim();
        if text.is_empty() { return Err("Message is empty".into()) }
        let title: String = text.lines().next().unwrap_or_default().chars().take(80).collect();
        let mut call = json!({
            "projectId": launch.project_id,
            "title": title,
            "modelSelection": { "instanceId": launch.instance_id, "model": launch.model },
            "runtimeMode": if launch.runtime_mode.is_empty() { "full-access" } else { &launch.runtime_mode },
            "interactionMode": "default",
            "workspaceStrategy": if launch.worktree { json!({ "type": "worktree", "baseRef": "HEAD" }) } else { json!({ "type": "root" }) },
        });
        let result = if launch.environment_id.is_empty() {
            call["commandId"] = json!(uuid_v4()?);
            call["generateTitle"] = json!(true);
            call["initialMessage"] = json!({ "text": text, "attachments": [] });
            rpc(device, "orchestration.launchThread", call)?
        } else {
            call["environmentId"] = json!(launch.environment_id);
            call["message"] = json!(text);
            self.mcp(device, "t3_thread_launch", call)?
        };
        Ok(result["threadId"].as_str().unwrap_or_default().to_string())
    }
}

/// The name `detect_local` gives the device; removing it turns detection off.
pub const LOCAL_NAME: &str = "This computer";

/// T3's data folder: `T3CODE_HOME`, else `~/.t3`.
fn t3_home() -> Option<PathBuf> {
    let raw = std::env::var("T3CODE_HOME").unwrap_or_default();
    let raw = raw.trim();
    let home = rune::dirs::home();
    match raw {
        "" => home.map(|h| h.join(".t3")),
        _ if raw.starts_with('~') => home.map(|h| crate::limits::expand(&h, raw)),
        _ => Some(PathBuf::from(raw)),
    }
}

/// The origin the local T3 server last announced in `userdata/server-runtime.json`.
fn local_origin(home: &std::path::Path) -> Option<String> {
    let runtime: Value = serde_json::from_str(&std::fs::read_to_string(home.join("userdata").join("server-runtime.json")).ok()?).ok()?;
    let origin = runtime["origin"].as_str()?.trim().trim_end_matches('/');
    // Loopback only: the token minted for it must never leave this machine.
    loopback_port(origin).map(|_| origin.to_string())
}

/// The port of an `http://` origin that is exactly a loopback host and an optional numeric port:
/// no userinfo, path or query to smuggle another host in.
fn loopback_port(origin: &str) -> Option<u16> {
    let authority = origin.strip_prefix("http://")?;
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) if !h.ends_with(':') => (h, Some(p)),
        _ => (authority, None),
    };
    let ip = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host);
    let loopback = host.eq_ignore_ascii_case("localhost") || ip.parse::<std::net::IpAddr>().is_ok_and(|a| a.is_loopback());
    if !loopback { return None }
    match port {
        None => Some(80),
        Some(p) => p.bytes().all(|b| b.is_ascii_digit()).then(|| p.parse().ok()).flatten(),
    }
}

/// The T3 Code installed on this machine, with a token minted by its own `t3` CLI.
/// Machines on T3 Connect come along as that server's linked environments.
/// None when T3 has never run here or the CLI is missing. Blocking.
pub fn detect_local() -> Option<T3Device> {
    let home = t3_home()?;
    let url = local_origin(&home)?;
    // The file outlives the server, and another process may hold its port: mint a token only
    // when this T3's own environment answers there.
    let id = std::fs::read_to_string(home.join("userdata").join("environment-id")).ok()?;
    let answer = request("GET", &format!("{url}/.well-known/t3/environment"), &[], None).ok()?;
    if id.trim().is_empty() || answer["environmentId"].as_str() != Some(id.trim()) { return None }
    let base_dir = home.to_string_lossy();
    let args = ["auth", "session", "issue", "--token-only", "--label", "Ripple", "--ttl", "365d", "--base-dir", &base_dir];
    let token = crate::system::output("t3", &args);
    // npm installs the CLI as a `.cmd` shim on Windows.
    #[cfg(windows)]
    let token = token.or_else(|| crate::system::output("t3.cmd", &args));
    let token = token.filter(|t| !t.is_empty() && !t.contains(char::is_whitespace))?;
    Some(T3Device { name: LOCAL_NAME.into(), url, token })
}

/// initialize, initialized, tools/call. `Ok(None)` means the token was refused (401).
fn mcp_call(device: &T3Device, token: &str, name: &str, args: &Value) -> Result<Option<Value>, String> {
    verify_peer(device)?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .http_status_as_error(false)
        .build()
        .into();
    let url = format!("{}/mcp", base(device));
    let mut session: Option<String> = None;
    // `Ok(None)`: unauthorized. A message without an id is a notification and has no answer.
    let mut post = |message: Value| -> Result<Option<Value>, String> {
        let mut req = agent.post(&url)
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .header("mcp-protocol-version", "2025-06-18");
        if let Some(id) = &session { req = req.header("mcp-session-id", id) }
        let mut res = req.send(message.to_string()).map_err(|_| format!("Could not reach {}", origin_of(&url)))?;
        if res.status().as_u16() == 401 { return Ok(None) }
        if let Some(id) = res.headers().get("mcp-session-id").and_then(|v| v.to_str().ok()) { session = Some(id.to_string()) }
        let event_stream = res.headers().get("content-type").and_then(|v| v.to_str().ok()).is_some_and(|t| t.contains("text/event-stream"));
        let text = res.body_mut().read_to_string().unwrap_or_default();
        if !res.status().is_success() { return Err(format!("T3 /mcp failed ({})", res.status().as_u16())) }
        if message.get("id").is_none() { return Ok(Some(Value::Null)) }
        let body = if event_stream { text.lines().filter_map(|l| l.strip_prefix("data:")).next_back().unwrap_or_default() } else { text.as_str() };
        serde_json::from_str(body).map(Some).map_err(|_| "T3 /mcp sent an unreadable answer".to_string())
    };
    let init = post(json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "Ripple", "version": "1" } } }))?;
    if init.is_none() { return Ok(None) }
    post(json!({ "jsonrpc": "2.0", "method": "notifications/initialized", "params": {} }))?;
    let Some(res) = post(json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": { "name": name, "arguments": args } }))? else { return Ok(None) };
    if let Some(message) = res["error"]["message"].as_str() { return Err(message.into()) }
    if res["result"]["isError"].as_bool().unwrap_or(false) {
        let text = res["result"]["content"].as_array().into_iter().flatten().find_map(|c| c["text"].as_str().filter(|t| !t.is_empty())).unwrap_or("T3 tool failed");
        let message = serde_json::from_str::<Value>(text).ok().and_then(|v| v["message"].as_str().map(String::from));
        return Err(message.unwrap_or_else(|| text.to_string()));
    }
    Ok(Some(res["result"]["structuredContent"].clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_split_into_origin_and_path() {
        assert_eq!(origin_of("http://127.0.0.1:3773/api/projects?x=1"), "http://127.0.0.1:3773");
        assert_eq!(path_of("http://127.0.0.1:3773/api/projects?x=1"), "/api/projects");
        assert_eq!(origin_of("https://t3.example.ts.net"), "https://t3.example.ts.net");
        assert_eq!(base(&T3Device { url: " http://host:1// ".into(), ..Default::default() }), "http://host:1");
    }

    #[test]
    fn redirect_parameters_are_read_and_decoded() {
        let url = "http://127.0.0.1/ripple?code=a%2Fb+c&state=xyz#frag";
        assert_eq!(query_param(url, "code").as_deref(), Some("a/b c"));
        assert_eq!(query_param(url, "state").as_deref(), Some("xyz"));
        assert_eq!(query_param(url, "error_description"), None);
        assert_eq!(query_param("http://127.0.0.1/ripple", "code"), None);
    }

    #[test]
    fn uuids_are_version_4() {
        let id = uuid_v4().unwrap();
        assert_eq!(id.len(), 36);
        assert_eq!(&id[14..15], "4");
        assert!(matches!(&id[19..20], "8" | "9" | "a" | "b"));
    }

    #[test]
    fn the_local_server_origin_is_read_from_its_runtime_file() {
        let home = std::env::temp_dir().join(format!("ripple-t3-home-{}", std::process::id()));
        assert_eq!(local_origin(&home), None, "T3 never ran here");
        std::fs::create_dir_all(home.join("userdata")).unwrap();
        std::fs::write(home.join("userdata/server-runtime.json"), r#"{"port":3773,"origin":"http://127.0.0.1:3773/"}"#).unwrap();
        assert_eq!(local_origin(&home).as_deref(), Some("http://127.0.0.1:3773"));
        for bad in [r#"{"origin":"file:///x"}"#, r#"{"origin":"httpx://h"}"#, r#"{"origin":"http://evil.example:3773"}"#, r#"{"origin":"https://127.0.0.1"}"#, r#"{"origin":"http://localhost.evil.example"}"#, r#"{"origin":"http://127.0.0.1:3773@evil.example"}"#, r#"{"origin":"http://127.0.0.1:3773/x"}"#, r#"{"port":3773}"#, "not json"] {
            std::fs::write(home.join("userdata/server-runtime.json"), bad).unwrap();
            assert_eq!(local_origin(&home), None, "{bad}");
        }
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn loopback_ports_are_read_from_exact_origins() {
        assert_eq!(loopback_port("http://127.0.0.1:3773"), Some(3773));
        assert_eq!(loopback_port("http://localhost"), Some(80));
        assert_eq!(loopback_port("http://[::1]:8080"), Some(8080));
        assert_eq!(loopback_port("http://LOCALHOST:1"), Some(1));
        assert_eq!(loopback_port("http://127.0.0.2:3773"), Some(3773));
        for other in ["https://127.0.0.1:3773", "http://127.0.0.1.evil.example", "http://127.0.0.1:99999", "http://t3.example:3773", "http://127.0.0.1:3773@evil.example", "http://127.0.0.1:3773/x", "http://127.0.0.1:"] {
            assert_eq!(loopback_port(other), None, "{other}");
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn only_our_own_listeners_get_the_token() {
        let header = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n";
        let row = |port: &str, st: &str, uid: u32| format!("   0: 0100007F:{port} 00000000:0000 {st} 00000000:00000000 00:00000000 00000000  {uid}        0 1 1 0000000000000000 100 0 0 10 0\n");
        let ours = format!("{header}{}{}", row("0EBD", "0A", 1000), row("0EBD", "01", 1001));
        assert_eq!(listeners_ours(&[ours], 3773, 1000), Some(true), "a connection by someone else is not a listener");
        let squatted = format!("{header}{}", row("0EBD", "0A", 1001));
        assert_eq!(listeners_ours(&[squatted], 3773, 1000), Some(false));
        let docker = format!("{header}{}", row("0EBD", "0A", 0));
        assert_eq!(listeners_ours(&[docker], 3773, 1000), Some(true));
        assert_eq!(listeners_ours(&[format!("{header}{}", row("0EBE", "0A", 1000))], 3773, 1000), None, "nothing on 3773");
    }

    #[test]
    fn token_keys_depend_on_url_and_token() {
        let a = T3Device { url: "http://h:1/".into(), token: "t".into(), ..Default::default() };
        let b = T3Device { url: "http://h:1".into(), token: " t ".into(), ..Default::default() };
        let c = T3Device { url: "http://h:1".into(), token: "u".into(), ..Default::default() };
        assert_eq!(Client::token_key(&a), Client::token_key(&b));
        assert_ne!(Client::token_key(&a), Client::token_key(&c));
    }

    /// A T3 server: HTTP API, the /mcp OAuth sign-in and tools, and the /ws RPC.
    fn mock_t3(rejected_tokens: usize) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        use crate::testserver::{Response, json, serve};
        use std::sync::{Arc, Mutex};
        let log = Arc::new(Mutex::new(Vec::<String>::new()));
        let challenge = Arc::new(Mutex::new(String::new()));
        let rejects = Arc::new(Mutex::new(rejected_tokens));
        let (http_log, ws_log) = (log.clone(), log.clone());
        let base = serve(move |r| {
            http_log.lock().unwrap().push(format!("{} {}", r.method, r.path.split('?').next().unwrap_or_default()));
            let body: Value = serde_json::from_str(&r.body).unwrap_or_default();
            let bearer = r.header("authorization").unwrap_or_default().to_string();
            match r.path.as_str() {
                "/api/auth/websocket-ticket" | "/api/projects" | "/api/auth/pairing-token" if bearer != "Bearer dev-token" => json(401, json!({ "message": "bad token" })),
                "/api/auth/websocket-ticket" => json(200, json!({ "ticket": "ticket+1" })),
                "/api/projects" => json(200, json!({ "projects": [
                    { "id": "p1", "title": "Ripple", "workspaceRoot": "/src/ripple" },
                    { "id": "p2", "title": "Old", "workspaceRoot": "/src/old", "archivedAt": "2026-01-01" },
                ] })),
                "/api/auth/pairing-token" => json(200, json!({ "credential": "pair-code" })),
                "/oauth/mcp/register" => json(200, json!({ "client_id": "client-1" })),
                "/oauth/mcp/decision" => {
                    let auth = &body["authorization"];
                    assert_eq!(body["decision"]["code"], "pair-code");
                    *challenge.lock().unwrap() = auth["code_challenge"].as_str().unwrap_or_default().into();
                    json(200, json!({ "redirectTo": format!("{}?code=auth-code&state={}", auth["redirect_uri"].as_str().unwrap_or_default(), auth["state"].as_str().unwrap_or_default()) }))
                }
                "/oauth/mcp/token" => {
                    // PKCE: the verifier must hash to the challenge sent with the decision.
                    let verifier = query_param(&format!("?{}", r.body), "code_verifier").unwrap_or_default();
                    let ok = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())) == *challenge.lock().unwrap()
                        && query_param(&format!("?{}", r.body), "code").as_deref() == Some("auth-code");
                    if !ok { return json(400, json!({ "error_description": "bad PKCE" })) }
                    json(200, json!({ "access_token": format!("mcp-{}", now_ms()), "expires_in": 2_592_000 }))
                }
                "/mcp" => {
                    let mut left = rejects.lock().unwrap();
                    if !bearer.starts_with("Bearer mcp-") || *left > 0 {
                        *left = left.saturating_sub(1);
                        return Response { status: 401, headers: vec![], body: String::new() };
                    }
                    let id = body["id"].clone();
                    if id.is_null() { return Response { status: 202, headers: vec![], body: String::new() } }
                    let result = match (body["method"].as_str(), body["params"]["name"].as_str()) {
                        (Some("initialize"), _) => json!({ "protocolVersion": "2025-06-18" }),
                        (_, Some("t3_environment_links")) => json!({ "structuredContent": { "links": [{ "environmentId": "env-1", "label": "Desktop" }] } }),
                        (_, Some("t3_project_list")) => {
                            let cursor = body["params"]["arguments"]["cursor"].as_i64().unwrap_or(0);
                            json!({ "structuredContent": { "projects": [{ "id": format!("e{cursor}"), "title": format!("Env {cursor}"), "workspaceRoot": "/w" }], "nextCursor": if cursor == 0 { json!(1) } else { Value::Null } } })
                        }
                        (_, Some("t3_thread_launch")) => {
                            let args = &body["params"]["arguments"];
                            assert_eq!((args["environmentId"].as_str(), args["message"].as_str()), (Some("env-1"), Some("Fix it")));
                            json!({ "structuredContent": { "threadId": "thread-env" } })
                        }
                        _ => json!({ "isError": true, "content": [{ "type": "text", "text": "{\"message\":\"no such tool\"}" }] }),
                    };
                    // Streamable HTTP answers as an event stream.
                    Response { status: 200, headers: vec![("content-type".into(), "text/event-stream".into()), ("mcp-session-id".into(), "session-1".into())],
                        body: format!("event: message\ndata: {}\n\n", json!({ "jsonrpc": "2.0", "id": id, "result": result })) }
                }
                _ => json(404, json!({})),
            }
        }, move |text| {
            let call: Value = serde_json::from_str(text).unwrap_or_default();
            ws_log.lock().unwrap().push(format!("ws {}", call["tag"].as_str().unwrap_or_default()));
            let value = match call["tag"].as_str() {
                Some("server.getConfig") => json!({
                    "providers": [{ "instanceId": "claude", "models": [{ "slug": "opus", "name": "Claude Opus" }] }],
                    "settings": { "favorites": [{ "provider": "claude", "model": "opus" }, { "provider": "codex", "model": "gpt" }],
                        "defaultRuntimeMode": "approval-required", "linkedEnvironmentsEnabled": true },
                }),
                Some("orchestration.launchThread") => {
                    let p = &call["payload"];
                    assert_eq!((p["initialMessage"]["text"].as_str(), p["title"].as_str()), (Some("Fix it\nplease"), Some("Fix it")));
                    assert_eq!(p["workspaceStrategy"]["type"], "worktree");
                    json!({ "threadId": "thread-local" })
                }
                _ => return json!({ "_tag": "Exit", "exit": { "_tag": "Failure", "cause": [{ "error": { "message": "unknown tag" } }] } }).to_string(),
            };
            json!({ "_tag": "Exit", "exit": { "_tag": "Success", "value": value } }).to_string()
        });
        (base, log)
    }

    fn device(base: &str) -> T3Device {
        T3Device { name: "Laptop".into(), url: format!("{base}/"), token: "dev-token".into() }
    }

    #[test]
    fn the_client_loads_lists_and_starts_threads_on_a_device_and_its_linked_environment() {
        let (base, log) = mock_t3(0);
        let file = std::env::temp_dir().join(format!("ripple-t3-{}.json", std::process::id()));
        let client = Client::new(Some(file.clone()));
        let dev = device(&base);

        let loaded = client.load(&dev).unwrap();
        assert_eq!(loaded.favorites[0].label, "Claude Opus");
        assert_eq!(loaded.favorites[1].label, "gpt", "a model the server can't name keeps its slug");
        assert_eq!(loaded.runtime_mode, "approval-required");
        assert_eq!(loaded.links, [Link { id: "env-1".into(), label: "Desktop".into() }]);

        let local = client.projects(&dev, "").unwrap();
        assert_eq!(local, [Project { id: "p1".into(), title: "Ripple".into(), path: "/src/ripple".into() }], "archived projects are left out");
        let linked = client.projects(&dev, "env-1").unwrap();
        assert_eq!(linked.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["e0", "e1"], "every page is read");

        let launch = |env: &str| Launch { environment_id: env.into(), project_id: "p1".into(), instance_id: "claude".into(), model: "opus".into(),
            message: "Fix it\nplease".into(), worktree: true, runtime_mode: loaded.runtime_mode.clone() };
        assert_eq!(client.start_thread(&dev, &launch("")).unwrap(), "thread-local");
        assert_eq!(client.start_thread(&dev, &Launch { message: "Fix it".into(), ..launch("env-1") }).unwrap(), "thread-env");
        assert_eq!(client.start_thread(&dev, &Launch { message: "  ".into(), ..launch("") }), Err("Message is empty".into()));

        // One sign-in, cached on disk for the user alone, reused by the next app start.
        let registered = || log.lock().unwrap().iter().filter(|l| l.as_str() == "POST /oauth/mcp/register").count();
        assert_eq!(registered(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
        }
        Client::new(Some(file.clone())).projects(&dev, "env-1").unwrap();
        assert_eq!(registered(), 1, "a restarted client reuses the saved token");
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn a_revoked_token_signs_in_again_once_and_bad_tokens_report_the_server_message() {
        let (base, log) = mock_t3(1);
        let client = Client::new(None);
        assert_eq!(client.projects(&device(&base), "env-1").unwrap().len(), 2);
        assert_eq!(log.lock().unwrap().iter().filter(|l| l.as_str() == "POST /oauth/mcp/register").count(), 2);
        let wrong = T3Device { token: "nope".into(), ..device(&base) };
        assert_eq!(client.projects(&wrong, ""), Err("bad token".into()));
        assert!(Client::new(None).load(&T3Device { url: "http://127.0.0.1:9".into(), ..device(&base) }).unwrap_err().starts_with("Could not reach"));
    }

}
