use super::Outcome;
use clap::Args;

#[derive(Args, Default)]
pub struct SuggestArgs {
    /// How far back to read at most, in days.
    #[arg(long, default_value_t = 90)]
    pub since: u32,
    /// Read every session in the window again, even the ones already read.
    #[arg(long)]
    pub reread: bool,
    /// Only these repos, by folder name, comma-separated.
    #[arg(long)]
    pub repos: Option<String>,
    /// The model the agents run on: a Claude id or alias, `codex`, or `codex:<model-id>`. Defaults to the one you used most lately.
    #[arg(long)]
    pub model: Option<String>,
    /// Agents running at once.
    #[arg(long, default_value_t = 8)]
    pub parallel: usize,
    /// At most this many suggestions.
    #[arg(long)]
    pub max: Option<usize>,
    /// Add every suggestion without asking.
    #[arg(long)]
    pub yes: bool,
}

pub fn run(args: SuggestArgs) -> Outcome {
    crate::onboard::suggest::run(args)
}
