//! Ripple's own HTTP calls: weather, exchange rates, the AI assistant's stream and images by URL.
//! Blocking; callers run them off the UI thread.

use std::io::{BufRead, BufReader};
use std::time::Duration;

use serde_json::{Value, json};

/// A client for short requests: 15 s for the whole call, and error statuses come back as
/// responses so their bodies can be read.
pub fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(15)))
        .http_status_as_error(false)
        .build()
        .into()
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Weather {
    /// Rounded degrees in the chosen unit; None until the first answer.
    pub temp: Option<i32>,
    /// The WMO weather code, for `icons::weather`.
    pub code: Option<u64>,
    /// The next days' forecast, from tomorrow.
    pub days: Vec<Day>,
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
    weather_from("https://geocoding-api.open-meteo.com", "https://api.open-meteo.com", location, unit)
}

fn get_json(req: ureq::RequestBuilder<ureq::typestate::WithoutBody>) -> Option<Value> {
    let mut res = req.call().ok()?;
    if !res.status().is_success() { return None }
    serde_json::from_str(&res.body_mut().read_to_string().ok()?).ok()
}

/// `location` is `lat,lon` or a place name; only the part before a comma is looked up
/// ("Paris, France" finds Paris), as Open-Meteo's search takes one name.
fn weather_from(geocoding: &str, forecast: &str, location: &str, unit: &str) -> Option<Weather> {
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
        .query("daily", "weather_code,temperature_2m_max,temperature_2m_min")
        .query("forecast_days", "6")
        .query("timezone", "auto")
        .query("temperature_unit", if unit == "f" { "fahrenheit" } else { "celsius" }))?;
    let (current, daily) = (&data["current"], &data["daily"]);
    let at = |field: &str, i: usize| daily[field][i].as_f64().map(|t| t.round() as i32);
    // Today is the first day; the forecast starts tomorrow.
    let days = daily["time"].as_array().into_iter().flatten().enumerate().skip(1).filter_map(|(i, date)| {
        let mut ymd = date.as_str()?.splitn(3, '-').map(str::parse::<u32>);
        let date = (ymd.next()?.ok()? as i32, ymd.next()?.ok()?, ymd.next()?.ok()?);
        Some(Day { date, code: daily["weather_code"][i].as_u64(), high: at("temperature_2m_max", i)?, low: at("temperature_2m_min", i)? })
    }).take(5).collect();
    Some(Weather {
        temp: current["temperature_2m"].as_f64().map(|t| t.round() as i32),
        code: current["weather_code"].as_u64(),
        days,
    })
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
    let at = |field: &str| data["daily"][field][0].as_str().and_then(|t| local_secs(t)).map(|t| t - offset);
    Some(City { name: place["name"].as_str().unwrap_or(name).to_string(), offset, sun: at("sunrise").zip(at("sunset")) })
}

/// `2026-11-20T07:12` as seconds since 1970 on that clock.
pub fn local_secs(t: &str) -> Option<i64> {
    let (date, time) = t.split_once('T')?;
    let mut d = date.splitn(3, '-').map(str::parse::<u32>);
    let (y, m, day) = (d.next()?.ok()?, d.next()?.ok()?, d.next()?.ok()?);
    let (h, min) = time.split_once(':')?;
    Some(crate::date::days_from_civil(y as i32, m, day) * 86_400 + i64::from(h.parse::<u32>().ok()? * 3600 + min.get(..2)?.parse::<u32>().ok()? * 60))
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

const SYSTEM_PROMPT: &str = "You are Ripple, a sleek and helpful desktop AI assistant. Your goal is to provide accurate, concise, and beautifully formatted answers that fit well in a compact desktop widget. \n- For general inquiries: Keep it to 2-4 sentences.\n- For complex or code-related questions: Provide detailed answers with Markdown code blocks, but stay as efficient as possible.\n- Use Markdown for bolding, lists, and headers to make information easy to scan.";

/// Streams a chat completion from the OpenAI-compatible API at `base`, calling `on_delta` with
/// each piece of the answer. Returns whether anything was received.
pub fn ask_ai(base: &str, model: &str, api_key: &str, question: &str, mut on_delta: impl FnMut(&str)) -> Result<bool, String> {
    // A stream runs as long as the answer takes; only connecting and the first byte are timed.
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(60)))
        .http_status_as_error(false)
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
        let path = percent_decode(path);
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

/// SVG or SVGZ, sniffed the way Rune's decoder does: gzip magic, or `<` after an optional BOM.
pub fn is_vector(bytes: &[u8]) -> bool {
    let text = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    bytes.starts_with(&[0x1f, 0x8b]) || text.iter().find(|b| !b.is_ascii_whitespace()) == Some(&b'<')
}

pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = (bytes[i] == b'%').then(|| s.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())).flatten();
        out.push(hex.unwrap_or(bytes[i]));
        i += if hex.is_some() { 3 } else { 1 };
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `encodeURIComponent`: everything but unreserved characters is percent-encoded.
pub fn percent_encode(s: &str) -> String {
    s.bytes().fold(String::new(), |mut out, b| {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) { out.push(b as char) } else { out.push_str(&format!("%{b:02X}")) }
        out
    })
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
    fn weather_looks_up_the_place_then_its_current_weather() {
        use crate::testserver::{json, serve};
        let base = serve(|r| match r.path.as_str() {
            "/v1/search?name=Paris&count=1" => json(200, json!({ "results": [{ "latitude": 48.85, "longitude": 2.35 }] })),
            "/v1/search?name=Nowhere&count=1" => json(200, json!({})),
            p if p.starts_with("/v1/forecast?latitude=48.85&longitude=2.35&") && p.contains("temperature_unit=celsius") =>
                json(200, json!({ "current": { "temperature_2m": 17.6, "weather_code": 61 },
                    "daily": { "time": ["2026-11-20", "2026-11-21", "2026-11-22"], "weather_code": [61, 3, null], "temperature_2m_max": [18.2, 12.6, 9.0], "temperature_2m_min": [10.0, 4.4, null] } })),
            p if p.starts_with("/v1/forecast?latitude=1.5&longitude=-2&") && p.contains("temperature_unit=fahrenheit") =>
                json(200, json!({ "current": { "temperature_2m": 70.2, "weather_code": 2 } })),
            _ => json(404, json!({})),
        });
        let tomorrow = Day { date: (2026, 11, 21), code: Some(3), high: 13, low: 4 };
        assert_eq!(weather_from(&base, &base, "Paris, France", "c"), Some(Weather { temp: Some(18), code: Some(61), days: vec![tomorrow] }), "a day missing a figure is left out");
        assert_eq!(weather_from(&base, &base, "1.5, -2", "f"), Some(Weather { temp: Some(70), code: Some(2), days: vec![] }));
        assert_eq!(weather_from(&base, &base, "Nowhere", "c"), None);
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
    fn urls_encode_and_decode() {
        assert_eq!(percent_encode("rust & c++ ü"), "rust%20%26%20c%2B%2B%20%C3%BC");
        assert_eq!(percent_decode("/tmp/a%20b%2Fc.png"), "/tmp/a b/c.png");
        assert_eq!(percent_decode("100%"), "100%");
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
