//! What Ripple asks the OS: now playing, battery, Bluetooth, camera and microphone use,
//! launching apps and links. Everything here blocks, so callers run it off the UI thread.

use std::ffi::OsString;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

#[cfg(target_os = "linux")]
use crate::command::tokenize_args;
#[cfg(windows)]
use crate::config::QuickApp;

/// `WAYLAND_DISPLAY` as Ripple was started with it. `main` unsets it so the Island runs on
/// X11 (Wayland can't place or raise a window); apps Ripple starts get it back.
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

/// Trimmed stdout, or None when the program is missing or failed.
// shortcut: no timeout, a hung tool stalls only its own poller; add one if a poller is seen stuck
fn output(program: &str, args: &[&str]) -> Option<String> {
    let out = command(program).args(args).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Starts a program Ripple doesn't wait for.
fn spawn(mut cmd: Command) {
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    if let Ok(mut child) = cmd.spawn() {
        // Reaps the child when it exits; otherwise it stays a zombie until Ripple quits.
        std::thread::spawn(move || { let _ = child.wait(); });
    }
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
    cmd.args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", &format!("Start-Process '{}'", target.replace('\'', "''"))]);
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaCommand {
    PlayPause,
    Next,
    Previous,
}

#[cfg(target_os = "linux")]
pub fn media() -> Option<Track> {
    let out = output("playerctl", &["metadata", "--format", "{{title}}||{{artist}}||{{album}}||{{status}}||{{mpris:artUrl}}"])?;
    let parts: Vec<&str> = out.split("||").collect();
    if parts.len() < 4 || parts[0].is_empty() { return None }
    let status = parts[3].to_lowercase();
    Some(Track {
        name: parts[0].into(),
        artist: parts[1].into(),
        album: parts[2].into(),
        artwork_url: parts.get(4).filter(|u| !u.is_empty()).map(|u| u.to_string()),
        playing: status == "playing",
        paused: status == "paused",
        source: "System".into(),
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
        end tell
        return "Spotify" & "||" & mediaState & "||" & songName & "||" & artistName & "||" & albumName & "||" & artUrl
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
        end tell
        return "Music" & "||" & mediaState & "||" & songName & "||" & artistName & "||" & albumName & "||" & ""
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
    Some(Track {
        name: parts[2].into(),
        artist: parts[3].into(),
        album: parts.get(4).unwrap_or(&"").to_string(),
        artwork_url: parts.get(5).filter(|u| !u.is_empty()).map(|u| u.to_string()),
        playing,
        paused: !playing,
        source: parts[0].into(),
    })
}

#[cfg(windows)]
pub fn media() -> Option<Track> {
    const SCRIPT: &str = "[Console]::OutputEncoding = [System.Text.Encoding]::UTF8; Add-Type -AssemblyName System.Runtime.WindowsRuntime; $manager = [Windows.Media.Control.GlobalSystemMediaTransportControlsSessionManager, Windows.Media.Control, ContentType = WindowsRuntime]::RequestAsync().GetAwaiter().GetResult(); $session = $manager.GetCurrentSession(); if ($session) { $props = $session.TryGetMediaPropertiesAsync().GetAwaiter().GetResult(); $playback = $session.GetPlaybackInfo(); $status = $playback.PlaybackStatus; $thumbnail = $props.Thumbnail; $artwork = ''; if ($thumbnail) { try { $stream = $thumbnail.OpenReadAsync().GetAwaiter().GetResult(); $buffer = New-Object byte[] $stream.Size; $reader = New-Object Windows.Storage.Streams.DataReader $stream; $reader.LoadAsync($stream.Size).GetAwaiter().GetResult() | Out-Null; $reader.ReadBytes($buffer); $artwork = 'data:image/png;base64,' + [Convert]::ToBase64String($buffer); $reader.Close(); $stream.Close(); } catch { } } $info = @{ Title = $props.Title; Artist = $props.Artist; Album = $props.AlbumTitle; Status = $status.ToString().ToLower(); Source = $session.SourceAppUserModelId; Artwork = $artwork }; return $info | ConvertTo-Json -Compress; } return 'null';";
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

/// (percent, plugged in). A machine without a battery reads as full and not charging.
pub fn battery() -> (u8, bool) {
    battery_now().unwrap_or((100, false))
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

// --- Launch at login ---

pub fn set_auto_launch(enable: bool) {
    let Ok(exe) = std::env::current_exe() else { return };
    #[cfg(target_os = "linux")]
    {
        let Some(dir) = rune::dirs::home().map(|h| h.join(".config/autostart")) else { return };
        let file = dir.join("ripple.desktop");
        let result = if enable {
            let entry = format!("[Desktop Entry]\nType=Application\nVersion=1.0\nName=Ripple\nComment=Ripple Desktop Assistant\nExec=\"{}\"\nIcon=ripple\nTerminal=false\n", exe.display());
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

// --- Opening links and apps ---

/// Opens a web page or a document in its default app. `file:` is allowed for documents but
/// not for anything that would execute.
pub fn open_external(url: &str) {
    let lower = url.to_ascii_lowercase();
    let web = lower.starts_with("http://") || lower.starts_with("https://");
    const EXECUTABLE: [&str; 12] = [".exe", ".bat", ".cmd", ".com", ".ps1", ".msi", ".scr", ".lnk", ".sh", ".app", ".desktop", ".jar"];
    let document = lower.starts_with("file://") && !EXECUTABLE.iter().any(|ext| lower.ends_with(ext));
    if !web && !document { return }
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
        // .cmd and .bat run through cmd.exe; std quotes their arguments safely or refuses.
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
    fn pmset_output_gives_percent_and_power_source() {
        let ac = "Now drawing from 'AC Power'\n -InternalBattery-0 (id=4653155)\t100%; charged; 0:00 remaining present: true";
        let battery = "Now drawing from 'Battery Power'\n -InternalBattery-0 (id=4653155)\t87%; discharging; 4:12 remaining present: true";
        assert_eq!(parse_pmset(ac), Some((100, true)));
        assert_eq!(parse_pmset(battery), Some((87, false)));
        assert_eq!(parse_pmset("Now drawing from 'AC Power'"), None);
    }
}
