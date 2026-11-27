//! The Calculator's sums and unit and currency conversions (`5 km to mi`, `20 usd in eur`),
//! and the exchange rates they use. Handles + - * / % ^, brackets and decimals.

use std::collections::HashMap;

/// Units per US dollar, by code, as last fetched. Calculator, Search and Stocks convert with them.
/// Stale rates stay in use until a refetch succeeds.
#[derive(Default)]
pub struct Rates {
    rates: Option<HashMap<String, f64>>,
    /// Wall-clock ms when `rates` were fetched: it counts the time a laptop sleeps.
    at: i64,
    /// A fetch is running.
    pub asked: bool,
}

/// The ECB publishes reference rates once a working day.
const RATES_FOR_MS: i64 = 24 * 60 * 60 * 1000;

impl Rates {
    pub fn get(&self) -> Option<&HashMap<String, f64>> { self.rates.as_ref() }

    /// Whether the rates were fetched within the last day.
    pub fn fresh(&self) -> bool {
        self.rates.is_some() && (0..RATES_FOR_MS).contains(&(crate::date::now_ms() - self.at))
    }

    /// Keeps newly fetched rates; a failed fetch keeps the old ones.
    pub fn got(&mut self, rates: Option<HashMap<String, f64>>) {
        if rates.is_some() { self.rates = rates; self.at = crate::date::now_ms() }
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
pub fn answer(text: &str, rates: Option<&HashMap<String, f64>>) -> Option<String> {
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

#[cfg(test)]
mod tests {
    use super::*;

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
        let mut kept = Rates::default();
        assert!(!kept.fresh());
        kept.got(Some(rates));
        assert!(kept.fresh());
        kept.got(None);
        assert!(kept.get().is_some(), "a failed refetch keeps the old rates");
        kept.at -= RATES_FOR_MS;
        assert!(!kept.fresh(), "a day old");
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
