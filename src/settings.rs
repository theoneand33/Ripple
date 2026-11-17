//! The Settings window. Every change saves at once and the Island picks it up.

use rune::{Color, Cursor, Cx, Drag, El, Phase, ScrollSpec, TextEdit, Tx, col, el, icon, row, text};
use rune_kit::{Button, ButtonVariant, IconButton, Kit, RadioGroup, RadioItem, Select, Switch, TextArea, TextInput};

use crate::config::{Config, POSITIONS, QuickApp, SEARCH_ENGINES, Store, T3Device, TABS, Workflow, default_model};
use crate::icons;
use crate::island::{Host, Screen, font, kit};

pub struct Settings {
    pub store: Store,
    pub cfg: Config,
    pub island: Option<Host>,
    pub screens: Vec<String>,
    /// The select that is open, by key.
    open: Option<&'static str>,
    /// The tab row being dragged, and how far it has moved.
    dragging: Option<(usize, f32)>,
    bg_color: TextEdit,
    text_color: TextEdit,
    bg_image: TextEdit,
    location: TextEdit,
    ai_model: TextEdit,
    api_key: TextEdit,
    new_app: TextEdit,
    /// The suggestion picked for `new_app`, with its own launch target.
    picked_app: Option<QuickApp>,
    /// Windows: installed apps, found once in the background.
    pub installed: Vec<QuickApp>,
    app_names: Vec<TextEdit>,
    t3_name: TextEdit,
    t3_url: TextEdit,
    t3_token: TextEdit,
    workflow_name: TextEdit,
    workflow_urls: TextEdit,
}

impl Settings {
    pub fn new(store: Store, island: Option<Host>, screens: &[Screen]) -> Settings {
        let cfg = store.get();
        let edit = |s: &str| TextEdit::new(s);
        Settings {
            bg_color: edit(&cfg.bg_color),
            text_color: edit(&cfg.text_color),
            bg_image: edit(&cfg.bg_image),
            location: edit(&cfg.location),
            ai_model: edit(&cfg.ai_model),
            api_key: TextEdit::new("").secure(),
            new_app: edit(""),
            picked_app: None,
            installed: Vec::new(),
            app_names: cfg.quick_apps.iter().map(|a| edit(&a.name)).collect(),
            t3_name: edit(""),
            t3_url: edit(""),
            t3_token: TextEdit::new("").secure(),
            workflow_name: edit(""),
            workflow_urls: edit(""),
            screens: screens.iter().map(|s| s.name.clone()).collect(),
            open: None,
            dragging: None,
            island,
            store,
            cfg,
        }
    }

    /// Saves a change and tells the Island.
    fn save(&mut self, f: impl FnOnce(&mut Config)) {
        self.store.update(f);
        self.cfg = self.store.get();
        // A theme rewrites both colours; the fields show what it set unless one is being typed in.
        for (edit, value) in [(&mut self.bg_color, &self.cfg.bg_color), (&mut self.text_color, &self.cfg.text_color)] {
            if !edit.focused && edit.text != *value { edit.replace_text(value) }
        }
        if let Some(island) = &self.island { island.update(|s, tx| s.reload(tx)) }
    }

    fn reset_app_names(&mut self) {
        self.app_names = self.cfg.quick_apps.iter().map(|a| TextEdit::new(&a.name)).collect();
    }

    fn suggestions(&self) -> Vec<QuickApp> {
        let q = self.new_app.text.trim().to_lowercase();
        if q.chars().count() < 2 || self.picked_app.is_some() { return Vec::new() }
        self.installed.iter().filter(|a| a.name.to_lowercase().contains(&q)).take(8).cloned().collect()
    }

    fn add_quick_app(&mut self) {
        let name = self.new_app.text.trim().to_string();
        if name.is_empty() { return }
        let entry = self.picked_app.take().unwrap_or(QuickApp { launch: name.clone(), name });
        self.save(|c| c.quick_apps.push(entry));
        self.new_app.clear();
        self.reset_app_names();
    }

    fn add_workflow(&mut self) {
        let name = self.workflow_name.text.trim().to_string();
        let urls: Vec<String> = self.workflow_urls.text.split(',').map(|u| u.trim().to_string()).filter(|u| !u.is_empty()).collect();
        if name.is_empty() || urls.is_empty() { return }
        self.save(|c| c.workflows.push(Workflow { name, urls }));
        self.workflow_name.clear();
        self.workflow_urls.clear();
    }

    fn t3_addable(&self) -> bool {
        let url = self.t3_url.text.trim();
        (url.starts_with("http://") || url.starts_with("https://")) && !self.t3_token.text.trim().is_empty()
    }

    fn add_t3_device(&mut self) {
        if !self.t3_addable() { return }
        let device = T3Device { name: self.t3_name.text.trim().into(), url: self.t3_url.text.trim().into(), token: self.t3_token.text.trim().into() };
        self.save(|c| c.t3_devices.push(device));
        self.t3_name.clear();
        self.t3_url.clear();
        self.t3_token.clear();
    }
}

fn toggle_open(key: &'static str) -> impl Fn(&mut Settings) + Clone {
    move |s: &mut Settings| s.open = if s.open == Some(key) { None } else { Some(key) }
}

fn close(s: &mut Settings) { s.open = None }

fn app_name_at(i: usize) -> impl for<'a> Fn(&'a mut Settings) -> &'a mut TextEdit {
    move |s| &mut s.app_names[i]
}

/// A select of fixed choices: `(value, label)` pairs, saving the chosen value through `set`.
#[allow(clippy::too_many_arguments)] // one call per setting reads better than a builder here
fn choice(s: &Settings, k: &Kit, layer: &mut Vec<El>, key: &'static str, label: &str, options: &[(&'static str, &'static str)], current: &str, set: fn(&mut Config, &str)) -> El {
    let labels: Vec<&str> = options.iter().map(|o| o.1).collect();
    let options = options.to_vec();
    Select::new(key, &labels).label(label).width(190.0)
        .current(options.iter().find(|o| o.0 == current).map(|o| o.1))
        .open(s.open == Some(key)).on_toggle(toggle_open(key)).on_close(close)
        .on_choose(move |s: &mut Settings, label: &str, _: &mut Tx| {
            s.open = None;
            if let Some(&(value, _)) = options.iter().find(|o| o.1 == label) { s.save(|c| set(c, value)) }
        })
        .build(k, layer)
}

fn switch(k: &Kit, label: &str, on: bool, flip: fn(&mut Config)) -> El {
    Switch::new(on).label(label).hide_label().on_toggle(move |s: &mut Settings| s.save(flip)).build(k)
}

fn setting(s: &Settings, label: &str, control: El) -> El {
    let fg = s.cfg.text().alpha(1.0);
    row().key(label.to_string()).w_full().min_h(40.0).gap(12.0).center_y().justify_between()
        .child(text(label).font(font(&s.cfg, 14.0, 500.0)).color(fg.alpha(0.85)).shrink())
        .child(control)
}

fn section(s: &Settings, key: &str, title: &str, rows: Vec<El>) -> El {
    let fg = s.cfg.text().alpha(1.0);
    rows.into_iter().fold(
        col().key(key.to_string()).w_full().gap(6.0)
            .child(text(title).font(font(&s.cfg, 13.0, 500.0)).color(fg.alpha(0.5)).mb(2.0)),
        El::child,
    )
}

/// A horizontal slider: `value` in `min..=max`, moved while dragging and saved on release.
fn slider(s: &Settings, key: &'static str, label: &str, (min, max, step): (f32, f32, f32), get: fn(&Config) -> f32, set: fn(&mut Config, f32)) -> El {
    let fg = s.cfg.text().alpha(1.0);
    let value = get(&s.cfg);
    let (w, knob) = (220.0, 16.0);
    let travel = w - knob;
    let t = ((value - min) / (max - min)).clamp(0.0, 1.0);
    let at = move |x: f32| {
        let v = min + ((x - knob / 2.0) / travel).clamp(0.0, 1.0) * (max - min);
        (v / step).round() * step
    };
    el().key(key).role("slider").label(label).focusable(true)
        .range_value(f64::from(value), f64::from(min), f64::from(max)).range_step(f64::from(step))
        .on_step(move |s: &mut Settings, steps, _| {
            let v = (get(&s.cfg) + steps as f32 * step).clamp(min, max);
            s.save(|c| set(c, v));
        })
        .size(w, 22.0)
        .on_drag(move |s: &mut Settings, d: &mut Drag, tx| {
            tx.instant();
            if d.phase == Phase::Down { d.data = get(&s.cfg) }
            if d.cancelled() { set(&mut s.cfg, d.data); return }
            set(&mut s.cfg, at(d.x));
            if d.phase == Phase::Up {
                let v = get(&s.cfg);
                s.save(|c| set(c, v));
            }
        })
        .child(el().size(w, 4.0).absolute(0.0, 9.0).radius(2.0).fill(fg.alpha(0.15)).a11y_hidden())
        .child(el().size(knob / 2.0 + t * travel, 4.0).absolute(0.0, 9.0).radius(2.0).fill(fg).a11y_hidden())
        .child(el().size(knob, knob).absolute(t * travel, 3.0).radius(knob / 2.0).fill(fg).shadow(1.0, 3.0, Color::BLACK.alpha(0.3)).a11y_hidden())
}

/// A tab row's height plus the gap to the next.
const TAB_ROW: f32 = 42.0;
const TAB_PITCH: f32 = TAB_ROW + 6.0;

/// Where a row dragged `dy` points from index `from` lands among `count` rows.
fn drop_index(from: usize, dy: f32, count: usize) -> usize {
    (from as f32 + (dy / TAB_PITCH).round()).clamp(0.0, count.saturating_sub(1) as f32) as usize
}

fn drag_tab(s: &mut Settings, from: usize, d: &mut Drag, tx: &mut Tx) {
    tx.instant();
    match d.phase {
        Phase::Down => s.dragging = None,
        Phase::Move => s.dragging = Some((from, d.dy)),
        Phase::Up => {
            s.dragging = None;
            if d.cancelled() { return }
            let to = drop_index(from, d.dy, s.cfg.tab_order.len());
            if to != from { s.save(|c| c.move_tab(from, to)) }
        }
    }
}

pub fn view(s: &Settings, cx: &mut Cx<Settings>) -> El {
    let k = kit(&s.cfg);
    let mut layer = Vec::new();
    let cfg = &s.cfg;
    let fg = cfg.text().alpha(1.0);
    let mut sections = Vec::new();

    // General
    let mut general = vec![setting(s, "Clock format", choice(s, &k, &mut layer, "clock", "Clock format", &[("12", "12-hour"), ("24", "24-hour")], if cfg.hour12 { "12" } else { "24" }, |c, v| c.hour12 = v == "12"))];
    if !cfg!(target_os = "macos") {
        general.push(setting(s, "Launch at login", switch(&k, "Launch at login", cfg.auto_launch, |c| {
            c.auto_launch = !c.auto_launch;
            let on = c.auto_launch;
            std::thread::spawn(move || crate::system::set_auto_launch(on));
        }).a11y_hidden()));
    }
    if !s.screens.is_empty() {
        let current = if s.screens.contains(&cfg.display) { cfg.display.clone() } else { s.screens[0].clone() };
        general.push(setting(s, "Display", Select::new("display", &s.screens).label("Display").width(190.0).current(Some(&current))
            .open(s.open == Some("display")).on_toggle(toggle_open("display")).on_close(close)
            .on_choose(|s: &mut Settings, name: &str, _: &mut Tx| { s.open = None; let name = name.to_string(); s.save(|c| c.display = name) })
            .build(&k, &mut layer)));
    }
    sections.push(section(s, "general", "General", general));

    // Tabs
    let mut tabs = vec![
        text("Drag to reorder, click the eye to hide").font(font(cfg, 12.0, 400.0)).color(fg.alpha(0.5)),
        setting(s, "Always open on the starred tab", switch(&k, "Always open on the starred tab", cfg.open_default_tab, |c| c.open_default_tab = !c.open_default_tab).a11y_hidden()),
    ];
    let last = cfg.tab_order.len().saturating_sub(1);
    for (i, &id) in cfg.tab_order.iter().enumerate() {
        let Some(def) = TABS.iter().find(|t| t.id == id) else { continue };
        let hidden = cfg.hidden_tabs.contains(&id);
        let is_default = cfg.default_tab == id;
        tabs.push(
            {
                let row = row().key(format!("tab-{id}")).w_full().h(TAB_ROW).px(10.0).gap(10.0).center_y().radius(10.0)
                    .fill(fg.alpha(0.03)).border(1.0, fg.alpha(if hidden { 0.05 } else { 0.08 }))
                    .cursor(Cursor::Grab)
                    .on_drag(move |s: &mut Settings, d: &mut Drag, tx| drag_tab(s, i, d, tx));
                match s.dragging {
                    // The dragged row follows the pointer over its neighbours.
                    Some((from, dy)) if from == i => row.offset(0.0, dy).raise().opacity(0.6).fill(fg.alpha(0.1)),
                    _ => row.opacity(if hidden { 0.45 } else { 1.0 }),
                }
            }
                .child(icon(&icons::GRIP, 16.0).color(fg.alpha(0.3)))
                .child(icon(icons::tab(id), 16.0).color(fg))
                .child(text(def.name).font(font(cfg, 14.0, 500.0)).color(fg).grow(1.0))
                .child(IconButton::new(if is_default { &icons::STAR_FILLED } else { &icons::STAR }).label(format!("Make {} the default tab", def.name))
                    .on_press(move |s: &mut Settings| s.save(|c| c.default_tab = id)).build(&k).opacity(if is_default { 1.0 } else { 0.4 }))
                .child(IconButton::new(if hidden { &icons::EYE_OFF } else { &icons::EYE }).label(format!("{} {}", if hidden { "Show" } else { "Hide" }, def.name))
                    .on_press(move |s: &mut Settings| s.save(|c| c.toggle_tab(id))).build(&k))
                .child(IconButton::new(&icons::CHEVRON_UP).label(format!("Move {} up", def.name)).disabled(i == 0)
                    .on_press(move |s: &mut Settings| s.save(|c| c.move_tab(i, i.saturating_sub(1)))).build(&k))
                .child(IconButton::new(&icons::CHEVRON_DOWN).label(format!("Move {} down", def.name)).disabled(i == last)
                    .on_press(move |s: &mut Settings| s.save(|c| c.move_tab(i, i + 1))).build(&k)),
        );
    }
    sections.push(section(s, "tabs", "Tabs", tabs));

    // Island style
    let mut style = vec![
        setting(s, "Notch mode", switch(&k, "Notch mode", cfg.notch_mode, |c| c.notch_mode = !c.notch_mode).a11y_hidden()),
        setting(s, "Theme", choice(s, &k, &mut layer, "theme", "Theme", &[("none", "Default"), ("sleek-black", "Sleek Black"), ("win95", "Windows 95"), ("glass", "Glass")], &cfg.theme, |c, v| c.set_theme(v))),
    ];
    if cfg.notch_mode {
        style.insert(1, setting(s, "Mini notch", switch(&k, "Mini notch", cfg.notch_mini, |c| c.notch_mini = !c.notch_mini).a11y_hidden()));
        // Auto size only sizes around a physical notch.
        if !cfg.notch_mini { style.insert(2, setting(s, "Auto size", switch(&k, "Auto size", cfg.notch_auto_size, |c| c.notch_auto_size = !c.notch_auto_size).a11y_hidden())) }
    }
    let position = POSITIONS.iter().chain([("free", "Free placement")].iter())
        .fold(RadioGroup::new().label("Position"), |group, &(value, label)| {
            group.item(RadioItem::new(label, cfg.position_mode == value).on_select(move |s: &mut Settings| s.save(|c| c.position_mode = value.into())))
        });
    style.push(col().key("position").w_full().gap(8.0).padding(14.0).radius(16.0).fill(fg.alpha(0.03)).border(1.0, fg.alpha(0.08)).child(position.build(&k)));
    if cfg.position_mode == "free" {
        style.push(setting(s, &format!("Position X ({:.1}%)", cfg.island_x), slider(s, "island-x", "Position X", (0.0, 100.0, 0.1), |c| c.island_x, |c, v| c.island_x = v)));
        style.push(setting(s, &format!("Position Y ({}px)", cfg.island_y.round()), slider(s, "island-y", "Position Y", (0.0, 500.0, 1.0), |c| c.island_y, |c, v| c.island_y = v)));
    }
    style.push(setting(s, "Island border", switch(&k, "Island border", cfg.island_border, |c| c.island_border = !c.island_border).a11y_hidden()));
    style.push(setting(s, "Hide when inactive", switch(&k, "Hide when inactive", cfg.hide_inactive, |c| c.hide_inactive = !c.hide_inactive).a11y_hidden()));
    sections.push(section(s, "style", "Island style", style));

    // Colours
    sections.push(section(s, "colors", "Colors and background", vec![
        setting(s, "Island color", TextInput::new(&s.bg_color).label("Island color").placeholder("#000000").width(Some(160.0))
            .on_edit(|s: &mut Settings| &mut s.bg_color).on_change(|s: &mut Settings, _| { let v = s.bg_color.text.clone(); s.save(|c| c.bg_color = v) }).build(&k)),
        setting(s, "Text color", TextInput::new(&s.text_color).label("Text color").placeholder("#FAFAFA").width(Some(160.0))
            .on_edit(|s: &mut Settings| &mut s.text_color).on_change(|s: &mut Settings, _| { let v = s.text_color.text.clone(); s.save(|c| c.text_color = v) }).build(&k)),
        col().w_full().gap(6.0)
            .child(text("Background image URL").font(font(cfg, 14.0, 500.0)).color(fg.alpha(0.85)))
            .child(TextInput::new(&s.bg_image).label("Background image URL").placeholder("https://...")
                .on_edit(|s: &mut Settings| &mut s.bg_image).on_change(|s: &mut Settings, _| { let v = s.bg_image.text.clone(); s.save(|c| c.bg_image = v) }).build(&k)),
    ]));

    // Features
    sections.push(section(s, "features", "Features", vec![
        setting(s, "Low battery alerts", switch(&k, "Low battery alerts", cfg.battery_alerts, |c| c.battery_alerts = !c.battery_alerts).a11y_hidden()),
        setting(s, "Standby mode", switch(&k, "Standby mode", cfg.standby, |c| c.standby = !c.standby).a11y_hidden()),
        setting(s, "Large standby mode", switch(&k, "Large standby mode", cfg.large_standby, |c| c.large_standby = !c.large_standby).a11y_hidden()),
        setting(s, "Show info when idle", switch(&k, "Show info when idle", cfg.show_info_idle, |c| c.show_info_idle = !c.show_info_idle).a11y_hidden()),
    ]));

    // Browser search
    let engines: Vec<(&str, &str)> = SEARCH_ENGINES.iter().map(|e| (e.0, e.1)).collect();
    sections.push(section(s, "search", "Browser Search", vec![
        setting(s, "Search engine", choice(s, &k, &mut layer, "engine", "Search engine", &engines, &cfg.search_engine, |c, v| c.search_engine = v.into())),
    ]));

    // Weather
    sections.push(section(s, "weather", "Weather", vec![
        setting(s, "Location", TextInput::new(&s.location).label("Location").placeholder("City, ST, Country").width(Some(200.0))
            .on_edit(|s: &mut Settings| &mut s.location).on_change(|s: &mut Settings, _| { let v = s.location.text.clone(); s.save(|c| c.location = v) }).build(&k)),
        setting(s, "Unit", choice(s, &k, &mut layer, "unit", "Unit", &[("f", "Fahrenheit (°F)"), ("c", "Celsius (°C)")], &cfg.weather_unit, |c, v| c.weather_unit = v.into())),
    ]));

    // Quick apps
    let mut apps = vec![
        row().key("add-app").w_full().gap(8.0).center_y()
            .child(TextInput::new(&s.new_app).label("Add app").placeholder("Add app, like Apple Music")
                .on_edit(|s: &mut Settings| &mut s.new_app)
                .on_change(|s: &mut Settings, _| s.picked_app = None)
                .on_submit(|s: &mut Settings, _| s.add_quick_app())
                .build(&k).grow(1.0))
            .child(IconButton::new(&icons::PLUS).label("Add app").on_press(Settings::add_quick_app).build(&k)),
    ];
    let suggestions = s.suggestions();
    if !suggestions.is_empty() {
        let list = suggestions.into_iter().enumerate().fold(col().key("suggestions").w_full().padding(4.0).radius(12.0).fill(fg.alpha(0.05)).border(1.0, fg.alpha(0.13)), |list, (i, app)| {
            list.child(
                col().key(format!("suggestion-{i}")).role("button").label(app.name.clone()).focusable(true).w_full().px(12.0).py(6.0).radius(8.0).hover(fg.alpha(0.07))
                    .child(text(&app.name).font(font(cfg, 13.0, 600.0)).color(fg))
                    .child(text(&app.launch).font(font(cfg, 11.0, 400.0)).color(fg.alpha(0.4)).truncate())
                    .on_press(move |s: &mut Settings| { s.new_app.replace_text(&app.name); s.picked_app = Some(app.clone()) }),
            )
        });
        apps.push(list);
    }
    for (i, edit) in s.app_names.iter().enumerate() {
        apps.push(
            row().key(format!("app-{i}")).w_full().gap(8.0).center_y()
                .child(TextInput::new(edit).label(format!("Quick app {}", i + 1)).plain()
                    .on_edit(app_name_at(i))
                    .on_change(move |s: &mut Settings, _| {
                        let name = s.app_names[i].text.clone();
                        s.save(|c| if let Some(a) = c.quick_apps.get_mut(i) { *a = QuickApp { launch: name.clone(), name } });
                    })
                    .build(&k).grow(1.0))
                .child(IconButton::new(&icons::TRASH).label(format!("Remove quick app {}", i + 1))
                    .on_press(move |s: &mut Settings| { s.save(|c| { if i < c.quick_apps.len() { c.quick_apps.remove(i); } }); s.reset_app_names() }).build(&k)),
        );
    }
    sections.push(section(s, "quick-apps", "Quick apps", apps));

    // Integrations
    let providers = [("groq", "Groq"), ("openrouter", "OpenRouter")];
    sections.push(section(s, "integrations", "Integrations", vec![
        setting(s, "AI provider", {
            let labels: Vec<&str> = providers.iter().map(|p| p.1).collect();
            Select::new("provider", &labels).label("AI provider").width(190.0)
                .current(providers.iter().find(|p| p.0 == cfg.ai_provider).map(|p| p.1))
                .open(s.open == Some("provider")).on_toggle(toggle_open("provider")).on_close(close)
                .on_choose(move |s: &mut Settings, label: &str, _: &mut Tx| {
                    s.open = None;
                    let Some(&(provider, _)) = providers.iter().find(|p| p.1 == label) else { return };
                    s.save(|c| { c.ai_provider = provider.into(); c.ai_model = default_model(provider).into() });
                    s.ai_model.replace_text(default_model(provider));
                })
                .build(&k, &mut layer)
        }),
        col().w_full().gap(6.0)
            .child(text("AI model").font(font(cfg, 14.0, 500.0)).color(fg.alpha(0.85)))
            .child(TextInput::new(&s.ai_model).label("AI model").placeholder(default_model(&cfg.ai_provider))
                .on_edit(|s: &mut Settings| &mut s.ai_model).on_change(|s: &mut Settings, _| { let v = s.ai_model.text.clone(); s.save(|c| c.ai_model = v) }).build(&k)),
        col().w_full().gap(6.0)
            .child(text("API key").font(font(cfg, 14.0, 500.0)).color(fg.alpha(0.85)))
            .child(TextInput::new(&s.api_key).label("API key").secure().placeholder(if cfg.ai_provider == "groq" { "gsk_..." } else { "sk-or-..." })
                .on_edit(|s: &mut Settings| &mut s.api_key).on_change(|s: &mut Settings, _| { let v = s.api_key.text.clone(); s.save(|c| c.api_key = v) }).build(&k)),
    ]));

    // T3 Code devices
    let mut t3 = vec![text("Token: run `t3 auth session issue --token-only` on that device.").font(font(cfg, 12.0, 400.0)).color(fg.alpha(0.6))];
    for (i, d) in cfg.t3_devices.iter().enumerate() {
        t3.push(setting(s, d.label(), IconButton::new(&icons::TRASH).label(format!("Remove {}", d.label()))
            .on_press(move |s: &mut Settings| s.save(|c| { if i < c.t3_devices.len() { c.t3_devices.remove(i); } })).build(&k)));
    }
    t3.push(TextInput::new(&s.t3_name).label("Device name").placeholder("Name (e.g. Laptop)").on_edit(|s: &mut Settings| &mut s.t3_name).build(&k));
    t3.push(TextInput::new(&s.t3_url).label("Device URL").placeholder("http://127.0.0.1:3773").on_edit(|s: &mut Settings| &mut s.t3_url).build(&k));
    t3.push(TextInput::new(&s.t3_token).label("Bearer token").secure().placeholder("Bearer token").on_edit(|s: &mut Settings| &mut s.t3_token).on_submit(|s: &mut Settings, _| s.add_t3_device()).build(&k));
    t3.push(row().child(Button::new("Add device").variant(ButtonVariant::Contrast).disabled(!s.t3_addable()).on_press(Settings::add_t3_device).build(&k)));
    sections.push(section(s, "t3", "T3 Code devices", t3));

    // Workflows
    let mut workflows = vec![
        text("Workflow name").font(font(cfg, 14.0, 500.0)).color(fg.alpha(0.8)),
        TextInput::new(&s.workflow_name).label("Workflow name").placeholder("e.g. Work Tools").on_edit(|s: &mut Settings| &mut s.workflow_name).build(&k),
        text("Apps or URLs, comma separated").font(font(cfg, 14.0, 500.0)).color(fg.alpha(0.8)).mt(10.0),
        TextArea::new(&s.workflow_urls).label("Apps or URLs").placeholder("e.g. Spotify, docs.google.com").rows(2).on_edit(|s: &mut Settings| &mut s.workflow_urls).build(&k),
        row().child(Button::new("Save workflow").variant(ButtonVariant::Contrast).on_press(Settings::add_workflow).build(&k)),
    ];
    for (i, wf) in cfg.workflows.iter().enumerate() {
        workflows.push(
            row().key(format!("workflow-{i}-{}", wf.name)).w_full().py(8.0).gap(10.0).center_y().justify_between()
                .border_sides([0.0, 0.0, 1.0, 0.0], fg.alpha(0.05))
                .child(col().shrink().min_w(0.0)
                    .child(text(&wf.name).font(font(cfg, 14.0, 600.0)).color(fg).truncate())
                    .child(text(format!("{} items", wf.urls.len())).font(font(cfg, 11.0, 400.0)).color(fg.alpha(0.6))))
                .child(Button::new("Remove").variant(ButtonVariant::Ghost).icon(&icons::TRASH)
                    .on_press(move |s: &mut Settings| s.save(|c| { if i < c.workflows.len() { c.workflows.remove(i); } })).build(&k)),
        );
    }
    sections.push(section(s, "workflows", "Workflows", workflows));

    let content = sections.into_iter().fold(col().key("settings-content").w_full().px(24.0).pt(20.0).pb(36.0).gap(28.0), El::child);
    // An opaque base, so a translucent Island colour stays readable in a normal window.
    let base = col().key("settings").w_full().h_full().min_h(0.0).fill(Color::BLACK)
        .child(col().key("settings-scroll").w_full().grow(1.0).min_h(0.0).fill(cfg.bg()).scroll(ScrollSpec { stick_bottom: false }).child(content));
    let _ = cx;
    rune_kit::root(base, layer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picking_a_theme_updates_the_colour_fields() {
        let mut s = Settings::new(Store::memory(Config::default()), None, &[]);
        s.save(|c| c.set_theme("win95"));
        assert_eq!((s.bg_color.text.as_str(), s.text_color.text.as_str()), ("rgba(195, 195, 195)", "rgba(0, 0, 0)"));
    }

    #[test]
    fn the_wheel_scrolls_the_page() {
        let s = Settings::new(Store::memory(Config::default()), None, &[]);
        let mut rt = rune::Runtime::new(s, view, (520.0, 720.0), rune::Env { system_dark: true, scale: 1.0 });
        rt.frame(0.0);
        let before = rt.find("tab-0").unwrap().1;
        rt.wheel(260.0, 360.0, -200.0);
        for _ in 0..30 { rt.frame(1.0 / 60.0); }
        let after = rt.find("tab-0").unwrap().1;
        assert!(after < before - 50.0, "tab-0 stayed at {before} -> {after}");
    }

    #[test]
    fn a_dragged_tab_lands_on_the_nearest_row() {
        assert_eq!(drop_index(0, 20.0, 7), 0);
        assert_eq!(drop_index(0, 30.0, 7), 1);
        assert_eq!(drop_index(3, -100.0, 7), 1);
        assert_eq!(drop_index(5, 500.0, 7), 6);
        assert_eq!(drop_index(1, -500.0, 7), 0);
    }

    #[test]
    fn dragging_a_tab_row_moves_it_in_the_saved_order() {
        let store = Store::memory(Config::default());
        let s = Settings::new(store.clone(), None, &[]);
        let mut rt = rune::Runtime::new(s, view, (520.0, 2600.0), rune::Env { system_dark: true, scale: 1.0 });
        rt.frame(0.0);
        let (x, y) = rt.find("tab-0").unwrap();
        rt.pointer_down(x, y);
        for step in 1..=10 { rt.pointer_move(x, y + step as f32 * 10.0); rt.frame(1.0 / 60.0); }
        rt.pointer_up(x, y + 100.0);
        rt.frame(1.0 / 60.0);
        assert_eq!(store.get().tab_order, [1, 2, 0, 4, 5, 6, 8, 9, 10, 11]);
    }
}
