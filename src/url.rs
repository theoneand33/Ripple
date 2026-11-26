//! URL text: percent-encoding and the origin of a URL.

pub fn percent_decode(s: &str) -> String { String::from_utf8_lossy(&percent_decode_bytes(s)).into_owned() }

pub fn percent_decode_bytes(s: &str) -> Vec<u8> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = (bytes[i] == b'%').then(|| s.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())).flatten();
        out.push(hex.unwrap_or(bytes[i]));
        i += if hex.is_some() { 3 } else { 1 };
    }
    out
}

/// `encodeURIComponent`: everything but unreserved characters is percent-encoded.
pub fn percent_encode(s: &str) -> String {
    s.bytes().fold(String::new(), |mut out, b| {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) { out.push(b as char) } else { out.push_str(&format!("%{b:02X}")) }
        out
    })
}

/// `scheme://host[:port]`: the URL up to its path.
pub fn origin(url: &str) -> &str {
    let after_scheme = url.find("://").map_or(0, |i| i + 3);
    &url[..url[after_scheme..].find('/').map_or(url.len(), |i| after_scheme + i)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_encode_and_decode() {
        assert_eq!(percent_encode("rust & c++ ü"), "rust%20%26%20c%2B%2B%20%C3%BC");
        assert_eq!(percent_decode("/tmp/a%20b%2Fc.png"), "/tmp/a b/c.png");
        assert_eq!(percent_decode("100%"), "100%");
    }

    #[test]
    fn the_origin_ends_where_the_path_starts() {
        assert_eq!(origin("http://127.0.0.1:3773/api/projects?x=1"), "http://127.0.0.1:3773");
        assert_eq!(origin("https://cloud.example.com"), "https://cloud.example.com");
    }
}
