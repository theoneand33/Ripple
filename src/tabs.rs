//! What the Island shows in Large mode: one view per tab.

use rune::{Color, El, Easing, Key, KeyEvent, LoopSpec, ScrollSpec, SurfaceFit, TextEdit, Transition, Tx, col, el, icon, row, text, text_input};
use rune_kit::{Markdown, Select};

use crate::icons;
use crate::island::{Island, control, cover};
use crate::system::{self, MediaCommand};

pub fn view(s: &Island, layer: &mut Vec<El>) -> El {
    match s.tab {
        0 => search(s),
        1 => workflows(s),
        2 => overview(s),
        4 => assistant(s),
        5 => clipboard(s),
        6 => tasks(s),
        9 => notes(s),
        10 => stremflix(s),
        _ => t3(s, layer),
    }
}

/// Left and Right switch tabs even while typing, as the Electron app's window-wide key
/// handler did. The T3 message box keeps them for its caret.
fn tab_arrows(s: &mut Island, ev: &KeyEvent, _: &mut Tx) -> bool {
    match ev.key {
        Key::Left => s.move_tab(-1),
        Key::Right => s.move_tab(1),
        _ => return false,
    }
    true
}

/// The Island's filled button: the text colour as fill, the Island colour as ink.
fn button(s: &Island, key: &str, label: &str, enabled: bool) -> El {
    let (bg, fg) = (s.cfg.text(), s.cfg.bg().alpha(1.0));
    el().key(key).role("button").label(label).focusable(enabled).disabled(!enabled)
        .px(16.0).h(34.0).center().radius(if s.cfg.win95() { 0.0 } else { 12.0 })
        .fill(bg.alpha(if enabled { bg.a } else { 0.4 * bg.a }))
        .hover(fg.alpha(0.1)).press(fg.alpha(0.2))
        .child(text(label).font(s.font(14.0, 600.0)).color(fg).a11y_hidden())
}

/// A one-line field in the Island's colours.
fn field(s: &Island, key: &str, edit: &TextEdit, placeholder: &str, access: fn(&mut Island) -> &mut TextEdit, submit: fn(&mut Island, &mut Tx)) -> El {
    let fg = s.cfg.text();
    let input = text_input(edit, s.font(14.0, 400.0).lh(20.0)).key(key).label(placeholder).single_line().grow(1.0).min_w(0.0)
        .color(fg).placeholder(placeholder, fg.alpha(0.4 * fg.a))
        .on_edit(access).on_submit(submit).on_input_key(tab_arrows);
    row().key(format!("{key}-field")).grow(1.0).min_w(0.0).h(36.0).px(12.0).center_y()
        .radius(if s.cfg.win95() { 0.0 } else { 12.0 })
        .fill(fg.alpha(0.05 * fg.a)).border(1.0, fg.alpha(0.1 * fg.a))
        .child(input)
}

fn empty(s: &Island, message: &str) -> El {
    let fg = s.cfg.text();
    el().key("empty").w_full().pt(30.0).center().child(text(message).font(s.font(13.0, 400.0)).color(fg.alpha(0.5 * fg.a)))
}

// --- Browser Search ---

fn search(s: &Island) -> El {
    let fg = s.cfg.text();
    let input = text_input(&s.search, s.font(16.0, 400.0).lh(22.0)).key("browser-searchbar").label("Search").single_line().grow(1.0).min_w(0.0)
        .color(fg).placeholder(format!("Search {} or enter URL", s.cfg.search_engine().0), fg.alpha(0.5 * fg.a))
        .on_edit(|s: &mut Island| &mut s.search)
        .on_submit(Island::search_browser)
        .on_input_key(tab_arrows);
    row().key("search").w_full().h_full().center().child(
        row().key("searchbar").w(360.0).h(46.0).px(16.0).gap(10.0).center_y()
            .radius(if s.cfg.win95() { 0.0 } else { 16.0 })
            .fill(fg.alpha(0.06 * fg.a)).border(1.0, fg.alpha(0.1 * fg.a))
            .child(icon(&icons::SEARCH, 16.0).color(fg.alpha(0.5 * fg.a)).a11y_hidden())
            .child(input),
    )
}

// --- Workflows & quick apps ---

fn workflows(s: &Island) -> El {
    let (bg, fg) = (s.cfg.bg().alpha(1.0), s.cfg.text());
    let radius = if s.cfg.win95() { 0.0 } else { 12.0 };
    let mut list = col().key("workflow-list").w_full().px(14.0).py(14.0).gap(10.0);
    if s.cfg.workflows.is_empty() {
        list = list.child(empty(s, "No workflows yet, add one in Settings"));
    }
    for (i, workflow) in s.cfg.workflows.iter().enumerate() {
        let urls = workflow.urls.clone();
        list = list.child(
            row().key(format!("workflow-{i}-{}", workflow.name)).role("button").label(workflow.name.clone()).focusable(true)
                .w_full().h(40.0).px(14.0).gap(5.0).center_y().radius(radius)
                .fill(fg).hover(bg.alpha(0.1)).press(bg.alpha(0.2))
                .shadow(4.0, 15.0, Color::BLACK.alpha(0.1))
                .transition(Transition::opacity())
                .child(text(&workflow.name).font(s.font(14.0, 600.0)).color(bg).truncate().a11y_hidden())
                .child(text(format!("({} sites)", urls.len())).font(s.font(14.0, 400.0)).color(bg.alpha(0.6)).a11y_hidden())
                .on_press(move |_: &mut Island| {
                    // One at a time, 400 ms apart, so the browser keeps them in order.
                    let urls = urls.clone();
                    std::thread::spawn(move || for url in urls {
                        system::open_app(&url);
                        std::thread::sleep(std::time::Duration::from_millis(400));
                    });
                }),
        );
    }

    let mut apps = row().key("quick-apps").px(15.0).gap(12.0);
    for (i, app) in s.cfg.quick_apps.iter().enumerate() {
        let launch = app.launch.clone();
        apps = apps.child(
            el().key(format!("quick-app-{i}-{}", app.name)).role("button").label(app.name.clone()).focusable(true)
                .h(34.0).px(14.0).center().radius(radius)
                .fill(fg).hover(bg.alpha(0.1)).press(bg.alpha(0.2))
                .transition(Transition::opacity())
                .child(text(&app.name).font(s.font(13.0, 600.0)).color(bg).single_line().a11y_hidden())
                .on_press(move |_: &mut Island| {
                    let launch = launch.clone();
                    std::thread::spawn(move || system::open_app(&launch));
                }),
        );
    }
    col().key("workflows").w_full().h_full()
        .child(el().key("workflow-scroll").w_full().grow(1.0).min_h(0.0).scroll(ScrollSpec { stick_bottom: false }).child(list))
        .child(
            el().key("quick-app-strip").w_full().py(12.0)
                .fill(fg.alpha(0.02 * fg.a)).border_sides([1.0, 0.0, 0.0, 0.0], fg.alpha(0.1 * fg.a))
                .scroll_x().child(apps),
        )
}

// --- Overview ---

/// A line that scrolls sideways, as one endless line, when it is wider than `width`.
fn marquee(key: &str, line: El, text_width: f32, width: f32, height: f32) -> El {
    if text_width <= width {
        return el().key(key).w(width).h(height).clip_soft(0.0).child(line);
    }
    let again = line.clone().a11y_hidden();
    let strip = row().key("strip").absolute(0.0, 0.0)
        .child(el().pr(30.0).child(line))
        .child(el().pr(30.0).child(again))
        .looping(LoopSpec::slide(10.0, [0.0, 0.0], [-0.5, 0.0]).easing(Easing::LINEAR));
    el().key(key).w(width).h(height).clip_soft(0.0).clip_fade(0.0, 0.0, 15.0, 15.0).child(strip)
}

fn overview(s: &Island) -> El {
    let (bg, fg) = (s.cfg.bg().alpha(1.0), s.cfg.text());
    let muted = fg.alpha(0.6 * fg.a);
    let weather = row().key("weather").absolute(24.0, 20.0).gap(4.0).center_y()
        .child(icon(icons::weather(&s.weather.status), 16.0).color(fg))
        .child(text(s.weather.temp.map_or("??º".to_string(), |t| format!("{t}º"))).font(s.font(15.0, 500.0)).color(fg));
    let mut battery = row().key("battery").pin(None, Some(20.0), Some(24.0), None).px(8.0).h(24.0).gap(2.0).center_y()
        .radius(if s.cfg.win95() { 0.0 } else { 8.0 }).fill(fg);
    if s.charging { battery = battery.child(icon(&icons::ZAP, 14.0).color(bg)) }
    battery = battery.child(text(format!("{}%", s.percent)).font(s.font(13.0, 500.0).features("'tnum' 1")).color(bg));
    let gear = el().key("open-settings").role("button").label("Open settings").focusable(true)
        .size(28.0, 28.0).center().radius(8.0).hover(fg.alpha(0.1 * fg.a)).press(fg.alpha(0.2 * fg.a))
        .child(icon(&icons::SETTINGS, 16.0).color(muted))
        .on_press(|s: &mut Island| if let Some(settings) = &s.settings { settings.open() });
    let root = el().key("overview").w_full().h_full().child(weather).child(battery);

    let Some(track) = &s.track else {
        return root
            .child(gear.pin(Some(14.0), None, None, Some(10.0)))
            .child(
                col().key("date").w_full().h_full().center().gap(8.0)
                    .child(text(&s.time).key("time").font(s.font(52.0, 600.0).lh(52.0).features("'tnum' 1")).color(fg))
                    .child(text(&s.date).font(s.font(15.0, 500.0)).color(muted)),
            );
    };

    let name = if track.name.is_empty() { "Unknown Title" } else { &track.name };
    let artist = if track.artist.is_empty() { "Unknown Artist" } else { &track.artist };
    let estimate = |t: &str, size: f32| t.chars().count() as f32 * size * 0.58;
    let media = |key: &'static str, glyph: &'static rune::Icon, size: f32, command: MediaCommand, label: &str| {
        el().key(key).role("button").label(label).focusable(true)
            .padding(4.0).radius(8.0).hover(fg.alpha(0.1 * fg.a)).press(fg.alpha(0.2 * fg.a))
            .child(icon(glyph, size).color(fg))
            .on_press(move |_: &mut Island| control(command))
    };
    let details = col().key("details").grow(1.0).min_w(0.0).pl(5.0)
        .child(marquee("track-name", text(name).font(s.font(18.0, 600.0)).color(fg).single_line(), estimate(name, 18.0), 175.0, 24.0))
        .child(marquee("track-artist", text(artist).font(s.font(13.0, 400.0)).color(fg.alpha(0.8 * fg.a)).single_line(), estimate(artist, 13.0), 175.0, 18.0).mt(4.0))
        .child(
            row().key("controls").mt(14.0).gap(12.0).center_y()
                .child(media("previous", &icons::SKIP_BACK, 20.0, MediaCommand::Previous, "Previous"))
                .child(media("play-pause", if track.playing { &icons::PAUSE } else { &icons::PLAY }, 24.0, MediaCommand::PlayPause, if track.playing { "Pause" } else { "Play" }))
                .child(media("next", &icons::SKIP_FORWARD, 20.0, MediaCommand::Next, "Next")),
        );
    root
        .child(text(&s.time).key("time").pin(Some(0.0), Some(16.0), Some(0.0), None).align(rune::Align::Center).font(s.font(15.0, 600.0).features("'tnum' 1")).color(fg))
        .child(gear.pin(None, None, Some(14.0), Some(10.0)))
        .child(
            row().key(format!("track-{}-{}", track.name, track.artist)).pin(Some(17.0), Some(44.0), Some(14.0), Some(10.0)).gap(8.0).center_y()
                .opacity(if track.playing { 1.0 } else { 0.5 })
                .transition(Transition::blur_replace())
                .child(cover(s, 110.0, 13.0, 40.0, 1.08))
                .child(details),
        )
}

// --- AI Assistant ---

fn assistant(s: &Island) -> El {
    let (bg, fg) = (s.cfg.bg().alpha(1.0), s.cfg.text());
    if !s.asked {
        let input = text_input(&s.ask, s.font(20.0, 400.0).lh(26.0)).key("userinput").label("Ask").w_full().grow(1.0).min_h(0.0)
            .color(fg).placeholder("Ask", fg.alpha(0.7 * fg.a))
            .on_edit(|s: &mut Island| &mut s.ask)
            .on_submit(|s: &mut Island, _| s.ask_ai())
            .on_input_key(tab_arrows);
        return col().key("ask").w_full().h_full().pt(16.0).px(24.0).pb(20.0).gap(8.0)
            .transition(Transition::blur_replace())
            .child(el().w_full().grow(1.0).min_h(0.0).padding(8.0).scroll(ScrollSpec { stick_bottom: false }).child(input))
            .child(row().w_full().justify_end().child(button(s, "chatsubmit", "Ask", true).on_press(|s: &mut Island| s.ask_ai())));
    }
    let answer = match &s.answer {
        Some(answer) if !answer.is_empty() => Markdown::new(answer.as_str()).selectable(true).streaming(true).build(&s.kit),
        _ => text("Thinking...").font(s.font(14.0, 400.0).italic()).color(fg.alpha(0.5 * fg.a)),
    };
    el().key("result").w_full().h_full()
        .transition(Transition::blur_replace())
        .child(
            el().key("answer-scroll").w_full().h_full().scroll(ScrollSpec { stick_bottom: false }).scroll_edge_fade(16.0)
                .child(col().w_full().px(24.0).pt(28.0).pb(56.0).child(answer)),
        )
        .child(button(s, "ask-another", "Ask another", true).pin(None, None, Some(16.0), Some(15.0))
            .shadow(2.0, 10.0, bg.alpha(0.5))
            .on_press(Island::ask_another))
}

// --- Clipboard ---

fn clipboard(s: &Island) -> El {
    let fg = s.cfg.text();
    let mut list = col().key("clipboard-list").w_full().px(14.0).py(14.0).gap(8.0);
    if s.clipboard.is_empty() { list = list.child(empty(s, "Clipboard is empty")) }
    for (i, item) in s.clipboard.iter().enumerate() {
        let copied = s.copied.is_some_and(|c| c.0 == i);
        let value = item.clone();
        let copy = el().key("copy").role("button").label("Copy").focusable(true)
            .px(8.0).h(24.0).center().radius(6.0)
            .fill(if copied { Color { r: 0.03, g: 0.57, b: 0.1, a: 0.4 } } else { Color::WHITE.alpha(0.15) })
            .hover(fg.alpha(0.1 * fg.a)).press(fg.alpha(0.2 * fg.a))
            .child(text(if copied { "Copied" } else { "Copy" }).font(s.font(11.0, 500.0)).color(fg).a11y_hidden())
            .on_press_tx(move |s: &mut Island, tx| {
                tx.copy(value.clone());
                s.copied = Some((i, tx.clock() + 2.0));
                tx.wake_after(std::time::Duration::from_secs(2));
            });
        list = list.child(
            row().key(format!("clip-{i}")).w_full().px(12.0).py(8.0).gap(8.0).items_start()
                .radius(if s.cfg.win95() { 0.0 } else { 12.0 })
                .fill(fg.alpha(0.05 * fg.a)).border(1.0, fg.alpha(0.08 * fg.a))
                .child(text(item).font(s.font(13.0, 400.0).lh(18.0)).color(fg).grow(1.0).min_w(0.0).wrap_anywhere().max_lines(4))
                .child(copy),
        );
    }
    el().key("clipboard").w_full().h_full().scroll(ScrollSpec { stick_bottom: false }).scroll_edge_fade(12.0).child(list)
}

// --- Tasks ---

fn checkbox(s: &Island, key: &str, label: &str, checked: bool) -> El {
    let (bg, fg) = (s.cfg.bg().alpha(1.0), s.cfg.text());
    let mut b = el().key(key).role("checkbox").label(label).checked(checked).focusable(true)
        .size(18.0, 18.0).center().radius(if s.cfg.win95() { 0.0 } else { 6.0 })
        .fill(if checked { fg } else { Color::TRANSPARENT }).border(1.5, fg.alpha(if checked { fg.a } else { 0.5 * fg.a }))
        .hover(fg.alpha(0.15 * fg.a));
    if checked { b = b.child(icon(&icons::CHECK, 12.0).color(bg)) }
    b
}

fn add_task(s: &mut Island, _: &mut Tx) {
    let task = s.task.text.trim().to_string();
    if task.is_empty() { return }
    s.save(|c| c.tasks.push(task));
    s.task.clear();
}

fn tasks(s: &Island) -> El {
    let fg = s.cfg.text();
    let mut list = col().key("task-list").w_full().gap(6.0);
    if s.cfg.tasks.is_empty() { list = list.child(empty(s, "No tasks yet")) }
    for (i, task) in s.cfg.tasks.iter().enumerate() {
        list = list.child(
            row().key(format!("task-{i}-{task}")).w_full().px(12.0).py(8.0).gap(10.0).center_y()
                .radius(if s.cfg.win95() { 0.0 } else { 12.0 }).fill(fg.alpha(0.05 * fg.a))
                .transition(Transition::opacity())
                // Ticking a task off removes it.
                .child(checkbox(s, "done", &format!("Done: {task}"), false).on_press(move |s: &mut Island| s.save(|c| { if i < c.tasks.len() { c.tasks.remove(i); } })))
                .child(text(task).font(s.font(14.0, 500.0)).color(fg).grow(1.0).min_w(0.0).wrap_anywhere()),
        );
    }
    col().key("tasks").w_full().h_full().px(16.0).pt(18.0).pb(14.0).gap(10.0)
        .child(el().key("task-scroll").w_full().grow(1.0).min_h(0.0).scroll(ScrollSpec { stick_bottom: false }).child(list))
        .child(
            row().key("task-input").w_full().gap(8.0).center_y()
                .child(field(s, "new-task", &s.task, "New task...", |s: &mut Island| &mut s.task, add_task))
                .child(button(s, "add-task", "Add", true).on_press_tx(add_task)),
        )
}

// --- Notes ---

/// A note's title is its first line; the lines after it, run together, preview it.
fn note_heading(note: &str) -> (&str, String) {
    let mut lines = note.lines().map(str::trim).filter(|l| !l.is_empty());
    (lines.next().unwrap_or("New Note"), lines.collect::<Vec<_>>().join(" "))
}

fn notes(s: &Island) -> El {
    let fg = s.cfg.text();
    let muted = fg.alpha(0.55 * fg.a);
    let accent = crate::island::status::bluetooth();
    let round = |r: f32| if s.cfg.win95() { 0.0 } else { r };
    let tool = |key: &str, glyph: &'static rune::Icon, label: &str| {
        el().key(key).role("button").label(label).focusable(true)
            .size(28.0, 28.0).center().radius(round(14.0)).fill(fg.alpha(0.1 * fg.a))
            .hover(fg.alpha(0.18 * fg.a)).press(fg.alpha(0.26 * fg.a))
            .child(icon(glyph, 15.0).color(fg).a11y_hidden())
    };
    let mut header = row().key("notes-header").w_full().h(28.0).gap(8.0).center_y()
        .child(text("Notes").font(s.font(17.0, 700.0)).color(fg))
        .child(text(s.cfg.notes.len().to_string()).font(s.font(15.0, 500.0).features("'tnum' 1")).color(muted))
        .child(el().grow(1.0));
    if !s.cfg.notes.is_empty() {
        header = header.child(tool("delete-note", &icons::TRASH, "Delete note").on_press(Island::delete_note));
    }
    header = header.child(tool("new-note", &icons::PLUS, "New note").on_press(Island::add_note));
    let root = col().key("notes").w_full().h_full().px(16.0).pt(14.0).pb(14.0).gap(10.0).child(header);
    if s.cfg.notes.is_empty() { return root.child(empty(s, "No notes yet, press + to write one")) }

    let mut list = col().key("note-list").w_full().gap(2.0);
    for (i, note) in s.cfg.notes.iter().enumerate() {
        let on = i == s.note_index;
        let (title, preview) = note_heading(note);
        list = list.child(
            row().key(format!("note-{i}")).role("button").label(title.to_string()).checked(on).focusable(true)
                .w_full().px(6.0).py(6.0).gap(8.0).items_start().radius(round(10.0))
                .hover(fg.alpha(0.06 * fg.a)).press(fg.alpha(0.12 * fg.a))
                .on_press(move |s: &mut Island| s.open_note(i))
                .child(el().mt(6.0).size(7.0, 7.0).radius(3.5).fill(if on { accent } else { fg.alpha(0.3 * fg.a) }).a11y_hidden())
                .child(
                    col().grow(1.0).min_w(0.0).gap(2.0).a11y_hidden()
                        .child(text(title).font(s.font(14.0, 600.0)).color(if on { accent } else { fg }).truncate())
                        .child(text(if preview.is_empty() { "No additional text" } else { &preview }).font(s.font(12.0, 400.0)).color(muted).truncate()),
                ),
        );
    }
    let editor = text_input(&s.note, s.font(14.0, 400.0).lh(20.0)).key("note-editor").label("Note").w_full()
        .color(fg).placeholder("Title, then your note", fg.alpha(0.4 * fg.a))
        .on_edit(|s: &mut Island| &mut s.note)
        // Return breaks the line.
        .on_submit(|s: &mut Island, _| { s.note.insert("\n"); s.note_edited() })
        .on_input_change(|s: &mut Island, _| s.note_edited());
    root.child(
        row().key("notes-body").w_full().grow(1.0).min_h(0.0).gap(12.0)
            .child(el().key("note-scroll").w(160.0).h_full().scroll(ScrollSpec { stick_bottom: false }).scroll_edge_fade(8.0).child(list))
            .child(
                el().key(format!("note-card-{}", s.note_index)).grow(1.0).min_w(0.0).h_full().px(14.0).py(10.0)
                    .radius(round(18.0)).fill(fg.alpha(0.08 * fg.a)).border(1.0, fg.alpha(0.06 * fg.a))
                    .transition(Transition::opacity())
                    .scroll(ScrollSpec { stick_bottom: false }).scroll_edge_fade(10.0).child(editor),
            ),
    )
}

// --- Stremflix ---

fn stremflix(s: &Island) -> El {
    let fg = s.cfg.text();
    let muted = fg.alpha(0.55 * fg.a);
    let round = |r: f32| if s.cfg.win95() { 0.0 } else { r };
    let (pw, ph) = (88.0, 132.0);
    let header = row().key("stremflix-header").w_full().h(28.0).gap(8.0).center_y()
        .child(text("Continue Watching").font(s.font(17.0, 700.0)).color(fg).single_line())
        .child(text(&s.stremflix.status).key("stremflix-status").role("status").font(s.font(12.0, 400.0)).color(muted).grow(1.0).min_w(0.0).align(rune::Align::End).truncate());
    let root = col().key("stremflix").w_full().h_full().px(16.0).pt(14.0).pb(14.0).gap(10.0).child(header);
    let entries = match &s.stremflix.entries {
        None => return root.child(empty(s, "Loading...")),
        Some(Err(e)) => return root.child(empty(s, e)),
        Some(Ok(list)) if list.is_empty() => return root.child(empty(s, "Nothing to continue")),
        Some(Ok(list)) => list,
    };
    let mut cards = row().key("stremflix-cards").gap(12.0);
    for entry in entries {
        let poster = el().key("poster").size(pw, ph).radius(round(10.0)).clip().fill(fg.alpha(0.08 * fg.a)).a11y_hidden();
        let poster = match entry.poster.as_ref().and_then(|p| s.stremflix.posters.get(p)).filter(|i| i.is_ready() && i.error().is_none()) {
            Some(image) => poster.pixels_fit(image.pixels(), SurfaceFit::Fill),
            None => poster.center().child(icon(&icons::TV, 28.0).color(muted)),
        };
        let progress = el().key("progress").w(pw).h(3.0).radius(1.5).fill(fg.alpha(0.15 * fg.a)).a11y_hidden()
            .child(el().w(pw * entry.progress()).h_full().radius(1.5).fill(fg));
        let play = entry.clone();
        cards = cards.child(
            col().key(format!("watch-{}-{}", entry.kind, entry.id)).role("button").label(format!("Play {}", entry.title)).focusable(true)
                .w(pw).gap(6.0).radius(round(10.0)).hover(fg.alpha(0.06 * fg.a)).press(fg.alpha(0.12 * fg.a))
                .on_press(move |s: &mut Island| s.stremflix_play(play.clone()))
                .child(poster)
                .child(progress)
                .child(text(&entry.title).font(s.font(12.0, 500.0)).color(fg).truncate().a11y_hidden()),
        );
    }
    root.child(el().key("stremflix-scroll").w_full().grow(1.0).min_h(0.0).scroll_x().scroll_edge_fade(10.0).child(cards))
}

// --- T3 Code ---

fn t3(s: &Island, layer: &mut Vec<El>) -> El {
    let fg = s.cfg.text();
    let root = col().key("t3").w_full().h_full().px(16.0).pt(18.0).pb(14.0).gap(8.0);
    let Some((device_index, _)) = s.t3_device() else {
        return root.child(empty(s, "Add a T3 Code device in Settings"));
    };
    let k = &s.kit;
    let links = s.t3.data.as_ref().map(|d| d.links.as_slice()).unwrap_or_default();
    let toggle = |name: &'static str| move |s: &mut Island| s.t3.open = if s.t3.open == Some(name) { None } else { Some(name) };
    let close = |s: &mut Island| s.t3.open = None;

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
                .open(s.t3.open == Some("device")).on_toggle(toggle("device")).on_close(close)
                .on_choose(move |s: &mut Island, label: &str, _: &mut Tx| {
                    let Some((_, index, env)) = options.iter().find(|o| o.0 == label).cloned() else { return };
                    let device_changed = s.t3_device().map(|d| d.0) != Some(index);
                    s.save(|c| { c.t3_device = index; c.t3_env = env; });
                    s.t3.open = None;
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
            .open(s.t3.open == Some("folder")).on_toggle(toggle("folder")).on_close(close)
            .on_choose(move |s: &mut Island, title: &str, _: &mut Tx| {
                if let Some(i) = titles.iter().position(|t| t == title) { let id = ids[i].clone(); s.save(|c| c.t3_project = id) }
                s.t3.open = None;
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
                .open(s.t3.open == Some("model")).on_toggle(toggle("model")).on_close(close)
                .on_choose(move |s: &mut Island, label: &str, _: &mut Tx| {
                    if let Some(i) = labels.iter().position(|l| l == label) { let key = keys[i].clone(); s.save(|c| c.t3_model = key) }
                    s.t3.open = None;
                })
                .build(k, layer),
        )
        .child(
            row().key("t3-worktree").gap(6.0).center_y()
                .on_press(move |s: &mut Island| s.save(|c| c.t3_worktree = !worktree))
                .child(checkbox(s, "worktree", "New worktree", worktree).on_press(move |s: &mut Island| s.save(|c| c.t3_worktree = !worktree)))
                .child(text("New worktree").font(s.font(13.0, 400.0)).color(fg).single_line().a11y_hidden()),
        );

    let message = text_input(&s.t3.message, s.font(14.0, 400.0).lh(20.0)).key("t3-message").label("Message").w_full()
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
            el().key("t3-message-box").w_full().grow(1.0).min_h(0.0).px(12.0).py(8.0)
                .radius(if s.cfg.win95() { 0.0 } else { 10.0 })
                .fill(fg.alpha(0.05 * fg.a)).border(1.0, fg.alpha(0.1 * fg.a))
                .scroll(ScrollSpec { stick_bottom: false }).child(message),
        )
        .child(
            row().key("t3-actions").w_full().gap(8.0).center_y()
                .child(text(&s.t3.status).key("t3-status").role("status").font(s.font(12.0, 400.0)).color(fg.alpha(0.6 * fg.a)).grow(1.0).min_w(0.0).truncate())
                .child(button(s, "t3-start", "Start", s.t3_can_start()).on_press(Island::t3_start)),
        )
}

#[cfg(test)]
mod tests {
    use super::note_heading;

    #[test]
    fn a_note_is_titled_by_its_first_line() {
        assert_eq!(note_heading("PR queue\n\nGitHub\n• PR #482"), ("PR queue", "GitHub • PR #482".to_string()));
        assert_eq!(note_heading("  \n"), ("New Note", String::new()));
    }
}
