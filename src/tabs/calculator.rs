//! Calculator: type a sum, see the answer as you type, press Enter to keep it on the tape,
//! which is saved. Handles + - * / % ^, brackets and decimals, and converts units and
//! currencies: `5 km to mi`, `20 usd in eur`.

use std::collections::HashMap;

use rune::{El, TextEdit, Tx, col, row, text};

use super::{Scrolls, empty, field, pill};
use crate::island::Island;

#[derive(Default)]
pub struct Calculator {
    pub input: TextEdit,
    /// Units per US dollar, by code; None until loaded. Stocks converts with them too.
    /// Stale rates stay in use until a refetch succeeds.
    pub rates: Option<HashMap<String, f64>>,
    /// Wall-clock ms when `rates` were fetched: it counts the time a laptop sleeps.
    rates_at: i64,
    rates_asked: bool,
}

/// The ECB publishes reference rates once a working day.
const RATES_FOR_MS: i64 = 24 * 60 * 60 * 1000;

impl Calculator {
    /// Whether `rates` were fetched within the last day.
    pub fn rates_fresh(&self) -> bool {
        self.rates.is_some() && (0..RATES_FOR_MS).contains(&(crate::date::now_ms() - self.rates_at))
    }

    /// Keeps newly fetched rates; a failed fetch keeps the old ones.
    pub fn got_rates(&mut self, rates: Option<HashMap<String, f64>>) {
        if rates.is_some() { self.rates = rates; self.rates_at = crate::date::now_ms() }
    }
}

impl Island {
    /// Loads exchange rates as the tab opens, unless they are under a day old.
    pub fn calc_load(&mut self) {
        if self.calc.rates_asked || self.calc.rates_fresh() { return }
        let Some(me) = self.me.clone() else { return };
        self.calc.rates_asked = true;
        me.spawn(crate::net::rates, |s, rates| { s.calc.rates_asked = false; s.calc.got_rates(rates) });
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Dim { Length, Mass, Volume, Time, Data, Speed, Temp }

/// (names, dimension, size in the dimension's base unit). Temperatures convert by `to_kelvin`.
const UNITS: &[(&[&str], Dim, f64)] = &[
    (&["m", "meter", "meters", "metre", "metres"], Dim::Length, 1.0),
    (&["km", "kilometer", "kilometers"], Dim::Length, 1000.0),
    (&["cm", "centimeter", "centimeters"], Dim::Length, 0.01),
    (&["mm", "millimeter", "millimeters"], Dim::Length, 0.001),
    (&["mi", "mile", "miles"], Dim::Length, 1609.344),
    (&["yd", "yard", "yards"], Dim::Length, 0.9144),
    (&["ft", "foot", "feet"], Dim::Length, 0.3048),
    (&["in", "inch", "inches"], Dim::Length, 0.0254),
    (&["nmi"], Dim::Length, 1852.0),
    (&["kg", "kilogram", "kilograms"], Dim::Mass, 1.0),
    (&["g", "gram", "grams"], Dim::Mass, 0.001),
    (&["mg", "milligram", "milligrams"], Dim::Mass, 1e-6),
    (&["t", "tonne", "tonnes"], Dim::Mass, 1000.0),
    (&["lb", "lbs", "pound", "pounds"], Dim::Mass, 0.45359237),
    (&["oz", "ounce", "ounces"], Dim::Mass, 0.028349523125),
    (&["st", "stone"], Dim::Mass, 6.35029318),
    (&["l", "liter", "liters", "litre", "litres"], Dim::Volume, 1.0),
    (&["ml", "milliliter", "milliliters"], Dim::Volume, 0.001),
    (&["gal", "gallon", "gallons"], Dim::Volume, 3.785411784),
    (&["qt", "quart", "quarts"], Dim::Volume, 0.946352946),
    (&["pt", "pint", "pints"], Dim::Volume, 0.473176473),
    (&["cup", "cups"], Dim::Volume, 0.2365882365),
    (&["floz"], Dim::Volume, 0.0295735295625),
    (&["tbsp"], Dim::Volume, 0.01478676478125),
    (&["tsp"], Dim::Volume, 0.00492892159375),
    (&["ms"], Dim::Time, 0.001),
    (&["s", "sec", "secs", "second", "seconds"], Dim::Time, 1.0),
    (&["min", "mins", "minute", "minutes"], Dim::Time, 60.0),
    (&["h", "hr", "hrs", "hour", "hours"], Dim::Time, 3600.0),
    (&["d", "day", "days"], Dim::Time, 86_400.0),
    (&["wk", "week", "weeks"], Dim::Time, 604_800.0),
    (&["b", "byte", "bytes"], Dim::Data, 1.0),
    (&["kb"], Dim::Data, 1e3),
    (&["mb"], Dim::Data, 1e6),
    (&["gb"], Dim::Data, 1e9),
    (&["tb"], Dim::Data, 1e12),
    (&["kib"], Dim::Data, 1024.0),
    (&["mib"], Dim::Data, 1_048_576.0),
    (&["gib"], Dim::Data, 1_073_741_824.0),
    (&["kph", "kmh"], Dim::Speed, 1.0 / 3.6),
    (&["mph"], Dim::Speed, 0.44704),
    (&["mps"], Dim::Speed, 1.0),
    (&["knot", "knots", "kn"], Dim::Speed, 0.514444),
    (&["c", "°c", "celsius"], Dim::Temp, 0.0),
    (&["f", "°f", "fahrenheit"], Dim::Temp, 0.0),
    (&["k", "kelvin"], Dim::Temp, 0.0),
];

fn unit(name: &str) -> Option<(Dim, f64, &'static str)> {
    let name = name.to_lowercase();
    UNITS.iter().find(|u| u.0.contains(&name.as_str())).map(|u| (u.1, u.2, u.0[0]))
}

fn to_kelvin(v: f64, unit: &str) -> f64 {
    match unit { "c" => v + 273.15, "f" => (v - 32.0) * 5.0 / 9.0 + 273.15, _ => v }
}

fn from_kelvin(v: f64, unit: &str) -> f64 {
    match unit { "c" => v - 273.15, "f" => (v - 273.15) * 9.0 / 5.0 + 32.0, _ => v }
}

/// `<sum> <unit> to|in <unit>`, as the answer and its unit; None for anything else.
pub fn convert(text: &str, rates: Option<&HashMap<String, f64>>) -> Option<(f64, String)> {
    // ASCII only: full lowercasing can change byte lengths (İ), and the offsets slice `text`.
    let lower = text.to_ascii_lowercase();
    // The last " to ", else the last " in ", so "12 in to cm" reads inches.
    let at = lower.rfind(" to ").map(|i| (i, 4)).or_else(|| lower.rfind(" in ").map(|i| (i, 4)))?;
    let (left, to) = (text[..at.0].trim(), text[at.0 + at.1..].trim());
    // The unit is the letters (and °) at the end of the left side.
    let split = left.char_indices().rev().take_while(|(_, c)| c.is_alphabetic() || *c == '°').last()?.0;
    let (sum, from) = (left[..split].trim(), &left[split..]);
    let v = eval(sum)?;
    if let (Some((fd, ff, fname)), Some((td, tf, tname))) = (unit(from), unit(to)) {
        if fd != td { return None }
        let out = if fd == Dim::Temp { from_kelvin(to_kelvin(v, fname), tname) } else { v * ff / tf };
        let label = match tname { "c" => "°C", "f" => "°F", "k" => "K", name => name };
        return out.is_finite().then(|| (out, label.to_string()));
    }
    let rates = rates?;
    let (from, to) = (from.to_ascii_uppercase(), to.to_ascii_uppercase());
    let out = v / rates.get(&from)? * rates.get(&to)?;
    out.is_finite().then_some((out, to))
}

/// The answer as the tape shows it: a sum's, or a conversion's with its unit.
fn answer(text: &str, rates: Option<&HashMap<String, f64>>) -> Option<String> {
    if let Some((v, unit)) = convert(text, rates) {
        let shown = if unit.len() == 3 && unit.chars().all(|c| c.is_ascii_uppercase()) { format!("{v:.2}") } else { format((v * 1e6).round() / 1e6) };
        return Some(format!("{shown} {unit}"));
    }
    eval(text).map(format)
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
    let converted = convert(&sum, s.calc.rates.as_ref()).is_some();
    let Some(answer) = answer(&sum, s.calc.rates.as_ref()) else { return };
    s.save(|c| { c.calc_tape.insert(0, (sum, answer.clone())); c.calc_tape.truncate(50) });
    // A sum's answer stays in the field, so the next sum can build on it; a conversion's has a unit.
    if converted { s.calc.input.clear() } else { s.calc.input.replace_text(answer) }
}

pub fn view(s: &Island) -> El {
    let fg = s.cfg.text();
    let sum = s.calc.input.text.trim();
    let live = if sum.is_empty() { None } else { answer(sum, s.calc.rates.as_ref()) };
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

#[cfg(test)]
mod tests {
    use super::{answer, convert, eval, format};

    #[test]
    fn units_and_currencies_convert() {
        let near = |text: &str, want: f64, unit: &str| {
            let (v, u) = convert(text, None).unwrap();
            assert!((v - want).abs() < 1e-6 && u == unit, "{text} = {v} {u}");
        };
        near("5 km to mi", 3.106855961, "mi");
        near("(2 + 1) ft in in", 36.0, "in");
        near("12 in to cm", 30.48, "cm");
        near("100 °F to C", 37.777777778, "°C");
        near("-40c to f", -40.0, "°F");
        near("0 C to kelvin", 273.15, "K");
        near("1.5 GB to MiB", 1430.511474609, "mib");
        near("2 lbs to kg", 0.90718474, "kg");
        assert_eq!(convert("5 km to kg", None), None, "length is not mass");
        assert_eq!(convert("5 km", None), None);
        assert_eq!(convert("İ 5 to °C", None), None, "no panic on text whose lowercase is longer");
        let rates = [("USD".to_string(), 1.0), ("EUR".to_string(), 0.9), ("JPY".to_string(), 150.0)].into();
        assert_eq!(answer("20 usd in eur", Some(&rates)).as_deref(), Some("18.00 EUR"));
        assert_eq!(answer("900 JPY to eur", Some(&rates)).as_deref(), Some("5.40 EUR"));
        assert_eq!(answer("20 usd in eur", None), None, "no rates yet");
        let mut calc = Calculator::default();
        assert!(!calc.rates_fresh());
        calc.got_rates(Some(rates));
        assert!(calc.rates_fresh());
        calc.got_rates(None);
        assert!(calc.rates.is_some(), "a failed refetch keeps the old rates");
        calc.rates_at -= RATES_FOR_MS;
        assert!(!calc.rates_fresh(), "a day old");
        assert_eq!(answer("1 cup to ml", None).as_deref(), Some("236.588237 ml"));
        assert_eq!(answer("2 * 3", None).as_deref(), Some("6"));
    }

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
