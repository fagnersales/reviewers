use serde_json::{Value, json};
use std::path::Path;

/// Reading the repository takes longer than judging hunks alone.
pub const REVIEW_TIMEOUT_SECS: u64 = 300;
const CONTEXT_FILE_MAX_BYTES: usize = 64 * 1024;

pub struct ContextFile {
    pub path: String,
    /// `None` when the file is not in the repository at review time.
    pub content: Option<String>,
}

pub fn read_context_files(root: &Path, paths: &[String]) -> Vec<ContextFile> {
    paths
        .iter()
        .map(|relative| {
            let absolute = root.join(relative);
            let inside = absolute.canonicalize().ok().zip(root.canonicalize().ok()).is_some_and(|(file, root)| file.starts_with(root));
            let content = inside.then(|| std::fs::read_to_string(&absolute).ok()).flatten().map(|content| {
                if content.len() > CONTEXT_FILE_MAX_BYTES {
                    let cut = (0..=CONTEXT_FILE_MAX_BYTES).rev().find(|&index| content.is_char_boundary(index)).unwrap_or(0);
                    format!("{}\n[truncated at {CONTEXT_FILE_MAX_BYTES} bytes]", &content[..cut])
                } else {
                    content
                }
            });
            ContextFile { path: relative.clone(), content }
        })
        .collect()
}

pub fn decision_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "verdict": { "type": "string", "enum": ["approved", "blocked"] },
            "summary": { "type": "string", "minLength": 1 },
            "reasoning": { "type": "string", "minLength": 1 },
            "evidence": {
                "type": "array",
                "description": "Required when the verdict is blocked: one entry per violation, quoting the changed lines.",
                "items": {
                    "type": "object",
                    "properties": {
                        "file": { "type": "string", "minLength": 1 },
                        "startLine": { "type": "integer", "minimum": 1 },
                        "endLine": { "type": "integer", "minimum": 1 },
                        "excerpt": { "type": "string", "minLength": 1 },
                        "explanation": { "type": "string", "minLength": 1 }
                    },
                    "required": ["file", "excerpt", "explanation"]
                }
            }
        },
        // Evidence is checked in code (a block needs it): some models fumble an empty required list and retry until they give up.
        "required": ["verdict", "summary", "reasoning"]
    })
}

pub struct PromptInput<'a> {
    pub reviewer_name: &'a str,
    pub instruction: &'a str,
    pub repository: &'a str,
    pub diff: &'a str,
    pub context_files: &'a [ContextFile],
}

pub fn review_prompt(input: &PromptInput) -> String {
    let mut lines: Vec<String> = vec![
        "You are a single-purpose code Reviewer.".into(),
        "Judge the supplied diff against the single instruction below, and nothing else.".into(),
        "Do not perform a general code review. Do not comment on unrelated quality.".into(),
        String::new(),
        "The diff is what is under judgment. Your working directory is the repository root with the change already applied, so a changed file on disk is its post-change version. Use the available read-only tools to see what the diff does not show — the rest of a changed file, its sibling files, an existing module the instruction points at — whenever the instruction cannot be judged from the hunks alone. Never modify anything.".into(),
        String::new(),
        "Evidence must quote changed lines from the diff: lines added, or lines removed. What you read elsewhere in the repository informs the reasoning but is not evidence by itself.".into(),
        "If you block, cite exact files and lines from the diff, and say in each explanation what the code should do instead: the agent that wrote it reads your answer and fixes the code from it.".into(),
        "Block only when the diff clearly breaks the instruction. If the code can fairly be read either way, or you can't establish the violation from the diff and the repository, approve, and say in the reasoning what you could not establish.".into(),
        String::new(),
        "Trigger: commit".into(),
        format!("Repository: {}", input.repository),
        format!("Reviewer: {}", input.reviewer_name),
        String::new(),
        "Instruction:".into(),
        input.instruction.to_string(),
        String::new(),
    ];
    if !input.context_files.is_empty() {
        lines.push("Reference material attached by the Reviewer's author. It is authoritative for what the instruction means:".into());
        lines.push(String::new());
        for file in input.context_files {
            match &file.content {
                Some(content) => {
                    lines.push(format!("--- {} ---", file.path));
                    lines.push(content.clone());
                    lines.push(format!("--- end {} ---", file.path));
                }
                None => lines.push(format!("--- {} (not found in the repository) ---", file.path)),
            }
            lines.push(String::new());
        }
    }
    lines.push("Diff:".into());
    lines.push(input.diff.to_string());
    lines.join("\n")
}
