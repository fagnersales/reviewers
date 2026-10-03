use super::{Outcome, print_json};
use crate::classifier::{self, Connection, Provider, Question};
use crate::review::{classifier_cutoff, percent};
use crate::store::{ClassifierUse, Run, Store, Verdict};
use crate::util::{compact, plural};
use crate::ui;
use clap::Subcommand;
use rayon::prelude::*;
use serde_json::json;
use std::collections::BTreeMap;
use std::io::{IsTerminal, Read};

const NOT_CONNECTED: &str = "no classifier is connected: `reviewers classifier connect gateway` (an AI Gateway key) or `reviewers classifier connect jev` (a TypeSafe key)";

#[derive(Subcommand)]
pub enum ClassifierCommand {
    /// What's connected, the cutoffs, and what it cleared lately.
    Status {
        /// Print JSON instead of text, for agents and scripts.
        #[arg(long)]
        json: bool,
    },
    /// Connect Jev, TypeSafe's evaluation model, with a key read from a hidden prompt or from stdin. One small call checks it first.
    Connect {
        #[arg(value_enum)]
        provider: Provider,
        /// A SystemOne-compatible URL of your own, instead of the provider's.
        #[arg(long)]
        endpoint: Option<String>,
    },
    /// Forget the key. Every Reviewer runs in full again.
    Disconnect,
    /// Show or set the default cutoff, from 0 to 1. Under it, the classifier clears a Reviewer.
    Cutoff {
        /// From 0 to 1; leave it out to print the current one.
        value: Option<f64>,
    },
    /// Replay recent commits from every repo through the classifier: what it would have cleared, and any block it would have missed. Makes real calls on the key.
    Check {
        /// How many recent commits.
        #[arg(long, default_value_t = 50)]
        runs: u32,
        /// Print JSON instead of text, for agents and scripts.
        #[arg(long)]
        json: bool,
    },
}

pub fn run(command: Option<ClassifierCommand>) -> Outcome {
    match command.unwrap_or(ClassifierCommand::Status { json: false }) {
        ClassifierCommand::Status { json } => status(json),
        ClassifierCommand::Connect { provider, endpoint } => connect(provider, endpoint),
        ClassifierCommand::Disconnect => disconnect(),
        ClassifierCommand::Cutoff { value } => cutoff(value),
        ClassifierCommand::Check { runs, json } => check(runs, json),
    }
}

fn default_cutoff(store: &Store) -> Result<f64, String> {
    Ok(store.setting("classifier_cutoff")?.and_then(|text| text.parse().ok()).unwrap_or(classifier::DEFAULT_CUTOFF))
}

fn status(json_output: bool) -> Outcome {
    let store = Store::open_default()?;
    let connection = classifier::connected();
    let default = default_cutoff(&store)?;
    let reviewers = store.reviewers()?;
    let since = (chrono::Utc::now() - chrono::Duration::days(30)).format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
    let outcomes = store.classifier_outcomes(&since)?;
    if json_output {
        return print_json(&json!({
            "connected": connection.as_ref().map(|connection| json!({ "provider": connection.provider, "key": connection.key_hint() })),
            "defaultCutoff": default,
            "reviewers": reviewers.iter().map(|reviewer| json!({ "name": reviewer.name, "classifier": reviewer.classifier })).collect::<Vec<_>>(),
            "last30Days": outcomes.iter().cloned().collect::<BTreeMap<String, u64>>(),
        }));
    }
    let Some(connection) = connection else {
        println!("No classifier connected, so every Reviewer runs a reviewer session on every commit it applies to.");
        println!("{}", ui::dim("Connect one: `reviewers classifier connect gateway` (an AI Gateway key) or `reviewers classifier connect jev` (a TypeSafe key)."));
        return Ok(0);
    };
    println!("{} {}", ui::bold(connection.provider.label()), ui::dim(&format!("· key {}", connection.key_hint())));
    println!("Default cutoff {}: a Reviewer is cleared when the chance the change breaks its rule is under it.", ui::bold(&percent(default)));
    let own: Vec<String> = reviewers
        .iter()
        .filter_map(|reviewer| match reviewer.classifier {
            ClassifierUse::Cutoff(cutoff) => Some(format!("{} {}", reviewer.name, percent(cutoff))),
            _ => None,
        })
        .collect();
    let off: Vec<&str> = reviewers.iter().filter(|reviewer| reviewer.classifier == ClassifierUse::Off).map(|reviewer| reviewer.name.as_str()).collect();
    if !own.is_empty() {
        println!("{}", ui::dim(&format!("Own cutoff: {}", own.join(", "))));
    }
    if !off.is_empty() {
        println!("{}", ui::dim(&format!("Always run in full: {}", off.join(", "))));
    }
    let count = |name: &str| outcomes.iter().find(|(outcome, _)| outcome == name).map(|(_, count)| *count).unwrap_or(0);
    println!(
        "{}",
        ui::dim(&format!(
            "Last 30 days: {} cleared, {} ran in full, {} without an answer",
            count("cleared"),
            count("escalated"),
            count("unavailable")
        ))
    );
    Ok(0)
}

fn read_key(provider: Provider) -> Result<String, String> {
    let key = if std::io::stdin().is_terminal() {
        ui::secret(&format!("Paste your {}", provider.key_name())).ok_or("cancelled")?
    } else {
        let mut text = String::new();
        std::io::stdin().read_to_string(&mut text).map_err(|error| error.to_string())?;
        text
    };
    let key = key.trim().to_string();
    if key.is_empty() {
        return Err(format!("no {} given", provider.key_name()));
    }
    Ok(key)
}

fn connect(provider: Provider, endpoint: Option<String>) -> Outcome {
    let connection = Connection::new(provider, read_key(provider)?, endpoint);
    classifier::ping(&connection).map_err(|problem| format!("the key didn't work, so nothing was saved: {problem}"))?;
    connection.save()?;
    println!("Connected {} {}", ui::bold(provider.label()), ui::dim(&format!("(key {})", connection.key_hint())));
    println!(
        "{}",
        ui::dim("Reviewers a change can't concern are now cleared without a reviewer session. `reviewers classifier check` replays your recent commits to show what it would have skipped.")
    );
    Ok(0)
}

fn disconnect() -> Outcome {
    if classifier::disconnect()? {
        println!("Disconnected. Every Reviewer runs in full again.");
    } else {
        println!("No classifier was connected.");
    }
    Ok(0)
}

fn cutoff(value: Option<f64>) -> Outcome {
    let store = Store::open_default()?;
    match value {
        None => println!("{}", percent(default_cutoff(&store)?)),
        Some(value) if (0.0..=1.0).contains(&value) => {
            store.set_setting("classifier_cutoff", Some(&value.to_string()))?;
            println!("Default cutoff set to {}.", percent(value));
        }
        Some(_) => return Err("a cutoff goes from 0 to 1".into()),
    }
    Ok(0)
}

#[derive(Default)]
struct Tally {
    name: String,
    judged: u64,
    blocked: u64,
    cleared: u64,
    missed: u64,
    tokens_saved: u64,
    lowest_block: Option<f64>,
    cutoff: Option<f64>,
    /// Each judged decision as `[score, blocked, tokens]`, so any cutoff can be tried on the JSON.
    scores: Vec<(f64, bool, u64)>,
}

/// A decision a reviewer session actually reached: the classifier is measured against those only.
fn judged_by_a_session(decision: &crate::store::Decision) -> bool {
    decision.reused_from.is_none() && decision.usage.turns > 0
}

fn check(limit: u32, json_output: bool) -> Outcome {
    let store = Store::open_default()?;
    let connection = classifier::connected().ok_or(NOT_CONNECTED)?;
    let reviewers = store.reviewers()?;
    let runs: Vec<Run> = store
        .run_summaries(None, false, limit)?
        .iter()
        .filter_map(|summary| store.run(&summary.id, false).ok().flatten())
        .filter(|run| run.failure.is_none() && run.decisions.iter().any(judged_by_a_session))
        .collect();
    if runs.is_empty() {
        println!("No past commits with verdicts to replay yet.");
        return Ok(0);
    }
    let spinner = (!json_output).then(|| ui::Spinner::start(&format!("Asking the classifier about {}", plural(runs.len(), "past commit"))));
    let started = std::time::Instant::now();
    let answers: Vec<Result<classifier::Scores, String>> = runs
        .par_iter()
        .map(|run| {
            let questions: Vec<Question> = run
                .decisions
                .iter()
                .enumerate()
                .filter(|(_, decision)| judged_by_a_session(decision))
                .map(|(index, decision)| Question { id: format!("d{index}"), name: &decision.reviewer_name, instruction: &decision.instruction })
                .collect();
            classifier::score(&connection, "replay", &run.diff, &questions)
        })
        .collect();
    let mut tallies: BTreeMap<String, Tally> = BTreeMap::new();
    let (mut unanswered, mut classifier_tokens) = (0, 0);
    for (run, answer) in runs.iter().zip(&answers) {
        let Ok(scores) = answer else {
            unanswered += 1;
            continue;
        };
        classifier_tokens += scores.tokens;
        for (index, decision) in run.decisions.iter().enumerate() {
            let Some(probability) = scores.probabilities.get(&format!("d{index}")) else { continue };
            let current = reviewers.iter().find(|reviewer| reviewer.id == decision.reviewer_id);
            let tally = tallies.entry(decision.reviewer_id.clone()).or_insert_with(|| Tally {
                name: current.map(|reviewer| reviewer.name.clone()).unwrap_or_else(|| decision.reviewer_name.clone()),
                cutoff: current.and_then(|reviewer| classifier_cutoff(&store, reviewer)),
                ..Tally::default()
            });
            tally.judged += 1;
            let blocked = decision.verdict == Verdict::Blocked;
            tally.scores.push((*probability, blocked, decision.usage.tokens_read + decision.usage.tokens_written));
            let would_clear = tally.cutoff.is_some_and(|cutoff| *probability < cutoff);
            if blocked {
                tally.blocked += 1;
                tally.lowest_block = Some(tally.lowest_block.map_or(*probability, |lowest| lowest.min(*probability)));
            }
            if would_clear {
                tally.cleared += 1;
                if blocked {
                    tally.missed += 1;
                } else {
                    tally.tokens_saved += decision.usage.tokens_read + decision.usage.tokens_written;
                }
            }
        }
    }
    if let Some(spinner) = spinner {
        spinner.stop(&format!("Replayed {} in {}", plural(runs.len(), "commit"), crate::util::duration(started.elapsed().as_millis() as u64)));
    }
    let mut rows: Vec<&Tally> = tallies.values().collect();
    rows.sort_by(|a, b| b.tokens_saved.cmp(&a.tokens_saved));
    if json_output {
        return print_json(&json!({
            "provider": connection.provider,
            "commits": runs.len(),
            "unanswered": unanswered,
            "classifierTokens": classifier_tokens,
            "reviewers": rows.iter().map(|tally| json!({
                "name": tally.name, "judged": tally.judged, "blocked": tally.blocked, "cleared": tally.cleared,
                "missedBlocks": tally.missed, "tokensSaved": tally.tokens_saved, "cutoff": tally.cutoff, "lowestBlockScore": tally.lowest_block,
                "scores": tally.scores,
            })).collect::<Vec<_>>(),
        }));
    }
    let width = rows.iter().map(|tally| tally.name.chars().count()).max().unwrap_or(0).min(44) + 2;
    println!("{}", ui::dim(&format!("{}{:>8}{:>9}{:>8}{:>9}{:>8}{:>13}", ui::pad("", width), "judged", "cleared", "missed", "saved", "cutoff", "safe under")));
    for tally in &rows {
        let missed = if tally.missed > 0 { ui::red(&format!("{:>8}", tally.missed)) } else { format!("{:>8}", tally.missed) };
        println!(
            "{}{:>8}{:>9}{missed}{:>9}{:>8}{:>13}",
            ui::pad(&ui::truncate(&tally.name, width - 2), width),
            tally.judged,
            tally.cleared,
            compact(tally.tokens_saved),
            tally.cutoff.map(percent).unwrap_or_else(|| "off".into()),
            tally.lowest_block.map(percent).unwrap_or_else(|| "any".into()),
        );
    }
    let (judged, cleared, missed, saved) = rows.iter().fold((0, 0, 0, 0), |(j, c, m, s), tally| (j + tally.judged, c + tally.cleared, m + tally.missed, s + tally.tokens_saved));
    println!(
        "\nAt these cutoffs the classifier would have skipped {cleared} of {judged} sessions, saving {} tokens, and let {} through. It used {} tokens of its own.",
        compact(saved),
        plural(missed as usize, "block"),
        compact(classifier_tokens)
    );
    if unanswered > 0 {
        println!("{}", ui::dim(&format!("{} got no answer (too large, or an error) and would have run in full.", plural(unanswered, "commit"))));
    }
    println!(
        "{}",
        ui::dim("\"safe under\" is the highest cutoff that misses none of that Reviewer's past blocks. Set one with `reviewers edit <name> --classifier 0.1`, or `--classifier off`.")
    );
    Ok(0)
}
