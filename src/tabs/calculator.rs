//! Calculator: type a sum, see the answer as you type, press Enter to keep it on the tape,
//! which is saved. `crate::calc` works out the answers.

use rune::{El, TextEdit, Tx, col, row, text};

use super::{Scrolls, empty, field, pill};
use crate::calc::{answer, convert};
use crate::island::Island;

#[derive(Default)]
pub struct Calculator {
    pub input: TextEdit,
}

fn submit(s: &mut Island, _: &mut Tx) {
    let sum = s.calc.input.text.trim().to_string();
    let converted = convert(&sum, s.rates.get()).is_some();
    let Some(answer) = answer(&sum, s.rates.get()) else { return };
    s.save(|c| { c.calc_tape.insert(0, (sum, answer.clone())); c.calc_tape.truncate(50) });
    // A sum's answer stays in the field, so the next sum can build on it; a conversion's has a unit.
    if converted { s.calc.input.clear() } else { s.calc.input.replace_text(answer) }
}

pub fn view(s: &Island) -> El {
    let fg = s.cfg.text();
    let sum = s.calc.input.text.trim();
    let live = if sum.is_empty() { None } else { answer(sum, s.rates.get()) };
    let preview = if sum.is_empty() { String::new() } else { live.as_ref().map_or("…".into(), |a| format!("= {a}")) };
    let tape_items = &s.cfg.calc_tape;
    let mut tape = col().key("calc-tape").w_full().gap(4.0);
    if tape_items.is_empty() { tape = tape.child(empty(s, "Type a sum and press Enter")) }
    for (i, (sum, answer)) in tape_items.iter().enumerate() {
        tape = tape.child(
            row().key(format!("tape-{}", tape_items.len() - i)).w_full().px(12.0).py(6.0).gap(8.0).center_y()
                .radius(s.cfg.radius(12.0)).fill(super::card(fg))
                .child(text(sum).font(s.cfg.font(13.0, 400.0)).color(fg.alpha(0.6 * fg.a)).grow(1.0).min_w(0.0).max_lines(1))
                .child(text(answer).font(s.cfg.font(14.0, 600.0).features("'tnum' 1")).color(fg)),
        );
    }
    let mut header = row().key("calc-result").w_full().h(40.0).gap(8.0).center_y()
        .child(text(preview).key("calc-preview").font(s.cfg.font(28.0, 600.0).features("'tnum' 1")).color(fg).grow(1.0).min_w(0.0).max_lines(1));
    if let Some(a) = live { header = header.child(pill(s, "calc-copy", "Copy").on_press_tx(move |_: &mut Island, tx| tx.copy(a.clone()))) }
    if !tape_items.is_empty() { header = header.child(pill(s, "calc-clear", "Clear").on_press(|s: &mut Island| s.save(|c| c.calc_tape.clear()))) }
    col().key("calculator").w_full().h_full().px(16.0).pt(14.0).pb(14.0).gap(10.0)
        .child(header)
        .child(col().key("calc-scroll").w_full().grow(1.0).min_h(0.0).scrolls().child(tape))
        .child(row().key("calc-row").w_full().child(field(s, "calc-input", &s.calc.input, "12 × (3 + 4), 5 km to mi, 20 usd in eur", |s: &mut Island| &mut s.calc.input, submit)))
}
