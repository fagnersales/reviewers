//! Codex CLI adapter. The CLI owns authentication; no API keys are stored here.
use crate::agent::{Activity, Outcome, Request, Tokens};
use crate::util::{clip, new_id, now_iso, str_field, u64_field};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock, mpsc};
use std::time::{Duration, Instant};

fn running() -> &'static Mutex<Vec<u32>> {
    static RUNNING: OnceLock<Mutex<Vec<u32>>> = OnceLock::new();
    RUNNING.get_or_init(|| Mutex::new(Vec::new()))
}

pub fn stop_all() {
    for pid in running().lock().map(|pids| pids.clone()).unwrap_or_default() {
        terminate(pid);
    }
}

fn terminate(pid: u32) {
    #[cfg(unix)]
    let _ = Command::new("kill").args(["-KILL", "--", &format!("-{pid}")]).stdout(Stdio::null()).stderr(Stdio::null()).status();
    #[cfg(not(unix))]
    let _ = Command::new("taskkill").args(["/F", "/T", "/PID", &pid.to_string()]).status();
}

pub fn is_installed() -> bool {
    Command::new("codex").arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|status| status.success())
}

/// Codex's strict structured output requires closed objects and every property
/// in `required`. Optional properties remain optional in meaning via null.
fn strict_schema(schema: &Value) -> Value {
    let mut schema = schema.clone();
    if schema["type"] == "object" {
        let required = schema["required"].as_array().cloned().unwrap_or_default();
        let mut names = Vec::new();
        if let Some(properties) = schema["properties"].as_object_mut() {
            for (name, property) in properties {
                let strict = strict_schema(property);
                *property = if required.contains(&json!(name)) { strict } else { json!({ "anyOf": [strict, { "type": "null" }] }) };
                names.push(name.clone());
            }
        }
        schema["required"] = json!(names);
        schema["additionalProperties"] = json!(false);
    } else if schema["type"] == "array" {
        schema["items"] = strict_schema(&schema["items"]);
    }
    schema
}

struct SchemaFile(PathBuf);

impl SchemaFile {
    fn create(schema: &Value) -> Result<Self, String> {
        let path = std::env::temp_dir().join(format!("reviewers-schema-{}.json", new_id("codex")));
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&path).map_err(|error| format!("cannot create Codex schema: {error}"))?;
        let guard = Self(path);
        file.write_all(strict_schema(schema).to_string().as_bytes()).map_err(|error| format!("cannot write Codex schema: {error}"))?;
        Ok(guard)
    }
}

impl Drop for SchemaFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn arguments(request: &Request, schema: &Path) -> Vec<String> {
    let mut args: Vec<String> = [
        "exec",
        "--json",
        "--ephemeral",
        "--sandbox",
        "read-only",
        "--ignore-user-config",
        "--ignore-rules",
        "--skip-git-repo-check",
        "-c",
        "approval_policy=\"never\"",
        "-c",
        "web_search=\"disabled\"",
        "-c",
        "features.hooks=false",
        "-c",
        "features.multi_agent=false",
        "-c",
        "project_doc_max_bytes=0",
        "--color",
        "never",
        "--output-schema",
    ]
    .iter()
    .map(|arg| arg.to_string())
    .collect();
    args.push(schema.display().to_string());
    // The merge step needs no tools. Reviews and extraction can read with the
    // shell, constrained by the read-only sandbox. --add-dir grants writes and
    // must not be used for the reference directories in Request::add_dirs.
    if request.tools.is_empty() {
        args.extend(["-c".into(), "features.shell_tool=false".into()]);
    }
    if let Some(model) = request.model {
        args.extend(["--model".into(), model.into()]);
    }
    args.push("-".into());
    args
}

#[derive(Default)]
struct Events {
    answer: Option<String>,
    failure: Option<String>,
    completed: bool,
    tokens: Tokens,
    turns: u32,
    tool_calls: u32,
    session: Vec<Value>,
}

impl Events {
    fn accept(&mut self, event: &Value, on_activity: &mut dyn FnMut(Activity)) {
        let at = now_iso();
        match str_field(event, "type").unwrap_or_default() {
            "turn.completed" => {
                self.completed = true;
                self.turns += 1;
                let usage = &event["usage"];
                // cached_input_tokens is a subset of input_tokens, not extra input.
                self.tokens = self.tokens + Tokens { read: u64_field(usage, "input_tokens"), written: u64_field(usage, "output_tokens") };
                on_activity(Activity::Tokens(self.tokens));
            }
            "turn.failed" => self.failure = Some(str_field(&event["error"], "message").unwrap_or("turn failed").into()),
            "error" => self.failure = Some(str_field(event, "message").unwrap_or("unknown error").into()),
            "item.started" | "item.completed" => {
                let item = &event["item"];
                let completed = event["type"] == "item.completed";
                let kind = str_field(item, "type").unwrap_or_default();
                match kind {
                    "agent_message" if completed => {
                        let text = str_field(item, "text").unwrap_or_default();
                        self.answer = Some(text.into());
                        self.session.push(json!({ "kind": "reviewer", "timestamp": at, "content": text, "action": { "type": "none" } }));
                        if let Ok(output) = serde_json::from_str::<Value>(text) {
                            let names = output["rules"]
                                .as_array()
                                .or_else(|| output["reviewers"].as_array())
                                .map(|rules| rules.iter().filter_map(|rule| str_field(rule, "name").map(str::to_string)).collect())
                                .unwrap_or_default();
                            on_activity(Activity::Answering(names));
                        }
                    }
                    "reasoning" => {
                        on_activity(Activity::Thinking(0));
                        if completed {
                            self.session.push(json!({ "kind": "thinking", "timestamp": at, "content": str_field(item, "text").unwrap_or_default() }));
                        }
                    }
                    "command_execution" | "mcp_tool_call" | "web_search" | "file_change" => {
                        let name = if kind == "command_execution" { "Shell" } else { kind };
                        let input = json!({ "command": item["command"] });
                        if completed {
                            self.tool_calls += 1;
                            self.session.push(json!({
                                "kind": "tool", "timestamp": at, "name": name, "input": input,
                                "detail": str_field(item, "command").unwrap_or(name),
                                "output": clip(str_field(item, "aggregated_output").unwrap_or_default(), 8_000),
                                "isError": item["status"] == "failed" || item["exit_code"].as_i64().is_some_and(|code| code != 0),
                            }));
                        } else {
                            on_activity(Activity::Tool { name: name.into(), input });
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn finish(self, model: Option<&str>) -> Result<Outcome, String> {
        if let Some(error) = self.failure {
            return Err(format!("codex: {}", clip(&error, 300)));
        }
        if !self.completed {
            return Err("codex: no completed turn".into());
        }
        let output = self.answer.ok_or("codex: no final answer")?;
        let output: Value = serde_json::from_str(&output).map_err(|error| format!("codex: malformed structured answer: {error}"))?;
        if !output.is_object() {
            return Err("codex: structured answer is not an object".into());
        }
        Ok(Outcome {
            output,
            tokens: self.tokens,
            turns: self.turns,
            tool_calls: self.tool_calls,
            model: Some(model.map(|model| format!("codex:{model}")).unwrap_or_else(|| "codex".into())),
            session: self.session,
        })
    }
}

pub fn run(request: &Request, on_activity: &mut dyn FnMut(Activity)) -> Result<Outcome, String> {
    let schema = SchemaFile::create(request.schema)?;
    let mut command = Command::new("codex");
    command.args(arguments(request, &schema.0)).current_dir(request.cwd).env("REVIEWERS_BYPASS", "1").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    run_command(command, request, on_activity)
}

fn run_command(mut command: Command, request: &Request, on_activity: &mut dyn FnMut(Activity)) -> Result<Outcome, String> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.spawn().map_err(|error| format!("cannot start codex: {error}. Is Codex CLI installed?"))?;
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
    let stderr = child.stderr.take().map(|stderr| {
        std::thread::spawn(move || {
            let mut text = String::new();
            let _ = std::io::Read::read_to_string(&mut BufReader::new(stderr), &mut text);
            text
        })
    });
    let (sender, receiver) = mpsc::channel();
    if let Some(stdout) = child.stdout.take() {
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
    }
    let mut trace = std::env::var_os("REVIEWERS_TRACE_DIR").and_then(|dir| {
        std::fs::create_dir_all(&dir).ok()?;
        std::fs::File::create(PathBuf::from(dir).join(format!("codex-{}-{pid}.jsonl", now_iso().replace(':', "-")))).ok()
    });
    let mut events = Events::default();
    events.session.push(json!({ "kind": "prompt", "timestamp": now_iso(), "content": request.prompt }));
    events.session.push(
        json!({ "kind": "system", "timestamp": now_iso(), "content": format!("Codex · model {} · read-only · cwd {}", request.model.unwrap_or("default"), request.cwd.display()) }),
    );
    let started = Instant::now();
    let mut timed_out = false;
    loop {
        // Check even under a continuous stream of progress events.
        if request.timeout.is_some_and(|limit| started.elapsed() >= limit) {
            terminate(pid);
            let _ = child.kill();
            timed_out = true;
            break;
        }
        match receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(line) => {
                if let Some(trace) = trace.as_mut() {
                    let _ = writeln!(trace, "{line}");
                }
                if let Ok(event) = serde_json::from_str(&line) {
                    events.accept(&event, on_activity);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                if child.try_wait().map_err(|error| format!("cannot wait for codex: {error}"))?.is_some() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
    let status = child.wait();
    if let Ok(mut pids) = running().lock() {
        pids.retain(|running| *running != pid);
    }
    let stderr = stderr.and_then(|handle| handle.join().ok()).unwrap_or_default();
    if timed_out {
        return Err(format!("codex timed out after {}s", request.timeout.map(|limit| limit.as_secs()).unwrap_or_default()));
    }
    if !status.as_ref().is_ok_and(|status| status.success()) {
        return Err(format!("codex exited {}: {}", status.ok().and_then(|status| status.code()).unwrap_or(-1), clip(stderr.trim(), 300)));
    }
    events.finish(request.model)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn subprocess_failures_and_timeouts_fail_closed() {
        let schema = json!({});
        let request = Request {
            prompt: "test",
            cwd: Path::new("."),
            schema: &schema,
            tools: &[],
            allowed_tools: &[],
            add_dirs: &[],
            model: None,
            timeout: Some(Duration::from_millis(150)),
            live: false,
        };
        for script in [
            "cat >/dev/null; echo failure >&2; exit 7",
            "cat >/dev/null; sleep 30",
            "cat >/dev/null; exec 1>&-; sleep 30",
            "cat >/dev/null; while :; do echo '{\"type\":\"turn.started\"}'; done",
        ] {
            let mut command = Command::new("sh");
            command.args(["-c", script]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
            let started = Instant::now();
            let result = run_command(command, &request, &mut |_| {});
            assert!(result.is_err());
            assert!(started.elapsed() < Duration::from_secs(3));
        }
    }

    #[test]
    fn strict_objects_preserve_optional_evidence_and_line_numbers() {
        let original = crate::review::prompt::decision_schema();
        let schema = strict_schema(&original);
        assert_eq!(schema["additionalProperties"], false);
        let evidence = &schema["properties"]["evidence"]["anyOf"][0];
        assert_eq!(evidence["items"]["additionalProperties"], false);
        assert_eq!(evidence["items"]["properties"]["startLine"]["anyOf"][1]["type"], "null");
        assert_eq!(schema["required"].as_array().unwrap().len(), 4);
        assert!(original.get("additionalProperties").is_none());
    }

    #[test]
    fn normalizes_events_without_double_counting_cached_tokens() {
        let mut events = Events::default();
        let mut activity = Vec::new();
        for event in [
            json!({"type":"item.started","item":{"type":"command_execution","command":"cat src/lib.rs"}}),
            json!({"type":"item.completed","item":{"type":"command_execution","command":"cat src/lib.rs","aggregated_output":"code","exit_code":0}}),
            json!({"type":"item.completed","item":{"type":"agent_message","text":"{\"verdict\":\"approved\"}"}}),
            json!({"type":"turn.completed","usage":{"input_tokens":100,"cached_input_tokens":80,"output_tokens":10}}),
        ] {
            events.accept(&event, &mut |event| activity.push(event));
        }
        let result = events.finish(Some("custom-model")).unwrap();
        assert_eq!(result.output["verdict"], "approved");
        assert_eq!(result.tokens, Tokens { read: 100, written: 10 });
        assert_eq!(result.tool_calls, 1);
        assert_eq!(result.session[0]["output"], "code");
        assert_eq!(result.model.as_deref(), Some("codex:custom-model"));
        assert_eq!(activity.len(), 3);
    }

    #[test]
    fn failed_incomplete_and_malformed_turns_fail_closed() {
        for event in [json!({"type":"turn.failed","error":{"message":"rate limit"}}), json!({"type":"error","message":"unauthorized"})] {
            let mut events = Events { completed: true, answer: Some("{}".into()), ..Events::default() };
            events.accept(&event, &mut |_| {});
            assert!(events.finish(None).is_err());
        }
        assert!(Events::default().finish(None).is_err());
        assert!(Events { completed: true, answer: Some("not JSON".into()), ..Events::default() }.finish(None).is_err());
    }
}
