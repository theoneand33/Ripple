//! Coding agents: `ripple hook <agent>` is a command hook for Claude Code, Codex, the OpenCode
//! plugin, or any agent whose hooks send Claude-style JSON on stdin. It keeps each session as a
//! file in an `agents` folder beside prefs.json and, for a permission or a question, waits there
//! for the Island's answer. Files, not a socket, as with `ripple notify`, but on one computer:
//! liveness is read from file times, so a folder synced between computers would mislead it.
//!
//! `agents/sessions/<key>.json` is a session; `agents/requests/<key>.json` is a hook waiting, which
//! it touches each second, and the Island answers it in `<key>.answer`. The Island touches
//! `agents/alive` each second; a hook that finds it stale leaves the agent to ask in its terminal.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::paths::write_atomic;

/// Where hooks and the Island meet: a folder beside prefs.json.
pub fn folder(app_dir: &Path) -> std::path::PathBuf { app_dir.join("agents") }

/// How old `alive`, or a waiting hook's request, may get before it counts as gone.
const GONE: Duration = Duration::from_secs(5);
/// Sessions with no event for this long are dropped; so is one whose agent process is gone.
const STALE: Duration = Duration::from_secs(6 * 3600);
/// Hooks often run under a shell (`sh -c`): the agent is the nearest ancestor that isn't one.
const SHELLS: [&str; 12] = ["sh", "bash", "zsh", "dash", "fish", "ksh", "tcsh", "csh", "nu", "cmd", "powershell", "pwsh"];
/// The longest prompt or reply kept.
const DETAIL: usize = 400;
/// The longest command shown for approval: long enough not to hide what it does.
const COMMAND: usize = 4000;

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Session {
    /// The file's name: the agent and its session id.
    #[serde(skip)]
    pub key: String,
    /// The name `ripple hook` was given: claude, codex, opencode, …
    pub agent: String,
    pub cwd: String,
    pub working: bool,
    /// The prompt it works on, or its last reply once it stops.
    pub detail: String,
    /// The agent's process and when it started (Unix seconds), so a reused pid isn't taken
    /// for it; 0 where it couldn't be read, and then only `STALE` drops the session.
    pub pid: u32,
    pub started: u64,
}

/// A permission the agent asks for, or its questions when it has some.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Request {
    #[serde(skip)]
    pub key: String,
    /// The session's key.
    pub session: String,
    pub tool: String,
    pub detail: String,
    pub questions: Vec<Question>,
}

/// One of Claude Code's AskUserQuestion questions, as its tool input has it.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct Question {
    pub question: String,
    pub header: String,
    pub options: Vec<Choice>,
    pub multi_select: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Choice {
    pub label: String,
    pub description: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Answer {
    Allow,
    Deny,
    /// Each question's text to the labels picked, joined with ", ".
    Answers(BTreeMap<String, String>),
}

pub fn name(agent: &str) -> &str {
    match agent {
        "claude" => "Claude Code",
        "codex" => "Codex",
        "opencode" => "OpenCode",
        other => other,
    }
}

/// A file name from untrusted ids: no separators or dots, so it stays inside its folder.
fn key(parts: &[&str]) -> String {
    let joined = parts.join("-");
    joined.chars().take(120).map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect()
}

fn clip(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() > max { format!("{}…", text.chars().take(max - 1).collect::<String>()) } else { text.to_string() }
}

/// What a tool call does, in one line: its command, file, URL or description.
fn summary(input: &Value) -> String {
    let field = ["command", "file_path", "path", "url", "pattern", "description"].iter().find_map(|k| input.get(*k).and_then(Value::as_str));
    clip(&field.map_or_else(|| if input.is_null() { String::new() } else { input.to_string() }, str::to_string), COMMAND)
}

fn age(path: &Path) -> Option<Duration> {
    let modified = fs::metadata(path).and_then(|m| m.modified()).ok()?;
    Some(SystemTime::now().duration_since(modified).unwrap_or_default())
}

fn touch(path: &Path) {
    let _ = fs::File::options().write(true).open(path).and_then(|f| f.set_modified(SystemTime::now()));
}

fn alive(dir: &Path) -> bool { age(&dir.join("alive")).is_some_and(|a| a < GONE) }

fn refresh(sys: &mut sysinfo::System, pids: &[sysinfo::Pid]) {
    sys.refresh_processes_specifics(sysinfo::ProcessesToUpdate::Some(pids), true, sysinfo::ProcessRefreshKind::nothing());
}

/// The agent running this hook, as (pid, start time): the nearest ancestor that isn't a shell.
fn agent_process() -> Option<(u32, u64)> {
    let mut sys = sysinfo::System::new();
    let mut pid = sysinfo::get_current_pid().ok()?;
    refresh(&mut sys, &[pid]);
    for _ in 0..8 {
        // Past pid 1 is init or launchd: the agent is gone or was never found.
        let parent = sys.process(pid)?.parent().filter(|p| p.as_u32() > 1)?;
        refresh(&mut sys, &[parent]);
        let p = sys.process(parent)?;
        let name = p.name().to_string_lossy().to_lowercase();
        if !SHELLS.contains(&name.strip_suffix(".exe").unwrap_or(&name)) { return Some((parent.as_u32(), p.start_time())) }
        pid = parent;
    }
    None
}

/// Whether each session's agent still runs; one with no recorded process counts as running.
fn running(sessions: &[Session]) -> Vec<bool> {
    let mut pids: Vec<sysinfo::Pid> = sessions.iter().filter(|s| s.pid != 0).map(|s| sysinfo::Pid::from_u32(s.pid)).collect();
    // Once each: sysinfo takes a pid listed twice for a process that died between the two.
    pids.sort();
    pids.dedup();
    let mut sys = sysinfo::System::new();
    if !pids.is_empty() { refresh(&mut sys, &pids) }
    // A second's slack: start times are rounded.
    sessions.iter().map(|s| s.pid == 0 || sys.process(sysinfo::Pid::from_u32(s.pid)).is_some_and(|p| p.start_time().abs_diff(s.started) <= 1)).collect()
}

/// Records one hook event in the session's file; a PermissionRequest comes back to wait on.
fn record(dir: &Path, agent: &str, input: &Value) -> std::io::Result<Option<Request>> {
    let s = |k: &str| input.get(k).and_then(Value::as_str).unwrap_or_default();
    let session = key(&[agent, if s("session_id").is_empty() { "default" } else { s("session_id") }]);
    let path = dir.join("sessions").join(format!("{session}.json"));
    let mut info = fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str::<Session>(&t).ok()).unwrap_or_default();
    info.agent = agent.to_string();
    // Again once the saved process is gone: a resumed session runs in a new one.
    if (info.pid == 0 || !running(std::slice::from_ref(&info))[0]) && let Some((pid, started)) = agent_process() { (info.pid, info.started) = (pid, started) }
    if !s("cwd").is_empty() { info.cwd = s("cwd").to_string() }
    let event = s("hook_event_name");
    match event {
        "SessionEnd" => { let _ = fs::remove_file(&path); return Ok(None) }
        // OpenCode's plugin sends no prompt: keep the one shown.
        "UserPromptSubmit" => {
            info.working = true;
            if !s("prompt").is_empty() { info.detail = clip(s("prompt"), DETAIL) }
        }
        "Stop" => {
            info.working = false;
            if !s("last_assistant_message").is_empty() { info.detail = clip(s("last_assistant_message"), DETAIL) }
        }
        "PreToolUse" | "PostToolUse" | "PermissionRequest" => info.working = true,
        _ => {}
    }
    write_atomic(&path, &serde_json::to_string(&info)?)?;
    if event != "PermissionRequest" { return Ok(None) }
    let tool_input = input.get("tool_input").unwrap_or(&Value::Null);
    let questions: Vec<Question> = if s("tool_name") == "AskUserQuestion" { serde_json::from_value(tool_input["questions"].clone()).unwrap_or_default() } else { Vec::new() };
    // Allow alone can't answer a question: one we can't read is asked in the terminal.
    if s("tool_name") == "AskUserQuestion" && questions.is_empty() { return Ok(None) }
    // Not through `key`, whose cut could drop the part that keeps two requests apart.
    let id = format!("{session}-{}-{}", crate::date::now_ms(), std::process::id());
    Ok(Some(Request { key: id, session, tool: s("tool_name").to_string(), detail: summary(tool_input), questions }))
}

/// Puts the request up for the Island and waits for its answer. None when the Island stops, drops
/// it, or isn't running: the agent then asks in its terminal as usual.
fn ask(dir: &Path, request: &Request) -> Option<Answer> {
    let requests = dir.join("requests");
    let (file, reply) = (requests.join(format!("{}.json", request.key)), requests.join(format!("{}.answer", request.key)));
    write_atomic(&file, &serde_json::to_string(request).ok()?).ok()?;
    let mut beat = Instant::now();
    let answer = loop {
        std::thread::sleep(Duration::from_millis(200));
        if let Ok(text) = fs::read_to_string(&reply) { break serde_json::from_str(&text).ok() }
        if !file.exists() || !alive(dir) { break None }
        if beat.elapsed() >= Duration::from_secs(1) { touch(&file); beat = Instant::now() }
    };
    let _ = fs::remove_file(&file);
    let _ = fs::remove_file(&reply);
    answer
}

/// The hook's answer, in the PermissionRequest shape Claude Code and Codex both read.
pub fn output(input: &Value, answer: &Answer) -> Value {
    let decision = match answer {
        Answer::Allow => json!({ "behavior": "allow" }),
        Answer::Deny => json!({ "behavior": "deny", "message": "Denied from Ripple" }),
        // The tool's own input, with the answers added.
        Answer::Answers(answers) => {
            let mut updated = input.get("tool_input").filter(|v| v.is_object()).cloned().unwrap_or_else(|| json!({}));
            updated["answers"] = json!(answers);
            json!({ "behavior": "allow", "updatedInput": updated })
        }
    };
    json!({ "hookSpecificOutput": { "hookEventName": "PermissionRequest", "decision": decision } })
}

/// The command itself. Always exits 0 with no output unless the Island answered, so a hook that
/// fails never blocks the agent.
pub fn cli(args: &[String]) -> i32 {
    let agent = key(&[args.first().map_or("agent", String::as_str)]);
    let mut raw = String::new();
    let _ = std::io::stdin().read_to_string(&mut raw);
    let Ok(input) = serde_json::from_str::<Value>(&raw) else { eprintln!("Ripple: hook input is not JSON"); return 0 };
    let Some(dir) = crate::paths::app_dir().map(|d| folder(&d)) else { return 0 };
    let request = match record(&dir, &agent, &input) {
        Ok(r) => r,
        Err(e) => { eprintln!("Ripple: could not record the session: {e}"); return 0 }
    };
    if let Some(answer) = request.filter(|_| alive(&dir)).and_then(|r| ask(&dir, &r)) {
        println!("{}", output(&input, &answer));
    }
    0
}

/// Once a second from the Island: marks it running, and reads the live sessions (newest first)
/// and the requests waiting on it. Files left by gone sessions and hooks are deleted; with
/// `processes`, so are those of sessions whose agent process is gone.
pub fn scan(dir: &Path, processes: bool) -> (Vec<Session>, Vec<Request>) {
    let _ = fs::create_dir_all(dir).and_then(|()| fs::write(dir.join("alive"), ""));
    let read = |folder: &str, max: Duration| -> Vec<(String, String, Duration)> {
        let Ok(entries) = fs::read_dir(dir.join(folder)) else { return Vec::new() };
        let mut files = Vec::new();
        for p in entries.flatten().map(|e| e.path()) {
            let Some(a) = age(&p) else { continue };
            if a > max { let _ = fs::remove_file(&p); continue }
            if p.extension().is_none_or(|e| e != "json") { continue }
            let (Some(stem), Ok(text)) = (p.file_stem().and_then(|s| s.to_str()), fs::read_to_string(&p)) else { continue };
            files.push((stem.to_string(), text, a));
        }
        files.sort_by_key(|f| f.2);
        files
    };
    let sessions: Vec<Session> = read("sessions", STALE).into_iter()
        .filter_map(|(key, text, _)| serde_json::from_str(&text).ok().map(|s| Session { key, ..s })).collect();
    // An agent killed without SessionEnd: its process is gone, so is the session.
    let live = if processes { running(&sessions) } else { vec![true; sessions.len()] };
    let sessions = sessions.into_iter().zip(live).filter_map(|(s, live)| {
        if !live { let _ = fs::remove_file(dir.join("sessions").join(format!("{}.json", s.key))); }
        live.then_some(s)
    }).collect();
    let mut requests: Vec<Request> = read("requests", GONE).into_iter()
        .filter_map(|(key, text, _)| serde_json::from_str(&text).ok().map(|r| Request { key, ..r })).collect();
    requests.sort_by(|a, b| a.key.cmp(&b.key));
    (sessions, requests)
}

pub fn answer(dir: &Path, key: &str, answer: &Answer) -> std::io::Result<()> {
    write_atomic(&dir.join("requests").join(format!("{key}.answer")), &serde_json::to_string(answer)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ripple-agents-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn hook_events_keep_one_file_per_session() {
        let dir = temp("events");
        let event = |e: Value| record(&dir, "claude", &e).unwrap();
        assert!(event(json!({"session_id": "a/../b", "cwd": "/src/ripple", "hook_event_name": "UserPromptSubmit", "prompt": "Fix the build"})).is_none());
        let (sessions, _) = scan(&dir, true);
        assert_eq!(sessions, [Session { key: "claude-a____b".into(), agent: "claude".into(), cwd: "/src/ripple".into(), working: true, detail: "Fix the build".into(), ..sessions[0].clone() }]);
        assert_ne!(sessions[0].pid, 0, "the agent's process is recorded: here, what runs the tests");
        event(json!({"session_id": "a/../b", "hook_event_name": "Stop", "last_assistant_message": "Done."}));
        let (sessions, _) = scan(&dir, true);
        assert_eq!((sessions[0].working, sessions[0].detail.as_str(), sessions[0].cwd.as_str()), (false, "Done.", "/src/ripple"), "cwd kept from before");
        event(json!({"session_id": "a/../b", "hook_event_name": "SessionEnd"}));
        assert!(scan(&dir, true).0.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_session_goes_with_its_agents_process() {
        let dir = temp("gone");
        let me = sysinfo::get_current_pid().unwrap();
        let mut sys = sysinfo::System::new();
        refresh(&mut sys, &[me]);
        let started = sys.process(me).unwrap().start_time();
        let write = |key: &str, pid: u32, started: u64| write_atomic(&dir.join("sessions").join(format!("{key}.json")), &serde_json::to_string(&Session { agent: "claude".into(), pid, started, ..Session::default() }).unwrap()).unwrap();
        write("alive", me.as_u32(), started);
        write("reused", me.as_u32(), started + 100);
        write("gone", u32::MAX - 7, 1);
        write("unknown", 0, 0);
        let mut keys: Vec<String> = scan(&dir, true).0.into_iter().map(|s| s.key).collect();
        keys.sort();
        let gone_file = dir.join("sessions/gone.json").exists();
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(keys, ["alive", "unknown"], "a pid now used by another process doesn't keep it");
        assert!(!gone_file, "its file is deleted");
    }

    #[test]
    fn a_permission_waits_for_the_islands_answer() {
        let dir = temp("ask");
        let input = json!({"session_id": "s1", "hook_event_name": "PermissionRequest", "tool_name": "Bash", "tool_input": {"command": "rm -rf target"}});
        let request = record(&dir, "codex", &input).unwrap().unwrap();
        assert_eq!((request.tool.as_str(), request.detail.as_str(), request.session.as_str()), ("Bash", "rm -rf target", "codex-s1"));
        assert!(!alive(&dir), "no Island yet: the agent asks in its terminal");
        scan(&dir, true);
        assert!(alive(&dir));
        let hook = {
            let (dir, request) = (dir.clone(), request.clone());
            std::thread::spawn(move || ask(&dir, &request))
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        let seen = loop {
            let (_, requests) = scan(&dir, true);
            if !requests.is_empty() || Instant::now() > deadline { break requests }
            std::thread::sleep(Duration::from_millis(50));
        };
        assert_eq!(seen, [request.clone()]);
        answer(&dir, &request.key, &Answer::Deny).unwrap();
        assert_eq!(hook.join().unwrap(), Some(Answer::Deny));
        assert!(scan(&dir, true).1.is_empty(), "the hook cleans up after itself");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn answers_take_the_shape_agents_read() {
        let input = json!({"tool_name": "AskUserQuestion", "tool_input": {"questions": [{"question": "Which?", "header": "Pick", "options": [{"label": "A"}, {"label": "B"}], "multiSelect": true}]}});
        let decision = |a: &Answer| output(&input, a)["hookSpecificOutput"]["decision"].clone();
        assert_eq!(decision(&Answer::Allow), json!({"behavior": "allow"}));
        assert_eq!(decision(&Answer::Deny)["behavior"], "deny");
        let picked = Answer::Answers([("Which?".to_string(), "A, B".to_string())].into());
        assert_eq!(decision(&picked)["updatedInput"], json!({"questions": input["tool_input"]["questions"], "answers": {"Which?": "A, B"}}));
        assert_eq!(output(&json!({}), &picked)["hookSpecificOutput"]["decision"]["updatedInput"], json!({"answers": {"Which?": "A, B"}}));
        let questions: Vec<Question> = serde_json::from_value(input["tool_input"]["questions"].clone()).unwrap();
        assert!(questions[0].multi_select && questions[0].options[1].label == "B");
    }
}
