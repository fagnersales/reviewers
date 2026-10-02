use super::{Outcome, cwd, print_json, project_here};
use crate::store::{ClassifierUse, NewReviewer, ReviewerChanges, Scope, Store, Verdict};
use crate::util::{compact, duration, plural};
use crate::{scope, ui};
use clap::Args;
use serde_json::json;

#[derive(Args)]
pub struct NewArgs {
    /// Short name, stated as the rule: "No type casts".
    #[arg(long)]
    pub name: String,
    /// The rule, as a brief for the judge: what to block, what's allowed, real examples.
    #[arg(long)]
    pub instruction: String,
    /// Judge every repo instead of just this one.
    #[arg(long)]
    pub everywhere: bool,
    /// Repos to judge (paths). Defaults to the repo you're in.
    #[arg(long = "repo")]
    pub repos: Vec<std::path::PathBuf>,
    /// Only run when the diff touches these globs, comma-separated: `convex/**,shared/**`.
    #[arg(long)]
    pub paths: Option<String>,
    /// Repo files attached to every review as reference, comma-separated.
    #[arg(long)]
    pub context_files: Option<String>,
    /// Model for this Reviewer only.
    #[arg(long)]
    pub model: Option<String>,
    /// When the classifier may skip it: `default`, `off`, or a cutoff from 0 to 1.
    #[arg(long, value_parser = classifier_use)]
    pub classifier: Option<ClassifierUse>,
    /// Create it turned off.
    #[arg(long)]
    pub disabled: bool,
}

#[derive(Args)]
pub struct EditArgs {
    /// Name, slug or id.
    pub reviewer: String,
    #[arg(long)]
    pub name: Option<String>,
    /// A new instruction is a new version.
    #[arg(long)]
    pub instruction: Option<String>,
    /// Comma-separated globs, or `none`.
    #[arg(long)]
    pub paths: Option<String>,
    /// Comma-separated files, or `none`.
    #[arg(long)]
    pub context_files: Option<String>,
    /// A model id, or `default` to inherit.
    #[arg(long)]
    pub model: Option<String>,
    /// Judge every repo.
    #[arg(long, conflicts_with = "repos_only")]
    pub everywhere: bool,
    /// Judge only linked repos.
    #[arg(long)]
    pub repos_only: bool,
    /// Link a repo (path).
    #[arg(long)]
    pub add_repo: Vec<std::path::PathBuf>,
    /// Unlink a repo (path).
    #[arg(long)]
    pub remove_repo: Vec<std::path::PathBuf>,
    /// Report without stopping the commit.
    #[arg(long)]
    pub advisory: bool,
    /// Stop the commit when it blocks (the default).
    #[arg(long, conflicts_with = "advisory")]
    pub blocking: bool,
    /// When the classifier may skip it: `default`, `off`, or a cutoff from 0 to 1.
    #[arg(long, value_parser = classifier_use)]
    pub classifier: Option<ClassifierUse>,
}

fn classifier_use(text: &str) -> Result<ClassifierUse, String> {
    ClassifierUse::parse(text).ok_or_else(|| "expected `default`, `off`, or a cutoff from 0 to 1".to_string())
}

fn project_ids_for(store: &Store, paths: &[std::path::PathBuf]) -> Result<Vec<String>, String> {
    let mut ids = Vec::new();
    for path in paths {
        let absolute = path.canonicalize().map_err(|error| format!("{}: {error}", path.display()))?;
        let project = project_here(store, &absolute)?.ok_or_else(|| format!("{} isn't a repo Reviewers judge; run `reviewers init {}` first", path.display(), path.display()))?;
        ids.push(project.id);
    }
    Ok(ids)
}

pub fn list(all: bool, json: bool) -> Outcome {
    let store = Store::open_default()?;
    let here = if all { None } else { project_here(&store, &cwd())? };
    let reviewers = match &here {
        Some(project) => store.reviewers_for_project(&project.id)?,
        None if all => store.reviewers()?,
        None => return Err("not inside a repo Reviewers judge; pass --all or run `reviewers init`".into()),
    };
    let records = store.reviewer_records(here.as_ref().map(|project| project.id.as_str()), None)?;
    let projects = store.projects()?;
    if json {
        let items: Vec<_> = reviewers
            .iter()
            .map(|reviewer| {
                let record = records.iter().find(|record| record.reviewer_id == reviewer.id);
                json!({
                    "id": reviewer.id, "slug": reviewer.slug, "name": reviewer.name, "enabled": reviewer.enabled,
                    "scope": reviewer.scope, "repos": reviewer.project_ids.iter().filter_map(|id| projects.iter().find(|p| &p.id == id).map(|p| p.name.clone())).collect::<Vec<_>>(),
                    "paths": reviewer.paths, "version": reviewer.version,
                    "runs": record.map(|r| r.runs).unwrap_or(0), "blocked": record.map(|r| r.blocked).unwrap_or(0),
                })
            })
            .collect();
        return print_json(&items);
    }
    if reviewers.is_empty() {
        println!("No Reviewers. `reviewers onboard` suggests them from your sessions; `reviewers new` writes one.");
        return Ok(0);
    }
    let width = reviewers.iter().map(|reviewer| reviewer.name.chars().count()).max().unwrap_or(0).min(46) + 2;
    for reviewer in &reviewers {
        let record = records.iter().find(|record| record.reviewer_id == reviewer.id);
        let mark = if reviewer.enabled { ui::green("●") } else { ui::dim("○") };
        let place = match reviewer.scope {
            Scope::Everywhere => "everywhere".to_string(),
            Scope::Projects => reviewer
                .project_ids
                .iter()
                .filter_map(|id| projects.iter().find(|project| &project.id == id).map(|project| project.name.clone()))
                .collect::<Vec<_>>()
                .join(", "),
        };
        let record_text = record.map(|record| format!("{} blocks in {} runs", record.blocked, record.runs)).unwrap_or_else(|| "no runs yet".into());
        let name = ui::pad(&ui::truncate(&reviewer.name, width - 2), width);
        let name = if reviewer.enabled { ui::bold(&name) } else { ui::dim(&name) };
        println!("{mark} {name}{}", ui::dim(&format!("{place:<22} {record_text}{}", if reviewer.enabled { "" } else { " · off" })));
    }
    println!("{}", ui::dim(&format!("\n{} · `reviewers show <name>` for one", plural(reviewers.len(), "Reviewer"))));
    Ok(0)
}

pub fn show(handle: &str, limit: u32, json: bool) -> Outcome {
    let store = Store::open_default()?;
    let reviewer = store.find_reviewer(handle)?;
    let records = store.reviewer_records(None, None)?;
    let record = records.iter().find(|record| record.reviewer_id == reviewer.id);
    let decisions = store.decisions_for(&reviewer.id, None, limit)?;
    let cases = store.cases(&reviewer.id, false)?;
    let projects = store.projects()?;
    let repos: Vec<String> = reviewer.project_ids.iter().filter_map(|id| projects.iter().find(|project| &project.id == id).map(|project| project.name.clone())).collect();
    if json {
        return print_json(&json!({
            "reviewer": reviewer,
            "repos": repos,
            "record": record,
            "cases": cases.len(),
            "recentDecisions": decisions.iter().map(|(decision, at, sha)| json!({
                "runId": decision.run_id, "at": at, "commit": sha, "verdict": decision.verdict, "summary": decision.summary,
                "evidence": decision.evidence, "tokens": decision.usage.tokens_read + decision.usage.tokens_written,
            })).collect::<Vec<_>>(),
        }));
    }
    println!("{} {}", ui::bold(&reviewer.name), ui::dim(&format!("· {} · v{}{}", reviewer.slug, reviewer.version, if reviewer.enabled { "" } else { " · off" })));
    let place = match reviewer.scope {
        Scope::Everywhere => "every repo".to_string(),
        Scope::Projects => repos.join(", "),
    };
    println!("{}", ui::dim(&format!(
        "Judges {place}{}{}",
        if reviewer.paths.is_empty() { String::new() } else { format!(" · only {}", reviewer.paths.join(", ")) },
        reviewer.model.as_ref().map(|model| format!(" · model {model}")).unwrap_or_default()
    )));
    println!("\n{}\n", reviewer.instruction);
    if let Some(why) = reviewer.origin.get("why").and_then(|why| why.as_str()) {
        println!("{} {why}\n", ui::dim("Why:"));
    }
    match record {
        Some(record) => println!(
            "Record: {} blocks in {} commits ({:.1}%) · {} per review · {} tokens per review{}",
            record.blocked,
            record.runs,
            record.blocked as f64 * 100.0 / record.runs.max(1) as f64,
            duration(record.avg_duration_ms),
            compact((record.tokens_read + record.tokens_written) / record.runs.max(1)),
            if record.blocked > 0 { format!(" · {} tokens per catch", compact((record.tokens_read + record.tokens_written) / record.blocked)) } else { String::new() }
        ),
        None => println!("Record: no commits judged yet"),
    }
    println!("Eval cases: {}", cases.len());
    if !decisions.is_empty() {
        println!();
        for (decision, at, sha) in &decisions {
            let mark = if decision.verdict == Verdict::Blocked { ui::red("✗") } else { ui::green("✓") };
            println!("{mark} {} {} {}", ui::dim(&at[..16.min(at.len())].replace('T', " ")), ui::dim(&sha.as_deref().map(|sha| &sha[..7.min(sha.len())]).unwrap_or("       ").to_string()), ui::truncate(&decision.summary, 110));
        }
    }
    Ok(0)
}

pub fn new(args: NewArgs) -> Outcome {
    let store = Store::open_default()?;
    let (scope_kind, project_ids) = if args.everywhere {
        (Scope::Everywhere, Vec::new())
    } else if !args.repos.is_empty() {
        (Scope::Projects, project_ids_for(&store, &args.repos)?)
    } else {
        let project = super::require_project_here(&store)?;
        (Scope::Projects, vec![project.id])
    };
    let reviewer = store.create_reviewer(NewReviewer {
        name: args.name.trim().to_string(),
        instruction: args.instruction.trim().to_string(),
        scope: scope_kind,
        project_ids,
        paths: args.paths.as_deref().map(scope::parse_list).unwrap_or_default(),
        context_files: args.context_files.as_deref().map(scope::parse_list).unwrap_or_default(),
        enabled: !args.disabled,
        model: args.model,
        classifier: args.classifier.unwrap_or(ClassifierUse::Default),
        origin: json!({ "kind": "manual" }),
    })?;
    println!("Created {} ({}){}", ui::bold(&reviewer.name), reviewer.slug, if reviewer.enabled { "" } else { ", turned off" });
    Ok(0)
}

pub fn edit(args: EditArgs) -> Outcome {
    let store = Store::open_default()?;
    let reviewer = store.find_reviewer(&args.reviewer)?;
    let scope_change = if args.everywhere {
        Some(Scope::Everywhere)
    } else if args.repos_only {
        Some(Scope::Projects)
    } else {
        None
    };
    let updated = store.update_reviewer(
        &reviewer.id,
        ReviewerChanges {
            name: args.name,
            instruction: args.instruction,
            paths: args.paths.as_deref().map(scope::parse_list),
            context_files: args.context_files.as_deref().map(scope::parse_list),
            model: args.model.map(|model| (model != "default").then_some(model)),
            blocking: if args.advisory { Some(false) } else if args.blocking { Some(true) } else { None },
            scope: scope_change,
            classifier: args.classifier,
        },
    )?;
    for id in project_ids_for(&store, &args.add_repo)? {
        store.link(&updated.id, &id)?;
    }
    for id in project_ids_for(&store, &args.remove_repo)? {
        store.unlink(&updated.id, &id)?;
    }
    let bumped = if updated.version != reviewer.version { format!(", now v{}", updated.version) } else { String::new() };
    println!("Updated {}{bumped}", ui::bold(&updated.name));
    Ok(0)
}

pub fn set_enabled(handles: &[String], enabled: bool) -> Outcome {
    let store = Store::open_default()?;
    for handle in handles {
        let reviewer = store.find_reviewer(handle)?;
        store.set_enabled(&reviewer.id, enabled)?;
        println!("{} {}", if enabled { ui::green("●") } else { ui::dim("○") }, reviewer.name);
    }
    Ok(0)
}

pub fn remove(handle: &str) -> Outcome {
    let store = Store::open_default()?;
    let reviewer = store.find_reviewer(handle)?;
    store.delete_reviewer(&reviewer.id)?;
    println!("Removed {}. Its past decisions stay in the history.", reviewer.name);
    Ok(0)
}
