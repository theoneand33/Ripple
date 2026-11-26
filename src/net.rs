//! Ripple's own HTTP calls: weather, exchange rates, the AI assistant's stream and images by URL.
//! Blocking; callers run them off the UI thread.

use std::io::{BufRead, BufReader};
use std::time::Duration;

use serde_json::{Value, json};

/// Every Ripple client's settings: its User-Agent (GitHub refuses requests without one), and
/// error statuses come back as responses so their bodies can be read.
pub fn config() -> ureq::config::ConfigBuilder<ureq::typestate::AgentScope> {
    ureq::Agent::config_builder().user_agent("Ripple").http_status_as_error(false)
}

/// A client for short requests: 15 s for the whole call.
pub fn agent() -> ureq::Agent { config().timeout_global(Some(Duration::from_secs(15))).build().into() }

/// A response's body as JSON; `Null` when it is unreadable or not JSON.
pub fn read_json(res: &mut ureq::http::Response<ureq::Body>) -> Value {
    res.body_mut().read_to_string().ok().and_then(|b| serde_json::from_str(&b).ok()).unwrap_or_default()
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Weather {
    /// Rounded degrees in the chosen unit; None until the first answer.
    pub temp: Option<i32>,
    /// The WMO weather code, for `icons::weather`.
    pub code: Option<u64>,
    /// The next days' forecast, from tomorrow.
    pub days: Vec<Day>,
    /// Today's sunrise and sunset at the location, in Unix seconds.
    pub sun: Option<(i64, i64)>,
    /// Rain or snow starting within the next hours while it isn't falling now: the hour it starts
    /// (Unix seconds), and whether it is snow.
    pub precip: Option<(i64, bool)>,
}

/// Whether a WMO code is rain (false) or snow (true); None for neither.
fn precip_kind(code: u64) -> Option<bool> {
    match code { 51..=67 | 80..=82 | 95..=99 => Some(false), 71..=77 | 85 | 86 => Some(true), _ => None }
}

/// One day's forecast: its date, WMO code, and rounded high and low.
#[derive(Clone, Debug, PartialEq)]
pub struct Day {
    pub date: (i32, u32, u32),
    pub code: Option<u64>,
    pub high: i32,
    pub low: i32,
}

/// The current weather from Open-Meteo, which needs no API key.
pub fn weather(location: &str, unit: &str) -> Option<Weather> {
    weather_from("https://geocoding-api.open-meteo.com", "https://api.open-meteo.com", location, unit, crate::date::now_secs())
}

fn get_json(req: ureq::RequestBuilder<ureq::typestate::WithoutBody>) -> Option<Value> {
    let mut res = req.call().ok()?;
    if !res.status().is_success() { return None }
    Some(read_json(&mut res)).filter(|v| !v.is_null())
}

/// `location` is `lat,lon` or a place name; only the part before a comma is looked up
/// ("Paris, France" finds Paris), as Open-Meteo's search takes one name.
fn weather_from(geocoding: &str, forecast: &str, location: &str, unit: &str, now: i64) -> Option<Weather> {
    let coords = location.split_once(',').and_then(|(a, b)| Some((a.trim().parse::<f64>().ok()?, b.trim().parse::<f64>().ok()?)));
    let (lat, lon) = match coords {
        Some(c) => c,
        None => {
            let name = location.split(',').next().unwrap_or(location).trim();
            let found = get_json(agent().get(format!("{geocoding}/v1/search")).query("name", name).query("count", "1"))?;
            let place = &found["results"][0];
            (place["latitude"].as_f64()?, place["longitude"].as_f64()?)
        }
    };
    let data = get_json(agent().get(format!("{forecast}/v1/forecast"))
        .query("latitude", lat.to_string()).query("longitude", lon.to_string())
        .query("current", "temperature_2m,weather_code")
        .query("daily", "weather_code,temperature_2m_max,temperature_2m_min,sunrise,sunset")
        .query("hourly", "weather_code")
        // Starts at the current hour, which has already begun; four hours always cover the next 60 minutes.
        .query("forecast_hours", "4")
        .query("forecast_days", "6")
        .query("timezone", "auto")
        .query("temperature_unit", if unit == "f" { "fahrenheit" } else { "celsius" }))?;
    let (current, daily) = (&data["current"], &data["daily"]);
    let at = |field: &str, i: usize| daily[field][i].as_f64().map(|t| t.round() as i32);
    // Today is the first day; the forecast starts tomorrow.
    let days = daily["time"].as_array().into_iter().flatten().enumerate().skip(1).filter_map(|(i, date)| {
        let date = crate::date::parse_ymd(date.as_str()?)?;
        Some(Day { date, code: daily["weather_code"][i].as_u64(), high: at("temperature_2m_max", i)?, low: at("temperature_2m_min", i)? })
    }).take(5).collect();
    // `2026-11-20T07:12` on the location's clock, as Unix seconds.
    let unix = |t: &str| Some(crate::date::local_secs(t)? - data["utc_offset_seconds"].as_i64()?);
    let sun = |field: &str| daily[field][0].as_str().and_then(unix);
    let hourly = &data["hourly"];
    let falling = current["weather_code"].as_u64().and_then(precip_kind).is_some();
    // Hours run one real hour apart, so only the first is read on the clock: the offset is right for
    // it, and a daylight saving change within the next hours doesn't shift the rest.
    let first = hourly["time"][0].as_str().and_then(unix);
    let precip = hourly["weather_code"].as_array().into_iter().flatten().enumerate().filter(|_| !falling)
        .find_map(|(i, code)| Some((first? + i as i64 * 3600, precip_kind(code.as_u64()?)?)).filter(|&(at, _)| at > now));
    Some(Weather {
        temp: current["temperature_2m"].as_f64().map(|t| t.round() as i32),
        code: current["weather_code"].as_u64(),
        days,
        sun: sun("sunrise").zip(sun("sunset")),
        precip,
    })
}

/// A contribution graph: each active day (days since 1970) and its level, 1 to 4.
pub type Graph = Vec<(i64, u8)>;

/// A GitHub user's public contribution graph, read from the page github.com draws it from.
pub fn github_graph(user: &str) -> Result<Graph, String> { github_graph_from("https://github.com", user) }

fn github_graph_from(base: &str, user: &str) -> Result<Graph, String> {
    let user = user.trim().trim_start_matches('@');
    if user.is_empty() || !user.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') { return Err("Not a GitHub user name".into()) }
    let mut res = agent().get(format!("{base}/users/{user}/contributions")).call().map_err(|e| e.to_string())?;
    if !res.status().is_success() { return Err(format!("GitHub answered {}", res.status().as_u16())) }
    Ok(github_cells(&res.body_mut().read_to_string().map_err(|e| e.to_string())?))
}

/// The active days of the graph's `<td data-date="…" data-level="…">` cells.
fn github_cells(html: &str) -> Graph {
    let attr = |tag: &str, name: &str| tag.split_once(&format!("{name}=\"")).and_then(|(_, rest)| rest.split_once('"')).map(|(v, _)| v.to_string());
    let mut days: Graph = html.split("<td").skip(1).filter_map(|cell| {
        let tag = cell.split_once('>')?.0;
        let date = attr(tag, "data-date")?;
        let level: u8 = attr(tag, "data-level")?.parse().ok().filter(|l| *l > 0)?;
        let (y, m, d) = crate::date::parse_ymd(&date)?;
        Some((crate::date::days_from_civil(y, m, d), level.min(4)))
    }).collect();
    days.sort_unstable();
    days
}

/// A LeetCode user's accepted-submission calendar, from its public GraphQL API.
pub fn leetcode_graph(user: &str) -> Result<Graph, String> { leetcode_graph_from("https://leetcode.com", user) }

fn leetcode_graph_from(base: &str, user: &str) -> Result<Graph, String> {
    let body = json!({ "query": "query($u: String!) { matchedUser(username: $u) { userCalendar { submissionCalendar } } }", "variables": { "u": user.trim() } });
    let mut res = agent().post(format!("{base}/graphql")).header("content-type", "application/json").header("referer", format!("{base}/"))
        .send(body.to_string()).map_err(|e| e.to_string())?;
    if !res.status().is_success() { return Err(format!("LeetCode answered {}", res.status().as_u16())) }
    let data: Value = serde_json::from_str(&res.body_mut().read_to_string().map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let calendar = data["data"]["matchedUser"]["userCalendar"]["submissionCalendar"].as_str().ok_or("No such LeetCode user")?;
    let counts: std::collections::HashMap<String, u64> = serde_json::from_str(calendar).map_err(|e| e.to_string())?;
    let mut days: Graph = counts.into_iter().filter_map(|(t, n)| {
        let level = match n { 0 => return None, 1 => 1, 2..=3 => 2, 4..=6 => 3, _ => 4 };
        Some((t.parse::<i64>().ok()?.div_euclid(86_400), level))
    }).collect();
    days.sort_unstable();
    Ok(days)
}

/// An ICS calendar's text from its link; `webcal://` links are fetched over HTTPS.
pub fn calendar_feed(url: &str) -> Result<String, String> {
    let url = url.trim();
    let url = match url.strip_prefix("webcal://") { Some(rest) => format!("https://{rest}"), None => url.to_string() };
    if !(url.starts_with("https://") || url.starts_with("http://")) { return Err(format!("Not a calendar link: {url}")) }
    let mut res = agent().get(&url).call().map_err(|e| e.to_string())?;
    if !res.status().is_success() { return Err(format!("The calendar link answered {}", res.status().as_u16())) }
    res.body_mut().with_config().limit(16 * 1024 * 1024).read_to_string().map_err(|e| e.to_string())
}

/// A world clock's city: its name as Open-Meteo has it, its UTC offset now, and today's sunrise
/// and sunset there, in Unix seconds.
#[derive(Clone, Debug, PartialEq)]
pub struct City {
    pub name: String,
    pub offset: i64,
    pub sun: Option<(i64, i64)>,
}

pub fn city(name: &str) -> Option<City> { city_from("https://geocoding-api.open-meteo.com", "https://api.open-meteo.com", name) }

fn city_from(geocoding: &str, forecast: &str, name: &str) -> Option<City> {
    let found = get_json(agent().get(format!("{geocoding}/v1/search")).query("name", name.split(',').next().unwrap_or(name).trim()).query("count", "1"))?;
    let place = &found["results"][0];
    let (lat, lon) = (place["latitude"].as_f64()?, place["longitude"].as_f64()?);
    let data = get_json(agent().get(format!("{forecast}/v1/forecast"))
        .query("latitude", lat.to_string()).query("longitude", lon.to_string())
        .query("daily", "sunrise,sunset").query("forecast_days", "1").query("timezone", "auto"))?;
    let offset = data["utc_offset_seconds"].as_i64()?;
    let at = |field: &str| data["daily"][field][0].as_str().and_then(crate::date::local_secs).map(|t| t - offset);
    Some(City { name: place["name"].as_str().unwrap_or(name).to_string(), offset, sun: at("sunrise").zip(at("sunset")) })
}

/// Units of each currency per US dollar, by upper-case code, from Frankfurter (ECB reference
/// rates, no API key).
pub fn rates() -> Option<std::collections::HashMap<String, f64>> { rates_from("https://api.frankfurter.dev/v1") }

fn rates_from(base: &str) -> Option<std::collections::HashMap<String, f64>> {
    let data = get_json(agent().get(format!("{base}/latest")).query("base", "USD"))?;
    let mut rates: std::collections::HashMap<String, f64> = data["rates"].as_object()?.iter()
        .filter_map(|(code, rate)| Some((code.to_ascii_uppercase(), rate.as_f64().filter(|r| *r > 0.0)?))).collect();
    rates.insert("USD".into(), 1.0);
    Some(rates)
}

/// The newest release's tag on GitHub ("v0.7.0"); None when GitHub can't say.
pub fn latest_release() -> Option<String> { latest_release_from("https://api.github.com/repos/theoneand33/Ripple") }

fn latest_release_from(repo: &str) -> Option<String> {
    let data = get_json(agent().get(format!("{repo}/releases/latest")).header("Accept", "application/vnd.github+json"))?;
    data["tag_name"].as_str().map(str::to_string)
}

/// Whether the release `tag` ("v0.7.0") is a later version than `current` ("0.6.0").
pub fn newer(tag: &str, current: &str) -> bool {
    let version = |v: &str| v.trim().trim_start_matches('v').split(['-', '+']).next()?.split('.').map(|p| p.parse::<u64>().ok()).collect::<Option<Vec<_>>>();
    matches!((version(tag), version(current)), (Some(t), Some(c)) if t > c)
}

const SYSTEM_PROMPT: &str = "You are Ripple, a sleek and helpful desktop AI assistant. Your goal is to provide accurate, concise, and beautifully formatted answers that fit well in a compact desktop widget. \n- For general inquiries: Keep it to 2-4 sentences.\n- For complex or code-related questions: Provide detailed answers with Markdown code blocks, but stay as efficient as possible.\n- Use Markdown for bolding, lists, and headers to make information easy to scan.";

/// Streams a chat completion from the OpenAI-compatible API at `base`, calling `on_delta` with
/// each piece of the answer. Returns whether anything was received.
pub fn ask_ai(base: &str, model: &str, api_key: &str, question: &str, mut on_delta: impl FnMut(&str)) -> Result<bool, String> {
    // A stream runs as long as the answer takes; only connecting and the first byte are timed.
    let agent: ureq::Agent = config()
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(60)))
        .build()
        .into();
    // OpenRouter credits the app by these; other services ignore them.
    let mut request = agent
        .post(format!("{base}/chat/completions"))
        .header("content-type", "application/json")
        .header("http-referer", "https://github.com/TopMyster/Ripple")
        .header("x-title", "Ripple");
    if !api_key.is_empty() {
        request = request.header("authorization", format!("Bearer {api_key}"));
    }
    let body = json!({
        "model": model,
        "messages": [
            { "role": "system", "content": SYSTEM_PROMPT },
            { "role": "user", "content": question },
        ],
        "temperature": 1,
        "stream": true,
    });
    let res = request.send(body.to_string()).map_err(|e| e.to_string())?;
    let status = res.status();
    if !status.is_success() {
        return Err(format!("API Error: {} {}", status.as_u16(), status.canonical_reason().unwrap_or_default()));
    }
    let mut received = false;
    for line in BufReader::new(res.into_body().into_reader()).lines() {
        let line = line.map_err(|e| e.to_string())?;
        match sse_delta(&line) {
            Sse::Done => break,
            Sse::Delta(delta) => {
                received = true;
                on_delta(&delta);
            }
            Sse::Other => {}
        }
    }
    Ok(received)
}

#[derive(Debug, PartialEq, Eq)]
enum Sse {
    Delta(String),
    Done,
    Other,
}

/// One line of an OpenAI-style event stream.
fn sse_delta(line: &str) -> Sse {
    let Some(data) = line.strip_prefix("data: ") else { return Sse::Other };
    // Only the sentinel line ends the stream; an answer may mention `[DONE]` itself.
    if data.trim() == "[DONE]" { return Sse::Done }
    match serde_json::from_str::<Value>(data) {
        Ok(v) => match v["choices"][0]["delta"]["content"].as_str() {
            Some(delta) if !delta.is_empty() => Sse::Delta(delta.into()),
            _ => Sse::Other,
        },
        Err(_) => Sse::Other,
    }
}

/// An image's bytes from an `http(s):`, `file:` or `data:` URL.
pub fn fetch_image(url: &str) -> Option<Vec<u8>> {
    const LIMIT: u64 = 16 * 1024 * 1024;
    if let Some(data) = url.strip_prefix("data:") {
        use base64::Engine;
        let (meta, payload) = data.split_once(',')?;
        // Artwork URLs come from other apps: refuse oversized payloads before decoding them.
        if payload.len() as u64 / 4 * 3 > LIMIT { return None }
        return meta.ends_with(";base64").then(|| base64::engine::general_purpose::STANDARD.decode(payload.trim()).ok()).flatten();
    }
    if let Some(path) = url.strip_prefix("file://") {
        let path = crate::url::percent_decode(path);
        // file:///C:/a.png names C:/a.png on Windows.
        #[cfg(windows)]
        let path = match path.strip_prefix('/') { Some(p) if p.get(1..2) == Some(":") => p.to_string(), _ => path };
        // A FIFO blocks on open and /dev/zero never ends: regular files only, checked on the handle
        // that is read. O_NONBLOCK (values from libc) keeps the open itself from waiting on a FIFO.
        if !std::fs::metadata(&path).ok()?.is_file() { return None }
        let mut open = std::fs::OpenOptions::new();
        open.read(true);
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        std::os::unix::fs::OpenOptionsExt::custom_flags(&mut open, if cfg!(target_os = "linux") { 0o4000 } else { 0x4 });
        let file = open.open(&path).ok()?;
        if !file.metadata().ok()?.is_file() { return None }
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut std::io::Read::take(file, LIMIT + 1), &mut bytes).ok()?;
        return (bytes.len() as u64 <= LIMIT).then_some(bytes);
    }
    if !(url.starts_with("http://") || url.starts_with("https://")) { return None }
    let mut res = agent().get(url).call().ok()?;
    if !res.status().is_success() { return None }
    res.body_mut().with_config().limit(LIMIT).read_to_vec().ok()
}

/// One synced lyric line: when it starts, in milliseconds, and its words.
pub type Lyric = (u32, String);

/// A track's synced lyrics from LRCLIB, which needs no API key; None when it has none.
/// `length` (seconds) picks the result whose duration matches, as a song has many recordings.
pub fn lyrics(artist: &str, title: &str, length: u32) -> Option<Vec<Lyric>> {
    lyrics_from("https://lrclib.net", artist, title, length)
}

fn lyrics_from(base: &str, artist: &str, title: &str, length: u32) -> Option<Vec<Lyric>> {
    let found = get_json(agent().get(format!("{base}/api/search")).query("artist_name", artist).query("track_name", title))?;
    let synced: Vec<&Value> = found.as_array()?.iter().filter(|r| r["syncedLyrics"].as_str().is_some_and(|l| !l.trim().is_empty())).collect();
    let off = |r: &&Value| (r["duration"].as_f64().unwrap_or(f64::MAX) - length as f64).abs();
    // Search is fuzzy: a result more than 5 s off in length is another song or recording.
    let best = synced.iter().filter(|r| off(r) <= 5.0).min_by(|a, b| off(a).total_cmp(&off(b)))?;
    let lines = parse_lrc(best["syncedLyrics"].as_str()?);
    (!lines.is_empty()).then_some(lines)
}

/// LRC lines in time order. A line may carry several `[mm:ss.xx]` stamps; tags like `[ar:…]` are skipped.
pub fn parse_lrc(lrc: &str) -> Vec<Lyric> {
    let mut out = Vec::new();
    for line in lrc.lines() {
        let mut rest = line.trim();
        let mut stamps = Vec::new();
        while let Some((stamp, after)) = rest.strip_prefix('[').and_then(|r| r.split_once(']')) {
            let Some((m, s)) = stamp.split_once(':') else { break };
            let (Ok(m), Ok(s)) = (m.trim().parse::<u32>(), s.trim().parse::<f64>()) else { break };
            if !(0.0..60.0).contains(&s) { break }
            stamps.push(m.saturating_mul(60_000).saturating_add((s * 1000.0).round() as u32));
            rest = after;
        }
        out.extend(stamps.into_iter().map(|t| (t, rest.trim().to_string())));
    }
    out.sort_by_key(|l| l.0);
    out
}

/// The line sung at `ms`: the last one started, if any has.
pub fn lyric_at(lines: &[Lyric], ms: u32) -> Option<&str> {
    lines[..lines.partition_point(|l| l.0 <= ms)].last().map(|l| l.1.as_str())
}

/// SVG or SVGZ, sniffed the way Rune's decoder does: gzip magic, or `<` after an optional BOM.
pub fn is_vector(bytes: &[u8]) -> bool {
    let text = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    bytes.starts_with(&[0x1f, 0x8b]) || text.iter().find(|b| !b.is_ascii_whitespace()) == Some(&b'<')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_lines_yield_deltas_until_done() {
        assert_eq!(sse_delta(r#"data: {"choices":[{"delta":{"content":"Hi"}}]}"#), Sse::Delta("Hi".into()));
        assert_eq!(sse_delta(r#"data: {"choices":[{"delta":{}}]}"#), Sse::Other);
        assert_eq!(sse_delta("data: [DONE]"), Sse::Done);
        assert_eq!(sse_delta(r#"data: {"choices":[{"delta":{"content":"say [DONE]"}}]}"#), Sse::Delta("say [DONE]".into()));
        assert_eq!(sse_delta(": keep-alive"), Sse::Other);
        assert_eq!(sse_delta("data: {broken"), Sse::Other);
    }

    #[test]
    fn lrc_lines_come_in_time_order_and_skip_tags() {
        let lines = parse_lrc("[ar:Someone]\n[00:12.50]First\n[01:02.00][00:20.00] Chorus \n[00:30.00]\nplain text");
        assert_eq!(lines, [(12_500, "First".into()), (20_000, "Chorus".into()), (30_000, String::new()), (62_000, "Chorus".into())]);
        assert_eq!(lyric_at(&lines, 0), None);
        assert_eq!(lyric_at(&lines, 12_500), Some("First"));
        assert_eq!(lyric_at(&lines, 25_000), Some("Chorus"));
        assert_eq!(lyric_at(&lines, 99_000), Some("Chorus"));
    }

    #[test]
    fn lyrics_take_the_synced_result_closest_in_length() {
        use crate::testserver::{json, serve};
        let base = serve(|r| match r.path.as_str() {
            "/api/search?artist_name=Artist&track_name=Song" => json(200, json!([
                { "duration": 200.0, "syncedLyrics": null, "plainLyrics": "Plain" },
                { "duration": 300.0, "syncedLyrics": "[00:01.00]Long cut" },
                { "duration": 201.0, "syncedLyrics": "[00:01.00]Album cut" },
            ])),
            _ => json(200, json!([])),
        });
        assert_eq!(lyrics_from(&base, "Artist", "Song", 200), Some(vec![(1000, "Album cut".into())]));
        assert_eq!(lyrics_from(&base, "Artist", "Song", 250), None, "no result within 5 s");
        assert_eq!(lyrics_from(&base, "Artist", "Unknown", 200), None);
    }

    #[test]
    fn weather_looks_up_the_place_then_its_current_weather() {
        use crate::testserver::{json, serve};
        let base = serve(|r| match r.path.as_str() {
            "/v1/search?name=Paris&count=1" => json(200, json!({ "results": [{ "latitude": 48.85, "longitude": 2.35 }] })),
            "/v1/search?name=Nowhere&count=1" => json(200, json!({})),
            p if p.starts_with("/v1/forecast?latitude=48.85&longitude=2.35&") && p.contains("temperature_unit=celsius") =>
                json(200, json!({ "current": { "temperature_2m": 17.6, "weather_code": 61 }, "utc_offset_seconds": 3600,
                    "daily": { "time": ["2026-11-20", "2026-11-21", "2026-11-22"], "weather_code": [61, 3, null], "temperature_2m_max": [18.2, 12.6, 9.0], "temperature_2m_min": [10.0, 4.4, null],
                        "sunrise": ["2026-11-20T08:10", "2026-11-21T08:11", null], "sunset": ["2026-11-20T17:00", "2026-11-21T16:59", null] } })),
            p if p.starts_with("/v1/forecast?latitude=1.5&longitude=-2&") && p.contains("temperature_unit=fahrenheit") && p.contains("hourly=weather_code") =>
                json(200, json!({ "current": { "temperature_2m": 70.2, "weather_code": 2 }, "utc_offset_seconds": -7200,
                    "hourly": { "time": ["2026-11-20T10:00", "2026-11-20T11:00", "2026-11-20T12:00"], "weather_code": [2, 73, 61] } })),
            _ => json(404, json!({})),
        });
        let tomorrow = Day { date: (2026, 11, 21), code: Some(3), high: 13, low: 4 };
        let day = crate::date::days_from_civil(2026, 11, 20) * 86_400;
        let sun = Some((day + 7 * 3600 + 600, day + 16 * 3600));
        let now = day + 12 * 3600 + 1800;
        assert_eq!(weather_from(&base, &base, "Paris, France", "c", now), Some(Weather { temp: Some(18), code: Some(61), days: vec![tomorrow], sun, precip: None }), "a day missing a figure is left out");
        let snow_at = crate::date::days_from_civil(2026, 11, 20) * 86_400 + 13 * 3600;
        assert_eq!(weather_from(&base, &base, "1.5, -2", "f", now), Some(Weather { temp: Some(70), code: Some(2), days: vec![], sun: None, precip: Some((snow_at, true)) }), "the first rain or snow hour");
        assert_eq!(weather_from(&base, &base, "1.5, -2", "f", snow_at + 1800).and_then(|w| w.precip), Some((snow_at + 3600, false)), "an hour already started is skipped");
        assert_eq!(weather_from(&base, &base, "Nowhere", "c", now), None);
    }

    #[test]
    fn contribution_graphs_read_github_cells_and_leetcode_counts() {
        use crate::testserver::{Response, json as reply, serve};
        let day = |m, d| crate::date::days_from_civil(2026, m, d);
        let base = serve(move |r| match (r.method.as_str(), r.path.as_str()) {
            ("GET", "/users/octo/contributions") => Response { status: 200, headers: vec![], body: r#"<td tabindex="0" data-ix="0" data-date="2026-01-02" id="c1" data-level="0" class="day"></td><td data-date="2026-01-03" data-level="3" class="day"></td><td data-date="2026-01-01" data-level="1"></td>"#.into() },
            ("POST", "/graphql") if r.body.contains(r#""u":"ana""#) => reply(200, json!({ "data": { "matchedUser": { "userCalendar": { "submissionCalendar": format!(r#"{{"{}": 5, "{}": 1}}"#, day(1, 4) * 86_400 + 3600, day(1, 5) * 86_400) } } } })),
            ("POST", "/graphql") => reply(200, json!({ "data": { "matchedUser": null } })),
            _ => Response { status: 404, headers: vec![], body: String::new() },
        });
        assert_eq!(github_graph_from(&base, "@octo"), Ok(vec![(day(1, 1), 1), (day(1, 3), 3)]));
        assert!(github_graph_from(&base, "nobody").unwrap_err().contains("404"));
        assert!(github_graph_from(&base, "../x").is_err());
        assert_eq!(leetcode_graph_from(&base, "ana"), Ok(vec![(day(1, 4), 3), (day(1, 5), 1)]));
        assert!(leetcode_graph_from(&base, "ghost").is_err());
    }

    #[test]
    fn a_calendar_link_is_fetched_and_webcal_needs_https() {
        use crate::testserver::{Response, serve};
        let base = serve(|r| match r.path.as_str() {
            "/cal.ics" => Response { status: 200, headers: vec![], body: "BEGIN:VCALENDAR".into() },
            _ => Response { status: 404, headers: vec![], body: String::new() },
        });
        assert_eq!(calendar_feed(&format!("{base}/cal.ics")).as_deref(), Ok("BEGIN:VCALENDAR"));
        assert!(calendar_feed(&format!("{base}/gone.ics")).unwrap_err().contains("404"));
        assert!(calendar_feed("file:///etc/passwd").is_err());
    }

    #[test]
    fn a_city_comes_with_its_offset_and_sunrise_and_sunset() {
        use crate::testserver::{json, serve};
        let base = serve(|r| match r.path.as_str() {
            "/v1/search?name=Tokyo&count=1" => json(200, json!({ "results": [{ "name": "Tokyo", "latitude": 35.69, "longitude": 139.69 }] })),
            p if p.starts_with("/v1/forecast?latitude=35.69&longitude=139.69&") =>
                json(200, json!({ "utc_offset_seconds": 32400, "daily": { "sunrise": ["2026-11-20T06:20"], "sunset": ["2026-11-20T16:31"] } })),
            _ => json(200, json!({})),
        });
        let tokyo = city_from(&base, &base, "Tokyo, Japan").unwrap();
        let day = crate::date::days_from_civil(2026, 11, 20) * 86_400;
        assert_eq!(tokyo, City { name: "Tokyo".into(), offset: 32400, sun: Some((day + 6 * 3600 + 20 * 60 - 32400, day + 16 * 3600 + 31 * 60 - 32400)) });
        assert_eq!(city_from(&base, &base, "Atlantis"), None);
    }

    #[test]
    fn rates_come_per_dollar_with_the_dollar_itself() {
        use crate::testserver::{json, serve};
        let base = serve(|r| match r.path.as_str() {
            "/v1/latest?base=USD" => json(200, json!({ "amount": 1.0, "base": "USD", "rates": { "EUR": 0.9, "JPY": 150.5, "BAD": -1 } })),
            _ => json(404, json!({})),
        });
        let rates = rates_from(&format!("{base}/v1")).unwrap();
        assert_eq!((rates["EUR"], rates["USD"], rates.len()), (0.9, 1.0, 3));
        assert_eq!(rates_from(&format!("{base}/nope")), None);
    }

    #[test]
    fn the_latest_release_is_compared_with_this_version() {
        use crate::testserver::{json, serve};
        let base = serve(|r| match (r.path.as_str(), r.header("user-agent")) {
            ("/repos/r/releases/latest", Some("Ripple")) => json(200, json!({ "tag_name": "v0.7.0", "name": "Ripple v0.7.0" })),
            _ => json(404, json!({ "message": "Not Found" })),
        });
        assert_eq!(latest_release_from(&format!("{base}/repos/r")).as_deref(), Some("v0.7.0"));
        assert_eq!(latest_release_from(&format!("{base}/repos/none")), None);
        assert!(newer("v0.7.0", "0.6.0") && newer("v0.6.10", "0.6.9") && newer("1.0", "0.9.9"));
        assert!(!newer("v0.6.0", "0.6.0") && !newer("v0.5.9", "0.6.0") && !newer("v0.6.0-beta", "0.6.0") && !newer("nightly", "0.6.0"));
    }


    #[test]
    fn data_urls_decode_and_other_schemes_are_refused() {
        assert_eq!(fetch_image("data:image/png;base64,aGk="), Some(b"hi".to_vec()));
        assert_eq!(fetch_image("data:text/plain,hi"), None);
        assert_eq!(fetch_image(&format!("data:image/png;base64,{}", "A".repeat(23 << 20))), None, "over 16 MiB decoded");
        assert_eq!(fetch_image("ftp://example.com/a.png"), None);
        assert!(is_vector(b"\xef\xbb\xbf \n<svg/>") && is_vector(&[0x1f, 0x8b, 8]));
        assert!(!is_vector(b"\x89PNG\r\n") && !is_vector(b"GIF89a") && !is_vector(b""));
    }

    #[test]
    fn the_assistant_streams_an_answer_from_a_chat_completions_server() {
        use crate::testserver::{Response, serve};
        let base = serve(|r| {
            let body: serde_json::Value = serde_json::from_str(&r.body).unwrap_or_default();
            // A keyless (local) model gets no authorization header at all.
            let auth = if body["model"] == "local" { None } else { Some("Bearer gsk_test") };
            let ok = r.path == "/v1/chat/completions" && r.header("authorization") == auth
                && body["stream"] == true && body["messages"][1]["content"] == "Hi?" && (body["model"] == "m" || body["model"] == "local");
            if !ok { return Response { status: 401, headers: vec![], body: String::new() } }
            let events = ["Hel", "lo **", "you** [DONE]"].iter()
                .map(|d| format!("data: {}\n\n", json!({ "choices": [{ "delta": { "content": d } }] })))
                .collect::<String>() + "data: [DONE]\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"late\"}}]}\n\n";
            Response { status: 200, headers: vec![("content-type".into(), "text/event-stream".into())], body: events }
        });
        let mut answer = String::new();
        assert_eq!(ask_ai(&format!("{base}/v1"), "m", "gsk_test", "Hi?", |d| answer.push_str(d)), Ok(true));
        assert_eq!(answer, "Hello **you** [DONE]");
        let refused = ask_ai(&format!("{base}/v1"), "m", "wrong", "Hi?", |_| {});
        assert!(refused.unwrap_err().starts_with("API Error: 401"));
        assert_eq!(ask_ai(&format!("{base}/v1"), "local", "", "Hi?", |_| {}), Ok(true), "no authorization header for an empty key");
    }

}
