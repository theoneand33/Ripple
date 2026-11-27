//! Match found: a multiplayer game asking for attention, shown as a live activity.
//!
//! Games flash their taskbar entry (they ask the window manager for attention) when a match
//! pops while you are tabbed out. Each OS's signal for that, narrowed to the games below:
//! - Windows: the shell hook's HSHELL_FLASH, the signal the taskbar draws.
//! - Linux: the urgency hint and _NET_WM_STATE_DEMANDS_ATTENTION over X11. On a GNOME or Plasma
//!   Wayland session that is XWayland, where games run: Proton's and Steam's native builds.
//! - Windows and macOS: League of Legends' ready check from its client API (wherever it is
//!   installed), with Accept.
//!
//! Native Wayland windows tell no other client they want attention, so they aren't seen; nor
//! does macOS show another app's Dock bounce, so League is the only game there.
//! Blocking; callers run it off the UI thread.

use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A match waiting on the user, and what the Island's button acts on.
#[derive(Clone, PartialEq, Debug)]
pub struct Match {
    pub game: &'static str,
    pub target: Target,
}

#[derive(Clone, PartialEq, Debug)]
pub enum Target {
    /// The game's window. The button brings it forward.
    Window(crate::platform::WindowId),
    /// League's client. The button accepts the match.
    League(League),
}

impl Match {
    /// The button's text.
    pub fn action(&self) -> &'static str { if matches!(self.target, Target::League(_)) { "Accept" } else { "Show" } }
}

/// Popular match-based games, by name, and what identifies their window, lower case: the
/// Windows exe or X11/Wayland class (`steam_app_<id>` under Proton), or the exact window title.
const GAMES: &[(&str, &[&str])] = &[
    ("Counter-Strike 2", &["cs2", "steam_app_730", "counter-strike 2"]),
    ("Dota 2", &["dota2", "steam_app_570", "dota 2"]),
    ("Deadlock", &["deadlock", "project8", "steam_app_1422450"]),
    ("League of Legends", &["leagueclientux", "leagueclient", "league of legends"]),
    ("Valorant", &["valorant-win64-shipping", "valorant"]),
    ("Overwatch 2", &["overwatch", "steam_app_2357570", "overwatch 2"]),
    ("Marvel Rivals", &["marvel-win64-shipping", "steam_app_2767030", "marvel rivals"]),
    ("Apex Legends", &["r5apex", "r5apex_dx12", "steam_app_1172470", "apex legends"]),
    ("Rocket League", &["rocketleague", "steam_app_252950", "rocket league"]),
    ("Rainbow Six Siege", &["rainbowsix", "rainbowsix_vulkan", "steam_app_359550"]),
    ("Team Fortress 2", &["tf_win64", "tf_linux64", "steam_app_440", "team fortress 2"]),
    ("PUBG: Battlegrounds", &["tslgame", "steam_app_578080"]),
    ("Fortnite", &["fortniteclient-win64-shipping", "fortnite"]),
    ("Call of Duty", &["cod", "steam_app_1938090", "call of duty"]),
    ("Battlefield 6", &["bf6", "steam_app_2807960", "battlefield 6"]),
    ("Halo Infinite", &["haloinfinite", "steam_app_1240440", "halo infinite"]),
    ("The Finals", &["discovery", "steam_app_2073850", "the finals"]),
    ("Arc Raiders", &["pioneergame", "pioneergame-e", "steam_app_1808500", "arc raiders"]),
    ("Delta Force", &["deltaforceclient-win64-shipping", "steam_app_2507950", "delta force"]),
    ("Escape from Tarkov", &["escapefromtarkov", "escape from tarkov"]),
    ("Hunt: Showdown 1896", &["huntgame", "steam_app_594650"]),
    ("Dead by Daylight", &["deadbydaylight-win64-shipping", "steam_app_381210", "dead by daylight"]),
    ("Street Fighter 6", &["streetfighter6", "steam_app_1364780", "street fighter 6"]),
    ("Tekken 8", &["polaris-win64-shipping", "steam_app_1778820", "tekken 8"]),
    ("Naraka: Bladepoint", &["narakabladepoint", "steam_app_1203220"]),
    ("StarCraft II", &["sc2_x64", "sc2", "starcraft ii"]),
    ("Hearthstone", &["hearthstone"]),
    ("Age of Empires IV", &["reliccardinal", "steam_app_1466860", "age of empires iv"]),
    ("Age of Empires II", &["aoe2de_s", "steam_app_813780"]),
    ("War Thunder", &["aces", "steam_app_236390", "war thunder"]),
    ("World of Tanks", &["worldoftanks", "world of tanks"]),
    ("Brawlhalla", &["brawlhalla", "steam_app_291550"]),
];

/// The game a window is, from its exe or class names and its title.
pub fn game<S: AsRef<str>>(keys: &[S]) -> Option<&'static str> {
    GAMES.iter().find(|(_, ids)| keys.iter().any(|k| ids.iter().any(|id| k.as_ref().trim().eq_ignore_ascii_case(id)))).map(|g| g.0)
}

/// Calls `on` once a second with the match waiting now, League's first. Runs for good: call it
/// on a thread of its own.
pub fn watch(on: impl Fn(Option<Match>)) {
    let flagged: Arc<Mutex<Vec<Match>>> = Arc::default();
    let f = flagged.clone();
    std::thread::spawn(move || attention(move |m| *f.lock().unwrap_or_else(|e| e.into_inner()) = m));
    let agent = league_agent();
    let mut lockfile = Lockfile::default();
    loop {
        let league = lockfile.read().and_then(|t| parse_lockfile(&t))
            .filter(|l| ready_check(&agent, l))
            .map(|l| Match { game: "League of Legends", target: Target::League(l) });
        on(league.or_else(|| flagged.lock().unwrap_or_else(|e| e.into_inner()).first().cloned()));
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// The Island's button: brings the game forward, or accepts League's match.
pub fn answer(m: &Match) {
    let done = match &m.target {
        Target::Window(id) => { crate::platform::focus_window(*id); Ok(()) }
        Target::League(l) => accept(&league_agent(), l),
    };
    if let Err(e) = done { eprintln!("Ripple: could not answer the {} match: {e}", m.game) }
}

/// Reports every game window asking for attention, each time that changes.
fn attention(report: impl Fn(Vec<Match>) + Send + 'static) {
    #[cfg(windows)]
    shell::watch(report);
    #[cfg(target_os = "linux")]
    x11_watch(report);
    #[cfg(target_os = "macos")]
    let _ = report;
}

// --- League of Legends ---

/// League's client API: its address and Basic auth header, from the lockfile it writes while open.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct League {
    base: String,
    auth: String,
}

/// League's lockfile, found beside the running client wherever it is installed. The path is
/// kept while it reads; without one, the processes are scanned every 5 seconds.
#[derive(Default)]
struct Lockfile {
    sys: sysinfo::System,
    path: Option<std::path::PathBuf>,
    next_scan: Option<std::time::Instant>,
}

impl Lockfile {
    fn read(&mut self) -> Option<String> {
        if let Some(text) = self.path.as_ref().and_then(|p| std::fs::read_to_string(p).ok()) { return Some(text) }
        // League runs on Windows and macOS only.
        if cfg!(target_os = "linux") || self.next_scan.is_some_and(|at| std::time::Instant::now() < at) { return None }
        self.next_scan = Some(std::time::Instant::now() + Duration::from_secs(5));
        let exe = sysinfo::ProcessRefreshKind::nothing().with_exe(sysinfo::UpdateKind::OnlyIfNotSet);
        self.sys.refresh_processes_specifics(sysinfo::ProcessesToUpdate::All, true, exe);
        self.path = self.sys.processes().values()
            .filter(|p| p.name().to_string_lossy().to_ascii_lowercase().starts_with("leagueclient"))
            .find_map(|p| lockfile_near(p.exe()?));
        std::fs::read_to_string(self.path.as_ref()?).ok()
    }
}

/// The lockfile in a folder above the client's exe: its own on Windows, `Contents/LoL` four up
/// from the macOS app's binary.
fn lockfile_near(exe: &std::path::Path) -> Option<std::path::PathBuf> {
    exe.ancestors().skip(1).take(5).map(|d| d.join("lockfile")).find(|p| p.is_file())
}

/// `LeagueClient:<pid>:<port>:<password>:<protocol>`.
fn parse_lockfile(text: &str) -> Option<League> {
    use base64::Engine;
    let [_, _, port, password, protocol] = text.trim().split(':').collect::<Vec<_>>()[..] else { return None };
    let port: u16 = port.parse().ok()?;
    if !matches!(protocol, "http" | "https") { return None }
    let auth = format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(format!("riot:{password}")));
    Some(League { base: format!("{protocol}://127.0.0.1:{port}"), auth })
}

/// League's client serves a certificate Riot signs itself, on loopback only, so it isn't checked.
fn league_agent() -> ureq::Agent {
    let tls = ureq::tls::TlsConfig::builder().disable_verification(true).build();
    crate::net::config().tls_config(tls).timeout_global(Some(Duration::from_secs(2))).build().into()
}

/// A ready check is up and the user hasn't answered it.
fn ready_check(agent: &ureq::Agent, l: &League) -> bool {
    let Ok(mut r) = agent.get(format!("{}/lol-matchmaking/v1/ready-check", l.base)).header("Authorization", &l.auth).call() else { return false };
    let Some(v) = r.body_mut().read_to_string().ok().and_then(|b| serde_json::from_str::<serde_json::Value>(&b).ok()) else { return false };
    v["state"] == "InProgress" && v["playerResponse"] == "None"
}

fn accept(agent: &ureq::Agent, l: &League) -> Result<(), String> {
    let r = agent.post(format!("{}/lol-matchmaking/v1/ready-check/accept", l.base)).header("Authorization", &l.auth).send_empty().map_err(|e| e.to_string())?;
    if r.status().is_success() { Ok(()) } else { Err(format!("League answered {}", r.status())) }
}

// --- Windows ---

#[cfg(windows)]
mod shell {
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::mpsc::{RecvTimeoutError, Sender};
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};

    use windows_sys::Win32::Foundation::{CloseHandle, HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW};
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    use super::{Match, Target};

    // The shell hook's codes as WPARAM, to match on; these shadow windows-sys' u32 ones.
    const HSHELL_WINDOWDESTROYED: usize = 2;
    const HSHELL_WINDOWACTIVATED: usize = 4;
    const HSHELL_RUDEAPPACTIVATED: usize = 0x8004;
    const HSHELL_FLASH: usize = 0x8006;
    /// A flash with no word since is over: no match waits longer to be accepted.
    const HOLD: Duration = Duration::from_secs(30);

    static SHELLHOOK: AtomicU32 = AtomicU32::new(0);
    static EVENTS: OnceLock<Mutex<Sender<(usize, isize)>>> = OnceLock::new();

    unsafe extern "system" fn proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        if msg != 0 && msg == SHELLHOOK.load(Ordering::Relaxed) {
            if let Some(tx) = EVENTS.get() { let _ = tx.lock().unwrap_or_else(|e| e.into_inner()).send((w, l)); }
            return 0;
        }
        // SAFETY: the arguments Windows passed in.
        unsafe { DefWindowProcW(hwnd, msg, w, l) }
    }

    /// A hidden window registered for shell hook messages, pumping them on its own thread.
    fn listen() {
        let class: Vec<u16> = "RippleGames\0".encode_utf16().collect();
        let hook: Vec<u16> = "SHELLHOOK\0".encode_utf16().collect();
        // SAFETY: NUL-terminated names that outlive the calls; the window is this thread's own.
        unsafe {
            SHELLHOOK.store(RegisterWindowMessageW(hook.as_ptr()), Ordering::Relaxed);
            let module = GetModuleHandleW(std::ptr::null());
            let mut wc: WNDCLASSW = std::mem::zeroed();
            wc.lpfnWndProc = Some(proc);
            wc.hInstance = module;
            wc.lpszClassName = class.as_ptr();
            RegisterClassW(&wc);
            let hwnd = CreateWindowExW(0, class.as_ptr(), class.as_ptr(), 0, 0, 0, 0, 0, std::ptr::null_mut(), std::ptr::null_mut(), module, std::ptr::null());
            if hwnd.is_null() || RegisterShellHookWindow(hwnd) == 0 { eprintln!("Ripple: could not listen for game windows flashing"); return }
            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 { DispatchMessageW(&msg); }
        }
    }

    /// The window's exe name without `.exe`, and its title.
    fn keys(hwnd: HWND) -> Vec<String> {
        let mut keys = Vec::new();
        // SAFETY: sized buffers with their lengths; a stale window or a process we may not open only makes the calls fail.
        unsafe {
            let mut pid = 0;
            GetWindowThreadProcessId(hwnd, &mut pid);
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if !process.is_null() {
                let (mut buf, mut len) = ([0u16; 1024], 1024u32);
                if QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) != 0 {
                    let path = String::from_utf16_lossy(&buf[..len as usize]);
                    if let Some(stem) = std::path::Path::new(&path).file_stem() { keys.push(stem.to_string_lossy().into_owned()) }
                }
                CloseHandle(process);
            }
            let mut title = [0u16; 512];
            let n = GetWindowTextW(hwnd, title.as_mut_ptr(), title.len() as i32);
            if n > 0 { keys.push(String::from_utf16_lossy(&title[..n as usize])) }
        }
        keys
    }

    /// A flashing game shows until its window comes forward or closes, or `HOLD` passes.
    pub fn watch(report: impl Fn(Vec<Match>)) {
        let (tx, rx) = std::sync::mpsc::channel();
        if EVENTS.set(Mutex::new(tx)).is_err() { return }
        std::thread::spawn(listen);
        // Ordered, so the match shown when two games flash stays the same one.
        let mut flagged: BTreeMap<isize, (Match, Instant)> = BTreeMap::new();
        loop {
            match rx.recv_timeout(Duration::from_secs(1)) {
                Ok((HSHELL_FLASH, hwnd)) => if let Some(game) = super::game(&keys(hwnd as HWND)) {
                    flagged.insert(hwnd, (Match { game, target: Target::Window(crate::platform::WindowId::Hwnd(hwnd)) }, Instant::now()));
                },
                Ok((HSHELL_WINDOWACTIVATED | HSHELL_RUDEAPPACTIVATED | HSHELL_WINDOWDESTROYED, hwnd)) => { flagged.remove(&hwnd); }
                Ok(_) | Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            // SAFETY: no arguments.
            let front = unsafe { GetForegroundWindow() } as isize;
            flagged.retain(|hwnd, (_, at)| *hwnd != front && at.elapsed() < HOLD);
            report(flagged.values().map(|f| f.0.clone()).collect());
        }
    }
}

// --- X11 ---

/// Polls X11 each second; reconnects after an error, such as the X server restarting.
#[cfg(target_os = "linux")]
fn x11_watch(report: impl Fn(Vec<Match>)) {
    let mut last = None;
    loop {
        if let Ok((conn, screen)) = x11rb::connect(None) {
            while let Ok(now) = x11_scan(&conn, screen) {
                if last.as_ref() != Some(&now) { report(now.clone()); last = Some(now) }
                std::thread::sleep(Duration::from_secs(1));
            }
        }
        // Lost the X server: what it last reported is no longer known to be waiting.
        if last.take().is_some_and(|l| !l.is_empty()) { report(Vec::new()) }
        std::thread::sleep(Duration::from_secs(10));
    }
}

/// Game windows with the urgency hint or _NET_WM_STATE_DEMANDS_ATTENTION.
#[cfg(target_os = "linux")]
fn x11_scan(conn: &impl x11rb::connection::Connection, screen: usize) -> Result<Vec<Match>, Box<dyn std::error::Error>> {
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt};
    let atom = |name: &[u8]| -> Result<u32, Box<dyn std::error::Error>> { Ok(conn.intern_atom(false, name)?.reply()?.atom) };
    let (list, state, demands, name, utf8) = (atom(b"_NET_CLIENT_LIST")?, atom(b"_NET_WM_STATE")?, atom(b"_NET_WM_STATE_DEMANDS_ATTENTION")?, atom(b"_NET_WM_NAME")?, atom(b"UTF8_STRING")?);
    let root = conn.setup().roots[screen].root;
    let windows: Vec<u32> = conn.get_property(false, root, list, AtomEnum::WINDOW, 0, 4096)?.reply()?.value32().into_iter().flatten().collect();
    // All requests first, then the replies: one round trip, not one per window. A window that just closed answers an error and is skipped.
    let asks: Vec<_> = windows.iter().map(|&w| (
        conn.get_property(false, w, AtomEnum::WM_HINTS, AtomEnum::WM_HINTS, 0, 1),
        conn.get_property(false, w, state, AtomEnum::ATOM, 0, 32),
    )).collect();
    let mut wanting = Vec::new();
    for (&w, (hints, states)) in windows.iter().zip(asks) {
        // WM_HINTS' first word is its flags; 256 is UrgencyHint.
        let urgent = hints.ok().and_then(|c| c.reply().ok()).is_some_and(|r| r.value32().and_then(|mut v| v.next()).is_some_and(|f| f & 256 != 0));
        let asks = states.ok().and_then(|c| c.reply().ok()).is_some_and(|r| r.value32().is_some_and(|mut v| v.any(|a| a == demands)));
        if urgent || asks { wanting.push(w) }
    }
    let mut found = Vec::new();
    for w in wanting {
        let class = conn.get_property(false, w, AtomEnum::WM_CLASS, AtomEnum::STRING, 0, 256)?.reply().map(|r| r.value).unwrap_or_default();
        let title = conn.get_property(false, w, name, utf8, 0, 256)?.reply().map(|r| r.value).unwrap_or_default();
        if let Some(game) = game(&x11_keys(&class, &title)) { found.push(Match { game, target: Target::Window(crate::platform::WindowId::X11(w)) }) }
    }
    Ok(found)
}

/// WM_CLASS is the instance and class names, each NUL-terminated; then the title.
#[cfg(any(target_os = "linux", test))]
fn x11_keys(class: &[u8], title: &[u8]) -> Vec<String> {
    class.split(|b| *b == 0).chain([title]).filter(|s| !s.is_empty()).map(|s| String::from_utf8_lossy(s).into_owned()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_name_their_game() {
        assert_eq!(game(&["cs2"]), Some("Counter-Strike 2"));
        assert_eq!(game(&["steam_app_570", "steam_app_570"]), Some("Dota 2"), "a Proton game by its Steam app id");
        assert_eq!(game(&["unknown", "VALORANT  "]), Some("Valorant"), "by title, any case");
        assert_eq!(game(&["firefox", "Counter-Strike 2 guide - Firefox"]), None, "a title only matches whole");
        assert_eq!(game::<&str>(&[]), None);
        assert_eq!(x11_keys(b"steam_app_730\0steam_app_730\0", b"Counter-Strike 2"), ["steam_app_730", "steam_app_730", "Counter-Strike 2"]);
        assert_eq!(x11_keys(b"", b""), Vec::<String>::new());
    }

    #[test]
    fn league_lockfile_parses() {
        let l = parse_lockfile("LeagueClient:1234:54321:s3cr3t:https\n").unwrap();
        assert_eq!(l.base, "https://127.0.0.1:54321");
        assert_eq!(l.auth, "Basic cmlvdDpzM2NyM3Q=");
        assert_eq!(parse_lockfile("LeagueClient:1234:notaport:pw:https"), None);
        assert_eq!(parse_lockfile("LeagueClient:1234:1:pw:file"), None);
        assert_eq!(parse_lockfile(""), None);
    }

    #[test]
    fn the_lockfile_is_found_above_the_clients_exe() {
        let root = std::env::temp_dir().join(format!("ripple-league-{}", std::process::id()));
        let lol = root.join("League of Legends.app/Contents/LoL");
        let mac = lol.join("LeagueClient.app/Contents/MacOS");
        std::fs::create_dir_all(&mac).unwrap();
        let before = lockfile_near(&mac.join("LeagueClient"));
        std::fs::write(lol.join("lockfile"), "LeagueClient:1:2:pw:https").unwrap();
        let mac_found = lockfile_near(&mac.join("LeagueClient"));
        let windows_found = lockfile_near(&lol.join("LeagueClientUx.exe"));
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(before, None);
        assert_eq!(mac_found, Some(lol.join("lockfile")));
        assert_eq!(windows_found, Some(lol.join("lockfile")), "beside the exe on Windows");
    }

    #[test]
    fn league_ready_check_shows_until_answered_and_accepts() {
        use crate::testserver::{json, serve};
        use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
        let accepted = Arc::new(AtomicBool::new(false));
        let a = accepted.clone();
        let base = serve(move |r| match (r.method.as_str(), r.path.as_str()) {
            _ if r.header("authorization") != Some("Basic cmlvdDpwdw==") => json(401, serde_json::json!({})),
            ("GET", "/lol-matchmaking/v1/ready-check") => {
                let answer = if a.load(SeqCst) { "Accepted" } else { "None" };
                json(200, serde_json::json!({ "state": "InProgress", "playerResponse": answer, "timer": 3.0 }))
            }
            ("POST", "/lol-matchmaking/v1/ready-check/accept") => { a.store(true, SeqCst); json(204, serde_json::json!(null)) }
            _ => json(404, serde_json::json!({ "message": "Not attached to a matchmaking queue." })),
        });
        let agent = league_agent();
        let l = League { base, auth: "Basic cmlvdDpwdw==".into() };
        assert!(ready_check(&agent, &l));
        assert!(accept(&agent, &l).is_ok() && accepted.load(SeqCst));
        assert!(!ready_check(&agent, &l), "accepted: nothing left to answer");
        assert!(!ready_check(&agent, &League { auth: "Basic x".into(), ..l.clone() }));
        assert!(accept(&agent, &League { auth: "Basic x".into(), ..l }).is_err());
    }
}
