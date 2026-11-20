//! The Settings window. Every change saves at once and the Island picks it up.

use rune::{Color, Cursor, Cx, Drag, El, Phase, ScrollSpec, TextEdit, Tx, col, el, icon, row, text};
use rune_kit::{Button, ButtonVariant, IconButton, Kit, RadioGroup, RadioItem, Select, Switch, TextArea, TextInput};

use crate::config::{Config, POSITIONS, PROVIDERS, QuickApp, SEARCH_ENGINES, Store, T3Device, THEMES, Tab, Workflow};
use crate::icons;
use crate::island::Host;
use crate::platform::Screen;

/// A sidebar entry: the app-wide pages, then one per tab that has settings.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Page { General, Tabs, Appearance, Features, Tab(Tab) }

impl Page {
    fn title(self) -> &'static str {
        match self {
            Page::General => "General",
            Page::Tabs => "Tabs",
            Page::Appearance => "Appearance",
            Page::Features => "Features",
            Page::Tab(tab) => tab.name(),
        }
    }

    fn icon(self) -> &'static rune::Icon {
        match self {
            Page::General => &icons::SETTINGS,
            Page::Tabs => &icons::LIST,
            Page::Appearance => &icons::MONITOR,
            Page::Features => &icons::ZAP,
            Page::Tab(tab) => icons::tab(tab),
        }
    }
}

pub struct Settings {
    pub store: Store,
    pub cfg: Config,
    pub island: Option<Host>,
    pub screens: Vec<String>,
    /// The select that is open, by key.
    open: Option<&'static str>,
    /// The tab row being dragged, and how far it has moved.
    dragging: Option<(usize, f32)>,
    /// The page the sidebar has selected.
    pub page: Page,
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
    agent_dirs: TextEdit,
    cliproxy_dir: TextEdit,
    nc_url: TextEdit,
    nc_user: TextEdit,
    nc_password: TextEdit,
    nc_list: TextEdit,
    nc_calendar: TextEdit,
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
            agent_dirs: edit(&cfg.agent_dirs.join("\n")),
            cliproxy_dir: edit(&cfg.cliproxy_dir),
            nc_url: edit(&cfg.nextcloud.url),
            nc_user: edit(&cfg.nextcloud.user),
            nc_password: TextEdit::new("").secure(),
            nc_list: edit(&cfg.nextcloud.list),
            nc_calendar: edit(&cfg.nextcloud.calendar),
            screens: screens.iter().map(|s| s.name.clone()).collect(),
            open: None,
            dragging: None,
            page: Page::General,
            island,
            store,
            cfg,
        }
    }

    /// Saves a change and tells the Island.
    fn save(&mut self, f: impl FnOnce(&mut Config)) {
        self.cfg = self.store.update(f);
        // A theme rewrites both colours; the fields show what it set unless one is being typed in.
        for (edit, value) in [(&mut self.bg_color, &self.cfg.bg_color), (&mut self.text_color, &self.cfg.text_color)] {
            if !edit.focused && edit.text != *value { edit.replace_text(value) }
        }
        if let Some(island) = &self.island { island.update(|s, _| s.reload()) }
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

/// A field bound to a setting: it edits `edit`, and each change saves its text through `set`.
trait Binds {
    fn binds(self, edit: fn(&mut Settings) -> &mut TextEdit, set: fn(&mut Config, &str)) -> Self;
}

impl Binds for TextInput {
    fn binds(self, edit: fn(&mut Settings) -> &mut TextEdit, set: fn(&mut Config, &str)) -> TextInput {
        self.on_edit(edit).on_change(move |s: &mut Settings, _| { let v = edit(s).text.clone(); s.save(|c| set(c, &v)) })
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

/// A switch on its own labelled row.
fn toggle(s: &Settings, k: &Kit, label: &str, on: bool, flip: fn(&mut Config)) -> El {
    setting(s, label, switch(k, label, on, flip).a11y_hidden())
}

fn setting(s: &Settings, label: &str, control: El) -> El {
    let fg = s.cfg.text().alpha(1.0);
    row().key(label.to_string()).w_full().min_h(40.0).gap(12.0).center_y().justify_between()
        .child(text(label).font(s.cfg.font(14.0, 500.0)).color(fg.alpha(0.85)).shrink())
        .child(control)
}

/// A titled card of rows.
fn section(s: &Settings, key: &str, title: &str, rows: Vec<El>) -> El {
    let fg = s.cfg.text().alpha(1.0);
    let card = rows.into_iter().fold(
        col().w_full().gap(6.0).px(16.0).py(12.0).radius(12.0).fill(fg.alpha(0.04)).border(1.0, fg.alpha(0.08)),
        El::child,
    );
    col().key(key.to_string()).w_full().gap(8.0)
        .child(text(title).font(s.cfg.font(12.0, 600.0)).color(fg.alpha(0.5)).px(4.0))
        .child(card)
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

/// Whether a tab has its own settings page: one `panel` has sections for.
fn has_panel(tab: Tab) -> bool {
    match tab {
        Tab::Search | Tab::Workflows | Tab::Overview | Tab::Assistant | Tab::T3 | Tab::Limits | Tab::Tasks | Tab::Notes | Tab::Calendar => true,
        Tab::Clipboard | Tab::Stremflix | Tab::Shelf | Tab::System | Tab::Plugins | Tab::Pomodoro | Tab::Calculator => false,
    }
}

/// The settings for one tab's panel, as sections.
fn panel(s: &Settings, k: &Kit, layer: &mut Vec<El>, tab: Tab) -> Vec<El> {
    let cfg = &s.cfg;
    let fg = cfg.text().alpha(1.0);
    let mut sections = Vec::new();
    match tab {
        Tab::Search => {
            let engines: Vec<(&str, &str)> = SEARCH_ENGINES.iter().map(|e| (e.0, e.1)).collect();
            sections.push(section(s, "search", "Search", vec![
                setting(s, "Search engine", choice(s, k, layer, "engine", "Search engine", &engines, &cfg.search_engine, |c, v| c.search_engine = v.into())),
            ]));
        }
        Tab::Overview => {
            sections.push(section(s, "weather", "Weather", vec![
                setting(s, "Location", TextInput::new(&s.location).label("Location").placeholder("City, ST, Country").width(Some(200.0))
                    .binds(|s| &mut s.location, |c, v| c.location = v.into()).build(k)),
                setting(s, "Unit", choice(s, k, layer, "unit", "Unit", &[("f", "Fahrenheit (°F)"), ("c", "Celsius (°C)")], &cfg.weather_unit, |c, v| c.weather_unit = v.into())),
            ]));
        }
        Tab::Workflows => {
            let mut apps = vec![
                row().key("add-app").w_full().gap(8.0).center_y()
                    .child(TextInput::new(&s.new_app).label("Add app").placeholder("Add app, like Apple Music")
                        .on_edit(|s: &mut Settings| &mut s.new_app)
                        .on_change(|s: &mut Settings, _| s.picked_app = None)
                        .on_submit(|s: &mut Settings, _| s.add_quick_app())
                        .build(k).grow(1.0))
                    .child(IconButton::new(&icons::PLUS).label("Add app").on_press(Settings::add_quick_app).build(k)),
            ];
            let suggestions = s.suggestions();
            if !suggestions.is_empty() {
                let list = suggestions.into_iter().enumerate().fold(col().key("suggestions").w_full().padding(4.0).radius(12.0).fill(fg.alpha(0.05)).border(1.0, fg.alpha(0.13)), |list, (i, app)| {
                    list.child(
                        col().key(format!("suggestion-{i}")).role("button").label(app.name.clone()).focusable(true).w_full().px(12.0).py(6.0).radius(8.0).hover(fg.alpha(0.07))
                            .child(text(&app.name).font(cfg.font(13.0, 600.0)).color(fg))
                            .child(text(&app.launch).font(cfg.font(11.0, 400.0)).color(fg.alpha(0.4)).truncate())
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
                            .build(k).grow(1.0))
                        .child(IconButton::new(&icons::TRASH).label(format!("Remove quick app {}", i + 1))
                            .on_press(move |s: &mut Settings| { s.save(|c| { if i < c.quick_apps.len() { c.quick_apps.remove(i); } }); s.reset_app_names() }).build(k)),
                );
            }
            sections.push(section(s, "quick-apps", "Quick apps", apps));
            let mut workflows = vec![
                text("Workflow name").font(cfg.font(14.0, 500.0)).color(fg.alpha(0.8)),
                TextInput::new(&s.workflow_name).width(None).label("Workflow name").placeholder("e.g. Work Tools").on_edit(|s: &mut Settings| &mut s.workflow_name).build(k),
                text("Apps or URLs, comma separated").font(cfg.font(14.0, 500.0)).color(fg.alpha(0.8)).mt(10.0),
                TextArea::new(&s.workflow_urls).width(None).label("Apps or URLs").placeholder("e.g. Spotify, docs.google.com").rows(2).on_edit(|s: &mut Settings| &mut s.workflow_urls).build(k),
                row().child(Button::new("Save workflow").variant(ButtonVariant::Contrast).on_press(Settings::add_workflow).build(k)),
            ];
            for (i, wf) in cfg.workflows.iter().enumerate() {
                workflows.push(
                    row().key(format!("workflow-{i}-{}", wf.name)).w_full().py(8.0).gap(10.0).center_y().justify_between()
                        .border_sides([0.0, 0.0, 1.0, 0.0], fg.alpha(0.05))
                        .child(col().shrink().min_w(0.0)
                            .child(text(&wf.name).font(cfg.font(14.0, 600.0)).color(fg).truncate())
                            .child(text(format!("{} items", wf.urls.len())).font(cfg.font(11.0, 400.0)).color(fg.alpha(0.6))))
                        .child(Button::new("Remove").variant(ButtonVariant::Ghost).icon(&icons::TRASH)
                            .on_press(move |s: &mut Settings| s.save(|c| { if i < c.workflows.len() { c.workflows.remove(i); } })).build(k)),
                );
            }
            sections.push(section(s, "workflows", "Workflows", workflows));
        }
        Tab::Assistant => {
            sections.push(section(s, "integrations", "Integrations", vec![
                setting(s, "AI provider", {
                    let labels: Vec<&str> = PROVIDERS.iter().map(|p| p.label).collect();
                    Select::new("provider", &labels).label("AI provider").width(190.0)
                        .current(Some(cfg.provider().label))
                        .open(s.open == Some("provider")).on_toggle(toggle_open("provider")).on_close(close)
                        .on_choose(move |s: &mut Settings, label: &str, _: &mut Tx| {
                            s.open = None;
                            let Some(provider) = PROVIDERS.iter().find(|p| p.label == label) else { return };
                            s.save(|c| { c.ai_provider = provider.id.into(); c.ai_model = provider.model.into() });
                            s.ai_model.replace_text(provider.model);
                        })
                        .build(k, layer)
                }),
                col().w_full().gap(6.0)
                    .child(text("AI model").font(cfg.font(14.0, 500.0)).color(fg.alpha(0.85)))
                    .child(TextInput::new(&s.ai_model).width(None).label("AI model").placeholder(cfg.provider().model)
                        .binds(|s| &mut s.ai_model, |c, v| c.ai_model = v.into()).build(k)),
                col().w_full().gap(6.0)
                    .child(text("API key").font(cfg.font(14.0, 500.0)).color(fg.alpha(0.85)))
                    .child(TextInput::new(&s.api_key).width(None).label("API key").secure().placeholder(cfg.provider().key_hint)
                        .binds(|s| &mut s.api_key, |c, v| c.api_key = v.into()).build(k)),
            ]));
        }
        Tab::T3 => {
            let mut t3 = vec![text("T3 Code on this computer is added by itself; removing it here doesn't end its session, so revoke it in T3's Sharing & access. For another device, run `t3 auth session issue --token-only` there.").font(cfg.font(12.0, 400.0)).color(fg.alpha(0.6))];
            for (i, d) in cfg.t3_devices.iter().enumerate() {
                t3.push(setting(s, d.label(), IconButton::new(&icons::TRASH).label(format!("Remove {}", d.label()))
                    .on_press(move |s: &mut Settings| s.save(|c| { if i < c.t3_devices.len() && c.t3_devices.remove(i).name == crate::t3::LOCAL_NAME { c.t3_detected = true } })).build(k)));
            }
            t3.push(TextInput::new(&s.t3_name).width(None).label("Device name").placeholder("Name (e.g. Laptop)").on_edit(|s: &mut Settings| &mut s.t3_name).build(k));
            t3.push(TextInput::new(&s.t3_url).width(None).label("Device URL").placeholder("http://127.0.0.1:3773").on_edit(|s: &mut Settings| &mut s.t3_url).build(k));
            t3.push(TextInput::new(&s.t3_token).width(None).label("Bearer token").secure().placeholder("Bearer token").on_edit(|s: &mut Settings| &mut s.t3_token).on_submit(|s: &mut Settings, _| s.add_t3_device()).build(k));
            t3.push(row().child(Button::new("Add device").variant(ButtonVariant::Contrast).disabled(!s.t3_addable()).on_press(Settings::add_t3_device).build(k)));
            sections.push(section(s, "t3", "T3 Code devices", t3));
        }
        Tab::Limits => {
            sections.push(section(s, "limits", "Accounts", vec![
                text("Each CLI's own sign-in is found by itself. Add more accounts here.").font(cfg.font(12.0, 400.0)).color(fg.alpha(0.6)),
                text("More Claude Code or Codex config folders, one per line").font(cfg.font(14.0, 500.0)).color(fg.alpha(0.8)).mt(10.0),
                TextArea::new(&s.agent_dirs).width(None).label("Claude Code or Codex config folders").placeholder("~/.claude-work").rows(3)
                    .on_edit(|s: &mut Settings| &mut s.agent_dirs)
                    .on_change(|s: &mut Settings, _| { let v = s.agent_dirs.text.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect(); s.save(|c| c.agent_dirs = v) }).build(k),
                text("CLIProxyAPI auth folder").font(cfg.font(14.0, 500.0)).color(fg.alpha(0.8)).mt(10.0),
                TextInput::new(&s.cliproxy_dir).width(None).label("CLIProxyAPI auth folder").placeholder("~/.cli-proxy-api")
                    .binds(|s| &mut s.cliproxy_dir, |c, v| c.cliproxy_dir = v.trim().into()).build(k),
            ]));
        }
        Tab::Tasks | Tab::Notes | Tab::Calendar => {
            let label = |t: &str| text(t.to_string()).font(cfg.font(14.0, 500.0)).color(fg.alpha(0.8)).mt(10.0);
            let mut rows = vec![
                text("Notes, Tasks and Calendar sync with this account. Make an app password in Nextcloud under Settings → Security.").font(cfg.font(12.0, 400.0)).color(fg.alpha(0.6)).wrap(),
                label("Server"),
                TextInput::new(&s.nc_url).width(None).label("Nextcloud server").placeholder("https://cloud.example.com")
                    .binds(|s| &mut s.nc_url, |c, v| c.nextcloud.url = v.trim().into()).build(k),
                label("User"),
                TextInput::new(&s.nc_user).width(None).label("Nextcloud user").placeholder("User name")
                    .binds(|s| &mut s.nc_user, |c, v| c.nextcloud.user = v.trim().into()).build(k),
                label("App password"),
                TextInput::new(&s.nc_password).width(None).label("Nextcloud app password").secure().placeholder(if cfg.nextcloud.password.is_empty() { "App password" } else { "Saved" })
                    .binds(|s| &mut s.nc_password, |c, v| c.nextcloud.password = v.into()).build(k),
            ];
            if tab == Tab::Tasks {
                rows.push(label("Tasks list"));
                rows.push(TextInput::new(&s.nc_list).width(None).label("Nextcloud tasks list").placeholder("The first list with tasks")
                    .binds(|s| &mut s.nc_list, |c, v| c.nextcloud.list = v.trim().into()).build(k));
            }
            if tab == Tab::Calendar {
                rows.push(label("Calendar"));
                rows.push(TextInput::new(&s.nc_calendar).width(None).label("Nextcloud calendar").placeholder("The first calendar with events")
                    .binds(|s| &mut s.nc_calendar, |c, v| c.nextcloud.calendar = v.trim().into()).build(k));
            }
            sections.push(section(s, "nextcloud", "Nextcloud", rows));
        }
        Tab::Clipboard | Tab::Stremflix | Tab::Shelf | Tab::System | Tab::Plugins | Tab::Pomodoro | Tab::Calculator => {}
    }
    sections
}

/// The sections of an app-wide page.
fn page(s: &Settings, k: &Kit, layer: &mut Vec<El>, page: Page) -> Vec<El> {
    let cfg = &s.cfg;
    let fg = cfg.text().alpha(1.0);
    let mut sections = Vec::new();
    match page {
        Page::Tab(tab) => return panel(s, k, layer, tab),
        Page::General => {
            let mut general = vec![setting(s, "Clock format", choice(s, k, layer, "clock", "Clock format", &[("12", "12-hour"), ("24", "24-hour")], if cfg.hour12 { "12" } else { "24" }, |c, v| c.hour12 = v == "12"))];
            general.push(setting(s, "Launch at login", switch(k, "Launch at login", cfg.auto_launch, |c| {
                c.auto_launch = !c.auto_launch;
                let on = c.auto_launch;
                std::thread::spawn(move || crate::system::set_auto_launch(on));
            }).a11y_hidden()));
            if !s.screens.is_empty() {
                let current = if s.screens.contains(&cfg.display) { cfg.display.clone() } else { s.screens[0].clone() };
                general.push(setting(s, "Display", Select::new("display", &s.screens).label("Display").width(190.0).current(Some(&current))
                    .open(s.open == Some("display")).on_toggle(toggle_open("display")).on_close(close)
                    .on_choose(|s: &mut Settings, name: &str, _: &mut Tx| { s.open = None; let name = name.to_string(); s.save(|c| c.display = name) })
                    .build(k, layer)));
            }
            sections.push(section(s, "general", "System", general));
        }
        Page::Tabs => {
            let mut tabs = vec![
                text("Drag to reorder, click the eye to hide").font(cfg.font(12.0, 400.0)).color(fg.alpha(0.5)),
                toggle(s, k, "Always open on the starred tab", cfg.open_default_tab, |c| c.open_default_tab = !c.open_default_tab),
                col().key("tab-linger-row").w_full().gap(8.0).py(6.0)
                    .child(text(format!("Stay open after a tab change ({:.1}s)", cfg.tab_linger)).font(cfg.font(14.0, 500.0)).color(fg.alpha(0.85)))
                    .child(slider(s, "tab-linger", "Stay open after a tab change", (0.0, 10.0, 0.5), |c| c.tab_linger, |c, v| c.tab_linger = v)),
            ];
            let last = cfg.tab_order.len().saturating_sub(1);
            for (i, &tab) in cfg.tab_order.iter().enumerate() {
                let name = tab.name();
                let hidden = cfg.hidden_tabs.contains(&tab);
                let is_default = cfg.default_tab == tab;
                tabs.push(
                    {
                        let row = row().key(format!("tab-{}", tab as u8)).w_full().h(TAB_ROW).px(10.0).gap(10.0).center_y().radius(10.0)
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
                        .child(icon(icons::tab(tab), 16.0).color(fg))
                        .child(text(name).font(cfg.font(14.0, 500.0)).color(fg).grow(1.0).min_w(0.0).truncate())
                        .child(if has_panel(tab) {
                            IconButton::new(&icons::SETTINGS).label(format!("{} settings", name))
                                .on_press(move |s: &mut Settings| { s.open = None; s.page = Page::Tab(tab) }).build(k)
                        } else { el().size(32.0, 32.0).a11y_hidden() })
                        .child(IconButton::new(if is_default { &icons::STAR_FILLED } else { &icons::STAR }).label(format!("Make {} the default tab", name))
                            .on_press(move |s: &mut Settings| s.save(|c| c.default_tab = tab)).build(k).opacity(if is_default { 1.0 } else { 0.4 }))
                        .child(IconButton::new(if hidden { &icons::EYE_OFF } else { &icons::EYE }).label(format!("{} {}", if hidden { "Show" } else { "Hide" }, name))
                            .on_press(move |s: &mut Settings| s.save(|c| c.toggle_tab(tab))).build(k))
                        .child(IconButton::new(&icons::CHEVRON_UP).label(format!("Move {} up", name)).disabled(i == 0)
                            .on_press(move |s: &mut Settings| s.save(|c| c.move_tab(i, i.saturating_sub(1)))).build(k))
                        .child(IconButton::new(&icons::CHEVRON_DOWN).label(format!("Move {} down", name)).disabled(i == last)
                            .on_press(move |s: &mut Settings| s.save(|c| c.move_tab(i, i + 1))).build(k)),
                );
            }
            sections.push(section(s, "tabs", "Order and visibility", tabs));
        }
        Page::Appearance => {
            let mut style = vec![
                toggle(s, k, "Notch mode", cfg.notch_mode, |c| c.notch_mode = !c.notch_mode),
                setting(s, "Theme", choice(s, k, layer, "theme", "Theme", &THEMES.map(|t| (t.0, t.1)), &cfg.theme, |c, v| c.set_theme(v))),
            ];
            if cfg.notch_mode {
                style.insert(1, toggle(s, k, "Mini notch", cfg.notch_mini, |c| c.notch_mini = !c.notch_mini));
                // Auto size only sizes around a physical notch.
                if !cfg.notch_mini { style.insert(2, toggle(s, k, "Auto size", cfg.notch_auto_size, |c| c.notch_auto_size = !c.notch_auto_size)) }
            }
            let position = POSITIONS.iter()
                .fold(RadioGroup::new().label("Position"), |group, &(value, label)| {
                    group.item(RadioItem::new(label, cfg.position_mode == value).on_select(move |s: &mut Settings| s.save(|c| c.position_mode = value.into())))
                });
            style.push(col().key("position").w_full().gap(8.0).padding(14.0).radius(16.0).fill(fg.alpha(0.03)).border(1.0, fg.alpha(0.08)).child(position.build(k)));
            if cfg.position_mode == "free" {
                style.push(setting(s, &format!("Position X ({:.1}%)", cfg.island_x), slider(s, "island-x", "Position X", (0.0, 100.0, 0.1), |c| c.island_x, |c, v| c.island_x = v)).key("island-x-row"));
                style.push(setting(s, &format!("Position Y ({}px)", cfg.island_y.round()), slider(s, "island-y", "Position Y", (0.0, 500.0, 1.0), |c| c.island_y, |c, v| c.island_y = v)).key("island-y-row"));
            }
            style.push(toggle(s, k, "Island border", cfg.island_border, |c| c.island_border = !c.island_border));
            // Only the Wayland layer-shell overlay can be pushed below a panel.
            if matches!(s.island, Some(Host::Overlay(..))) {
                style.push(toggle(s, k, "Overlap panels", cfg.overlap_panels, |c| c.overlap_panels = !c.overlap_panels));
            }
            style.push(toggle(s, k, "Hide when inactive", cfg.hide_inactive, |c| c.hide_inactive = !c.hide_inactive));
            sections.push(section(s, "style", "Island style", style));

            sections.push(section(s, "colors", "Colors and background", vec![
                setting(s, "Island color", TextInput::new(&s.bg_color).label("Island color").placeholder("#000000").width(Some(160.0))
                    .binds(|s| &mut s.bg_color, |c, v| c.bg_color = v.into()).build(k)),
                setting(s, "Text color", TextInput::new(&s.text_color).label("Text color").placeholder("#FAFAFA").width(Some(160.0))
                    .binds(|s| &mut s.text_color, |c, v| c.text_color = v.into()).build(k)),
                col().w_full().gap(6.0)
                    .child(text("Background image URL").font(cfg.font(14.0, 500.0)).color(fg.alpha(0.85)))
                    .child(TextInput::new(&s.bg_image).width(None).label("Background image URL").placeholder("https://...")
                        .binds(|s| &mut s.bg_image, |c, v| c.bg_image = v.into()).build(k)),
            ]));
        }
        Page::Features => {
            sections.push(section(s, "features", "Alerts and standby", vec![
                toggle(s, k, "Low battery alerts", cfg.battery_alerts, |c| c.battery_alerts = !c.battery_alerts),
                toggle(s, k, "Standby mode", cfg.standby, |c| c.standby = !c.standby),
                toggle(s, k, "Large standby mode", cfg.large_standby, |c| c.large_standby = !c.large_standby),
                toggle(s, k, "Show info when idle", cfg.show_info_idle, |c| c.show_info_idle = !c.show_info_idle),
            ]));
        }
    }
    sections
}

fn nav_item(s: &Settings, item: Page) -> El {
    let fg = s.cfg.text().alpha(1.0);
    let on = s.page == item;
    let key = match item { Page::Tab(tab) => format!("nav-tab-{}", tab as u8), _ => format!("nav-{item:?}") };
    row().key(key).role("tab").label(item.title()).checked(on).focusable(true)
        .w_full().h(34.0).px(10.0).gap(10.0).center_y().radius(8.0)
        .fill(fg.alpha(if on { 0.1 } else { 0.0 })).hover(fg.alpha(if on { 0.12 } else { 0.06 }))
        .child(icon(item.icon(), 16.0).color(fg.alpha(if on { 1.0 } else { 0.6 })))
        .child(text(item.title()).font(s.cfg.font(13.0, if on { 600.0 } else { 500.0 })).color(fg.alpha(if on { 1.0 } else { 0.75 })).truncate().shrink())
        .on_press(move |s: &mut Settings| { s.open = None; s.page = item })
}

fn sidebar(s: &Settings) -> El {
    let fg = s.cfg.text().alpha(1.0);
    let group = |title: &str| text(title.to_string()).font(s.cfg.font(11.0, 600.0)).color(fg.alpha(0.4)).px(10.0).pt(14.0).pb(4.0);
    let mut items = vec![text("Settings").font(s.cfg.font(17.0, 700.0)).color(fg).px(10.0).pb(10.0)];
    items.extend([Page::General, Page::Tabs, Page::Appearance, Page::Features].map(|p| nav_item(s, p)));
    items.push(group("Tab settings"));
    items.extend(s.cfg.tab_order.iter().filter(|&&t| has_panel(t)).map(|&t| nav_item(s, Page::Tab(t))));
    let list = items.into_iter().fold(col().key("sidebar-list").w_full().gap(2.0).px(10.0).pt(20.0).pb(16.0), El::child);
    col().key("sidebar").w(SIDEBAR).h_full().min_h(0.0).fill(fg.alpha(0.03)).border_sides([0.0, 1.0, 0.0, 0.0], fg.alpha(0.08))
        .child(col().key("sidebar-scroll").w_full().grow(1.0).min_h(0.0).scroll(ScrollSpec { stick_bottom: false }).child(list))
}

const SIDEBAR: f32 = 190.0;

pub fn view(s: &Settings, _: &mut Cx<Settings>) -> El {
    let k = s.cfg.kit();
    let mut layer = Vec::new();
    let cfg = &s.cfg;
    let fg = cfg.text().alpha(1.0);
    let title = text(s.page.title()).font(cfg.font(22.0, 700.0)).color(fg).key("page-title");
    let sections = std::iter::once(title).chain(page(s, &k, &mut layer, s.page));
    let content = sections.fold(col().key("settings-content").w_full().max_w(720.0).px(28.0).pt(24.0).pb(36.0).gap(24.0), El::child);
    let scroll_key = match s.page { Page::Tab(tab) => format!("settings-scroll-tab-{}", tab as u8), p => format!("settings-scroll-{p:?}") };
    // An opaque base, so a translucent Island colour stays readable in a normal window.
    let base = row().key("settings").w_full().h_full().min_h(0.0).fill(Color::BLACK)
        .child(row().w_full().h_full().min_h(0.0).fill(cfg.bg())
            .child(sidebar(s))
            .child(col().key(scroll_key).grow(1.0).min_w(0.0).h_full().min_h(0.0).scroll(ScrollSpec { stick_bottom: false }).child(content)));
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
        let mut s = Settings::new(Store::memory(Config::default()), None, &[]);
        s.page = Page::Tabs;
        let mut rt = rune::Runtime::new(s, view, (720.0, 720.0), rune::Env { system_dark: true, scale: 1.0 });
        rt.frame(0.0);
        let before = rt.find("tab-0").unwrap().1;
        rt.wheel(460.0, 360.0, -200.0);
        for _ in 0..30 { rt.frame(1.0 / 60.0); }
        let after = rt.find("tab-0").unwrap().1;
        assert!(after < before - 50.0, "tab-0 stayed at {before} -> {after}");
    }

    #[test]
    fn a_tab_settings_page_replaces_the_main_page() {
        let mut s = Settings::new(Store::memory(Config::default()), None, &[]);
        s.page = Page::Tab(Tab::Assistant);
        let mut rt = rune::Runtime::new(s, view, (720.0, 720.0), rune::Env { system_dark: true, scale: 1.0 });
        rt.frame(0.0);
        assert!(rt.find("integrations").is_some() && rt.find("tab-0").is_none());
    }

    #[test]
    fn dragging_a_tab_row_moves_it_in_the_saved_order() {
        let store = Store::memory(Config::default());
        let mut s = Settings::new(store.clone(), None, &[]);
        s.page = Page::Tabs;
        let mut rt = rune::Runtime::new(s, view, (720.0, 2600.0), rune::Env { system_dark: true, scale: 1.0 });
        rt.frame(0.0);
        let (x, y) = rt.find("tab-0").unwrap();
        rt.pointer_down(x, y);
        for step in 1..=10 { rt.pointer_move(x, y + step as f32 * 10.0); rt.frame(1.0 / 60.0); }
        rt.pointer_up(x, y + 100.0);
        rt.frame(1.0 / 60.0);
        use Tab::*;
        assert_eq!(store.get().tab_order, [Overview, Workflows, Assistant, Search, Clipboard, Tasks, T3, Notes, Stremflix, Shelf, System, Calendar, Limits, Pomodoro, Calculator, Plugins]);
    }

    #[test]
    fn a_dragged_slider_saves_on_release() {
        let store = Store::memory(Config::default());
        let mut s = Settings::new(store.clone(), None, &[]);
        s.page = Page::Tabs;
        let mut rt = rune::Runtime::new(s, view, (720.0, 2600.0), rune::Env { system_dark: true, scale: 1.0 });
        rt.frame(0.0);
        // The row's label shows the value, so a drag that changed it must not lose its release.
        let (x, y) = rt.find("tab-linger").unwrap();
        rt.pointer_down(x, y);
        for step in 1..=6 { rt.pointer_move(x + step as f32 * 10.0, y); rt.frame(1.0 / 60.0); }
        rt.pointer_up(x + 60.0, y);
        rt.frame(1.0 / 60.0);
        assert!(store.get().tab_linger > 1.0, "saved {}", store.get().tab_linger);
    }
}
