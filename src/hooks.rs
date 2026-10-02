use crate::review::{self, EXIT_APPROVED, EXIT_FAILED};
use crate::store::{Project, RunKind, Store};
use crate::{diff, git, scope};
use std::path::{Path, PathBuf};
use std::time::Instant;

const MARKER: &str = "# managed by reviewers";
/// The hooks the TypeScript workspace installed; `reviewers import --hooks` takes them over.
const WORKSPACE_MARKER: &str = "# Personal Workspace hook";

pub const HOOKS: [&str; 2] = ["commit-msg", "post-commit"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookState {
    Installed,
    Updated,
    TookOver,
    /// Another tool's hook is there; it was left alone.
    Foreign,
    /// The old Personal Workspace hook is there; left alone unless taking over.
    Workspace,
}

fn binary_path() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.canonicalize().ok())
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "reviewers".to_string())
}

/// Git hands hooks a bare environment, so the binary is named by path, with
/// PATH as the fallback; a missing binary lets the commit through rather than
/// blocking every commit forever.
fn script(hook: &str) -> String {
    let (purpose, arguments) = match hook {
        "commit-msg" => ("Reviewers judge every commit; a block stops it.", " \"$1\""),
        _ => ("Ties the landed commit to the review that let it through.", ""),
    };
    format!(
        "#!/bin/sh\n{MARKER}: {purpose}\n# Reinstall with `reviewers hooks install`. Skip once with REVIEWERS_BYPASS=1.\n\
[ \"$REVIEWERS_BYPASS\" = \"1\" ] && exit 0\n\
REVIEWERS=\"{}\"\n\
[ -x \"$REVIEWERS\" ] || REVIEWERS=\"$(command -v reviewers)\"\n\
[ -n \"$REVIEWERS\" ] || {{ echo \"reviewers: not installed; this commit was not reviewed\" >&2; exit 0; }}\n\
exec \"$REVIEWERS\" hook {hook}{arguments}\n",
        binary_path()
    )
}

pub fn state_of(root: &Path) -> Result<Vec<(&'static str, Option<HookState>)>, String> {
    let directory = git::hooks_dir(root)?;
    Ok(HOOKS
        .iter()
        .map(|hook| {
            let content = std::fs::read_to_string(directory.join(hook)).ok();
            let state = content.map(|content| {
                if content.contains(MARKER) {
                    HookState::Installed
                } else if content.contains(WORKSPACE_MARKER) {
                    HookState::Workspace
                } else {
                    HookState::Foreign
                }
            });
            (*hook, state)
        })
        .collect())
}

pub fn install(root: &Path, take_over_workspace: bool) -> Result<Vec<(&'static str, HookState)>, String> {
    let directory = git::hooks_dir(root)?;
    std::fs::create_dir_all(&directory).map_err(|error| format!("cannot create {}: {error}", directory.display()))?;
    let mut states = Vec::new();
    for hook in HOOKS {
        let path = directory.join(hook);
        let existing = std::fs::read_to_string(&path).ok();
        let state = match &existing {
            None => HookState::Installed,
            Some(content) if content.contains(MARKER) => HookState::Updated,
            Some(content) if content.contains(WORKSPACE_MARKER) && take_over_workspace => HookState::TookOver,
            Some(content) if content.contains(WORKSPACE_MARKER) => HookState::Workspace,
            Some(_) => HookState::Foreign,
        };
        if matches!(state, HookState::Installed | HookState::Updated | HookState::TookOver) {
            write_executable(&path, &script(hook))?;
        }
        states.push((hook, state));
    }
    Ok(states)
}

fn write_executable(path: &PathBuf, content: &str) -> Result<(), String> {
    std::fs::write(path, content).map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).map_err(|error| format!("cannot make {} executable: {error}", path.display()))?;
    }
    Ok(())
}

fn project_for(store: &Store, root: &Path) -> Result<Option<Project>, String> {
    if let Some(project) = store.project_by_root(&root.display().to_string())? {
        return Ok(Some(project));
    }
    store.project_by_root(&git::main_checkout(root).display().to_string())
}

fn bypassed() -> bool {
    std::env::var("REVIEWERS_BYPASS").is_ok_and(|value| value == "1")
}

/// `commit-msg`: judge the staged change. Exit 0 lets the commit through.
pub fn commit_msg(message_file: Option<&str>) -> i32 {
    if bypassed() {
        return EXIT_APPROVED;
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let Some(root) = git::root_of(&cwd) else {
        return EXIT_APPROVED;
    };
    let mut store = match Store::open_default() {
        Ok(store) => store,
        Err(error) => {
            eprintln!("reviewers: {error}\nThe commit is stopped because it could not be reviewed. Skip once with REVIEWERS_BYPASS=1.");
            return EXIT_FAILED;
        }
    };
    let project = match project_for(&store, &root) {
        Ok(Some(project)) => project,
        Ok(None) => return EXIT_APPROVED,
        Err(error) => {
            eprintln!("reviewers: {error}");
            return EXIT_FAILED;
        }
    };
    let diff = match git::staged_diff(&root) {
        Ok(diff) => diff,
        Err(error) => {
            eprintln!("reviewers: {error}");
            return EXIT_FAILED;
        }
    };
    if !diff::has_reviewable_content(&diff) {
        return EXIT_APPROVED;
    }
    let files = diff::changed_paths(&diff);
    let reviewers: Vec<_> = match store.reviewers_for_project(&project.id) {
        Ok(reviewers) => reviewers.into_iter().filter(|reviewer| reviewer.enabled && scope::matches(&reviewer.paths, &files)).collect(),
        Err(error) => {
            eprintln!("reviewers: {error}");
            return EXIT_FAILED;
        }
    };
    if reviewers.is_empty() {
        return EXIT_APPROVED;
    }
    let attempted = message_file.and_then(|file| git::attempted_message(&root.join(file)));
    eprintln!("reviewers: checking {} at once…", crate::util::plural(reviewers.len(), "Reviewer"));
    let started_at = crate::util::now_iso();
    let started = Instant::now();
    let judged = review::judge_all(&store, &project, reviewers, &diff, &root, &mut |judged| match &judged.outcome {
        Ok(decision) => {
            let mark = if decision.verdict == crate::store::Verdict::Approved { "✓" } else { "✗" };
            eprintln!("  {mark} {} ({})", judged.reviewer.name, crate::util::duration(decision.duration_ms));
        }
        Err(_) => eprintln!("  ! {} (no verdict)", judged.reviewer.name),
    });
    match review::record(&mut store, &project, RunKind::Review, judged, &diff, attempted, started_at, started) {
        Ok(reviewed) => {
            eprintln!("{}", review::terminal::report(&reviewed.run));
            reviewed.exit_code
        }
        Err(error) => {
            eprintln!("reviewers: the review ran but could not be saved: {error}");
            EXIT_FAILED
        }
    }
}

/// `post-commit`: can't stop anything, so it never fails loudly.
pub fn post_commit() -> i32 {
    if bypassed() {
        return 0;
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let Some(root) = git::root_of(&cwd) else {
        return 0;
    };
    let Ok(store) = Store::open_default() else {
        return 0;
    };
    let Ok(Some(project)) = project_for(&store, &root) else {
        return 0;
    };
    let Ok(info) = git::commit_info(&root, "HEAD") else {
        return 0;
    };
    let Ok(commit_diff) = git::commit_diff(&root, &info.sha) else {
        return 0;
    };
    if let Err(error) = store.stamp_commit(&project.id, &diff::hash(&commit_diff), &info.sha, &info.subject, &info.committed_at) {
        eprintln!("reviewers: {error}");
    }
    0
}

pub fn describe(state: HookState) -> &'static str {
    match state {
        HookState::Installed => "installed",
        HookState::Updated => "up to date",
        HookState::TookOver => "took over from Personal Workspace",
        HookState::Foreign => "left alone: another tool's hook is there",
        HookState::Workspace => "left alone: Personal Workspace's hook is there",
    }
}
