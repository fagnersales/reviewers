mod claude;
mod commands;
mod diff;
mod evals;
mod git;
mod help;
mod hooks;
mod import;
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
        #[arg(required = true)]
        reviewers: Vec<String>,
    },
    /// Turn Reviewers off without deleting them or their history.
    Disable {
        #[arg(required = true)]
        reviewers: Vec<String>,
    },
    /// Delete a Reviewer and its eval cases. Past decisions stay in the history.
    Remove { reviewer: String },
    /// Recent commit reviews: this repo's, or every repo's with --all.
    Runs {
        #[arg(long)]
        all: bool,
        /// Include eval runs.
        #[arg(long)]
        evals: bool,
        #[arg(long, default_value_t = 20)]
        limit: u32,
        #[command(flatten)]
        output: Output,
    },
    /// One review in full: every verdict, its evidence and reasoning.
    Run {
        id: String,
        /// Also print each Reviewer's whole session: files read, thinking, tools.
        #[arg(long)]
        session: bool,
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
        reviewer: String,
        #[arg(long, default_value_t = 10)]
        limit: u32,
        /// Delete one batch instead.
        #[arg(long)]
        delete: Option<String>,
        #[command(flatten)]
        output: Output,
    },
    /// Repos Reviewers judge.
    Repos(Output),
    /// Start judging this repo (or PATH): register it and install its hooks.
    Init {
        path: Option<std::path::PathBuf>,
    },
    /// The git hooks that run Reviewers.
    #[command(subcommand)]
    Hooks(commands::repos::HooksCommand),
    /// Show or set the default model Reviewers run on.
    Model {
        /// A model id or alias, or `default` to use Claude Code's own default.
        model: Option<String>,
        /// Set it for this repo only.
        #[arg(long)]
        repo: bool,
    },
    /// Bring over Reviewers, history and evals from Personal Workspace.
    Import(import::ImportArgs),
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
    CommitMsg { message_file: Option<String> },
    PostCommit,
}

fn main() {
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
