pub mod digest;
pub mod transcripts;

use crate::agent::{self, Activity, Provider, Request, Tokens};
use crate::commands::Outcome;
use crate::commands::onboard::OnboardArgs;
use crate::store::{ClassifierUse, NewReviewer, Scope, Store};
use crate::util::{compact, duration, home_path, plural, slugify, str_field, thousands};
use crate::{git, hooks, skill, ui};
use digest::{Job, RepoSummary};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, mpsc};
use std::time::{Duration, Instant};

const EXTRACT: &str = include_str!("extract.md");
const MERGE: &str = include_str!("merge.md");
const ATTEMPTS: usize = 2;
const EVIDENCE_PER_REVIEWER: usize = 4;
/// The first run turns on the strongest rules, up to this many: every one is an agent session on every commit it applies to.
const STARTING_REVIEWERS: usize = 10;
/// Past this many, each Reviewer gets one line; the full instructions are in the files.
const DETAILED_LIST_LIMIT: usize = 20;
const LABEL_WIDTH: usize = 40;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuleEvidence {
    date: String,
    quote: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Rule {
    #[serde(default)]
    repo: String,
    name: String,
    instruction: String,
    #[serde(default)]
    why: String,
    #[serde(default = "seen_once")]
    times_seen: u32,
    #[serde(default)]
    stated: bool,
    #[serde(default)]
    general: bool,
    #[serde(default)]
    paths: Vec<String>,
    #[serde(default)]
    lintable: bool,
    #[serde(default)]
    lint_rule: String,
    #[serde(default)]
    evidence: Vec<RuleEvidence>,
}

fn seen_once() -> u32 {
    1
}

#[derive(Clone, Serialize)]
struct Candidate {
    id: String,
    #[serde(flatten)]
    rule: Rule,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Planned {
    name: String,
    scope: String,
    sources: Vec<String>,
    instruction_from: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Suggestion {
    name: String,
    everywhere: bool,
    repos: Vec<String>,
    paths: Vec<String>,
    instruction: String,
    why: String,
    times_seen: u32,
    lintable: bool,
    lint_rule: String,
    evidence: Vec<Value>,
}

fn extract_schema() -> Value {
    let text = json!({ "type": "string" });
    json!({
        "type": "object",
        "properties": {
            "rules": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "repo": text, "name": text, "instruction": text, "why": text,
                        "timesSeen": { "type": "integer" }, "stated": { "type": "boolean" }, "general": { "type": "boolean" },
                        "paths": { "type": "array", "items": text }, "lintable": { "type": "boolean" }, "lintRule": text,
                        "evidence": { "type": "array", "items": { "type": "object", "properties": { "date": text, "quote": text }, "required": ["date", "quote"] } }
                    },
                    "required": ["repo", "name", "instruction", "why", "timesSeen", "stated", "general", "paths", "lintable", "lintRule", "evidence"]
                }
            }
        },
        "required": ["rules"]
    })
}

fn merge_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "reviewers": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" },
                        "scope": { "type": "string", "enum": ["everywhere", "project"] },
                        "sources": { "type": "array", "items": { "type": "string" } },
                        "instructionFrom": { "type": "string" }
                    },
                    "required": ["name", "scope", "sources", "instructionFrom"]
                }
            }
        },
        "required": ["reviewers"]
    })
}

/// A function replacement, so `$&` or `$'` inside quoted code is never read as a pattern.
fn fill(template: &str, values: &[(&str, &str)]) -> String {
    values.iter().fold(template.to_string(), |text, (key, value)| text.replace(&format!("{{{{{key}}}}}"), value))
}

/// `claude-opus-5-5` → "Opus 5.5", `claude-haiku-4-5-20251001` → "Haiku 4.5".
pub fn model_label(id: &str) -> String {
    if id == "codex" {
        return "Codex (default)".into();
    }
    if let Some(model) = id.strip_prefix("codex:") {
        return format!("Codex · {model}");
    }
    static PATTERN: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let pattern = PATTERN.get_or_init(|| Regex::new(r"^claude-([a-z]+)-(\d+)(?:-(\d{1,2}))?(?:-\d{8})?$").expect("the pattern compiles"));
    let bare = id.split('[').next().unwrap_or(id);
    match pattern.captures(bare) {
        Some(captures) => {
            let family = &captures[1];
            let mut label = format!("{}{} {}", family[..1].to_uppercase(), &family[1..], &captures[2]);
            if let Some(minor) = captures.get(3) {
                label.push('.');
                label.push_str(minor.as_str());
            }
            label
        }
        None => id.to_string(),
    }
}

/// The models this person actually ran, most used lately first, then the latest of each family they never touched.
fn model_choices(usage: &[transcripts::ModelUsage], claude: bool, codex: bool) -> Vec<(String, ui::Choice)> {
    let mut choices: Vec<(String, ui::Choice)> = usage
        .iter()
        .filter(|model| match agent::selection(Some(&model.id)).0 { Provider::Claude => claude, Provider::Codex => codex })
        .take(4)
        .map(|model| {
            let hint = if model.recent_sessions > 0 {
                format!("used in {} of your last {} sessions", model.recent_sessions, transcripts::RECENT_SESSIONS)
            } else {
                format!("used in {} earlier sessions", model.sessions)
            };
            (model.id.clone(), ui::Choice { label: model_label(&model.id), hint })
        })
        .collect();
    for family in ["opus", "sonnet", "haiku"] {
        if claude && !choices.iter().any(|(id, _)| id.split('-').nth(1) == Some(family)) {
            choices.push((family.to_string(), ui::Choice { label: format!("{}{} (latest)", family[..1].to_uppercase(), &family[1..]), hint: String::new() }));
        }
    }
    if codex {
        choices.push(("codex".into(), ui::Choice { label: model_label("codex"), hint: "Codex CLI's default model".into() }));
    }
    choices
}

enum JobState {
    Waiting,
    Running { started: Instant, action: String },
    Done { started: Instant, ended: Instant, rules: usize, tokens: Tokens },
    Failed { started: Instant, ended: Instant, error: String },
}

enum JobEvent {
    Started(usize),
    Action(usize, String),
    Tokens(usize, usize, Tokens),
    Done(usize, usize, Vec<Rule>, Tokens),
    Failed(usize, String),
}

fn describe(activity: &Activity, job: &Job) -> Option<String> {
    let clean = |text: &str| {
        let mut out = text.replace(&format!("{}/", job.directory.display()), "");
        for stretch in &job.stretches {
            out = out.replace(&format!("{}/", stretch.root.display()), &format!("{}/", stretch.repo));
        }
        out.split_whitespace().collect::<Vec<_>>().join(" ")
    };
    match activity {
        Activity::Thinking(tokens) if *tokens > 0 => Some(format!("Thinking · {} tokens", compact(*tokens))),
        Activity::Thinking(_) => Some("Thinking".into()),
        Activity::Answering(names) => names.last().map(|name| format!("Writing rule {}: {name}", names.len())),
        Activity::Tool { name, input } => {
            let field = |key: &str| str_field(input, key).map(clean).unwrap_or_default();
            Some(match name.as_str() {
                "Read" => format!("Reading {}", field("file_path")),
                "Grep" => format!("Searching \"{}\"{}", field("pattern"), if field("path").is_empty() { String::new() } else { format!(" in {}", field("path")) }),
                "Glob" => format!("Listing {}", field("pattern")),
                "Shell" => format!("Reading · {}", field("command")),
                other => other.to_string(),
            })
        }
        Activity::Tokens(_) => None,
    }
}

fn agent_row(job: &Job, state: &JobState, frame: &str) -> String {
    let label = ui::pad(&ui::truncate(&job.label, LABEL_WIDTH), LABEL_WIDTH);
    let bar = ui::bar();
    match state {
        JobState::Waiting => format!("{bar}  {} {}{}  {}", ui::dim("·"), ui::dim(&label), " ".repeat(8), ui::dim(&format!("{} · waiting", plural(job.messages, "message")))),
        JobState::Running { started, action } => format!("{bar}  {} {label}{}  {}", ui::magenta(frame), ui::dim(&format!("{:>8}", duration(started.elapsed().as_millis() as u64))), ui::dim(action)),
        JobState::Done { started, ended, rules, tokens } => format!(
            "{bar}  {} {label}{}  {}",
            ui::green("✓"),
            ui::dim(&format!("{:>8}", duration(ended.duration_since(*started).as_millis() as u64))),
            ui::dim(&format!("{} · {} tokens", plural(*rules, "rule"), compact(tokens.read + tokens.written)))
        ),
        JobState::Failed { started, ended, error } => format!(
            "{bar}  {} {label}{}  {}",
            ui::red("✗"),
            ui::dim(&format!("{:>8}", duration(ended.duration_since(*started).as_millis() as u64))),
            ui::red(error)
        ),
    }
}

/// Every agent at once, one row each, redrawn in place. Returns every rule found and the tokens spent.
fn extract(jobs: &[Job], parallel: usize, model: Option<&str>, live: bool) -> (Vec<Candidate>, Tokens, usize) {
    let schema = extract_schema();
    let queue: Mutex<VecDeque<usize>> = Mutex::new((0..jobs.len()).collect());
    let (sender, receiver) = mpsc::channel::<JobEvent>();
    let mut states: Vec<JobState> = jobs.iter().map(|_| JobState::Waiting).collect();
    let mut spent: Vec<Vec<Tokens>> = jobs.iter().map(|_| vec![Tokens::default(); ATTEMPTS]).collect();
    let mut candidates: Vec<Candidate> = Vec::new();
    let repos: Vec<String> = {
        let mut names: Vec<String> = jobs.iter().flat_map(|job| job.stretches.iter().map(|stretch| stretch.repo.clone())).collect();
        names.sort();
        names.dedup();
        names
    };
    let messages: usize = jobs.iter().map(|job| job.messages).sum();
    let started = Instant::now();
    std::thread::scope(|scope| {
        for _ in 0..parallel.min(jobs.len()) {
            let sender = sender.clone();
            let queue = &queue;
            let schema = &schema;
            scope.spawn(move || {
                loop {
                    let Some(index) = queue.lock().ok().and_then(|mut queue| queue.pop_front()) else {
                        break;
                    };
                    let job = &jobs[index];
                    let prompt = fill(EXTRACT, &[("SCOPE", &digest::scope_sentence(job)), ("DIGEST", &job.directory.display().to_string()), ("MATERIAL", &digest::material(job))]);
                    let add_dirs: Vec<PathBuf> = job.stretches.iter().map(|stretch| stretch.root.clone()).collect();
                    for attempt in 0..ATTEMPTS {
                        let _ = sender.send(JobEvent::Started(index));
                        let result = agent::run(
                            &Request {
                                prompt: &prompt,
                                cwd: &job.directory,
                                schema,
                                tools: &["Read", "Grep", "Glob"],
                                allowed_tools: &["Read", "Grep", "Glob"],
                                add_dirs: &add_dirs,
                                model,
                                timeout: Some(Duration::from_secs(900)),
                                live: true,
                            },
                            &mut |activity| {
                                if let Activity::Tokens(tokens) = &activity {
                                    let _ = sender.send(JobEvent::Tokens(index, attempt, *tokens));
                                } else if let Some(action) = describe(&activity, job) {
                                    let _ = sender.send(JobEvent::Action(index, action));
                                }
                            },
                        )
                        .and_then(|outcome| {
                            let rules: Vec<Rule> = serde_json::from_value(outcome.output["rules"].clone()).map_err(|error| format!("malformed answer: {error}"))?;
                            Ok((rules, outcome.tokens))
                        });
                        match result {
                            Ok((rules, tokens)) => {
                                let _ = sender.send(JobEvent::Done(index, attempt, rules, tokens));
                                break;
                            }
                            Err(error) if attempt + 1 == ATTEMPTS => {
                                let _ = sender.send(JobEvent::Failed(index, error));
                            }
                            Err(_) => {}
                        }
                    }
                }
            });
        }
        drop(sender);

        let mut region = live.then(ui::LiveRegion::new);
        let mut frame = 0usize;
        let lines = |states: &[JobState], spent: &[Vec<Tokens>], found: usize, frame: usize, finished: bool| {
            let tokens = spent.iter().flatten().fold(Tokens::default(), |total, tokens| total + *tokens);
            let agents = plural(jobs.len(), "agent");
            let header = if finished {
                format!("{}  Read {} from {} with {agents}", ui::green(ui::STEP_DONE), plural(messages, "message"), plural(repos.len(), "repo"))
            } else {
                let at_once = if parallel < jobs.len() { format!(", {parallel} at a time") } else { String::new() };
                format!("{}  Reading {} from {} · {agents}{at_once}", ui::cyan(ui::STEP_ACTIVE), plural(messages, "message"), plural(repos.len(), "repo"))
            };
            let mut out = vec![ui::bar(), header];
            out.extend(jobs.iter().zip(states).map(|(job, state)| agent_row(job, state, ui::SPINNER[frame % 4])));
            out.push(ui::bar());
            out.push(format!(
                "{}  {}",
                ui::bar(),
                ui::dim(&format!("{} · {} tokens read · {} written · {} found", duration(started.elapsed().as_millis() as u64), compact(tokens.read), compact(tokens.written), plural(found, "rule")))
            ));
            out
        };
        let mut finished = 0usize;
        while finished < jobs.len() {
            match receiver.recv_timeout(Duration::from_millis(120)) {
                Ok(JobEvent::Started(index)) => {
                    states[index] = JobState::Running { started: Instant::now(), action: format!("Reading {}", plural(jobs[index].messages, "message")) };
                    if region.is_none() {
                        ui::line(&format!("{}: reading", jobs[index].label));
                    }
                }
                Ok(JobEvent::Action(index, action)) => {
                    if let JobState::Running { action: current, .. } = &mut states[index] {
                        *current = action;
                    }
                }
                Ok(JobEvent::Tokens(index, attempt, tokens)) => spent[index][attempt] = tokens,
                Ok(JobEvent::Done(index, attempt, rules, tokens)) => {
                    spent[index][attempt] = tokens;
                    let started_at = match &states[index] {
                        JobState::Running { started, .. } => *started,
                        _ => Instant::now(),
                    };
                    if region.is_none() {
                        ui::line(&format!("{}: {}", jobs[index].label, plural(rules.len(), "rule")));
                    }
                    states[index] = JobState::Done { started: started_at, ended: Instant::now(), rules: rules.len(), tokens };
                    let known: Vec<&str> = jobs[index].stretches.iter().map(|stretch| stretch.repo.as_str()).collect();
                    for mut rule in rules {
                        if !known.contains(&rule.repo.as_str()) {
                            rule.repo = known[0].to_string();
                        }
                        candidates.push(Candidate { id: format!("r{}", candidates.len() + 1), rule });
                    }
                    finished += 1;
                }
                Ok(JobEvent::Failed(index, error)) => {
                    let started_at = match &states[index] {
                        JobState::Running { started, .. } => *started,
                        _ => Instant::now(),
                    };
                    if region.is_none() {
                        ui::line(&format!("{}: failed: {error}", jobs[index].label));
                    }
                    states[index] = JobState::Failed { started: started_at, ended: Instant::now(), error };
                    finished += 1;
                }
                Err(_) => {}
            }
            frame += 1;
            if let Some(region) = region.as_mut() {
                region.update(&lines(&states, &spent, candidates.len(), frame / 2, false));
            }
        }
        if let Some(region) = region.take() {
            region.finish(&lines(&states, &spent, candidates.len(), 0, true));
        }
    });
    let tokens = spent.iter().flatten().fold(Tokens::default(), |total, tokens| total + *tokens);
    let failed = states.iter().filter(|state| matches!(state, JobState::Failed { .. })).count();
    (candidates, tokens, failed)
}

fn plan_merge(candidates: &[Candidate], directory: &Path, model: Option<&str>, spinner: &ui::Spinner) -> Result<(Vec<Planned>, Tokens), String> {
    let reports: Vec<Value> = candidates
        .iter()
        .map(|candidate| {
            json!({
                "id": candidate.id, "repo": candidate.rule.repo, "name": candidate.rule.name, "instruction": candidate.rule.instruction,
                "timesSeen": candidate.rule.times_seen, "stated": candidate.rule.stated, "general": candidate.rule.general, "lintable": candidate.rule.lintable,
            })
        })
        .collect();
    let prompt = fill(MERGE, &[("REPORTS", &serde_json::to_string_pretty(&reports).unwrap_or_default())]);
    let outcome = agent::run(
        &Request {
            prompt: &prompt,
            cwd: directory,
            schema: &merge_schema(),
            tools: &[],
            allowed_tools: &[],
            add_dirs: &[],
            model,
            timeout: Some(Duration::from_secs(600)),
            live: true,
        },
        &mut |activity| {
            if let Activity::Answering(names) = activity {
                spinner.message(&format!("Merging into Reviewers · {}: {}", names.len(), names.last().cloned().unwrap_or_default()));
            }
        },
    )?;
    let planned: Vec<Planned> = serde_json::from_value(outcome.output["reviewers"].clone()).map_err(|error| format!("malformed plan: {error}"))?;
    Ok((planned, outcome.tokens))
}

/// When the merge can't run, every report stands alone rather than the whole run being lost.
fn unmerged(candidates: &[Candidate]) -> Vec<Planned> {
    candidates
        .iter()
        .map(|candidate| Planned {
            name: candidate.rule.name.clone(),
            scope: if candidate.rule.general { "everywhere".into() } else { "project".into() },
            sources: vec![candidate.id.clone()],
            instruction_from: candidate.id.clone(),
        })
        .collect()
}

/// Round-robin over repos, so an everywhere rule shows where it came from, not four quotes from one place.
fn pick_evidence(sources: &[&Candidate]) -> Vec<Value> {
    let mut by_repo: Vec<(String, Vec<&RuleEvidence>)> = Vec::new();
    for source in sources {
        let index = match by_repo.iter().position(|(repo, _)| *repo == source.rule.repo) {
            Some(index) => index,
            None => {
                by_repo.push((source.rule.repo.clone(), Vec::new()));
                by_repo.len() - 1
            }
        };
        for item in &source.rule.evidence {
            if !by_repo[index].1.iter().any(|existing| existing.quote == item.quote) {
                by_repo[index].1.push(item);
            }
        }
    }
    let mut picked: Vec<(String, &RuleEvidence)> = Vec::new();
    for round in 0.. {
        let layer: Vec<(String, &RuleEvidence)> = by_repo.iter().filter_map(|(repo, items)| items.get(round).map(|item| (repo.clone(), *item))).collect();
        if layer.is_empty() || picked.len() >= EVIDENCE_PER_REVIEWER {
            break;
        }
        picked.extend(layer.into_iter().take(EVIDENCE_PER_REVIEWER - picked.len()));
    }
    picked.sort_by(|a, b| a.1.date.cmp(&b.1.date));
    picked.into_iter().map(|(repo, item)| json!({ "repo": repo, "date": item.date, "quote": item.quote })).collect()
}

/// The merge only groups. Everything decided by counting is decided here: scope
/// from how many repos a rule came from, the bar (seen twice, or stated as a
/// standing rule), the order, and the cap.
fn assemble(planned: &[Planned], candidates: &[Candidate], max: Option<usize>) -> (Vec<Suggestion>, usize) {
    let mut all: Vec<(Suggestion, bool)> = Vec::new();
    for plan in planned {
        let mut sources: Vec<&Candidate> = Vec::new();
        for id in &plan.sources {
            if let Some(candidate) = candidates.iter().find(|candidate| &candidate.id == id) {
                if !sources.iter().any(|source| source.id == candidate.id) {
                    sources.push(candidate);
                }
            }
        }
        let Some(base) = candidates.iter().find(|candidate| candidate.id == plan.instruction_from).or(sources.first().copied()) else {
            continue;
        };
        let mut repos: Vec<String> = sources.iter().map(|source| source.rule.repo.clone()).collect();
        repos.sort();
        repos.dedup();
        let everywhere = plan.scope == "everywhere" || repos.len() > 1;
        all.push((
            Suggestion {
                name: plan.name.clone(),
                everywhere,
                paths: if everywhere { Vec::new() } else { base.rule.paths.clone() },
                repos,
                instruction: base.rule.instruction.trim().to_string(),
                why: base.rule.why.clone(),
                times_seen: sources.iter().map(|source| source.rule.times_seen).sum(),
                lintable: base.rule.lintable,
                lint_rule: base.rule.lint_rule.clone(),
                evidence: pick_evidence(&sources),
            },
            sources.iter().any(|source| source.rule.stated),
        ));
    }
    let total = all.len();
    let mut kept: Vec<Suggestion> = all.into_iter().filter(|(suggestion, stated)| suggestion.times_seen >= 2 || *stated).map(|(suggestion, _)| suggestion).collect();
    kept.sort_by(|a, b| b.times_seen.cmp(&a.times_seen).then(b.evidence.len().cmp(&a.evidence.len())));
    let thin = total - kept.len();
    let (mut rules, lint): (Vec<Suggestion>, Vec<Suggestion>) = kept.into_iter().partition(|suggestion| !suggestion.lintable);
    if let Some(max) = max {
        rules.truncate(max);
    }
    rules.extend(lint);
    (rules, thin)
}

fn suggestion_file(suggestion: &Suggestion) -> String {
    let mut lines = vec!["---".to_string(), format!("name: {}", json!(suggestion.name))];
    lines.push(if suggestion.everywhere { "scope: everywhere".into() } else { format!("repos: {}", json!(suggestion.repos)) });
    if !suggestion.paths.is_empty() {
        lines.push(format!("paths: {}", json!(suggestion.paths)));
    }
    lines.extend(["---".into(), String::new(), suggestion.instruction.clone(), String::new(), "## Why".into(), String::new(), suggestion.why.clone()]);
    if suggestion.lintable {
        lines.push(format!("\nA linter could enforce this instead: {}", suggestion.lint_rule));
    }
    lines.extend([String::new(), "## Evidence".into(), String::new()]);
    for item in &suggestion.evidence {
        lines.push(format!("- {} · {}: {}", str_field(item, "repo").unwrap_or_default(), str_field(item, "date").unwrap_or_default(), str_field(item, "quote").unwrap_or_default()));
    }
    lines.push(String::new());
    lines.join("\n")
}

fn write_outputs(directory: &Path, candidates: &[Candidate], planned: &[Planned], suggestions: &[Suggestion]) -> Result<PathBuf, String> {
    let write = |name: &str, value: &dyn erased::Json| std::fs::write(directory.join(name), value.pretty()).map_err(|error| error.to_string());
    write("candidates.json", &candidates)?;
    write("plan.json", &planned)?;
    write("reviewers.json", &suggestions)?;
    let root = directory.join("reviewers");
    for suggestion in suggestions {
        let group = if suggestion.lintable {
            "lint".to_string()
        } else if suggestion.everywhere {
            "everywhere".to_string()
        } else {
            suggestion.repos.first().cloned().unwrap_or_else(|| "repo".into())
        };
        let folder = root.join(group);
        std::fs::create_dir_all(&folder).map_err(|error| error.to_string())?;
        std::fs::write(folder.join(format!("{}.md", slugify(&suggestion.name))), suggestion_file(suggestion)).map_err(|error| error.to_string())?;
    }
    Ok(root)
}

mod erased {
    pub trait Json {
        fn pretty(&self) -> String;
    }
    impl<T: serde::Serialize> Json for T {
        fn pretty(&self) -> String {
            serde_json::to_string_pretty(self).unwrap_or_default()
        }
    }
}

fn first_sentence(text: &str) -> String {
    let trimmed = text.trim();
    match trimmed.find(". ") {
        Some(end) => trimmed[..=end].to_string(),
        None => trimmed.to_string(),
    }
}

fn print_suggestions(suggestions: &[Suggestion]) {
    let (rules, lint): (Vec<&Suggestion>, Vec<&Suggestion>) = suggestions.iter().partition(|suggestion| !suggestion.lintable);
    let width = rules.iter().map(|suggestion| suggestion.name.chars().count()).max().unwrap_or(0).min(42) + 2;
    let detailed = rules.len() <= DETAILED_LIST_LIMIT;
    let row = |suggestion: &Suggestion| {
        let place = if suggestion.everywhere { suggestion.repos.join(", ") } else { suggestion.paths.join(", ") };
        let gap = " ".repeat(width.saturating_sub(suggestion.name.chars().count()).max(1));
        ui::line(&format!("{} {}{gap}{}  {}", ui::green("●"), ui::bold(&suggestion.name), ui::dim(&format!("{:>4}", format!("{}×", suggestion.times_seen))), ui::dim(&place)));
        if detailed {
            ui::line(&format!("  {}", ui::dim(&first_sentence(&suggestion.instruction))));
        }
    };
    ui::line("");
    println!("{}  {}", ui::green(ui::STEP_DONE), ui::bold(&plural(rules.len(), "Reviewer")));
    let everywhere: Vec<&&Suggestion> = rules.iter().filter(|suggestion| suggestion.everywhere).collect();
    if !everywhere.is_empty() {
        ui::line("");
        ui::line(&format!("{}  {}", ui::bold("Everywhere"), ui::dim("your rules, in every repo")));
        everywhere.iter().for_each(|suggestion| row(suggestion));
    }
    let mut repos: Vec<&str> = rules.iter().filter(|suggestion| !suggestion.everywhere).filter_map(|suggestion| suggestion.repos.first().map(String::as_str)).collect();
    repos.dedup();
    let mut seen: Vec<&str> = Vec::new();
    for repo in repos {
        if seen.contains(&repo) {
            continue;
        }
        seen.push(repo);
        ui::line("");
        ui::line(&ui::bold(repo));
        rules.iter().filter(|suggestion| !suggestion.everywhere && suggestion.repos.first().map(String::as_str) == Some(repo)).for_each(|suggestion| row(suggestion));
    }
    if !lint.is_empty() {
        ui::line("");
        ui::line(&format!("{}  {}", ui::bold("Better as lint"), ui::dim("cheaper and instant")));
        for suggestion in lint {
            ui::line(&format!("{} {}  {}", ui::yellow("○"), suggestion.name, ui::dim(&suggestion.lint_rule)));
        }
    }
}

fn pick_repos(summaries: &[RepoSummary]) -> Option<Vec<usize>> {
    let width = |value: usize| thousands(value as u64).len();
    let widths = (
        summaries.iter().map(|repo| width(repo.messages.len())).max().unwrap_or(1),
        summaries.iter().map(|repo| width(repo.corrections)).max().unwrap_or(1),
        summaries.iter().map(|repo| width(repo.commits.len())).max().unwrap_or(1),
    );
    let rows: Vec<ui::PickRow> = summaries
        .iter()
        .map(|repo| ui::PickRow {
            label: repo.name.clone(),
            detail: format!(
                "{:>a$} messages · {:>b$} corrections · {:>c$} commits",
                thousands(repo.messages.len() as u64),
                thousands(repo.corrections as u64),
                thousands(repo.commits.len() as u64),
                a = widths.0,
                b = widths.1,
                c = widths.2
            ),
            selected: true,
        })
        .collect();
    ui::multiselect("Which repos should Reviewers learn from?", &rows, "repo", 1)
}

/// Saves the suggestions as Reviewers, the strong ones on, and puts the hook where they'll run.
fn activate(store: &Store, suggestions: &[Suggestion], chosen: &[&RepoSummary], interactive: bool, yes: bool, run_directory: &Path) -> Result<(usize, usize, usize), String> {
    let existing: Vec<String> = store.reviewers()?.iter().map(|reviewer| reviewer.name.to_lowercase()).collect();
    let fresh: Vec<&Suggestion> = suggestions.iter().filter(|suggestion| !suggestion.lintable && !existing.contains(&suggestion.name.to_lowercase())).collect();
    let picked: Vec<bool> = if interactive && !fresh.is_empty() {
        let rows: Vec<ui::PickRow> = fresh
            .iter()
            .enumerate()
            .map(|(rank, suggestion)| ui::PickRow {
                label: suggestion.name.clone(),
                detail: format!("{:>4}  {}", format!("{}×", suggestion.times_seen), if suggestion.everywhere { "everywhere".to_string() } else { suggestion.repos.join(", ") }),
                selected: rank < STARTING_REVIEWERS,
            })
            .collect();
        match ui::multiselect("Which Reviewers should run on your commits?", &rows, "Reviewer", 0) {
            Some(indexes) => (0..fresh.len()).map(|index| indexes.contains(&index)).collect(),
            None => return Err("cancelled; the suggestions are kept in the run folder".into()),
        }
    } else {
        (0..fresh.len()).map(|rank| rank < STARTING_REVIEWERS).collect()
    };
    let mut projects = Vec::new();
    for repo in chosen {
        let root = git::main_checkout(&repo.root);
        let project = store.ensure_project(&root.display().to_string(), git::remote_url(&root).as_deref())?;
        projects.push((repo.name.clone(), project));
    }
    let mut on = 0;
    for (suggestion, enabled) in fresh.iter().zip(&picked) {
        let project_ids: Vec<String> = if suggestion.everywhere {
            Vec::new()
        } else {
            projects.iter().filter(|(name, _)| suggestion.repos.contains(name)).map(|(_, project)| project.id.clone()).collect()
        };
        if !suggestion.everywhere && project_ids.is_empty() {
            continue;
        }
        store.create_reviewer(NewReviewer {
            name: suggestion.name.clone(),
            instruction: suggestion.instruction.clone(),
            scope: if suggestion.everywhere { Scope::Everywhere } else { Scope::Projects },
            project_ids,
            paths: suggestion.paths.clone(),
            context_files: Vec::new(),
            enabled: *enabled,
            blocking: true,
            model: None,
            classifier: ClassifierUse::Default,
            origin: json!({ "kind": "onboard", "why": suggestion.why, "timesSeen": suggestion.times_seen, "evidence": suggestion.evidence, "run": run_directory }),
        })?;
        on += usize::from(*enabled);
    }
    let install = if interactive {
        ui::confirm("Run Reviewers on every commit, in every repo? (git's global hooks; each repo's own hooks still run)", true).unwrap_or(false)
    } else {
        yes
    };
    let mut hooked = 0;
    if install {
        // Every repo, new ones included, with no setup; a repo with its own hooks folder gets the hooks there.
        if let Err(error) = hooks::install_global(store) {
            ui::line(&format!("{} {}", ui::red("✗"), ui::dim(&format!("global hooks: {error}"))));
        }
        for (name, project) in &projects {
            match hooks::cover(Path::new(&project.root)) {
                Ok(hooks::Coverage::Global) => hooked += 1,
                Ok(hooks::Coverage::Repo(states)) => {
                    let ours = states.iter().all(|(_, state)| matches!(state, hooks::HookState::Installed | hooks::HookState::Updated));
                    hooked += usize::from(ours);
                    if !ours {
                        let reason = states.iter().find(|(_, state)| *state == hooks::HookState::Foreign).map(|(_, state)| hooks::describe(*state)).unwrap_or("");
                        ui::line(&format!("{} {name} {}", ui::yellow("!"), ui::dim(reason)));
                    }
                }
                Err(error) => ui::line(&format!("{} {name} {}", ui::red("✗"), ui::dim(&error))),
            }
        }
    }
    Ok((on, fresh.len() - on, hooked))
}

pub fn run(args: OnboardArgs) -> Outcome {
    let interactive = ui::interactive() && !args.yes;
    let live = ui::stdout_is_tty();
    let stamp = crate::util::now_iso()[..16].replace([':', 'T'], "-");
    let directory = crate::util::data_dir().join("onboard").join(stamp);
    std::fs::create_dir_all(&directory).map_err(|error| format!("cannot create {}: {error}", directory.display()))?;
    let directory = directory.canonicalize().unwrap_or(directory);
    let started = Instant::now();
    let stopped_in = directory.clone();
    let _ = ctrlc::set_handler(move || {
        agent::stop_all();
        ui::restore_cursor();
        println!("\n{}  Stopped. What was read so far is in {}\n", ui::red(ui::STEP_CANCEL), home_path(&stopped_in));
        std::process::exit(130);
    });

    ui::intro(&format!("{} {}", ui::bold(&format!("{} reviewers", ui::LOGO)), ui::dim("· first run")));
    let spinner = ui::Spinner::start("Reading your agent sessions");
    let progress = |done: usize, total: usize| {
        if done % 50 == 0 || done == total {
            spinner.message(&format!("Reading your agent sessions · {} of {}", thousands(done as u64), thousands(total as u64)));
        }
    };
    let scan = transcripts::scan(args.since, &progress);
    spinner.message("Collecting commits");
    let summaries = digest::summarize(scan.repos, args.since);
    spinner.stop(&format!("Read {} of yours from the last {} days", plural(scan.sessions_read - scan.sessions_skipped, "session"), args.since));
    ui::line(&ui::dim(&format!("{} · {} scripted sessions skipped", plural(summaries.len(), "repo"), thousands(scan.sessions_skipped as u64))));
    if summaries.is_empty() {
        ui::outro("No repos found in your agent sessions.");
        return Ok(0);
    }

    let chosen: Vec<&RepoSummary> = if let Some(names) = &args.repos {
        let wanted = crate::scope::parse_list(names);
        summaries.iter().filter(|repo| wanted.contains(&repo.name)).collect()
    } else if interactive {
        match pick_repos(&summaries) {
            Some(indexes) => indexes.iter().map(|&index| &summaries[index]).collect(),
            None => {
                ui::cancelled("Nothing was sent to an agent.");
                return Ok(130);
            }
        }
    } else {
        summaries.iter().collect()
    };
    if chosen.is_empty() {
        ui::outro("No repos picked.");
        return Ok(0);
    }

    let claude_installed = agent::is_installed(Provider::Claude);
    let codex_installed = agent::is_installed(Provider::Codex);
    let choices = model_choices(&scan.models, claude_installed || args.dry_run, codex_installed || args.dry_run);
    let model: Option<String> = match &args.model {
        Some(model) => Some(model.clone()),
        None if interactive && !choices.is_empty() => {
            let labels: Vec<ui::Choice> = choices.iter().map(|(_, choice)| ui::Choice { label: choice.label.clone(), hint: choice.hint.clone() }).collect();
            match ui::select("Which model should read them?", &labels, 0) {
                Some(index) => Some(choices[index].0.clone()),
                None => {
                    ui::cancelled("Nothing was sent to an agent.");
                    return Ok(130);
                }
            }
        }
        None => choices.first().map(|(id, _)| id.clone()),
    };
    if !interactive {
        ui::step(&format!("{} · {}", plural(chosen.len(), "repo"), model.as_deref().map(model_label).unwrap_or_else(|| "your default model".into())));
    }

    let jobs = digest::write_jobs(&directory, &chosen, args.since, args.parallel.max(1))?;
    if args.dry_run {
        ui::outro(&format!("Material for {} written to {}", plural(jobs.len(), "agent"), home_path(&directory)));
        return Ok(0);
    }
    let provider = agent::selection(model.as_deref()).0;
    if !match provider { Provider::Claude => claude_installed, Provider::Codex => codex_installed } {
        let cli = match provider { Provider::Claude => "claude", Provider::Codex => "codex" };
        return Err(format!("onboarding needs `{cli}` on PATH for this model; install it and sign in, or choose another --model"));
    }

    let (candidates, extract_tokens, failed) = extract(&jobs, args.parallel.max(1), model.as_deref(), live);
    if candidates.is_empty() {
        ui::outro(if failed > 0 { "Every agent failed; nothing to merge." } else { "No rules found in these sessions." });
        return Ok(if failed > 0 { 1 } else { 0 });
    }

    let spinner = ui::Spinner::start(&format!("Merging {} from {}", plural(candidates.len(), "rule"), plural(jobs.len(), "agent")));
    let (planned, merge_tokens, merged) = match plan_merge(&candidates, &directory, model.as_deref(), &spinner) {
        Ok((planned, tokens)) => (planned, tokens, true),
        Err(error) => {
            spinner.message(&format!("Merge failed ({error}); each rule stands alone"));
            (unmerged(&candidates), Tokens::default(), false)
        }
    };
    let (suggestions, thin) = assemble(&planned, &candidates, args.max);
    let kept = suggestions.iter().filter(|suggestion| !suggestion.lintable).count();
    let left_out = if thin > 0 { ui::dim(&format!(" · {thin} seen only once left out")) } else { String::new() };
    if merged {
        spinner.stop(&format!("Merged {} into {}{left_out}", plural(candidates.len(), "rule"), plural(kept, "Reviewer")));
    } else {
        spinner.fail(&format!("Couldn't merge; {} kept as found{left_out}", plural(kept, "rule")));
    }
    write_outputs(&directory, &candidates, &planned, &suggestions)?;
    print_suggestions(&suggestions);

    let store = Store::open_default()?;
    let (on, off, hooked) = activate(&store, &suggestions, &chosen, interactive, args.yes, &directory)?;
    // A Codex-only installation must not silently switch to Claude when its
    // first commit is reviewed. A Claude pick only chose who reads transcripts.
    if provider == Provider::Codex && store.setting("default_model")?.is_none() {
        store.set_setting("default_model", model.as_deref())?;
    }
    let skill_agents = if args.no_skill { 0 } else { skill::install().map(|placements| placements.iter().filter(|placement| placement.state == "linked").count()).unwrap_or(0) };
    let tokens = extract_tokens + merge_tokens;
    ui::line("");
    ui::line(&format!(
        "{} on · {} off · hook in {} · skill for {}",
        plural(on, "Reviewer"),
        off,
        plural(hooked, "repo"),
        plural(skill_agents, "agent")
    ));
    if off > 0 {
        ui::line(&ui::dim("`reviewers list --all` shows them; `reviewers enable <name>` turns one on."));
    }
    if crate::classifier::connected().is_none() {
        ui::line(&ui::dim("Optional: with a Vercel AI Gateway or TypeSafe key, `reviewers classifier connect` skips the Reviewers a commit can't concern."));
    }
    ui::outro(&format!(
        "Saved to {}  {}",
        home_path(&directory),
        ui::dim(&format!("{} · {} tokens read · {} written", duration(started.elapsed().as_millis() as u64), compact(tokens.read), compact(tokens.written)))
    ));
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Explicit opt-in: uses the signed-in Codex CLI and consumes model tokens.
    #[test]
    #[ignore = "requires an authenticated recent Codex CLI"]
    fn live_codex_accepts_review_extract_and_merge_schemas() {
        let directory = std::env::temp_dir();
        for (schema, prompt, key) in [
            (crate::review::prompt::decision_schema(), "Return an approved verdict with summary and reasoning saying there is no diff to review. Evidence is empty. Do not use tools.", "verdict"),
            (extract_schema(), "There are no user messages. Return an empty rules array. Do not use tools.", "rules"),
            (merge_schema(), "There are no candidates. Return an empty reviewers array. Do not use tools.", "reviewers"),
        ] {
            let outcome = agent::run(&Request {
                prompt, cwd: &directory, schema: &schema, tools: &[], allowed_tools: &[], add_dirs: &[],
                model: Some("codex"), timeout: Some(Duration::from_secs(90)), live: false,
            }, &mut |_| {}).unwrap_or_else(|error| panic!("{key}: {error}"));
            assert!(outcome.output.get(key).is_some(), "{key}: {}", outcome.output);
        }
        let directory = std::env::temp_dir().join(crate::util::new_id("reviewers-live-codex"));
        std::fs::create_dir(&directory).unwrap();
        let marker = crate::util::new_id("marker");
        std::fs::write(directory.join("marker.txt"), &marker).unwrap();
        let outcome = agent::run(&Request {
            prompt: "Read marker.txt using a read-only shell command. Return approved, put its exact contents in summary, and explain in reasoning that you read the file. Do not modify anything.",
            cwd: &directory, schema: &crate::review::prompt::decision_schema(), tools: &["Read", "Grep", "Glob"], allowed_tools: &["Read", "Grep", "Glob"], add_dirs: &[],
            model: Some("codex"), timeout: Some(Duration::from_secs(90)), live: false,
        }, &mut |_| {});
        let unchanged = std::fs::read_to_string(directory.join("marker.txt")).unwrap();
        std::fs::remove_dir_all(&directory).unwrap();
        let outcome = outcome.unwrap();
        assert_eq!(outcome.output["summary"], marker);
        assert_eq!(unchanged, marker);
        assert!(outcome.tool_calls > 0);
    }

    #[test]
    fn model_picker_only_offers_installed_providers() {
        assert_eq!(OnboardArgs::default().since, 90);
        assert_eq!(OnboardArgs::default().parallel, 8);
        let usage = vec![
            transcripts::ModelUsage { id: "claude-test".into(), sessions: 5, recent_sessions: 5 },
            transcripts::ModelUsage { id: "codex:test-model".into(), sessions: 3, recent_sessions: 3 },
        ];
        let codex = model_choices(&usage, false, true);
        assert_eq!(codex.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(), ["codex:test-model", "codex"]);
        assert!(model_choices(&usage, true, false).iter().all(|(id, _)| !id.starts_with("codex")));
        assert!(model_choices(&usage, false, false).is_empty());
    }
}
