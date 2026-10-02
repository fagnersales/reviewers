pub mod prompt;
pub mod terminal;

use crate::claude::{self, Request};
use crate::store::{Decision, Evidence, Project, Reviewer, Run, RunKind, Store, Usage, Verdict, new_decision_id};
use crate::util::{new_id, now_iso};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Exit codes the commit-msg hook returns; anything but 0 stops the commit.
pub const EXIT_APPROVED: i32 = 0;
pub const EXIT_BLOCKED: i32 = 1;
pub const EXIT_FAILED: i32 = 2;

pub struct StructuredDecision {
    pub verdict: Verdict,
    pub summary: String,
    pub reasoning: String,
    pub evidence: Vec<Evidence>,
}

/// A model that fumbles its tool call can leave the next parameter's markup inside a string.
fn without_leaked_markup(text: &str) -> &str {
    text.split("</parameter>").next().unwrap_or(text).trim()
}

pub fn parse_decision(output: &Value) -> Result<StructuredDecision, String> {
    let verdict = output["verdict"].as_str().and_then(Verdict::parse).ok_or("the answer has no verdict")?;
    let text = |key: &str| output[key].as_str().map(without_leaked_markup).filter(|text| !text.is_empty()).map(str::to_string);
    let evidence: Vec<Evidence> = match &output["evidence"] {
        Value::Null => Vec::new(),
        value => serde_json::from_value(value.clone()).map_err(|error| format!("the evidence is malformed: {error}"))?,
    };
    if verdict == Verdict::Blocked && evidence.is_empty() {
        return Err("a blocked verdict came without evidence".into());
    }
    Ok(StructuredDecision {
        verdict,
        summary: text("summary").ok_or("the answer has no summary")?,
        reasoning: text("reasoning").ok_or("the answer has no reasoning")?,
        evidence,
    })
}

/// The model a Reviewer runs on: its own, else the repo's, else the default set with `reviewers model`, else the CLI's.
pub fn resolve_model(store: &Store, project: &Project, reviewer: &Reviewer) -> Option<String> {
    reviewer
        .model
        .clone()
        .or_else(|| project.model.clone())
        .or_else(|| store.setting("default_model").ok().flatten())
}

pub struct Judged {
    pub reviewer: Reviewer,
    pub outcome: Result<Decision, String>,
}

/// One Reviewer, one diff, one verdict. Anything that isn't a verdict is an error, so callers fail closed.
pub fn judge(reviewer: &Reviewer, model: Option<&str>, repository: &str, diff: &str, root: &Path) -> Result<Decision, String> {
    let context_files = prompt::read_context_files(root, &reviewer.context_files);
    let text = prompt::review_prompt(&prompt::PromptInput {
        reviewer_name: &reviewer.name,
        instruction: &reviewer.instruction,
        repository,
        diff,
        context_files: &context_files,
    });
    let schema = prompt::decision_schema();
    let started = Instant::now();
    let outcome = claude::run(
        &Request {
            prompt: &text,
            cwd: root,
            schema: &schema,
            tools: &["Read", "Grep", "Glob"],
            allowed_tools: &["Read", "Grep", "Glob"],
            add_dirs: &[],
            model,
            timeout: Some(Duration::from_secs(prompt::REVIEW_TIMEOUT_SECS)),
            live: false,
        },
        &mut |_| {},
    )?;
    let decision = parse_decision(&outcome.output)?;
    let mut session = outcome.session;
    session.push(json!({ "kind": "result", "timestamp": now_iso(), "verdict": decision.verdict, "content": decision.summary }));
    Ok(Decision {
        id: new_decision_id(),
        run_id: String::new(),
        reviewer_id: reviewer.id.clone(),
        reviewer_name: reviewer.name.clone(),
        reviewer_version: reviewer.version,
        instruction: reviewer.instruction.clone(),
        verdict: decision.verdict,
        summary: decision.summary,
        reasoning: decision.reasoning,
        evidence: decision.evidence,
        session: Value::Array(session),
        model: outcome.model.or_else(|| model.map(str::to_string)),
        duration_ms: started.elapsed().as_millis() as u64,
        usage: Usage {
            tokens_read: outcome.tokens.read,
            tokens_written: outcome.tokens.written,
            turns: outcome.turns,
            tool_calls: outcome.tool_calls,
        },
    })
}

/// Every Reviewer at once: a commit waits for the slowest one, not for all of them in a row.
pub fn judge_all(store: &Store, project: &Project, reviewers: Vec<Reviewer>, diff: &str, root: &Path, on_done: &mut dyn FnMut(&Judged)) -> Vec<Judged> {
    let repository = project.remote.clone().unwrap_or_else(|| project.name.clone());
    let (sender, receiver) = mpsc::channel::<Judged>();
    let total = reviewers.len();
    std::thread::scope(|scope| {
        for reviewer in reviewers {
            let model = resolve_model(store, project, &reviewer);
            let sender = sender.clone();
            let repository = repository.as_str();
            scope.spawn(move || {
                let outcome = judge(&reviewer, model.as_deref(), repository, diff, root);
                let _ = sender.send(Judged { reviewer, outcome });
            });
        }
        drop(sender);
        let mut judged = Vec::with_capacity(total);
        for result in receiver {
            on_done(&result);
            judged.push(result);
        }
        judged
    })
}

pub struct Reviewed {
    pub run: Run,
    pub exit_code: i32,
}

pub fn record(store: &mut Store, project: &Project, kind: RunKind, judged: Vec<Judged>, diff: &str, attempted_message: Option<String>, started_at: String, started: Instant) -> Result<Reviewed, String> {
    let run_id = new_id("run");
    let mut failures = Vec::new();
    let mut decisions = Vec::new();
    let mut blocked = false;
    for item in judged {
        match item.outcome {
            Ok(mut decision) => {
                decision.run_id = run_id.clone();
                blocked |= decision.verdict == Verdict::Blocked && item.reviewer.blocking;
                decisions.push(decision);
            }
            Err(error) => failures.push(format!("{}: {error}", item.reviewer.name)),
        }
    }
    decisions.sort_by(|a, b| a.reviewer_name.cmp(&b.reviewer_name));
    let failure = (!failures.is_empty()).then(|| failures.join("\n"));
    let verdict = if blocked || failure.is_some() { Verdict::Blocked } else { Verdict::Approved };
    let run = Run {
        id: run_id,
        project_id: project.id.clone(),
        kind,
        verdict,
        failure,
        attempted_message,
        diff: diff.to_string(),
        diff_hash: crate::diff::hash(diff),
        started_at,
        ended_at: now_iso(),
        duration_ms: started.elapsed().as_millis() as u64,
        commit_sha: None,
        commit_message: None,
        committed_at: None,
        decisions,
    };
    store.record_run(&run)?;
    let exit_code = if run.failure.is_some() {
        EXIT_FAILED
    } else if run.verdict == Verdict::Blocked {
        EXIT_BLOCKED
    } else {
        EXIT_APPROVED
    };
    Ok(Reviewed { run, exit_code })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_verdict_whose_tool_call_went_wrong() {
        let fumbled = json!({ "verdict": "approved", "summary": "No abbreviations.", "reasoning": "All names are full words.</parameter>\n<parameter name=\"evidence\">[]" });
        let decision = parse_decision(&fumbled).expect("an approval needs no evidence");
        assert_eq!(decision.reasoning, "All names are full words.");
        assert!(parse_decision(&json!({ "verdict": "blocked", "summary": "x", "reasoning": "y" })).is_err());
    }
}
