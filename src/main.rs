mod claude;
mod classifier;
mod commands;
mod diff;
mod evals;
mod git;
mod help;
mod hooks;
mod onboard;
mod review;
mod scope;
mod skill;
mod store;
mod ui;
mod upgrade;
mod util;

use clap::{Args, Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "reviewers",
    version,
    about = "Your rules, checked on every commit your agent makes.",
    long_about = "Reviewers judge every commit an agent makes against one rule each, and stop the commit when the rule is broken. Run `reviewers` in a terminal to set them up from your own agent sessions.",
    disable_help_subcommand = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Args, Clone, Copy, Default)]
pub struct Output {
    /// Print JSON instead of text, for agents and scripts.
    #[arg(long)]
    pub json: bool,
}

#[derive(Subcommand)]
pub enum Command {
    /// Read your agent sessions and turn the rules you keep repeating into Reviewers.
    Onboard(commands::onboard::OnboardArgs),
    /// Read only the sessions since the last read and suggest new Reviewers, each with why and the words behind it.
    Suggest(commands::suggest::SuggestArgs),
    /// This repo: the Reviewers that judge it and its latest reviews.
    Status(Output),
    /// List Reviewers: this repo's, or every one with --all.
    List {
        /// Every Reviewer, not just the ones judging this repo.
        #[arg(long)]
        all: bool,
        #[command(flatten)]
        output: Output,
    },
    /// One Reviewer: its instruction, where it runs, and its record.
    Show {
        /// Name, slug or id.
        reviewer: String,
        /// How many recent decisions to list.
        #[arg(long, default_value_t = 10)]
        decisions: u32,
        #[command(flatten)]
        output: Output,
    },
    /// Create a Reviewer.
    New(commands::reviewers::NewArgs),
    /// Change a Reviewer. A new instruction is a new version.
    Edit(commands::reviewers::EditArgs),
    /// Turn Reviewers on.
    Enable {
        /// Names, slugs or ids.
        #[arg(required = true)]
        reviewers: Vec<String>,
    },
    /// Turn Reviewers off without deleting them or their history.
    Disable {
        /// Names, slugs or ids.
        #[arg(required = true)]
        reviewers: Vec<String>,
    },
    /// Delete a Reviewer and its eval cases. Past decisions stay in the history.
    Remove {
        /// Name, slug or id.
        reviewer: String,
    },
    /// Recent commit reviews: this repo's, or every repo's with --all.
    Runs {
        /// Every repo, not just this one.
        #[arg(long)]
        all: bool,
        /// Include eval runs.
        #[arg(long)]
        evals: bool,
        /// How many runs to list.
        #[arg(long, default_value_t = 20)]
        limit: u32,
        /// Delete every eval run kept by `reviewers eval --record`. Eval scores (`reviewers evals`) stay; commit reviews are never deleted in bulk.
        #[arg(long)]
        prune_evals: bool,
        #[command(flatten)]
        output: Output,
    },
    /// One review in full: every verdict, its evidence and reasoning.
    Run {
        /// The run id, from the hook's output or `reviewers runs`.
        id: String,
        /// Also print each Reviewer's whole session: files read, thinking, tools.
        #[arg(long)]
        session: bool,
        /// Delete this run from the history.
        #[arg(long)]
        delete: bool,
        #[command(flatten)]
        output: Output,
    },
    /// What each Reviewer catches, how long it adds to a commit, and the tokens it uses.
    Stats {
        /// Every repo, not just this one.
        #[arg(long)]
        all: bool,
        /// Only the last N days.
        #[arg(long)]
        days: Option<u32>,
        #[command(flatten)]
        output: Output,
    },
    /// Eval cases: diffs a Reviewer must judge a known way.
    #[command(subcommand)]
    Case(evals::CaseCommand),
    /// Run a Reviewer's eval cases and record the score.
    Eval(evals::EvalArgs),
    /// A Reviewer's eval history, newest first.
    Evals {
        /// Name, slug or id.
        reviewer: String,
        /// How many batches to list.
        #[arg(long, default_value_t = 10)]
        limit: u32,
        /// Delete one batch instead.
        #[arg(long)]
        delete: Option<String>,
        #[command(flatten)]
        output: Output,
    },
    /// Repos registered with Reviewers, and how many Reviewers judge each; ignored ones are marked.
    Repos(Output),
    /// Start judging this repo (or PATH): register it and install its hooks.
    Init {
        /// Any path inside the repo; defaults to the current directory.
        path: Option<std::path::PathBuf>,
    },
    /// Stop judging this repo (or PATH), even under the global hooks. `reviewers init` undoes it.
    Ignore {
        /// Any path inside the repo; defaults to the current directory.
        path: Option<std::path::PathBuf>,
    },
    /// The git hooks that run Reviewers.
    #[command(subcommand)]
    Hooks(commands::repos::HooksCommand),
    /// Show or set the model Reviewers run on. A Reviewer's own model (`edit --model`) wins, then the repo's, then this default, then Claude Code's own.
    Model {
        /// Any model id or alias `claude --model` accepts (`sonnet`, `opus`, or a full id), or `default` to clear this level. Leave it out to print the current one.
        model: Option<String>,
        /// For the repo you're in only.
        #[arg(long)]
        repo: bool,
    },
    /// The classifier: a cheap first pass that skips Reviewers a change can't concern. With no subcommand, the same as `status`.
    Classifier {
        #[command(subcommand)]
        command: Option<commands::classifier::ClassifierCommand>,
    },
    /// The skill that tells your coding agents Reviewers exist.
    #[command(subcommand)]
    Skill(skill::SkillCommand),
    /// Update reviewers to the latest release.
    Upgrade(upgrade::UpgradeArgs),
    /// What you can do. Agents: `reviewers help --agent`.
    Help(help::HelpArgs),
    /// Called by the git hooks.
    #[command(hide = true, subcommand)]
    Hook(HookCommand),
}

#[derive(Subcommand)]
pub enum HookCommand {
    /// What the commit-msg hook calls: judges the staged change; anything but exit 0 stops the commit.
    CommitMsg {
        /// The commit message file git hands the hook; without it, the review is kept without a message.
        message_file: Option<String>,
    },
    /// What the post-commit hook calls: ties the landed commit to the review that let it through.
    PostCommit,
}

#[cfg(unix)]
unsafe extern "C" {
    fn signal(signum: i32, handler: usize) -> usize;
}

/// Rust ignores SIGPIPE, so printing into a closed pipe (`reviewers runs --json | head`) panics.
/// Like any command-line tool, end quietly instead.
fn exit_quietly_on_closed_pipe() {
    #[cfg(unix)]
    {
        const SIGPIPE: i32 = 13;
        const SIG_DFL: usize = 0;
        // SAFETY: restores the default action before any thread starts; nothing else handles SIGPIPE.
        unsafe {
            signal(SIGPIPE, SIG_DFL);
        }
    }
}

fn main() {
    exit_quietly_on_closed_pipe();
    let cli = Cli::parse();
    let code = match cli.command {
        Some(Command::Hook(HookCommand::CommitMsg { message_file })) => hooks::commit_msg(message_file.as_deref()),
        Some(Command::Hook(HookCommand::PostCommit)) => hooks::post_commit(),
        command => match commands::dispatch(command) {
            Ok(code) => code,
            Err(error) => {
                eprintln!("{} {error}", ui::red("reviewers:"));
                1
            }
        },
    };
    std::process::exit(code);
}
