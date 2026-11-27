//! Discord: the voice channel the user is in, who is speaking, and mute, deafen and leave. The
//! Overview's Discord Voice widget shows the same channel in a tile.

use std::collections::{HashMap, HashSet};

use rune::{Color, El, Image, SurfaceFit, col, el, icon, row, text};

use super::{Live, Scrolls, accent, card, clip, empty};
use crate::config::Config;
use crate::discord::{self as api, App, Command, Member, Voice};
use crate::icons;
use crate::island::{Island, fetch_image};

fn app_of(c: &Config) -> Option<App> {
    let (id, secret) = (c.discord_id.trim(), c.discord_secret.trim());
    (!id.is_empty() && !secret.is_empty()).then(|| App { id: id.into(), secret: secret.into() })
}

#[derive(Default)]
pub struct Discord {
    /// The user's voice, or why it can't show; None until the watcher first reports.
    pub voice: Option<Result<Voice, String>>,
    /// Loaded avatars by URL.
    avatars: HashMap<String, Image>,
    /// Avatars asked for, loaded or not: each is fetched once, failed ones included.
    requested: HashSet<String>,
    /// A command that failed.
    status: String,
}

impl Island {
    /// The Discord app set up in Settings; None turns Discord off.
    pub fn discord_app(&self) -> Option<App> { app_of(&self.cfg) }

    /// Follows the user's voice from Discord for as long as the Island runs.
    pub fn discord_watch(&mut self) {
        let Some(me) = self.me.clone() else { return };
        let store = self.store.clone();
        std::thread::spawn(move || {
            let (save, on) = (me.clone(), me);
            api::watch(
                move || { let c = store.get(); app_of(&c).map(|app| (app, c.discord_token)) },
                move |token| save.update(move |s, _| s.save(|c| c.discord_token = Some(token))),
                // A call starting or ending opens or closes a live activity, so it animates.
                move |voice| on.update(move |s, tx| { s.report(tx, |s, _| { s.discord_set(voice); true }); }),
            );
        });
    }

    pub fn discord_set(&mut self, voice: Result<Voice, String>) {
        let urls: Vec<String> = voice.iter().flat_map(|v| &v.channel).flat_map(|c| &c.members)
            .filter_map(|m| m.avatar.clone()).filter(|u| self.discord.requested.insert(u.clone())).collect();
        self.discord.voice = Some(voice);
        let Some(me) = self.me.clone() else { return };
        for url in urls {
            let me2 = me.clone();
            std::thread::spawn(move || fetch_image(&me2, url.clone(), true, move |s, image| { s.discord.avatars.insert(url, image); }));
        }
    }

    fn discord_command(&mut self, cmd: Command) {
        let (Some(me), Some(app), Some(token)) = (self.me.clone(), self.discord_app(), self.cfg.discord_token.clone()) else { return };
        self.discord.status.clear();
        me.spawn(move || api::command(&app, &token, cmd), |s, (renewed, result)| {
            if let Some(t) = renewed { s.save(|c| c.discord_token = Some(t)) }
            s.discord.status = result.err().unwrap_or_default();
        });
    }
}

/// The voice to draw, or the message to show instead.
fn voice(s: &Island) -> Result<&Voice, &str> {
    if s.discord_app().is_none() { return Err("Set up your Discord app in Settings → Discord") }
    match &s.discord.voice {
        None => Err("Connecting to Discord…"),
        Some(Err(e)) => Err(e),
        Some(Ok(v)) => Ok(v),
    }
}

/// The call the user is in, as a live activity: who speaks, green; red while muted or deafened.
/// Shown only with the Discord tab, which its bubble opens.
pub fn live(s: &Island) -> Option<Live> {
    if !s.visible_tabs().contains(&crate::config::Tab::Discord) { return None }
    let Some(Ok(v)) = &s.discord.voice else { return None };
    let channel = v.channel.as_ref()?;
    let speaking = channel.members.iter().find(|m| m.speaking);
    let (key, glyph, tint) = if v.deaf { ("call-deaf", &icons::HEADPHONE_OFF, accent::red()) }
        else if v.mute { ("call-muted", &icons::MIC_OFF, accent::red()) }
        else if speaking.is_some() { ("call-speaking", &icons::HEADPHONES, accent::green()) }
        else { ("call", &icons::HEADPHONES, accent::indigo()) };
    let state = if v.deaf { ", deafened" } else if v.mute { ", muted" } else { "" };
    let spoken = format!("In {} on Discord{state}{}", channel.name, speaking.map_or(String::new(), |m| format!(", {} speaking", m.name)));
    Some(Live { key, glyph, tint, label: clip(speaking.map_or(&channel.name, |m| &m.name), 12), spoken })
}

/// A member's picture in a circle, ringed green while they speak.
fn avatar(s: &Island, m: &Member, size: f32) -> El {
    let fg = s.cfg.text();
    let mut a = el().key(format!("avatar-{}", m.id)).size(size, size).center().radius(size / 2.0).fill(fg.alpha(0.14 * fg.a)).a11y_hidden();
    a = match m.avatar.as_ref().and_then(|u| s.discord.avatars.get(u)).filter(|i| i.is_ready() && i.error().is_none()) {
        Some(image) => a.pixels_fit(image.pixels(), SurfaceFit::Fill).clip_rounded(size / 2.0),
        None => a.child(text(m.name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default()).font(s.cfg.font(size * 0.42, 700.0)).color(fg)),
    };
    if m.speaking { a = a.border(2.0, accent::green()) }
    a
}

/// A round toggle: tinted red while `on` (muted, deafened).
fn toggle(s: &Island, key: &str, glyph: &'static rune::Icon, label: &str, on: bool, size: f32) -> El {
    let fg = s.cfg.text();
    let (fill, ink) = if on { (accent::red().alpha(0.22), accent::red()) } else { (fg.alpha(0.1 * fg.a), fg) };
    el().key(key).role("button").label(label).focusable(true)
        .size(size, size).center().radius(s.cfg.radius(size / 2.0)).fill(fill)
        .hover(fg.alpha(0.18 * fg.a)).press(fg.alpha(0.26 * fg.a))
        .child(icon(glyph, size * 0.54).color(ink).a11y_hidden())
}

fn mute(s: &Island, v: &Voice, size: f32) -> El {
    let (muted, glyph) = (v.mute || v.deaf, if v.mute || v.deaf { &icons::MIC_OFF } else { &icons::MIC });
    let next = !v.mute;
    toggle(s, "discord-mute", glyph, if v.mute { "Unmute" } else { "Mute" }, muted, size)
        .on_press(move |s: &mut Island| s.discord_command(Command::Mute(next)))
}

fn deafen(s: &Island, v: &Voice, size: f32) -> El {
    let next = !v.deaf;
    toggle(s, "discord-deafen", if v.deaf { &icons::HEADPHONE_OFF } else { &icons::HEADPHONES }, if v.deaf { "Undeafen" } else { "Deafen" }, v.deaf, size)
        .on_press(move |s: &mut Island| s.discord_command(Command::Deaf(next)))
}

fn state_icons(m: &Member, muted: Color) -> El {
    let mut r = row().gap(6.0).center_y().a11y_hidden();
    if m.muted { r = r.child(icon(&icons::MIC_OFF, 14.0).color(muted)) }
    if m.deafened { r = r.child(icon(&icons::HEADPHONE_OFF, 14.0).color(muted)) }
    r
}

fn describe(m: &Member) -> String {
    let mut label = m.name.clone();
    for (on, what) in [(m.speaking, "speaking"), (m.muted, "muted"), (m.deafened, "deafened")] {
        if on { label += ", "; label += what }
    }
    label
}

pub fn view(s: &Island) -> El {
    let fg = s.cfg.text();
    let muted = fg.alpha(0.55 * fg.a);
    let root = col().key("discord").w_full().h_full().px(16.0).pt(14.0).pb(14.0).gap(10.0);
    let v = match voice(s) {
        Ok(v) => v,
        Err(e) => return root.child(text("Discord").font(s.cfg.font(17.0, 700.0)).color(fg)).child(empty(s, e)),
    };
    let title = v.channel.as_ref().map_or("Discord", |c| c.name.as_str());
    let mut header = row().key("discord-header").w_full().min_h(28.0).gap(8.0).center_y()
        .child(text(title).font(s.cfg.font(17.0, 700.0)).color(fg).truncate().min_w(0.0))
        .child(text(&s.discord.status).key("discord-status").role("status").font(s.cfg.font(12.0, 400.0)).color(muted).grow(1.0).min_w(0.0).align(rune::Align::End).truncate())
        .child(mute(s, v, 28.0))
        .child(deafen(s, v, 28.0));
    if v.channel.is_some() {
        header = header.child(toggle(s, "discord-leave", &icons::PHONE_OFF, "Leave the call", true, 28.0)
            .on_press(|s: &mut Island| s.discord_command(Command::Leave)));
    }
    let root = root.child(header);
    let Some(channel) = &v.channel else { return root.child(empty(s, "Not in a voice channel")) };
    let mut list = col().key("discord-members").w_full().gap(6.0);
    for m in &channel.members {
        list = list.child(row().key(format!("member-{}", m.id)).role("group").label(describe(m)).w_full().h(44.0).px(8.0).gap(10.0).center_y()
            .radius(s.cfg.radius(12.0)).fill(card(fg))
            .child(avatar(s, m, 32.0))
            .child(text(&m.name).font(s.cfg.font(14.0, if m.speaking { 700.0 } else { 500.0 })).color(fg).truncate().grow(1.0).min_w(0.0).a11y_hidden())
            .child(state_icons(m, muted)));
    }
    root.child(col().key("discord-scroll").w_full().grow(1.0).min_h(0.0).scrolls().child(list))
}

/// The Overview tile: the channel's name and its members' avatars, speakers ringed.
pub fn widget(s: &Island, w: f32, h: f32) -> El {
    let fg = s.cfg.text();
    let muted = fg.alpha(0.6 * fg.a);
    let short = h < 100.0;
    let message = |m: &str| {
        let c = if short { row() } else { col() };
        c.key("voice").w_full().h_full().px(10.0).center().gap(6.0)
            .child(icon(&icons::HEADPHONES, 24.0).color(fg.alpha(0.5 * fg.a)))
            .child(text(m).font(s.cfg.font(12.0, 500.0)).color(muted).wrap().align(rune::Align::Center))
    };
    let v = match voice(s) { Ok(v) => v, Err(e) => return message(e) };
    let Some(channel) = &v.channel else { return message("Not in voice") };
    let size = if short { 28.0 } else { 36.0 };
    // Room for the avatars beside the mute button; the last slot counts the rest.
    let room = (((w - 24.0 - if short { 34.0 } else { 0.0 }) + 6.0) / (size + 6.0)).floor().max(1.0) as usize;
    let n = channel.members.len();
    let shown = if n > room { room - 1 } else { n };
    let mut faces = row().key("voice-faces").gap(6.0).center_y();
    for m in channel.members.iter().take(shown) { faces = faces.child(avatar(s, m, size)) }
    if n > shown {
        faces = faces.child(el().size(size, size).center().radius(size / 2.0).fill(fg.alpha(0.14 * fg.a))
            .child(text(format!("+{}", n - shown)).font(s.cfg.font(12.0, 700.0)).color(fg)));
    }
    let speaking: Vec<&str> = channel.members.iter().filter(|m| m.speaking).map(|m| m.name.as_str()).collect();
    let label = format!("{}: {} in voice{}", channel.name, n, if speaking.is_empty() { String::new() } else { format!(", {} speaking", speaking.join(", ")) });
    if short {
        return row().key("voice").role("group").label(label).w_full().h_full().px(12.0).gap(8.0).center_y()
            .child(faces.grow(1.0).min_w(0.0)).child(mute(s, v, 28.0));
    }
    let line = if speaking.is_empty() { format!("{n} in voice") } else { speaking.join(", ") };
    col().key("voice").role("group").label(label).w_full().h_full().padding(12.0).gap(8.0)
        .child(row().w_full().gap(8.0).center_y()
            .child(col().grow(1.0).min_w(0.0).gap(2.0)
                .child(text(&channel.name).font(s.cfg.font(13.0, 700.0)).color(accent::indigo()).truncate())
                .child(text(line).font(s.cfg.font(12.0, 500.0)).color(muted).truncate()))
            .child(mute(s, v, 28.0))
            .child(deafen(s, v, 28.0)))
        .child(el().grow(1.0))
        .child(faces)
}
