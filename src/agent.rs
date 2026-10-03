//! Model selection shared by reviews, evals and onboarding. Unqualified models
//! keep their historical Claude meaning; Codex is always an explicit choice.
use crate::{claude, codex};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tokens {
    /// Everything the model took in, cache hits included: each turn re-reads the conversation.
    pub read: u64,
    pub written: u64,
}

impl std::ops::Add for Tokens {
    type Output = Tokens;
    fn add(self, other: Tokens) -> Tokens {
        Tokens { read: self.read + other.read, written: self.written + other.written }
    }
}

pub enum Activity {
    Thinking(u64),
    Tool {
        name: String,
        input: Value,
    },
    /// The structured answer as it streams: every `name` written so far.
    Answering(Vec<String>),
    Tokens(Tokens),
}

pub struct Request<'a> {
    pub prompt: &'a str,
    pub cwd: &'a Path,
    pub schema: &'a Value,
    /// Claude's tool allowlist; Codex enables its sandboxed shell when nonempty.
    pub tools: &'a [&'a str],
    pub allowed_tools: &'a [&'a str],
    /// Read access for Claude. Codex's read-only sandbox already permits reads;
    /// these must never become Codex --add-dir flags (which grant writes).
    pub add_dirs: &'a [PathBuf],
    pub model: Option<&'a str>,
    pub timeout: Option<Duration>,
    /// Stream token-by-token events, for live progress. Costs nothing but stdout volume.
    pub live: bool,
}

pub struct Outcome {
    pub output: Value,
    pub tokens: Tokens,
    pub turns: u32,
    pub tool_calls: u32,
    pub model: Option<String>,
    /// The session as stored on a decision: prompt, system line, thinking, tools, text.
    pub session: Vec<Value>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    Claude,
    Codex,
}

pub fn selection(model: Option<&str>) -> (Provider, Option<&str>) {
    match model {
        Some("codex") => (Provider::Codex, None),
        Some(model) if model.starts_with("codex:") => (Provider::Codex, Some(&model[6..])),
        Some("claude") => (Provider::Claude, None),
        Some(model) if model.starts_with("claude:") => (Provider::Claude, Some(&model[7..])),
        model => (Provider::Claude, model),
    }
}

pub fn is_installed(provider: Provider) -> bool {
    match provider {
        Provider::Claude => claude::is_installed(),
        Provider::Codex => codex::is_installed(),
    }
}

pub fn run(request: &Request, on_activity: &mut dyn FnMut(Activity)) -> Result<Outcome, String> {
    let (provider, model) = selection(request.model);
    if model.is_some_and(|model| model.trim().is_empty()) {
        return Err("a provider prefix needs a model id; use `claude` or `codex` for its default".into());
    }
    let request = Request { model, ..*request };
    match provider {
        Provider::Claude => claude::run(&request, on_activity),
        Provider::Codex => codex::run(&request, on_activity),
    }
}

pub fn stop_all() {
    claude::stop_all();
    codex::stop_all();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_models_stay_on_claude_and_codex_is_explicit() {
        assert_eq!(selection(None), (Provider::Claude, None));
        assert_eq!(selection(Some("sonnet")), (Provider::Claude, Some("sonnet")));
        assert_eq!(selection(Some("claude:opus")), (Provider::Claude, Some("opus")));
        assert_eq!(selection(Some("codex")), (Provider::Codex, None));
        assert_eq!(selection(Some("codex:custom-model")), (Provider::Codex, Some("custom-model")));
    }
}
