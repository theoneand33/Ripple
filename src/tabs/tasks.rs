//! Tasks: a to-do list; ticking a task off removes it. With a Nextcloud account the list syncs
//! with Nextcloud Tasks, and a ticked task is marked done there.

use rune::{El, Transition, Tx, col, row, text};

use super::{Scrolls, button, checkbox, empty, field};
use crate::config::Item;
use crate::island::Island;
use crate::nextcloud::{self, Kind};

fn add_task(s: &mut Island, _: &mut Tx) {
    let task = s.task.text.trim().to_string();
    if task.is_empty() { return }
    s.save(|c| c.tasks.push(Item::new(task)));
    s.task.clear();
    s.nextcloud_sync(Kind::Tasks);
}

/// By key, not index: a sync may have removed a task above it since the list was drawn.
fn tick(s: &mut Island, key: u64) {
    s.save(|c| { nextcloud::remove(c, Kind::Tasks, key); });
    s.nextcloud_sync(Kind::Tasks);
}

pub fn view(s: &Island) -> El {
    let fg = s.cfg.text();
    let mut list = col().key("task-list").w_full().gap(6.0);
    if let Some(e) = &s.nextcloud[Kind::Tasks as usize].error {
        list = list.child(text(e).font(s.cfg.font(12.0, 400.0)).color(fg.alpha(0.55 * fg.a)).wrap_anywhere());
    }
    if s.cfg.tasks.is_empty() { list = list.child(empty(s, "No tasks yet")) }
    for task in &s.cfg.tasks {
        let (key, task) = (task.key, &task.text);
        list = list.child(
            row().key(format!("task-{key}")).w_full().px(12.0).py(8.0).gap(10.0).center_y()
                .radius(s.cfg.radius(16.0)).fill(super::card(fg))
                .transition(Transition::opacity())
                .child(checkbox(s, "done", &format!("Done: {task}"), false).on_press(move |s: &mut Island| tick(s, key)))
                .child(text(task).font(s.cfg.font(14.0, 500.0)).color(fg).grow(1.0).min_w(0.0).wrap_anywhere()),
        );
    }
    col().key("tasks").w_full().h_full().px(16.0).pt(18.0).pb(14.0).gap(10.0)
        .child(col().key("task-scroll").w_full().grow(1.0).min_h(0.0).scrolls().child(list))
        .child(
            row().key("task-input").w_full().gap(8.0).center_y()
                .child(field(s, "new-task", &s.task, "New task…", |s: &mut Island| &mut s.task, add_task))
                .child(button(s, "add-task", "Add", true).on_press_tx(add_task)),
        )
}
