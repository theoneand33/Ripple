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
  state = { n = 0 },         -- passed to view, tick and every on_press
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

Every plugin has its own Lua state, with the standard libraries except `debug`, so it can read files and run commands as you. Install only plugins you trust, as you would a shell script.

Plugins run on the UI thread. Each load, tick or button press must finish in 250 ms, its view included, or it is stopped with an error, each plugin may use up to 64 MB, and a view may have up to 2,000 nodes, 32 deep. Time spent inside a C function such as `io.popen` isn't counted, so keep slow work out of plugins. Give every button a unique `key`; two buttons with the same label need one.

Errors show at the top of the panel and on stderr. An error stays until you change the file or open the Plugins tab again, which reloads the plugin.

## Testing a plugin

These commands need no display, so they work in CI, over SSH and for agents.

`--test-plugin` works in any build, the installed app included. It loads one file, runs the steps in order and prints JSON. It exits 1 if the plugin has an error.

```bash
ripple --test-plugin counter.lua --press + --press + --tick
```

```json
{
  "name": "Counter",
  "error": null,
  "view": { "type": "col", "gap": 6.0, "children": [{ "type": "text", "text": "Count: 2", ... }, ...] }
}
```

`--press KEY` presses the button with that key. `--tick` runs one tick.

Debug builds also render the panel. `RIPPLE_PLUGINS` points the Island at one file or a folder instead of the plugins folder:

```bash
RIPPLE_PLUGINS=counter.lua cargo run -- --window island --scene plugins --snap plugin.png
```

To press buttons in the render, pass a [Rune input script](https://github.com/anomalyco/rune) that taps an element by its key, and `--tree` to dump what is on screen:

```bash
echo '[{"t": 0.5, "type": "tap", "key": "+"}]' > taps.json
RIPPLE_PLUGINS=counter.lua cargo run -- --window island --scene plugins --script taps.json --snap plugin.png --tree tree.json
```
