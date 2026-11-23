//! What Ripple asks the OS: now playing, battery, Bluetooth, camera and microphone use,
//! CPU, memory, disk and network load, launching apps and links. Everything here blocks, so callers run it off the UI thread.

use std::ffi::OsString;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use crate::command::tokenize_args;
use crate::config::QuickApp;

/// `WAYLAND_DISPLAY` as Ripple was started with it; set only in a Wayland session.
static WAYLAND_DISPLAY: OnceLock<OsString> = OnceLock::new();

/// Records the Wayland session, if any. With `xwayland`, Ripple's own windows go on XWayland:
/// `WAYLAND_DISPLAY` is unset for them, and apps Ripple starts still get it.
///
/// # Safety
/// With `xwayland`, no other thread may be running, as for `std::env::remove_var`.
#[cfg(target_os = "linux")]
pub unsafe fn note_wayland(xwayland: bool) {
    let Some(display) = std::env::var_os("WAYLAND_DISPLAY") else { return };
    let _ = WAYLAND_DISPLAY.set(display);
    // SAFETY: the caller guarantees no other thread runs.
    if xwayland { unsafe { std::env::remove_var("WAYLAND_DISPLAY") } }
}

/// The session is Wayland: XWayland's active window doesn't follow native Wayland windows.
#[cfg(target_os = "linux")]
pub fn wayland_session() -> bool { WAYLAND_DISPLAY.get().is_some() }

/// No console flashes up for a child process.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn command(program: &str) -> Command {
    let mut cmd = Command::new(program);
    if let Some(display) = WAYLAND_DISPLAY.get() { cmd.env("WAYLAND_DISPLAY", display); }
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(&mut cmd, CREATE_NO_WINDOW);
    cmd
}

/// A status tool that runs longer than this is killed, so a hung one can't stall its poller.
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Trimmed stdout, or None when the program is missing, failed or ran past `TIMEOUT`.
pub(crate) fn output(program: &str, args: &[&str]) -> Option<String> {
    use std::io::Read;
    let mut child = command(program).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    // Read stdout on its own thread: a full pipe would otherwise block the child forever.
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || { let mut out = Vec::new(); let _ = stdout.read_to_end(&mut out); out });
    let started = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < TIMEOUT => std::thread::sleep(std::time::Duration::from_millis(20)),
            _ => { let _ = child.kill(); let _ = child.wait(); return None }
        }
    };
    let out = reader.join().ok()?;
    status.success().then(|| String::from_utf8_lossy(&out).trim().to_string())
}

/// Starts a program Ripple doesn't wait for, in its own process group so it outlives Ripple.
fn spawn(mut cmd: Command) {
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW | 0x0000_0200); // | CREATE_NEW_PROCESS_GROUP
    }
    match cmd.spawn() {
        // Reaps the child when it exits; otherwise it stays a zombie until Ripple quits.
        Ok(mut child) => { std::thread::spawn(move || { let _ = child.wait(); }); }
        Err(e) => eprintln!("Ripple: could not start {:?}: {e}", cmd.get_program()),
    }
}

/// Text inside a PowerShell single-quoted string: PowerShell ends one at any of these quotes.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
fn ps_quote(text: &str) -> String {
    text.chars().flat_map(|c| match c {
        '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' => vec![c, c],
        c => vec![c],
    }).collect()
}

/// Runs each script it reads (one base64 UTF-16LE line) and answers with its output as one
/// base64 UTF-8 line. It quits when Ripple closes its stdin.
#[cfg(windows)]
const PS_SERVER: &str = r#"
while ($true) {
  $line = [Console]::In.ReadLine()
  if ($null -eq $line) { exit }
  $out = try {
    $script = [Text.Encoding]::Unicode.GetString([Convert]::FromBase64String($line))
    (& ([scriptblock]::Create($script)) | ForEach-Object { "$_" }) -join "`n"
  } catch { '' }
  [Console]::Out.WriteLine([Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($out.Trim())))
  [Console]::Out.Flush()
}
"#;

#[cfg(windows)]
struct PsServer {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    lines: std::sync::mpsc::Receiver<String>,
}

#[cfg(windows)]
fn start_ps_server() -> Option<PsServer> {
    use std::io::BufRead;
    let mut child = command("powershell").args(["-NoProfile", "-EncodedCommand", &ps_encode(PS_SERVER)])
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let (stdin, stdout) = (child.stdin.take()?, child.stdout.take()?);
    let (send, lines) = std::sync::mpsc::channel();
    std::thread::spawn(move || for line in std::io::BufReader::new(stdout).lines().map_while(Result::ok) { if send.send(line).is_err() { break } });
    Some(PsServer { child, stdin, lines })
}

/// Runs a PowerShell script in one long-lived PowerShell: the pollers run several a second, and
/// starting a PowerShell for each costs a few hundred ms of CPU. Calls run one at a time, so slow
/// one-off scripts use their own process. Trimmed output, or "" on failure; a script that runs past
/// `TIMEOUT` (or a dead process) takes the process down, and the next call starts another.
#[cfg(windows)]
fn powershell(script: &str) -> String {
    static SERVER: PsSlot = std::sync::Mutex::new(None);
    powershell_in(&SERVER, script)
}

#[cfg(windows)]
type PsSlot = std::sync::Mutex<Option<PsServer>>;

/// Now playing: WinRT calls that can hang get a process of their own, so a stuck
/// one doesn't hold up the camera, mic and Bluetooth pollers.
#[cfg(windows)]
fn powershell_media(script: &str) -> String {
    static SERVER: PsSlot = std::sync::Mutex::new(None);
    powershell_in(&SERVER, script)
}

#[cfg(windows)]
fn powershell_in(slot: &PsSlot, script: &str) -> String {
    use base64::Engine;
    use std::io::Write;
    let mut server = slot.lock().unwrap_or_else(|e| e.into_inner());
    if server.is_none() { *server = start_ps_server() }
    let Some(ps) = server.as_mut() else { return String::new() };
    let answer = writeln!(ps.stdin, "{}", ps_encode(script)).and_then(|_| ps.stdin.flush()).ok()
        .and_then(|_| ps.lines.recv_timeout(TIMEOUT).ok())
        .and_then(|line| base64::engine::general_purpose::STANDARD.decode(line.trim()).ok());
    match answer {
        Some(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        None => {
            if let Some(mut ps) = server.take() { let _ = ps.child.kill(); let _ = ps.child.wait(); }
            String::new()
        }
    }
}

/// A script as `-EncodedCommand` takes it: base64 of its UTF-16LE.
#[cfg(windows)]
fn ps_encode(script: &str) -> String {
    use base64::Engine;
    let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    base64::engine::general_purpose::STANDARD.encode(utf16)
}

#[cfg(windows)]
fn start_process(target: &str) {
    let mut cmd = command("powershell");
    cmd.args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", &format!("Start-Process '{}'", ps_quote(target))]);
    spawn(cmd);
}

// --- Now playing ---

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Track {
    pub name: String,
    pub artist: String,
    /// `https:`, `file:` or `data:` URL of the cover, when the player gives one.
    pub artwork_url: Option<String>,
    pub playing: bool,
    pub paused: bool,
    /// The app it plays in, which a cover press opens; empty when unknown.
    pub source: String,
    /// Position and length in whole seconds, when the player reports a length.
    pub progress: Option<(u32, u32)>,
}

/// Position and length as the player gives them, in seconds, as `Track::progress`.
fn progress(position: Option<f64>, length: Option<f64>) -> Option<(u32, u32)> {
    let length = length.filter(|l| l.is_finite() && *l >= 1.0)?;
    let position = position.filter(|p| p.is_finite()).unwrap_or(0.0).clamp(0.0, length);
    Some((position as u32, length as u32))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaCommand {
    PlayPause,
    Next,
    Previous,
}

/// One `playerctl -a` line per player: `instance||status||title||artist||album||artUrl||position||length`.
const PLAYERCTL_FORMAT: &str = "{{playerInstance}}||{{status}}||{{title}}||{{artist}}||{{album}}||{{mpris:artUrl}}||{{position}}||{{mpris:length}}";

/// `playerctl -a` output for every player. One player that can't be reached (a browser tab or
/// KDE Connect device that just went away) makes `-a` exit 1 and stop, hiding the one that plays:
/// then ask each player alone.
#[cfg(target_os = "linux")]
fn playerctl_all() -> Option<String> {
    output("playerctl", &["-a", "metadata", "--format", PLAYERCTL_FORMAT]).or_else(|| {
        let lines: Vec<String> = output("playerctl", &["-l"])?.lines()
            .filter_map(|name| output("playerctl", &["-p", name.trim(), "metadata", "--format", PLAYERCTL_FORMAT]))
            .collect();
        (!lines.is_empty()).then(|| lines.join("\n"))
    })
}

/// The player to show from `playerctl -a` output: the first one playing, else the first paused
/// one, else any with a title. `playerctl` alone takes the first player, often an idle tab.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn pick_player(out: &str) -> Option<Vec<&str>> {
    let players: Vec<Vec<&str>> = out.lines().map(|l| l.split("||").collect::<Vec<_>>())
        .filter(|p| p.len() >= 5 && !p[2].trim().is_empty()).collect();
    let status = |want: &str| players.iter().find(|p| p[1].eq_ignore_ascii_case(want)).cloned();
    status("playing").or_else(|| status("paused")).or_else(|| players.into_iter().next())
}

#[cfg(target_os = "linux")]
pub fn media() -> Option<Track> {
    let out = playerctl_all()?;
    let p = pick_player(&out)?;
    let status = p[1].to_lowercase();
    let micros = |i: usize| p.get(i).and_then(|v| v.trim().parse::<f64>().ok()).map(|v| v / 1e6);
    Some(Track {
        name: p[2].into(),
        artist: p[3].into(),
        artwork_url: p.get(5).filter(|u| !u.is_empty()).map(|u| u.to_string()),
        playing: status == "playing",
        paused: status == "paused",
        // Not the MPRIS name: any bus client can claim one, and a cover press would launch it.
        source: String::new(),
        // playerctl reports microseconds.
        progress: progress(micros(6), micros(7)),
    })
}

/// AppleScript that sets `spotifyRunning` and `musicRunning`, leaving at most one true while
/// both run: Music wins only while Spotify is not playing.
#[cfg(target_os = "macos")]
const PICK_PLAYER: &str = r#"
tell application "System Events"
    set spotifyRunning to (name of every process) contains "Spotify"
    set musicRunning to (name of every process) contains "Music"
end tell
if spotifyRunning and musicRunning then
    try
        tell application "Spotify" to set spotifyState to player state as string
        tell application "Music" to set musicState to player state as string
        if musicState is "playing" and spotifyState is not "playing" then set spotifyRunning to false
    end try
end if
"#;

#[cfg(target_os = "macos")]
pub fn media() -> Option<Track> {
    const SCRIPT: &str = r#"
if spotifyRunning then
    try
        tell application "Spotify"
            set mediaState to player state as string
            set songName to name of current track
            set artistName to artist of current track
            set albumName to album of current track
            try
                set artUrl to artwork url of current track
            on error
                set artUrl to ""
            end try
            set posText to ""
            set lenText to ""
            try
                set posText to (player position as integer) as string
                set lenText to ((duration of current track) div 1000) as string
            end try
        end tell
        return "Spotify" & "||" & mediaState & "||" & songName & "||" & artistName & "||" & albumName & "||" & artUrl & "||" & posText & "||" & lenText
    on error
        return "Error"
    end try
else if musicRunning then
    try
        tell application "Music"
            set mediaState to player state as string
            set songName to name of current track
            set artistName to artist of current track
            set albumName to album of current track
            set posText to ""
            set lenText to ""
            try
                set posText to (player position as integer) as string
                set lenText to (duration of current track as integer) as string
            end try
        end tell
        return "Music" & "||" & mediaState & "||" & songName & "||" & artistName & "||" & albumName & "||" & "" & "||" & posText & "||" & lenText
    on error
        return "Error"
    end try
else
    return "None"
end if"#;
    let out = output("osascript", &["-e", &format!("{PICK_PLAYER}{SCRIPT}")])?;
    let parts: Vec<&str> = out.split("||").collect();
    if parts.len() < 4 { return None }
    let playing = parts[1] == "playing";
    let seconds = |i: usize| parts.get(i).and_then(|v| v.trim().parse::<f64>().ok());
    Some(Track {
        name: parts[2].into(),
        artist: parts[3].into(),
        artwork_url: parts.get(5).filter(|u| !u.is_empty()).map(|u| u.to_string()),
        playing,
        paused: !playing,
        source: parts[0].into(),
        progress: progress(seconds(6), seconds(7)),
    })
}

#[cfg(windows)]
pub fn media() -> Option<Track> {
    const SCRIPT: &str = "[Console]::OutputEncoding = [System.Text.Encoding]::UTF8; Add-Type -AssemblyName System.Runtime.WindowsRuntime; $manager = [Windows.Media.Control.GlobalSystemMediaTransportControlsSessionManager, Windows.Media.Control, ContentType = WindowsRuntime]::RequestAsync().GetAwaiter().GetResult(); $session = $manager.GetSessions() | Where-Object { $_.GetPlaybackInfo().PlaybackStatus -eq 'Playing' } | Select-Object -First 1; if (-not $session) { $session = $manager.GetCurrentSession() }; if ($session) { $props = $session.TryGetMediaPropertiesAsync().GetAwaiter().GetResult(); $playback = $session.GetPlaybackInfo(); $status = $playback.PlaybackStatus; $timeline = $session.GetTimelineProperties(); $position = $timeline.Position; if ($status.ToString() -eq 'Playing' -and $timeline.LastUpdatedTime.Year -gt 2000) { $position += [DateTimeOffset]::Now - $timeline.LastUpdatedTime }; $length = $timeline.EndTime - $timeline.StartTime; $thumbnail = $props.Thumbnail; $key = [string]$session.SourceAppUserModelId + '|' + $props.Title + '|' + $props.Artist + '|' + $props.AlbumTitle + '|' + $timeline.EndTime; if ($key -ne $global:artKey) { $global:art = ''; if ($thumbnail) { try { $stream = $thumbnail.OpenReadAsync().GetAwaiter().GetResult(); $buffer = New-Object byte[] $stream.Size; $reader = New-Object Windows.Storage.Streams.DataReader $stream; $reader.LoadAsync($stream.Size).GetAwaiter().GetResult() | Out-Null; $reader.ReadBytes($buffer); $global:art = 'data:image/png;base64,' + [Convert]::ToBase64String($buffer); $global:artKey = $key; $reader.Close(); $stream.Close(); } catch { } } }; $artwork = $global:art; $info = @{ Title = $props.Title; Artist = $props.Artist; Album = $props.AlbumTitle; Status = $status.ToString().ToLower(); Source = $session.SourceAppUserModelId; Artwork = $artwork; Position = $position.TotalSeconds; Length = $length.TotalSeconds }; return $info | ConvertTo-Json -Compress; } return 'null';";
    // The cover is read once per track (`$global:art` lasts in the shared PowerShell); a player sets it after the title, so an empty one is retried.
    let out = powershell_media(SCRIPT);
    if let Ok(data) = serde_json::from_str::<serde_json::Value>(&out)
        && data.is_object()
    {
        let field = |k: &str| data[k].as_str().unwrap_or_default().to_string();
        let or = |v: String, fallback: &str| if v.is_empty() { fallback.to_string() } else { v };
        let playing = field("Status") == "playing";
        return Some(Track {
            name: or(field("Title"), "Unknown Title"),
            artist: or(field("Artist"), "Unknown Artist"),
            artwork_url: Some(field("Artwork")).filter(|a| !a.is_empty()),
            playing,
            paused: !playing,
            source: field("Source"),
            progress: progress(data["Position"].as_f64(), data["Length"].as_f64()),
        });
    }
    // No media session: fall back to Spotify's window title ("Artist - Song").
    let out = powershell_media("[Console]::OutputEncoding = [System.Text.Encoding]::UTF8; Get-Process | Where-Object {$_.ProcessName -eq 'Spotify'} | Select-Object -ExpandProperty MainWindowTitle");
    let title = out.lines().map(str::trim).find(|l| l.contains(" - "))?;
    let (artist, song) = title.split_once(" - ")?;
    Some(Track { name: song.into(), artist: artist.into(), playing: true, source: "Spotify".into(), ..Track::default() })
}

pub fn media_control(cmd: MediaCommand) {
    #[cfg(target_os = "linux")]
    {
        let arg = match cmd { MediaCommand::PlayPause => "play-pause", MediaCommand::Next => "next", MediaCommand::Previous => "previous" };
        // Control the player the Island shows, not playerctl's first one.
        let shown = playerctl_all()
            .and_then(|out| pick_player(&out).map(|p| p[0].to_string()));
        match shown {
            Some(name) => output("playerctl", &["-p", &name, arg]),
            None => output("playerctl", &[arg]),
        };
    }
    #[cfg(target_os = "macos")]
    {
        // A fixed word, never user text: it is spliced into the script.
        let verb = match cmd { MediaCommand::PlayPause => "playpause", MediaCommand::Next => "next track", MediaCommand::Previous => "previous track" };
        let script = format!(r#"{PICK_PLAYER}
if spotifyRunning then
    tell application "Spotify" to {verb}
else if musicRunning then
    tell application "Music" to {verb}
end if"#);
        output("osascript", &["-e", &script]);
    }
    #[cfg(windows)]
    {
        let method = match cmd { MediaCommand::PlayPause => "TryTogglePlayPauseAsync", MediaCommand::Next => "TrySkipNextAsync", MediaCommand::Previous => "TrySkipPreviousAsync" };
        // A one-off process: a click mustn't wait behind the media poll's shared one.
        let _ = output("powershell", &["-NoProfile", "-EncodedCommand", &ps_encode(&format!("Add-Type -AssemblyName System.Runtime.WindowsRuntime; $m = [Windows.Media.Control.GlobalSystemMediaTransportControlsSessionManager, Windows.Media.Control, ContentType = WindowsRuntime]::RequestAsync().GetAwaiter().GetResult(); $s = $m.GetSessions() | Where-Object {{ $_.GetPlaybackInfo().PlaybackStatus -eq 'Playing' }} | Select-Object -First 1; if (-not $s) {{ $s = $m.GetCurrentSession() }}; if ($s) {{ $s.{method}().GetAwaiter().GetResult() | Out-Null }}"))]);
    }
}

// --- Desktop drags ---

/// Calls `on(true)` when a drag and drop starts anywhere on the desktop, and `on(false)` once
/// its mouse button comes up. Blocks for good; run it on its own thread.
pub fn watch_drags(on: impl Fn(bool)) {
    #[cfg(target_os = "linux")]
    {
        // shortcut: X11 (and XWayland) drags only; native Wayland has no global drag signal.
        let _ = x11_drags(&on);
    }
    #[cfg(target_os = "macos")]
    { crate::mac::watch_drags(on) }
    // shortcut: Windows has no public signal for a drag that starts in another app; add an OLE hook if users ask.
    #[cfg(windows)]
    { let _ = on; }
}

/// A drag source owns `XdndSelection` from its start (XDND); the pointer's buttons say when it ends.
#[cfg(target_os = "linux")]
fn x11_drags(on: &impl Fn(bool)) -> Result<(), Box<dyn std::error::Error>> {
    use x11rb::connection::Connection;
    use x11rb::protocol::Event;
    use x11rb::protocol::xfixes::{ConnectionExt as _, SelectionEventMask};
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, KeyButMask};
    let (conn, screen) = x11rb::connect(None)?;
    let root = conn.setup().roots[screen].root;
    conn.xfixes_query_version(5, 0)?.reply()?;
    let dnd = conn.intern_atom(false, b"XdndSelection")?.reply()?.atom;
    let (types, uris) = (conn.intern_atom(false, b"XdndTypeList")?.reply()?.atom, conn.intern_atom(false, b"text/uri-list")?.reply()?.atom);
    conn.xfixes_select_selection_input(root, dnd, SelectionEventMask::SET_SELECTION_OWNER)?.check()?;
    let buttons = KeyButMask::BUTTON1 | KeyButMask::BUTTON2 | KeyButMask::BUTTON3;
    loop {
        let Event::XfixesSelectionNotify(e) = conn.wait_for_event()? else { continue };
        if e.owner == x11rb::NONE { continue }
        // A source with more than three types lists them here; one without files (text, a tab) is skipped.
        let listed = conn.get_property(false, e.owner, types, AtomEnum::ATOM, 0, 256)?.reply()?;
        if listed.type_ != x11rb::NONE && !listed.value32().is_some_and(|mut t| t.any(|a| a == uris)) { continue }
        on(true);
        // shortcut: gives up after 30 s, as XWayland may not see the release over a native Wayland window.
        let until = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while conn.query_pointer(root)?.reply()?.mask.intersects(buttons) && std::time::Instant::now() < until {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        on(false);
    }
}

// --- Bluetooth, camera, microphone ---

pub fn bluetooth_connected() -> bool {
    #[cfg(target_os = "linux")]
    { output("bluetoothctl", &["devices", "Connected"]).is_some_and(|o| !o.is_empty()) }
    #[cfg(target_os = "macos")]
    {
        output("system_profiler", &["SPBluetoothDataType", "-json"])
            .and_then(|o| serde_json::from_str::<serde_json::Value>(&o).ok())
            .is_some_and(|d| d["SPBluetoothDataType"][0]["device_connected"].as_array().is_some_and(|a| !a.is_empty()))
    }
    #[cfg(windows)]
    // A paired device keeps status OK while away; DEVPKEY_Device_IsConnected says whether it is linked now.
    { powershell("$ids = @(Get-PnpDevice -Class Bluetooth -PresentOnly -ErrorAction SilentlyContinue | Where-Object { $_.InstanceId -like 'BTHENUM\\DEV_*' } | ForEach-Object { $_.InstanceId }); if ($ids.Count -eq 0) { 'false' } else { @(Get-PnpDeviceProperty -InstanceId $ids -KeyName '{83DA6326-97A6-4088-9453-A1923F573B29} 15' -ErrorAction SilentlyContinue | Where-Object { $_.Data -eq $true }).Count -gt 0 }").eq_ignore_ascii_case("true") }
}

pub fn camera_in_use() -> bool {
    #[cfg(target_os = "linux")]
    {
        let devices: Vec<String> = std::fs::read_dir("/dev").into_iter().flatten().flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| n.starts_with("video"))
            .map(|n| format!("/dev/{n}"))
            .collect();
        if devices.is_empty() { return false }
        // fuser prints the PIDs holding a device on stdout, and exits non-zero when there are none.
        let args: Vec<&str> = devices.iter().map(String::as_str).collect();
        command("fuser").args(&args).stdin(Stdio::null()).stderr(Stdio::null()).output()
            .is_ok_and(|o| !String::from_utf8_lossy(&o.stdout).trim().is_empty())
    }
    #[cfg(target_os = "macos")]
    {
        // Any CoreMediaIO device (built-in, USB or virtual) some process is capturing from.
        #[repr(C)]
        struct Address { selector: u32, scope: u32, element: u32 }
        #[link(name = "CoreMediaIO", kind = "framework")]
        unsafe extern "C" {
            fn CMIOObjectGetPropertyDataSize(object: u32, address: *const Address, qualifier_size: u32, qualifier: *const std::ffi::c_void, size: *mut u32) -> i32;
            fn CMIOObjectGetPropertyData(object: u32, address: *const Address, qualifier_size: u32, qualifier: *const std::ffi::c_void, size: u32, used: *mut u32, data: *mut std::ffi::c_void) -> i32;
        }
        // 'dev#' kCMIOHardwarePropertyDevices, 'gone' kCMIODevicePropertyDeviceIsRunningSomewhere, 'glob' global scope.
        const GLOBAL: u32 = u32::from_be_bytes(*b"glob");
        let devices_at = Address { selector: u32::from_be_bytes(*b"dev#"), scope: GLOBAL, element: 0 };
        let running_at = Address { selector: u32::from_be_bytes(*b"gone"), scope: GLOBAL, element: 0 };
        // SAFETY: 1 is kCMIOObjectSystemObject; every buffer is sized as the call is told.
        unsafe {
            let mut size = 0u32;
            if CMIOObjectGetPropertyDataSize(1, &devices_at, 0, std::ptr::null(), &mut size) != 0 { return false }
            let mut ids = vec![0u32; size as usize / 4];
            let mut used = 0u32;
            if CMIOObjectGetPropertyData(1, &devices_at, 0, std::ptr::null(), (ids.len() * 4) as u32, &mut used, ids.as_mut_ptr().cast()) != 0 { return false }
            ids.truncate(used as usize / 4);
            ids.iter().any(|&id| {
                let mut running = 0u32;
                CMIOObjectGetPropertyData(id, &running_at, 0, std::ptr::null(), 4, &mut used, (&raw mut running).cast()) == 0 && running != 0
            })
        }
    }
    #[cfg(windows)]
    { capability_in_use("webcam") }
}

pub fn microphone_in_use() -> bool {
    #[cfg(target_os = "linux")]
    // One line per stream recording from a source.
    { output("pactl", &["list", "short", "source-outputs"]).is_some_and(|o| !o.is_empty()) }
    #[cfg(target_os = "macos")]
    {
        #[repr(C)]
        struct Address { selector: u32, scope: u32, element: u32 }
        #[link(name = "CoreAudio", kind = "framework")]
        unsafe extern "C" {
            fn AudioObjectGetPropertyDataSize(object: u32, address: *const Address, qualifier_size: u32, qualifier: *const std::ffi::c_void, size: *mut u32) -> i32;
            fn AudioObjectGetPropertyData(object: u32, address: *const Address, qualifier_size: u32, qualifier: *const std::ffi::c_void, size: *mut u32, data: *mut std::ffi::c_void) -> i32;
        }
        // 'dev#' kAudioHardwarePropertyDevices, 'stm#' kAudioDevicePropertyStreams, 'gone' kAudioDevicePropertyDeviceIsRunningSomewhere;
        // scopes 'glob' (global) and 'inpt' (input). 1 is kAudioObjectSystemObject.
        let at = |selector: &[u8; 4], scope: &[u8; 4]| Address { selector: u32::from_be_bytes(*selector), scope: u32::from_be_bytes(*scope), element: 0 };
        // SAFETY (both closures): a size query, and a read into a buffer of the size it gave.
        let size_of = |object: u32, a: &Address| { let mut n = 0u32; (unsafe { AudioObjectGetPropertyDataSize(object, a, 0, std::ptr::null(), &mut n) } == 0).then_some(n) };
        let running = |device: u32| { let (mut v, mut n) = (0u32, 4u32); unsafe { AudioObjectGetPropertyData(device, &at(b"gone", b"glob"), 0, std::ptr::null(), &mut n, (&raw mut v).cast()) == 0 && v != 0 } };
        let devices = at(b"dev#", b"glob");
        let Some(mut n) = size_of(1, &devices) else { return false };
        let mut ids = vec![0u32; n as usize / 4];
        // SAFETY: `ids` holds `n` bytes; `n` is updated to the bytes written.
        if unsafe { AudioObjectGetPropertyData(1, &devices, 0, std::ptr::null(), &mut n, ids.as_mut_ptr().cast()) } != 0 { return false }
        ids.truncate(n as usize / 4);
        // Any device that has input streams: an app may record from a mic that isn't the system default.
        // shortcut: "running somewhere" is device-wide, so a headset playing music, or a loopback driver carrying audio,
        // lights the dot; a missed recording is worse, so it is read at global scope too (the input scope may be refused).
        ids.iter().any(|&d| size_of(d, &at(b"stm#", b"inpt")).is_some_and(|n| n > 0) && running(d))
    }
    #[cfg(windows)]
    { capability_in_use("microphone") }
}

/// Windows records a capability's use per app; a stop time of 0 means it is in use now.
#[cfg(windows)]
fn capability_in_use(name: &str) -> bool {
    // Their own process: the privacy dots shouldn't wait on a slow Bluetooth scan.
    static SERVER: PsSlot = std::sync::Mutex::new(None);
    powershell_in(&SERVER, &format!("@(Get-ChildItem -Path \"HKCU:\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\CapabilityAccessManager\\ConsentStore\\{name}\" -Recurse -ErrorAction SilentlyContinue | ForEach-Object {{ Get-ItemProperty -Path $_.PSPath -Name \"LastUsedTimeStop\" -ErrorAction SilentlyContinue }} | Where-Object {{ $_ -and $_.LastUsedTimeStop -eq 0 }}).Count -gt 0"))
        .eq_ignore_ascii_case("true")
}

// --- Volume and brightness ---

/// Calls `on(volume, brightness)` with each as a percent (a muted output reads 0), about ten times
/// a second; None where the OS doesn't say or nothing was read. Blocks for good; run it on its own thread.
pub fn watch_levels(on: impl Fn(Option<u8>, Option<u8>)) {
    // Volume is read when `pactl subscribe` reports a sink change (or twice a second without it);
    // brightness is a cheap sysfs read, polled every 100 ms.
    #[cfg(target_os = "linux")]
    {
        use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
        let changed = Arc::new(AtomicBool::new(true));
        let events = { let changed = changed.clone(); std::thread::spawn(move || watch_sinks(&changed)) };
        for tick in 0u64.. {
            let read = changed.swap(false, Ordering::Relaxed) || (events.is_finished() && tick % 5 == 0);
            on(if read { volume() } else { None }, brightness());
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
    // One process for good: starting one per poll costs too much CPU at this rate.
    #[cfg(windows)]
    let (mut cmd, from_stderr) = { let mut c = command("powershell"); c.args(["-NoProfile", "-EncodedCommand", &ps_encode(&format!("{WINDOWS_AUDIO}{WINDOWS_LEVELS}"))]); (c, false) };
    // AppleScript's `log` writes to stderr. shortcut: no brightness, as macOS has no public CLI for it; read it through IOKit/DisplayServices if users ask.
    #[cfg(target_os = "macos")]
    let (mut cmd, from_stderr) = { let mut c = command("osascript"); for line in MAC_LEVELS { c.args(["-e", line]); } (c, true) };
    #[cfg(not(target_os = "linux"))]
    follow_lines(&mut cmd, from_stderr, |line| { let (volume, brightness) = parse_levels(line); on(volume, brightness) });
}

/// Runs `cmd` and calls `on_line` with each line it writes (to stderr with `from_stderr`),
/// starting it again whenever it exits. Blocks for good; run it on its own thread.
pub(crate) fn follow_lines(cmd: &mut Command, from_stderr: bool, mut on_line: impl FnMut(&str)) -> ! {
    use std::io::BufRead;
    loop {
        cmd.stdin(Stdio::null());
        if from_stderr { cmd.stdout(Stdio::null()).stderr(Stdio::piped()) } else { cmd.stdout(Stdio::piped()).stderr(Stdio::null()) };
        if let Ok(mut child) = cmd.spawn() {
            let out: Option<Box<dyn std::io::Read>> = if from_stderr { child.stderr.take().map(|e| Box::new(e) as _) } else { child.stdout.take().map(|o| Box::new(o) as _) };
            for line in out.into_iter().flat_map(|o| std::io::BufReader::new(o).lines().map_while(Result::ok)) { on_line(&line) }
            let _ = child.kill();
            let _ = child.wait();
        }
        // It died or is missing: start another later, without spinning.
        std::thread::sleep(std::time::Duration::from_secs(10));
    }
}

/// Sets `changed` on each sink or default-sink event from `pactl subscribe`; returns once pactl exits.
#[cfg(target_os = "linux")]
fn watch_sinks(changed: &std::sync::atomic::AtomicBool) {
    use std::io::BufRead;
    let Ok(mut child) = command("pactl").arg("subscribe").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn() else { return };
    for line in child.stdout.take().into_iter().flat_map(|o| std::io::BufReader::new(o).lines().map_while(Result::ok)) {
        if line.contains(" on sink ") || line.contains(" on server") { changed.store(true, std::sync::atomic::Ordering::Relaxed) }
    }
    let _ = child.wait();
}

/// Logs the volume ten times a second, 0 when muted; "missing value" where the output has none (HDMI).
#[cfg(target_os = "macos")]
const MAC_LEVELS: [&str; 9] = [
    "repeat", "set s to get volume settings",
    "if output muted of s then", "log 0", "else", "log (output volume of s)", "end if",
    "delay 0.1", "end repeat",
];

/// `RippleAudio`: the default output's level and mute, through Core Audio. Added once per process.
#[cfg(windows)]
const WINDOWS_AUDIO: &str = r#"
if (-not ('RippleAudio' -as [type])) { Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
[Guid("5CDF2C82-841E-4546-9722-0CF74078229A"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
interface IAudioEndpointVolume {
  int f(); int g(); int h(); int i();
  int SetMasterVolumeLevelScalar(float level, Guid context);
  int j();
  int GetMasterVolumeLevelScalar(out float level);
  int k(); int l(); int m(); int n();
  int SetMute([MarshalAs(UnmanagedType.Bool)] bool mute, IntPtr context);
  int GetMute([MarshalAs(UnmanagedType.Bool)] out bool mute);
}
[Guid("D666063F-1587-4E43-81F1-B948E807363F"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
interface IMMDevice { int Activate(ref Guid id, int ctx, int args, out IAudioEndpointVolume volume); }
[Guid("A95664D2-9614-4F35-A746-DE8DB63617E6"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
interface IMMDeviceEnumerator { int f(); int GetDefaultAudioEndpoint(int flow, int role, out IMMDevice device); }
[ComImport, Guid("BCDE0395-E52F-467C-8E3D-C4579291692E")] class MMDeviceEnumerator { }
public static class RippleAudio {
  static IAudioEndpointVolume Endpoint() {
    IMMDevice device;
    Marshal.ThrowExceptionForHR(((IMMDeviceEnumerator)new MMDeviceEnumerator()).GetDefaultAudioEndpoint(0, 1, out device));
    IAudioEndpointVolume volume;
    Guid id = typeof(IAudioEndpointVolume).GUID;
    Marshal.ThrowExceptionForHR(device.Activate(ref id, 23, 0, out volume));
    return volume;
  }
  public static bool Muted() {
    bool mute;
    Marshal.ThrowExceptionForHR(Endpoint().GetMute(out mute));
    return mute;
  }
  public static void Mute(bool mute) { Marshal.ThrowExceptionForHR(Endpoint().SetMute(mute, IntPtr.Zero)); }
  public static int Level() {
    if (Muted()) { return 0; }
    float level;
    Marshal.ThrowExceptionForHR(Endpoint().GetMasterVolumeLevelScalar(out level));
    return (int)Math.Round(level * 100);
  }
}
'@ }
"#;

/// Prints "volume brightness" ten times a second, after `WINDOWS_AUDIO`; -1 for one it can't read.
#[cfg(windows)]
const WINDOWS_LEVELS: &str = r#"
while ($true) {
  $v = try { [RippleAudio]::Level() } catch { -1 }
  # WMI is slow: brightness every third pass; -1 keeps the last level.
  $i++
  $b = if ($i % 3) { -1 } else { try { [int](Get-CimInstance -Namespace root/wmi -ClassName WmiMonitorBrightness -ErrorAction Stop | Select-Object -First 1).CurrentBrightness } catch { -1 } }
  # Ripple closed the pipe: quit rather than poll on as an orphan.
  try { [Console]::Out.WriteLine("$v $b"); [Console]::Out.Flush() } catch { exit }
  Start-Sleep -Milliseconds 100
}
"#;

/// "volume brightness" from `WINDOWS_LEVELS`, or "volume" from `MAC_LEVELS`; anything outside 0-100 is None.
#[cfg_attr(not(any(windows, target_os = "macos", test)), allow(dead_code))]
fn parse_levels(line: &str) -> (Option<u8>, Option<u8>) {
    let mut parts = line.split_whitespace().map(|p| p.parse::<u8>().ok().filter(|p| *p <= 100));
    (parts.next().flatten(), parts.next().flatten())
}

#[cfg(target_os = "linux")]
fn volume() -> Option<u8> {
    output("wpctl", &["get-volume", "@DEFAULT_AUDIO_SINK@"]).and_then(|o| parse_wpctl(&o)).or_else(|| {
        if output("pactl", &["get-sink-mute", "@DEFAULT_SINK@"])?.contains("yes") { return Some(0) }
        first_percent(&output("pactl", &["get-sink-volume", "@DEFAULT_SINK@"])?).map(|p| p.min(100))
    })
}

/// `wpctl get-volume`: "Volume: 0.45", with " [MUTED]" when muted; it can pass 1.0.
#[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
fn parse_wpctl(out: &str) -> Option<u8> {
    let level: f32 = out.strip_prefix("Volume:")?.split_whitespace().next()?.parse().ok()?;
    Some(if out.contains("[MUTED]") { 0 } else { (level * 100.0).round().clamp(0.0, 100.0) as u8 })
}

/// The first backlight, as a percent of its maximum.
#[cfg(target_os = "linux")]
fn brightness() -> Option<u8> {
    let dir = std::fs::read_dir("/sys/class/backlight").ok()?.flatten().next()?.path();
    let read = |name: &str| std::fs::read_to_string(dir.join(name)).ok()?.trim().parse::<f32>().ok();
    let (now, max) = (read("brightness")?, read("max_brightness")?);
    (max > 0.0).then(|| (now / max * 100.0).round().clamp(0.0, 100.0) as u8)
}


// --- Quick toggles and power actions ---

/// A setting the Controls tab switches on and off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Toggle { Wifi, Bluetooth, Mute }

impl Toggle {
    pub const ALL: [Toggle; 3] = [Toggle::Wifi, Toggle::Bluetooth, Toggle::Mute];
}

/// Whether a toggle is on; None where the OS (or a missing tool) can't say, so it can't be switched either.
pub fn toggle(t: Toggle) -> Option<bool> {
    #[cfg(target_os = "linux")]
    {
        match t {
            Toggle::Wifi => output("nmcli", &["radio", "wifi"]).map(|o| o == "enabled"),
            Toggle::Bluetooth => output("bluetoothctl", &["show"]).filter(|o| o.contains("Powered:")).map(|o| o.contains("Powered: yes")),
            Toggle::Mute => output("wpctl", &["get-volume", "@DEFAULT_AUDIO_SINK@"]).map(|o| o.contains("[MUTED]"))
                .or_else(|| output("pactl", &["get-sink-mute", "@DEFAULT_SINK@"]).map(|o| o.contains("yes"))),
        }
    }
    #[cfg(target_os = "macos")]
    {
        match t {
            // shortcut: assumes Wi-Fi is en0, as on every Mac without extra adapters; look it up with -listallhardwareports if users ask.
            Toggle::Wifi => output("networksetup", &["-getairportpower", "en0"]).map(|o| o.ends_with("On")),
            // macOS has no Bluetooth CLI of its own: blueutil (Homebrew) when installed.
            Toggle::Bluetooth => output("blueutil", &["-p"]).map(|o| o == "1"),
            Toggle::Mute => output("osascript", &["-e", "output muted of (get volume settings)"]).map(|o| o == "true"),
        }
    }
    #[cfg(windows)]
    {
        let out = match t {
            Toggle::Wifi | Toggle::Bluetooth => powershell_radios(&format!("{WINDOWS_RADIOS}(@($radios | Where-Object {{ \"$($_.Kind)\" -eq '{}' }}) | Select-Object -First 1).State", radio_kind(t))),
            Toggle::Mute => powershell(&format!("{WINDOWS_AUDIO}[RippleAudio]::Muted()")),
        };
        match out.as_str() { "On" | "True" => Some(true), "Off" | "False" => Some(false), _ => None }
    }
}

/// Switches a toggle. Blocks until the OS has it; call `toggle` after to see what took.
pub fn set_toggle(t: Toggle, on: bool) {
    #[cfg(target_os = "linux")]
    {
        let (word, bit) = (if on { "on" } else { "off" }, if on { "1" } else { "0" });
        match t {
            Toggle::Wifi => { output("nmcli", &["radio", "wifi", word]); }
            Toggle::Bluetooth => { output("bluetoothctl", &["power", word]); }
            Toggle::Mute => if output("wpctl", &["set-mute", "@DEFAULT_AUDIO_SINK@", bit]).is_none() { output("pactl", &["set-sink-mute", "@DEFAULT_SINK@", bit]); },
        }
    }
    #[cfg(target_os = "macos")]
    {
        match t {
            Toggle::Wifi => { output("networksetup", &["-setairportpower", "en0", if on { "on" } else { "off" }]); }
            Toggle::Bluetooth => { output("blueutil", &["-p", if on { "1" } else { "0" }]); }
            Toggle::Mute => { output("osascript", &["-e", &format!("set volume output muted {on}")]); }
        }
    }
    #[cfg(windows)]
    {
        let _ = match t {
            Toggle::Wifi | Toggle::Bluetooth => powershell_radios(&format!("{WINDOWS_RADIOS}foreach ($r in @($radios | Where-Object {{ \"$($_.Kind)\" -eq '{}' }})) {{ Await ($r.SetStateAsync('{}')) ([Windows.Devices.Radios.RadioAccessStatus]) | Out-Null }}", radio_kind(t), if on { "On" } else { "Off" })),
            Toggle::Mute => powershell(&format!("{WINDOWS_AUDIO}[RippleAudio]::Mute(${on})")),
        };
    }
}

/// WinRT radio calls get a process of their own, so a slow one doesn't hold up the shared pollers.
#[cfg(windows)]
fn powershell_radios(script: &str) -> String {
    static SERVER: PsSlot = std::sync::Mutex::new(None);
    powershell_in(&SERVER, script)
}

#[cfg(windows)]
fn radio_kind(t: Toggle) -> &'static str { if t == Toggle::Wifi { "WiFi" } else { "Bluetooth" } }

/// `$radios`: the PC's Wi-Fi and Bluetooth radios, through WinRT, which switches them without admin rights.
#[cfg(windows)]
const WINDOWS_RADIOS: &str = r#"
Add-Type -AssemblyName System.Runtime.WindowsRuntime
$asTask = [System.WindowsRuntimeSystemExtensions].GetMethods() | Where-Object { $_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncOperation`1' } | Select-Object -First 1
function Await($op, [Type]$type) { $t = $asTask.MakeGenericMethod($type).Invoke($null, @($op)); $t.Wait(-1) | Out-Null; $t.Result }
[Windows.Devices.Radios.Radio, Windows.System.Devices, ContentType = WindowsRuntime] | Out-Null
Await ([Windows.Devices.Radios.Radio]::RequestAccessAsync()) ([Windows.Devices.Radios.RadioAccessStatus]) | Out-Null
$radios = Await ([Windows.Devices.Radios.Radio]::GetRadiosAsync()) ([System.Collections.Generic.IReadOnlyList[Windows.Devices.Radios.Radio]])
"#;

/// What the Controls tab's power buttons do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Power { Lock, Sleep, Restart, ShutDown }

/// Locks, sleeps, restarts or shuts down the computer. Doesn't wait for it.
pub fn power(p: Power) {
    #[cfg(target_os = "linux")]
    let (program, args): (&str, &[&str]) = match p {
        Power::Lock => ("loginctl", &["lock-session"]),
        Power::Sleep => ("systemctl", &["suspend"]),
        Power::Restart => ("systemctl", &["reboot"]),
        Power::ShutDown => ("systemctl", &["poweroff"]),
    };
    // System Events asks each app to quit, so unsaved work gets its prompt.
    #[cfg(target_os = "macos")]
    let (program, args): (&str, &[&str]) = match p {
        // Sleeps the display, which locks when a password is asked on wake (macOS's default).
        Power::Lock => ("pmset", &["displaysleepnow"]),
        Power::Sleep => ("pmset", &["sleepnow"]),
        Power::Restart => ("osascript", &["-e", "tell application \"System Events\" to restart"]),
        Power::ShutDown => ("osascript", &["-e", "tell application \"System Events\" to shut down"]),
    };
    #[cfg(windows)]
    let (program, args): (&str, &[&str]) = match p {
        Power::Lock => ("rundll32.exe", &["user32.dll,LockWorkStation"]),
        Power::Sleep => ("powershell", &["-NoProfile", "-Command", "Add-Type -AssemblyName System.Windows.Forms; [System.Windows.Forms.Application]::SetSuspendState('Suspend', $false, $false)"]),
        Power::Restart => ("shutdown", &["/r", "/t", "0"]),
        Power::ShutDown => ("shutdown", &["/s", "/t", "0"]),
    };
    let mut cmd = command(program);
    cmd.args(args);
    spawn(cmd);
}

// --- Caps Lock ---

/// Whether Caps Lock is on; None where it can't be read. Cheap enough to poll often.
pub fn caps_lock() -> Option<bool> {
    #[cfg(target_os = "linux")]
    {
        // Each keyboard's Caps Lock LED, which the kernel keeps in step on X11 and Wayland alike.
        let leds: Vec<_> = std::fs::read_dir("/sys/class/leds").ok()?.flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with("::capslock")).collect();
        if leds.is_empty() { return None }
        Some(leds.iter().any(|e| std::fs::read_to_string(e.path().join("brightness")).is_ok_and(|b| b.trim() != "0")))
    }
    #[cfg(target_os = "macos")]
    {
        #[link(name = "CoreGraphics", kind = "framework")]
        unsafe extern "C" { fn CGEventSourceFlagsState(state: i32) -> u64; }
        // SAFETY: a pure query. 0 is kCGEventSourceStateCombinedSessionState; 0x10000 kCGEventFlagMaskAlphaShift.
        Some(unsafe { CGEventSourceFlagsState(0) } & 0x10000 != 0)
    }
    #[cfg(windows)]
    {
        #[link(name = "user32")]
        unsafe extern "system" {
            fn GetKeyState(key: i32) -> i16;
            fn GetForegroundWindow() -> isize;
            fn GetWindowThreadProcessId(window: isize, process: *mut u32) -> u32;
            fn AttachThreadInput(from: u32, to: u32, attach: i32) -> i32;
        }
        #[link(name = "kernel32")]
        unsafe extern "system" { fn GetCurrentThreadId() -> u32; }
        // GetKeyState reads this thread's own input state, which a poller that pumps no messages
        // never updates; attaching to the focused window's thread shares its key state for the read.
        // SAFETY: plain Win32 queries; a null window gives thread 0, which is skipped.
        unsafe {
            let (me, focused) = (GetCurrentThreadId(), GetWindowThreadProcessId(GetForegroundWindow(), std::ptr::null_mut()));
            let attached = focused != 0 && focused != me && AttachThreadInput(me, focused, 1) != 0;
            let on = GetKeyState(0x14) & 1 != 0; // 0x14 is VK_CAPITAL; the low bit is its toggle
            if attached { AttachThreadInput(me, focused, 0); }
            Some(on)
        }
    }
}

// --- Battery ---

/// (percent, plugged in), or None on a machine without a battery.
#[cfg(target_os = "linux")]
pub fn battery() -> Option<(u8, bool)> {
    let read = |path: std::path::PathBuf| std::fs::read_to_string(path).ok().map(|s| s.trim().to_string());
    let supplies: Vec<_> = std::fs::read_dir("/sys/class/power_supply").ok()?.flatten().map(|e| e.path()).collect();
    let battery = supplies.iter().find(|p| {
        // A mouse or headset battery reports scope "Device".
        read(p.join("type")).as_deref() == Some("Battery") && read(p.join("scope")).as_deref() != Some("Device")
    })?;
    let percent: u8 = read(battery.join("capacity"))?.parse().ok()?;
    let mains = supplies.iter().any(|p| read(p.join("type")).as_deref() == Some("Mains") && read(p.join("online")).as_deref() == Some("1"));
    let status = read(battery.join("status")).unwrap_or_default();
    Some((percent.min(100), mains || status == "Charging" || status == "Full"))
}

#[cfg(target_os = "macos")]
pub fn battery() -> Option<(u8, bool)> {
    parse_pmset(&output("pmset", &["-g", "batt"])?)
}

/// `pmset -g batt`: "Now drawing from 'AC Power'" then " -InternalBattery-0 (id=…)\t87%; charging; …".
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
fn parse_pmset(out: &str) -> Option<(u8, bool)> {
    Some((first_percent(out)?.min(100), out.contains("AC Power")))
}

#[cfg_attr(not(any(target_os = "macos", target_os = "linux", test)), allow(dead_code))]
fn first_percent(out: &str) -> Option<u8> {
    let end = out.find('%')?;
    let start = out[..end].rfind(|c: char| !c.is_ascii_digit()).map_or(0, |i| i + 1);
    out[start..end].parse().ok()
}

#[cfg(windows)]
pub fn battery() -> Option<(u8, bool)> {
    #[repr(C)]
    #[derive(Default)]
    struct SystemPowerStatus { ac_line: u8, battery_flag: u8, percent: u8, status_flag: u8, life_time: u32, full_life_time: u32 }
    #[link(name = "kernel32")]
    unsafe extern "system" { fn GetSystemPowerStatus(status: *mut SystemPowerStatus) -> i32; }
    let mut status = SystemPowerStatus::default();
    // SAFETY: the struct matches SYSTEM_POWER_STATUS and outlives the call.
    if unsafe { GetSystemPowerStatus(&mut status) } == 0 { return None }
    // Flag 128: no system battery. 255: unknown.
    if status.battery_flag & 128 != 0 || status.percent > 100 { return None }
    Some((status.percent, status.ac_line == 1))
}

// --- System monitor ---

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stats {
    /// Percent of all cores.
    pub cpu: f32,
    pub mem_used: u64,
    pub mem_total: u64,
    /// The disk that holds the home folder.
    pub disk_free: u64,
    pub disk_total: u64,
    /// Bytes per second on the busiest interface.
    pub down: u64,
    pub up: u64,
    /// The apps using the most CPU (percent of all cores) and memory (bytes), processes of the
    /// same name added up; empty unless sampled with `processes`.
    pub top_cpu: Vec<(String, f32)>,
    pub top_mem: Vec<(String, u64)>,
}

/// The `n` largest of each name's summed values.
fn top<T: Copy + PartialOrd + std::ops::Add<Output = T>>(values: impl Iterator<Item = (String, T)>, n: usize) -> Vec<(String, T)> {
    let mut sums: Vec<(String, T)> = Vec::new();
    for (name, v) in values {
        match sums.iter_mut().find(|s| s.0 == name) { Some(s) => s.1 = s.1 + v, None => sums.push((name, v)) }
    }
    sums.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    sums.truncate(n);
    sums
}

/// Keeps the previous sample: CPU load and network rates are deltas between two.
pub struct Monitor {
    sys: sysinfo::System,
    disks: sysinfo::Disks,
    nets: sysinfo::Networks,
    at: std::time::Instant,
}

impl Monitor {
    pub fn new() -> Monitor {
        let mut sys = sysinfo::System::new();
        sys.refresh_cpu_usage();
        Monitor { sys, disks: sysinfo::Disks::new_with_refreshed_list(), nets: sysinfo::Networks::new_with_refreshed_list(), at: std::time::Instant::now() }
    }

    /// `processes` reads every process too, which costs more than the totals.
    pub fn sample(&mut self, processes: bool) -> Stats {
        self.sys.refresh_cpu_usage();
        self.sys.refresh_memory();
        // Re-list, so a mounted or unmounted disk is noticed.
        self.disks.refresh(true);
        self.nets.refresh(true);
        let secs = std::mem::replace(&mut self.at, std::time::Instant::now()).elapsed().as_secs_f64().max(0.001);
        let home = rune::dirs::home().unwrap_or_else(|| "/".into());
        // macOS firmlinks /Users from the Data volume; "/" is the small read-only system volume.
        #[cfg(target_os = "macos")]
        let home = std::path::Path::new("/System/Volumes/Data").join(home.strip_prefix("/").unwrap_or(&home));
        let disks = self.disks.list();
        let disk = disks.iter().filter(|d| home.starts_with(d.mount_point())).max_by_key(|d| d.mount_point().as_os_str().len())
            .or_else(|| disks.iter().max_by_key(|d| d.total_space()));
        // VPN, container and Hyper-V layers repeat the physical NIC's bytes, so a sum counts them twice.
        // shortcut: two NICs busy at once (Wi-Fi and Ethernet) show only the busier; sum physical NICs if that matters.
        let nets = self.nets.list().iter().filter(|(name, _)| !(*name == "lo" || *name == "lo0" || name.contains("Loopback"))).map(|(_, n)| n);
        let (down, up) = nets.fold((0, 0), |(d, u), n| (d.max(n.received()), u.max(n.transmitted())));
        let (mut top_cpu, mut top_mem) = (Vec::new(), Vec::new());
        if processes {
            use sysinfo::{ProcessRefreshKind, ProcessesToUpdate};
            self.sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing().with_cpu().with_memory());
            let cores = self.sys.cpus().len().max(1) as f32;
            let name = |p: &sysinfo::Process| { let n = p.name().to_string_lossy(); n.strip_suffix(".exe").unwrap_or(&n).to_string() };
            let procs = self.sys.processes().values();
            top_cpu = top(procs.clone().map(|p| (name(p), p.cpu_usage() / cores)), 3);
            top_mem = top(procs.map(|p| (name(p), p.memory())), 3);
        }
        Stats {
            cpu: self.sys.global_cpu_usage(),
            mem_used: self.sys.used_memory(),
            mem_total: self.sys.total_memory(),
            disk_free: disk.map_or(0, |d| d.available_space()),
            disk_total: disk.map_or(0, |d| d.total_space()),
            down: (down as f64 / secs) as u64,
            up: (up as f64 / secs) as u64,
            top_cpu,
            top_mem,
        }
    }
}

// --- Launch at login ---

/// The file Ripple runs from. An AppImage runs from a mount that changes each launch; $APPIMAGE is the file itself.
fn own_exe() -> Option<std::path::PathBuf> {
    std::env::var_os("APPIMAGE").map(std::path::PathBuf::from).or_else(|| std::env::current_exe().ok())
}

/// Writes `text` to `file`, its folder made first, or removes the file for None.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn set_file(file: &std::path::Path, text: Option<String>) -> std::io::Result<()> {
    match text {
        Some(text) => file.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(file, text)),
        None if file.exists() => std::fs::remove_file(file),
        None => Ok(()),
    }
}

/// A `.desktop` file that runs `exe`.
#[cfg(target_os = "linux")]
fn desktop_entry(exe: &std::path::Path, comment: &str, extra: &str) -> String {
    format!("[Desktop Entry]\nType=Application\nVersion=1.0\nName=Ripple\nComment={comment}\nExec={}\nIcon=ripple\nTerminal=false\n{extra}", desktop_exec(&exe.to_string_lossy()))
}

/// `$XDG_DATA_HOME`, or its default under `home`.
#[cfg(target_os = "linux")]
fn xdg_data_home(home: &std::path::Path) -> std::path::PathBuf {
    std::env::var("XDG_DATA_HOME").ok().filter(|d| !d.is_empty()).map(std::path::PathBuf::from).unwrap_or_else(|| home.join(".local/share"))
}

/// `$XDG_DATA_DIRS`, or its default.
#[cfg(target_os = "linux")]
fn xdg_data_dirs() -> Vec<std::path::PathBuf> {
    std::env::var("XDG_DATA_DIRS").ok().filter(|d| !d.is_empty()).unwrap_or_else(|| "/usr/local/share:/usr/share".into()).split(':').map(Into::into).collect()
}

pub fn set_auto_launch(enable: bool) {
    let Some(exe) = own_exe() else { return };
    #[cfg(target_os = "linux")]
    {
        let Some(file) = rune::dirs::home().map(|h| h.join(".config/autostart/ripple.desktop")) else { return };
        let entry = enable.then(|| desktop_entry(&exe, "Ripple Desktop Assistant", ""));
        if let Err(e) = set_file(&file, entry) { eprintln!("Ripple: failed to set launch at login: {e}") }
    }
    #[cfg(windows)]
    {
        const RUN: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
        let value = format!("\"{}\"", exe.display());
        if enable { output("reg", &["add", RUN, "/v", "Ripple", "/t", "REG_SZ", "/d", &value, "/f"]); }
        else { output("reg", &["delete", RUN, "/v", "Ripple", "/f"]); }
    }
    // A per-user LaunchAgent that starts the binary once at login; launchd ignores it once removed.
    #[cfg(target_os = "macos")]
    {
        let Some(file) = rune::dirs::home().map(|h| h.join("Library/LaunchAgents/com.topmyster.ripple.plist")) else { return };
        if let Err(e) = set_file(&file, enable.then(|| launch_agent(&exe.to_string_lossy()))) { eprintln!("Ripple: failed to set launch at login: {e}") }
    }
}

/// The LaunchAgent plist that runs `exe` at login, with the path escaped for XML.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
fn launch_agent(exe: &str) -> String {
    let exe = exe.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    format!(r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>com.topmyster.ripple</string>
  <key>ProgramArguments</key><array><string>{exe}</string></array>
  <key>RunAtLoad</key><true/>
</dict></plist>
"#)
}

/// A path as one quoted Exec argument: `"`, `` ` ``, `$` and `\` take a backslash, `%` doubles,
/// then every backslash doubles again, since Exec is itself an escaped string.
#[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
fn desktop_exec(path: &str) -> String {
    let quoted: String = path.chars().flat_map(|c| match c {
        '"' | '`' | '$' | '\\' => vec!['\\', c],
        '%' => vec!['%', '%'],
        c => vec![c],
    }).collect();
    format!("\"{}\"", quoted.replace('\\', "\\\\"))
}

// --- Opening links and apps ---

/// Opens a web page or a document in its default app. `file:` is allowed for documents but
/// not for anything that would execute.
pub fn open_external(url: &str) {
    if !openable(url) { return }
    let mut cmd = opener();
    cmd.arg(url);
    spawn(cmd);
}

/// The OS's "open in the default app" command, waiting for its argument.
fn opener() -> Command {
    #[cfg(target_os = "linux")]
    let cmd = command("xdg-open");
    #[cfg(target_os = "macos")]
    let cmd = command("open");
    #[cfg(windows)]
    let cmd = {
        // No shell: `&` in a query string stays part of the URL.
        let mut cmd = command("rundll32");
        cmd.arg("url.dll,FileProtocolHandler");
        cmd
    };
    cmd
}

/// Opens a document in its default app; programs are refused, as `open_external` refuses them.
pub fn open_file(path: &std::path::Path) {
    // The real name, not a URL's: a `?` or `#` in it must not hide the extension.
    if runs_as_program(&path.to_string_lossy()) { return }
    let mut cmd = opener();
    cmd.arg(path);
    spawn(cmd);
}

/// Shows a file in its folder: selected in Finder and Explorer, the folder itself elsewhere.
pub fn reveal(path: &std::path::Path) {
    #[cfg(target_os = "linux")]
    let cmd = { let mut cmd = command("xdg-open"); cmd.arg(path.parent().unwrap_or(path)); cmd };
    #[cfg(target_os = "macos")]
    let cmd = { let mut cmd = command("open"); cmd.arg("-R").arg(path); cmd };
    #[cfg(windows)]
    let cmd = {
        use std::os::windows::process::CommandExt;
        // Explorer parses `/select,"path"` itself; std's quoting of the whole argument confuses it.
        let mut cmd = command("explorer");
        cmd.raw_arg(format!("/select,\"{}\"", path.display()));
        cmd
    };
    spawn(cmd);
}

/// Web pages, and `file:` documents that wouldn't run as programs.
pub(crate) fn openable(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") { return true }
    let Some(path) = lower.strip_prefix("file://") else { return false };
    // The name the OS opens: decoded as the file-URL handler decodes it, without its query,
    // fragment or trailing slash (a macOS bundle is a folder), and without the trailing dots
    // and spaces Windows drops (`x.exe.` is `x.exe`).
    let path = crate::net::percent_decode(path.split(['?', '#']).next().unwrap_or_default());
    let path = path.trim_end_matches(['/', '\\', '.', ' ']);
    !path.is_empty() && !runs_as_program(path)
}

/// Names the OS would run rather than open, by extension.
fn runs_as_program(path: &str) -> bool {
    const EXECUTABLE: [&str; 58] = [".exe", ".bat", ".cmd", ".com", ".ps1", ".msi", ".scr", ".lnk", ".sh", ".app", ".desktop", ".jar",
        ".vbs", ".vbe", ".js", ".jse", ".wsf", ".hta", ".reg", ".command", ".tool", ".pkg", ".appimage", ".run", ".py", ".pl",
        ".cpl", ".msc", ".pif", ".terminal", ".application", ".appref-ms", ".url", ".scf", ".ws", ".msix", ".appx", ".msixbundle", ".appxbundle", ".jnlp", ".mpkg", ".msp", ".sct", ".wsc", ".msu", ".diagcab",
        // Windows shell handlers that open as code or redirect: help, setup, search and settings files, Office add-ins.
        ".website", ".chm", ".inf", ".library-ms", ".search-ms", ".searchconnector-ms", ".settingcontent-ms",
        ".xll", ".xla", ".xlam", ".ppa", ".ppam"];
    let path = path.trim_end_matches(['/', '\\', '.', ' ']).to_ascii_lowercase();
    EXECUTABLE.iter().any(|ext| path.ends_with(ext))
}

/// Starts an app by name or command line. No shell: the text is split into argv, so it
/// can't chain commands.
pub fn launch_app(name: &str) {
    #[cfg(target_os = "linux")]
    {
        let mut argv = tokenize_args(name).into_iter();
        let Some(program) = argv.next() else { return };
        let mut cmd = command(&program);
        cmd.args(argv);
        spawn(cmd);
    }
    #[cfg(target_os = "macos")]
    {
        let mut cmd = command("open");
        cmd.args(["-a", name]);
        spawn(cmd);
    }
    #[cfg(windows)]
    launch_windows(name);
}

#[cfg(windows)]
fn launch_windows(input: &str) {
    let trimmed = input.trim();
    let has_separator = |s: &str| s.contains(['\\', '/']);
    // UWP apps and schemes, App Paths names ("spotify") and bare executables.
    if trimmed.starts_with("shell:") || !has_separator(trimmed) { return start_process(trimmed) }

    let (exe, args) = crate::command::parse_command(trimmed);
    if args.is_empty() {
        if exe.to_lowercase().ends_with(".url") {
            let content = std::fs::read_to_string(&exe).unwrap_or_default();
            if let Some(url) = content.lines().find_map(|l| l.trim().strip_prefix("URL=").or_else(|| l.trim().strip_prefix("url="))) { open_external(url.trim()) }
            return;
        }
        // The shell's own "open": documents, shortcuts and executables alike.
        return start_process(&exe);
    }

    // Arguments given: spawn exactly, so nothing in them is interpreted.
    let file = exe.rsplit(['\\', '/']).next().unwrap_or(&exe);
    let exe = if file.contains('.') { exe.clone() } else { format!("{exe}.exe") };
    let mut cmd = if exe.to_lowercase().ends_with(".ps1") {
        let mut cmd = command("powershell.exe");
        cmd.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", &exe]);
        cmd
    } else {
        // std runs .cmd and .bat through cmd.exe itself, quoting their arguments safely, and
        // refuses arguments it can't quote (`%`, `"`); `spawn` logs that refusal.
        command(&exe)
    };
    cmd.args(args);
    spawn(cmd);
}

/// Opens what a quick app or a workflow entry names: a link, a path, a host or an app.
pub fn open_app(app: &str) {
    let app = app.trim();
    if app.is_empty() { return }
    match classify(app) {
        Target::Url => open_external(app),
        Target::Http => open_external(&format!("http://{app}")),
        Target::Https => open_external(&format!("https://{app}")),
        Target::App => launch_app(app),
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Target { Url, Http, Https, App }

fn classify(app: &str) -> Target {
    let lower = app.to_ascii_lowercase();
    if ["http://", "https://", "file://"].iter().any(|p| lower.starts_with(p)) { return Target::Url }
    if app.contains(['\\', '/']) || lower.ends_with(".exe") || app.starts_with("shell:") { return Target::App }
    // An IPv4 address or localhost: dev servers rarely run https.
    let host = app.split(':').next().unwrap_or(app);
    let port_ok = app.split_once(':').is_none_or(|(_, p)| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
    let ipv4 = host.split('.').count() == 4 && host.split('.').all(|o| (1..=3).contains(&o.len()) && o.bytes().all(|b| b.is_ascii_digit()));
    if port_ok && (ipv4 || host.eq_ignore_ascii_case("localhost")) { return Target::Http }
    // A bare domain must end in two or more letters, so python3.11 isn't one.
    if let Some((name, tld)) = app.rsplit_once('.')
        && !name.is_empty()
        && tld.len() >= 2
        && tld.bytes().all(|b| b.is_ascii_alphabetic())
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
    {
        return Target::Https;
    }
    Target::App
}

pub fn open_music_player(source: &str) {
    if source.is_empty() { return }
    if source.contains("Apple") { open_app("Music") } else { open_app(source) }
}

/// Writes a hidden `.desktop` file named for the app id, unless a package installed one: a
/// Wayland portal serves GlobalShortcuts (the hotkey) only to an app id naming an installed one.
#[cfg(target_os = "linux")]
pub fn install_desktop_entry(app_id: &str) {
    let name = format!("{app_id}.desktop");
    if xdg_data_dirs().iter().any(|d| d.join("applications").join(&name).exists()) { return }
    let (Some(exe), Some(home)) = (own_exe(), rune::dirs::home()) else { return };
    let file = xdg_data_home(&home).join("applications").join(name);
    let entry = desktop_entry(&exe, "A Dynamic Island for All", "NoDisplay=true\n");
    if std::fs::read_to_string(&file).is_ok_and(|old| old == entry) { return }
    if let Err(e) = set_file(&file, Some(entry)) { eprintln!("Ripple: failed to write {}: {e}", file.display()) }
}

// --- Installed apps (Search results; on Windows also suggestions for the quick apps field) ---

/// `.desktop` files in the XDG applications folders, as (Name, Exec without field codes).
#[cfg(target_os = "linux")]
pub fn discover_apps() -> Vec<QuickApp> {
    let data_home = xdg_data_home(&rune::dirs::home().unwrap_or_default());
    let dirs = std::iter::once(data_home.clone())
        .chain(xdg_data_dirs())
        .chain([data_home.join("flatpak/exports/share"), "/var/lib/flatpak/exports/share".into()]);
    let mut seen = std::collections::HashSet::new();
    let mut apps = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir.join("applications")) else { continue };
        for entry in entries.flatten() {
            // The first folder wins for a desktop-file id, as XDG says.
            if !seen.insert(entry.file_name()) { continue }
            if let Some(app) = std::fs::read_to_string(entry.path()).ok().and_then(|t| parse_desktop(&t)) { apps.push(app) }
        }
    }
    apps.sort_by_key(|a| a.name.to_lowercase());
    apps.dedup_by(|a, b| a.name == b.name && a.launch == b.launch);
    apps
}

/// One `.desktop` file's app, or None when it is hidden or not an app.
#[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
fn parse_desktop(text: &str) -> Option<QuickApp> {
    let (mut name, mut exec, mut app, mut in_entry) = (None, None, false, false);
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') { in_entry = line == "[Desktop Entry]"; continue }
        if !in_entry { continue }
        let Some((k, v)) = line.split_once('=') else { continue };
        match (k.trim(), v.trim()) {
            ("Name", v) => name = Some(v.to_string()),
            ("Exec", v) => exec = Some(v.to_string()),
            ("Type", v) => app = v == "Application",
            // Terminal apps would start with no terminal to show them.
            ("NoDisplay" | "Hidden" | "Terminal", "true") => return None,
            _ => {}
        }
    }
    // %f, %U and the like are filled in by a launcher with files; there are none here.
    let exec = tokenize_args(&exec?).into_iter().filter(|a| !(a.len() == 2 && a.starts_with('%'))).collect::<Vec<_>>();
    if !app || exec.is_empty() { return None }
    // Back to one string `launch_app` splits again: tokens with spaces stay quoted.
    let launch = exec.iter().map(|a| if a.contains(char::is_whitespace) { format!("\"{a}\"") } else { a.clone() }).collect::<Vec<_>>().join(" ");
    Some(QuickApp { name: name?, launch })
}

// --- File search: the OS's own index ---

/// Files and folders whose name (or, where the index does, content) matches `query`, from
/// Spotlight, Windows Search, GNOME's LocalSearch/Tracker or KDE's Baloo. Empty when there's no index.
#[cfg(target_os = "linux")]
pub fn find_files(query: &str, limit: usize) -> Vec<std::path::PathBuf> {
    let n = limit.to_string();
    let kde = std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|d| d.to_ascii_uppercase().contains("KDE"));
    // `--` keeps a query that starts with `-` from reading as an option.
    let baloo = || ["baloosearch6", "baloosearch"].iter().find_map(|b| output(b, &["-l", &n, "--", query])).map(|o| baloo_paths(&o));
    let tracker = || ["localsearch", "tracker3"].iter().find_map(|t| output(t, &["search", "-f", "-l", &n, "--", query])).map(|o| tracker_paths(&o));
    let found = if kde { baloo().or_else(tracker) } else { tracker().or_else(baloo) };
    found.unwrap_or_default().into_iter().take(limit).collect()
}

/// `baloosearch` prints one path a line, then how long it took.
#[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
fn baloo_paths(out: &str) -> Vec<std::path::PathBuf> {
    out.lines().map(str::trim).filter(|l| l.starts_with('/')).map(Into::into).collect()
}

/// `localsearch search` prints a heading, then indented `file://` URIs.
#[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
fn tracker_paths(out: &str) -> Vec<std::path::PathBuf> {
    out.lines().filter_map(|l| l.trim().strip_prefix("file://")).map(|p| crate::net::percent_decode(p).into()).collect()
}

#[cfg(target_os = "macos")]
pub fn find_files(query: &str, limit: usize) -> Vec<std::path::PathBuf> {
    let home = rune::dirs::home().unwrap_or_else(|| "/".into());
    let out = output("mdfind", &["-onlyin", &home.to_string_lossy(), "-name", query]).unwrap_or_default();
    out.lines().filter(|l| !l.is_empty()).take(limit).map(Into::into).collect()
}

#[cfg(windows)]
pub fn find_files(query: &str, limit: usize) -> Vec<std::path::PathBuf> {
    // Its own process: a slow index query mustn't hold up the pollers' shared one.
    static SERVER: PsSlot = std::sync::Mutex::new(None);
    // The query is a PowerShell literal, then a SQL literal with LIKE's wildcards bracketed.
    let script = format!(r#"
$q = '{}' -replace '([\[%_])', '[$1]' -replace "'", "''"
$c = New-Object -ComObject ADODB.Connection
$c.Open("Provider=Search.CollatorDSO;Extended Properties='Application=Windows';")
$r = $c.Execute("SELECT TOP {limit} System.ItemPathDisplay FROM SYSTEMINDEX WHERE SCOPE='file:' AND System.FileName LIKE '%" + $q + "%'")
while (-not $r.EOF) {{ $r.Fields.Item('System.ItemPathDisplay').Value; $r.MoveNext() }}
$c.Close()
"#, ps_quote(query));
    powershell_in(&SERVER, &script).lines().filter(|l| !l.trim().is_empty()).map(|l| l.trim().into()).collect()
}

/// `.app` bundles in the Applications folders; `open -a` takes the name.
#[cfg(target_os = "macos")]
pub fn discover_apps() -> Vec<QuickApp> {
    let home = rune::dirs::home().unwrap_or_default();
    let dirs = ["/Applications".into(), "/Applications/Utilities".into(), "/System/Applications".into(), "/System/Applications/Utilities".into(), home.join("Applications")];
    let mut apps: Vec<QuickApp> = dirs.iter().filter_map(|d: &std::path::PathBuf| std::fs::read_dir(d).ok()).flatten().flatten()
        .filter_map(|e| e.file_name().to_str()?.strip_suffix(".app").map(|n| QuickApp { name: n.into(), launch: n.into() }))
        .collect();
    apps.sort_by_key(|a| a.name.to_lowercase());
    apps.dedup_by(|a, b| a.name == b.name);
    apps
}


/// Start Menu shortcuts and Store apps as (name, launch target), sorted by name.
#[cfg(windows)]
pub fn discover_apps() -> Vec<QuickApp> {
    const START_MENU: &str = r#"
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$shell   = New-Object -ComObject WScript.Shell
$dirs    = @("$env:ProgramData\Microsoft\Windows\Start Menu\Programs","$env:APPDATA\Microsoft\Windows\Start Menu\Programs")
$results = [System.Collections.Generic.List[object]]::new()
foreach ($dir in $dirs) {
  if (-not (Test-Path $dir)) { continue }
  Get-ChildItem $dir -Recurse -Filter '*.lnk' -EA SilentlyContinue | ForEach-Object {
    try {
      $target = $shell.CreateShortcut($_.FullName).TargetPath
      if ($target -and $target.EndsWith('.exe') -and
          $target -notlike '*\explorer.exe' -and
          $target -notmatch 'WindowsApps' -and
          (Test-Path $target -EA SilentlyContinue)) {
        $results.Add([PSCustomObject]@{ name = $_.BaseName; launch = $target })
      }
    } catch {}
  }
}
@($results) | ConvertTo-Json -Compress -Depth 2
"#;
    // Store apps have an AppID of the form PackageFamilyName!AppId.
    const STORE: &str = r#"
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$results = [System.Collections.Generic.List[object]]::new()
Get-StartApps -EA SilentlyContinue | ForEach-Object {
  if ($_.AppID -match '.+!.+') {
    $results.Add([PSCustomObject]@{ name = $_.Name; launch = 'shell:AppsFolder\' + $_.AppID })
  }
}
@($results) | ConvertTo-Json -Compress -Depth 2
"#;
    let list = |script: &str| -> Vec<QuickApp> {
        // ConvertTo-Json writes a lone result as an object, not an array.
        // Its own process: these scans take seconds and would hold up the pollers' shared one.
        let out = output("powershell", &["-NoProfile", "-EncodedCommand", &ps_encode(script)]).unwrap_or_default();
        match serde_json::from_str::<serde_json::Value>(&out) {
            Ok(serde_json::Value::Array(items)) => items.into_iter().filter_map(|v| serde_json::from_value(v).ok()).collect(),
            Ok(v) => serde_json::from_value(v).ok().into_iter().collect(),
            Err(_) => Vec::new(),
        }
    };
    let mut seen = std::collections::HashSet::new();
    let mut apps: Vec<QuickApp> = list(START_MENU).into_iter().chain(list(STORE))
        .filter(|a| !a.name.is_empty() && !a.launch.is_empty() && seen.insert(a.launch.to_lowercase()))
        .collect();
    apps.sort_by_key(|a| a.name.to_lowercase());
    apps
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_adds_up_processes_by_name_and_keeps_the_largest() {
        let procs = [("a", 1.0), ("b", 5.0), ("a", 4.5), ("c", 0.5), ("d", 2.0)].map(|(n, v)| (n.to_string(), v));
        assert_eq!(top(procs.into_iter(), 2), [("a".to_string(), 5.5), ("b".to_string(), 5.0)]);
    }

    #[cfg(windows)]
    #[test]
    fn powershell_scripts_share_one_process_and_keep_their_lines() {
        assert_eq!(powershell("1 + 1"), "2");
        assert_eq!(powershell("$a = 'é'\nreturn @($a, 'b')"), "é\nb");
        assert_eq!(powershell("throw 'x'"), "");
        assert_eq!(powershell("@(1).Count -gt 0"), "True");
    }

    #[test]
    fn volume_and_brightness_read_as_percents() {
        assert_eq!(parse_wpctl("Volume: 0.45"), Some(45));
        assert_eq!(parse_wpctl("Volume: 0.45 [MUTED]"), Some(0));
        assert_eq!(parse_wpctl("Volume: 1.50"), Some(100));
        assert_eq!(parse_wpctl("Could not connect to PipeWire"), None);
        assert_eq!(first_percent("Volume: front-left: 29491 /  45% / -20.81 dB"), Some(45));
        assert_eq!(parse_levels("37 80"), (Some(37), Some(80)));
        assert_eq!(parse_levels("37 -1"), (Some(37), None));
        assert_eq!(parse_levels("-1 "), (None, None));
        assert_eq!(parse_levels("50"), (Some(50), None));
        assert_eq!(parse_levels("missing value"), (None, None));
    }

    #[test]
    fn progress_needs_a_length_and_stays_within_it() {
        assert_eq!(progress(Some(30.5), Some(200.0)), Some((30, 200)));
        assert_eq!(progress(Some(250.0), Some(200.0)), Some((200, 200)));
        assert_eq!(progress(None, Some(200.0)), Some((0, 200)));
        assert_eq!(progress(Some(30.0), Some(0.0)), None);
        assert_eq!(progress(Some(30.0), None), None);
    }

    #[test]
    fn desktop_files_give_visible_apps_without_field_codes() {
        let app = parse_desktop("[Desktop Entry]\nType=Application\nName=Code\nName[de]=Kode\nExec=\"/opt/my code/code\" --new %F\n[Desktop Action new]\nName=New Window\nExec=code -n");
        assert_eq!(app, Some(QuickApp { name: "Code".into(), launch: "\"/opt/my code/code\" --new".into() }));
        assert_eq!(parse_desktop("[Desktop Entry]\nType=Application\nName=X\nExec=x\nNoDisplay=true"), None);
        assert_eq!(parse_desktop("[Desktop Entry]\nType=Link\nName=X\nExec=x"), None);
        assert_eq!(parse_desktop("[Desktop Entry]\nType=Application\nName=htop\nExec=htop\nTerminal=true"), None);
    }

    #[test]
    fn index_search_output_becomes_paths() {
        assert_eq!(baloo_paths("/home/me/a b.txt\n/home/me/c\nElapsed: 1.2 msecs\n"), ["/home/me/a b.txt", "/home/me/c"].map(std::path::PathBuf::from));
        assert_eq!(tracker_paths("Files:\n  file:///home/me/a%20b.txt\n  file:///home/me/c\n"), ["/home/me/a b.txt", "/home/me/c"].map(std::path::PathBuf::from));
    }

    #[test]
    fn quick_app_entries_open_as_links_hosts_or_apps() {
        assert_eq!(classify("https://example.com/a?b=1&c=2"), Target::Url);
        assert_eq!(classify("file:///home/me/notes.pdf"), Target::Url);
        assert_eq!(classify(r"C:\Program Files\App\app.exe"), Target::App);
        assert_eq!(classify("notepad.exe"), Target::App);
        assert_eq!(classify("shell:AppsFolder\\Some.App!Id"), Target::App);
        assert_eq!(classify("192.168.1.1"), Target::Http);
        assert_eq!(classify("localhost:5173"), Target::Http);
        assert_eq!(classify("docs.google.com"), Target::Https);
        assert_eq!(classify("python3.11"), Target::App);
        assert_eq!(classify("Spotify"), Target::App);
        assert_eq!(classify("Apple Music"), Target::App);
    }

    #[test]
    fn only_web_pages_and_documents_open() {
        assert!(openable("https://example.com/a?b=1&c=2"));
        assert!(openable("file:///home/me/notes.pdf"));
        assert!(!openable("file:///x/Foo.app/"));
        assert!(!openable("file:///C:/x.exe#a"));
        assert!(!openable("file:///C:/x.exe?"));
        assert!(!openable("file:///C:/run.vbs"));
        assert!(!openable("file:///C:/x.exe."));
        assert!(!openable("file:///C:/x.exe%20"));
        assert!(!openable("file:///C:/x%2Eexe"));
        assert!(!openable("file:///Users/me/run.command"));
        assert!(!openable("file:///home/me/Tool.AppImage"));
        assert!(!openable("file:///C:/Users/me/Desktop/site.url"));
        assert!(!openable("file:///C:/x/setup.application"));
        assert!(!openable("file:///C:/x/help.chm"));
        assert!(!openable("file:///C:/x/invoice.jnlp"));
        assert!(!openable("file:///C:/x/addin.xll"));
        assert!(!openable("javascript:alert(1)"));
        assert!(!openable("file://"));
    }

    #[test]
    fn powershell_strings_double_every_quote_it_ends_on() {
        assert_eq!(ps_quote("it's"), "it''s");
        assert_eq!(ps_quote("x\u{2019}; calc; \u{2019}"), "x\u{2019}\u{2019}; calc; \u{2019}\u{2019}");
    }

    #[test]
    fn a_tool_reads_as_its_output_or_as_failed() {
        assert_eq!(output("sh", &["-c", "echo hi"]).as_deref(), Some("hi"));
        assert_eq!(output("sh", &["-c", "exit 3"]), None);
        assert_eq!(output("ripple-no-such-program", &[]), None);
    }

    /// Asks this machine's media player, battery, Bluetooth, camera and microphone, as the
    /// Island's pollers do. CI runs it on every OS with `-- --ignored probes`.
    #[test]
    #[ignore = "asks the real OS"]
    fn probes_answer_on_this_machine() {
        let started = std::time::Instant::now();
        let battery = battery();
        assert!(battery.is_none_or(|(percent, _)| percent <= 100));
        let track = media();
        let (bluetooth, camera, microphone) = (bluetooth_connected(), camera_in_use(), microphone_in_use());
        println!("battery={battery:?} media={track:?} bluetooth={bluetooth} camera={camera} microphone={microphone}");
        assert!(started.elapsed() < TIMEOUT * 5, "a hung tool must time out");
    }

    #[test]
    fn launch_at_login_paths_are_escaped_for_the_desktop_entry() {
        assert_eq!(desktop_exec("/opt/Ripple/ripple"), r#""/opt/Ripple/ripple""#);
        assert_eq!(desktop_exec(r#"/a "b"/$c`d`/100%\e"#), r#""/a \\"b\\"/\\$c\\`d\\`/100%%\\\\e""#);
    }

    #[test]
    fn the_playing_player_wins_over_idle_ones() {
        let out = "firefox||Paused||Tab||A||||\nspotify||Playing||Song||B||Album||https://i/1\nchromium||Stopped||||||||";
        assert_eq!(pick_player(out).map(|p| p[0]), Some("spotify"));
        assert_eq!(pick_player("firefox||Stopped||Tab||A||||\nvlc||Paused||Film||||||").map(|p| p[0]), Some("vlc"));
        assert_eq!(pick_player("chromium||Stopped||||||||"), None);
        assert_eq!(pick_player(""), None);
        // Two instances of one app keep apart, so controls reach the playing one.
        let two = "chromium.instance11||Paused||Idle||A||||\nchromium.instance42||Playing||Song||B||||";
        assert_eq!(pick_player(two).map(|p| p[0]), Some("chromium.instance42"));
    }

    #[test]
    fn pmset_output_gives_percent_and_power_source() {
        let ac = "Now drawing from 'AC Power'\n -InternalBattery-0 (id=4653155)\t100%; charged; 0:00 remaining present: true";
        let battery = "Now drawing from 'Battery Power'\n -InternalBattery-0 (id=4653155)\t87%; discharging; 4:12 remaining present: true";
        assert_eq!(parse_pmset(ac), Some((100, true)));
        assert_eq!(parse_pmset(battery), Some((87, false)));
        assert_eq!(parse_pmset("Now drawing from 'AC Power'"), None);
    }

    #[test]
    fn launch_agent_paths_are_escaped_for_xml() {
        let plist = launch_agent("/Applications/R&D <1>.app/Contents/MacOS/Ripple");
        assert!(plist.contains("<string>/Applications/R&amp;D &lt;1&gt;.app/Contents/MacOS/Ripple</string>"));
        assert!(plist.contains("<key>RunAtLoad</key><true/>"));
    }
}
