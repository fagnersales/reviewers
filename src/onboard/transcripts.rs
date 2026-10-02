use crate::git;
use crate::util::{home, str_field};
use rayon::prelude::*;
use regex::Regex;
use serde_json::Value;
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::SystemTime;

const MESSAGE_LIMIT: usize = 2_000;
const CITATION_QUOTE_LIMIT: usize = 300;
/// The default model follows what the person used lately, not over the whole window.
pub const RECENT_SESSIONS: usize = 30;

/// Text a harness wrote into the user turn, not the person.
const HARNESS_PREFIXES: [&str; 12] = [
    "<command-name>",
    "<command-message>",
    "<local-command-stdout>",
    "<local-command-stderr>",
    "<local-command-caveat>",
    "<task-notification>",
    "<bash-input>",
    "<bash-stdout>",
    "Caveat:",
    "[Request interrupted",
    "This session is being continued",
    "Base directory for this skill:",
];

/// `claude -p` sessions are scripts talking, not a person: Reviewer runs, evals, automation.
const SCRIPTED_ENTRYPOINTS: [&str; 1] = ["sdk-cli"];

#[derive(Clone, Debug)]
pub struct HumanMessage {
    pub at: String,
    pub text: String,
}

pub struct RepoTranscripts {
    pub root: PathBuf,
    pub name: String,
    pub sessions: usize,
    pub messages: Vec<HumanMessage>,
}

#[derive(Clone, Debug)]
pub struct ModelUsage {
    pub id: String,
    pub sessions: usize,
    pub recent_sessions: usize,
}

pub struct Scan {
    pub repos: Vec<RepoTranscripts>,
    pub models: Vec<ModelUsage>,
    pub sessions_read: usize,
    pub sessions_skipped: usize,
}

struct SessionRead {
    cwd: Option<String>,
    entrypoint: Option<String>,
    messages: Vec<HumanMessage>,
    model_turns: HashMap<String, u32>,
}

fn regex(slot: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    slot.get_or_init(|| Regex::new(pattern).expect("the pattern compiles"))
}

/// Every Claude Code config folder counts: `~/.claude`, `CLAUDE_CONFIG_DIR`, and any `~/.claude-*`.
fn transcript_roots() -> Vec<PathBuf> {
    let mut roots = vec![home().join(".claude")];
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR") {
        roots.push(PathBuf::from(dir));
    }
    if let Ok(entries) = std::fs::read_dir(home()) {
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().starts_with(".claude") {
                roots.push(entry.path());
            }
        }
    }
    let mut seen = Vec::new();
    roots
        .into_iter()
        .map(|root| root.join("projects"))
        .filter(|projects| projects.is_dir())
        .filter(|projects| {
            let resolved = projects.canonicalize().unwrap_or(projects.clone());
            let fresh = !seen.contains(&resolved);
            seen.push(resolved);
            fresh
        })
        .collect()
}

fn session_files(since: SystemTime) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for projects in transcript_roots() {
        let Ok(directories) = std::fs::read_dir(&projects) else {
            continue;
        };
        for directory in directories.flatten().filter(|entry| entry.path().is_dir()) {
            let Ok(entries) = std::fs::read_dir(directory.path()) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let fresh = entry.metadata().and_then(|meta| meta.modified()).is_ok_and(|modified| modified >= since);
                if path.extension().is_some_and(|extension| extension == "jsonl") && fresh {
                    files.push(path);
                }
            }
        }
    }
    files
}

fn is_throwaway(cwd: &str) -> bool {
    cwd.starts_with("/private/var/folders/") || cwd.starts_with("/tmp/") || cwd.starts_with(&std::env::temp_dir().display().to_string())
}

fn text_parts(content: &Value) -> Option<String> {
    match content {
        Value::String(text) => Some(text.clone()),
        Value::Array(parts) => {
            let mut texts = Vec::new();
            for part in parts {
                match str_field(part, "type") {
                    Some("tool_result") => return None,
                    Some("text") => {
                        if let Some(text) = str_field(part, "text") {
                            texts.push(text.to_string());
                        }
                    }
                    _ => {}
                }
            }
            (!texts.is_empty()).then(|| texts.join("\n"))
        }
        _ => None,
    }
}

/// A comment pinned to a quote of the agent's reply is the most direct feedback a transcript holds.
fn render_citations(block: &str) -> String {
    let Ok(Value::Array(citations)) = serde_json::from_str::<Value>(block) else {
        return String::new();
    };
    citations
        .iter()
        .filter_map(|item| item.get("citation"))
        .map(|citation| {
            let quote: String = str_field(citation, "text").unwrap_or_default().split_whitespace().collect::<Vec<_>>().join(" ").chars().take(CITATION_QUOTE_LIMIT).collect();
            match str_field(citation, "comment") {
                Some(comment) => format!("[on the agent's words: \"{quote}\"] → {comment}"),
                None => format!("[quoting the agent: \"{quote}\"]"),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn clean_human_text(raw: &str) -> Option<String> {
    static REMINDER: OnceLock<Regex> = OnceLock::new();
    static CITATIONS: OnceLock<Regex> = OnceLock::new();
    static PASTED: OnceLock<Regex> = OnceLock::new();
    static T3: OnceLock<Regex> = OnceLock::new();
    static ATTACHED: OnceLock<Regex> = OnceLock::new();
    static IMAGE: OnceLock<Regex> = OnceLock::new();
    static QUOTE_MARK: OnceLock<Regex> = OnceLock::new();
    let trimmed = raw.trim();
    if HARNESS_PREFIXES.iter().any(|prefix| trimmed.starts_with(prefix)) {
        return None;
    }
    let text = regex(&REMINDER, r"(?s)<system-reminder>.*?</system-reminder>").replace_all(trimmed, "");
    let text = regex(&CITATIONS, r"(?s)<assistant_citations>(.*?)</assistant_citations>").replace_all(&text, |captures: &regex::Captures| {
        let block = &captures[1];
        match (block.find('['), block.rfind(']')) {
            (Some(start), Some(end)) if end > start => format!("\n{}", render_citations(&block[start..=end])),
            _ => String::new(),
        }
    });
    let text = regex(&PASTED, r"(?s)<pasted_content[^>]*>(.*?)</pasted_content[^>]*>").replace_all(&text, |captures: &regex::Captures| format!("[pasted {} chars]", captures[1].len()));
    let text = regex(&T3, r"(?s)<t3_context.*?</t3_context>").replace_all(&text, "");
    let text = regex(&ATTACHED, r#"\[Attached image "[^"]*" is saved at: [^\]]*\]"#).replace_all(&text, "[image]");
    let text = regex(&IMAGE, r"\[Image: [^\]]*\]").replace_all(&text, "[image]");
    let text = regex(&QUOTE_MARK, r"\[assistant-quote-\d+\]").replace_all(&text, "");
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    Some(if text.chars().count() > MESSAGE_LIMIT { format!("{} […]", text.chars().take(MESSAGE_LIMIT).collect::<String>()) } else { text.to_string() })
}

fn read_session(path: &Path, since: &str) -> Option<SessionRead> {
    static MODEL: OnceLock<Regex> = OnceLock::new();
    let file = std::fs::File::open(path).ok()?;
    let mut reader = BufReader::new(file);
    let mut read = SessionRead { cwd: None, entrypoint: None, messages: Vec::new(), model_turns: HashMap::new() };
    let mut buffer = Vec::new();
    loop {
        buffer.clear();
        match reader.read_until(b'\n', &mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let line = String::from_utf8_lossy(&buffer);
        // Assistant turns are large; the model id is all that's needed, so skip the parse.
        if line.contains("\"type\":\"assistant\"") && !line.contains("\"isSidechain\":true") {
            if let Some(model) = regex(&MODEL, r#""model":"(claude-[^"]+)""#).captures(&line) {
                *read.model_turns.entry(model[1].to_string()).or_insert(0) += 1;
            }
            continue;
        }
        if !line.contains("\"type\":\"user\"") {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let flag = |key: &str| entry[key].as_bool() == Some(true);
        if str_field(&entry, "type") != Some("user") || flag("isSidechain") || flag("isMeta") || flag("isCompactSummary") {
            continue;
        }
        if read.cwd.is_none() {
            read.cwd = str_field(&entry, "cwd").map(str::to_string);
        }
        if read.entrypoint.is_none() {
            read.entrypoint = str_field(&entry, "entrypoint").map(str::to_string);
        }
        let Some(at) = str_field(&entry, "timestamp").filter(|at| *at >= since) else {
            continue;
        };
        if let Some(text) = text_parts(&entry["message"]["content"]).and_then(|raw| clean_human_text(&raw)) {
            read.messages.push(HumanMessage { at: at.to_string(), text });
        }
    }
    Some(read)
}

/// Agents work in worktrees that are often deleted by the time we look. A
/// live folder resolves through git; a dead one borrows its repo by the
/// worktree folder's parent name (`…/worktrees/<repo>/…`).
#[derive(Default)]
struct RepoResolver {
    by_cwd: HashMap<String, Option<PathBuf>>,
    by_name: HashMap<String, PathBuf>,
}

impl RepoResolver {
    fn resolve(&mut self, cwd: &str) -> Option<PathBuf> {
        if let Some(cached) = self.by_cwd.get(cwd) {
            return cached.clone();
        }
        let root = Path::new(cwd).is_dir().then(|| git::root_of(Path::new(cwd))).flatten().map(|root| git::main_checkout(&root));
        if let Some(root) = &root {
            if let Some(name) = root.file_name() {
                self.by_name.insert(name.to_string_lossy().to_string(), root.clone());
            }
        }
        self.by_cwd.insert(cwd.to_string(), root.clone());
        root
    }

    fn resolve_dead(&self, cwd: &str) -> Option<PathBuf> {
        static WORKTREE: OnceLock<Regex> = OnceLock::new();
        let name = regex(&WORKTREE, r"/worktrees/([^/]+)/").captures(cwd)?;
        self.by_name.get(&name[1]).cloned()
    }
}

pub fn scan(since_days: u32, on_progress: &(dyn Fn(usize, usize) + Sync)) -> Scan {
    let since_time = SystemTime::now() - std::time::Duration::from_secs(since_days as u64 * 86_400);
    let since_iso = crate::util::to_iso(chrono::Utc::now() - chrono::Duration::days(since_days as i64));
    let files = session_files(since_time);
    let total = files.len();
    let done = AtomicUsize::new(0);
    let reads: Vec<SessionRead> = files
        .par_iter()
        .filter_map(|file| {
            let read = read_session(file, &since_iso);
            on_progress(done.fetch_add(1, Ordering::Relaxed) + 1, total);
            read
        })
        .collect();

    let mut resolver = RepoResolver::default();
    let live: Vec<Option<PathBuf>> = reads
        .iter()
        .map(|read| read.cwd.as_deref().filter(|cwd| !is_throwaway(cwd)).and_then(|cwd| resolver.resolve(cwd)))
        .collect();
    let mut repos: HashMap<PathBuf, RepoTranscripts> = HashMap::new();
    let mut personal: Vec<&SessionRead> = Vec::new();
    let mut skipped = 0;
    for (read, live_root) in reads.iter().zip(live) {
        let scripted = read.entrypoint.as_deref().is_some_and(|entrypoint| SCRIPTED_ENTRYPOINTS.contains(&entrypoint));
        if !read.messages.is_empty() && !scripted {
            personal.push(read);
        }
        let root = live_root.or_else(|| read.cwd.as_deref().filter(|cwd| !is_throwaway(cwd)).and_then(|cwd| resolver.resolve_dead(cwd)));
        let Some(root) = root.filter(|_| !read.messages.is_empty() && !scripted) else {
            skipped += 1;
            continue;
        };
        let repo = repos.entry(root.clone()).or_insert_with(|| RepoTranscripts {
            name: root.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default(),
            root: root.clone(),
            sessions: 0,
            messages: Vec::new(),
        });
        repo.sessions += 1;
        repo.messages.extend(read.messages.iter().cloned());
    }
    let mut repos: Vec<RepoTranscripts> = repos.into_values().collect();
    for repo in &mut repos {
        repo.messages.sort_by(|a, b| a.at.cmp(&b.at));
    }
    repos.sort_by(|a, b| b.messages.len().cmp(&a.messages.len()));
    Scan { repos, models: model_usage(&personal), sessions_read: reads.len(), sessions_skipped: skipped }
}

/// A session counts for the model that ran most of its turns.
fn model_usage(sessions: &[&SessionRead]) -> Vec<ModelUsage> {
    let mut ordered: Vec<&&SessionRead> = sessions.iter().collect();
    let last_at = |read: &SessionRead| read.messages.last().map(|message| message.at.clone()).unwrap_or_default();
    ordered.sort_by_key(|read| std::cmp::Reverse(last_at(read)));
    let mut usage: Vec<ModelUsage> = Vec::new();
    for (index, read) in ordered.iter().enumerate() {
        let Some((model, _)) = read.model_turns.iter().max_by_key(|(_, turns)| **turns) else {
            continue;
        };
        let entry = match usage.iter_mut().position(|usage| &usage.id == model) {
            Some(position) => &mut usage[position],
            None => {
                usage.push(ModelUsage { id: model.clone(), sessions: 0, recent_sessions: 0 });
                usage.last_mut().expect("just pushed")
            }
        };
        entry.sessions += 1;
        if index < RECENT_SESSIONS {
            entry.recent_sessions += 1;
        }
    }
    usage.sort_by(|a, b| b.recent_sessions.cmp(&a.recent_sessions).then(b.sessions.cmp(&a.sessions)));
    usage
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_what_the_person_typed() {
        assert_eq!(clean_human_text("<command-name>/clear</command-name>"), None);
        assert_eq!(clean_human_text("fix it<system-reminder>noise</system-reminder>").as_deref(), Some("fix it"));
        let cited = r#"<assistant_citations>[{"citation":{"text":"I added a cast","comment":"never cast"}}]</assistant_citations>"#;
        assert_eq!(clean_human_text(cited).as_deref(), Some("[on the agent's words: \"I added a cast\"] → never cast"));
    }
}
