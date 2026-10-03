use super::transcripts::{HumanMessage, RepoTranscripts};
use crate::git;
use regex::Regex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// An agent's time goes into writing rules, so the wait is the agent with the
/// most to read. Shares aim to fill every parallel slot evenly, between these
/// sizes: below the floor an agent's startup outweighs its reading; above the
/// ceiling (about 20k tokens) one agent holds up the rest.
const MIN_SHARE_BYTES: usize = 16 * 1024;
const MAX_SHARE_BYTES: usize = 80 * 1024;
const RULE_FILE_BYTES: usize = 12 * 1024;

const RULE_FILES: [&str; 12] = [
    "AGENTS.md",
    "CLAUDE.md",
    ".cursorrules",
    ".github/copilot-instructions.md",
    "eslint.config.mjs",
    "eslint.config.js",
    "eslint.config.ts",
    ".eslintrc.json",
    ".eslintrc.js",
    "biome.json",
    "biome.jsonc",
    ".oxlintrc.json",
];

pub struct RepoSummary {
    pub name: String,
    pub root: PathBuf,
    /// Where reading started, as an ISO timestamp.
    pub since: String,
    pub messages: Vec<HumanMessage>,
    /// Messages that look like corrections; only shown in the picker, agents read everything.
    pub corrections: usize,
    pub commits: Vec<String>,
    pub rule_files: Vec<String>,
}

#[derive(Clone)]
pub struct Stretch {
    pub repo: String,
    pub root: PathBuf,
    pub part: usize,
    pub parts: usize,
    pub from: String,
    pub to: String,
    pub messages: Vec<HumanMessage>,
    pub commits: Vec<String>,
    pub rule_files: Vec<String>,
    pub bytes: usize,
}

/// One agent's share: a stretch of a big repo, or several small repos together.
pub struct Job {
    pub label: String,
    pub stretches: Vec<Stretch>,
    pub directory: PathBuf,
    pub messages: usize,
}

fn correction() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(r"(?i)\b(don'?t|do not|never|always|stop|instead|wrong|again|should(?:n'?t)?|must|hate|i (?:said|told you|asked)|why (?:did|do|are|is) (?:you|it|this)|no,|not like that|bro|dude|nunca|sempre|não|nao|pare|errado|de novo|por que|em vez)\b")
            .expect("the pattern compiles")
    })
}

fn is_correction(message: &HumanMessage) -> bool {
    message.text.contains("[on the agent's words") || correction().is_match(&message.text)
}

pub fn short_date(iso_date: &str) -> String {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let mut parts = iso_date.split('-');
    let _year = parts.next();
    let month: usize = parts.next().and_then(|month| month.parse().ok()).unwrap_or(1);
    let day: usize = parts.next().and_then(|day| day.get(..2)).and_then(|day| day.parse().ok()).unwrap_or(1);
    format!("{} {day}", MONTHS[(month.clamp(1, 12)) - 1])
}

fn rule_files(root: &Path) -> Vec<String> {
    let mut found: Vec<String> = RULE_FILES.iter().filter(|file| root.join(file).exists()).map(|file| file.to_string()).collect();
    // Nested AGENTS.md / CLAUDE.md one level down are common in monorepos.
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !entry.path().is_dir() || name.starts_with('.') || name == "node_modules" {
                continue;
            }
            for file in ["AGENTS.md", "CLAUDE.md"] {
                if entry.path().join(file).exists() {
                    found.push(format!("{name}/{file}"));
                }
            }
        }
    }
    found
}

/// `abc1234 2026-09-02 subject`, newest first.
fn commit_log(root: &Path, since: &str) -> Vec<String> {
    let since = format!("--since={since}");
    let output = git::run(root, &["log", &since, "--no-merges", "--date=short", "--format=%h %ad %s"], &[]);
    if output.ok { output.stdout.lines().map(str::to_string).collect() } else { Vec::new() }
}

/// Two repos can share a folder name (`a/web`, `b/web`); the parent breaks the tie.
pub fn summarize(repos: Vec<RepoTranscripts>) -> Vec<RepoSummary> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for repo in &repos {
        *counts.entry(repo.name.clone()).or_insert(0) += 1;
    }
    repos
        .into_iter()
        .map(|repo| {
            let name = if counts.get(&repo.name).copied().unwrap_or(0) > 1 {
                let parent = repo.root.parent().and_then(|parent| parent.file_name()).map(|name| name.to_string_lossy().to_string()).unwrap_or_default();
                format!("{parent}-{}", repo.name)
            } else {
                repo.name.clone()
            };
            RepoSummary {
                corrections: repo.messages.iter().filter(|message| is_correction(message)).count(),
                commits: commit_log(&repo.root, &repo.since),
                rule_files: rule_files(&repo.root),
                name,
                root: repo.root,
                since: repo.since,
                messages: repo.messages,
            }
        })
        .collect()
}

fn message_bytes(messages: &[HumanMessage]) -> usize {
    messages.iter().map(|message| message.text.len()).sum()
}

/// Cuts a repo into stretches of time that each fit one agent.
fn stretches_of(repo: &RepoSummary, share: usize) -> Vec<Stretch> {
    let total = message_bytes(&repo.messages);
    let parts = total.div_ceil(share).max(1);
    let target = total as f64 / parts as f64;
    let mut cuts: Vec<Vec<HumanMessage>> = vec![Vec::new()];
    let mut seen = 0usize;
    for message in &repo.messages {
        if seen as f64 >= target * cuts.len() as f64 && cuts.len() < parts {
            cuts.push(Vec::new());
        }
        cuts.last_mut().expect("there is always a cut").push(message.clone());
        seen += message.text.len();
    }
    let today = crate::util::now_iso()[..10].to_string();
    let count = cuts.len();
    (0..count)
        .map(|index| {
            let from = if index == 0 { repo.since[..10].to_string() } else { cuts[index].first().map(|message| message.at[..10].to_string()).unwrap_or_default() };
            let next = cuts.get(index + 1).and_then(|cut| cut.first()).map(|message| message.at[..10].to_string());
            let commits = repo
                .commits
                .iter()
                .filter(|line| {
                    let date = line.split(' ').nth(1).unwrap_or_default();
                    date >= from.as_str() && next.as_deref().is_none_or(|next| date < next)
                })
                .cloned()
                .collect();
            Stretch {
                repo: repo.name.clone(),
                root: repo.root.clone(),
                part: index + 1,
                parts: count,
                from,
                to: next.unwrap_or_else(|| today.clone()),
                bytes: message_bytes(&cuts[index]),
                messages: cuts[index].clone(),
                commits,
                rule_files: repo.rule_files.clone(),
            }
        })
        .collect()
}

fn pack(mut stretches: Vec<Stretch>, share: usize) -> Vec<Vec<Stretch>> {
    stretches.sort_by(|a, b| b.bytes.cmp(&a.bytes));
    let mut bins: Vec<Vec<Stretch>> = Vec::new();
    for stretch in stretches {
        match bins.iter_mut().find(|bin| bin.iter().map(|item| item.bytes).sum::<usize>() + stretch.bytes <= share) {
            Some(bin) => bin.push(stretch),
            None => bins.push(vec![stretch]),
        }
    }
    bins
}

fn label(stretches: &[Stretch]) -> String {
    let names: Vec<String> = stretches
        .iter()
        .map(|stretch| if stretch.parts > 1 { format!("{} {}–{}", stretch.repo, short_date(&stretch.from), short_date(&stretch.to)) } else { stretch.repo.clone() })
        .collect();
    if names.len() > 3 { format!("{} +{} more", names[..2].join(", "), names.len() - 2) } else { names.join(", ") }
}

fn render_messages(messages: &[HumanMessage]) -> String {
    messages.iter().map(|message| format!("### {}\n\n{}\n", message.at.get(..16).unwrap_or(&message.at).replace('T', " "), message.text)).collect::<Vec<_>>().join("\n")
}

/// First-fit decreasing: big stretches each take an agent and small repos
/// share one. Bins never pack perfectly, so the share grows until the run is
/// a single wave; a second wave would double the wait.
pub fn write_jobs(directory: &Path, repos: &[&RepoSummary], agents: usize) -> Result<Vec<Job>, String> {
    let total: usize = repos.iter().map(|repo| message_bytes(&repo.messages)).sum();
    let mut share = total.div_ceil(agents.max(1)).clamp(MIN_SHARE_BYTES, MAX_SHARE_BYTES);
    let stretches = |share: usize| repos.iter().flat_map(|repo| stretches_of(repo, share)).collect::<Vec<_>>();
    let mut bins = pack(stretches(share), share);
    while bins.len() > agents && share < MAX_SHARE_BYTES {
        share = ((share as f64 * 1.1).ceil() as usize).min(MAX_SHARE_BYTES);
        bins = pack(stretches(share), share);
    }
    let mut jobs = Vec::new();
    for (index, bin) in bins.into_iter().enumerate() {
        let job_directory = directory.join("jobs").join(format!("agent-{}", index + 1));
        for stretch in &bin {
            let folder = job_directory.join(if stretch.parts > 1 { format!("{}-part-{}", stretch.repo, stretch.part) } else { stretch.repo.clone() });
            std::fs::create_dir_all(&folder).map_err(|error| error.to_string())?;
            let header = format!("# {}: every message, {} to {} ({})\n\n", stretch.repo, stretch.from, stretch.to, stretch.messages.len());
            std::fs::write(folder.join("messages.md"), header + &render_messages(&stretch.messages)).map_err(|error| error.to_string())?;
        }
        jobs.push(Job { label: label(&bin), messages: bin.iter().map(|stretch| stretch.messages.len()).sum(), stretches: bin, directory: job_directory });
    }
    Ok(jobs)
}

fn read_rule_file(root: &Path, file: &str) -> String {
    let text = std::fs::read_to_string(root.join(file)).unwrap_or_default();
    if text.len() <= RULE_FILE_BYTES {
        return text;
    }
    let cut = (0..=RULE_FILE_BYTES).rev().find(|&index| text.is_char_boundary(index)).unwrap_or(0);
    format!("{}\n[… cut at {} KB; open the file for the rest]", &text[..cut], RULE_FILE_BYTES / 1024)
}

/// Everything an agent needs, inline, so it can answer without opening files one call at a time.
pub fn material(job: &Job) -> String {
    job.stretches
        .iter()
        .map(|stretch| {
            let part = if stretch.parts > 1 { format!(", part {} of {}", stretch.part, stretch.parts) } else { String::new() };
            let files = stretch
                .rule_files
                .iter()
                .map(|file| format!("#### {file}\n\n```\n{}\n```", read_rule_file(&stretch.root, file)))
                .collect::<Vec<_>>()
                .join("\n\n");
            [
                format!("## Repo: {}{part}", stretch.repo),
                String::new(),
                format!("Path: `{}`. From {} to {}.", stretch.root.display(), stretch.from, stretch.to),
                String::new(),
                "### Rule files".into(),
                String::new(),
                if files.is_empty() { "None.".into() } else { files },
                String::new(),
                format!("### Commits ({})", stretch.commits.len()),
                String::new(),
                if stretch.commits.is_empty() { "None in this stretch.".into() } else { stretch.commits.join("\n") },
                String::new(),
                format!("### Every message they typed ({})", stretch.messages.len()),
                String::new(),
                render_messages(&stretch.messages),
            ]
            .join("\n")
        })
        .collect::<Vec<_>>()
        .join("\n\n---\n\n")
}

pub fn scope_sentence(job: &Job) -> String {
    match job.stretches.as_slice() {
        [only] if only.parts > 1 => format!(
            "one stretch of **{}**, from {} to {} (part {} of {})",
            only.repo,
            short_date(&only.from),
            short_date(&only.to),
            only.part,
            only.parts
        ),
        [only] => format!("the repo **{}**", only.repo),
        many => format!(
            "{} pieces: {}",
            many.len(),
            many.iter()
                .map(|stretch| if stretch.parts > 1 {
                    format!("**{}** from {} to {}", stretch.repo, short_date(&stretch.from), short_date(&stretch.to))
                } else {
                    format!("**{}**", stretch.repo)
                })
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}
