//! Lua plugins: each `.lua` file in the plugins folder is a panel on the Plugins tab.
//! PLUGINS.md documents the API and how to test a plugin.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime};

use mlua::{Function, HookTriggers, Lua, LuaSerdeExt, Table, Value, VmState};
use rune::TextEdit;
use serde::Serialize;

/// Defines `ui.*`, the helpers a plugin's view builds its nodes with.
const PRELUDE: &str = r#"
ui = {}
local function node(kind, t) t.type = kind; return t end
function ui.text(text, opts) local t = opts or {}; t.text = tostring(text); return node("text", t) end
function ui.row(children) return node("row", children) end
function ui.col(children) return node("col", children) end
function ui.button(label, on_press, opts) local t = opts or {}; t.label = tostring(label); t.on_press = on_press; return node("button", t) end
function ui.bar(value) return node("bar", { value = value }) end
function ui.toggle(label, on, on_press, opts) local t = opts or {}; t.label = tostring(label); t.on = on; t.on_press = on_press; return node("toggle", t) end
function ui.input(key, opts) local t = opts or {}; t.key = tostring(key); return node("input", t) end
function ui.icon(name, opts) local t = opts or {}; t.name = tostring(name); return node("icon", t) end
function ui.spacer() return node("spacer", {}) end
-- Past the time budget, errors pass through pcall, xpcall, coroutine.resume and load's reader, so a plugin can't catch its stop.
do
  local expired = __ripple_expired; __ripple_expired = nil
  local function check(ok, ...) if not ok and expired() then error((...), 0) end return ok, ... end
  local pcall0, xpcall0, resume0, load0 = pcall, xpcall, coroutine.resume, load
  pcall = function(...) return check(pcall0(...)) end
  xpcall = function(...) return check(xpcall0(...)) end
  coroutine.resume = function(...) return check(resume0(...)) end
  load = function(...) return check(load0(...)) end -- fails as nil, msg
  -- Lua runs no hooks inside __gc, so a finalizer could never be stopped. Lua reads __gc only here.
  local setmetatable0, rawget0 = setmetatable, rawget
  setmetatable = function(t, mt)
    if type(mt) == "table" and rawget0(mt, "__gc") ~= nil then error("plugins can't use __gc", 2) end
    return setmetatable0(t, mt)
  end
end
-- `ripple.*` asks the Island for work a plugin can't do on the UI thread or in its sandbox.
-- The chunk returns `finish`, which Rust calls with a job's id, the state and its output.
ripple = {}
do
  local spawn, effect = __ripple_spawn, __ripple_effect; __ripple_spawn, __ripple_effect = nil, nil
  local jobs = {}
  function ripple.run(cmd, on_done) local id = spawn(cmd); jobs[id] = on_done; return id end
  function ripple.notify(text) effect("notify", tostring(text)) end
  function ripple.open(target) effect("open", tostring(target)) end
  function ripple.copy(text) effect("copy", tostring(text)) end
  return function(id, state, out) local f = jobs[id]; jobs[id] = nil; if f then f(state, out) end end
end
"#;

/// How long one call into a plugin may run before it is stopped, so a loop can't freeze the Island.
const BUDGET: Duration = Duration::from_millis(250);
/// Deeper views are refused: a table that contains itself would otherwise recurse forever.
const MAX_DEPTH: usize = 32;
/// More nodes are refused: a table shared by its parent many times would otherwise grow without bound.
const MAX_NODES: usize = 2000;
/// Job ids are unique across plugins and reloads, so a job that ends after its plugin reloaded finds no callback.
static NEXT_JOB: AtomicU64 = AtomicU64::new(1);

/// What a plugin's view returned, ready to draw.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Node {
    Text { text: String, size: f32, weight: f32, dim: bool },
    Row { gap: f32, children: Vec<Node> },
    Col { gap: f32, children: Vec<Node> },
    /// `action` indexes the plugin's handlers from this render.
    Button { key: String, label: String, #[serde(skip)] action: usize },
    Bar { value: f32 },
    Toggle { key: String, label: String, on: bool, #[serde(skip)] action: usize },
    /// A one-line field; its text lives in `Plugin::inputs` under `key`.
    Input { key: String, placeholder: String, value: Option<String>, #[serde(skip)] change: Option<usize>, #[serde(skip)] submit: Option<usize> },
    Icon { name: String, size: f32 },
    Spacer,
}

/// What a plugin asked the Island to do; the Island takes them after each call.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Effect {
    /// Run `argv` off the UI thread, then `Plugins::finish` with its output.
    Run { id: u64, argv: Vec<String> },
    Notify { text: String },
    Open { target: String },
    Copy { text: String },
}

/// A field's edit, the `value` the view last gave it, and the text `on_change` last saw.
#[derive(Default)]
pub struct Input { pub edit: TextEdit, given: Option<String>, seen: String }

/// Where a `persist = true` plugin's state is saved, and the JSON last written there.
struct Saved { path: PathBuf, last: String }

pub struct Plugin {
    pub name: String,
    path: PathBuf,
    modified: Option<SystemTime>,
    lua: Lua,
    /// The table the file returned; None when it failed to load.
    spec: Option<Table>,
    pub view: Option<Node>,
    /// The first error; it stays until the file is loaded again.
    pub error: Option<String>,
    handlers: Vec<Function>,
    /// The prelude's `finish(id, state, out)`, which runs a finished job's callback.
    finish: Option<Function>,
    pub inputs: HashMap<String, Input>,
    saved: Option<Saved>,
}

impl Plugin {
    /// `store` is the file a `persist = true` plugin saves its state to; None saves nothing.
    pub fn load(path: PathBuf, modified: Option<SystemTime>, source: &str, store: Option<PathBuf>) -> Plugin {
        let name = path.file_stem().map_or("plugin".into(), |s| s.to_string_lossy().into_owned());
        let mut p = Plugin { name, path, modified, lua: Lua::new(), spec: None, view: None, error: None, handlers: Vec::new(), finish: None, inputs: HashMap::new(), saved: None };
        let deadline = Instant::now() + BUDGET;
        let spec = guarded(&p.lua, deadline, || {
            p.lua.set_memory_limit(64 << 20)?;
            p.lua.set_app_data(Vec::<Effect>::new());
            let g = p.lua.globals();
            g.set("__ripple_expired", p.lua.create_function(|lua, ()| Ok(lua.app_data_ref::<Instant>().is_some_and(|d| Instant::now() > *d)))?)?;
            g.set("__ripple_spawn", p.lua.create_function(|lua, cmd: Value| {
                let argv = match cmd {
                    Value::String(s) => crate::command::tokenize_args(&s.to_str()?),
                    Value::Table(t) => t.sequence_values::<String>().collect::<mlua::Result<_>>()?,
                    other => return Err(mlua::Error::runtime(format!("ripple.run takes a command string or an argv table, not a {}", other.type_name()))),
                };
                if argv.is_empty() { return Err(mlua::Error::runtime("ripple.run has no command")) }
                let id = NEXT_JOB.fetch_add(1, Ordering::Relaxed);
                effect(lua, Effect::Run { id, argv });
                Ok(id)
            })?)?;
            g.set("__ripple_effect", p.lua.create_function(|lua, (kind, text): (String, String)| {
                effect(lua, match kind.as_str() { "notify" => Effect::Notify { text }, "open" => Effect::Open { target: text }, _ => Effect::Copy { text } });
                Ok(())
            })?)?;
            p.finish = Some(p.lua.load(PRELUDE).eval::<Function>()?);
            p.lua.load(source).set_name(format!("@{}", p.path.display())).eval::<Table>()
        });
        match spec {
            Ok(spec) => {
                if let Ok(name) = spec.get::<String>("name") { p.name = name }
                if let (Some(path), Ok(Some(true))) = (store, spec.get::<Option<bool>>("persist")) {
                    // A missing file is a first run; a broken one is reported and replaced by the next save.
                    let last = std::fs::read_to_string(&path).unwrap_or_default();
                    if !last.is_empty() {
                        let state = serde_json::from_str::<serde_json::Value>(&last).map_err(mlua::Error::external).and_then(|v| p.lua.to_value(&v));
                        match state.and_then(|v| spec.set("state", v)) {
                            Ok(()) => {}
                            Err(e) => p.fail(mlua::Error::runtime(format!("saved state {}: {e}", path.display()))),
                        }
                    }
                    p.saved = Some(Saved { path, last });
                }
                p.spec = Some(spec);
                p.render(deadline);
            }
            Err(e) => p.fail(e),
        }
        p
    }

    fn read(path: PathBuf, modified: Option<SystemTime>, store: Option<PathBuf>) -> Plugin {
        match std::fs::read_to_string(&path) {
            // Lua's load skips no byte-order mark, and some Windows editors write one.
            Ok(source) => Plugin::load(path, modified, source.strip_prefix('\u{feff}').unwrap_or(&source), store),
            // An empty panel that loads cleanly, so the read error is the one shown.
            Err(e) => { let mut p = Plugin::load(path, modified, r#"return { view = function() return "" end }"#, None); p.fail(mlua::Error::runtime(e)); p }
        }
    }

    /// The effects the plugin asked for since the last take, oldest first.
    pub fn take_effects(&mut self) -> Vec<Effect> {
        self.lua.app_data_mut::<Vec<Effect>>().map(|mut v| std::mem::take(&mut *v)).unwrap_or_default()
    }

    fn fail(&mut self, e: mlua::Error) {
        eprintln!("Ripple: plugin {}: {e}", self.name);
        self.error.get_or_insert(e.to_string());
    }

    fn call(&mut self, name: &str, deadline: Instant) {
        let Some(spec) = &self.spec else { return };
        let r = guarded(&self.lua, deadline, || match spec.get::<Option<Function>>(name)? {
            Some(f) => f.call::<()>(spec.get::<Value>("state")?),
            None => Ok(()),
        });
        if let Err(e) = r { self.fail(e) }
    }

    /// Runs `view(state)` again; a failed view keeps the last good one.
    fn render(&mut self, deadline: Instant) {
        let Some(spec) = &self.spec else { return };
        let mut handlers = Vec::new();
        let view = guarded(&self.lua, deadline, || {
            let view: Function = spec.get("view").map_err(|_| mlua::Error::runtime("the plugin's table has no view function"))?;
            node(&view.call::<Value>(spec.get::<Value>("state")?)?, &mut handlers, 0, &mut 0)
        });
        match view {
            Ok(view) => {
                self.sync_inputs(&view);
                (self.view, self.handlers) = (Some(view), handlers);
            }
            Err(e) => self.fail(e),
        }
        self.save();
    }

    /// Keeps one edit per field the view shows; a `value` the view changes replaces the text.
    fn sync_inputs(&mut self, view: &Node) {
        let mut shown = Vec::new();
        walk(view, &mut |n| if let Node::Input { key, value, .. } = n { shown.push((key.clone(), value.clone())) });
        self.inputs.retain(|k, _| shown.iter().any(|(s, _)| s == k));
        for (key, value) in shown {
            let input = self.inputs.entry(key).or_default();
            if value != input.given {
                // A value that echoes the typed text leaves the edit alone, so the caret stays put.
                if let Some(v) = value.as_ref().filter(|v| **v != input.edit.text) { input.edit.replace_text(v.clone()); input.seen = v.clone() }
                input.given = value;
            }
        }
    }

    /// Writes the state when it changed since the last save. Functions in it aren't saved.
    fn save(&mut self) {
        let (Some(saved), Some(spec)) = (&mut self.saved, &self.spec) else { return };
        let json = spec.get::<Value>("state").and_then(|v| serde_json::to_string(&v.to_serializable().deny_unsupported_types(false)).map_err(mlua::Error::external));
        let r = json.and_then(|json| {
            if json != saved.last { crate::paths::write_atomic(&saved.path, &json).map_err(mlua::Error::external)?; saved.last = json }
            Ok(())
        });
        if let Err(e) = r { self.fail(mlua::Error::runtime(format!("saving state: {e}"))) }
    }

    /// Runs handler `action` with the state and `arg` (a field's text), then the view.
    fn handle(&mut self, action: usize, arg: Option<String>) {
        let (Some(f), Some(spec)) = (self.handlers.get(action).cloned(), &self.spec) else { return };
        // The handler and the view after it share one budget.
        let deadline = Instant::now() + BUDGET;
        if let Err(e) = guarded(&self.lua, deadline, || f.call::<()>((spec.get::<Value>("state")?, arg))) { self.fail(e) }
        self.render(deadline);
    }

    pub fn press(&mut self, action: usize) { self.handle(action, None) }

    /// Return in field `key`: its `on_submit` with the text.
    pub fn submit(&mut self, key: &str) {
        let mut action = None;
        if let Some(v) = &self.view { walk(v, &mut |n| if let Node::Input { key: k, submit: Some(a), .. } = n { if k == key { action = Some(*a) } }) }
        // Submitted text counts as seen, so a later `changed` doesn't replay it over what `on_submit` set,
        // and the view's value applies afresh, so `on_submit` can clear a field whose value it never changed.
        let text = self.inputs.get_mut(key).map(|i| { i.seen = i.edit.text.clone(); i.given = None; i.seen.clone() });
        if let Some(a) = action { self.handle(a, text) }
    }

    /// Calls `on_change` for each field whose text changed since it last ran. True if any did.
    pub fn changed(&mut self) -> bool {
        let mut any = false;
        // One field at a time: each handler renders again, which can move the others' actions.
        loop {
            let mut next = None;
            if let Some(v) = &self.view {
                walk(v, &mut |n| if let Node::Input { key, change: Some(a), .. } = n {
                    if next.is_none() && self.inputs.get(key).is_some_and(|i| i.edit.text != i.seen) { next = Some((key.clone(), *a)) }
                })
            }
            let Some((key, action)) = next else { return any };
            let input = self.inputs.get_mut(&key).expect("found above");
            input.seen = input.edit.text.clone();
            let text = input.seen.clone();
            self.handle(action, Some(text));
            any = true;
        }
    }

    /// A job's output (trimmed stdout, None if it failed) reaches its `ripple.run` callback.
    pub fn finish(&mut self, id: u64, out: Option<String>) {
        let (Some(f), Some(spec)) = (self.finish.clone(), &self.spec) else { return };
        let deadline = Instant::now() + BUDGET;
        if let Err(e) = guarded(&self.lua, deadline, || f.call::<()>((id, spec.get::<Value>("state")?, out))) { self.fail(e) }
        self.render(deadline);
    }

    /// The handler of the button or toggle with this key, as `--test-plugin --press` finds it.
    pub fn press_key(&mut self, key: &str) -> bool {
        let mut action = None;
        if let Some(v) = &self.view {
            walk(v, &mut |n| match n {
                Node::Button { key: k, action: a, .. } | Node::Toggle { key: k, action: a, .. } if k == key && action.is_none() => action = Some(*a),
                _ => {}
            })
        }
        let Some(action) = action else { return false };
        self.press(action);
        true
    }

    /// Types `text` into field `key`, as a user would, then runs its `on_change`.
    pub fn type_key(&mut self, key: &str, text: &str) -> bool {
        let Some(input) = self.inputs.get_mut(key) else { return false };
        input.edit.replace_text(text.to_string());
        self.changed();
        true
    }

    /// Once a second while the panel shows: `tick(state)`, then the view again. True if it changed.
    pub fn tick(&mut self) -> bool {
        let before = (self.view.clone(), self.error.clone());
        let deadline = Instant::now() + BUDGET;
        self.call("tick", deadline);
        self.render(deadline);
        before != (self.view.clone(), self.error.clone())
    }
}

fn effect(lua: &Lua, e: Effect) {
    if let Some(mut v) = lua.app_data_mut::<Vec<Effect>>() { v.push(e) }
}

/// Calls `f` on `n` and every node under it.
fn walk(n: &Node, f: &mut impl FnMut(&Node)) {
    f(n);
    if let Node::Row { children, .. } | Node::Col { children, .. } = n { for c in children { walk(c, f) } }
}

fn guarded<R>(lua: &Lua, deadline: Instant, f: impl FnOnce() -> mlua::Result<R>) -> mlua::Result<R> {
    lua.set_app_data(deadline);
    lua.set_global_hook(HookTriggers::new().every_nth_instruction(10_000), move |_, _| {
        if Instant::now() > deadline { Err(mlua::Error::runtime(format!("stopped after {} ms", BUDGET.as_millis()))) } else { Ok(VmState::Continue) }
    })?;
    let r = f();
    lua.remove_global_hook();
    r
}

/// A number field, or `default` when it is missing or not finite, kept within `lo..=hi`.
fn num(t: &Table, field: &str, default: f32, lo: f32, hi: f32) -> mlua::Result<f32> {
    Ok(t.get::<Option<f32>>(field)?.filter(|v| v.is_finite()).unwrap_or(default).clamp(lo, hi))
}

fn node(v: &Value, handlers: &mut Vec<Function>, depth: usize, count: &mut usize) -> mlua::Result<Node> {
    if depth > MAX_DEPTH { return Err(mlua::Error::runtime(format!("a view nests deeper than {MAX_DEPTH}"))) }
    *count += 1;
    if *count > MAX_NODES { return Err(mlua::Error::runtime(format!("a view has more than {MAX_NODES} nodes"))) }
    let t = match v {
        Value::Table(t) => t,
        Value::String(s) => return Ok(Node::Text { text: s.to_string_lossy(), size: 14.0, weight: 400.0, dim: false }),
        other => return Err(mlua::Error::runtime(format!("a view returns ui.* nodes, not a {}", other.type_name()))),
    };
    let children = |handlers: &mut Vec<Function>, count: &mut usize| t.sequence_values::<Value>().map(|c| node(&c?, handlers, depth + 1, count)).collect::<mlua::Result<Vec<_>>>();
    let kind: String = t.get("type").map_err(|_| mlua::Error::runtime("a view node has no type; build it with ui.*"))?;
    Ok(match kind.as_str() {
        "text" => Node::Text {
            text: t.get("text")?,
            size: num(t, "size", 14.0, 6.0, 64.0)?,
            weight: num(t, "weight", 400.0, 100.0, 900.0)?,
            dim: t.get::<Option<bool>>("dim")?.unwrap_or(false),
        },
        "row" => Node::Row { gap: num(t, "gap", 8.0, 0.0, 64.0)?, children: children(handlers, count)? },
        "col" => Node::Col { gap: num(t, "gap", 6.0, 0.0, 64.0)?, children: children(handlers, count)? },
        "button" => {
            let label: String = t.get("label")?;
            handlers.push(t.get("on_press").map_err(|_| mlua::Error::runtime(format!("button {label:?} has no on_press function")))?);
            Node::Button { key: t.get::<Option<String>>("key")?.unwrap_or_else(|| label.clone()), label, action: handlers.len() - 1 }
        }
        "bar" => Node::Bar { value: num(t, "value", 0.0, 0.0, 1.0)? },
        "toggle" => {
            let label: String = t.get("label")?;
            handlers.push(t.get("on_press").map_err(|_| mlua::Error::runtime(format!("toggle {label:?} has no on_press function")))?);
            Node::Toggle { key: t.get::<Option<String>>("key")?.unwrap_or_else(|| label.clone()), on: t.get::<Option<bool>>("on")?.unwrap_or(false), label, action: handlers.len() - 1 }
        }
        "input" => {
            let mut handler = |field: &str| -> mlua::Result<Option<usize>> {
                Ok(t.get::<Option<Function>>(field)?.map(|f| { handlers.push(f); handlers.len() - 1 }))
            };
            let (change, submit) = (handler("on_change")?, handler("on_submit")?);
            Node::Input {
                key: t.get("key")?,
                placeholder: t.get::<Option<String>>("placeholder")?.unwrap_or_default(),
                value: t.get::<Option<String>>("value")?,
                change,
                submit,
            }
        }
        "icon" => {
            let name: String = t.get("name")?;
            if crate::icons::named(&name).is_none() { return Err(mlua::Error::runtime(format!("unknown icon {name:?}"))) }
            Node::Icon { name, size: num(t, "size", 16.0, 6.0, 64.0)? }
        }
        "spacer" => Node::Spacer,
        other => return Err(mlua::Error::runtime(format!("unknown node type {other:?}"))),
    })
}

#[derive(Default)]
pub struct Plugins {
    pub list: Vec<Plugin>,
    pub open: usize,
    /// A folder of `.lua` files, or one file.
    pub dir: Option<PathBuf>,
    /// Where `persist = true` plugins save their state, one `<file name>.json` each.
    data: Option<PathBuf>,
    /// The edit a field binds to while no plugin is open, so a stale binding has one.
    spare: TextEdit,
}

impl Plugins {
    /// `RIPPLE_PLUGINS` (a folder or one `.lua` file) overrides `<config folder>/plugins`, and
    /// then nothing is saved, so renders and tests start from each plugin's own state.
    pub fn new(config_dir: Option<&Path>) -> Plugins {
        let (dir, data) = match std::env::var_os("RIPPLE_PLUGINS") {
            Some(d) => (Some(PathBuf::from(d)), None),
            None => (config_dir.map(|d| d.join("plugins")).inspect(|d| { let _ = std::fs::create_dir_all(d); }), config_dir.map(|d| d.join("plugin-data"))),
        };
        let mut plugins = Plugins { dir, data, ..Default::default() };
        plugins.scan();
        plugins
    }

    /// Loads new, changed and failed files and drops deleted ones; the rest keep their state.
    pub fn scan(&mut self) {
        let Some(dir) = &self.dir else { return };
        let mut files: Vec<PathBuf> = if dir.is_file() { vec![dir.clone()] } else {
            std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "lua")).collect()
        };
        files.sort();
        let shown = self.list.get(self.open).map(|p| p.path.clone());
        let mut old = std::mem::take(&mut self.list);
        self.list = files.into_iter().map(|path| {
            let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
            // A plugin with an error loads again, so a one-off failure clears when the tab reopens.
            match old.iter().position(|p| p.path == path && p.modified == modified && p.error.is_none()) {
                Some(i) => old.swap_remove(i),
                None => {
                    let store = self.data.as_ref().zip(path.file_stem()).map(|(d, stem)| d.join(format!("{}.json", stem.to_string_lossy())));
                    Plugin::read(path, modified, store)
                }
            }
        }).collect();
        // The plugin shown stays shown when files are added or removed around it.
        self.open = shown.and_then(|s| self.list.iter().position(|p| p.path == s)).unwrap_or(self.open).min(self.list.len().saturating_sub(1));
    }

    pub fn press(&mut self, action: usize) {
        if let Some(p) = self.list.get_mut(self.open) { p.press(action) }
    }

    pub fn tick(&mut self) -> bool {
        self.list.get_mut(self.open).is_some_and(Plugin::tick)
    }

    pub fn submit(&mut self, key: &str) {
        if let Some(p) = self.list.get_mut(self.open) { p.submit(key) }
    }

    /// The open plugin's field `key`, for the text input bound to it.
    pub fn edit(&mut self, key: &str) -> &mut TextEdit {
        match self.list.get_mut(self.open) {
            Some(p) => &mut p.inputs.entry(key.to_string()).or_default().edit,
            None => &mut self.spare,
        }
    }

    /// Every frame: `on_change` for fields typed in, then every plugin's effects with its path. True if a view changed.
    pub fn poll(&mut self) -> (bool, Vec<(PathBuf, Effect)>) {
        let changed = self.list.get_mut(self.open).is_some_and(Plugin::changed);
        let effects = self.list.iter_mut().flat_map(|p| { let path = p.path.clone(); p.take_effects().into_iter().map(move |e| (path.clone(), e)) }).collect();
        (changed, effects)
    }

    /// A job ended: its output goes to the plugin at `path`, if that plugin is still loaded.
    pub fn finish(&mut self, path: &Path, id: u64, out: Option<String>) {
        if let Some(p) = self.list.iter_mut().find(|p| p.path == path) { p.finish(id, out) }
    }
}

/// A `ripple.run` job: trimmed stdout, or None when it failed or ran too long. Blocks.
pub fn run(argv: &[String]) -> Option<String> {
    let (program, args) = argv.split_first()?;
    crate::system::output(program, &args.iter().map(String::as_str).collect::<Vec<_>>())
}

const USAGE: &str = "usage: ripple --test-plugin FILE [--press KEY | --type KEY TEXT | --submit KEY | --tick | --wait]...";

/// `ripple --test-plugin FILE [steps]...`: loads one plugin without a window, runs the steps in
/// order and prints its name, error, view and effects as JSON. Fails if it has an error.
/// `--wait` runs the `ripple.run` jobs asked for so far, here and in order, and delivers their output.
pub fn test_cli(args: &[String]) -> i32 {
    let Some(file) = args.first() else { eprintln!("{USAGE}"); return 2 };
    let mut p = Plugin::read(file.into(), None, None);
    let mut effects = p.take_effects();
    let mut steps = args[1..].iter();
    while let Some(step) = steps.next() {
        let rest = steps.as_slice();
        match (step.as_str(), rest.first(), rest.get(1)) {
            ("--tick", ..) => { p.tick(); }
            ("--wait", ..) => {
                for e in std::mem::take(&mut effects) {
                    match e { Effect::Run { id, argv } => p.finish(id, run(&argv)), other => effects.push(other) }
                }
            }
            ("--press", Some(key), _) => {
                steps.next();
                if !p.press_key(key) { p.error.get_or_insert(format!("no button with key {key:?}")); }
            }
            ("--submit", Some(key), _) => {
                steps.next();
                if !p.inputs.contains_key(key.as_str()) { p.error.get_or_insert(format!("no field with key {key:?}")); }
                p.submit(key);
            }
            ("--type", Some(key), Some(text)) => {
                steps.nth(1);
                if !p.type_key(key, text) { p.error.get_or_insert(format!("no field with key {key:?}")); }
            }
            _ => { eprintln!("unknown step {step}: {USAGE}"); return 2 }
        }
        effects.extend(p.take_effects());
    }
    let out = serde_json::json!({ "name": p.name, "error": p.error, "view": p.view, "effects": effects });
    println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
    if p.error.is_some() { 1 } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plugin(source: &str) -> Plugin { Plugin::load("test.lua".into(), None, source, None) }

    #[test]
    fn example_counts_and_renders() {
        let mut p = plugin(include_str!("../plugins/example.lua"));
        assert_eq!(p.error, None);
        assert_eq!(p.name, "Focus Timer");
        let label = |p: &Plugin| format!("{:?}", p.view);
        assert!(label(&p).contains("\"Start\"") && p.press_key("start"));
        assert!(label(&p).contains("\"Pause\""), "the press ran the handler and rendered again");
        assert!(!p.press_key("missing"));
        assert_eq!(p.error, None);
    }

    #[test]
    fn buttons_change_state() {
        let mut p = plugin(r#"return { state = { n = 0 }, view = function(s) return ui.row { ui.text(s.n), ui.button("+", function(s) s.n = s.n + 1 end) } end }"#);
        assert!(p.press_key("+") && p.press_key("+"));
        let Some(Node::Row { children, .. }) = &p.view else { panic!("{:?}", p.view) };
        assert_eq!(children[0], Node::Text { text: "2".into(), size: 14.0, weight: 400.0, dim: false });
    }

    #[test]
    fn errors_show_and_loops_stop() {
        assert!(plugin("return 1").error.is_some(), "not a table");
        assert!(plugin("return {}").error.unwrap().contains("no view"));
        assert!(plugin(r#"return { view = function() return { type = "nope" } end }"#).error.unwrap().contains("nope"));
        let mut p = plugin(r#"return { view = function() return ui.button("x", function() while true do end end) end }"#);
        p.press_key("x");
        assert!(p.error.unwrap().contains("stopped"));
        let mut p = plugin(r#"return { view = function() return ui.button("x", function()
            while true do pcall(function() while true do end end); coroutine.resume(coroutine.create(function() while true do end end)); load(function() while true do end end) end
        end) end }"#);
        p.press_key("x");
        assert!(p.error.unwrap().contains("stopped"), "pcall can't catch the stop");
        assert!(plugin(r#"return { view = function() local ok = pcall(error, "x"); return ui.text(tostring(ok)) end }"#).error.is_none(), "pcall still works in time");
        assert!(plugin(r#"return { view = function() return ui.text(tostring(load("return 1")())) end }"#).error.is_none(), "load still works");
        assert!(plugin("return { view = function() local t = ui.row {}; t[1] = t; return t end }").error.unwrap().contains("deeper"));
        assert!(plugin(r#"return { view = function() local a = ui.text("x"); for _ = 1, 32 do a = ui.row { a, a } end; return a end }"#).error.unwrap().contains("more than"));
    }

    #[test]
    fn unreadable_files_say_why() {
        let path = std::env::temp_dir().join(format!("ripple-plugin-{}.lua", std::process::id()));
        std::fs::write(&path, [0xff, 0xfe]).unwrap();
        let p = Plugin::read(path.clone(), None, None);
        let _ = std::fs::remove_file(&path);
        assert!(p.error.unwrap().contains("UTF-8"));
        std::fs::write(&path, "\u{feff}return { view = function() return 'ok' end }").unwrap();
        let p = Plugin::read(path.clone(), None, None);
        let _ = std::fs::remove_file(&path);
        assert_eq!(p.error, None, "a byte-order mark is skipped");
    }

    #[test]
    fn finalizers_are_refused() {
        let p = plugin(r#"local keep = setmetatable({}, { __gc = function() while true do end end }); return { view = function() return "x" end }"#);
        assert!(p.error.as_deref().unwrap_or("").contains("__gc"));
        drop(p);
        assert!(plugin(r#"return { view = function() return tostring(getmetatable(setmetatable({}, { __index = {} })) ~= nil) end }"#).error.is_none());
    }

    #[test]
    fn rescans_keep_the_plugin_shown() {
        let dir = std::env::temp_dir().join(format!("ripple-plugins-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let view = "return { view = function() return 'x' end }";
        std::fs::write(dir.join("b.lua"), view).unwrap();
        let mut plugins = Plugins { dir: Some(dir.clone()), ..Default::default() };
        plugins.scan();
        std::fs::write(dir.join("a.lua"), view).unwrap();
        plugins.scan();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(plugins.list[plugins.open].name, "b");
    }

    #[test]
    fn ticks_report_changes_only() {
        let mut p = plugin(r#"return { state = { n = 0 }, tick = function(s) s.n = s.n + 1 end, view = function(s) return ui.col { ui.text(s.n), ui.bar(0/0) } end }"#);
        assert!(p.tick(), "the count changed");
        let mut q = plugin(r#"return { view = function() return ui.bar(0/0) end }"#);
        assert!(!q.tick(), "a NaN bar is drawn empty, not changed every second");
    }

    #[test]
    fn todo_example_loads() {
        let mut p = plugin(include_str!("../plugins/todo.lua"));
        assert_eq!(p.error, None);
        assert!(p.type_key("draft", "Milk"));
        p.submit("draft");
        assert!(p.press_key("item-1") && p.press_key("Clear done"));
        assert_eq!(p.take_effects().last(), Some(&Effect::Notify { text: "1 task done".into() }));
        assert_eq!(p.error, None);
    }

    #[test]
    fn jobs_and_effects_reach_the_island() {
        let mut p = plugin(r#"return { state = {}, view = function(s)
            return ui.col { ui.text(s.out or "-"), ui.button("go", function(s)
                ripple.run({ "echo", "hi" }, function(s, out) s.out = out end)
                ripple.notify("done"); ripple.open("https://example.com"); ripple.copy(42)
            end) }
        end }"#);
        assert!(p.press_key("go"));
        let effects = p.take_effects();
        let Some(Effect::Run { id, argv }) = effects.first().cloned() else { panic!("{effects:?}") };
        assert_eq!(argv, ["echo", "hi"]);
        assert_eq!(effects[1..], [Effect::Notify { text: "done".into() }, Effect::Open { target: "https://example.com".into() }, Effect::Copy { text: "42".into() }]);
        assert!(p.take_effects().is_empty(), "taken once");
        p.finish(id, Some("hi".into()));
        p.finish(id, Some("again".into()));
        let Some(Node::Col { children, .. }) = &p.view else { panic!("{:?}", p.view) };
        assert_eq!(children[0], Node::Text { text: "hi".into(), size: 14.0, weight: 400.0, dim: false }, "a job calls back once");
        assert!(plugin(r#"ripple.run(""); return { view = function() return "" end }"#).error.unwrap().contains("no command"));
        let mut q = plugin(r#"ripple.run('git -C "my dir" status'); return { view = function() return "" end }"#);
        let Some(Effect::Run { argv, .. }) = q.take_effects().pop() else { panic!() };
        assert_eq!(argv, ["git", "-C", "my dir", "status"], "a string splits into words without a shell");
    }

    #[test]
    fn fields_toggles_and_icons() {
        let src = r#"return { state = { q = "", items = {}, on = false }, view = function(s)
            local rows = { ui.input("q", { value = s.q, placeholder = "Add",
                on_change = function(s, t) s.typed = t end,
                on_submit = function(s, t) table.insert(s.items, t); s.q = t; s.q = "" end }) }
            table.insert(rows, ui.text((s.typed or "") .. "|" .. table.concat(s.items, ",")))
            table.insert(rows, ui.toggle("On", s.on, function(s) s.on = not s.on end))
            table.insert(rows, ui.row { ui.icon("timer"), ui.spacer() })
            return ui.col(rows)
        end }"#;
        let mut p = plugin(src);
        assert_eq!(p.error, None);
        let line = |p: &Plugin| { let Some(Node::Col { children, .. }) = &p.view else { panic!() }; children[1].clone() };
        assert!(p.type_key("q", "milk"));
        assert_eq!(line(&p), Node::Text { text: "milk|".into(), size: 14.0, weight: 400.0, dim: false });
        assert!(!p.changed(), "on_change runs once per edit");
        p.submit("q");
        assert_eq!(line(&p), Node::Text { text: "milk|milk".into(), size: 14.0, weight: 400.0, dim: false });
        assert_eq!(p.inputs["q"].edit.text, "", "on_submit set the value to empty, which clears the field");
        assert!(p.press_key("On"));
        assert!(format!("{:?}", p.view).contains("on: true"));
        assert!(plugin(r#"return { view = function() return ui.icon("nope") end }"#).error.unwrap().contains("unknown icon"));
        // Return pressed before the frame that runs on_change: the submit wins.
        let mut t = plugin(include_str!("../plugins/todo.lua"));
        t.inputs.get_mut("draft").unwrap().edit.replace_text("Eggs".to_string());
        t.submit("draft");
        assert!(!t.changed(), "on_change doesn't replay the submitted text");
        assert_eq!(t.inputs["draft"].edit.text, "", "on_submit cleared the field");
    }

    #[test]
    fn persisted_state_survives_a_reload() {
        let path = std::env::temp_dir().join(format!("ripple-plugin-state-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let src = r#"return { persist = true, state = { n = 0, list = {} }, view = function(s) return ui.button(tostring(s.n), function(s) s.n = s.n + 1; s.f = print end, { key = "+" }) end }"#;
        let mut p = Plugin::load("test.lua".into(), None, src, Some(path.clone()));
        p.press_key("+");
        assert_eq!(p.error, None);
        let p = Plugin::load("test.lua".into(), None, src, Some(path.clone()));
        assert!(format!("{:?}", p.view).contains("label: \"1\""), "{:?}", p.view);
        std::fs::write(&path, "{").unwrap();
        let p = Plugin::load("test.lua".into(), None, src, Some(path.clone()));
        let _ = std::fs::remove_file(&path);
        assert!(p.error.unwrap().contains("saved state"));
        assert!(format!("{:?}", p.view).contains("label: \"0\""), "a broken file starts from the file's state");
    }
}
