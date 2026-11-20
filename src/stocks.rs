//! Stocks: watchlist quotes from Yahoo Finance's chart API (no key), and the portfolio from a
//! Sure server's API (`X-Api-Key`). Blocking; callers run them off the UI thread.

use std::collections::BTreeMap;

use serde_json::Value;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Range { #[default] Day, Week, Month, All }

impl Range {
    pub const ALL: [Range; 4] = [Range::Day, Range::Week, Range::Month, Range::All];

    pub fn label(self) -> &'static str {
        match self { Range::Day => "1D", Range::Week => "7D", Range::Month => "30D", Range::All => "All" }
    }

    /// Yahoo's (range, interval).
    fn yahoo(self) -> (&'static str, &'static str) {
        match self { Range::Day => ("1d", "5m"), Range::Week => ("5d", "30m"), Range::Month => ("1mo", "1d"), Range::All => ("max", "1mo") }
    }

    /// Days of Sure's daily balances; None for all of them.
    fn days(self) -> Option<i64> {
        match self { Range::Day => Some(1), Range::Week => Some(7), Range::Month => Some(30), Range::All => None }
    }
}

/// A ticker as Yahoo spells it ("AAPL", "BRK-B", "VWCE.DE", "^GSPC", "BTC-USD", "EURUSD=X"); None if it can't be one.
pub fn ticker(input: &str) -> Option<String> {
    let t = input.trim().to_uppercase();
    (1..=15).contains(&t.len()).then_some(())?;
    (t.chars().all(|c| c.is_ascii_alphanumeric() || ".-^=".contains(c)) && t.chars().any(|c| c.is_ascii_alphanumeric())).then_some(t)
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Quote {
    pub name: String,
    pub currency: String,
    pub price: f64,
    /// Since the range began.
    pub change: f64,
    /// Closes over the range, oldest first.
    pub points: Vec<f64>,
}

impl Quote {
    pub fn percent(&self) -> f64 { percent(self.change, self.price - self.change) }
}

pub fn percent(change: f64, base: f64) -> f64 { if base == 0.0 { 0.0 } else { change / base * 100.0 } }

pub fn quote(symbol: &str, range: Range) -> Result<Quote, String> { quote_from("https://query1.finance.yahoo.com", symbol, range) }

fn quote_from(base: &str, symbol: &str, range: Range) -> Result<Quote, String> {
    let (r, i) = range.yahoo();
    // `ticker` leaves `^` as the only character a path needs escaped.
    let mut res = crate::net::agent().get(format!("{base}/v8/finance/chart/{}", symbol.replace('^', "%5E")))
        .query("range", r).query("interval", i)
        // Yahoo turns away clients that don't look like a browser.
        .header("user-agent", "Mozilla/5.0")
        .call().map_err(|_| "Couldn't reach Yahoo Finance".to_string())?;
    let body: Value = res.body_mut().read_to_string().ok().and_then(|b| serde_json::from_str(&b).ok()).unwrap_or_default();
    let chart = &body["chart"]["result"][0];
    let meta = &chart["meta"];
    let Some(price) = meta["regularMarketPrice"].as_f64() else {
        let status = res.status().as_u16();
        return Err(body["chart"]["error"]["description"].as_str().map_or_else(|| if status == 200 { format!("No quote for {symbol}") } else { format!("Yahoo Finance answered {status}") }, str::to_string));
    };
    let points: Vec<f64> = chart["indicators"]["quote"][0]["close"].as_array().map(|c| c.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
    // Under `max` the close before the chart is not its start.
    let previous = if range == Range::All { None } else { meta["chartPreviousClose"].as_f64() };
    let start = previous.or(points.first().copied()).unwrap_or(price);
    let name = [&meta["shortName"], &meta["longName"]].into_iter().find_map(Value::as_str).unwrap_or(symbol).to_string();
    Ok(Quote { name, currency: meta["currency"].as_str().unwrap_or_default().into(), price, change: price - start, points })
}

/// One investment or crypto account in Sure.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Account {
    pub name: String,
    pub balance: f64,
    pub cash: f64,
    pub positions: usize,
    /// What the positions cost, where Sure knows their average cost.
    pub cost: f64,
    /// What those same positions are worth now.
    pub value: f64,
}

impl Account {
    pub fn open(&self) -> f64 { self.value - self.cost }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Portfolio {
    pub currency: String,
    pub accounts: Vec<Account>,
    /// Every account's balance summed by day, oldest first.
    pub history: Vec<f64>,
}

impl Portfolio {
    pub fn total(&self) -> f64 { self.accounts.iter().map(|a| a.balance).sum() }
    pub fn cash(&self) -> f64 { self.accounts.iter().map(|a| a.cash).sum() }
    pub fn cost(&self) -> f64 { self.accounts.iter().map(|a| a.cost).sum() }
    pub fn open(&self) -> f64 { self.accounts.iter().map(Account::open).sum() }
    /// The change over the last day of `history`, and over all of it.
    pub fn changes(&self) -> (f64, f64) {
        let h = &self.history;
        if h.len() < 2 { return (0.0, 0.0) }
        (h[h.len() - 1] - h[h.len() - 2], h[h.len() - 1] - h[0])
    }
}

/// `amount_cents` are minor units: hundredths, except in the currencies that have none.
fn minor(currency: &str) -> f64 {
    if matches!(currency, "JPY" | "KRW" | "VND" | "CLP" | "ISK" | "PYG" | "UGX") { 1.0 } else { 100.0 }
}

/// A Money#format string ("$1,234.56", "1.234,56 €", "¥1,234", "-$5.00") as a number: the last
/// `.` or `,` is the decimal mark unless three digits follow it.
fn money(s: &str) -> Option<f64> {
    let kept: String = s.chars().filter(|c| c.is_ascii_digit() || *c == '.' || *c == ',').collect();
    let digits = |p: &str| p.chars().filter(char::is_ascii_digit).collect::<String>();
    let n = match kept.rfind(['.', ',']) {
        Some(i) if kept.len() - i - 1 != 3 => format!("{}.{}", digits(&kept[..i]), &kept[i + 1..]),
        _ => digits(&kept),
    };
    let n: f64 = n.parse().ok()?;
    Some(if s.contains('-') { -n } else { n })
}

/// `GET {url}/api/v1/{path}`, every page of it up to `max_pages`, each page's `field` array joined.
fn pages(url: &str, key: &str, path: &str, query: &[(&str, String)], field: &str, max_pages: u32) -> Result<Vec<Value>, String> {
    let mut all = Vec::new();
    for page in 1..=max_pages {
        let mut req = crate::net::agent().get(format!("{url}/api/v1/{path}")).header("x-api-key", key)
            .query("per_page", "100").query("page", page.to_string());
        for (k, v) in query { req = req.query(*k, v) }
        let mut res = req.call().map_err(|_| "Couldn't reach Sure".to_string())?;
        let status = res.status().as_u16();
        let body: Value = res.body_mut().read_to_string().ok().and_then(|b| serde_json::from_str(&b).ok()).unwrap_or_default();
        match status {
            200 => {}
            401 => return Err("Sure refused the API key".into()),
            _ => return Err(body["message"].as_str().map_or_else(|| format!("Sure answered {status}"), str::to_string)),
        }
        all.extend(body[field].as_array().cloned().unwrap_or_default());
        if i64::from(page) >= body["pagination"]["total_pages"].as_i64().unwrap_or(0) { break }
    }
    Ok(all)
}

/// The day `days` before today (UTC), as `YYYY-MM-DD`.
fn days_ago(days: i64) -> String {
    let t = rune::time::LocalTime::at_offset(crate::date::now_ms() - days * 86_400_000, 0);
    format!("{:04}-{:02}-{:02}", t.year, t.month, t.day)
}

/// Sure's investment and crypto accounts, their positions, and their balances over `range`.
pub fn portfolio(url: &str, key: &str, range: Range) -> Result<Portfolio, String> {
    let url = if url.trim().is_empty() { "https://app.sure.am" } else { url.trim().trim_end_matches('/') };
    // shortcut: the first 100 accounts only, enough for a person's own accounts.
    let accounts: Vec<Value> = pages(url, key, "accounts", &[], "accounts", 1)?.into_iter()
        .filter(|a| matches!(a["account_type"].as_str(), Some("investment" | "crypto"))).collect();
    let currency = accounts.first().and_then(|a| a["currency"].as_str()).unwrap_or("USD").to_string();
    let ids: Vec<&str> = accounts.iter().filter_map(|a| a["id"].as_str()).collect();
    // The last week's holdings: each account's latest day of them is what it holds now.
    let holdings = if ids.is_empty() { Vec::new() } else { pages(url, key, "holdings", &[("start_date", days_ago(7))], "holdings", 20)? };
    let mut history: BTreeMap<String, f64> = BTreeMap::new();
    let mut out = Vec::new();
    for a in &accounts {
        let id = a["id"].as_str().unwrap_or_default();
        let cur = a["currency"].as_str().unwrap_or(&currency);
        let mine: Vec<&Value> = holdings.iter().filter(|h| h["account"]["id"].as_str() == Some(id)).collect();
        let latest = mine.iter().filter_map(|h| h["date"].as_str()).max().unwrap_or_default();
        let mut acct = Account {
            name: a["name"].as_str().unwrap_or_default().into(),
            balance: a["balance_cents"].as_f64().unwrap_or(0.0) / minor(cur),
            cash: a["cash_balance_cents"].as_f64().unwrap_or(0.0) / minor(cur),
            ..Account::default()
        };
        for h in mine.iter().filter(|h| h["date"].as_str() == Some(latest)) {
            let qty: f64 = h["qty"].as_str().and_then(|q| q.parse().ok()).unwrap_or(0.0);
            if qty == 0.0 { continue }
            acct.positions += 1;
            if let (Some(avg), Some(amount)) = (h["avg_cost"].as_str().and_then(money), h["amount"].as_str().and_then(money)) {
                (acct.cost, acct.value) = (acct.cost + qty * avg, acct.value + amount);
            }
        }
        let since: Vec<(&str, String)> = range.days().map(|d| ("start_date", days_ago(d))).into_iter().collect();
        let mut query = vec![("account_id", id.to_string())];
        query.extend(since);
        // shortcut: up to 2,000 days of balances per account under All.
        for b in pages(url, key, "balances", &query, "balances", 20)? {
            if let (Some(day), Some(cents)) = (b["date"].as_str(), b["end_balance_cents"].as_f64()) {
                *history.entry(day.into()).or_default() += cents / minor(cur);
            }
        }
        out.push(acct);
    }
    // shortcut: totals add each account in its own currency; right while they share one.
    Ok(Portfolio { currency, accounts: out, history: history.into_values().collect() })
}

/// "€34,721.55": the currency's sign, thousands grouped, two decimals.
pub fn format_money(v: f64, currency: &str) -> String {
    let neg = if v < 0.0 { "-" } else { "" };
    let n = group(v.abs(), if minor(currency) == 1.0 { 0 } else { 2 });
    match currency {
        "USD" => format!("{neg}${n}"),
        "EUR" => format!("{neg}€{n}"),
        "GBP" => format!("{neg}£{n}"),
        "JPY" => format!("{neg}¥{n}"),
        "INR" => format!("{neg}₹{n}"),
        "" => format!("{neg}{n}"),
        c => format!("{neg}{n} {c}"),
    }
}

/// "+€312.49" or "-€5.00".
pub fn signed_money(v: f64, currency: &str) -> String {
    format!("{}{}", if v < 0.0 { "-" } else { "+" }, format_money(v.abs(), currency))
}

fn group(v: f64, decimals: usize) -> String {
    let s = format!("{v:.decimals$}");
    let (int, frac) = s.split_once('.').map_or((s.as_str(), ""), |(i, f)| (i, f));
    let mut out = String::new();
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 { out.push(',') }
        out.push(c);
    }
    if frac.is_empty() { out } else { format!("{out}.{frac}") }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testserver::{json, serve};

    #[test]
    fn reads_money_and_formats_it() {
        assert_eq!(money("$1,234.56"), Some(1234.56));
        assert_eq!(money("1.234,56 €"), Some(1234.56));
        assert_eq!(money("¥1,234"), Some(1234.0));
        assert_eq!(money("-$5.00"), Some(-5.0));
        assert_eq!(money("$0.1234"), Some(0.1234));
        assert_eq!(format_money(34721.554, "EUR"), "€34,721.55");
        assert_eq!(format_money(-5.0, "USD"), "-$5.00");
        assert_eq!(format_money(1234.5, "CHF"), "1,234.50 CHF");
        assert_eq!(signed_money(-312.4, "EUR"), "-€312.40");
        assert_eq!(signed_money(999.0, "JPY"), "+¥999");
        assert_eq!((ticker(" brk-b "), ticker("^gspc"), ticker("a b"), ticker(""), ticker("..")), (Some("BRK-B".into()), Some("^GSPC".into()), None, None, None));
    }

    #[test]
    fn quotes_a_ticker() {
        let base = serve(|r| match r.path.split('?').next() {
            Some("/v8/finance/chart/%5EGSPC") => json(200, serde_json::json!({"chart": {"result": [{
                "meta": {"regularMarketPrice": 110.0, "chartPreviousClose": 100.0, "currency": "USD", "shortName": "S&P 500"},
                "indicators": {"quote": [{"close": [101.0, null, 105.0, 110.0]}]},
            }], "error": null}})),
            _ => json(404, serde_json::json!({"chart": {"result": null, "error": {"description": "No data found, symbol may be delisted"}}})),
        });
        let q = quote_from(&base, "^GSPC", Range::Day).unwrap();
        assert_eq!((q.name.as_str(), q.price, q.change, q.percent(), q.points.len()), ("S&P 500", 110.0, 10.0, 10.0, 3));
        assert_eq!(quote_from(&base, "^GSPC", Range::All).unwrap().change, 9.0);
        assert_eq!(quote_from(&base, "NOPE", Range::Day), Err("No data found, symbol may be delisted".into()));
    }

    #[test]
    fn sums_a_sure_portfolio() {
        let base = serve(|r| {
            if r.header("x-api-key") != Some("k") { return json(401, serde_json::json!({"error": "unauthorized"})) }
            let page = |items: &str, v: serde_json::Value| { let mut b = serde_json::json!({"pagination": {"page": 1, "per_page": 100, "total_count": 1, "total_pages": 1}}); b[items] = v; json(200, b) };
            match r.path.split('?').next().unwrap() {
                "/api/v1/accounts" => page("accounts", serde_json::json!([
                    {"id": "a", "name": "Broker", "balance_cents": 1_000_00, "cash_balance_cents": 100_00, "currency": "EUR", "account_type": "investment"},
                    {"id": "b", "name": "Checking", "balance_cents": 5_00, "cash_balance_cents": 5_00, "currency": "EUR", "account_type": "depository"},
                ])),
                "/api/v1/holdings" => page("holdings", serde_json::json!([
                    {"date": "2026-01-01", "qty": "10", "amount": "€800,00", "avg_cost": "€70,00", "account": {"id": "a"}},
                    {"date": "2026-01-02", "qty": "10", "amount": "€900,00", "avg_cost": "€70,00", "account": {"id": "a"}},
                    {"date": "2026-01-02", "qty": "0", "amount": "€0,00", "avg_cost": null, "account": {"id": "a"}},
                ])),
                "/api/v1/balances" => page("balances", serde_json::json!([
                    {"date": "2026-01-02", "end_balance_cents": 1_000_00},
                    {"date": "2026-01-01", "end_balance_cents": 900_00},
                ])),
                p => json(404, serde_json::json!({"error": "not_found", "message": format!("no {p}")})),
            }
        });
        let p = portfolio(&format!("{base}/"), "k", Range::Week).unwrap();
        assert_eq!(p.accounts.len(), 1);
        let a = &p.accounts[0];
        assert_eq!((a.balance, a.cash, a.positions, a.cost, a.value), (1000.0, 100.0, 1, 700.0, 900.0));
        assert_eq!((p.history.clone(), p.changes(), p.open()), (vec![900.0, 1000.0], (100.0, 100.0), 200.0));
        assert_eq!(portfolio(&base, "bad", Range::Day), Err("Sure refused the API key".into()));
    }
}
