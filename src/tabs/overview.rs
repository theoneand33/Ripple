//! Overview: the clock and date, or the playing track with its controls; weather and battery.

use rune::{El, Transition, col, el, icon, row, text};

use super::accent;
use crate::icons;
use crate::island::{Island, status, control, cover, estimate_width, marquee, visualizer};
use crate::system::MediaCommand;

pub fn view(s: &Island) -> El {
    let (bg, fg) = (s.cfg.bg().alpha(1.0), s.cfg.text());
    let muted = fg.alpha(0.6 * fg.a);
    let weather = row().key("weather").absolute(24.0, 20.0).gap(4.0).center_y()
        .child(icon(icons::weather(&s.weather.status), 16.0).color(fg))
        .child(text(s.weather.temp.map_or("??º".to_string(), |t| format!("{t}º"))).font(s.cfg.font(15.0, 500.0)).color(fg));
    let mut root = el().key("overview").w_full().h_full();
    if !s.cfg.location.trim().is_empty() { root = root.child(weather) }
    if let Some(percent) = s.percent {
        let mut battery = row().key("battery").pin(None, Some(20.0), Some(24.0), None).px(8.0).h(24.0).gap(2.0).center_y()
            .radius(s.cfg.radius(8.0)).fill(if s.low_battery() { status::low() } else { fg });
        if s.charging { battery = battery.child(icon(&icons::ZAP, 14.0).color(bg)) }
        root = root.child(battery.child(text(format!("{percent}%")).font(s.cfg.font(13.0, 500.0).features("'tnum' 1")).color(bg)));
    }
    let gear = el().key("open-settings").role("button").label("Open settings").focusable(true)
        .size(28.0, 28.0).center().radius(s.cfg.radius(8.0)).hover(fg.alpha(0.1 * fg.a)).press(fg.alpha(0.2 * fg.a))
        .child(icon(&icons::SETTINGS, 16.0).color(muted))
        .on_press(|s: &mut Island| if let Some(settings) = &s.settings { settings.open() });

    let Some(track) = &s.track else {
        return root
            .child(gear.pin(Some(14.0), None, None, Some(10.0)))
            .child(
                col().key("date").w_full().h_full().center().gap(8.0)
                    .child(text(&s.time).key("time").font(s.cfg.font(52.0, 600.0).lh(52.0).features("'tnum' 1")).color(fg))
                    .child(text(&s.date).font(s.cfg.font(15.0, 500.0)).color(muted)),
            );
    };

    let name = if track.name.is_empty() { "Unknown Title" } else { &track.name };
    let artist = if track.artist.is_empty() { "Unknown Artist" } else { &track.artist };
    let media = |key: &'static str, glyph: &'static rune::Icon, size: f32, command: MediaCommand, label: &str| {
        el().key(key).role("button").label(label).focusable(true)
            .padding(4.0).radius(s.cfg.radius(8.0)).hover(fg.alpha(0.1 * fg.a)).press(fg.alpha(0.2 * fg.a)).press_scale(0.97)
            .child(icon(glyph, size).color(fg))
            .on_press(move |s: &mut Island| control(s, command))
    };
    // Droppy's player: cover and title on top, the progress bar with both times, then the controls.
    const TITLE_W: f32 = 250.0;
    let line = |key: &str, text: El, text_width: f32, height: f32| marquee(s, key, text, text_width, TITLE_W, 10.0).w(TITLE_W).h(height);
    let head = row().key("track-head").w_full().gap(12.0).center_y()
        .child(cover(s, 56.0, 12.0, 24.0, 1.06))
        .child(col().key("details").grow(1.0).min_w(0.0)
            .child(line("track-name", text(name).font(s.cfg.font(17.0, 700.0)).color(fg).single_line(), estimate_width(name, 17.0), 22.0))
            .child(line("track-artist", text(artist).font(s.cfg.font(14.0, 500.0)).color(muted).single_line(), estimate_width(artist, 14.0), 18.0)))
        .child(visualizer(track.playing, accent::pink()));
    let mut body = col().key(format!("track-{}-{}", track.name, track.artist)).pin(Some(20.0), Some(46.0), Some(20.0), None).gap(12.0)
        .opacity(if track.playing { 1.0 } else { 0.5 })
        .transition(Transition::blur_replace())
        .child(head);
    if let Some((position, length)) = track.progress {
        let clock = |t: u32| format!("{}:{:02}", t / 60, t % 60);
        let time = |t: String| text(t).font(s.cfg.font(12.0, 600.0).features("'tnum' 1")).color(muted);
        body = body.child(row().key("track-progress").role("progressbar").label(format!("{} of {}", clock(position), clock(length))).w_full().gap(10.0).center_y()
            .child(time(clock(position)).a11y_hidden())
            .child(el().grow(1.0).h(6.0).radius(3.0).fill(fg.alpha(0.15 * fg.a)).a11y_hidden()
                .child(el().h_full().w_frac(position.min(length) as f32 / length as f32).radius(3.0).fill(fg.alpha(0.55 * fg.a))))
            .child(time(format!("−{}", clock(length.saturating_sub(position)))).a11y_hidden()));
    }
    body = body.child(row().key("controls").w_full().gap(36.0).center()
        .child(media("previous", &icons::SKIP_BACK, 22.0, MediaCommand::Previous, "Previous"))
        .child(media("play-pause", if track.playing { &icons::PAUSE } else { &icons::PLAY }, 28.0, MediaCommand::PlayPause, if track.playing { "Pause" } else { "Play" }))
        .child(media("next", &icons::SKIP_FORWARD, 22.0, MediaCommand::Next, "Next")));
    root
        .child(text(&s.time).key("time").pin(Some(0.0), Some(16.0), Some(0.0), None).align(rune::Align::Center).font(s.cfg.font(15.0, 600.0).features("'tnum' 1")).color(fg))
        .child(gear.pin(None, None, Some(14.0), Some(10.0)))
        .child(body)
}
