//! Workflows & QA: saved sets of sites and apps opened together, and the quick apps.

use rune::{Color, El, Transition, col, el, row, text};

use super::{Scrolls, empty};
use crate::island::Island;
use crate::system;

pub fn view(s: &Island) -> El {
    let (bg, fg) = (s.cfg.bg().alpha(1.0), s.cfg.text());
    let radius = s.cfg.radius(12.0);
    let mut list = col().key("workflow-list").w_full().px(14.0).py(14.0).gap(10.0);
    if s.cfg.workflows.is_empty() {
        list = list.child(empty(s, "No workflows yet, add one in Settings"));
    }
    for (i, workflow) in s.cfg.workflows.iter().enumerate() {
        let urls = workflow.urls.clone();
        list = list.child(
            row().key(format!("workflow-{i}-{}", workflow.name)).role("button").label(workflow.name.clone()).focusable(true)
                .w_full().h(40.0).px(14.0).gap(5.0).center_y().radius(radius)
                .fill(fg).hover(bg.alpha(0.1)).press(bg.alpha(0.2)).press_scale(0.97)
                .shadow(4.0, 15.0, Color::BLACK.alpha(0.1))
                .transition(Transition::opacity())
                .child(text(&workflow.name).font(s.cfg.font(14.0, 600.0)).color(bg).truncate().a11y_hidden())
                .child(text(format!("({} sites)", urls.len())).font(s.cfg.font(14.0, 400.0)).color(bg.alpha(0.6)).a11y_hidden())
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
                .h(34.0).px(16.0).center().radius(s.cfg.radius(17.0))
                .fill(fg).hover(bg.alpha(0.1)).press(bg.alpha(0.2)).press_scale(0.97)
                .transition(Transition::opacity())
                .child(text(&app.name).font(s.cfg.font(13.0, 600.0)).color(bg).single_line().a11y_hidden())
                .on_press(move |_: &mut Island| {
                    let launch = launch.clone();
                    std::thread::spawn(move || system::open_app(&launch));
                }),
        );
    }
    col().key("workflows").w_full().h_full()
        .child(col().key("workflow-scroll").w_full().grow(1.0).min_h(0.0).scrolls().child(list))
        .child(
            el().key("quick-app-strip").w_full().py(12.0)
                .fill(fg.alpha(0.02 * fg.a)).border_sides([1.0, 0.0, 0.0, 0.0], fg.alpha(0.1 * fg.a))
                .scroll_x().child(apps),
        )
}
