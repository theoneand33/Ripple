//! Discord's local RPC, over the desktop app's IPC socket (a named pipe on Windows): the voice
//! channel the user is in, who is in it and speaking, and mute, deafen and leave.
//! Uses the user's own Discord app: Discord only lets an unapproved app's testers use RPC.
//! Blocking; callers run it off the UI thread.

use std::collections::VecDeque;
use std::io::{Read, Write};

use crate::config::DiscordToken as Token;
use serde_json::{Value, json};

const SCOPES: [&str; 3] = ["rpc", "rpc.voice.read", "rpc.voice.write"];
/// The token exchange must name a redirect the app registered; Settings says to add this one.
pub const REDIRECT: &str = "http://127.0.0.1";
pub const TOKEN_URL: &str = "https://discord.com/api/oauth2/token";
pub const CLOSED: &str = "Open Discord to see your voice channel";

/// The user's Discord app (Developer Portal → OAuth2).
#[derive(Clone, Debug, PartialEq)]
pub struct App {
    pub id: String,
    pub secret: String,
}


#[derive(Clone, Debug, PartialEq, Default)]
pub struct Member {
    pub id: String,
    pub name: String,
    /// The avatar's URL on Discord's CDN; None for a default avatar.
    pub avatar: Option<String>,
    pub speaking: bool,
    pub muted: bool,
    pub deafened: bool,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Channel {
    pub id: String,
    pub name: String,
    pub members: Vec<Member>,
}

/// The user's voice: the channel they are in, and their own mute and deafen.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Voice {
    pub channel: Option<Channel>,
    pub mute: bool,
    pub deaf: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Command {
    Mute(bool),
    Deaf(bool),
    Leave,
}

/// Why a connection ended: Discord refused something (a sign-in, a scope, the app), which
/// trying again at once won't fix; or the connection or network was lost.
#[derive(Clone, Debug, PartialEq)]
enum Failure {
    Refused(String),
    Lost(String),
}

impl From<Failure> for String {
    fn from(f: Failure) -> String { match f { Failure::Refused(e) | Failure::Lost(e) => e } }
}

fn lost<T>(_: T) -> Failure { Failure::Lost(CLOSED.into()) }

trait Stream: Read + Write + Send {}
impl<T: Read + Write + Send> Stream for T {}

/// The first IPC endpoint Discord (or its Flatpak or Snap) listens on. Another local user can
/// make one in a shared folder like /tmp while Discord is closed, and `AUTHENTICATE` sends the
/// token: only a socket the user owns counts.
#[cfg(unix)]
fn connect() -> Option<Box<dyn Stream>> {
    use std::os::unix::fs::MetadataExt;
    // SAFETY: getuid takes nothing and cannot fail.
    let me = unsafe { libc::getuid() };
    let dirs: Vec<String> = ["XDG_RUNTIME_DIR", "TMPDIR", "TMP", "TEMP"].iter().filter_map(|v| std::env::var(v).ok()).chain(["/tmp".into()]).collect();
    for dir in dirs {
        for sub in ["", "app/com.discordapp.Discord/", "snap.discord/", ".flatpak/dev.vencord.Vesktop/xdg-run/"] {
            for i in 0..10 {
                let path = format!("{dir}/{sub}discord-ipc-{i}");
                if std::fs::metadata(&path).is_ok_and(|m| m.uid() == me) && let Ok(s) = std::os::unix::net::UnixStream::connect(&path) { return Some(Box::new(s)) }
            }
        }
    }
    None
}

/// Any process can serve `discord-ipc-N` while Discord is closed: only a pipe served from the
/// user's own login session counts. (A process of the same user could read the token from
/// `prefs.json` anyway.)
#[cfg(windows)]
fn connect() -> Option<Box<dyn Stream>> {
    (0..10).find_map(|i| std::fs::OpenOptions::new().read(true).write(true).open(format!(r"\\.\pipe\discord-ipc-{i}")).ok().filter(same_session))
        .map(|f| Box::new(f) as Box<dyn Stream>)
}

#[cfg(windows)]
fn same_session(pipe: &std::fs::File) -> bool {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::{Pipes::GetNamedPipeServerProcessId, RemoteDesktop::ProcessIdToSessionId};
    let (mut server, mut theirs, mut ours) = (0u32, u32::MAX, 0u32);
    // SAFETY: the handle is the open pipe's, and the out pointers are locals.
    unsafe {
        GetNamedPipeServerProcessId(pipe.as_raw_handle(), &mut server) != 0
            && ProcessIdToSessionId(server, &mut theirs) != 0
            && ProcessIdToSessionId(std::process::id(), &mut ours) != 0
            && theirs == ours
    }
}

#[cfg(not(any(unix, windows)))]
fn connect() -> Option<Box<dyn Stream>> { None }

const HANDSHAKE: u32 = 0;
const FRAME: u32 = 1;
const CLOSE: u32 = 2;
const PING: u32 = 3;
const PONG: u32 = 4;

fn write_frame(w: &mut impl Write, op: u32, body: &Value) -> Result<(), Failure> {
    let body = body.to_string();
    let mut frame = Vec::with_capacity(8 + body.len());
    frame.extend(op.to_le_bytes());
    frame.extend((body.len() as u32).to_le_bytes());
    frame.extend(body.as_bytes());
    w.write_all(&frame).and_then(|_| w.flush()).map_err(lost)
}

fn read_frame(r: &mut impl Read) -> Result<(u32, Value), Failure> {
    let mut head = [0u8; 8];
    r.read_exact(&mut head).map_err(lost)?;
    let op = u32::from_le_bytes(head[..4].try_into().unwrap());
    let len = u32::from_le_bytes(head[4..].try_into().unwrap()) as usize;
    // A voice channel's state is a few KB: anything near this is not Discord.
    if len > 4 << 20 { return Err(Failure::Lost("Discord sent a frame too large".into())) }
    let mut body = vec![0; len];
    r.read_exact(&mut body).map_err(lost)?;
    Ok((op, serde_json::from_slice(&body).unwrap_or_default()))
}

/// One RPC connection, handshaken.
struct Rpc<S> {
    stream: S,
    nonce: u64,
    /// Events that came while a command waited for its answer.
    events: VecDeque<Value>,
    /// How Discord closed the connection: every later read fails the same way, so a call whose
    /// error is ignored doesn't hide it.
    closed: Option<Failure>,
}

impl<S: Read + Write> Rpc<S> {
    fn open(mut stream: S, client_id: &str) -> Result<Rpc<S>, Failure> {
        write_frame(&mut stream, HANDSHAKE, &json!({ "v": 1, "client_id": client_id }))?;
        let mut rpc = Rpc { stream, nonce: 0, events: VecDeque::new(), closed: None };
        loop {
            let frame = rpc.frame()?;
            if frame["evt"] == "READY" { return Ok(rpc) }
        }
    }

    /// The next message, answering pings. Discord closes with a reason when it refuses the app;
    /// quitting just drops the connection.
    fn frame(&mut self) -> Result<Value, Failure> {
        loop {
            if let Some(e) = &self.closed { return Err(e.clone()) }
            match read_frame(&mut self.stream)? {
                (PING, body) => write_frame(&mut self.stream, PONG, &body)?,
                (CLOSE, body) => self.closed = Some(body["message"].as_str().map_or_else(|| Failure::Lost(CLOSED.into()), |m| Failure::Refused(format!("Discord: {m}")))),
                (_, body) => return Ok(body),
            }
        }
    }

    /// Sends `cmd` and waits for its answer's data.
    fn call(&mut self, cmd: &str, args: Value, evt: Option<&str>) -> Result<Value, Failure> {
        self.nonce += 1;
        let nonce = self.nonce.to_string();
        let mut msg = json!({ "cmd": cmd, "args": args, "nonce": nonce });
        if let Some(evt) = evt { msg["evt"] = evt.into() }
        write_frame(&mut self.stream, FRAME, &msg)?;
        loop {
            let mut frame = self.frame()?;
            if frame["nonce"] != nonce.as_str() {
                if frame["cmd"] == "DISPATCH" { self.events.push_back(frame) }
                continue;
            }
            if frame["evt"] == "ERROR" {
                return Err(Failure::Refused(frame["data"]["message"].as_str().map_or_else(|| format!("Discord refused {cmd}"), |m| format!("Discord: {m}"))));
            }
            return Ok(frame["data"].take());
        }
    }

    /// The next event Discord dispatches.
    fn event(&mut self) -> Result<Value, Failure> {
        if let Some(e) = self.events.pop_front() { return Ok(e) }
        loop {
            let frame = self.frame()?;
            if frame["cmd"] == "DISPATCH" { return Ok(frame) }
        }
    }

    fn authenticate(&mut self, token: &Token) -> Result<(), Failure> {
        self.call("AUTHENTICATE", json!({ "access_token": token.access }), None).map(drop)
    }
}

/// Trades a code or a refresh token for a new token at `url`.
fn grant(url: &str, app: &App, form: &[(&str, &str)], now: i64) -> Result<Token, Failure> {
    let form = [("client_id", app.id.as_str()), ("client_secret", app.secret.as_str())].into_iter().chain(form.iter().copied());
    let mut res = crate::net::agent().post(url).send_form(form).map_err(|e| Failure::Lost(format!("Couldn't reach Discord: {e}")))?;
    // Only OAuth's refusals (400 invalid_grant, 401 invalid_client) say the token or app is no
    // good; a rate limit or outage tries again later rather than asking the user anew.
    let status = res.status().as_u16();
    if !res.status().is_success() && !matches!(status, 400 | 401) { return Err(Failure::Lost(format!("Discord answered {status}"))) }
    let body = crate::net::read_json(&mut res);
    let (Some(access), Some(refresh)) = (body["access_token"].as_str(), body["refresh_token"].as_str()) else {
        return Err(Failure::Refused(format!("Discord refused the sign-in: {}", body["error_description"].as_str().or(body["error"].as_str()).unwrap_or("check the app's ID and secret"))));
    };
    Ok(Token { access: access.into(), refresh: refresh.into(), expires: now + body["expires_in"].as_i64().unwrap_or(0) * 1000 })
}

/// `token`, renewed when it nears expiry; Ok(None) when Discord refused the renewal.
fn fresh(token: Token, app: &App, token_url: &str, now: i64) -> Result<Option<Token>, Failure> {
    if token.expires - 60_000 >= now { return Ok(Some(token)) }
    match grant(token_url, app, &[("grant_type", "refresh_token"), ("refresh_token", &token.refresh)], now) {
        Ok(t) => Ok(Some(t)),
        Err(Failure::Refused(_)) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Authenticates with the saved token, renewing it near expiry; otherwise asks the user in
/// Discord. Each new token goes to `save`.
fn sign_in<S: Read + Write>(rpc: &mut Rpc<S>, app: &App, token: Option<Token>, token_url: &str, now: i64, save: &impl Fn(Token), on: &impl Fn(Result<Voice, String>)) -> Result<(), Failure> {
    let token = match token {
        Some(old) => {
            let t = fresh(old.clone(), app, token_url, now)?;
            if let Some(t) = t.as_ref().filter(|t| **t != old) { save(t.clone()) }
            t
        }
        None => None,
    };
    if let Some(t) = &token && rpc.authenticate(t).is_ok() { return Ok(()) }
    on(Err("Approve Ripple in Discord".into()));
    let code = rpc.call("AUTHORIZE", json!({ "client_id": app.id, "scopes": SCOPES }), None)?;
    let code = code["code"].as_str().ok_or_else(|| Failure::Refused("Discord sent no sign-in code".into()))?;
    let token = grant(token_url, app, &[("grant_type", "authorization_code"), ("code", code), ("redirect_uri", REDIRECT)], now)?;
    save(token.clone());
    rpc.authenticate(&token)
}

/// A voice state as Discord sends it, in a channel's list and in its events.
fn member(v: &Value) -> Member {
    let user = &v["user"];
    let id = user["id"].as_str().unwrap_or_default().to_string();
    let name = [&v["nick"], &user["global_name"], &user["username"]].into_iter().filter_map(|n| n.as_str()).find(|n| !n.is_empty()).unwrap_or("Someone");
    let state = &v["voice_state"];
    let on = |k: &str| state[k].as_bool().unwrap_or(false);
    Member {
        avatar: user["avatar"].as_str().map(|hash| format!("https://cdn.discordapp.com/avatars/{id}/{hash}.png?size=64")),
        name: name.into(),
        speaking: false,
        // Muted for the user too counts: they won't hear them.
        muted: on("mute") || on("self_mute") || on("suppress") || v["mute"].as_bool().unwrap_or(false),
        deafened: on("deaf") || on("self_deaf"),
        id,
    }
}

const CHANNEL_EVENTS: [&str; 5] = ["VOICE_STATE_CREATE", "VOICE_STATE_UPDATE", "VOICE_STATE_DELETE", "SPEAKING_START", "SPEAKING_STOP"];

/// The channel the user is in now, subscribed to its members' events.
fn selected<S: Read + Write>(rpc: &mut Rpc<S>) -> Result<Option<Channel>, Failure> {
    let data = rpc.call("GET_SELECTED_VOICE_CHANNEL", json!({}), None)?;
    let Some(id) = data["id"].as_str() else { return Ok(None) };
    for evt in CHANNEL_EVENTS { rpc.call("SUBSCRIBE", json!({ "channel_id": id }), Some(evt))?; }
    let members = data["voice_states"].as_array().map(|v| v.iter().map(member).collect()).unwrap_or_default();
    Ok(Some(Channel { id: id.into(), name: data["name"].as_str().unwrap_or("Voice").into(), members }))
}

/// Applies one event to `voice`; true when it changed.
fn apply<S: Read + Write>(rpc: &mut Rpc<S>, voice: &mut Voice, ev: &Value) -> Result<bool, Failure> {
    let before = voice.clone();
    let data = &ev["data"];
    let user = data["user_id"].as_str().or(data["user"]["id"].as_str()).unwrap_or_default();
    match ev["evt"].as_str().unwrap_or_default() {
        "VOICE_SETTINGS_UPDATE" => {
            voice.mute = data["mute"].as_bool().unwrap_or(voice.mute);
            voice.deaf = data["deaf"].as_bool().unwrap_or(voice.deaf);
        }
        "VOICE_CHANNEL_SELECT" => {
            if let Some(old) = voice.channel.take() {
                // Best effort: Discord drops a left channel's events anyway.
                for evt in CHANNEL_EVENTS { let _ = rpc.call("UNSUBSCRIBE", json!({ "channel_id": old.id }), Some(evt)); }
            }
            voice.channel = selected(rpc)?;
        }
        evt => if let Some(c) = &mut voice.channel {
            let at = c.members.iter().position(|m| m.id == user);
            match (evt, at) {
                ("VOICE_STATE_CREATE" | "VOICE_STATE_UPDATE", Some(i)) => c.members[i] = Member { speaking: c.members[i].speaking, ..member(data) },
                ("VOICE_STATE_CREATE" | "VOICE_STATE_UPDATE", None) => c.members.push(member(data)),
                ("VOICE_STATE_DELETE", Some(i)) => { c.members.remove(i); }
                ("SPEAKING_START" | "SPEAKING_STOP", Some(i)) => c.members[i].speaking = evt == "SPEAKING_START",
                _ => {}
            }
        },
    }
    Ok(*voice != before)
}

/// One connection's life: signs in, then reports the voice to `on` until Discord closes.
fn session<S: Read + Write>(stream: S, app: &App, token: Option<Token>, token_url: &str, now: i64, save: &impl Fn(Token), on: &impl Fn(Result<Voice, String>)) -> Result<(), Failure> {
    let mut rpc = Rpc::open(stream, &app.id)?;
    sign_in(&mut rpc, app, token, token_url, now, save, on)?;
    rpc.call("SUBSCRIBE", json!({}), Some("VOICE_CHANNEL_SELECT"))?;
    rpc.call("SUBSCRIBE", json!({}), Some("VOICE_SETTINGS_UPDATE"))?;
    let settings = rpc.call("GET_VOICE_SETTINGS", json!({}), None)?;
    let mut voice = Voice { channel: selected(&mut rpc)?, mute: settings["mute"].as_bool().unwrap_or(false), deaf: settings["deaf"].as_bool().unwrap_or(false) };
    on(Ok(voice.clone()));
    loop {
        let ev = rpc.event()?;
        if apply(&mut rpc, &mut voice, &ev)? { on(Ok(voice.clone())) }
    }
}

/// Watches the user's voice for as long as the app runs, reconnecting while Discord is closed.
/// `app` gives the Discord app and its saved token, or None while none is set up.
pub fn watch(app: impl Fn() -> Option<(App, Option<Token>)>, save: impl Fn(Token), on: impl Fn(Result<Voice, String>)) {
    // The app Discord last refused, and until when (Unix ms) it isn't asked again: each try
    // can put an approval prompt in front of the user.
    let mut refused: Option<(App, i64)> = None;
    loop {
        let now = crate::date::now_ms();
        // shortcut: a changed app ID is picked up on the next connection, not mid-session.
        if let Some((app, token)) = app() && !refused.as_ref().is_some_and(|(a, until)| *a == app && now < *until) {
            let end = match connect() {
                Some(stream) => session(stream, &app, token, TOKEN_URL, now, &save, &on),
                None => Err(Failure::Lost(CLOSED.into())),
            };
            match end {
                Err(Failure::Refused(e)) => {
                    on(Err(format!("{e}. Ripple tries again in 10 minutes, or once the app changes in Settings.")));
                    refused = Some((app, now + 600_000));
                }
                end => on(Err(end.err().map_or_else(|| CLOSED.into(), String::from))),
            }
        }
        std::thread::sleep(std::time::Duration::from_secs(10));
    }
}

/// Sends one voice command on a connection of its own, as the watcher's only reads. Returns
/// the token if it was renewed first, to save even when the command fails: the old refresh
/// token may be spent.
pub fn command(app: &App, token: &Token, cmd: Command) -> (Option<Token>, Result<(), String>) {
    let Some(stream) = connect() else { return (None, Err(CLOSED.into())) };
    run(stream, app, token, TOKEN_URL, crate::date::now_ms(), cmd)
}

fn run<S: Read + Write>(stream: S, app: &App, token: &Token, token_url: &str, now: i64, cmd: Command) -> (Option<Token>, Result<(), String>) {
    let fresh = match fresh(token.clone(), app, token_url, now) {
        Ok(Some(t)) => t,
        Ok(None) => return (None, Err("Discord signed Ripple out: it asks again shortly".into())),
        Err(e) => return (None, Err(e.into())),
    };
    let renewed = (fresh != *token).then(|| fresh.clone());
    (renewed, send(stream, app, &fresh, cmd).map_err(String::from))
}

fn send<S: Read + Write>(stream: S, app: &App, token: &Token, cmd: Command) -> Result<(), Failure> {
    let mut rpc = Rpc::open(stream, &app.id)?;
    rpc.authenticate(token)?;
    let (name, args) = match cmd {
        Command::Mute(on) => ("SET_VOICE_SETTINGS", json!({ "mute": on })),
        Command::Deaf(on) => ("SET_VOICE_SETTINGS", json!({ "deaf": on })),
        Command::Leave => ("SELECT_VOICE_CHANNEL", json!({ "channel_id": null })),
    };
    rpc.call(name, args, None).map(drop)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;
    use std::sync::{Arc, Mutex};

    use crate::testserver::{json as reply, serve};

    /// A fake Discord on the other end of `them`: answers each command with `answer`, sends
    /// `events` once subscribed to the channel, and quits after the channel is next read.
    fn discord(mut them: UnixStream, answer: impl Fn(&str, &Value) -> Value + Send + 'static, events: Vec<Value>) -> std::thread::JoinHandle<Vec<String>> {
        std::thread::spawn(move || {
            let mut seen = Vec::new();
            let (op, hello) = read_frame(&mut them).unwrap();
            assert_eq!((op, hello["client_id"].as_str()), (HANDSHAKE, Some("app")));
            write_frame(&mut them, FRAME, &json!({ "cmd": "DISPATCH", "evt": "READY", "data": {} })).unwrap();
            let (mut events, mut sent) = (Some(events), false);
            while let Ok((_, msg)) = read_frame(&mut them) {
                let cmd = msg["cmd"].as_str().unwrap().to_string();
                seen.push(format!("{cmd} {}", msg["evt"].as_str().unwrap_or("")).trim().to_string());
                let data = answer(&cmd, &msg["args"]);
                let evt = if data["error"].is_string() { json!("ERROR") } else { Value::Null };
                let data = if evt.is_null() { data } else { json!({ "message": data["error"] }) };
                write_frame(&mut them, FRAME, &json!({ "cmd": cmd, "evt": evt, "nonce": msg["nonce"], "data": data })).unwrap();
                if sent && cmd == "GET_SELECTED_VOICE_CHANNEL" {
                    // Discord quits.
                    write_frame(&mut them, CLOSE, &json!({ "message": "bye" })).unwrap();
                }
                if msg["evt"] == "SPEAKING_STOP" && let Some(events) = events.take() {
                    for e in events { write_frame(&mut them, FRAME, &json!({ "cmd": "DISPATCH", "evt": e["evt"], "data": e["data"] })).unwrap() }
                    sent = true;
                }
            }
            seen
        })
    }

    fn state(id: &str, name: &str, self_mute: bool) -> Value {
        let avatar = if id == "1" { json!("abc") } else { Value::Null };
        json!({ "nick": "", "mute": false, "user": { "id": id, "username": name, "global_name": null, "avatar": avatar },
            "voice_state": { "mute": false, "deaf": false, "self_mute": self_mute, "self_deaf": false, "suppress": false } })
    }

    fn channel() -> Value { json!({ "id": "c1", "name": "General", "voice_states": [state("1", "ada", false), state("2", "bob", true)] }) }

    #[test]
    fn signs_in_once_then_follows_the_channel() {
        let token_url = format!("{}/token", serve(|r| {
            assert!(r.body.contains("grant_type=authorization_code") && r.body.contains("code=c0de") && r.body.contains("client_secret=s3cret"), "{}", r.body);
            reply(200, json!({ "access_token": "acc", "refresh_token": "ref", "expires_in": 604800 }))
        }));
        let (ours, theirs) = UnixStream::pair().unwrap();
        let reads = std::sync::atomic::AtomicUsize::new(0);
        let fake = discord(theirs, move |cmd, args| match cmd {
            "AUTHORIZE" => json!({ "code": "c0de" }),
            "AUTHENTICATE" if args["access_token"] == "acc" => json!({ "user": {} }),
            "AUTHENTICATE" => json!({ "error": "Invalid token" }),
            "GET_VOICE_SETTINGS" => json!({ "mute": true, "deaf": false }),
            // In General at first; out of voice once the user leaves.
            "GET_SELECTED_VOICE_CHANNEL" if reads.fetch_add(1, std::sync::atomic::Ordering::Relaxed) == 0 => channel(),
            "GET_SELECTED_VOICE_CHANNEL" => Value::Null,
            _ => json!({}),
        }, vec![
            json!({ "evt": "SPEAKING_START", "data": { "user_id": "1" } }),
            json!({ "evt": "VOICE_STATE_UPDATE", "data": state("1", "ada", true) }),
            json!({ "evt": "VOICE_STATE_DELETE", "data": state("2", "bob", true) }),
            json!({ "evt": "VOICE_SETTINGS_UPDATE", "data": { "mute": false, "deaf": true } }),
            json!({ "evt": "VOICE_CHANNEL_SELECT", "data": { "channel_id": null } }),
        ]);
        let app = App { id: "app".into(), secret: "s3cret".into() };
        let saved = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let stale = Token { access: "old".into(), refresh: "r".into(), expires: i64::MAX };
        let (sv, sn) = (saved.clone(), seen.clone());
        let end = session(ours, &app, Some(stale), &token_url, 1000, &move |t| sv.lock().unwrap().push(t), &move |v| sn.lock().unwrap().push(v));
        assert_eq!(end, Err(Failure::Refused("Discord: bye".into())));
        assert_eq!(*saved.lock().unwrap(), vec![Token { access: "acc".into(), refresh: "ref".into(), expires: 1000 + 604_800_000 }]);
        let seen = seen.lock().unwrap();
        assert_eq!(seen[0], Err("Approve Ripple in Discord".into()));
        let first = seen[1].clone().unwrap();
        let c = first.channel.unwrap();
        assert_eq!((first.mute, c.name.as_str(), c.members.len()), (true, "General", 2));
        assert_eq!(c.members[0].avatar.as_deref(), Some("https://cdn.discordapp.com/avatars/1/abc.png?size=64"));
        assert_eq!((c.members[0].muted, c.members[1].muted, c.members[1].avatar.clone()), (false, true, None));
        let in_call = seen[5].clone().unwrap();
        let ada = &in_call.channel.as_ref().unwrap().members;
        // Still speaking after a state update; bob left.
        assert_eq!((ada.len(), ada[0].speaking, ada[0].muted, in_call.mute, in_call.deaf), (1, true, true, false, true));
        assert_eq!(seen.len(), 7);
        assert_eq!(seen[6], Ok(Voice { channel: None, mute: false, deaf: true }));
        let calls = fake.join().unwrap();
        assert_eq!(&calls[..3], ["AUTHENTICATE", "AUTHORIZE", "AUTHENTICATE"]);
        assert!(calls.contains(&"UNSUBSCRIBE SPEAKING_START".to_string()));
        assert!(calls.contains(&"SUBSCRIBE SPEAKING_START".to_string()) && calls.contains(&"SUBSCRIBE VOICE_CHANNEL_SELECT".to_string()));
    }

    #[test]
    fn renews_an_expiring_token_and_sends_commands() {
        let token_url = format!("{}/token", serve(|r| {
            assert!(r.body.contains("grant_type=refresh_token") && r.body.contains("refresh_token=r1"), "{}", r.body);
            reply(200, json!({ "access_token": "a2", "refresh_token": "r2", "expires_in": 10 }))
        }));
        let (ours, theirs) = UnixStream::pair().unwrap();
        let fake = discord(theirs, |cmd, args| match cmd {
            "AUTHENTICATE" if args["access_token"] == "a2" => json!({}),
            "AUTHENTICATE" => json!({ "error": "Invalid token" }),
            "GET_SELECTED_VOICE_CHANNEL" => Value::Null,
            _ => json!({}),
        }, vec![]);
        let app = App { id: "app".into(), secret: "s".into() };
        let saved = Arc::new(Mutex::new(None));
        let sv = saved.clone();
        let (out, done) = std::sync::mpsc::channel();
        let url = token_url.clone();
        std::thread::spawn(move || {
            let r = session(ours, &app, Some(Token { access: "a1".into(), refresh: "r1".into(), expires: 0 }), &url, 5, &move |t| *sv.lock().unwrap() = Some(t), &move |v| { let _ = out.send(v); });
            drop(r);
        });
        assert_eq!(done.recv().unwrap(), Ok(Voice::default()));
        assert_eq!(saved.lock().unwrap().clone().unwrap().expires, 10_005);
        drop(fake);

        let (ours, theirs) = UnixStream::pair().unwrap();
        let fake = discord(theirs, |cmd, args| match cmd {
            "SELECT_VOICE_CHANNEL" => { assert!(args["channel_id"].is_null()); json!({}) }
            "SET_VOICE_SETTINGS" => json!({ "error": "Missing scope" }),
            _ => json!({}),
        }, vec![]);
        let app = App { id: "app".into(), secret: "s".into() };
        let token = Token { access: "a2".into(), refresh: "r2".into(), expires: i64::MAX };
        assert_eq!(run(ours, &app, &token, &token_url, 5, Command::Leave), (None, Ok(())));
        assert_eq!(fake.join().unwrap(), ["AUTHENTICATE", "SELECT_VOICE_CHANNEL"]);
        let (ours, theirs) = UnixStream::pair().unwrap();
        let fake = discord(theirs, |cmd, _| if cmd == "SET_VOICE_SETTINGS" { json!({ "error": "Missing scope" }) } else { json!({}) }, vec![]);
        assert_eq!(run(ours, &app, &token, &token_url, 5, Command::Mute(true)), (None, Err("Discord: Missing scope".into())));
        drop(fake);
        // A command with an expired token renews it first, and hands the new one back to save,
        // even when the command then fails.
        let (ours, theirs) = UnixStream::pair().unwrap();
        let fake = discord(theirs, |cmd, args| match cmd {
            "AUTHENTICATE" if args["access_token"] != "a2" => json!({ "error": "Invalid token" }),
            "SET_VOICE_SETTINGS" => json!({ "error": "Missing scope" }),
            _ => json!({}),
        }, vec![]);
        let expired = Token { access: "a1".into(), refresh: "r1".into(), expires: 0 };
        let renewed = Token { access: "a2".into(), refresh: "r2".into(), expires: 10_005 };
        assert_eq!(run(ours, &app, &expired, &token_url, 5, Command::Deaf(true)), (Some(renewed), Err("Discord: Missing scope".into())));
        assert_eq!(fake.join().unwrap(), ["AUTHENTICATE", "SET_VOICE_SETTINGS"]);
    }

    #[test]
    fn a_denied_approval_or_a_refused_renewal_is_refused_and_an_outage_is_lost() {
        let token_url = format!("{}/token", serve(|r| match r.body.contains("refresh_token=gone") {
            true => reply(400, json!({ "error": "invalid_grant" })),
            false => reply(429, json!({ "message": "You are being rate limited." })),
        }));
        let app = App { id: "app".into(), secret: "s".into() };
        let expired = |refresh: &str| Some(Token { access: "a".into(), refresh: refresh.into(), expires: 0 });
        // Refused renewal: asks in Discord, where the user says no.
        let (ours, theirs) = UnixStream::pair().unwrap();
        let fake = discord(theirs, |cmd, _| if cmd == "AUTHORIZE" { json!({ "error": "User denied" }) } else { json!({}) }, vec![]);
        assert_eq!(session(ours, &app, expired("gone"), &token_url, 5, &|_| {}, &|_| {}), Err(Failure::Refused("Discord: User denied".into())));
        assert_eq!(fake.join().unwrap(), ["AUTHORIZE"]);
        // Discord's token server rate-limiting: no prompt, try again later.
        let (ours, theirs) = UnixStream::pair().unwrap();
        let fake = discord(theirs, |_, _| json!({}), vec![]);
        assert_eq!(session(ours, &app, expired("ok"), &token_url, 5, &|_| {}, &|_| {}), Err(Failure::Lost("Discord answered 429".into())));
        assert_eq!(fake.join().unwrap(), Vec::<String>::new());
    }

    #[test]
    fn a_member_is_named_by_nick_then_display_name() {
        let mut v = state("3", "carol", false);
        assert_eq!(member(&v).name, "carol");
        v["user"]["global_name"] = json!("Carol");
        assert_eq!(member(&v).name, "Carol");
        v["nick"] = json!("CJ");
        v["voice_state"]["self_deaf"] = json!(true);
        let m = member(&v);
        assert_eq!((m.name.as_str(), m.deafened, m.muted), ("CJ", true, false));
    }
}
