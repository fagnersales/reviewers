pub mod prompt;
pub mod terminal;

use crate::claude::{self, Request};
use crate::classifier::{self, Classified};
use crate::store::{ClassifierUse, Decision, Evidence, Project, Reviewer, Run, RunKind, Store, Usage, Verdict, new_decision_id};
use crate::util::{new_id, now_iso};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Exit codes the commit-msg hook returns; anything but 0 stops the commit.
pub const EXIT_APPROVED: i32 = 0;
pub const EXIT_BLOCKED: i32 = 1;
/// Not 2: that's what a usage error (an unknown flag) exits with.
pub const EXIT_FAILED: i32 = 3;

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
        classifier: None,
        advisory: false,
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

/// The cutoff under which the classifier clears this Reviewer; none when it may not.
pub fn classifier_cutoff(store: &Store, reviewer: &Reviewer) -> Option<f64> {
    match reviewer.classifier {
        ClassifierUse::Off => None,
        ClassifierUse::Cutoff(cutoff) => Some(cutoff),
        ClassifierUse::Default => Some(store.setting("classifier_cutoff").ok().flatten().and_then(|text| text.parse().ok()).unwrap_or(classifier::DEFAULT_CUTOFF)),
    }
}

pub fn percent(probability: f64) -> String {
    format!("{:.0}%", probability * 100.0)
}

/// Approved without a session: the classifier put the chance of a broken rule under the cutoff.
fn cleared(reviewer: &Reviewer, note: Classified) -> Decision {
    let probability = note.probability.unwrap_or_default();
    Decision {
        id: new_decision_id(),
        run_id: String::new(),
        reviewer_id: reviewer.id.clone(),
        reviewer_name: reviewer.name.clone(),
        reviewer_version: reviewer.version,
        instruction: reviewer.instruction.clone(),
        verdict: Verdict::Approved,
        summary: format!("Cleared by the classifier: a {} chance this change breaks the rule, under the {} cutoff.", percent(probability), percent(note.cutoff)),
        reasoning: "No Claude session ran: the classifier judged the change unlikely to break this rule.".into(),
        evidence: Vec::new(),
        session: Value::Array(Vec::new()),
        model: None,
        duration_ms: note.duration_ms,
        usage: Usage::default(),
        input_hash: None,
        reused_from: None,
        classifier: Some(note),
        advisory: false,
    }
}

/// Every Reviewer at once: a commit waits for the slowest one, not for all of them in a row.
///
/// A Reviewer asked exactly what it was asked before gets that verdict back without running. A retry
/// after a timeout reruns only what timed out, and committing an unchanged diff can't re-roll a block.
/// With a classifier, the Reviewers it may skip wait for one call that clears the ones the change
/// can't concern; the others start right away.
pub fn judge_all(
    store: &Store,
    project: &Project,
    reviewers: Vec<Reviewer>,
    diff: &str,
    root: &Path,
    classifier: Option<&classifier::Connection>,
    on_done: &mut dyn FnMut(&Judged),
) -> Vec<Judged> {
    let repository = project.remote.clone().unwrap_or_else(|| project.name.clone());
    // `REVIEWERS_FRESH=1`: judge again even when the same input was judged before.
    let fresh = std::env::var("REVIEWERS_FRESH").is_ok_and(|value| value == "1");
    let mut judged = Vec::with_capacity(reviewers.len());
    let mut to_classify = Vec::new();
    let mut to_run = Vec::new();
    for reviewer in reviewers {
        let model = resolve_model(store, project, &reviewer);
        let prepared = prepare(&reviewer, model.as_deref(), &repository, diff, root);
        match store.judged_before(&project.id, &prepared.input_hash).map(|earlier| earlier.filter(|_| !fresh)) {
            Ok(Some(earlier)) => {
                let result = Judged { reviewer, outcome: Ok(reused(earlier)) };
                on_done(&result);
                judged.push(result);
            }
            _ => match classifier.and_then(|_| classifier_cutoff(store, &reviewer)) {
                Some(cutoff) => to_classify.push((reviewer, model, prepared, cutoff)),
                None => to_run.push((reviewer, model, prepared, None)),
            },
        }
    }
    let (sender, receiver) = mpsc::channel::<Judged>();
    std::thread::scope(|scope| {
        let start = |(reviewer, model, prepared, note): (Reviewer, Option<String>, Prepared, Option<Classified>)| {
            let sender = sender.clone();
            scope.spawn(move || {
                let outcome = judge_prepared(&reviewer, model.as_deref(), &prepared, root).map(|decision| Decision { classifier: note, ..decision });
                let _ = sender.send(Judged { reviewer, outcome });
            });
        };
        for item in to_run {
            start(item);
        }
        if let (Some(connection), false) = (classifier, to_classify.is_empty()) {
            let questions: Vec<classifier::Question> = to_classify
                .iter()
                .enumerate()
                .map(|(index, (reviewer, ..))| classifier::Question { id: format!("r{index}"), name: &reviewer.name, instruction: &reviewer.instruction })
                .collect();
            let scores = classifier::score(connection, &repository, diff, &questions);
            let share = scores.as_ref().map(|scores| scores.tokens / to_classify.len() as u64).unwrap_or(0);
            for (index, (reviewer, model, prepared, cutoff)) in to_classify.into_iter().enumerate() {
                let (probability, duration_ms, problem) = match &scores {
                    Ok(scores) => (scores.probabilities.get(&format!("r{index}")).copied(), scores.duration_ms, None),
                    Err(problem) => (None, 0, Some(problem.clone())),
                };
                let outcome = match probability {
                    Some(probability) if probability < cutoff => classifier::Outcome::Cleared,
                    Some(_) => classifier::Outcome::Escalated,
                    None => classifier::Outcome::Unavailable,
                };
                let note = Classified { provider: connection.provider, outcome, probability, cutoff, problem, duration_ms, tokens: share };
                if outcome == classifier::Outcome::Cleared {
                    let result = Judged { outcome: Ok(cleared(&reviewer, note)), reviewer };
                    on_done(&result);
                    judged.push(result);
                } else {
                    start((reviewer, model, prepared, Some(note)));
                }
            }
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
                decision.advisory = !item.reviewer.blocking;
                blocked |= decision.verdict == Verdict::Blocked && item.reviewer.blocking;
                decisions.push(decision);
            }
            Err(error) if item.reviewer.blocking => failures.push(format!("{}: {error}", item.reviewer.name)),
            // Advisory: it never stops a commit, not even by failing. The hook already said so.
            Err(_) => {}
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
                blocking: true,
                model: None,
                classifier: crate::store::ClassifierUse::Default,
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
            classifier: None,
            advisory: false,
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
            let judged = judge_all(&store, &project, vec![reviewer.clone()], DIFF, &directory, None, &mut |_| {});
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

    #[test]
    fn an_advisory_block_is_reported_and_lets_the_commit_through() {
        let (mut store, project, reviewer, directory) = fixture();
        let advisory = store.update_reviewer(&reviewer.id, crate::store::ReviewerChanges { blocking: Some(false), ..Default::default() }).unwrap();
        let judged = vec![Judged { reviewer: advisory.clone(), outcome: Ok(judged_block(&advisory, "hash".into())) }];
        let reviewed = record(&mut store, &project, RunKind::Review, judged, DIFF, None, now_iso(), Instant::now()).unwrap();
        assert_eq!(reviewed.exit_code, EXIT_APPROVED);
        assert!(reviewed.run.decisions[0].advisory);
        let report = terminal::report(&reviewed.run, &terminal::Paint::plain());
        assert!(report.contains("1 Reviewer passed") && report.contains("1 advisory note"), "{report}");
        assert!(report.contains("Advisory only: the commit went through"), "{report}");
        assert!(!report.contains("blocked by") && !report.contains("Fix the code above"), "{report}");
        // Read back, it's still marked advisory.
        assert!(store.run(&reviewed.run.id, false).unwrap().unwrap().decisions[0].advisory);
        // An advisory Reviewer that reaches no verdict doesn't stop the commit either; a blocking one does.
        let failed = |reviewer: &Reviewer| vec![Judged { reviewer: reviewer.clone(), outcome: Err("timed out".into()) }];
        let reviewed = record(&mut store, &project, RunKind::Review, failed(&advisory), DIFF, None, now_iso(), Instant::now()).unwrap();
        assert_eq!((reviewed.exit_code, reviewed.run.failure.is_none()), (EXIT_APPROVED, true));
        let reviewed = record(&mut store, &project, RunKind::Review, failed(&reviewer), DIFF, None, now_iso(), Instant::now()).unwrap();
        assert_eq!(reviewed.exit_code, EXIT_FAILED);
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// A one-request HTTP server that answers every question with the same probability.
    fn classifier_answering(probability: f64) -> String {
        use std::io::{BufRead, BufReader, Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut length = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap();
                }
                if line == "\r\n" {
                    break;
                }
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            let request: Value = serde_json::from_slice(&body).unwrap();
            let answers: serde_json::Map<String, Value> = request["questions"].as_object().unwrap().keys().map(|id| (id.clone(), json!({ "type": "boolean", "probability": probability }))).collect();
            let reply = json!({ "answers": answers, "usage": { "inputTokens": 1200, "outputTokens": 3 } }).to_string();
            let mut stream = stream;
            write!(stream, "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}", reply.len()).unwrap();
        });
        format!("http://{address}/")
    }

    #[test]
    fn the_classifier_clears_a_reviewer_without_starting_a_session() {
        let (store, project, reviewer, directory) = fixture();
        let connection = classifier::Connection::new(classifier::Provider::Gateway, "test-key".into(), Some(classifier_answering(0.04)));
        let started = Instant::now();
        let judged = judge_all(&store, &project, vec![reviewer.clone()], DIFF, &directory, Some(&connection), &mut |_| {});
        let decision = judged.into_iter().next().unwrap().outcome.unwrap();
        assert_eq!(decision.verdict, Verdict::Approved);
        let note = decision.classifier.expect("the classifier's note");
        assert_eq!((note.outcome, note.probability, note.cutoff), (classifier::Outcome::Cleared, Some(0.04), classifier::DEFAULT_CUTOFF));
        assert_eq!(note.tokens, 1203);
        // No Claude session: no tokens of its own, and far quicker than one.
        assert_eq!(decision.usage.tokens_read + decision.usage.turns as u64, 0);
        assert!(started.elapsed() < Duration::from_secs(5));
        // A Reviewer turned off for the classifier never asks it.
        let off = Reviewer { classifier: ClassifierUse::Off, ..reviewer };
        assert_eq!(classifier_cutoff(&store, &off), None);
        let _ = std::fs::remove_dir_all(&directory);
    }
}
