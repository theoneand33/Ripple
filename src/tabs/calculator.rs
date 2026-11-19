//! Calculator: type a sum, see the answer as you type, press Enter to keep it on the tape.
//! Handles + - * / % ^, brackets and decimals.

use rune::{El, TextEdit, Tx, col, row, text};

use super::{Scrolls, empty, field, pill};
use crate::island::Island;

#[derive(Default)]
pub struct Calculator {
    pub input: TextEdit,
    /// (sum, answer), newest first.
    pub tape: Vec<(String, String)>,
}

/// The answer, or None for a sum that doesn't parse or has no finite answer.
pub fn eval(sum: &str) -> Option<f64> {
    // The parser recurses per bracket and sign: a long paste would overflow the stack.
    if sum.len() > 200 { return None }
    // "1 2" is two numbers, not 12.
    let digit = |c: char| c.is_ascii_digit() || c == '.';
    let mut prev = None;
    for (i, c) in sum.char_indices() {
        if c.is_whitespace() { continue }
        if digit(c) && prev.is_some_and(|(j, p): (usize, char)| digit(p) && j + p.len_utf8() < i) { return None }
        prev = Some((i, c));
    }
    let tokens: Vec<char> = sum.chars().filter(|c| !c.is_whitespace()).map(|c| match c { '×' | 'x' => '*', '÷' => '/', '−' => '-', c => c }).collect();
    let mut p = Parser { t: &tokens, i: 0 };
    let v = p.expr()?;
    (p.i == tokens.len() && v.is_finite()).then_some(v)
}

struct Parser<'a> { t: &'a [char], i: usize }

impl Parser<'_> {
    fn eat(&mut self, c: char) -> bool { let hit = self.t.get(self.i) == Some(&c); if hit { self.i += 1 } hit }

    fn expr(&mut self) -> Option<f64> {
        let mut v = self.term()?;
        loop {
            if self.eat('+') { v += self.term()? } else if self.eat('-') { v -= self.term()? } else { return Some(v) }
        }
    }

    fn term(&mut self) -> Option<f64> {
        let mut v = self.power()?;
        loop {
            if self.eat('*') { v *= self.power()? } else if self.eat('/') { v /= self.power()? } else if self.eat('%') { v %= self.power()? } else { return Some(v) }
        }
    }

    /// Right-associative, and above unary minus on its left: -2^2 is -4.
    fn power(&mut self) -> Option<f64> {
        if self.eat('-') { return Some(-self.power()?) }
        if self.eat('+') { return self.power() }
        let base = self.atom()?;
        if self.eat('^') { Some(base.powf(self.power()?)) } else { Some(base) }
    }

    fn atom(&mut self) -> Option<f64> {
        if self.eat('(') { let v = self.expr()?; return self.eat(')').then_some(v) }
        let start = self.i;
        while self.t.get(self.i).is_some_and(|c| c.is_ascii_digit() || *c == '.') { self.i += 1 }
        self.t[start..self.i].iter().collect::<String>().parse().ok()
    }
}

/// Up to 10 significant decimals, without trailing zeros.
pub fn format(v: f64) -> String {
    let s = format!("{v:.10}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

fn submit(s: &mut Island, _: &mut Tx) {
    let sum = s.calc.input.text.trim().to_string();
    let Some(v) = eval(&sum) else { return };
    let answer = format(v);
    s.calc.tape.insert(0, (sum, answer.clone()));
    s.calc.tape.truncate(50);
    // The answer stays in the field, so the next sum can build on it.
    s.calc.input.replace_text(answer);
}

pub fn view(s: &Island) -> El {
    let fg = s.cfg.text();
    let sum = s.calc.input.text.trim();
    let preview = if sum.is_empty() { String::new() } else { eval(sum).map_or("…".into(), |v| format!("= {}", format(v))) };
    let mut tape = col().key("calc-tape").w_full().gap(4.0);
    if s.calc.tape.is_empty() { tape = tape.child(empty(s, "Type a sum and press Enter")) }
    for (i, (sum, answer)) in s.calc.tape.iter().enumerate() {
        tape = tape.child(
            row().key(format!("tape-{}", s.calc.tape.len() - i)).w_full().px(12.0).py(6.0).gap(8.0).center_y()
                .radius(s.cfg.radius(12.0)).fill(super::card(fg))
                .child(text(sum).font(s.cfg.font(13.0, 400.0)).color(fg.alpha(0.6 * fg.a)).grow(1.0).min_w(0.0).max_lines(1))
                .child(text(answer).font(s.cfg.font(14.0, 600.0).features("'tnum' 1")).color(fg)),
        );
    }
    let mut header = row().key("calc-result").w_full().h(40.0).gap(8.0).center_y()
        .child(text(preview).key("calc-preview").font(s.cfg.font(28.0, 600.0).features("'tnum' 1")).color(fg).grow(1.0).min_w(0.0).max_lines(1));
    if !s.calc.tape.is_empty() { header = header.child(pill(s, "calc-clear", "Clear").on_press(|s: &mut Island| s.calc.tape.clear())) }
    col().key("calculator").w_full().h_full().px(16.0).pt(14.0).pb(14.0).gap(10.0)
        .child(header)
        .child(col().key("calc-scroll").w_full().grow(1.0).min_h(0.0).scrolls().child(tape))
        .child(row().key("calc-row").w_full().child(field(s, "calc-input", &s.calc.input, "12 × (3 + 4)", |s: &mut Island| &mut s.calc.input, submit)))
}

#[cfg(test)]
mod tests {
    use super::{eval, format};

    #[test]
    fn sums_follow_the_usual_order() {
        assert_eq!(eval("1 + 2 * 3"), Some(7.0));
        assert_eq!(eval("(1 + 2) × 3"), Some(9.0));
        assert_eq!(eval("2^3^2"), Some(512.0));
        assert_eq!(eval("-2^2"), Some(-4.0));
        assert_eq!(eval("10 % 4 - -1"), Some(3.0));
        assert_eq!(eval("1.5 ÷ .5"), Some(3.0));
        assert_eq!(eval("1 / 0"), None);
        assert_eq!(eval("2 +"), None);
        assert_eq!(eval("(2"), None);
        assert_eq!(eval("1..2"), None);
        assert_eq!(eval("1 2"), None);
        assert_eq!(eval("12 + 3"), Some(15.0));
        assert_eq!(eval(&"(".repeat(5000)), None);
        assert_eq!(format(0.1 + 0.2), "0.3");
        assert_eq!(format(-0.0), "0");
        assert_eq!(format(42.0), "42");
    }
}
