use crate::util::{clip, now_iso, str_field, u64_field};
use regex::Regex;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Tool outputs are clipped before storage; a Read of a large file is otherwise most of the row.
const TOOL_OUTPUT_MAX_CHARS: usize = 8_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tokens {
    /// Everything the model took in, cache hits included: each turn re-reads the conversation.
    pub read: u64,
    pub written: u64,
}

impl std::ops::Add for Tokens {
    type Output = Tokens;
    fn add(self, other: Tokens) -> Tokens {
        Tokens { read: self.read + other.read, written: self.written + other.written }
    }
}

pub enum Activity {
    Thinking(u64),
    Tool { name: String, input: Value },
    /// The structured answer as it streams: every `name` written so far.
    Answering(Vec<String>),
    Tokens(Tokens),
}

pub struct Request<'a> {
    pub prompt: &'a str,
    pub cwd: &'a Path,
    pub schema: &'a Value,
    /// The tool definitions the model sees at all; `StructuredOutput` is added by `--json-schema`.
    pub tools: &'a [&'a str],
    pub allowed_tools: &'a [&'a str],
    pub add_dirs: &'a [PathBuf],
    pub model: Option<&'a str>,
    pub timeout: Option<Duration>,
    /// Stream token-by-token events, for live progress. Costs nothing but stdout volume.
    pub live: bool,
}

pub struct Outcome {
    pub output: Value,
    pub tokens: Tokens,
    pub turns: u32,
    pub tool_calls: u32,
    pub model: Option<String>,
    /// The session as stored on a decision: prompt, system line, thinking, tools, text.
    pub session: Vec<Value>,
}

fn running() -> &'static Mutex<Vec<u32>> {
    static RUNNING: OnceLock<Mutex<Vec<u32>>> = OnceLock::new();
    RUNNING.get_or_init(|| Mutex::new(Vec::new()))
}

/// Ctrl+C stops every agent instead of orphaning it.
pub fn stop_all() {
    let pids = running().lock().map(|pids| pids.clone()).unwrap_or_default();
    for pid in pids {
        let _ = Command::new("kill").args(["-TERM", &pid.to_string()]).status();
    }
}

pub fn is_installed() -> bool {
    Command::new("claude").arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|status| status.success())
}

fn name_field() -> &'static Regex {
    static NAME: OnceLock<Regex> = OnceLock::new();
    NAME.get_or_init(|| Regex::new(r#""name"\s*:\s*"((?:[^"\\]|\\.)*)""#).expect("the pattern compiles"))
}

fn decode_json_string(raw: &str) -> String {
    serde_json::from_str::<String>(&format!("\"{raw}\"")).unwrap_or_else(|_| raw.to_string())
}

/// One `claude -p` run, shaped to be one of many in parallel:
/// - the prompt goes through stdin, so its size never hits the argument limit;
/// - no MCP servers and only user settings, so a run doesn't pay for tools it
///   can't use or fire a repo's hooks on every call;
/// - `default` permission mode with an explicit allowlist, so it stays
///   read-only even when the person's settings default to auto or bypass;
/// - no session file, so these runs never show up as the person's sessions.
pub fn run(request: &Request, on_activity: &mut dyn FnMut(Activity)) -> Result<Outcome, String> {
    let mut args: Vec<String> = vec![
        "-p".into(),
        "--output-format".into(),
        "stream-json".into(),
        "--verbose".into(),
        "--json-schema".into(),
        request.schema.to_string(),
        "--tools".into(),
        request.tools.join(","),
        "--permission-mode".into(),
        "default".into(),
        "--strict-mcp-config".into(),
        "--mcp-config".into(),
        r#"{"mcpServers":{}}"#.into(),
        "--setting-sources".into(),
        "user".into(),
        "--no-session-persistence".into(),
    ];
    if request.live {
        args.push("--include-partial-messages".into());
    }
    if !request.allowed_tools.is_empty() {
        args.push("--allowedTools".into());
        args.extend(request.allowed_tools.iter().map(|tool| tool.to_string()));
    }
    if !request.add_dirs.is_empty() {
        args.push("--add-dir".into());
        args.extend(request.add_dirs.iter().map(|dir| dir.display().to_string()));
    }
    if let Some(model) = request.model {
        args.push("--model".into());
        args.push(model.to_string());
    }

    let mut child = Command::new("claude")
        .args(&args)
        .current_dir(request.cwd)
        .env("REVIEWERS_BYPASS", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("cannot start claude: {error}. Is Claude Code installed?"))?;
    let pid = child.id();
    if let Ok(mut pids) = running().lock() {
        pids.push(pid);
    }
    if let Some(mut stdin) = child.stdin.take() {
        let prompt = request.prompt.to_string();
        std::thread::spawn(move || {
            let _ = stdin.write_all(prompt.as_bytes());
        });
    }
    let stderr_handle = child.stderr.take().map(|stderr| {
        std::thread::spawn(move || {
            let mut text = String::new();
            let _ = std::io::Read::read_to_string(&mut BufReader::new(stderr), &mut text);
            text
        })
    });
    let (sender, receiver) = mpsc::channel::<String>();
    if let Some(stdout) = child.stdout.take() {
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
    }

    let started = Instant::now();
    let mut lines: Vec<(String, Value)> = Vec::new();
    let mut result: Option<Value> = None;
    let mut model: Option<String> = None;
    let mut answer: Option<String> = None;
    let mut named = 0usize;
    let mut finished = Tokens::default();
    let mut current = Tokens::default();
    let mut timed_out = false;
    loop {
        match receiver.recv_timeout(Duration::from_millis(200)) {
            Ok(line) => {
                let Ok(event) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                let kind = str_field(&event, "type").unwrap_or_default();
                match kind {
                    "system" if str_field(&event, "subtype") == Some("init") => {
                        model = str_field(&event, "model").map(str::to_string);
                    }
                    "system" if str_field(&event, "subtype") == Some("thinking_tokens") => {
                        on_activity(Activity::Thinking(u64_field(&event, "estimated_tokens")));
                    }
                    "stream_event" => {
                        let stream = &event["event"];
                        match str_field(stream, "type").unwrap_or_default() {
                            "message_start" => {
                                let usage = &stream["message"]["usage"];
                                finished = finished + current;
                                current = Tokens {
                                    read: u64_field(usage, "input_tokens")
                                        + u64_field(usage, "cache_creation_input_tokens")
                                        + u64_field(usage, "cache_read_input_tokens"),
                                    written: u64_field(usage, "output_tokens"),
                                };
                                on_activity(Activity::Tokens(finished + current));
                            }
                            "message_delta" => {
                                current.written = u64_field(&stream["usage"], "output_tokens");
                                on_activity(Activity::Tokens(finished + current));
                            }
                            "content_block_start" => {
                                let block = &stream["content_block"];
                                answer = (str_field(block, "type") == Some("tool_use") && str_field(block, "name") == Some("StructuredOutput"))
                                    .then(String::new);
                            }
                            "content_block_delta" if str_field(&stream["delta"], "type") == Some("input_json_delta") => {
                                if let Some(text) = answer.as_mut() {
                                    text.push_str(str_field(&stream["delta"], "partial_json").unwrap_or_default());
                                    let names: Vec<String> = name_field().captures_iter(text).map(|capture| decode_json_string(&capture[1])).collect();
                                    if names.len() != named {
                                        named = names.len();
                                        on_activity(Activity::Answering(names));
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    "assistant" => {
                        if let Some(parts) = event["message"]["content"].as_array() {
                            for part in parts {
                                let name = str_field(part, "name").unwrap_or_default();
                                if str_field(part, "type") == Some("tool_use") && name != "StructuredOutput" {
                                    on_activity(Activity::Tool { name: name.to_string(), input: part["input"].clone() });
                                }
                            }
                        }
                    }
                    "result" => result = Some(event.clone()),
                    _ => {}
                }
                if kind != "stream_event" {
                    lines.push((now_iso(), event));
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if request.timeout.is_some_and(|limit| started.elapsed() > limit) {
                    let _ = child.kill();
                    timed_out = true;
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let status = child.wait();
    if let Ok(mut pids) = running().lock() {
        pids.retain(|&running_pid| running_pid != pid);
    }
    let stderr = stderr_handle.and_then(|handle| handle.join().ok()).unwrap_or_default();
    if timed_out {
        return Err(format!("claude timed out after {}s", request.timeout.map(|limit| limit.as_secs()).unwrap_or_default()));
    }

    let Some(result) = result else {
        let detail = stderr.trim().lines().last().unwrap_or("no result event").to_string();
        let code = status.ok().and_then(|status| status.code()).unwrap_or(-1);
        return Err(format!("claude exited {code}: {detail}"));
    };
    let is_error = result["is_error"].as_bool() == Some(true) || str_field(&result, "subtype").is_some_and(|subtype| subtype != "success");
    let output = result.get("structured_output").cloned().unwrap_or(Value::Null);
    if is_error || output.is_null() {
        let detail = str_field(&result, "result").map(str::to_string).unwrap_or_else(|| stderr.trim().to_string());
        let first_line = detail.lines().next().unwrap_or("no structured answer").chars().take(300).collect::<String>();
        return Err(format!("claude: {first_line}"));
    }
    let usage = &result["usage"];
    let reported = Tokens {
        read: u64_field(usage, "input_tokens") + u64_field(usage, "cache_creation_input_tokens") + u64_field(usage, "cache_read_input_tokens"),
        written: u64_field(usage, "output_tokens"),
    };
    let streamed = finished + current;
    let session = session_from(&lines, request.prompt, request.cwd);
    let tool_calls = session.iter().filter(|entry| str_field(entry, "kind") == Some("tool") && str_field(entry, "name") != Some("StructuredOutput")).count();
    Ok(Outcome {
        output,
        // A multi-turn session's result reports the last turn; the stream saw every turn.
        tokens: if streamed.read > reported.read { streamed } else { reported },
        turns: u64_field(&result, "num_turns") as u32,
        tool_calls: tool_calls as u32,
        model,
        session,
    })
}

fn relative(root: &Path, path: &str) -> String {
    let prefix = format!("{}/", root.display());
    path.strip_prefix(&prefix).map(str::to_string).unwrap_or_else(|| path.to_string())
}

/// One line a person can scan in a list of calls: the tool and its main argument.
pub fn tool_detail(name: &str, input: &Value, root: &Path) -> String {
    let field = |key: &str| str_field(input, key).map(|value| relative(root, value)).unwrap_or_default();
    let detail = match name {
        "Read" => format!("Read {}", field("file_path")),
        "Grep" => format!("Grep /{}/ {}", field("pattern"), if field("path").is_empty() { field("glob") } else { field("path") }),
        "Glob" => format!("Glob {} {}", field("pattern"), field("path")),
        "StructuredOutput" => "Returned the verdict".to_string(),
        other => other.to_string(),
    };
    detail.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn tool_result_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks.iter().filter_map(|block| str_field(block, "text")).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

/// The stored session, in the same shape the old workspace used, so imported and new sessions read alike.
fn session_from(lines: &[(String, Value)], prompt: &str, root: &Path) -> Vec<Value> {
    let started = lines.first().map(|(at, _)| at.clone()).unwrap_or_else(now_iso);
    let mut session = vec![json!({ "kind": "prompt", "timestamp": started, "content": prompt })];
    let mut pending: HashMap<String, (String, Value, String)> = HashMap::new();
    for (at, event) in lines {
        let kind = str_field(event, "type").unwrap_or_default();
        if kind == "system" && str_field(event, "subtype") == Some("init") {
            let tools: Vec<&str> = event["tools"].as_array().map(|tools| tools.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
            session.push(json!({
                "kind": "system",
                "timestamp": at,
                "content": format!(
                    "Session started · model {} · tools {} · cwd {}",
                    str_field(event, "model").unwrap_or("?"),
                    if tools.is_empty() { "none".to_string() } else { tools.join(", ") },
                    str_field(event, "cwd").unwrap_or("?")
                ),
            }));
            continue;
        }
        if kind != "assistant" && kind != "user" {
            continue;
        }
        let Some(blocks) = event["message"]["content"].as_array() else {
            continue;
        };
        for block in blocks {
            match (kind, str_field(block, "type").unwrap_or_default()) {
                ("assistant", "text") => {
                    if let Some(text) = str_field(block, "text").filter(|text| !text.trim().is_empty()) {
                        session.push(json!({ "kind": "reviewer", "timestamp": at, "content": text, "action": { "type": "none" } }));
                    }
                }
                ("assistant", "thinking") => {
                    if let Some(text) = str_field(block, "thinking").filter(|text| !text.trim().is_empty()) {
                        session.push(json!({ "kind": "thinking", "timestamp": at, "content": text }));
                    }
                }
                ("assistant", "tool_use") => {
                    if let (Some(id), Some(name)) = (str_field(block, "id"), str_field(block, "name")) {
                        pending.insert(id.to_string(), (name.to_string(), block["input"].clone(), at.clone()));
                    }
                }
                ("user", "tool_result") => {
                    let Some(id) = str_field(block, "tool_use_id") else {
                        continue;
                    };
                    let (name, input, called_at) = pending.remove(id).unwrap_or_else(|| ("tool".to_string(), Value::Null, at.clone()));
                    session.push(json!({
                        "kind": "tool",
                        "timestamp": called_at,
                        "name": name,
                        "detail": tool_detail(&name, &input, root),
                        "input": input,
                        "output": clip(&tool_result_text(&block["content"]), TOOL_OUTPUT_MAX_CHARS),
                        "isError": block["is_error"].as_bool() == Some(true),
                    }));
                }
                _ => {}
            }
        }
    }
    for (name, input, at) in pending.into_values() {
        let detail = tool_detail(&name, &input, root);
        session.push(json!({ "kind": "tool", "timestamp": at, "name": name, "detail": detail, "input": input, "isError": name != "StructuredOutput", "output": "(no result)" }));
    }
    session
}
