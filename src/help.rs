use crate::commands::Outcome;
use clap::{Args, CommandFactory};

#[derive(Args, Default)]
pub struct HelpArgs {
    /// A topic: writing, evals, onboarding, hooks, classifier, commands.
    pub topic: Option<String>,
    /// The full guide for coding agents: how Reviewers work and every command.
    #[arg(long)]
    pub agent: bool,
}

const AGENT_GUIDE: &str = include_str!("help/agent.md");
const WRITING: &str = include_str!("help/writing.md");
const EVALS: &str = include_str!("help/evals.md");
const ONBOARDING: &str = include_str!("help/onboarding.md");
const HOOKS: &str = include_str!("help/hooks.md");
const CLASSIFIER: &str = include_str!("help/classifier.md");

const TOPICS: [(&str, &str, &str); 5] = [
    ("writing", "How to write a Reviewer that judges well", WRITING),
    ("evals", "Cases and the tuning loop", EVALS),
    ("onboarding", "How the first run finds your rules", ONBOARDING),
    ("hooks", "What happens on a commit, and how to skip it", HOOKS),
    ("classifier", "Skipping the Reviewers a change can't concern", CLASSIFIER),
];

fn argument_line(argument: &clap::Arg) -> Option<String> {
    let id = argument.get_id().as_str();
    if id == "help" || id == "version" || argument.is_hide_set() {
        return None;
    }
    let help = argument.get_help().map(|help| help.to_string()).unwrap_or_default();
    let name = match argument.get_long() {
        Some(long) if argument.get_action().takes_values() => format!("--{long} <{}>", id.to_uppercase()),
        Some(long) => format!("--{long}"),
        None => format!("<{}>", id.to_uppercase()),
    };
    let required = if argument.is_required_set() { " (required)" } else { "" };
    let values: Vec<String> = argument
        .get_possible_values()
        .iter()
        .filter(|value| !value.is_hide_set())
        .map(|value| match value.get_help() {
            Some(help) => format!("`{}` ({})", value.get_name(), help.to_string().trim_end_matches('.')),
            None => format!("`{}`", value.get_name()),
        })
        .collect();
    let help = match (help.is_empty(), values.is_empty()) {
        (_, true) => help,
        (true, false) => format!("one of {}", values.join(", ")),
        (false, false) => format!("{help}; one of {}", values.join(", ")),
    };
    Some(format!("  {name}{required}{}", if help.is_empty() { String::new() } else { format!(": {help}") }))
}

fn reference(command: &clap::Command, prefix: &str, out: &mut Vec<String>) {
    for sub in command.get_subcommands().filter(|sub| !sub.is_hide_set()) {
        let path = format!("{prefix} {}", sub.get_name());
        if sub.has_subcommands() {
            reference(sub, &path, out);
            continue;
        }
        out.push(format!("### `{}`", path.trim()));
        out.push(String::new());
        if let Some(about) = sub.get_about() {
            out.push(about.to_string());
        }
        let arguments: Vec<String> = sub.get_arguments().filter_map(argument_line).collect();
        if !arguments.is_empty() {
            out.push(String::new());
            out.extend(arguments);
        }
        out.push(String::new());
    }
}

/// Generated from the CLI itself, so the guide never lists a command the binary doesn't have.
pub fn command_reference() -> String {
    let mut lines = vec!["## Commands".to_string(), String::new(), "Every read command takes `--json`; agents should always use it.".to_string(), String::new()];
    reference(&crate::Cli::command(), "reviewers", &mut lines);
    lines.join("\n")
}

pub fn run(args: HelpArgs) -> Outcome {
    if args.agent {
        println!("{}\n\n{}", AGENT_GUIDE.trim_end().replace("{{VERSION}}", env!("CARGO_PKG_VERSION")), command_reference());
        println!("\n## Topics\n");
        for (name, summary, _) in TOPICS {
            println!("- `reviewers help {name}`: {summary}");
        }
        return Ok(0);
    }
    if let Some(topic) = args.topic.as_deref() {
        if topic == "commands" {
            println!("{}", command_reference());
            return Ok(0);
        }
        return match TOPICS.iter().find(|(name, _, _)| *name == topic) {
            Some((_, _, text)) => {
                println!("{}", text.trim_end());
                Ok(0)
            }
            None => Err(format!("no topic \"{topic}\"; try: writing, evals, onboarding, hooks, commands")),
        };
    }
    let mut command = crate::Cli::command();
    println!("{}", command.render_help());
    println!("Topics: {}, commands", TOPICS.iter().map(|(name, _, _)| *name).collect::<Vec<_>>().join(", "));
    println!("Coding agents: `reviewers help --agent` is the full guide.");
    Ok(0)
}
