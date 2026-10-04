//! The classifier: a cheap first pass that clears the Reviewers a change can't concern, so they
//! don't start a reviewer session. It asks Jev, an evaluation model, one closed question per
//! Reviewer in a single call: should this Reviewer review this change at all? Not whether the
//! rule is broken, only whether the change holds anything the rule is about. An answer under the
//! Reviewer's cutoff clears it. The classifier never blocks: a high score, or any failure, and the Reviewer
//! runs as usual.

use crate::util::data_dir;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Below this, a Reviewer is cleared without running. Replayed over 183 real commits (667 sessions, 30
/// blocks), 15% skipped 46% of the sessions and missed no block; 20% missed 1, and 25% missed 3.
pub const DEFAULT_CUTOFF: f64 = 0.15;
/// Measured against the live model: a call is refused above roughly 32,768 input tokens.
const MAX_CHANGE_CHARS: usize = 40_000;
const MAX_QUESTION_CHARS: usize = 16_000;
/// The calls run at once, but past this many pieces a change costs more to classify than it saves.
const MAX_CHUNKS: usize = 12;
/// Reviewers the classifier may clear wait for it, so a slow answer can't hold a commit for long.
const TIMEOUT: Duration = Duration::from_secs(8);
const ATTEMPTS: u32 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    /// TypeSafe's own API, with a TypeSafe key.
    Jev,
    /// Jev through the Vercel AI Gateway, with an AI Gateway key.
    Gateway,
}

impl Provider {
    pub fn label(self) -> &'static str {
        match self {
            Provider::Jev => "Jev (TypeSafe)",
            Provider::Gateway => "Jev through the Vercel AI Gateway",
        }
    }

    pub fn key_name(self) -> &'static str {
        match self {
            Provider::Jev => "TypeSafe API key",
            Provider::Gateway => "AI Gateway API key",
        }
    }

    fn default_endpoint(self) -> String {
        match self {
            Provider::Jev => "https://api.typesafe.ai/v1/systemone",
            Provider::Gateway => "https://ai-gateway.vercel.sh/v4/ai/evaluation-model",
        }
        .to_string()
    }

    fn model(self) -> &'static str {
        match self {
            Provider::Jev => "jev-latest",
            Provider::Gateway => "typesafe-ai/jev",
        }
    }
}

/// The provider and its key, kept in the data directory, readable by the owner only.
#[derive(Clone, Serialize, Deserialize)]
pub struct Connection {
    pub provider: Provider,
    key: String,
    /// A SystemOne-compatible service of your own, instead of the provider's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
}

fn connection_path() -> PathBuf {
    data_dir().join("classifier.json")
}

pub fn connected() -> Option<Connection> {
    let text = std::fs::read_to_string(connection_path()).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn disconnect() -> Result<bool, String> {
    match std::fs::remove_file(connection_path()) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("cannot remove {}: {error}", connection_path().display())),
    }
}

impl Connection {
    pub fn new(provider: Provider, key: String, endpoint: Option<String>) -> Connection {
        Connection { provider, key: key.trim().to_string(), endpoint }
    }

    fn endpoint(&self) -> String {
        self.endpoint.clone().unwrap_or_else(|| self.provider.default_endpoint())
    }

    pub fn save(&self) -> Result<(), String> {
        let path = connection_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|error| error.to_string())?;
        write_private(&path, &text).map_err(|error| format!("cannot write {}: {error}", path.display()))
    }

    /// The key's last four characters, enough to tell two keys apart.
    pub fn key_hint(&self) -> String {
        let tail: String = self.key.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
        format!("…{tail}")
    }
}

#[cfg(unix)]
fn write_private(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?;
    file.write_all(text.as_bytes())
}

#[cfg(not(unix))]
fn write_private(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    std::fs::write(path, text)
}

/// What the classifier did for one Reviewer on one commit, kept on its decision.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Classified {
    pub provider: Provider,
    pub outcome: Outcome,
    /// The chance the Reviewer should review the change; none when the classifier couldn't answer.
    pub probability: Option<f64>,
    pub cutoff: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
    pub duration_ms: u64,
    pub tokens: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    /// Under the cutoff: approved without a reviewer session.
    Cleared,
    /// At or over the cutoff: the Reviewer ran as usual.
    Escalated,
    /// No answer (no key, an error, a change too large): the Reviewer ran as usual.
    Unavailable,
}

pub struct Question<'a> {
    pub id: String,
    pub name: &'a str,
    pub instruction: &'a str,
}

pub struct Scores {
    /// Per question id, the chance the Reviewer should review the change.
    pub probabilities: BTreeMap<String, f64>,
    pub duration_ms: u64,
    pub tokens: u64,
}

const TASK: &str = "A code Reviewer enforces one rule on every commit. Decide whether this Reviewer should review this change: whether the change holds anything the rule is about. Don't judge whether the rule is broken, only whether the Reviewer has something to check.";
const REVIEW: &str = "The change holds code, text or files the rule is about, so the Reviewer has something to check.";
const SKIP: &str = "Nothing in the change is something the rule is about, so the Reviewer has nothing to check.";

/// The change in pieces the model takes whole, files packed together where they fit.
fn change_chunks(diff: &str) -> Result<Vec<String>, String> {
    let mut chunks: Vec<String> = Vec::new();
    for piece in crate::diff::file_sections(diff).into_iter().flat_map(file_pieces) {
        match chunks.last_mut() {
            Some(last) if last.len() + piece.len() <= MAX_CHANGE_CHARS => last.push_str(&piece),
            _ => chunks.push(piece),
        }
    }
    if chunks.len() > MAX_CHUNKS {
        return Err(format!("the change is too large: over {MAX_CHUNKS} pieces for the classifier to read"));
    }
    Ok(chunks)
}

/// A file too large to read whole, cut between lines, each piece under the file's header so the
/// model knows which file it reads.
fn file_pieces(section: &str) -> Vec<String> {
    if section.len() <= MAX_CHANGE_CHARS {
        return vec![section.to_string()];
    }
    let hunks_start = section.match_indices("\n@@").next().map_or(section.len(), |(index, _)| index + 1);
    let (header, hunks) = section.split_at(hunks_start);
    let room = MAX_CHANGE_CHARS.saturating_sub(header.len());
    let mut pieces = Vec::new();
    let mut current = String::new();
    for line in hunks.split_inclusive('\n') {
        if !current.is_empty() && current.len() + line.len() > room {
            pieces.push(format!("{header}{current}"));
            current.clear();
        }
        current.push_str(line);
    }
    if !current.is_empty() || pieces.is_empty() {
        pieces.push(format!("{header}{current}"));
    }
    pieces
}

fn question_groups<'a, 'b>(questions: &'b [Question<'a>]) -> Vec<&'b [Question<'a>]> {
    let mut groups = Vec::new();
    let (mut start, mut size) = (0, 0);
    for (index, question) in questions.iter().enumerate() {
        let length = question.name.len() + question.instruction.len();
        if index > start && size + length > MAX_QUESTION_CHARS {
            groups.push(&questions[start..index]);
            (start, size) = (index, 0);
        }
        size += length;
    }
    if start < questions.len() {
        groups.push(&questions[start..]);
    }
    groups
}

fn request_body(provider: Provider, repository: &str, change: &str, questions: &[Question]) -> Value {
    let state = json!({ "repository": repository, "change": change });
    let asked: serde_json::Map<String, Value> = questions
        .iter()
        .map(|question| {
            let asked = match provider {
                Provider::Jev => json!({
                    "type": "choice",
                    "instructions": format!("{TASK}\n\nRule: {}\n{}\n\nShould this Reviewer review this change?", question.name, question.instruction),
                    // `fail` is the answer that sends the change to the Reviewer.
                    "criteria": { "pass": SKIP, "fail": REVIEW },
                }),
                Provider::Gateway => json!({
                    "type": "boolean",
                    "instructions": {
                        "task": TASK,
                        "rule": question.name,
                        "instruction": question.instruction,
                    },
                    "criteria": { "true": REVIEW, "false": SKIP },
                }),
            };
            (question.id.clone(), asked)
        })
        .collect();
    match provider {
        Provider::Jev => json!({ "model": provider.model(), "state": state, "questions": asked }),
        Provider::Gateway => json!({ "state": state, "questions": asked }),
    }
}

/// Each answer as the chance the Reviewer should review the change, plus the tokens the call used.
fn read_answers(provider: Provider, body: &Value, questions: &[Question]) -> Result<(Vec<(String, f64)>, u64), String> {
    // Cloudflare-hosted SystemOne models wrap the answers in `result`.
    let answers = body.get("answers").or_else(|| body.pointer("/result/answers")).ok_or("the classifier's answer has no answers")?;
    let mut scored = Vec::with_capacity(questions.len());
    for question in questions {
        let answer = answers.get(&question.id).ok_or_else(|| format!("no answer for {}", question.name))?;
        let probability = match provider {
            Provider::Jev => {
                let confidence = answer["confidence"].as_f64().ok_or_else(|| format!("no confidence for {}", question.name))?;
                match answer["choice"].as_str() {
                    Some("fail") => confidence,
                    Some("pass") => 1.0 - confidence,
                    other => return Err(format!("unexpected answer {other:?} for {}", question.name)),
                }
            }
            Provider::Gateway => answer["probability"].as_f64().ok_or_else(|| format!("no probability for {}", question.name))?,
        };
        if !(0.0..=1.0).contains(&probability) {
            return Err(format!("a probability of {probability} for {}", question.name));
        }
        scored.push((question.id.clone(), probability));
    }
    let tokens = ["inputTokens", "outputTokens"].iter().filter_map(|key| body["usage"][key].as_u64()).sum();
    Ok((scored, tokens))
}

fn post(connection: &Connection, body: &Value) -> Result<Value, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(TIMEOUT)).http_status_as_error(false).build().into();
    let payload = body.to_string();
    let mut last_problem = String::new();
    for attempt in 1..=ATTEMPTS {
        let mut request = agent
            .post(&connection.endpoint())
            .header("authorization", &format!("Bearer {}", connection.key))
            .header("content-type", "application/json")
            .header("user-agent", concat!("reviewers/", env!("CARGO_PKG_VERSION")));
        if connection.provider == Provider::Gateway {
            request = request
                .header("ai-gateway-protocol-version", "0.0.1")
                .header("ai-gateway-auth-method", "api-key")
                .header("ai-evaluation-model-specification-version", "4")
                .header("ai-model-id", Provider::Gateway.model());
        }
        match request.send(payload.as_str()) {
            Ok(mut response) => {
                let status = response.status().as_u16();
                let text = response.body_mut().read_to_string().unwrap_or_default();
                if (200..300).contains(&status) {
                    return serde_json::from_str(&text).map_err(|error| format!("the classifier's answer isn't JSON: {error}"));
                }
                last_problem = format!("the classifier answered {status}: {}", text.chars().take(300).collect::<String>());
                // A refused request is refused again; only rate limits and server trouble are worth a retry.
                if status < 500 && status != 429 {
                    return Err(last_problem);
                }
            }
            Err(error) => last_problem = format!("cannot reach the classifier: {error}"),
        }
        if attempt < ATTEMPTS {
            std::thread::sleep(Duration::from_millis(500 * u64::from(attempt)));
        }
    }
    Err(last_problem)
}

/// Every question about one change, in as few calls as the model's limits allow, all at once.
/// A question split over several pieces of the change takes its highest score.
pub fn score(connection: &Connection, repository: &str, diff: &str, questions: &[Question]) -> Result<Scores, String> {
    let started = Instant::now();
    let chunks = change_chunks(diff)?;
    let groups = question_groups(questions);
    let calls: Vec<(&String, &[Question])> = chunks.iter().flat_map(|chunk| groups.iter().map(move |group| (chunk, *group))).collect();
    let answers: Vec<Result<(Vec<(String, f64)>, u64), String>> = std::thread::scope(|scope| {
        let handles: Vec<_> = calls
            .iter()
            .map(|(chunk, group)| {
                scope.spawn(move || {
                    let body = post(connection, &request_body(connection.provider, repository, chunk, group))?;
                    read_answers(connection.provider, &body, group)
                })
            })
            .collect();
        handles.into_iter().map(|handle| handle.join().unwrap_or_else(|_| Err("the classifier call crashed".into()))).collect()
    });
    let mut probabilities: BTreeMap<String, f64> = BTreeMap::new();
    let mut tokens = 0;
    for answer in answers {
        let (scored, used) = answer?;
        tokens += used;
        for (id, probability) in scored {
            let highest = probabilities.entry(id).or_insert(probability);
            *highest = highest.max(probability);
        }
    }
    Ok(Scores { probabilities, duration_ms: started.elapsed().as_millis() as u64, tokens })
}

/// One small question, so a wrong key fails when it's connected, not on a commit.
pub fn ping(connection: &Connection) -> Result<(), String> {
    let diff = "diff --git a/hello.ts b/hello.ts\n--- a/hello.ts\n+++ b/hello.ts\n@@ -1 +1 @@\n-export const greeting = \"hi\";\n+export const greeting = \"hello\";\n";
    let question = Question { id: "ping".into(), name: "No type casts", instruction: "Block type assertions on unvalidated data." };
    score(connection, "reviewers", diff, std::slice::from_ref(&question)).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn questions() -> Vec<Question<'static>> {
        vec![
            Question { id: "r0".into(), name: "No type casts", instruction: "Block `as` casts." },
            Question { id: "r1".into(), name: "Spell out names", instruction: "Block abbreviations." },
        ]
    }

    #[test]
    fn asks_each_provider_in_its_own_shape() {
        let jev = request_body(Provider::Jev, "acme", "diff", &questions());
        assert_eq!(jev["model"], "jev-latest");
        assert_eq!(jev["questions"]["r0"]["type"], "choice");
        assert_eq!(jev["questions"]["r0"]["criteria"]["fail"], REVIEW);
        assert!(jev["questions"]["r1"]["instructions"].as_str().unwrap().contains("Spell out names"));

        let gateway = request_body(Provider::Gateway, "acme", "diff", &questions());
        assert!(gateway.get("model").is_none());
        assert_eq!(gateway["questions"]["r0"]["type"], "boolean");
        assert_eq!(gateway["questions"]["r0"]["criteria"]["true"], REVIEW);
        assert_eq!(gateway["state"]["change"], "diff");
    }

    #[test]
    fn reads_answers_as_the_chance_the_reviewer_should_review() {
        let jev = json!({ "answers": { "r0": { "type": "choice", "choice": "pass", "confidence": 0.9 }, "r1": { "type": "choice", "choice": "fail", "confidence": 0.7 } } });
        let (scored, _) = read_answers(Provider::Jev, &jev, &questions()).unwrap();
        assert!((scored[0].1 - 0.1).abs() < 1e-9 && (scored[1].1 - 0.7).abs() < 1e-9);

        let wrapped = json!({ "result": jev });
        assert!(read_answers(Provider::Jev, &wrapped, &questions()).is_ok());

        let gateway = json!({ "answers": { "r0": { "type": "boolean", "probability": 0.02 }, "r1": { "type": "boolean", "probability": 0.4 } }, "usage": { "inputTokens": 900, "outputTokens": 4 } });
        let (scored, tokens) = read_answers(Provider::Gateway, &gateway, &questions()).unwrap();
        assert_eq!((scored[0].1, scored[1].1, tokens), (0.02, 0.4, 904));

        let missing = json!({ "answers": { "r0": { "type": "boolean", "probability": 0.02 } } });
        assert!(read_answers(Provider::Gateway, &missing, &questions()).is_err());
    }

    #[test]
    fn splits_a_large_change_by_file_and_a_large_file_by_lines() {
        let file = |name: &str, lines: usize| format!("diff --git a/{name} b/{name}\n--- a/{name}\n+++ b/{name}\n@@ -1 +1 @@\n{}", format!("+{}\n", "x".repeat(99)).repeat(lines));
        let diff = format!("{}{}{}", file("a.ts", 250), file("b.ts", 250), file("c.ts", 100));
        let chunks = change_chunks(&diff).unwrap();
        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].contains("a.ts") && chunks[1].contains("b.ts") && chunks[1].contains("c.ts"));

        let huge = file("huge.ts", 1_000);
        let chunks = change_chunks(&huge).unwrap();
        assert_eq!(chunks.len(), 3);
        assert!(chunks.iter().all(|chunk| chunk.len() <= MAX_CHANGE_CHARS && chunk.starts_with("diff --git a/huge.ts b/huge.ts\n--- a/huge.ts\n+++ b/huge.ts\n")));
        assert_eq!(chunks.iter().map(|chunk| chunk.matches("+xxx").count()).sum::<usize>(), 1_000);

        let problem = change_chunks(&file("enormous.ts", 10_000)).unwrap_err();
        assert!(problem.contains("too large"), "{problem}");
    }
}
