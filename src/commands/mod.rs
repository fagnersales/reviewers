pub mod classifier;
pub mod onboard;
pub mod repos;
pub mod reviewers;
pub mod runs;
pub mod suggest;

use crate::store::{Project, Store};
use crate::{Command, git, ui};
use serde::Serialize;
use std::path::{Path, PathBuf};

pub type Outcome = Result<i32, String>;

pub fn print_json<T: Serialize>(value: &T) -> Outcome {
    println!("{}", serde_json::to_string_pretty(value).map_err(|error| error.to_string())?);
    Ok(0)
}

pub fn cwd() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// The registered repo this directory belongs to, worktrees included.
pub fn project_here(store: &Store, start: &Path) -> Result<Option<Project>, String> {
    let Some(root) = git::root_of(start) else {
        return Ok(None);
    };
    if let Some(project) = store.project_by_root(&root.display().to_string())? {
        return Ok(Some(project));
    }
    store.project_by_root(&git::main_checkout(&root).display().to_string())
}

pub fn require_project_here(store: &Store) -> Result<Project, String> {
    match project_here(store, &cwd())? {
        Some(project) => Ok(project),
        None => Err(match git::root_of(&cwd()) {
            Some(_) => "this repo isn't judged by Reviewers yet; run `reviewers init` here".to_string(),
            None => "not inside a git repo; pass --all, or run this inside a repo".to_string(),
        }),
    }
}

pub fn dispatch(command: Option<Command>) -> Outcome {
    match command {
        None => first_or_status(),
        Some(Command::Onboard(args)) => onboard::run(args),
        Some(Command::Suggest(args)) => suggest::run(args),
        Some(Command::Status(output)) => runs::status(output.json),
        Some(Command::List { all, output }) => reviewers::list(all, output.json),
        Some(Command::Show { reviewer, decisions, output }) => reviewers::show(&reviewer, decisions, output.json),
        Some(Command::New(args)) => reviewers::new(args),
        Some(Command::Edit(args)) => reviewers::edit(args),
        Some(Command::Enable { reviewers: names }) => reviewers::set_enabled(&names, true),
        Some(Command::Disable { reviewers: names }) => reviewers::set_enabled(&names, false),
        Some(Command::Remove { reviewer }) => reviewers::remove(&reviewer),
        Some(Command::Runs { prune_evals: true, .. }) => runs::prune_evals(),
        Some(Command::Runs { all, evals, limit, output, .. }) => runs::list(all, evals, limit, output.json),
        Some(Command::Run { id, delete: true, .. }) => runs::delete(&id),
        Some(Command::Run { id, session, output, .. }) => runs::show(&id, session, output.json),
        Some(Command::Stats { all, days, output }) => runs::stats(all, days, output.json),
        Some(Command::Case(command)) => crate::evals::case(command),
        Some(Command::Eval(args)) => crate::evals::eval(args),
        Some(Command::Evals { reviewer, limit, delete, output }) => crate::evals::history(&reviewer, limit, delete.as_deref(), output.json),
        Some(Command::Repos(output)) => repos::list(output.json),
        Some(Command::Init { path }) => repos::init(path),
        Some(Command::Ignore { path }) => repos::ignore(path),
        Some(Command::Hooks(command)) => repos::hooks(command),
        Some(Command::Model { model, repo }) => repos::model(model, repo),
        Some(Command::Context { context }) => onboard::context(context),
        Some(Command::Classifier { command }) => classifier::run(command),
        Some(Command::Skill(command)) => crate::skill::run(command),
        Some(Command::Upgrade(args)) => crate::upgrade::run(args),
        Some(Command::Help(args)) => crate::help::run(args),
        Some(Command::Hook(_)) => unreachable!("hooks are handled before dispatch"),
    }
}

/// Bare `reviewers`: onboarding the first time, this repo's status after.
fn first_or_status() -> Outcome {
    let store = Store::open_default()?;
    if store.reviewers()?.is_empty() {
        if ui::interactive() {
            return onboard::run(onboard::OnboardArgs::default());
        }
        println!("No Reviewers yet. Run `reviewers onboard` in a terminal, or `reviewers help`.");
        return Ok(0);
    }
    if project_here(&store, &cwd())?.is_some() {
        return runs::status(false);
    }
    crate::help::run(crate::help::HelpArgs::default())
}
