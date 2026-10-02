use super::{Outcome, cwd, print_json, project_here, require_project_here};
use crate::store::{RunKind, Store, Verdict};
use crate::ui;
use crate::util::{compact, duration, plural, str_field};
use serde_json::{Value, json};

/// Past this many runs without a block, a Reviewer is probably checking something that doesn't happen, or that a linter could.
const NEVER_BLOCKS_AFTER: u64 = 50;

pub fn status(json_output: bool) -> Outcome {
    let store = Store::open_default()?;
    let project = require_project_here(&store)?;
    let reviewers = store.reviewers_for_project(&project.id)?;
    let runs = store.run_summaries(Some(&project.id), false, 5)?;
    let hooks = crate::hooks::state_of(std::path::Path::new(&project.root)).unwrap_or_default();
    if json_output {
        return print_json(&json!({
            "repo": project,
            "reviewers": reviewers.iter().map(|r| json!({ "name": r.name, "slug": r.slug, "enabled": r.enabled, "scope": r.scope })).collect::<Vec<_>>(),
            "hooks": hooks.iter().map(|(hook, state)| json!({ "hook": hook, "state": state.map(|state| format!("{state:?}").to_lowercase()) })).collect::<Vec<_>>(),
            "classifier": crate::classifier::connected().map(|connection| connection.provider),
            "recentRuns": runs,
        }));
    }
    let enabled = reviewers.iter().filter(|reviewer| reviewer.enabled).count();
    println!("{} {}", ui::bold(&project.name), ui::dim(&format!("· {} on · {} off", enabled, reviewers.len() - enabled)));
    if project.ignored {
        println!("{}", ui::yellow("Ignored: no Reviewer judges this repo. `reviewers init` turns them back on."));
    } else if !crate::hooks::covered(std::path::Path::new(&project.root)) {
        println!("{}", ui::yellow("The commit hook isn't installed here; `reviewers hooks install` fixes it, or `reviewers hooks install --global` for every repo."));
    }
    match crate::classifier::connected() {
        Some(connection) => println!("{}", ui::dim(&format!("Classifier: {} · `reviewers classifier` for its cutoffs", connection.provider.label()))),
        None => println!("{}", ui::dim("No classifier: every Reviewer runs a session. `reviewers help classifier`")),
    }
    println!();
    for reviewer in reviewers.iter().filter(|reviewer| reviewer.enabled) {
        println!("{} {}", ui::green("●"), reviewer.name);
    }
    if !runs.is_empty() {
        println!("\n{}", ui::dim("Latest reviews"));
        print_runs(&runs, false);
    }
    Ok(0)
}

fn print_runs(runs: &[crate::store::RunSummary], with_project: bool) {
    for run in runs {
        let mark = match (run.verdict, &run.failure) {
            (_, Some(_)) => ui::yellow("!"),
            (Verdict::Blocked, None) => ui::red("✗"),
            (Verdict::Approved, None) => ui::green("✓"),
        };
        let when = run.started_at.get(..16).unwrap_or(&run.started_at).replace('T', " ");
        let message = run.attempted_message.clone().unwrap_or_default();
        let outcome = if run.failure.is_some() {
            "no verdict".to_string()
        } else if run.verdict == Verdict::Blocked {
            format!("blocked by {}", run.blocked_by.join(", "))
        } else {
            plural(run.reviewers as usize, "Reviewer")
        };
        let project = if with_project { format!("{} · ", run.project) } else { String::new() };
        println!(
            "{mark} {} {} {}",
            ui::dim(&format!("{when} {}", run.id)),
            ui::truncate(&message, 52),
            ui::dim(&format!("· {project}{outcome} · {} · {} tokens{}", duration(run.duration_ms), compact(run.tokens), if run.kind == RunKind::Eval { " · eval" } else { "" }))
        );
    }
}

pub fn list(all: bool, evals: bool, limit: u32, json_output: bool) -> Outcome {
    let store = Store::open_default()?;
    let project = if all { None } else { Some(require_project_here(&store)?) };
    let runs = store.run_summaries(project.as_ref().map(|project| project.id.as_str()), evals, limit)?;
    if json_output {
        return print_json(&runs);
    }
    if runs.is_empty() {
        println!("No reviews yet.");
        return Ok(0);
    }
    print_runs(&runs, all);
    Ok(0)
}

fn print_session(session: &Value) {
    let Some(entries) = session.as_array() else {
        return;
    };
    for entry in entries {
        match str_field(entry, "kind").unwrap_or_default() {
            "prompt" => println!("    {}", ui::dim(&format!("prompt · {} characters", str_field(entry, "content").unwrap_or_default().len()))),
            "system" => println!("    {}", ui::dim(str_field(entry, "content").unwrap_or_default())),
            "thinking" => println!("    {} {}", ui::magenta("thinking"), ui::dim(&ui::truncate(&str_field(entry, "content").unwrap_or_default().replace('\n', " "), 160))),
            "tool" => println!("    {} {}", ui::cyan("tool"), str_field(entry, "detail").unwrap_or_default()),
            "reviewer" => println!("    {} {}", ui::bold("said"), ui::truncate(&str_field(entry, "content").unwrap_or_default().replace('\n', " "), 200)),
            "result" => println!("    {} {}", ui::bold("verdict"), str_field(entry, "verdict").unwrap_or_default()),
            _ => {}
        }
    }
}

pub fn show(id: &str, with_session: bool, json_output: bool) -> Outcome {
    let store = Store::open_default()?;
    let run = store.run(id, with_session)?.ok_or_else(|| format!("no run {id}"))?;
    if json_output {
        return print_json(&run);
    }
    let project = store.project(&run.project_id)?.map(|project| project.name).unwrap_or_default();
    let verdict = if run.failure.is_some() { ui::yellow("no verdict") } else if run.verdict == Verdict::Blocked { ui::red("blocked") } else { ui::green("approved") };
    println!("{} {} {}", ui::bold(&run.id), verdict, ui::dim(&format!("· {project} · {} · {}", run.started_at.replace('T', " ").get(..16).unwrap_or_default(), duration(run.duration_ms))));
    if let Some(message) = &run.attempted_message {
        println!("{}", ui::dim(&format!("\"{message}\"")));
    }
    if let Some(sha) = &run.commit_sha {
        println!("{}", ui::dim(&format!("landed as {} {}", &sha[..7.min(sha.len())], run.commit_message.clone().unwrap_or_default())));
    }
    if let Some(failure) = &run.failure {
        println!("\n{}\n{failure}", ui::yellow("Could not reach a verdict:"));
    }
    for decision in &run.decisions {
        let mark = match (decision.verdict, decision.advisory) {
            (Verdict::Blocked, false) => ui::red("✗"),
            (Verdict::Blocked, true) => ui::yellow("✗ advisory"),
            _ => ui::green("✓"),
        };
        println!(
            "\n{mark} {} {}",
            ui::bold(&decision.reviewer_name),
            ui::dim(&format!(
                "v{} · {}{}",
                decision.reviewer_version,
                match (&decision.reused_from, decision.classifier.as_ref().filter(|note| note.outcome == crate::classifier::Outcome::Cleared)) {
                    (Some(run), _) => format!("unchanged since {run}, verdict given back"),
                    (None, Some(note)) => format!(
                        "cleared by the classifier: {} under the {} cutoff",
                        crate::review::percent(note.probability.unwrap_or_default()),
                        crate::review::percent(note.cutoff)
                    ),
                    (None, None) => format!(
                        "{} · {} tokens · {} tool calls",
                        duration(decision.duration_ms),
                        compact(decision.usage.tokens_read + decision.usage.tokens_written),
                        decision.usage.tool_calls
                    ),
                },
                decision.model.as_ref().map(|model| format!(" · {model}")).unwrap_or_default()
            ))
        );
        println!("  {}", decision.summary);
        if decision.verdict == Verdict::Blocked || with_session {
            println!("  {}", ui::dim(&decision.reasoning));
        }
        for evidence in &decision.evidence {
            let line = evidence.start_line.map(|line| format!(":{line}")).unwrap_or_default();
            println!("  {}{line}  {}", ui::cyan(&evidence.file), ui::dim(&evidence.explanation));
        }
        if with_session {
            match &decision.reused_from {
                Some(run) => println!("  {}", ui::dim(&format!("Its session: `reviewers run {run} --session`"))),
                None => print_session(&decision.session),
            }
        }
    }
    if !with_session {
        println!("\n{}", ui::dim(&format!("Each Reviewer's whole session: `reviewers run {} --session`", run.id)));
    }
    Ok(0)
}

fn percentile(sorted: &[u64], fraction: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    sorted[((sorted.len() - 1) as f64 * fraction).round() as usize]
}

pub fn stats(all: bool, days: Option<u32>, json_output: bool) -> Outcome {
    let store = Store::open_default()?;
    let project = if all { None } else { project_here(&store, &cwd())? };
    if !all && project.is_none() {
        return Err("not inside a repo Reviewers judge; pass --all".into());
    }
    let since = days.map(|days| crate::util::to_iso(chrono::Utc::now() - chrono::Duration::days(days as i64)));
    let project_id = project.as_ref().map(|project| project.id.as_str());
    let records = store.reviewer_records(project_id, since.as_deref())?;
    let outcomes = store.run_outcomes(project_id, since.as_deref())?;
    let mut waits: Vec<u64> = outcomes.iter().filter(|(_, _, failed)| !failed).map(|(ms, _, _)| *ms).collect();
    waits.sort_unstable();
    let commits = outcomes.len();
    let blocked = outcomes.iter().filter(|(_, verdict, failed)| *verdict == Verdict::Blocked && !failed).count();
    let failures = outcomes.iter().filter(|(_, _, failed)| *failed).count();
    let tokens: u64 = records.iter().map(|record| record.tokens_read + record.tokens_written).sum();
    if json_output {
        return print_json(&json!({
            "scope": project.as_ref().map(|project| project.name.clone()).unwrap_or_else(|| "all repos".into()),
            "commits": commits, "blocked": blocked, "failures": failures,
            "medianWaitMs": percentile(&waits, 0.5), "p90WaitMs": percentile(&waits, 0.9),
            "tokens": tokens,
            "reviewers": records.iter().map(|record| json!({
                "id": record.reviewer_id, "name": record.reviewer_name, "runs": record.runs, "blocked": record.blocked,
                "blockRate": record.blocked as f64 / record.runs.max(1) as f64, "avgMs": record.avg_duration_ms,
                "tokensRead": record.tokens_read, "tokensWritten": record.tokens_written,
                "tokensPerCatch": (record.blocked > 0).then(|| (record.tokens_read + record.tokens_written) / record.blocked),
                "neverBlocks": record.blocked == 0 && record.runs >= NEVER_BLOCKS_AFTER,
            })).collect::<Vec<_>>(),
        }));
    }
    let scope_name = project.as_ref().map(|project| project.name.clone()).unwrap_or_else(|| "every repo".into());
    println!(
        "{} {}",
        ui::bold(&format!("{} commits reviewed", crate::util::thousands(commits as u64))),
        ui::dim(&format!(
            "· {scope_name}{} · {blocked} blocked ({:.0}%) · {failures} without a verdict · wait {} median, {} p90 · {} tokens",
            days.map(|days| format!(", last {days} days")).unwrap_or_default(),
            blocked as f64 * 100.0 / commits.max(1) as f64,
            duration(percentile(&waits, 0.5)),
            duration(percentile(&waits, 0.9)),
            compact(tokens)
        ))
    );
    if records.is_empty() {
        return Ok(0);
    }
    let width = records.iter().map(|record| record.reviewer_name.chars().count()).max().unwrap_or(0).min(44) + 2;
    println!("\n{}", ui::dim(&format!("{}{:>8} {:>8} {:>7} {:>9} {:>10} {:>11}", ui::pad("Reviewer", width), "runs", "blocks", "rate", "time", "tokens", "per catch")));
    for record in &records {
        let per_catch = if record.blocked > 0 { compact((record.tokens_read + record.tokens_written) / record.blocked) } else { "—".into() };
        let never = record.blocked == 0 && record.runs >= NEVER_BLOCKS_AFTER;
        println!(
            "{}{:>8} {:>8} {:>6.1}% {:>9} {:>10} {:>11}{}",
            ui::pad(&ui::truncate(&record.reviewer_name, width - 2), width),
            record.runs,
            record.blocked,
            record.blocked as f64 * 100.0 / record.runs.max(1) as f64,
            duration(record.avg_duration_ms),
            compact(record.tokens_read + record.tokens_written),
            per_catch,
            if never { ui::yellow("  never blocks") } else { String::new() }
        );
    }
    if records.iter().any(|record| record.blocked == 0 && record.runs >= NEVER_BLOCKS_AFTER) {
        println!("{}", ui::dim(&format!("\n\"never blocks\": no block in {NEVER_BLOCKS_AFTER}+ runs. A linter may do its job for free, or the rule no longer comes up.")));
    }
    Ok(0)
}

pub fn delete(id: &str) -> Outcome {
    let store = Store::open_default()?;
    if !store.delete_run(id)? {
        return Err(format!("no run {id}"));
    }
    println!("Deleted {id}");
    Ok(0)
}

pub fn prune_evals() -> Outcome {
    let store = Store::open_default()?;
    println!("Deleted {}", plural(store.prune_eval_runs()?, "eval run"));
    Ok(0)
}
