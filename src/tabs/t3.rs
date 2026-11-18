//! T3 Code: start an agent thread on a T3 device, or on one of its linked environments.

use std::path::Path;
use std::sync::Arc;

use rune::{El, Key, KeyEvent, ScrollSpec, TextEdit, Tx, col, row, text, text_input};
use rune_kit::Select;

use super::{button, checkbox, empty};
use crate::config::T3Device;
use crate::island::Island;
use crate::t3::{self as client, Client};

pub struct T3 {
    pub data: Option<client::Loaded>,
    pub projects: Option<Vec<client::Project>>,
    pub status: String,
    pub message: TextEdit,
    /// Bumped on every load, so an answer for an earlier device or environment is dropped.
    generation: u64,
    /// A thread launch is in flight.
    starting: bool,
    /// Looking for the local T3 Code.
    detecting: bool,
    client: Arc<Client>,
}

impl T3 {
    /// `dir` keeps the MCP sign-in tokens between launches.
    pub fn new(dir: Option<&Path>) -> T3 {
        let client = Arc::new(Client::new(dir.map(|d| d.join("t3-mcp-tokens.json"))));
        T3 { data: None, projects: None, status: String::new(), message: TextEdit::default(), generation: 0, starting: false, detecting: false, client }
    }
}

impl Island {
    pub fn t3_device(&self) -> Option<(usize, &T3Device)> {
        let devices = &self.cfg.t3_devices;
        devices.get(self.cfg.t3_device).map(|d| (self.cfg.t3_device, d)).or(devices.first().map(|d| (0, d)))
    }

    /// The chosen environment, if the device still links it; "" is the device itself.
    pub fn t3_env(&self) -> &str {
        let linked = self.t3.data.as_ref().is_some_and(|d| d.links.iter().any(|l| l.id == self.cfg.t3_env));
        if linked { &self.cfg.t3_env } else { "" }
    }

    pub fn t3_project(&self) -> Option<&client::Project> {
        let projects = self.t3.projects.as_ref()?;
        projects.iter().find(|p| p.id == self.cfg.t3_project).or(projects.first())
    }

    pub fn t3_model(&self) -> Option<&client::Favorite> {
        let favorites = &self.t3.data.as_ref()?.favorites;
        favorites.iter().find(|f| f.key() == self.cfg.t3_model).or(favorites.first())
    }

    /// Asks the device for its models and linked environments, then for its folders.
    pub fn t3_load(&mut self) {
        self.t3.generation += 1;
        self.t3.data = None;
        self.t3.projects = None;
        // Retried on each open until found: T3 may not be installed or running yet.
        if self.cfg.t3_devices.is_empty() && !self.cfg.t3_detected && !self.t3.detecting {
            if let Some(me) = self.me.clone() {
                self.t3.detecting = true;
                self.t3.status = "Looking for T3 Code on this computer...".into();
                std::thread::spawn(move || {
                    let found = client::detect_local();
                    me.update(move |s, _| {
                        s.t3.detecting = false;
                        s.t3.status.clear();
                        let Some(device) = found else { return };
                        // Kept even if a device was added meanwhile: a dropped token would stay live in T3.
                        s.save(|c| { c.t3_detected = true; c.t3_devices.push(device) });
                        s.t3_load();
                    });
                });
            }
            return;
        }
        let (Some(device), Some(me)) = (self.t3_device().map(|d| d.1.clone()), self.me.clone()) else { return };
        self.t3.status = "Loading...".into();
        let (client, generation) = (self.t3.client.clone(), self.t3.generation);
        std::thread::spawn(move || {
            let result = client.load(&device);
            me.update(move |s, _| {
                if s.t3.generation != generation { return }
                match result {
                    Ok(data) => {
                        s.t3.data = Some(data);
                        s.t3_load_projects();
                    }
                    Err(e) => s.t3.status = e,
                }
            });
        });
    }

    pub fn t3_load_projects(&mut self) {
        self.t3.generation += 1;
        self.t3.projects = None;
        let (Some(device), Some(me)) = (self.t3_device().map(|d| d.1.clone()), self.me.clone()) else { return };
        self.t3.status = "Loading folders...".into();
        let (client, generation, env) = (self.t3.client.clone(), self.t3.generation, self.t3_env().to_string());
        std::thread::spawn(move || {
            let result = client.projects(&device, &env);
            me.update(move |s, _| {
                if s.t3.generation != generation { return }
                match result {
                    Ok(list) => {
                        s.t3.projects = Some(list);
                        s.t3.status.clear();
                    }
                    Err(e) => s.t3.status = e,
                }
            });
        });
    }

    pub fn t3_can_start(&self) -> bool {
        !self.t3.starting && self.t3_project().is_some() && self.t3_model().is_some() && !self.t3.message.text.trim().is_empty()
    }

    pub fn t3_start(&mut self) {
        // One launch at a time: a second Ctrl+Return or click while starting does nothing.
        if !self.t3_can_start() { return }
        let (Some((_, device)), Some(project), Some(model), Some(me)) = (self.t3_device(), self.t3_project(), self.t3_model(), self.me.clone()) else { return };
        let launch = client::Launch {
            environment_id: self.t3_env().to_string(),
            project_id: project.id.clone(),
            instance_id: model.instance_id.clone(),
            model: model.model.clone(),
            message: self.t3.message.text.clone(),
            worktree: self.cfg.t3_worktree,
            runtime_mode: self.t3.data.as_ref().map(|d| d.runtime_mode.clone()).unwrap_or_default(),
        };
        let (device, client, title) = (device.clone(), self.t3.client.clone(), project.title.clone());
        self.t3.status = "Starting...".into();
        self.t3.starting = true;
        std::thread::spawn(move || {
            let result = client.start_thread(&device, &launch);
            me.update(move |s, _| { s.t3.starting = false; match result {
                Ok(_) => {
                    s.t3.message.clear();
                    s.t3.status = format!("Thread started in {title}");
                }
                Err(e) => s.t3.status = e,
            } });
        });
    }
}

pub fn view(s: &Island, layer: &mut Vec<El>) -> El {
    let fg = s.cfg.text();
    let root = col().key("t3").w_full().h_full().px(16.0).pt(18.0).pb(14.0).gap(8.0);
    let Some((device_index, _)) = s.t3_device() else {
        return root.child(empty(s, if s.t3.detecting { "Looking for T3 Code on this computer..." } else { "Add a T3 Code device in Settings" }));
    };
    let k = &s.cfg.kit();
    let links = s.t3.data.as_ref().map(|d| d.links.as_slice()).unwrap_or_default();
    let toggle = |name: &'static str| move |s: &mut Island| s.open_select = if s.open_select == Some(name) { None } else { Some(name) };
    let close = |s: &mut Island| s.open_select = None;

    let mut selects = row().key("t3-targets").w_full().gap(8.0);
    if s.cfg.t3_devices.len() > 1 || !links.is_empty() {
        // Each device, with the chosen device's linked environments under it.
        let mut options: Vec<(String, usize, String)> = Vec::new();
        for (i, d) in s.cfg.t3_devices.iter().enumerate() {
            options.push((d.label().to_string(), i, String::new()));
            if i == device_index {
                options.extend(links.iter().map(|l| (format!("↳ {} (linked)", l.label), i, l.id.clone())));
            }
        }
        let labels: Vec<String> = options.iter().map(|o| o.0.clone()).collect();
        let current = options.iter().find(|o| o.1 == device_index && o.2 == s.t3_env()).map(|o| o.0.clone());
        selects = selects.child(
            Select::new("t3-device", &labels).label("Device").width(170.0).current(current.as_deref())
                .open(s.open_select == Some("device")).on_toggle(toggle("device")).on_close(close)
                .on_choose(move |s: &mut Island, label: &str, _: &mut Tx| {
                    let Some((_, index, env)) = options.iter().find(|o| o.0 == label).cloned() else { return };
                    let device_changed = s.t3_device().map(|d| d.0) != Some(index);
                    s.save(|c| { c.t3_device = index; c.t3_env = env; });
                    s.open_select = None;
                    if device_changed { s.t3_load() } else if s.t3.data.is_some() { s.t3_load_projects() }
                })
                .build(k, layer),
        );
    }
    let projects = s.t3.projects.as_deref().unwrap_or_default();
    let titles: Vec<String> = projects.iter().map(|p| p.title.clone()).collect();
    let ids: Vec<String> = projects.iter().map(|p| p.id.clone()).collect();
    selects = selects.child(
        Select::new("t3-folder", &titles).label("Folder").width(170.0).placeholder("Folder")
            .current(s.t3_project().map(|p| p.title.as_str()))
            .disabled(s.t3.projects.is_none())
            .open(s.open_select == Some("folder")).on_toggle(toggle("folder")).on_close(close)
            .on_choose(move |s: &mut Island, title: &str, _: &mut Tx| {
                if let Some(i) = titles.iter().position(|t| t == title) { let id = ids[i].clone(); s.save(|c| c.t3_project = id) }
                s.open_select = None;
            })
            .build(k, layer),
    );

    let favorites = s.t3.data.as_ref().map(|d| d.favorites.as_slice()).unwrap_or_default();
    let labels: Vec<String> = favorites.iter().map(|f| f.label.clone()).collect();
    let keys: Vec<String> = favorites.iter().map(|f| f.key()).collect();
    let worktree = s.cfg.t3_worktree;
    let model_row = row().key("t3-model-row").w_full().gap(10.0).center_y()
        .child(
            Select::new("t3-model", &labels).label("Model").width(210.0)
                .placeholder(if s.t3.data.is_some() && favorites.is_empty() { "No favorite models" } else { "Model" })
                .current(s.t3_model().map(|f| f.label.as_str()))
                .disabled(s.t3.data.is_none())
                .open(s.open_select == Some("model")).on_toggle(toggle("model")).on_close(close)
                .on_choose(move |s: &mut Island, label: &str, _: &mut Tx| {
                    if let Some(i) = labels.iter().position(|l| l == label) { let key = keys[i].clone(); s.save(|c| c.t3_model = key) }
                    s.open_select = None;
                })
                .build(k, layer),
        )
        .child(
            row().key("t3-worktree").gap(6.0).center_y()
                .on_press(move |s: &mut Island| s.save(|c| c.t3_worktree = !worktree))
                .child(checkbox(s, "worktree", "New worktree", worktree).on_press(move |s: &mut Island| s.save(|c| c.t3_worktree = !worktree)))
                .child(text("New worktree").font(s.cfg.font(13.0, 400.0)).color(fg).single_line().a11y_hidden()),
        );

    let message = text_input(&s.t3.message, s.cfg.font(14.0, 400.0).lh(20.0)).key("t3-message").label("Message").w_full()
        .color(fg).placeholder("What should the agent do?", fg.alpha(0.4 * fg.a))
        .on_edit(|s: &mut Island| &mut s.t3.message)
        // Return breaks the line; Ctrl+Return (⌘ on macOS) starts the thread.
        .on_submit(|s: &mut Island, _| s.t3.message.insert("\n"))
        .on_input_key(|s: &mut Island, ev: &KeyEvent, _| {
            let start = ev.key == Key::Enter && ev.mods.cmd;
            if start { s.t3_start() }
            start
        });
    root.child(selects)
        .child(model_row)
        .child(
            col().key("t3-message-box").w_full().grow(1.0).min_h(0.0).px(12.0).py(8.0)
                .radius(s.cfg.radius(10.0))
                .fill(fg.alpha(0.05 * fg.a)).border(1.0, fg.alpha(0.1 * fg.a))
                .scroll(ScrollSpec { stick_bottom: false }).on_wheel(Island::wheel).child(message),
        )
        .child(
            row().key("t3-actions").w_full().gap(8.0).center_y()
                .child(text(&s.t3.status).key("t3-status").role("status").font(s.cfg.font(12.0, 400.0)).color(fg.alpha(0.6 * fg.a)).grow(1.0).min_w(0.0).truncate())
                .child(button(s, "t3-start", "Start", s.t3_can_start()).on_press(Island::t3_start)),
        )
}
