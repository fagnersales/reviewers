use crate::commands::{Outcome, cwd, print_json};
use crate::store::{Case, EvalBatch, Reviewer, RunKind, SnapshotFile, Store, Verdict};
use crate::util::{compact, duration, new_id, now_iso};
use crate::{diff, git, review, scope, ui};
use clap::{Args, Subcommand, ValueEnum};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

/// A file larger than this is left out of a case; the Reviewer won't open it in an eval.
const SNAPSHOT_FILE_MAX_BYTES: usize = 256 * 1024;
/// How far back to look for a commit an old patch still applies to.
const BASE_SEARCH_MAX_COMMITS: usize = 400;
const PARALLEL_CASES: usize = 4;

#[derive(Clone, Copy, ValueEnum)]
pub enum Expect {
    Approved,
    Blocked,
}

impl From<Expect> for Verdict {
    fn from(expect: Expect) -> Verdict {
        match expect {
            Expect::Approved => Verdict::Approved,
            Expect::Blocked => Verdict::Blocked,
        }
    }
}

#[derive(Subcommand)]
pub enum CaseCommand {
    /// Capture a diff as a case: the working tree by default, or --staged, --from-run, --patch.
    Add(CaseAddArgs),
    /// A Reviewer's cases.
    List {
        /// Name, slug or id of the Reviewer.
        reviewer: String,
        /// Print JSON instead of text, for agents and scripts.
        #[arg(long)]
        json: bool,
    },
    /// Rename a case or flip its expected verdict.
    Update {
        /// The case id, from `reviewers case list`.
        case: String,
        /// A new name.
        #[arg(long)]
        name: Option<String>,
        #[arg(long, value_enum)]
        expect: Option<Expect>,
    },
    /// Delete a case. Past eval batches keep their record of it.
    Remove {
        /// The case id, from `reviewers case list`.
        case: String,
    },
}

#[derive(Args)]
pub struct CaseAddArgs {
    /// Name, slug or id of the Reviewer.
    pub reviewer: String,
    /// Kebab-case, describing the situation: `refactor-moves-guard`.
    #[arg(long)]
    pub name: String,
    #[arg(long, value_enum)]
    pub expect: Expect,
    /// Only what's staged, not the whole working tree.
    #[arg(long)]
    pub staged: bool,
    /// The diff a past review judged.
    #[arg(long)]
    pub from_run: Option<String>,
    /// A patch file.
    #[arg(long)]
    pub patch: Option<PathBuf>,
    /// The repo the diff belongs to. Defaults to the current directory.
    #[arg(long)]
    pub repo: Option<PathBuf>,
    /// Extra globs to include in the snapshot, comma-separated.
    #[arg(long)]
    pub include: Option<String>,
    /// Overwrite a case with the same name.
    #[arg(long)]
    pub replace: bool,
}

#[derive(Args)]
pub struct EvalArgs {
    /// Name, slug or id.
    pub reviewer: String,
    /// Only cases whose name contains this.
    #[arg(long)]
    pub only: Option<String>,
    /// Also keep each judged case as an eval run, so `reviewers run <id> --session` can show it.
    #[arg(long)]
    pub record: bool,
    /// Print JSON instead of text, for agents and scripts.
    #[arg(long)]
    pub json: bool,
}

fn temp_path(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!("reviewers-{label}-{}", new_id("x")))
}

/// Tracked changes against HEAD, plus untracked files as new-file hunks.
fn worktree_diff(root: &Path, staged: bool) -> Result<String, String> {
    if staged {
        return git::staged_diff(root);
    }
    let mut parts = vec![git::ok(root, &["diff", "HEAD", "--no-ext-diff"])?];
    let untracked = git::ok(root, &["ls-files", "--others", "--exclude-standard", "-z"])?;
    for file in untracked.split('\0').filter(|file| !file.is_empty()) {
        parts.push(git::run(root, &["diff", "--no-index", "--no-ext-diff", "--", "/dev/null", file], &[]).stdout);
    }
    Ok(parts.concat())
}

/// Whether the patch applies to the tree at `sha`, checked against a throwaway index.
fn applies_at(root: &Path, sha: &str, patch: &Path) -> bool {
    let index = temp_path("index");
    let index_text = index.display().to_string();
    let env = [("GIT_INDEX_FILE", index_text.as_str())];
    let patch_text = patch.display().to_string();
    let ok = git::run(root, &["read-tree", sha], &env).ok
        && git::run(root, &["apply", "--cached", "--check", "--whitespace=nowarn", &patch_text], &env).ok;
    let _ = std::fs::remove_file(&index);
    ok
}

/// `path → pre-image blob` from the diff's `index <old>..<new>` lines.
fn preimage_blobs(diff_text: &str) -> Vec<(String, String)> {
    let mut blobs = Vec::new();
    let mut current: Option<String> = None;
    for line in diff_text.lines() {
        if let Some(rest) = line.strip_prefix("diff --git a/") {
            current = rest.split(" b/").nth(1).map(str::to_string);
        } else if let Some(rest) = line.strip_prefix("index ") {
            let old = rest.split("..").next().unwrap_or_default();
            if let Some(path) = &current {
                if !old.is_empty() && !old.chars().all(|c| c == '0') {
                    blobs.push((path.clone(), old.to_string()));
                }
            }
        }
    }
    blobs
}

/// Commits on any ref where a touched file became exactly the blob the diff was cut from, newest first.
fn exact_preimage_commits(root: &Path, diff_text: &str) -> Vec<String> {
    let blobs = preimage_blobs(diff_text);
    if blobs.is_empty() {
        return Vec::new();
    }
    let mut args = vec!["log", "--all", "--format=%H", "--raw", "--no-abbrev", "--"];
    args.extend(blobs.iter().map(|(path, _)| path.as_str()));
    let raw = git::run(root, &args, &[]).stdout;
    let mut commits: Vec<String> = Vec::new();
    let mut sha = String::new();
    for line in raw.lines() {
        if line.len() == 40 && line.chars().all(|c| c.is_ascii_hexdigit()) {
            sha = line.to_string();
            continue;
        }
        let Some((meta, path)) = line.strip_prefix(':').and_then(|rest| rest.split_once('\t')) else {
            continue;
        };
        let new_blob = meta.split_whitespace().nth(3).unwrap_or_default();
        if blobs.iter().any(|(wanted_path, blob)| wanted_path == path && !new_blob.is_empty() && new_blob.starts_with(blob.as_str())) && !commits.contains(&sha) {
            commits.push(sha.clone());
        }
    }
    commits
}

/// The commit to snapshot for a diff not taken from the working tree just now.
fn find_base(root: &Path, diff_text: &str, preferred: &[String]) -> Result<(String, bool), String> {
    let patch = temp_path("patch");
    std::fs::write(&patch, diff_text).map_err(|error| error.to_string())?;
    let result = (|| {
        for sha in preferred {
            if applies_at(root, sha, &patch) {
                return Ok((sha.clone(), false));
            }
        }
        let paths = diff::changed_paths(diff_text);
        let count = BASE_SEARCH_MAX_COMMITS.to_string();
        let mut args = vec!["log", "--format=%H", "-n", count.as_str(), "HEAD", "--"];
        args.extend(paths.iter().map(String::as_str));
        let fallback: Vec<String> = git::run(root, &args, &[]).stdout.lines().map(str::to_string).collect();
        let mut tested: Vec<String> = preferred.to_vec();
        for sha in exact_preimage_commits(root, diff_text).into_iter().chain(fallback) {
            if tested.contains(&sha) {
                continue;
            }
            tested.push(sha.clone());
            if applies_at(root, &sha, &patch) {
                return Ok((sha, true));
            }
        }
        Err(format!(
            "the diff doesn't apply to HEAD, to any commit holding the files it was cut from, or to the last {BASE_SEARCH_MAX_COMMITS} commits touching its paths; it was probably taken on top of uncommitted changes"
        ))
    })();
    let _ = std::fs::remove_file(&patch);
    result
}

/// At `sha`: every file in the directories the diff touches, the Reviewer's reference files, and `include`.
fn snapshot(root: &Path, sha: &str, diff_text: &str, include: &[String], context_files: &[String]) -> Result<(Vec<SnapshotFile>, Vec<String>), String> {
    let tree = git::ok(root, &["ls-tree", "-r", "--name-only", "-z", sha])?;
    let directories: Vec<String> = diff::changed_paths(diff_text)
        .iter()
        .map(|file| Path::new(file).parent().map(|parent| parent.display().to_string()).unwrap_or_default())
        .collect();
    let patterns: Vec<_> = include.iter().map(|pattern| scope::glob_to_regex(pattern)).collect();
    let mut wanted: Vec<String> = context_files.to_vec();
    for file in tree.split('\0').filter(|file| !file.is_empty()) {
        let parent = Path::new(file).parent().map(|parent| parent.display().to_string()).unwrap_or_default();
        if (directories.contains(&parent) || patterns.iter().any(|pattern| pattern.is_match(file))) && !wanted.iter().any(|w| w == file) {
            wanted.push(file.to_string());
        }
    }
    wanted.sort();
    let mut files = Vec::new();
    let mut warnings = Vec::new();
    for file in wanted {
        let shown = git::run(root, &["show", &format!("{sha}:{file}")], &[]);
        if !shown.ok {
            if context_files.contains(&file) {
                warnings.push(format!("reference file {file} isn't in the repo at {}; the eval prompt will mark it missing", &sha[..7.min(sha.len())]));
            }
            continue;
        }
        if shown.stdout.len() > SNAPSHOT_FILE_MAX_BYTES {
            warnings.push(format!("{file} is {} KB, over the {} KB limit; left out", shown.stdout.len() / 1024, SNAPSHOT_FILE_MAX_BYTES / 1024));
            continue;
        }
        if shown.stdout.contains('\0') {
            continue;
        }
        files.push(SnapshotFile { path: file, content: shown.stdout });
    }
    Ok((files, warnings))
}

fn add_case(store: &Store, args: CaseAddArgs) -> Outcome {
    let reviewer = store.find_reviewer(&args.reviewer)?;
    let start = args.repo.clone().unwrap_or_else(cwd);
    let root = git::root_of(&start).ok_or_else(|| format!("{} isn't inside a git repo", start.display()))?;
    let mut warnings = Vec::new();
    let (diff_text, sha, origin) = if let Some(run_id) = &args.from_run {
        let run = store.run(run_id, false)?.ok_or_else(|| format!("no run {run_id}"))?;
        let mut preferred = Vec::new();
        if let Some(commit) = &run.commit_sha {
            if let Ok(parent) = git::ok(&root, &["rev-parse", "--verify", "--quiet", &format!("{commit}^")]) {
                preferred.push(parent.trim().to_string());
            }
        }
        preferred.push(git::head_sha(&root)?);
        let (sha, searched) = find_base(&root, &run.diff, &preferred)?;
        if searched {
            warnings.push(format!("the run's diff no longer applies to HEAD; snapshot taken at {}", &sha[..7]));
        }
        (run.diff, sha.clone(), json!({ "kind": "run", "runId": run_id, "repoPath": root, "sha": sha }))
    } else if let Some(patch) = &args.patch {
        let text = std::fs::read_to_string(patch).map_err(|error| format!("{}: {error}", patch.display()))?;
        let (sha, searched) = find_base(&root, &text, &[git::head_sha(&root)?])?;
        if searched {
            warnings.push(format!("the patch no longer applies to HEAD; snapshot taken at {}", &sha[..7]));
        }
        (text, sha.clone(), json!({ "kind": "patch", "file": patch.canonicalize().unwrap_or(patch.clone()), "repoPath": root, "sha": sha }))
    } else {
        let sha = git::head_sha(&root)?;
        (worktree_diff(&root, args.staged)?, sha.clone(), json!({ "kind": "worktree", "repoPath": root, "sha": sha }))
    };
    if diff_text.trim().is_empty() {
        return Err("nothing to capture: the diff is empty".into());
    }
    let include = args.include.as_deref().map(scope::parse_list).unwrap_or_default();
    let (files, snapshot_warnings) = snapshot(&root, &sha, &diff_text, &include, &reviewer.context_files)?;
    warnings.extend(snapshot_warnings);
    let existing = store.case_named(&reviewer.id, &args.name)?;
    if existing.is_some() && !args.replace {
        return Err(format!("{} already has a case named {}; pass --replace to overwrite it", reviewer.name, args.name));
    }
    let case = Case {
        id: existing.map(|case| case.id).unwrap_or_else(|| new_id("case")),
        reviewer_id: reviewer.id.clone(),
        name: args.name.clone(),
        expected: args.expect.into(),
        diff_hash: diff::hash(&diff_text),
        diff: diff_text,
        files,
        origin,
        reviewer_version: reviewer.version,
        created_at: now_iso(),
    };
    store.save_case(&case, true)?;
    println!("{} {} {}", ui::green("✓"), ui::bold(&case.name), ui::dim(&format!("· expects {} · {} files in the snapshot", case.expected.as_str(), case.files.len())));
    for warning in warnings {
        println!("  {}", ui::yellow(&warning));
    }
    Ok(0)
}

pub fn case(command: CaseCommand) -> Outcome {
    let store = Store::open_default()?;
    match command {
        CaseCommand::Add(args) => add_case(&store, args),
        CaseCommand::List { reviewer, json } => {
            let reviewer = store.find_reviewer(&reviewer)?;
            let cases = store.cases(&reviewer.id, false)?;
            if json {
                return print_json(&cases);
            }
            for case in &cases {
                println!("{} {} {}", ui::dim(&case.id), ui::bold(&case.name), ui::dim(&format!("· expects {} · v{}", case.expected.as_str(), case.reviewer_version)));
            }
            if cases.is_empty() {
                println!("No cases. `reviewers help evals` shows how to add them.");
            }
            Ok(0)
        }
        CaseCommand::Update { case, name, expect } => {
            store.case(&case)?.ok_or_else(|| format!("no case {case}"))?;
            store.update_case(&case, name.as_deref(), expect.map(Verdict::from))?;
            println!("Updated {case}");
            Ok(0)
        }
        CaseCommand::Remove { case } => {
            if !store.delete_case(&case)? {
                return Err(format!("no case {case}"));
            }
            println!("Removed {case}");
            Ok(0)
        }
    }
}

/// The case's own repository: the snapshot committed, the diff applied and staged, exactly what the hook would see.
fn materialize(case: &Case) -> Result<PathBuf, String> {
    let repo = temp_path("case");
    for file in &case.files {
        let target = repo.join(&file.path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        std::fs::write(&target, &file.content).map_err(|error| error.to_string())?;
    }
    std::fs::create_dir_all(&repo).map_err(|error| error.to_string())?;
    let identity = ["-c", "user.name=reviewers-eval", "-c", "user.email=eval@reviewers.local"];
    git::ok(&repo, &["init", "--quiet"])?;
    git::ok(&repo, &["add", "-A"])?;
    let message = format!("eval base: {}", case.name);
    let mut commit = identity.to_vec();
    commit.extend(["commit", "--quiet", "--allow-empty", "--no-verify", "-m", message.as_str()]);
    git::ok(&repo, &commit)?;
    let patch = temp_path("eval-patch");
    std::fs::write(&patch, &case.diff).map_err(|error| error.to_string())?;
    let applied = git::ok(&repo, &["apply", "--index", "--whitespace=nowarn", &patch.display().to_string()]);
    let _ = std::fs::remove_file(&patch);
    applied?;
    Ok(repo)
}

struct CaseResult {
    case: Case,
    actual: String,
    pass: bool,
    decision: Option<crate::store::Decision>,
    error: Option<String>,
    duration_ms: u64,
    run_id: Option<String>,
}

fn judge_case(store: &Mutex<Store>, reviewer: &Reviewer, model: Option<&str>, record_project: Option<&str>, case: Case) -> CaseResult {
    let started = Instant::now();
    let started_at = now_iso();
    let outcome = (|| -> Result<(Option<crate::store::Decision>, String), String> {
        let repo = materialize(&case)?;
        let result = (|| {
            let diff_text = git::staged_diff(&repo)?;
            if !scope::matches(&reviewer.paths, &diff::changed_paths(&diff_text)) {
                return Ok((None, diff_text));
            }
            review::judge(reviewer, model, "eval case", &diff_text, &repo).map(|decision| (Some(decision), diff_text))
        })();
        let _ = std::fs::remove_dir_all(&repo);
        result
    })();
    let duration_ms = started.elapsed().as_millis() as u64;
    match outcome {
        // A case the Reviewer never read proves nothing, even one that expects approval: otherwise
        // narrowing `--paths` would turn every approved case green without judging it.
        Ok((None, _)) => CaseResult {
            pass: false,
            actual: "not run".into(),
            error: Some("the case's files are outside this Reviewer's --paths, so it never judged it".into()),
            case,
            decision: None,
            duration_ms,
            run_id: None,
        },
        Ok((Some(decision), diff_text)) => {
            let mut run_id = None;
            if let Some(project_id) = record_project {
                let id = new_id("run");
                let mut recorded = decision.clone();
                recorded.run_id = id.clone();
                let run = crate::store::Run {
                    id: id.clone(),
                    project_id: project_id.to_string(),
                    kind: RunKind::Eval,
                    verdict: decision.verdict,
                    failure: None,
                    attempted_message: Some(format!("Eval: {} (expected {})", case.name, case.expected.as_str())),
                    diff_hash: diff::hash(&diff_text),
                    diff: diff_text,
                    started_at,
                    ended_at: now_iso(),
                    duration_ms,
                    commit_sha: None,
                    commit_message: None,
                    committed_at: None,
                    decisions: vec![recorded],
                };
                if store.lock().map(|mut store| store.record_run(&run)).is_ok() {
                    run_id = Some(id);
                }
            }
            CaseResult { pass: decision.verdict == case.expected, actual: decision.verdict.as_str().into(), case, decision: Some(decision), error: None, duration_ms, run_id }
        }
        Err(error) => CaseResult { pass: false, actual: "error".into(), case, decision: None, error: Some(error), duration_ms, run_id: None },
    }
}

fn print_case(result: &CaseResult) {
    let mark = if result.pass { ui::green("pass") } else { ui::red("FAIL") };
    let tokens = result.decision.as_ref().map(|decision| format!(" · {} tokens", compact(decision.usage.tokens_read + decision.usage.tokens_written))).unwrap_or_default();
    println!("[{mark}] {} {}", ui::bold(&result.case.name), ui::dim(&format!("— expected {}, got {} ({}{tokens})", result.case.expected.as_str(), result.actual, duration(result.duration_ms))));
    if let Some(error) = &result.error {
        println!("       {}", ui::red(error));
    }
    if let Some(decision) = &result.decision {
        println!("       {}", decision.summary);
        if !result.pass {
            println!("       {}", ui::dim(&decision.reasoning));
            for evidence in &decision.evidence {
                println!("       {}{}  {}", evidence.file, evidence.start_line.map(|line| format!(":{line}")).unwrap_or_default(), ui::dim(&evidence.explanation));
            }
        }
    }
}

pub fn eval(args: EvalArgs) -> Outcome {
    let store = Store::open_default()?;
    let reviewer = store.find_reviewer(&args.reviewer)?;
    let cases: Vec<Case> = store
        .cases(&reviewer.id, true)?
        .into_iter()
        .filter(|case| args.only.as_deref().is_none_or(|only| case.name.contains(only)))
        .collect();
    if cases.is_empty() {
        return Err(format!("{} has no cases{}; `reviewers help evals` shows how to add them", reviewer.name, if args.only.is_some() { " matching --only" } else { "" }));
    }
    let home_project = reviewer.project_ids.first().and_then(|id| store.project(id).ok().flatten());
    let model = home_project
        .as_ref()
        .map(|project| review::resolve_model(&store, project, &reviewer))
        .unwrap_or_else(|| reviewer.model.clone().or_else(|| store.setting("default_model").ok().flatten()));
    let record_project = if args.record { home_project.as_ref().map(|project| project.id.clone()).or_else(|| store.projects().ok().and_then(|projects| projects.first().map(|p| p.id.clone()))) } else { None };
    if !args.json {
        println!("{} {}", ui::bold(&format!("Evaluating {} v{}", reviewer.name, reviewer.version)), ui::dim(&format!("· {} · {} at a time", crate::util::plural(cases.len(), "case"), PARALLEL_CASES.min(cases.len()))));
    }
    let started_at = now_iso();
    let started = Instant::now();
    let shared = Mutex::new(store);
    let queue = Mutex::new(cases);
    let results = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..PARALLEL_CASES {
            scope.spawn(|| {
                loop {
                    let Some(case) = queue.lock().ok().and_then(|mut queue| queue.pop()) else {
                        break;
                    };
                    let result = judge_case(&shared, &reviewer, model.as_deref(), record_project.as_deref(), case);
                    if !args.json {
                        print_case(&result);
                    }
                    if let Ok(mut results) = results.lock() {
                        results.push(result);
                    }
                }
            });
        }
    });
    let results = results.into_inner().unwrap_or_default();
    let store = shared.into_inner().map_err(|_| "the store lock was poisoned".to_string())?;
    let passed = results.iter().filter(|result| result.pass).count() as u32;
    let failed = results.len() as u32 - passed;
    let outcomes: Vec<Value> = results
        .iter()
        .map(|result| {
            json!({
                "caseId": result.case.id, "caseName": result.case.name, "expected": result.case.expected, "actual": result.actual,
                "pass": result.pass, "summary": result.decision.as_ref().map(|decision| decision.summary.clone()), "error": result.error,
                "durationMs": result.duration_ms,
                "tokens": result.decision.as_ref().map(|decision| decision.usage.tokens_read + decision.usage.tokens_written),
                "runId": result.run_id,
            })
        })
        .collect();
    let batch = EvalBatch {
        id: new_id("evl"),
        reviewer_id: reviewer.id.clone(),
        reviewer_version: reviewer.version,
        instruction: reviewer.instruction.clone(),
        model: model.clone(),
        started_at,
        ended_at: now_iso(),
        duration_ms: started.elapsed().as_millis() as u64,
        passed,
        failed,
        outcomes: Value::Array(outcomes),
    };
    store.save_batch(&batch)?;
    let previous = store.batches(&reviewer.id, 2)?.into_iter().find(|other| other.id != batch.id);
    if args.json {
        print_json(&json!({ "batch": batch, "previous": previous }))?;
    } else {
        println!(
            "\n{} {}",
            ui::bold(&format!("{passed} of {} passed", passed + failed)),
            ui::dim(&format!("· v{} · {} · {}", batch.reviewer_version, duration(batch.duration_ms), batch.id))
        );
        if let Some(previous) = previous {
            let version = if previous.reviewer_version == batch.reviewer_version { String::new() } else { format!(" (v{})", previous.reviewer_version) };
            println!("{}", ui::dim(&format!("previous: {} of {} passed{version}", previous.passed, previous.passed + previous.failed)));
        }
    }
    Ok(if failed > 0 { 1 } else { 0 })
}

pub fn history(handle: &str, limit: u32, delete: Option<&str>, json_output: bool) -> Outcome {
    let store = Store::open_default()?;
    if let Some(id) = delete {
        if !store.delete_batch(id)? {
            return Err(format!("no eval batch {id}"));
        }
        println!("Deleted {id}");
        return Ok(0);
    }
    let reviewer = store.find_reviewer(handle)?;
    let batches = store.batches(&reviewer.id, limit)?;
    if json_output {
        return print_json(&batches);
    }
    if batches.is_empty() {
        println!("No evals yet for {}.", reviewer.name);
    }
    for batch in batches {
        let failing: Vec<String> = batch
            .outcomes
            .as_array()
            .map(|outcomes| outcomes.iter().filter(|outcome| outcome["pass"] == false).filter_map(|outcome| outcome["caseName"].as_str().map(str::to_string)).collect())
            .unwrap_or_default();
        let mark = if batch.failed == 0 { ui::green("✓") } else { ui::red("✗") };
        println!(
            "{mark} {} {} {}",
            ui::dim(&batch.started_at.get(..16).unwrap_or_default().replace('T', " ")),
            ui::bold(&format!("{} of {}", batch.passed, batch.passed + batch.failed)),
            ui::dim(&format!(
                "· v{} · {}{}",
                batch.reviewer_version,
                batch.model.clone().unwrap_or_else(|| "default model".into()),
                if failing.is_empty() { String::new() } else { format!(" · failing: {}", failing.join(", ")) }
            ))
        );
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_case_outside_the_paths_fails_without_running() {
        let directory = temp_path("test");
        std::fs::create_dir_all(&directory).unwrap();
        let store = Store::open(&directory.join("reviewers.sqlite")).unwrap();
        let reviewer = store
            .create_reviewer(crate::store::NewReviewer {
                name: "No type casts".into(),
                instruction: "Block `as` casts.".into(),
                scope: crate::store::Scope::Everywhere,
                project_ids: Vec::new(),
                paths: vec!["src/**".into()],
                context_files: Vec::new(),
                enabled: true,
                blocking: true,
                model: None,
                classifier: crate::store::ClassifierUse::Default,
                origin: json!({}),
            })
            .unwrap();
        let readme = "diff --git a/README.md b/README.md\nnew file mode 100644\n--- /dev/null\n+++ b/README.md\n@@ -0,0 +1 @@\n+hello\n";
        let case = Case {
            id: new_id("case"),
            reviewer_id: reviewer.id.clone(),
            name: "docs only".into(),
            expected: Verdict::Approved,
            diff: readme.into(),
            diff_hash: diff::hash(readme),
            files: Vec::new(),
            origin: json!({}),
            reviewer_version: reviewer.version,
            created_at: now_iso(),
        };
        let result = judge_case(&Mutex::new(store), &reviewer, None, None, case);
        assert!(!result.pass, "an approval nobody gave must not pass");
        assert_eq!(result.actual, "not run");
        assert!(result.decision.is_none());
        let _ = std::fs::remove_dir_all(&directory);
    }
}
