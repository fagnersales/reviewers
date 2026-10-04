use super::Outcome;
use clap::{Args, ValueEnum};

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum ContextScope {
    All,
    Project,
}

/// The default `reviewers context` sets; unset, onboarding and `suggest` ask each time.
pub const CONTEXT_SETTING: &str = "context";

impl ContextScope {
    pub fn name(self) -> &'static str {
        match self {
            ContextScope::All => "all",
            ContextScope::Project => "project",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        ContextScope::from_str(name, false).ok()
    }

    fn describe(self) -> &'static str {
        match self {
            ContextScope::All => "all selected projects, pooled",
            ContextScope::Project => "each project separately",
        }
    }
}

pub fn context(value: Option<String>) -> Outcome {
    let store = crate::store::Store::open_default()?;
    match value.as_deref() {
        Some("default") => {
            store.set_setting(CONTEXT_SETTING, None)?;
            println!("No default context: onboarding and `suggest` will ask, or need --context.");
        }
        Some(name) => {
            let scope = ContextScope::from_name(name).ok_or_else(|| format!("unknown context `{name}`; use all, project or default"))?;
            store.set_setting(CONTEXT_SETTING, Some(scope.name()))?;
            println!("Default context: {}", scope.describe());
        }
        None => match store.setting(CONTEXT_SETTING)?.as_deref().and_then(ContextScope::from_name) {
            Some(scope) => println!("{}", scope.describe()),
            None => println!("(no default: onboarding and `suggest` ask, or need --context)"),
        },
    }
    Ok(0)
}

#[derive(Args)]
pub struct OnboardArgs {
    /// How far back to read, in days.
    #[arg(long, default_value_t = 90)]
    pub since: u32,
    /// Only these repos, by folder name, comma-separated (skips the repo picker).
    #[arg(long)]
    pub repos: Option<String>,
    /// Pool evidence from all selected repos, or keep each project's evidence separate, for this run (skips the picker). Defaults to `reviewers context`.
    #[arg(long, value_enum)]
    pub context: Option<ContextScope>,
    /// Read sessions in the window again, including ones already read.
    #[arg(long)]
    pub reread: bool,
    /// The model for extraction and merging: a Claude id/alias, `codex:<model>` or `codex` (skips the picker).
    #[arg(long)]
    pub model: Option<String>,
    /// Agents running at once.
    #[arg(long, default_value_t = 8)]
    pub parallel: usize,
    /// At most this many Reviewers.
    #[arg(long)]
    pub max: Option<usize>,
    /// Take the defaults without asking: every repo, your usual model, the strongest Reviewers on, hooks installed.
    #[arg(long)]
    pub yes: bool,
    /// Build the material and stop before any agent runs.
    #[arg(long)]
    pub dry_run: bool,
    /// Don't give your coding agents the reviewers skill.
    #[arg(long)]
    pub no_skill: bool,
}

// Bare `reviewers` enters onboarding without Clap parsing this subcommand.
impl Default for OnboardArgs {
    fn default() -> Self {
        Self { since: 90, repos: None, context: None, reread: false, model: None, parallel: 8, max: None, yes: false, dry_run: false, no_skill: false }
    }
}

pub fn run(args: OnboardArgs) -> Outcome {
    crate::onboard::run(args)
}
