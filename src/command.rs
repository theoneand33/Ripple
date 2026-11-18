//! Splitting a launch string into argv without a shell, so the text can't chain commands.

/// Tokenizes an argument string, keeping mid-token quotes together.
/// `--flag="hello world"` -> `["--flag=hello world"]`
pub fn tokenize_args(s: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut chars = s.chars().peekable();
    loop {
        while chars.next_if(|c| c.is_whitespace()).is_some() {}
        if chars.peek().is_none() { break }
        let mut token = String::new();
        while let Some(c) = chars.next_if(|c| !c.is_whitespace()) {
            if c == '"' {
                for q in chars.by_ref() {
                    if q == '"' { break }
                    token.push(q);
                }
            } else {
                token.push(c);
            }
        }
        if !token.is_empty() { args.push(token) }
    }
    args
}

/// Parses a Windows command string into (exe, args): `%ENV_VAR%` expands before splitting, and
/// forward slashes in the exe path become backslashes. Arguments keep theirs (URLs, `/switches`).
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
pub fn parse_command(input: &str) -> (String, Vec<String>) {
    let (exe, args) = split_command(&expand_env(input));
    (exe.replace('/', "\\"), args)
}

fn split_command(prepared: &str) -> (String, Vec<String>) {
    // Quoted exe path: "C:\path with spaces\app.exe" [args...]
    if let Some(rest) = prepared.strip_prefix('"')
        && let Some(end) = rest.find('"')
        && end > 0
    {
        return (rest[..end].to_string(), tokenize_args(&rest[end + 1..]));
    }

    // Unquoted path: the exe ends at a known extension, so spaces inside the path
    // (C:\Program Files\...) don't split it early.
    let lower = prepared.to_ascii_lowercase();
    let boundary = [".exe", ".cmd", ".bat", ".com", ".ps1"]
        .iter()
        .flat_map(|ext| lower.match_indices(ext).map(|(i, _)| i + ext.len()).collect::<Vec<_>>())
        .filter(|&end| end > 4 && prepared[end..].chars().next().is_none_or(char::is_whitespace))
        .min();
    if let Some(end) = boundary {
        return (prepared[..end].to_string(), tokenize_args(&prepared[end..]));
    }

    // No recognised extension (cmd, wt): split on the first whitespace.
    match prepared.find(char::is_whitespace) {
        None => (prepared.to_string(), Vec::new()),
        Some(i) => (prepared[..i].to_string(), tokenize_args(&prepared[i..])),
    }
}

fn expand_env(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(start) = rest.find('%') {
        let Some(len) = rest[start + 1..].find('%') else { break };
        let name = &rest[start + 1..start + 1 + len];
        out.push_str(&rest[..start]);
        match std::env::var(name) {
            Ok(v) if !name.is_empty() => out.push_str(&v),
            _ => out.push_str(&rest[start..start + len + 2]),
        }
        rest = &rest[start + len + 2..];
    }
    out + rest
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> { v.iter().map(|x| x.to_string()).collect() }

    #[test]
    fn tokenize_args_keeps_mid_token_quotes_together() {
        assert_eq!(tokenize_args(r#"--flag="hello world" "quoted arg" --bare"#), s(&["--flag=hello world", "quoted arg", "--bare"]));
        assert!(tokenize_args("   ").is_empty());
    }

    #[test]
    fn parse_command_splits_exe_and_args() {
        assert_eq!(parse_command(r#""C:\Program Files\x.exe" --a="b c""#), (r"C:\Program Files\x.exe".into(), s(&["--a=b c"])));
        assert_eq!(parse_command("C:/Program Files/App/app.exe -v"), (r"C:\Program Files\App\app.exe".into(), s(&["-v"])));
        assert_eq!(parse_command("wt new-tab"), ("wt".into(), s(&["new-tab"])));
        assert_eq!(parse_command("cmd"), ("cmd".into(), vec![]));
        assert_eq!(parse_command("C:/a/app.exe --u=https://x /select,C:/b"), (r"C:\a\app.exe".into(), s(&["--u=https://x", "/select,C:/b"])));
        assert_eq!(parse_command(r#""C:/Program Files/a.exe" https://x"#), (r"C:\Program Files\a.exe".into(), s(&["https://x"])));
    }

    #[test]
    fn unknown_env_vars_stay_as_written() {
        assert_eq!(expand_env("%RIPPLE_NO_SUCH_VAR%\\x"), "%RIPPLE_NO_SUCH_VAR%\\x");
    }
}
