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

/// Exactly what a Reviewer is asked. The same input gets the same verdict, so its hash is the key to an earlier one.
pub struct Prepared {
    prompt: String,
    input_hash: String,
}

pub fn prepare(reviewer: &Reviewer, model: Option<&str>, repository: &str, diff: &str, root: &Path) -> Prepared {
    let context_files = prompt::read_context_files(root, &reviewer.context_files);
    let prompt = prompt::review_prompt(&prompt::PromptInput {
        reviewer_name: &reviewer.name,
        instruction: &reviewer.instruction,
        repository,
        diff,
        context_files: &context_files,
    });
    let input_hash = crate::util::sha256_hex(&format!("{}\n{}\n{prompt}", model.unwrap_or_default(), prompt::decision_schema()));
    Prepared { prompt, input_hash }
}

/// One Reviewer, one diff, one verdict. Anything that isn't a verdict is an error, so callers fail closed.
pub fn judge(reviewer: &Reviewer, model: Option<&str>, repository: &str, diff: &str, root: &Path) -> Result<Decision, String> {
    judge_prepared(reviewer, model, &prepare(reviewer, model, repository, diff, root), root)
}

fn judge_prepared(reviewer: &Reviewer, model: Option<&str>, prepared: &Prepared, root: &Path) -> Result<Decision, String> {
    let schema = prompt::decision_schema();
    let started = Instant::now();
    let outcome = claude::run(
        &Request {
            prompt: &prepared.prompt,
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
        input_hash: Some(prepared.input_hash.clone()),
        reused_from: None,
    })
}

/// An earlier verdict given back for the same input. Nothing ran, so it took no time and no tokens.
fn reused(earlier: Decision) -> Decision {
    let judged_in = earlier.reused_from.clone().unwrap_or_else(|| earlier.run_id.clone());
    Decision {
        id: new_decision_id(),
        run_id: String::new(),
        session: Value::Array(Vec::new()),
        duration_ms: 0,
        usage: Usage::default(),
        reused_from: Some(judged_in),
        ..earlier
    }
}

/// Every Reviewer at once: a commit waits for the slowest one, not for all of them in a row.
///
/// A Reviewer asked exactly what it was asked before gets that verdict back without running. A retry
/// after a timeout reruns only what timed out, and committing an unchanged diff can't re-roll a block.
pub fn judge_all(store: &Store, project: &Project, reviewers: Vec<Reviewer>, diff: &str, root: &Path, on_done: &mut dyn FnMut(&Judged)) -> Vec<Judged> {
    let repository = project.remote.clone().unwrap_or_else(|| project.name.clone());
    let mut judged = Vec::with_capacity(reviewers.len());
    let mut to_run = Vec::new();
    for reviewer in reviewers {
        let model = resolve_model(store, project, &reviewer);
        let prepared = prepare(&reviewer, model.as_deref(), &repository, diff, root);
        match store.judged_before(&project.id, &prepared.input_hash) {
            Ok(Some(earlier)) => {
                let result = Judged { reviewer, outcome: Ok(reused(earlier)) };
                on_done(&result);
                judged.push(result);
            }
            _ => to_run.push((reviewer, model, prepared)),
        }
    }
    let (sender, receiver) = mpsc::channel::<Judged>();
    std::thread::scope(|scope| {
        for (reviewer, model, prepared) in to_run {
            let sender = sender.clone();
            scope.spawn(move || {
                let outcome = judge_prepared(&reviewer, model.as_deref(), &prepared, root);
                let _ = sender.send(Judged { reviewer, outcome });
            });
        }
        drop(sender);
        for result in receiver {
            on_done(&result);
            judged.push(result);
        }
    });
    judged
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

    const DIFF: &str = "diff --git a/src/users.ts b/src/users.ts\n--- a/src/users.ts\n+++ b/src/users.ts\n@@ -1 +1 @@\n-export const user = body;\n+export const user = body as User;\n";

    fn fixture() -> (Store, Project, Reviewer, std::path::PathBuf) {
        let directory = std::env::temp_dir().join(new_id("reviewers-test"));
        std::fs::create_dir_all(&directory).unwrap();
        let store = Store::open(&directory.join("reviewers.sqlite")).unwrap();
        let project = store.ensure_project(&directory.display().to_string(), None).unwrap();
        let reviewer = store
            .create_reviewer(crate::store::NewReviewer {
                name: "No type casts".into(),
                instruction: "Block `as` casts on unvalidated data.".into(),
                scope: crate::store::Scope::Everywhere,
                project_ids: Vec::new(),
                paths: Vec::new(),
                context_files: Vec::new(),
                enabled: true,
                model: None,
                origin: json!({}),
            })
            .unwrap();
        (store, project, reviewer, directory)
    }

    fn judged_block(reviewer: &Reviewer, input_hash: String) -> Decision {
        Decision {
            id: new_decision_id(),
            run_id: String::new(),
            reviewer_id: reviewer.id.clone(),
            reviewer_name: reviewer.name.clone(),
            reviewer_version: reviewer.version,
            instruction: reviewer.instruction.clone(),
            verdict: Verdict::Blocked,
            summary: "Casts the API response.".into(),
            reasoning: "`body as User` asserts a type on unvalidated data.".into(),
            evidence: vec![Evidence { file: "src/users.ts".into(), start_line: Some(1), end_line: Some(1), excerpt: "body as User".into(), explanation: "Validate it instead.".into() }],
            session: json!([{ "kind": "result" }]),
            model: Some("claude-sonnet-5".into()),
            duration_ms: 9_000,
            usage: Usage { tokens_read: 18_000, tokens_written: 900, turns: 3, tool_calls: 2 },
            input_hash: Some(input_hash),
            reused_from: None,
        }
    }

    #[test]
    fn an_unchanged_input_gets_its_verdict_back_without_running() {
        let (mut store, project, reviewer, directory) = fixture();
        let model = resolve_model(&store, &project, &reviewer);
        let prepared = prepare(&reviewer, model.as_deref(), &project.name, DIFF, &directory);
        let first = record(&mut store, &project, RunKind::Review, vec![Judged { reviewer: reviewer.clone(), outcome: Ok(judged_block(&reviewer, prepared.input_hash.clone())) }], DIFF, None, now_iso(), Instant::now()).unwrap();
        // Checked before judge_all, so a miss fails here instead of starting a real Claude session.
        assert!(store.judged_before(&project.id, &prepared.input_hash).unwrap().is_some());

        for attempt in 0..2 {
            let judged = judge_all(&store, &project, vec![reviewer.clone()], DIFF, &directory, &mut |_| {});
            let decision = judged.into_iter().next().unwrap().outcome.unwrap();
            assert_eq!(decision.verdict, Verdict::Blocked);
            // A retry of a retry still points at the run that actually judged it.
            assert_eq!(decision.reused_from.as_deref(), Some(first.run.id.as_str()), "attempt {attempt}");
            assert_eq!((decision.duration_ms, decision.usage.tokens_read, decision.usage.tokens_written), (0, 0, 0));
            assert_eq!(decision.evidence.len(), 1);
            let retry = record(&mut store, &project, RunKind::Review, vec![Judged { reviewer: reviewer.clone(), outcome: Ok(decision) }], DIFF, None, now_iso(), Instant::now()).unwrap();
            assert_eq!(retry.exit_code, EXIT_BLOCKED);
        }

        // Stats count the one verdict that ran, not the two given back.
        let records = store.reviewer_records(Some(&project.id), None).unwrap();
        assert_eq!(records[0].runs, 1);

        // A change to the code or to the Reviewer is a new input, judged anew.
        let changed_code = prepare(&reviewer, model.as_deref(), &project.name, &DIFF.replace("User", "Account"), &directory);
        assert!(store.judged_before(&project.id, &changed_code.input_hash).unwrap().is_none());
        let edited = store
            .update_reviewer(&reviewer.id, crate::store::ReviewerChanges { instruction: Some("Block every `as` cast.".into()), ..Default::default() })
            .unwrap();
        let changed_rule = prepare(&edited, model.as_deref(), &project.name, DIFF, &directory);
        assert!(store.judged_before(&project.id, &changed_rule.input_hash).unwrap().is_none());
        let other_model = prepare(&reviewer, Some("opus"), &project.name, DIFF, &directory);
        assert!(store.judged_before(&project.id, &other_model.input_hash).unwrap().is_none());
        let _ = std::fs::remove_dir_all(&directory);
    }
}
