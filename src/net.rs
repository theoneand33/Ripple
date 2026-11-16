//! Ripple's own HTTP calls: weather, the AI assistant's stream and images by URL.
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
    pub status: String,
}

pub fn weather(location: &str, unit: &str) -> Option<Weather> {
    let mut res = agent()
        .get("https://api.weatherapi.com/v1/current.json")
        .query("key", "0b18c67c443543e0a6045401250911")
        .query("q", location)
        .query("aqi", "no")
        .call()
        .ok()?;
    let data: Value = serde_json::from_str(&res.body_mut().read_to_string().ok()?).ok()?;
    let key = if unit == "f" { "temp_f" } else { "temp_c" };
    Some(Weather {
        temp: data["current"][key].as_f64().map(|t| t.round() as i32),
        status: data["current"]["condition"]["text"].as_str().unwrap_or_default().into(),
    })
}

const SYSTEM_PROMPT: &str = "You are Ripple, a sleek and helpful desktop AI assistant. Your goal is to provide accurate, concise, and beautifully formatted answers that fit well in a compact desktop widget. \n- For general inquiries: Keep it to 2-4 sentences.\n- For complex or code-related questions: Provide detailed answers with Markdown code blocks, but stay as efficient as possible.\n- Use Markdown for bolding, lists, and headers to make information easy to scan.";

/// Streams a chat completion, calling `on_delta` with each piece of the answer. Returns
/// whether anything was received.
pub fn ask_ai(provider: &str, model: &str, api_key: &str, question: &str, mut on_delta: impl FnMut(&str)) -> Result<bool, String> {
    let base = if provider == "groq" { "https://api.groq.com/openai/v1" } else { "https://openrouter.ai/api/v1" };
    // A stream runs as long as the answer takes; only connecting and the first byte are timed.
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(60)))
        .http_status_as_error(false)
        .build()
        .into();
    let mut request = agent
        .post(format!("{base}/chat/completions"))
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {api_key}"));
    if provider == "openrouter" {
        request = request.header("http-referer", "https://github.com/TopMyster/Ripple").header("x-title", "Ripple");
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

/// An image's bytes from an `http(s):`, `file:` or `data:` URL, at most 16 MiB.
pub fn fetch_image(url: &str) -> Option<Vec<u8>> {
    const LIMIT: u64 = 16 * 1024 * 1024;
    if let Some(data) = url.strip_prefix("data:") {
        use base64::Engine;
        let (meta, payload) = data.split_once(',')?;
        return meta.ends_with(";base64").then(|| base64::engine::general_purpose::STANDARD.decode(payload.trim()).ok()).flatten();
    }
    if let Some(path) = url.strip_prefix("file://") {
        let path = percent_decode(path);
        // file:///C:/a.png names C:/a.png on Windows.
        #[cfg(windows)]
        let path = match path.strip_prefix('/') { Some(p) if p.get(1..2) == Some(":") => p.to_string(), _ => path };
        return (std::fs::metadata(&path).ok()?.len() <= LIMIT).then(|| std::fs::read(&path).ok()).flatten();
    }
    if !(url.starts_with("http://") || url.starts_with("https://")) { return None }
    let mut res = agent().get(url).call().ok()?;
    if !res.status().is_success() { return None }
    res.body_mut().with_config().limit(LIMIT).read_to_vec().ok()
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
    fn urls_encode_and_decode() {
        assert_eq!(percent_encode("rust & c++ ü"), "rust%20%26%20c%2B%2B%20%C3%BC");
        assert_eq!(percent_decode("/tmp/a%20b%2Fc.png"), "/tmp/a b/c.png");
        assert_eq!(percent_decode("100%"), "100%");
    }

    #[test]
    fn data_urls_decode_and_other_schemes_are_refused() {
        assert_eq!(fetch_image("data:image/png;base64,aGk="), Some(b"hi".to_vec()));
        assert_eq!(fetch_image("data:text/plain,hi"), None);
        assert_eq!(fetch_image("ftp://example.com/a.png"), None);
    }
}
