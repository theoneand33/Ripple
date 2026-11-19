//! A local HTTP server for tests: each connection is one request, answered by a handler.
//! `GET /ws...` upgrades to a WebSocket handled by `on_ws`.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;

pub struct Request {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

pub fn json(status: u16, body: serde_json::Value) -> Response {
    Response { status, headers: vec![("content-type".into(), "application/json".into())], body: body.to_string() }
}

type Handler = dyn Fn(&Request) -> Response + Send + Sync;
type WsHandler = dyn Fn(&str) -> String + Send + Sync;

/// Serves until the test process ends; returns `http://127.0.0.1:<port>`.
pub fn serve(handler: impl Fn(&Request) -> Response + Send + Sync + 'static) -> String { serve_ws(handler, |_| String::new()) }

/// `serve`, with `on_ws` answering each text message on `/ws`.
pub fn serve_ws(handler: impl Fn(&Request) -> Response + Send + Sync + 'static, on_ws: impl Fn(&str) -> String + Send + Sync + 'static) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (handler, on_ws): (Arc<Handler>, Arc<WsHandler>) = (Arc::new(handler), Arc::new(on_ws));
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let (handler, on_ws) = (handler.clone(), on_ws.clone());
            std::thread::spawn(move || connection(stream, &*handler, &*on_ws));
        }
    });
    format!("http://127.0.0.1:{port}")
}

fn connection(stream: TcpStream, handler: &Handler, on_ws: &WsHandler) {
    let mut peek = [0u8; 8];
    if stream.peek(&mut peek).is_ok() && peek.starts_with(b"GET /ws") {
        let Ok(mut ws) = tungstenite::accept(stream) else { return };
        while let Ok(msg) = ws.read() {
            if let tungstenite::Message::Text(text) = msg {
                let _ = ws.send(tungstenite::Message::text(on_ws(text.as_str())));
            }
        }
        return;
    }
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() { return }
    let mut parts = line.split_whitespace();
    let (method, path) = (parts.next().unwrap_or_default().to_string(), parts.next().unwrap_or_default().to_string());
    let mut headers = Vec::new();
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h).is_err() || h.trim().is_empty() { break }
        if let Some((k, v)) = h.trim_end().split_once(':') { headers.push((k.trim().to_string(), v.trim().to_string())) }
    }
    let len = headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("content-length")).and_then(|(_, v)| v.parse().ok()).unwrap_or(0);
    let mut body = vec![0; len];
    let _ = reader.read_exact(&mut body);
    let request = Request { method, path, headers, body: String::from_utf8_lossy(&body).into_owned() };
    let response = handler(&request);
    let mut out = format!("HTTP/1.1 {} X\r\ncontent-length: {}\r\nconnection: close\r\n", response.status, response.body.len());
    for (k, v) in &response.headers { out.push_str(&format!("{k}: {v}\r\n")) }
    out.push_str("\r\n");
    out.push_str(&response.body);
    let mut stream = reader.into_inner();
    let _ = stream.write_all(out.as_bytes());
}
