# Ripple

Dynamic Island, but for everyone.

Ripple is a native desktop app for Windows, Linux, and macOS, written in Rust with [Rune](https://github.com/anomalyco/rune). It puts a Dynamic Island-style bar at the top of your screen. It shows notifications, widgets, and an AI assistant, and stays compact until you hover over it or click it.

<div align="center">

[![Discord](https://img.shields.io/badge/Discord-Join%20Us-5865F2)](https://discord.gg/a2xzVkxFVg)
[![License](https://img.shields.io/badge/License-MIT-green)](LICENSE)
[![Version](https://img.shields.io/badge/Version-0.3.1-blue)](Cargo.toml)

https://github.com/user-attachments/assets/a7b9ed8a-97b5-4603-bad7-6f4c4bef1f5d

</div>

## Features

### Island

- The Island follows you across every desktop and monitor.
- Notch Mode on macOS moves the Island into the MacBook notch. Mini notch keeps that look, pinned to the top edge, on displays without a notch.
- Standby mode is on by default, so the Island rests in Quick Mode instead of hiding. You can also make it rest in Large Mode.
- While an app is full screen, the Island can switch to standby mode, hide when inactive, turn standby mode off, or make no change (the default). Native Wayland windows don't report full screen.
- Corners use iOS-style radii.
- Themes include Win95, SleekBlack, and more.
- On Linux, clicks pass through the empty area around the Island to the windows underneath.

### Tabs

- Search works like Spotlight: it lists matching apps first, then files from the OS search index (Spotlight, Windows Search, or on Linux GNOME LocalSearch or KDE Baloo), then a web search on Google, DuckDuckGo, Brave Search, Bing, or Startpage. Pick the engine in Settings.
- Workflows & QA opens a saved set of URLs with one click and keeps quick-access apps.
- Overview shows the time, weather, and battery. While music plays, it also shows the current track with playback controls. Long titles scroll with soft blurred edges, and the track stays on screen for 3 seconds after you pause.
- Weather shows the current conditions and a five-day forecast for the location set in Settings.
- AI Assistant answers questions using Groq.
- Clipboard keeps your clipboard history, images included. Search it, and pin items with the star to keep them on top and across restarts.
- Tasks is a to-do list. The controls under the field set a task's priority, due date and time (the arrows step a day or 30 minutes), and whether it repeats daily, weekly or monthly. Press a task to edit it. Ticking a repeating task brings it back due next time, and the Island shows a reminder when a task falls due (9:00 for a day without a time). One that fell due while Ripple was closed shows when it next runs. Tasks can sync with Nextcloud Tasks (CalDAV), where these are its priority, due date and repeat; ticking a task marks it done there. A repeat Ripple can't follow (every 2 weeks, set days) is left to Nextcloud: tick that task there.
- Calendar shows a month with today marked, and the picked day's events beside it. Type an event to add it to the picked day (start it with a time, like `09:30 Standup`), and press an event to edit it. Page through months, and press Today to jump back. Events can sync with a Nextcloud calendar (CalDAV); a recurring or multi-day event shows on its first day only, and events that ended over a year ago drop out. An edit is written over the event there, so its description, alarms and length are kept. A repeating event can be renamed here but only moved in Nextcloud.
- Notes keeps notes in a list beside an editor. A note's first line is its title. Pin a note with the star to keep it on top, and search to filter the list. Notes can sync with Nextcloud Notes. Set the account in the tab's settings page.
- Stremflix shows Continue Watching from [Stremflix-rs](https://github.com/theoneand33/stremflix-rs) when it is open. Click a title to resume it in Stremflix-rs. Set `STREMFLIX_RS_API` if Stremflix-rs serves its API on another address.
- System shows CPU, memory, storage, network and battery use, and the apps using the most CPU and memory.
- AI Limits shows how much of each coding agent's plan is used and when it resets: Claude Code, Codex, GitHub Copilot, OpenCode Go and Grok Build. It reads the sign-in each CLI already saved and lists only the agents signed in on this computer. For more accounts, add Claude Code or Codex config folders (one per account) in its settings; every Claude and Codex account in CLIProxyAPI's auth folder (`~/.cli-proxy-api` by default) shows too. When a token in a Claude Code or Codex config folder has expired, Ripple renews it with the saved refresh token and writes the new tokens back, so the CLI stays signed in. CLIProxyAPI renews its own.
- Shelf stashes files: drag a file over the Island and it opens the Shelf to drop it in. Drag files back out to use them. Both directions work on macOS, Windows, X11, KDE Plasma and other layer-shell compositors, and GNOME (where the Island runs under XWayland).
- T3 Code starts an agent thread on a project on one of your T3 devices. You choose the model, the worktree, and the linked environment.
- Pomodoro runs 25-minute focus rounds with 5-minute breaks, and has a timer and a stopwatch beside it. While one runs, the compact Island shows its time beside a ring; a finished timer stays there until you open the tab.
- Downloads show as a live activity: a ring fills as they progress. On Linux the percent comes from the app's launcher progress (Firefox, Chrome and KDE apps, on Wayland or X11, under Plasma, GNOME and others), on macOS from what browsers report to Finder. On Windows, and for Linux apps that report nothing, partial files in the Downloads folder show, without a percent. A download that finishes turns its ring into a tick for a moment.
- Calculator shows the answer as you type a sum (+ - × ÷ ^, % as remainder, and brackets). It also converts units and currencies, like `5 km to mi`, `100 °F to C` or `20 usd in eur` (reference rates from Frankfurter, fetched when the tab first opens). Press Enter to keep it on the history tape, which is kept across restarts.
- Habits tracks things to do every day. Tick off today or any of the last six days; each habit shows its streak of days in a row.
- Plugins shows panels you write in Lua: drop a `.lua` file in the `plugins` folder next to `prefs.json`. See [PLUGINS.md](PLUGINS.md) for the API and how to test a plugin without a display.

You can reorder and hide tabs in Settings.

### Alerts

Ripple alerts you when a charger connects or the battery runs low, when a Bluetooth device connects or disconnects, and when an app starts using your camera or microphone.

Scripts can raise their own alert: `ripple notify "Build passed"`.

### Settings

Settings open in their own window from the gear button on the Island. Changes apply to the Island right away.

## Quick start

### Installation

Download the latest release for your platform:

- Windows: `.zip`
- macOS: `.dmg` (Intel and Apple Silicon)
- Linux: `.AppImage` or `.tar.gz`

[Download Latest Release](https://github.com/TopMyster/Ripple/releases)

### First run

1. Install and launch Ripple. The Island appears at the top of your screen.
2. Hover over it to see Quick Mode.
3. Click it to open Large Mode.
4. Click the gear button on the Overview tab, or Settings in the tray menu, to open Settings.

### Connecting T3 Code

Open Settings and add a device under T3 Code Devices with a name, URL, and bearer token. For linked environments, Ripple signs in to the device's `/mcp` endpoint and caches the 30-day token (`t3-mcp-tokens.json` next to the settings file), so it does not register a new client on every launch.

## How to use

### The three modes

Still Mode is the compact idle state. With standby mode on, the Island rests in Quick Mode instead.

Quick Mode appears when you hover over the Island. It shows the time, weather, and battery, or the current track with playback controls when music is playing.

Large Mode opens when you click the Island. It shows the full interface with every tab.

### Switching tabs

- Arrow keys move between tabs.
- The mouse wheel scrolls between tabs.
- Ctrl + 1 to 8 jumps to a tab.

## Build and development

### Prerequisites

- Rust 1.99 or newer, the version Rune pins (`rustup` or your distribution's package)
- Linux: the development packages for `libxkbcommon` and `wayland` (`sudo apt-get install libxkbcommon-dev libwayland-dev`, or `sudo dnf install libxkbcommon-devel wayland-devel`)
- Windows: the Visual Studio C++ build tools
- macOS: Xcode Command Line Tools

At runtime on Linux, now playing and media keys use `playerctl`, Bluetooth uses `bluetoothctl`, the microphone check uses `pactl` and the camera check uses `fuser`. Ripple runs on X11, or on XWayland in a Wayland session. On Wayland, closing the Island hands focus back through `busctl`: on KDE Plasma by a KWin script, and on GNOME only when the [Window Calls](https://github.com/ickyicky/window-calls) extension is installed. On Wayland the global hotkey goes through the desktop portal (KDE Plasma 6 and GNOME 48 or newer), which may ask once to allow it; Ripple writes a hidden `io.github.theoneand33.Ripple.desktop` to `~/.local/share/applications` for the portal to recognize it.

### Run

```bash
git clone https://github.com/TopMyster/Ripple.git
cd Ripple
cargo run
```

Settings are saved as JSON in `prefs.json` in the app's config folder: `~/.config/Ripple` on Linux, `%APPDATA%\Ripple` on Windows, `~/Library/Application Support/Ripple` on macOS.

### Test

```bash
cargo test
```

Debug builds also render headlessly, so you can check a change without a display:

```bash
cargo run -- --list-scenes
cargo run -- --window island --scene large --snap island.png
cargo run -- --window settings --snap settings.png --size 520x1400
```

### Build a release

```bash
cargo build --release
```

The binary is `target/release/ripple` (`ripple.exe` on Windows). CI packages it as a `.dmg`, a `.zip`, an `.AppImage` and a `.tar.gz`; see `.github/workflows/build.yml`.

## Contributing

Bug reports, fixes, features, documentation, themes, and ideas are all welcome. See [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

## License

Ripple is open source under the [MIT License](LICENSE).
