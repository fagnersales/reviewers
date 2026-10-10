use crate::review::{self, EXIT_APPROVED, EXIT_FAILED};
use crate::store::{Project, Reviewer, RunKind, Scope, Store};
use crate::{diff, git, scope};
use std::path::{Path, PathBuf};
use std::time::Instant;

const MARKER: &str = "# managed by reviewers";
/// The hooks the TypeScript workspace installed; `reviewers import --hooks` takes them over.

pub const HOOKS: [&str; 2] = ["commit-msg", "post-commit"];

/// Every hook git looks for in a hooks folder. Under a global `core.hooksPath` git stops reading
/// each repo's own folder, so the global one carries them all, each handing over to the repo's
/// own. `push-to-checkout` and `proc-receive` are left out: their mere presence changes what git does.
const EVERY_HOOK: &[&str] = &[
    "applypatch-msg",
    "pre-applypatch",
    "post-applypatch",
    "pre-commit",
    "pre-merge-commit",
    "prepare-commit-msg",
    "commit-msg",
    "post-commit",
    "pre-rebase",
    "post-checkout",
    "post-merge",
    "pre-push",
    "pre-receive",
    "update",
    "post-receive",
    "post-update",
    "reference-transaction",
    "pre-auto-gc",
    "post-rewrite",
    "sendemail-validate",
    "post-index-change",
];

const PREVIOUS_GLOBAL_SETTING: &str = "previous_global_hooks_path";

/// Set in the git config of every repo Reviewers judge, so a commit knows it must be reviewed
/// even when Reviewers' own data can't be read.
const JUDGED_KEY: &str = "reviewers.judged";

pub fn mark_judged(root: &Path, judged: bool) -> Result<(), String> {
    git::set_local_config(root, JUDGED_KEY, judged.then_some("true"))
}

fn marked_judged(root: &Path) -> bool {
    git::local_config(root, JUDGED_KEY).as_deref() == Some("true")
}

/// Reviewers' data couldn't be read. A repo it judges stops the commit; under the global hooks
/// every other repo lets it through, so one broken file doesn't stop every commit on the machine.
fn cannot_read(root: &Path, error: &str) -> i32 {
    if marked_judged(root) {
        eprintln!("reviewers: {error}\nThe commit is stopped because it could not be reviewed. Skip once with REVIEWERS_BYPASS=1.");
        EXIT_FAILED
    } else {
        eprintln!("reviewers: {error}\nReviewers doesn't judge this repo, so the commit goes through.");
        EXIT_APPROVED
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookState {
    Installed,
    Updated,
    /// Another tool's hook is there; it was left alone.
    Foreign,
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
fn call_reviewers(hook: &str) -> String {
    let arguments = if hook == "commit-msg" { " \"$1\"" } else { "" };
    format!(
        "[ \"$REVIEWERS_BYPASS\" = \"1\" ] && exit 0\n\
REVIEWERS=\"{}\"\n\
[ -x \"$REVIEWERS\" ] || REVIEWERS=\"$(command -v reviewers)\"\n\
[ -n \"$REVIEWERS\" ] || {{ echo \"reviewers: not installed; this commit was not reviewed\" >&2; exit 0; }}\n\
exec \"$REVIEWERS\" hook {hook}{arguments}\n",
        binary_path()
    )
}

fn script(hook: &str) -> String {
    let purpose = match hook {
        "commit-msg" => "Reviewers judge every commit; a block stops it.",
        _ => "Ties the landed commit to the review that let it through.",
    };
    format!("#!/bin/sh\n{MARKER}: {purpose}\n# Reinstall with `reviewers hooks install`. Skip once with REVIEWERS_BYPASS=1.\n{}", call_reviewers(hook))
}

/// A hook in the global folder: it first runs the hook git would have run without it (the repo's
/// own, or the global folder that was set before), skipping a repo's own copy of Reviewers' hooks
/// so a commit isn't judged twice. The commit hooks then run Reviewers.
fn global_script(hook: &str, previous: Option<&str>) -> String {
    let own = match previous {
        Some(folder) => format!("{}/{hook}", folder.trim_end_matches('/')),
        None => format!("$(git rev-parse --git-common-dir 2>/dev/null)/hooks/{hook}"),
    };
    let header = format!(
        "#!/bin/sh\n{MARKER} (global hooks): a global core.hooksPath hides each repo's own hooks, so this runs\n\
# the {hook} hook git would have run. Remove with `reviewers hooks uninstall --global`.\n\
own=\"{own}\"\n"
    );
    let foreign = format!("[ -x \"$own\" ] && ! grep -q \"{MARKER}\" \"$own\" 2>/dev/null");
    match hook {
        "commit-msg" => format!("{header}if {foreign}; then\n  \"$own\" \"$@\" || exit $?\nfi\n{}", call_reviewers(hook)),
        "post-commit" => format!("{header}if {foreign}; then\n  \"$own\" \"$@\"\nfi\n{}", call_reviewers(hook)),
        _ => format!("{header}{foreign} && exec \"$own\" \"$@\"\nexit 0\n"),
    }
}

pub fn global_dir() -> PathBuf {
    crate::util::data_dir().join("hooks")
}

fn same_folder(path: &str, folder: &Path) -> bool {
    let path = Path::new(path);
    path == folder || path.canonicalize().ok().is_some_and(|path| folder.canonicalize().ok().is_some_and(|folder| path == folder))
}

/// Whether git's global `core.hooksPath` points at Reviewers' folder.
pub fn global_installed() -> bool {
    git::global_config("core.hooksPath").is_some_and(|path| same_folder(&path, &global_dir()))
}

pub struct GlobalInstall {
    /// The global hooks folder that was set before, still run by every hook.
    pub previous: Option<String>,
    pub updated: bool,
}

/// Points git's global `core.hooksPath` at Reviewers' folder, so every repo on the machine runs
/// Reviewers with no setup. A global hooks folder set before keeps running in place of each
/// repo's own hooks, as git did before; uninstalling sets it back.
pub fn install_global(store: &Store) -> Result<GlobalInstall, String> {
    let directory = global_dir();
    let current = git::global_config("core.hooksPath");
    let updated = current.as_deref().is_some_and(|path| same_folder(path, &directory));
    if let Some(path) = current.as_deref().filter(|_| !updated) {
        store.set_setting(PREVIOUS_GLOBAL_SETTING, Some(path))?;
    }
    let previous = store.setting(PREVIOUS_GLOBAL_SETTING)?;
    std::fs::create_dir_all(&directory).map_err(|error| format!("cannot create {}: {error}", directory.display()))?;
    for hook in EVERY_HOOK {
        write_executable(&directory.join(hook), &global_script(hook, previous.as_deref()))?;
    }
    git::set_global_config("core.hooksPath", Some(&directory.display().to_string()))?;
    Ok(GlobalInstall { previous, updated })
}

/// Sets git's global `core.hooksPath` back to what it was, and removes the folder.
pub fn uninstall_global(store: &Store) -> Result<bool, String> {
    let installed = global_installed();
    if installed {
        let previous = store.setting(PREVIOUS_GLOBAL_SETTING)?;
        git::set_global_config("core.hooksPath", previous.as_deref())?;
        store.set_setting(PREVIOUS_GLOBAL_SETTING, None)?;
    }
    if global_dir().exists() {
        std::fs::remove_dir_all(global_dir()).map_err(|error| format!("cannot remove {}: {error}", global_dir().display()))?;
    }
    Ok(installed)
}

pub enum Coverage {
    /// The global hooks reach it; nothing was written in the repo.
    Global,
    Repo(Vec<(&'static str, HookState)>),
}

/// Makes sure commits here reach Reviewers. The global hooks cover a repo unless it sets its own
/// hooks folder (`core.hooksPath`, as husky sets), which git prefers; then the hooks go in that folder.
pub fn cover(root: &Path) -> Result<Coverage, String> {
    mark_judged(root, true)?;
    if git::local_hooks_path(root).is_none() && global_installed() {
        return Ok(Coverage::Global);
    }
    install(root).map(Coverage::Repo)
}

/// Whether a commit here runs Reviewers.
pub fn covered(root: &Path) -> bool {
    if git::local_hooks_path(root).is_none() && global_installed() {
        return true;
    }
    state_of(root).is_ok_and(|states| states.iter().all(|(_, state)| *state == Some(HookState::Installed)))
}

/// Removes Reviewers' hooks from this repo's own hooks folder; another tool's are left alone.
pub fn uninstall(root: &Path) -> Result<Vec<&'static str>, String> {
    let directory = git::hooks_dir(root)?;
    let mut removed = Vec::new();
    for hook in HOOKS {
        let path = directory.join(hook);
        if std::fs::read_to_string(&path).is_ok_and(|content| content.contains(MARKER)) {
            std::fs::remove_file(&path).map_err(|error| format!("cannot remove {}: {error}", path.display()))?;
            removed.push(hook);
        }
    }
    Ok(removed)
}

pub fn state_of(root: &Path) -> Result<Vec<(&'static str, Option<HookState>)>, String> {
    let directory = git::hooks_dir(root)?;
    Ok(HOOKS
        .iter()
        .map(|hook| {
            let content = std::fs::read_to_string(directory.join(hook)).ok();
            let state = content.map(|content| {
                if content.contains(MARKER) { HookState::Installed } else { HookState::Foreign }
            });
            (*hook, state)
        })
        .collect())
}

pub fn install(root: &Path) -> Result<Vec<(&'static str, HookState)>, String> {
    mark_judged(root, true)?;
    let directory = git::hooks_dir(root)?;
    std::fs::create_dir_all(&directory).map_err(|error| format!("cannot create {}: {error}", directory.display()))?;
    let mut states = Vec::new();
    for hook in HOOKS {
        let path = directory.join(hook);
        let existing = std::fs::read_to_string(&path).ok();
        let state = match &existing {
            None => HookState::Installed,
            Some(content) if content.contains(MARKER) => HookState::Updated,
            Some(_) => HookState::Foreign,
        };
        if matches!(state, HookState::Installed | HookState::Updated) {
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

pub fn project_for(store: &Store, root: &Path) -> Result<Option<Project>, String> {
    if let Some(project) = store.project_by_root(&root.display().to_string())? {
        return Ok(Some(project));
    }
    store.project_by_root(&git::main_checkout(root).display().to_string())
}

/// Under the global hooks, a repo nobody added still answers to the Reviewers meant for every
/// repo. It's registered on its first such commit, so its reviews have a place in the history.
pub fn adopt(store: &Store, root: &Path) -> Result<Option<Project>, String> {
    if !store.reviewers()?.iter().any(|reviewer| reviewer.enabled && reviewer.scope == Scope::Everywhere) {
        return Ok(None);
    }
    let main = git::main_checkout(root);
    let project = store.ensure_project(&main.display().to_string(), git::remote_url(&main).as_deref())?;
    mark_judged(&main, true)?;
    eprintln!("reviewers: judging {} from now on; `reviewers ignore` in it turns that off", project.name);
    Ok(Some(project))
}

fn bypassed() -> bool {
    std::env::var("REVIEWERS_BYPASS").is_ok_and(|value| value == "1")
}

/// Said when no Reviewer runs only because the change is all text files, so the quiet commit isn't a mystery.
pub fn only_text_notice(judging: &[Reviewer], files: &[String], text_globs: &[String]) -> Option<String> {
    let all_text = !files.is_empty() && files.iter().all(|file| scope::is_text(file, text_globs));
    let would_run = judging.iter().any(|reviewer| reviewer.enabled && scope::applies(&reviewer.paths, true, files, text_globs));
    if !(all_text && would_run) {
        return None;
    }
    let mut kinds: Vec<String> = files.iter().filter_map(|file| file.rsplit_once('.').map(|(_, extension)| format!(".{extension}"))).collect();
    kinds.sort();
    kinds.dedup();
    Some(format!("reviewers: only text files changed ({}), nothing to judge; `reviewers text-files` says which files count", kinds.join(", ")))
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
        Err(error) => return cannot_read(&root, &error),
    };
    let project = match project_for(&store, &root).and_then(|found| match found {
        Some(project) => Ok(Some(project)),
        None => adopt(&store, &root),
    }) {
        Ok(Some(project)) if !project.ignored => project,
        Ok(_) => return EXIT_APPROVED,
        Err(error) => return cannot_read(&root, &error),
    };
    // Repos judged before the mark existed get it on their next commit.
    if !marked_judged(&root) {
        let _ = mark_judged(&root, true);
    }
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
    let (judging, text_globs) = match store.reviewers_for_project(&project.id).and_then(|judging| Ok((judging, store.text_globs()?))) {
        Ok(found) => found,
        Err(error) => {
            eprintln!("reviewers: {error}");
            return EXIT_FAILED;
        }
    };
    let reviewers: Vec<_> = judging.iter().filter(|reviewer| reviewer.enabled && scope::applies(&reviewer.paths, reviewer.reads_text, &files, &text_globs)).cloned().collect();
    if reviewers.is_empty() {
        if let Some(notice) = only_text_notice(&judging, &files, &text_globs) {
            eprintln!("{}", review::terminal::Paint::for_stderr().dim(&notice));
        }
        return EXIT_APPROVED;
    }
    let attempted = message_file.and_then(|file| git::attempted_message(&root.join(file)));
    let paint = review::terminal::Paint::for_stderr();
    eprintln!("reviewers: checking {} at once…", crate::util::plural(reviewers.len(), "Reviewer"));
    let started_at = crate::util::now_iso();
    let started = Instant::now();
    let classifier = crate::classifier::connected();
    let judged = review::judge_all(&store, &project, reviewers, &diff, &root, classifier.as_ref(), &mut |judged| {
        eprintln!("{}", review::terminal::progress(judged, &paint));
    });
    let unavailable = judged.iter().filter_map(|judged| judged.outcome.as_ref().ok()?.classifier.as_ref()?.problem.clone()).next();
    if let Some(problem) = unavailable {
        eprintln!("{}", paint.dim(&format!("reviewers: the classifier couldn't answer ({problem}), so those Reviewers ran in full")));
    }
    match review::record(&mut store, &project, RunKind::Review, judged, &diff, attempted, started_at, started) {
        Ok(reviewed) => {
            eprintln!("{}", review::terminal::report(&reviewed.run, &paint));
            if let Some(update) = crate::upgrade::available(&store) {
                eprintln!("{}", paint.dim(&update.line()));
            }
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
    if project.ignored {
        return 0;
    }
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
        HookState::Foreign => "left alone: another tool's hook is there",
    }
}
