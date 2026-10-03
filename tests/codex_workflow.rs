//! Exercise the public CLI against a deterministic Codex executable, without
//! authentication, network requests or changes to the user's hooks/settings.
#![cfg(unix)]
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};

struct Workspace(PathBuf);

impl Workspace {
    fn new() -> Self {
        // Transcript scanning intentionally excludes throwaway repos in /tmp.
        let path = std::env::current_dir().unwrap().join("target").join(format!("codex-workflow-{}", std::process::id()));
        fs::create_dir_all(path.join("repo")).unwrap();
        fs::create_dir_all(path.join("bin")).unwrap();
        fs::create_dir_all(path.join("home")).unwrap();
        Self(path)
    }

    fn command(&self, program: &str) -> Command {
        let mut command = Command::new(program);
        command
            .current_dir(self.0.join("repo"))
            .env("HOME", self.0.join("home"))
            .env("REVIEWERS_HOME", self.0.join("data"))
            .env("CODEX_HOME", self.0.join("codex"))
            .env("CLAUDE_CONFIG_DIR", self.0.join("claude"))
            .env("GIT_CONFIG_GLOBAL", self.0.join("home/gitconfig"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("PATH", format!("{}:{}", self.0.join("bin").display(), std::env::var("PATH").unwrap()))
            .env("REVIEWERS_TEST_CALLS", self.0.join("calls"))
            .env_remove("REVIEWERS_BYPASS");
        command
    }

    fn run(&self, program: &str, args: &[&str]) -> Output {
        let output = self.command(program).args(args).output().unwrap();
        assert!(output.status.success(), "{args:?}\n{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        output
    }

    fn cli(&self, args: &[&str]) -> String {
        String::from_utf8(self.run(env!("CARGO_BIN_EXE_reviewers"), args).stdout).unwrap()
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn codex_onboards_both_transcripts_then_reviews_and_evaluates() {
    let workspace = Workspace::new();
    workspace.run("git", &["init", "-q"]);
    workspace.run("git", &["config", "user.name", "Test"]);
    workspace.run("git", &["config", "user.email", "test@example.invalid"]);
    fs::write(workspace.0.join("repo/example.txt"), "initial\n").unwrap();
    workspace.run("git", &["add", "example.txt"]);
    workspace.run("git", &["-c", "core.hooksPath=/dev/null", "commit", "-qm", "Initial"]);

    let mock = r#"#!/bin/sh
if [ "$1" = --version ]; then echo codex-test; exit 0; fi
printf '%s\n' "$*" >> "$REVIEWERS_TEST_CALLS"
prompt=$(cat)
if [ "$REVIEWERS_TEST_RESULT" = failure ]; then echo 'test failure' >&2; exit 7; fi
case "$prompt" in
  'You are setting up Reviewers'*)
    case "$prompt" in *'Never use casts'*'Always validate input'*) ;; *) echo 'missing mixed transcripts' >&2; exit 8;; esac
    answer='{"rules":[{"repo":"repo","name":"Validate input","instruction":"Block unvalidated input.","why":"Repeated instruction","timesSeen":3,"stated":true,"general":true,"paths":[],"lintable":false,"lintRule":"","evidence":[{"date":"2026-10-03","quote":"Always validate input"}]}]}' ;;
  'Several agents just read'*) answer='{"reviewers":[{"name":"Validate input","scope":"everywhere","sources":["r1"],"instructionFrom":"r1"}]}' ;;
  *) answer='{"verdict":"approved","summary":"Valid input","reasoning":"No unvalidated input added.","evidence":null}' ;;
esac
if [ "$REVIEWERS_TEST_RESULT" = blocked ]; then
  answer='{"verdict":"blocked","summary":"Unvalidated input","reasoning":"Input is unvalidated.","evidence":[{"file":"example.txt","startLine":1,"endLine":1,"excerpt":"validated","explanation":"Validate first"}]}'
fi
# Encode the answer as the string field of a JSONL agent_message.
escaped=$(printf '%s' "$answer" | sed 's/"/\\"/g')
printf '{"type":"item.completed","item":{"type":"agent_message","text":"%s"}}\n' "$escaped"
printf '%s\n' '{"type":"turn.completed","usage":{"input_tokens":50,"cached_input_tokens":20,"output_tokens":10}}'
"#;
    fs::write(workspace.0.join("bin/codex"), mock).unwrap();
    fs::set_permissions(workspace.0.join("bin/codex"), fs::Permissions::from_mode(0o755)).unwrap();
    // Ensure this workflow also works when Claude is not installed.
    fs::write(workspace.0.join("bin/claude"), "#!/bin/sh\nexit 127\n").unwrap();
    fs::set_permissions(workspace.0.join("bin/claude"), fs::Permissions::from_mode(0o755)).unwrap();

    let timestamp = chrono::Utc::now().to_rfc3339();
    let root = workspace.0.join("repo");
    let claude = workspace.0.join("claude/projects/project");
    let codex = workspace.0.join("codex/archived_sessions/2026/10/03");
    fs::create_dir_all(&claude).unwrap();
    fs::create_dir_all(&codex).unwrap();
    fs::write(claude.join("session.jsonl"), json!({"type":"user","cwd":root,"timestamp":timestamp,"message":{"content":"Never use casts"}}).to_string()).unwrap();
    let events = [
        json!({"type":"session_meta","payload":{"cwd":root,"source":"vscode"}}),
        json!({"type":"turn_context","timestamp":timestamp,"payload":{"model":"test-model"}}),
        json!({"type":"event_msg","timestamp":timestamp,"payload":{"type":"user_message","message":"Always validate input"}}),
    ];
    fs::write(codex.join("rollout.jsonl"), events.iter().map(Value::to_string).collect::<Vec<_>>().join("\n")).unwrap();
    workspace.cli(&["onboard", "--yes", "--no-skill", "--model", "codex:test-model"]);
    assert_eq!(workspace.cli(&["model"]).trim(), "codex:test-model");
    let reviewers: Value = serde_json::from_str(&workspace.cli(&["list", "--all", "--json"])).unwrap();
    assert!(reviewers.to_string().contains("Validate input"));
    let calls = fs::read_to_string(workspace.0.join("calls")).unwrap();
    assert_eq!(calls.lines().count(), 2, "both extraction and merging must run");
    assert!(calls.contains("features.shell_tool=false"));
    assert!(
        calls.lines().all(|line| line.contains("--sandbox read-only") && line.contains("--ephemeral") && line.contains("--output-schema") && line.contains("--model test-model"))
    );

    fs::write(root.join("example.txt"), "validated\n").unwrap();
    workspace.run("git", &["add", "example.txt"]);
    workspace.cli(&["hook", "commit-msg"]);
    workspace.cli(&["hook", "commit-msg"]);
    assert_eq!(fs::read_to_string(workspace.0.join("calls")).unwrap().lines().count(), 3, "unchanged reviews should reuse the verdict");
    let runs: Value = serde_json::from_str(&workspace.cli(&["runs", "--json"])).unwrap();
    assert!(!runs.as_array().unwrap().is_empty());
    workspace.cli(&["case", "add", "validate-input", "--name", "valid-input", "--expect", "approved", "--staged"]);
    workspace.cli(&["eval", "validate-input", "--record", "--json"]);
    assert_eq!(fs::read_to_string(workspace.0.join("calls")).unwrap().lines().count(), 4);

    workspace.cli(&["model", "codex:repo-model", "--repo"]);
    workspace.cli(&["hook", "commit-msg"]);
    assert!(fs::read_to_string(workspace.0.join("calls")).unwrap().lines().last().unwrap().contains("--model repo-model"));
    workspace.cli(&["edit", "validate-input", "--model", "codex:reviewer-model"]);
    workspace.cli(&["hook", "commit-msg"]);
    assert!(fs::read_to_string(workspace.0.join("calls")).unwrap().lines().last().unwrap().contains("--model reviewer-model"));
    for (result, code) in [("blocked", 1), ("failure", 3)] {
        let output =
            workspace.command(env!("CARGO_BIN_EXE_reviewers")).args(["hook", "commit-msg"]).env("REVIEWERS_TEST_RESULT", result).env("REVIEWERS_FRESH", "1").output().unwrap();
        assert_eq!(output.status.code(), Some(code), "{}", String::from_utf8_lossy(&output.stdout));
    }
}
