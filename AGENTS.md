# AGENTS.md

Ripple is a Dynamic Island-style bar for Windows, Linux, and macOS: one Rust binary built on [Rune](https://github.com/anomalyco/rune). Features and usage live in `README.md`; the Lua plugin API in `PLUGINS.md`.

## Layout

| File | What it owns |
| --- | --- |
| `src/main.rs` | App setup, windows, tray, and the headless scenes (`island_spec().scenes`) |
| `src/island.rs` | Island state, the Still/Quick/Large rules, alerts, and the compact views |
| `src/tabs/` | One module per Large Mode tab: its state, actions, and view. `mod.rs` maps each tab to its view and size |
| `src/settings.rs` | The Settings window. Each change saves at once |
| `src/config.rs` | Every user setting (one JSON value in `prefs.json`), the `Tab` list, themes, AI providers |
| `src/system.rs` | OS queries: now playing, battery, Bluetooth, camera, mic, launching apps |
| `src/net.rs` | Weather, the AI assistant stream, images by URL |
| `src/stocks.rs` | Stock quotes (Yahoo Finance) and the Sure portfolio |
| `src/downloads.rs` | Download progress from each OS, shown as a live activity |
| `src/stremflix.rs` | Stremflix-rs's local API: Continue Watching and playback |
| `src/nextcloud.rs` | Notes API, CalDAV, WebDAV, and the merge that keeps local edits. `tabs/sync.rs` starts syncs and applies their outcome |
| `src/ics.rs` | iCalendar text: tasks and events to and from their one-line texts, in-place edits |
| `src/task.rs` | A task's one-line text (`!! Title due:… every:…`) |
| `src/discord.rs` | Discord local RPC (IPC socket or named pipe): voice channel, members, speakers, mute/deafen/leave |
| `src/limits.rs` | AI Limits: each coding agent's saved sign-in and plan usage |
| `src/plugins.rs` | Lua plugins: the plugins folder, `ui.*` nodes, `--test-plugin` |
| `src/t3.rs` | T3 Code client (HTTP, `/ws` RPC, `/mcp` sign-in) |
| `src/pick.rs` | Screen sampling: the colour of a clicked pixel, per OS |
| `src/platform.rs` | Displays, click-through around the Island (X11 input shape), focus hand-back |
| `src/mac.rs` | macOS status-bar item and the notch window frame |
| `src/date.rs` | Calendar date maths and the wall clock |
| `src/agents.rs` | `ripple hook <agent>`: records agent sessions and waits for the Island's answer to a permission or question. Called from `integrations/opencode/ripple.ts` |
| `src/notify.rs` | `ripple notify "text"`: queues a script's alert as a file the Island picks up |
| `src/paths.rs` | The app folder, `~` expansion, atomic file writes |
| `src/command.rs` | Splits a launch string into argv without a shell |
| `src/icons.rs` | Lucide icons as Rune icon bodies |
| `src/testserver.rs` | Local HTTP/WebSocket server for tests |

## Verify a change

1. Size the check to the change. A typo, string, constant, or a few lines with no new logic: `cargo check` only. New logic: `cargo test`, plus a render if it touches a view.
2. A new view or state gets a **scene** in `scenes` in `src/main.rs`. Debug builds render headlessly, no display needed:

   ```bash
   cargo run -- --list-scenes
   cargo run -- --window island --scene large --snap island.png
   cargo run -- --window settings --size 520x2600 --snap settings.png
   ```

3. Push and read CI (`.github/workflows/build.yml`). Done when CI is green on all three OSes: it runs the tests, renders every scene `--list-scenes` prints (PNGs in the `smoke-*` artifacts), and smoke-runs the app.

Toolchain: the Rust version Rune pins (1.99+). Linux deps: `libxkbcommon-dev libwayland-dev`. Keep `[profile.dev]` as is: a full debug link otherwise exhausts RAM.

## Rules

- **Headless**: tests and renders pass in CI with no display and no localhost services. Network tests serve from `testserver.rs`.
- **Blocking clients**: `system.rs`, `net.rs`, `t3`, `limits`, `stocks`, `stremflix`, and `nextcloud` block. Call them off the UI thread, the way existing callers do.
- **Launch strings**: split user-supplied commands with `command.rs` and spawn the argv directly.
- **Three OSes**: gate platform code with `#[cfg(target_os = ...)]` so every OS builds.
- **New `Config` field**: give it a value in `Default` (`Config` is `#[serde(default)]`), so old `prefs.json` files still load.
- **New tab**: add it to `Tab` in `src/config.rs` with a new number, then add an arm to every `match` the compiler flags.
- **Rune bump**: Rune is pre-release, pinned to one git `rev`. Change it in `[dependencies]` and every `[patch.crates-io]` entry together.

## Commits and PRs

- Conventional Commits with a scope: `fix(island): ...`, `feat: ...`, `ci: ...`.
- One concern per PR. CI review reports only bugs, security issues, and regressions (`.github/review-guidelines.md`).
