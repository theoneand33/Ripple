# Plugins

A plugin is one Lua 5.4 file that adds a panel to the Plugins tab. Put `.lua` files in the `plugins` folder next to `prefs.json`:

- Linux: `~/.config/Ripple/plugins`
- Windows: `%APPDATA%\Ripple\plugins`
- macOS: `~/Library/Application Support/Ripple/plugins`

Ripple creates the folder on launch. It loads new and changed files, and reloads plugins with an error, each time you open the Plugins tab, so you don't need to restart. With more than one plugin, chips at the top of the tab switch between them.

[`plugins/example.lua`](plugins/example.lua) is a complete plugin: a focus timer.

## The API

The file returns a table:

```lua
return {
  name = "Counter",          -- the chip's label; defaults to the file name
  state = { n = 0 },         -- passed to view, tick and every handler
  persist = true,            -- optional; keeps state across restarts (see Saved state)
  tick = function(state) end, -- optional; once a second while the panel shows
  view = function(state)     -- required; runs after load, each press and each tick
    return ui.col {
      ui.text("Count: " .. state.n, { size = 20, weight = 600 }),
      ui.row {
        ui.button("-", function(state) state.n = state.n - 1 end),
        ui.button("+", function(state) state.n = state.n + 1 end),
      },
    }
  end,
}
```

| Helper | Draws |
| --- | --- |
| `ui.text(value, { size = 14, weight = 400, dim = false })` | A line of text. `dim` draws it faded. A plain string in a view is a text too. |
| `ui.row { gap = 8, ... }` / `ui.col { gap = 6, ... }` | Children side by side, or stacked |
| `ui.button(label, on_press, { key = label })` | The Island's button. `key` names it for tests; it defaults to the label. |
| `ui.bar(fraction)` | A progress bar, 0 to 1 |
| `ui.toggle(label, on, on_press, { key = label })` | A checkbox with a label. `on_press(state)` runs on a click; set the new value yourself. |
| `ui.input(key, { value, placeholder, on_change, on_submit })` | A one-line text field. `on_change(state, text)` runs as the user types, `on_submit(state, text)` on Return. Ripple keeps the text; `value` replaces it whenever the view gives a new one, so `value = state.query` with `state.query = ""` in `on_submit` clears the field. |
| `ui.icon(name, { size = 16 })` | A [Lucide](https://lucide.dev) icon by name, such as `"timer"` or `"bell"`. Ripple ships the icons it uses itself; another name is an error. |
| `ui.spacer()` | Empty room that grows, to push the rest of a row to its far end |

### Talking to the Island

| Function | Does |
| --- | --- |
| `ripple.run(cmd, function(state, out) end)` | Runs a command off the UI thread, then calls back with its trimmed stdout, or `nil` if it failed, exited non-zero or ran over 30 s. `cmd` is a string, split into words without a shell (quotes group), or an argv table such as `{ "git", "-C", dir, "status" }`. Returns the job's id. |
| `ripple.notify(text)` | Shows `text` as an Island alert, as `ripple notify` does. It waits until the Island is compact. |
| `ripple.open(target)` | Opens an `http(s)://` or `file://` URL in its default app. Programs are refused. |
| `ripple.copy(text)` | Copies `text` to the clipboard. |

Use `ripple.run` for anything slow: `io.popen` blocks the Island until it returns.

### Saved state

With `persist = true`, Ripple saves `state` as JSON to `plugin-data/<file name>.json` beside `prefs.json` after each change, and loads it in place of the file's `state` next time. Only strings, numbers, booleans and tables are saved; functions are dropped. An empty table comes back as an empty table. Nothing is saved while `RIPPLE_PLUGINS` is set or under `--test-plugin`.

Every plugin has its own Lua state, with the standard libraries except `debug` and `__gc` metamethods, so it can read files and run commands as you. Install only plugins you trust, as you would a shell script.

Plugins run on the UI thread. Each load, tick or button press must finish in 250 ms, its view included, or it is stopped with an error, each plugin may use up to 64 MB, and a view may have up to 2,000 nodes, 32 deep. Time spent inside a C function such as `io.popen` isn't counted, so run slow work with `ripple.run`. Give every button a unique `key`; two buttons with the same label need one.

Errors show at the top of the panel and on stderr. An error stays until you change the file or open the Plugins tab again, which reloads the plugin.

## Testing a plugin

These commands need no display, so they work in CI, over SSH and for agents.

`--test-plugin` works in any build, the installed app included. It loads one file, runs the steps in order and prints JSON. It exits 1 if the plugin has an error. On Windows the release build is a GUI app, so the shell doesn't wait for it: pipe it (`ripple --test-plugin counter.lua | Out-String`) to get the output and exit code in order.

```bash
ripple --test-plugin counter.lua --press + --press + --tick
```

```json
{
  "name": "Counter",
  "error": null,
  "view": { "type": "col", "gap": 6.0, "children": [{ "type": "text", "text": "Count: 2", ... }, ...] },
  "effects": []
}
```

`--press KEY` presses the button or toggle with that key. `--type KEY TEXT` types into a field, which runs its `on_change`, and `--submit KEY` presses Return in it. `--tick` runs one tick. `--wait` runs the `ripple.run` jobs asked for so far and delivers their output. `effects` lists what the plugin asked the Island to do (`notify`, `open`, `copy`, and `run` jobs no `--wait` ran) without doing it.

Debug builds also render the panel. `RIPPLE_PLUGINS` points the Island at one file or a folder instead of the plugins folder:

```bash
RIPPLE_PLUGINS=counter.lua cargo run -- --window island --scene plugins --snap plugin.png
```

To press buttons in the render, pass a [Rune input script](https://github.com/anomalyco/rune) that taps an element by its key, and `--tree` to dump what is on screen:

```bash
echo '[{"t": 0.5, "type": "tap", "key": "+"}]' > taps.json
RIPPLE_PLUGINS=counter.lua cargo run -- --window island --scene plugins --script taps.json --snap plugin.png --tree tree.json
```
