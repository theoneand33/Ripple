//! Stremflix: Stremflix-rs's Continue Watching, each entry resumed in its window.

use std::collections::HashMap;

use rune::{El, Image, SurfaceFit, col, el, icon, row, text};

use super::empty;
use crate::icons;
use crate::island::{Island, fetch_image};
use crate::stremflix::{self as api, Entry};

#[derive(Default)]
pub struct Stremflix {
    /// Continue Watching, or why it couldn't load; None while loading.
    pub entries: Option<Result<Vec<Entry>, String>>,
    /// Loaded posters by URL.
    posters: HashMap<String, Image>,
    /// A play request that failed.
    status: String,
    /// Bumped on every load, so an earlier answer is dropped.
    generation: u64,
}

impl Island {
    /// Asks Stremflix-rs for Continue Watching, then fetches the posters not yet loaded.
    pub fn stremflix_load(&mut self) {
        let Some(me) = self.me.clone() else { return };
        let Some(base) = api::base() else {
            self.stremflix.entries = Some(Err("Stremflix-rs's API is off (STREMFLIX_RS_API=off)".into()));
            return;
        };
        // The last list stays up while it refreshes.
        if self.stremflix.entries.as_ref().is_some_and(Result::is_err) { self.stremflix.entries = None }
        self.stremflix.status.clear();
        self.stremflix.generation += 1;
        let generation = self.stremflix.generation;
        let cached: Vec<String> = self.stremflix.posters.keys().cloned().collect();
        std::thread::spawn(move || {
            let result = api::continue_watching(&base);
            // Posters come from addons: only the web, never file: or data: URLs.
            let posters: Vec<String> = result.iter().flatten().filter_map(|e| e.poster.clone())
                .filter(|p| (p.starts_with("https://") || p.starts_with("http://")) && !cached.contains(p)).collect();
            me.update(move |s, _| if s.stremflix.generation == generation { s.stremflix.entries = Some(result) });
            for url in posters {
                fetch_image(&me, url.clone(), true, move |s, image| { s.stremflix.posters.insert(url, image); });
            }
        });
    }

    /// Resumes an entry in Stremflix-rs's window.
    pub fn stremflix_play(&mut self, entry: Entry) {
        let (Some(base), Some(me)) = (api::base(), self.me.clone()) else { return };
        self.stremflix.status = format!("Opening {}...", entry.title);
        std::thread::spawn(move || {
            let result = api::play(&base, &entry);
            me.update(move |s, _| s.stremflix.status = result.err().unwrap_or_default());
        });
    }
}

pub fn view(s: &Island) -> El {
    let fg = s.cfg.text();
    let muted = fg.alpha(0.55 * fg.a);
    let (pw, ph) = (88.0, 132.0);
    let header = row().key("stremflix-header").w_full().h(28.0).gap(8.0).center_y()
        .child(text("Continue Watching").font(s.cfg.font(17.0, 700.0)).color(fg).single_line())
        .child(text(&s.stremflix.status).key("stremflix-status").role("status").font(s.cfg.font(12.0, 400.0)).color(muted).grow(1.0).min_w(0.0).align(rune::Align::End).truncate());
    let root = col().key("stremflix").w_full().h_full().px(16.0).pt(14.0).pb(14.0).gap(10.0).child(header);
    let entries = match &s.stremflix.entries {
        None => return root.child(empty(s, "Loading...")),
        Some(Err(e)) => return root.child(empty(s, e)),
        Some(Ok(list)) if list.is_empty() => return root.child(empty(s, "Nothing to continue")),
        Some(Ok(list)) => list,
    };
    let mut cards = row().key("stremflix-cards").gap(12.0);
    for entry in entries {
        let poster = el().key("poster").size(pw, ph).radius(s.cfg.radius(10.0)).fill(fg.alpha(0.08 * fg.a)).a11y_hidden();
        let poster = match entry.poster.as_ref().and_then(|p| s.stremflix.posters.get(p)).filter(|i| i.is_ready() && i.error().is_none()) {
            Some(image) => poster.pixels_fit(image.pixels(), SurfaceFit::Fill),
            None => poster.center().child(icon(&icons::TV, 28.0).color(muted)),
        };
        let progress = el().key("progress").w(pw).h(3.0).radius(1.5).fill(fg.alpha(0.15 * fg.a)).a11y_hidden()
            .child(el().w(pw * entry.progress()).h_full().radius(1.5).fill(fg));
        let play = entry.clone();
        cards = cards.child(
            col().key(format!("watch-{}-{}", entry.kind, entry.id)).role("button").label(format!("Play {}", entry.title)).focusable(true)
                .w(pw).gap(6.0).radius(s.cfg.radius(10.0)).hover(fg.alpha(0.06 * fg.a)).press(fg.alpha(0.12 * fg.a))
                .on_press(move |s: &mut Island| s.stremflix_play(play.clone()))
                .child(poster)
                .child(progress)
                .child(text(&entry.title).font(s.cfg.font(12.0, 500.0)).color(fg).truncate().a11y_hidden()),
        );
    }
    root.child(el().key("stremflix-scroll").w_full().grow(1.0).min_h(0.0).scroll_x().scroll_edge_fade(10.0).child(cards))
}
