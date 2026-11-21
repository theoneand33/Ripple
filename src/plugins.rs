//! Lua plugins: each `.lua` file in the plugins folder is a panel on the Plugins tab.
//! PLUGINS.md documents the API and how to test a plugin.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use mlua::{Function, HookTriggers, Lua, Table, Value, VmState};
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
"#;

/// How long one call into a plugin may run before it is stopped, so a loop can't freeze the Island.
const BUDGET: Duration = Duration::from_millis(250);
/// Deeper views are refused: a table that contains itself would otherwise recurse forever.
const MAX_DEPTH: usize = 32;
/// More nodes are refused: a table shared by its parent many times would otherwise grow without bound.
const MAX_NODES: usize = 2000;

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
}

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
}

impl Plugin {
    pub fn load(path: PathBuf, modified: Option<SystemTime>, source: &str) -> Plugin {
        let name = path.file_stem().map_or("plugin".into(), |s| s.to_string_lossy().into_owned());
        let mut p = Plugin { name, path, modified, lua: Lua::new(), spec: None, view: None, error: None, handlers: Vec::new() };
        let deadline = Instant::now() + BUDGET;
        let spec = guarded(&p.lua, deadline, || {
            p.lua.set_memory_limit(64 << 20)?;
            p.lua.globals().set("__ripple_expired", p.lua.create_function(|lua, ()| Ok(lua.app_data_ref::<Instant>().is_some_and(|d| Instant::now() > *d)))?)?;
            p.lua.load(PRELUDE).exec()?;
            p.lua.load(source).set_name(format!("@{}", p.path.display())).eval::<Table>()
        });
        match spec {
            Ok(spec) => {
                if let Ok(name) = spec.get::<String>("name") { p.name = name }
                p.spec = Some(spec);
                p.render(deadline);
            }
            Err(e) => p.fail(e),
        }
        p
    }

    fn read(path: PathBuf, modified: Option<SystemTime>) -> Plugin {
        match std::fs::read_to_string(&path) {
            // Lua's load skips no byte-order mark, and some Windows editors write one.
            Ok(source) => Plugin::load(path, modified, source.strip_prefix('\u{feff}').unwrap_or(&source)),
            // An empty panel that loads cleanly, so the read error is the one shown.
            Err(e) => { let mut p = Plugin::load(path, modified, r#"return { view = function() return "" end }"#); p.fail(mlua::Error::runtime(e)); p }
        }
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
            Ok(view) => (self.view, self.handlers) = (Some(view), handlers),
            Err(e) => self.fail(e),
        }
    }

    pub fn press(&mut self, action: usize) {
        let (Some(f), Some(spec)) = (self.handlers.get(action).cloned(), &self.spec) else { return };
        // The handler and the view after it share one budget.
        let deadline = Instant::now() + BUDGET;
        if let Err(e) = guarded(&self.lua, deadline, || f.call::<()>(spec.get::<Value>("state")?)) { self.fail(e) }
        self.render(deadline);
    }

    /// The handler of the button with this key, as `--test-plugin --press` finds it.
    fn press_key(&mut self, key: &str) -> bool {
        fn find(n: &Node, key: &str) -> Option<usize> {
            match n {
                Node::Button { key: k, action, .. } if k == key => Some(*action),
                Node::Row { children, .. } | Node::Col { children, .. } => children.iter().find_map(|c| find(c, key)),
                _ => None,
            }
        }
        let Some(action) = self.view.as_ref().and_then(|v| find(v, key)) else { return false };
        self.press(action);
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
        other => return Err(mlua::Error::runtime(format!("unknown node type {other:?}"))),
    })
}

#[derive(Default)]
pub struct Plugins {
    pub list: Vec<Plugin>,
    pub open: usize,
    /// A folder of `.lua` files, or one file.
    pub dir: Option<PathBuf>,
}

impl Plugins {
    /// `RIPPLE_PLUGINS` (a folder or one `.lua` file) overrides `<config folder>/plugins`.
    pub fn new(config_dir: Option<&Path>) -> Plugins {
        let dir = match std::env::var_os("RIPPLE_PLUGINS") {
            Some(d) => Some(PathBuf::from(d)),
            None => config_dir.map(|d| d.join("plugins")).inspect(|d| { let _ = std::fs::create_dir_all(d); }),
        };
        let mut plugins = Plugins { dir, ..Default::default() };
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
                None => Plugin::read(path, modified),
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
}

/// `ripple --test-plugin FILE [--press KEY | --tick]...`: loads one plugin without a window,
/// runs the steps in order and prints its name, error and view as JSON. Fails if it has an error.
pub fn test_cli(args: &[String]) -> i32 {
    let Some(file) = args.first() else { eprintln!("usage: ripple --test-plugin FILE [--press KEY | --tick]..."); return 2 };
    let mut p = Plugin::read(file.into(), None);
    let mut steps = args[1..].iter();
    while let Some(step) = steps.next() {
        match (step.as_str(), steps.as_slice().first()) {
            ("--tick", _) => { p.tick(); }
            ("--press", Some(key)) => {
                steps.next();
                if !p.press_key(key) { p.error.get_or_insert(format!("no button with key {key:?}")); }
            }
            _ => { eprintln!("unknown step {step}: use --press KEY or --tick"); return 2 }
        }
    }
    let out = serde_json::json!({ "name": p.name, "error": p.error, "view": p.view });
    println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
    if p.error.is_some() { 1 } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plugin(source: &str) -> Plugin { Plugin::load("test.lua".into(), None, source) }

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
        let p = Plugin::read(path.clone(), None);
        let _ = std::fs::remove_file(&path);
        assert!(p.error.unwrap().contains("UTF-8"));
        std::fs::write(&path, "\u{feff}return { view = function() return 'ok' end }").unwrap();
        let p = Plugin::read(path.clone(), None);
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
}
