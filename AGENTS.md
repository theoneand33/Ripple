# AGENTS.md

Ripple is a Dynamic Island-style bar for Windows, Linux, and macOS. It is one Rust binary built on [Rune](https://github.com/anomalyco/rune). The README covers features and usage. This file covers how to change the code.

## Layout

| File | What it owns |
| --- | --- |
| `src/main.rs` | App setup, windows, tray, and the headless scenes (`island_spec().scenes`) |
| `src/island.rs` | Island state, the Still/Quick/Large rules, alerts, and the compact views |
| `src/tabs/` | One module per Large Mode tab: its state, its actions, and its view. `mod.rs` maps each tab to its view and size |
| `src/settings.rs` | The Settings window. Each change saves at once. |
| `src/config.rs` | Every user setting, stored as one JSON value in `prefs.json`; the `Tab` list, themes and AI providers |
| `src/system.rs` | OS queries: now playing, battery, Bluetooth, camera, mic, launching apps |
| `src/net.rs` | Weather, the AI assistant stream, and images by URL |
| `src/stocks.rs` | Stock quotes (Yahoo Finance) and the Sure portfolio |
| `src/downloads.rs` | Download progress from each OS, shown as a live activity |
| `src/stremflix.rs` | Stremflix-rs's local API: Continue Watching and playback |
| `src/nextcloud.rs` | Nextcloud sync for Notes (Notes API) and Tasks (CalDAV), and the merge that keeps local edits |
| `src/limits.rs` | AI Limits: finds each coding agent's saved sign-in and reads its plan usage |
| `src/plugins.rs` | Lua plugins: loading the plugins folder, the `ui.*` nodes, and `--test-plugin`. API in `PLUGINS.md` |
| `src/t3.rs` | T3 Code client (HTTP, `/ws` RPC, `/mcp` sign-in) |
| `src/platform.rs` | Displays, click-through around the Island (X11 input shape), and focus hand-back |
| `src/mac.rs` | macOS status-bar item and the notch window frame |
| `src/date.rs` | Calendar date maths and the wall clock |
| `src/agents.rs` | `ripple hook <agent>`: records agent sessions and waits for the Island's answer to a permission or question. `integrations/opencode/ripple.ts` calls it from OpenCode |
| `src/notify.rs` | `ripple notify "text"`: queues a script's alert as a file the Island picks up |
| `src/paths.rs` | The app folder for the CLIs, `~` expansion, and atomic file writes |
| `src/command.rs` | Splits a launch string into argv without a shell |
| `src/icons.rs` | Lucide icons as Rune icon bodies |
| `src/testserver.rs` | Local HTTP/WebSocket server for tests |

## Build and verify

- GitHub Actions (`.github/workflows/build.yml`) runs the tests, renders every scene, and smoke-runs the app on each OS.
- You may run `cargo check`, `cargo test`, `cargo build`, and `cargo run` locally to confirm that a change or fix works. Debug builds render headlessly, so you need no display:

  ```bash
  cargo run -- --list-scenes
  cargo run -- --window island --scene large --snap island.png
  cargo run -- --window settings --size 520x2600 --snap settings.png
  ```

- For a very small change, such as a typo, a string, a constant, or a few lines with no new logic, run only `cargo check` (fairly quick). Skip `cargo build` and `cargo test` (both can take a while if uncached) and let CI run them.
- Then push and read the CI results.
- When you add a view or a state that needs a visual check, add a scene to `scenes` in `src/main.rs`. CI renders every scene `--list-scenes` prints and uploads the PNGs as the `smoke-*` artifacts.
- To add a tab, add it to `Tab` in `src/config.rs` with a new number, then follow the compiler: every `match` on `Tab` needs an arm.
- Linux build deps: `libxkbcommon-dev libwayland-dev`. The toolchain must be the Rust version that Rune pins (1.99 or newer).
- A full debug link uses a lot of RAM. Keep the `[profile.dev]` settings.

## Rules

- Rune is pre-release. It is pinned to one git `rev`. If you bump it, change the `rev` in `[dependencies]` and in every `[patch.crates-io]` entry together.
- `system.rs`, `net.rs` and the service clients (`t3`, `limits`, `stocks`, `stremflix`, `nextcloud`) block. Call them off the UI thread, the way the existing callers do.
- Run user-supplied launch strings through `command.rs`. Never pass them to a shell.
- Code must build and test on all three OSes. Gate platform code with `#[cfg(target_os = ...)]`. CI builds macOS, Windows, and Linux and smoke-runs each one.
- `Config` uses `#[serde(default)]`. When you add a field, give it a value in `Default` so that old `prefs.json` files still load.
- Network tests use `testserver.rs`. They never call real hosts.
- Tests and headless renders must pass in CI with no display and no localhost services.

## Commits and PRs

- Use Conventional Commits with a scope, for example `fix(island): ...`, `feat: ...`, or `ci: ...`.
- Keep one concern per PR. CI review reports only bugs, security issues, and regressions (`.github/review-guidelines.md`).
