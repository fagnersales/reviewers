use super::{Outcome, cwd, print_json};
use crate::review::{self, terminal::Paint};
use crate::store::{Reviewer, RunKind, Store};
use crate::{diff, git, hooks, scope, ui};
use clap::Args;
use std::time::Instant;

#[derive(Args)]
pub struct CheckArgs {
    /// Only these Reviewers, by name, slug or id. Without any, every Reviewer the commit would run.
    pub reviewers: Vec<String>,
    /// Judge only what's staged, as `git commit` would, instead of everything `git add -A` would stage.
    #[arg(long)]
    pub staged: bool,
    #[command(flatten)]
    pub output: crate::Output,
}

/// Why a Reviewer that was asked for by name doesn't run on this change.
fn skipped(reviewer: &Reviewer, files: &[String], text_globs: &[String]) -> Option<String> {
    if !reviewer.enabled {
        return Some(format!("{} is off; `reviewers enable` turns it on", reviewer.name));
    }
    if !scope::applies(&reviewer.paths, reviewer.reads_text, files, text_globs) {
        if files.iter().all(|file| scope::is_text(file, text_globs)) {
            return Some(format!("{} doesn't run: only text files changed, and it doesn't read them", reviewer.name));
        }
        return Some(format!("{} doesn't run: the change touches none of {}", reviewer.name, reviewer.paths.join(", ")));
    }
    None
}

/// The commit hook's judgement, before the commit. A commit of exactly this change afterwards gets
/// these verdicts back without running anything.
pub fn run(args: CheckArgs) -> Outcome {
    let root = git::root_of(&cwd()).ok_or("not inside a git repo")?;
    let mut store = Store::open_default()?;
    let project = match hooks::project_for(&store, &root)? {
        Some(project) => project,
        None => hooks::adopt(&store, &root)?.ok_or("no Reviewer judges this repo; `reviewers init` starts judging it")?,
    };
    if project.ignored {
        return Err(format!("{} is ignored; `reviewers init` judges it again", project.name));
    }
    let diff = if args.staged { git::staged_diff(&root)? } else { git::pending_diff(&root)? };
    if !diff::has_reviewable_content(&diff) {
        println!("Nothing to check: {}.", if args.staged { "nothing is staged" } else { "no changes since the last commit" });
        return Ok(0);
    }
    let files = diff::changed_paths(&diff);
    let judging = store.reviewers_for_project(&project.id)?;
    let text_globs = store.text_globs()?;
    let reviewers: Vec<Reviewer> = if args.reviewers.is_empty() {
        judging.iter().filter(|reviewer| reviewer.enabled && scope::applies(&reviewer.paths, reviewer.reads_text, &files, &text_globs)).cloned().collect()
    } else {
        let mut picked: Vec<Reviewer> = Vec::new();
        for handle in &args.reviewers {
            let reviewer = store.find_reviewer(handle)?;
            if !judging.iter().any(|candidate| candidate.id == reviewer.id) {
                return Err(format!("{} doesn't judge {}; `reviewers edit` links it", reviewer.name, project.name));
            }
            match skipped(&reviewer, &files, &text_globs) {
                Some(reason) => eprintln!("{}", ui::dim(&format!("reviewers: {reason}"))),
                None if picked.iter().any(|already| already.id == reviewer.id) => {}
                None => picked.push(reviewer),
            }
        }
        picked
    };
    if reviewers.is_empty() {
        match hooks::only_text_notice(&judging, &files, &text_globs) {
            Some(_) => println!("Only text files changed, so no Reviewer runs and a commit of it goes through. `reviewers text-files` says which files count."),
            None => println!("No Reviewer runs on this change, so a commit of it goes through."),
        }
        return Ok(0);
    }
    let paint = Paint::for_stderr();
    eprintln!("reviewers: checking {} at once…", crate::util::plural(reviewers.len(), "Reviewer"));
    let started_at = crate::util::now_iso();
    let started = Instant::now();
    let classifier = crate::classifier::connected();
    let judged = review::judge_all(&store, &project, reviewers, &diff, &root, classifier.as_ref(), &mut |judged| {
        eprintln!("{}", review::terminal::progress(judged, &paint));
    });
    let reviewed = review::record(&mut store, &project, RunKind::Check, judged, &diff, None, started_at, started)?;
    if args.output.json {
        print_json(&reviewed.run)?;
    } else {
        println!("{}", review::terminal::report(&reviewed.run, &Paint::for_stdout()));
    }
    Ok(reviewed.exit_code)
}
