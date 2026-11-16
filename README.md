# Ripple

Dynamic Island, but for everyone.

Ripple is a desktop app for Windows, Linux, and macOS that puts a Dynamic Island-style bar at the top of your screen. It shows notifications, widgets, and an AI assistant, and stays compact until you hover over it or click it.

<div align="center">

[![Discord](https://img.shields.io/badge/Discord-Join%20Us-5865F2)](https://discord.gg/a2xzVkxFVg)
[![License](https://img.shields.io/badge/License-MIT-green)](LICENSE)
[![Version](https://img.shields.io/badge/Version-0.1.0-blue)](package.json)

https://github.com/user-attachments/assets/a7b9ed8a-97b5-4603-bad7-6f4c4bef1f5d

</div>

## Features

### Island

- The Island follows you across every desktop and monitor.
- Notch Mode on macOS moves the Island into the MacBook notch.
- Standby mode is on by default, so the Island rests in Quick Mode instead of hiding. You can also make it rest in Large Mode.
- Corners use iOS-style radii.
- Themes include Win95, SleekBlack, and more.
- On Linux, clicks pass through the empty area around the Island to the windows underneath.

### Tabs

- Browser Search sends queries to Google, DuckDuckGo, Brave Search, Bing, or Startpage. Pick the engine in Settings.
- Workflows & QA opens a saved set of URLs with one click and keeps quick-access apps.
- Overview shows the time, weather, and battery.
- Now Playing shows the current track with playback controls. Long titles scroll with soft blurred edges, and the track stays on screen for 3 seconds after you pause.
- AI Assistant answers questions using Groq.
- Clipboard keeps your clipboard history.
- Tasks is a simple to-do list.
- T3 Code starts an agent thread on a project on one of your T3 devices. You choose the model, the worktree, and the linked environment.

You can reorder and hide tabs in Settings.

### Alerts

Ripple alerts you when a charger connects or the battery runs low, when a Bluetooth device connects or disconnects, and when an app starts using your camera or microphone.

### Settings

Settings open in their own window from the gear button on the Island. Changes apply to the Island right away.

## Quick start

### Installation

Download the latest release for your platform:

- Windows: `.msi` installer or `.zip`
- macOS: `.dmg` (Intel and Apple Silicon)
- Linux: `.deb`, `.rpm`, or `.AppImage`

[Download Latest Release](https://github.com/TopMyster/Ripple/releases)

### First run

1. Install and launch Ripple. The Island appears at the top of your screen.
2. Hover over it to see Quick Mode.
3. Click it to open Large Mode.
4. Click the gear button to open Settings.

### Connecting T3 Code

Open Settings and add a device under T3 Code Devices with a name, URL, and bearer token. For linked environments, Ripple signs in to the device's `/mcp` endpoint and caches the 30-day token, so it does not register a new client on every launch.

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

- Node.js 16+ and npm
- Platform build tools:
  - Windows: Visual Studio Build Tools
  - macOS: Xcode Command Line Tools
  - Linux: `build-essential` on Ubuntu/Debian

### Development setup

```bash
# Clone the repository
git clone https://github.com/TopMyster/Ripple.git
cd Ripple

# Install dependencies
npm install

# Start development server
npm start
```

`npm start` launches Ripple with hot reload. Press `Ctrl+R` (or `Cmd+R` on macOS) to refresh the app.

#### Build for the current OS

```bash
npm run make
```

#### Build for a specific platform

Windows (x64)
```bash
npm run make -- --platform=win32 --arch=x64
```

Linux (x64)
```bash
npm run make -- --platform=linux --arch=x64
```

macOS (Apple Silicon)
```bash
npm run make -- --platform=darwin --arch=arm64
```

macOS (Intel)
```bash
npm run make -- --platform=darwin --arch=x64
```

> Building Linux packages requires `dpkg`, `fakeroot`, and `rpm`:
> ```bash
> sudo apt-get install dpkg fakeroot rpm
> ```

Built packages go to `out/make/`.

## Contributing

Bug reports, fixes, features, documentation, themes, and ideas are all welcome. See [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

## License

Ripple is open source under the [MIT License](LICENSE).
