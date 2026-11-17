//! Tasks: a to-do list; ticking a task off removes it.

use rune::{El, ScrollSpec, Transition, Tx, col, row, text};

use super::{button, checkbox, empty, field};
use crate::island::Island;

fn add_task(s: &mut Island, _: &mut Tx) {
    let task = s.task.text.trim().to_string();
    if task.is_empty() { return }
    s.save(|c| c.tasks.push(task));
    s.task.clear();
}

pub fn view(s: &Island) -> El {
    let fg = s.cfg.text();
    let mut list = col().key("task-list").w_full().gap(6.0);
    if s.cfg.tasks.is_empty() { list = list.child(empty(s, "No tasks yet")) }
    for (i, task) in s.cfg.tasks.iter().enumerate() {
        // Keyed by text and occurrence, not index, so ticking one off leaves the rows below in place.
        let n = s.cfg.tasks[..i].iter().filter(|t| *t == task).count();
        list = list.child(
            row().key(format!("task-{task}-{n}")).w_full().px(12.0).py(8.0).gap(10.0).center_y()
                .radius(s.cfg.radius(12.0)).fill(fg.alpha(0.05 * fg.a))
                .transition(Transition::opacity())
                .child(checkbox(s, "done", &format!("Done: {task}"), false).on_press(move |s: &mut Island| s.save(|c| { if i < c.tasks.len() { c.tasks.remove(i); } })))
                .child(text(task).font(s.cfg.font(14.0, 500.0)).color(fg).grow(1.0).min_w(0.0).wrap_anywhere()),
        );
    }
    col().key("tasks").w_full().h_full().px(16.0).pt(18.0).pb(14.0).gap(10.0)
        .child(col().key("task-scroll").w_full().grow(1.0).min_h(0.0).scroll(ScrollSpec { stick_bottom: false }).child(list))
        .child(
            row().key("task-input").w_full().gap(8.0).center_y()
                .child(field(s, "new-task", &s.task, "New task...", |s: &mut Island| &mut s.task, add_task))
                .child(button(s, "add-task", "Add", true).on_press_tx(add_task)),
        )
}
