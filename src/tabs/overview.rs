//! Overview: the clock and date, or the playing track with its controls; weather and battery.

use rune::{El, Transition, col, el, icon, row, text};

use crate::icons;
use crate::island::{Island, status, control, cover, estimate_width, marquee};
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
    let line = |key: &str, text: El, text_width: f32, height: f32| marquee(s, key, text, text_width, 175.0, 10.0).w(175.0).h(height);
    let details = col().key("details").grow(1.0).min_w(0.0).pl(5.0)
        .child(line("track-name", text(name).font(s.cfg.font(18.0, 600.0)).color(fg).single_line(), estimate_width(name, 18.0), 24.0))
        .child(line("track-artist", text(artist).font(s.cfg.font(13.0, 400.0)).color(fg.alpha(0.8 * fg.a)).single_line(), estimate_width(artist, 13.0), 18.0).mt(4.0))
        .child(
            row().key("controls").mt(14.0).gap(12.0).center_y()
                .child(media("previous", &icons::SKIP_BACK, 20.0, MediaCommand::Previous, "Previous"))
                .child(media("play-pause", if track.playing { &icons::PAUSE } else { &icons::PLAY }, 24.0, MediaCommand::PlayPause, if track.playing { "Pause" } else { "Play" }))
                .child(media("next", &icons::SKIP_FORWARD, 20.0, MediaCommand::Next, "Next")),
        );
    root
        .child(text(&s.time).key("time").pin(Some(0.0), Some(16.0), Some(0.0), None).align(rune::Align::Center).font(s.cfg.font(15.0, 600.0).features("'tnum' 1")).color(fg))
        .child(gear.pin(None, None, Some(14.0), Some(10.0)))
        .child(
            row().key(format!("track-{}-{}", track.name, track.artist)).pin(Some(17.0), Some(44.0), Some(14.0), Some(10.0)).gap(8.0).center_y()
                .opacity(if track.playing { 1.0 } else { 0.5 })
                .transition(Transition::blur_replace())
                .child(cover(s, 110.0, 13.0, 40.0, 1.08))
                .child(details),
        )
}
