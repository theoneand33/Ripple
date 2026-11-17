//! What Ripple asks the OS: now playing, battery, Bluetooth, camera and microphone use,
//! CPU, memory, disk and network load, launching apps and links. Everything here blocks, so callers run it off the UI thread.

use std::ffi::OsString;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

#[cfg(target_os = "linux")]
use crate::command::tokenize_args;
#[cfg(windows)]
use crate::config::QuickApp;

/// `WAYLAND_DISPLAY` as Ripple was started with it. `main` unsets it when the Island runs on
/// XWayland (headless tools); apps Ripple starts get it back.
pub static WAYLAND_DISPLAY: OnceLock<Option<OsString>> = OnceLock::new();

fn command(program: &str) -> Command {
    let mut cmd = Command::new(program);
    if let Some(Some(display)) = WAYLAND_DISPLAY.get() { cmd.env("WAYLAND_DISPLAY", display); }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: no console flashes up
    }
    cmd
}

/// A status tool that runs longer than this is killed, so a hung one can't stall its poller.
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Trimmed stdout, or None when the program is missing, failed or ran past `TIMEOUT`.
fn output(program: &str, args: &[&str]) -> Option<String> {
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
        cmd.creation_flags(0x0800_0000 | 0x0000_0200); // CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP
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

/// Runs a PowerShell script through -EncodedCommand (a multi-line -Command is cut at the first
/// newline). Trimmed stdout, or "" on failure.
#[cfg(windows)]
fn powershell(script: &str) -> String {
    use base64::Engine;
    let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let encoded = base64::engine::general_purpose::STANDARD.encode(utf16);
    output("powershell", &["-NoProfile", "-EncodedCommand", &encoded]).unwrap_or_default()
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
    pub album: String,
    /// `https:`, `file:` or `data:` URL of the cover, when the player gives one.
    pub artwork_url: Option<String>,
    pub playing: bool,
    pub paused: bool,
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

#[cfg(target_os = "linux")]
pub fn media() -> Option<Track> {
    let out = output("playerctl", &["metadata", "--format", "{{title}}||{{artist}}||{{album}}||{{status}}||{{mpris:artUrl}}||{{position}}||{{mpris:length}}"])?;
    let parts: Vec<&str> = out.split("||").collect();
    if parts.len() < 4 || parts[0].is_empty() { return None }
    let status = parts[3].to_lowercase();
    let micros = |i: usize| parts.get(i).and_then(|v| v.trim().parse::<f64>().ok()).map(|v| v / 1e6);
    Some(Track {
        name: parts[0].into(),
        artist: parts[1].into(),
        album: parts[2].into(),
        artwork_url: parts.get(4).filter(|u| !u.is_empty()).map(|u| u.to_string()),
        playing: status == "playing",
        paused: status == "paused",
        source: "System".into(),
        // playerctl reports microseconds.
        progress: progress(micros(5), micros(6)),
    })
}

#[cfg(target_os = "macos")]
pub fn media() -> Option<Track> {
    const SCRIPT: &str = r#"
tell application "System Events"
    set spotifyRunning to (name of every process) contains "Spotify"
    set musicRunning to (name of every process) contains "Music"
end tell
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
    let out = output("osascript", &["-e", SCRIPT])?;
    let parts: Vec<&str> = out.split("||").collect();
    if parts.len() < 4 { return None }
    let playing = parts[1] == "playing";
    let seconds = |i: usize| parts.get(i).and_then(|v| v.trim().parse::<f64>().ok());
    Some(Track {
        name: parts[2].into(),
        artist: parts[3].into(),
        album: parts.get(4).unwrap_or(&"").to_string(),
        artwork_url: parts.get(5).filter(|u| !u.is_empty()).map(|u| u.to_string()),
        playing,
        paused: !playing,
        source: parts[0].into(),
        progress: progress(seconds(6), seconds(7)),
    })
}

#[cfg(windows)]
pub fn media() -> Option<Track> {
    const SCRIPT: &str = "[Console]::OutputEncoding = [System.Text.Encoding]::UTF8; Add-Type -AssemblyName System.Runtime.WindowsRuntime; $manager = [Windows.Media.Control.GlobalSystemMediaTransportControlsSessionManager, Windows.Media.Control, ContentType = WindowsRuntime]::RequestAsync().GetAwaiter().GetResult(); $session = $manager.GetCurrentSession(); if ($session) { $props = $session.TryGetMediaPropertiesAsync().GetAwaiter().GetResult(); $playback = $session.GetPlaybackInfo(); $status = $playback.PlaybackStatus; $timeline = $session.GetTimelineProperties(); $position = $timeline.Position; if ($status.ToString() -eq 'Playing' -and $timeline.LastUpdatedTime.Year -gt 2000) { $position += [DateTimeOffset]::Now - $timeline.LastUpdatedTime }; $length = $timeline.EndTime - $timeline.StartTime; $thumbnail = $props.Thumbnail; $artwork = ''; if ($thumbnail) { try { $stream = $thumbnail.OpenReadAsync().GetAwaiter().GetResult(); $buffer = New-Object byte[] $stream.Size; $reader = New-Object Windows.Storage.Streams.DataReader $stream; $reader.LoadAsync($stream.Size).GetAwaiter().GetResult() | Out-Null; $reader.ReadBytes($buffer); $artwork = 'data:image/png;base64,' + [Convert]::ToBase64String($buffer); $reader.Close(); $stream.Close(); } catch { } } $info = @{ Title = $props.Title; Artist = $props.Artist; Album = $props.AlbumTitle; Status = $status.ToString().ToLower(); Source = $session.SourceAppUserModelId; Artwork = $artwork; Position = $position.TotalSeconds; Length = $length.TotalSeconds }; return $info | ConvertTo-Json -Compress; } return 'null';";
    let out = powershell(SCRIPT);
    if let Ok(data) = serde_json::from_str::<serde_json::Value>(&out)
        && data.is_object()
    {
        let field = |k: &str| data[k].as_str().unwrap_or_default().to_string();
        let or = |v: String, fallback: &str| if v.is_empty() { fallback.to_string() } else { v };
        let playing = field("Status") == "playing";
        return Some(Track {
            name: or(field("Title"), "Unknown Title"),
            artist: or(field("Artist"), "Unknown Artist"),
            album: field("Album"),
            artwork_url: Some(field("Artwork")).filter(|a| !a.is_empty()),
            playing,
            paused: !playing,
            source: or(field("Source"), "System"),
            progress: progress(data["Position"].as_f64(), data["Length"].as_f64()),
        });
    }
    // No media session: fall back to Spotify's window title ("Artist - Song").
    let out = powershell("[Console]::OutputEncoding = [System.Text.Encoding]::UTF8; Get-Process | Where-Object {$_.ProcessName -eq 'Spotify'} | Select-Object -ExpandProperty MainWindowTitle");
    let title = out.lines().map(str::trim).find(|l| l.contains(" - "))?;
    let (artist, song) = title.split_once(" - ")?;
    Some(Track { name: song.into(), artist: artist.into(), playing: true, source: "Spotify".into(), ..Track::default() })
}

pub fn media_control(cmd: MediaCommand) {
    #[cfg(target_os = "linux")]
    {
        let arg = match cmd { MediaCommand::PlayPause => "play-pause", MediaCommand::Next => "next", MediaCommand::Previous => "previous" };
        output("playerctl", &[arg]);
    }
    #[cfg(target_os = "macos")]
    {
        // A fixed word, never user text: it is spliced into the script.
        let verb = match cmd { MediaCommand::PlayPause => "playpause", MediaCommand::Next => "next track", MediaCommand::Previous => "previous track" };
        let script = format!(r#"
tell application "System Events"
    set spotifyRunning to (name of every process) contains "Spotify"
    set musicRunning to (name of every process) contains "Music"
end tell
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
        powershell(&format!("Add-Type -AssemblyName System.Runtime.WindowsRuntime; $m = [Windows.Media.Control.GlobalSystemMediaTransportControlsSessionManager, Windows.Media.Control, ContentType = WindowsRuntime]::RequestAsync().GetAwaiter().GetResult(); $s = $m.GetCurrentSession(); if ($s) {{ $s.{method}().GetAwaiter().GetResult() | Out-Null }}"));
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
    { powershell("@(Get-PnpDevice -Class Bluetooth -ErrorAction SilentlyContinue | Where-Object { $_.Status -eq 'OK' -and $_.Present -eq $true -and $_.InstanceId -match 'BTHENUM' }).Count -gt 0").eq_ignore_ascii_case("true") }
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
        output("ioreg", &["-l"]).is_some_and(|o| o.lines()
            .any(|l| (l.contains("FrontCameraActive") || l.contains("FrontCameraStreaming")) && l.contains("= Yes")))
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
        output("ioreg", &["-l"]).is_some_and(|o| o.lines()
            .any(|l| (l.contains("IOAudioStreamActive") || l.contains("IOAudioEngine") || l.contains("IOAudioStream")) && l.to_lowercase().contains("yes")))
    }
    #[cfg(windows)]
    { capability_in_use("microphone") }
}

/// Windows records a capability's use per app; a stop time of 0 means it is in use now.
#[cfg(windows)]
fn capability_in_use(name: &str) -> bool {
    powershell(&format!("@(Get-ChildItem -Path \"HKCU:\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\CapabilityAccessManager\\ConsentStore\\{name}\" -Recurse -ErrorAction SilentlyContinue | ForEach-Object {{ Get-ItemProperty -Path $_.PSPath -Name \"LastUsedTimeStop\" -ErrorAction SilentlyContinue }} | Where-Object {{ $_ -and $_.LastUsedTimeStop -eq 0 }}).Count -gt 0"))
        .eq_ignore_ascii_case("true")
}

// --- Battery ---

/// (percent, plugged in), or None on a machine without a battery.
pub fn battery() -> Option<(u8, bool)> {
    battery_now()
}

#[cfg(target_os = "linux")]
fn battery_now() -> Option<(u8, bool)> {
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
fn battery_now() -> Option<(u8, bool)> {
    parse_pmset(&output("pmset", &["-g", "batt"])?)
}

/// `pmset -g batt`: "Now drawing from 'AC Power'" then " -InternalBattery-0 (id=…)\t87%; charging; …".
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
fn parse_pmset(out: &str) -> Option<(u8, bool)> {
    let end = out.find('%')?;
    let start = out[..end].rfind(|c: char| !c.is_ascii_digit()).map_or(0, |i| i + 1);
    Some((out[start..end].parse::<u8>().ok()?.min(100), out.contains("AC Power")))
}

#[cfg(windows)]
fn battery_now() -> Option<(u8, bool)> {
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

#[derive(Clone, Copy, Debug, Default, PartialEq)]
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

    pub fn sample(&mut self) -> Stats {
        self.sys.refresh_cpu_usage();
        self.sys.refresh_memory();
        // Re-list, so a mounted or unmounted disk is noticed.
        self.disks.refresh(true);
        self.nets.refresh(true);
        let secs = std::mem::replace(&mut self.at, std::time::Instant::now()).elapsed().as_secs_f64().max(0.001);
        let home = rune::dirs::home().unwrap_or_else(|| "/".into());
        let disks = self.disks.list();
        let disk = disks.iter().filter(|d| home.starts_with(d.mount_point())).max_by_key(|d| d.mount_point().as_os_str().len())
            .or_else(|| disks.iter().max_by_key(|d| d.total_space()));
        // VPN, container and Hyper-V layers repeat the physical NIC's bytes, so a sum counts them twice.
        // shortcut: two NICs busy at once (Wi-Fi and Ethernet) show only the busier; sum physical NICs if that matters.
        let nets = self.nets.list().iter().filter(|(name, _)| !(*name == "lo" || *name == "lo0" || name.contains("Loopback"))).map(|(_, n)| n);
        let (down, up) = nets.fold((0, 0), |(d, u), n| (d.max(n.received()), u.max(n.transmitted())));
        Stats {
            cpu: self.sys.global_cpu_usage(),
            mem_used: self.sys.used_memory(),
            mem_total: self.sys.total_memory(),
            disk_free: disk.map_or(0, |d| d.available_space()),
            disk_total: disk.map_or(0, |d| d.total_space()),
            down: (down as f64 / secs) as u64,
            up: (up as f64 / secs) as u64,
        }
    }
}

// --- Launch at login ---

pub fn set_auto_launch(enable: bool) {
    // An AppImage runs from a mount that changes each launch; $APPIMAGE is the file itself.
    let Some(exe) = std::env::var_os("APPIMAGE").map(std::path::PathBuf::from).or_else(|| std::env::current_exe().ok()) else { return };
    #[cfg(target_os = "linux")]
    {
        let Some(dir) = rune::dirs::home().map(|h| h.join(".config/autostart")) else { return };
        let file = dir.join("ripple.desktop");
        let result = if enable {
            let entry = format!("[Desktop Entry]\nType=Application\nVersion=1.0\nName=Ripple\nComment=Ripple Desktop Assistant\nExec={}\nIcon=ripple\nTerminal=false\n", desktop_exec(&exe.to_string_lossy()));
            std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&file, entry))
        } else if file.exists() {
            std::fs::remove_file(&file)
        } else {
            Ok(())
        };
        if let Err(e) = result { eprintln!("Ripple: failed to set launch at login: {e}") }
    }
    #[cfg(windows)]
    {
        const RUN: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
        let value = format!("\"{}\"", exe.display());
        if enable { output("reg", &["add", RUN, "/v", "Ripple", "/t", "REG_SZ", "/d", &value, "/f"]); }
        else { output("reg", &["delete", RUN, "/v", "Ripple", "/f"]); }
    }
    // macOS: not offered, as before (Settings hides the row).
    #[cfg(target_os = "macos")]
    let _ = (exe, enable);
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
    #[cfg(target_os = "linux")]
    let mut cmd = command("xdg-open");
    #[cfg(target_os = "macos")]
    let mut cmd = command("open");
    #[cfg(windows)]
    let mut cmd = {
        // No shell: `&` in a query string stays part of the URL.
        let mut cmd = command("rundll32");
        cmd.arg("url.dll,FileProtocolHandler");
        cmd
    };
    cmd.arg(url);
    spawn(cmd);
}

/// Web pages, and `file:` documents that wouldn't run as programs.
fn openable(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") { return true }
    let Some(path) = lower.strip_prefix("file://") else { return false };
    // The name the OS opens: decoded as the file-URL handler decodes it, without its query,
    // fragment or trailing slash (a macOS bundle is a folder), and without the trailing dots
    // and spaces Windows drops (`x.exe.` is `x.exe`).
    let path = crate::net::percent_decode(path.split(['?', '#']).next().unwrap_or_default());
    let path = path.trim_end_matches(['/', '\\', '.', ' ']);
    const EXECUTABLE: [&str; 29] = [".exe", ".bat", ".cmd", ".com", ".ps1", ".msi", ".scr", ".lnk", ".sh", ".app", ".desktop", ".jar",
        ".vbs", ".vbe", ".js", ".jse", ".wsf", ".hta", ".reg", ".command", ".tool", ".pkg", ".appimage", ".run", ".py", ".pl",
        ".cpl", ".msc", ".pif"];
    !path.is_empty() && !EXECUTABLE.iter().any(|ext| path.ends_with(ext))
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
    // Explicit links first, so file:// and https:// aren't caught by the path test below.
    if ["http://", "https://", "file://"].iter().any(|p| lower.starts_with(p)) { return Target::Url }
    // Paths, .exe names and shell: URIs, before any dot-based guess.
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

/// Opens the player a track came from.
pub fn open_music_player(source: &str) {
    if source.is_empty() { return }
    if source == "music.apple.com" || source.contains("Apple") { open_app("Music") } else { open_app(source) }
}

// --- Installed apps (Windows: suggestions for the quick apps field) ---

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
        match serde_json::from_str::<serde_json::Value>(&powershell(script)) {
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
    fn progress_needs_a_length_and_stays_within_it() {
        assert_eq!(progress(Some(30.5), Some(200.0)), Some((30, 200)));
        assert_eq!(progress(Some(250.0), Some(200.0)), Some((200, 200)));
        assert_eq!(progress(None, Some(200.0)), Some((0, 200)));
        assert_eq!(progress(Some(30.0), Some(0.0)), None);
        assert_eq!(progress(Some(30.0), None), None);
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
        assert!(!openable("javascript:alert(1)"));
        assert!(!openable("file://"));
    }

    #[test]
    fn powershell_strings_double_every_quote_it_ends_on() {
        assert_eq!(ps_quote("it's"), "it''s");
        assert_eq!(ps_quote("x\u{2019}; calc; \u{2019}"), "x\u{2019}\u{2019}; calc; \u{2019}\u{2019}");
    }

    #[test]
    fn a_hung_tool_is_killed_and_reads_as_failed() {
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
    fn pmset_output_gives_percent_and_power_source() {
        let ac = "Now drawing from 'AC Power'\n -InternalBattery-0 (id=4653155)\t100%; charged; 0:00 remaining present: true";
        let battery = "Now drawing from 'Battery Power'\n -InternalBattery-0 (id=4653155)\t87%; discharging; 4:12 remaining present: true";
        assert_eq!(parse_pmset(ac), Some((100, true)));
        assert_eq!(parse_pmset(battery), Some((87, false)));
        assert_eq!(parse_pmset("Now drawing from 'AC Power'"), None);
    }
}
