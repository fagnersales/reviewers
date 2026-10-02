use super::Outcome;
use clap::Args;

#[derive(Args, Default)]
pub struct OnboardArgs {
    /// How far back to read, in days.
    #[arg(long, default_value_t = 90)]
    pub since: u32,
    /// Only these repos, by folder name, comma-separated (skips the repo picker).
    #[arg(long)]
    pub repos: Option<String>,
    /// The model the agents run on (skips the model picker).
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

pub fn run(args: OnboardArgs) -> Outcome {
    crate::onboard::run(args)
}
