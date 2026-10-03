use super::{Outcome, cwd, print_json, project_here};
use crate::hooks::{self, HookState};
use crate::store::Store;
use crate::{git, ui};
use clap::Subcommand;
use serde_json::json;
use std::path::{Path, PathBuf};

#[derive(Subcommand)]
pub enum HooksCommand {
    /// Install the hooks in this repo (or PATH), in every registered repo with --all, or for every repo on this machine with --global.
    Install {
        /// Any path inside the repo; defaults to the current directory.
        path: Option<PathBuf>,
        /// Every registered repo that isn't ignored.
        #[arg(long)]
        all: bool,
        /// Through git's global core.hooksPath: every repo without its own hooks folder, new ones included, with no setup. Each repo's own hooks still run first (or, if another global hooks folder was set before, that folder's).
        #[arg(long, conflicts_with_all = ["path", "all"])]
        global: bool,
    },
    /// Remove the hooks from this repo (or PATH), or the global ones with --global, setting back what was there before.
    Uninstall {
        /// Any path inside the repo; defaults to the current directory.
        path: Option<PathBuf>,
        /// The global hooks, setting git's global core.hooksPath back to what it was.
        #[arg(long, conflicts_with = "path")]
        global: bool,
    },
    /// Whether the global hooks are on, and how each registered repo is covered.
    Status {
        /// Print JSON instead of text, for agents and scripts.
        #[arg(long)]
        json: bool,
    },
}

pub fn list(json_output: bool) -> Outcome {
    let store = Store::open_default()?;
    let projects = store.projects()?;
    if json_output {
        return print_json(&projects);
    }
    if projects.is_empty() {
        println!("No repos yet. `reviewers init` in a repo starts judging it.");
    }
    for project in projects {
        let reviewers = store.reviewers_for_project(&project.id)?.iter().filter(|reviewer| reviewer.enabled).count();
        let judged = if project.ignored { "ignored".to_string() } else { crate::util::plural(reviewers, "Reviewer") };
        println!("{} {}", ui::bold(&project.name), ui::dim(&format!("· {} · {judged}", crate::util::home_path(Path::new(&project.root)))));
    }
    Ok(0)
}

pub fn print_hook_states(name: &str, states: &[(&str, HookState)]) {
    let ours = states.iter().all(|(_, state)| matches!(state, HookState::Installed | HookState::Updated));
    let mark = if ours { ui::green("✓") } else { ui::yellow("!") };
    let detail: Vec<String> = states.iter().map(|(hook, state)| format!("{hook} {}", hooks::describe(*state))).collect();
    println!("{mark} {} {}", ui::bold(name), ui::dim(&detail.join(" · ")));
}

pub fn init(path: Option<PathBuf>) -> Outcome {
    let start = path.unwrap_or_else(cwd);
    let root = git::root_of(&start).ok_or_else(|| format!("{} isn't inside a git repo", start.display()))?;
    let main = git::main_checkout(&root);
    let store = Store::open_default()?;
    let project = store.ensure_project(&main.display().to_string(), git::remote_url(&main).as_deref())?;
    if project.ignored {
        store.set_ignored(&project.id, false)?;
    }
    match hooks::cover(&main)? {
        hooks::Coverage::Global => println!("{} {} {}", ui::green("✓"), ui::bold(&project.name), ui::dim("· the global hooks cover it")),
        hooks::Coverage::Repo(states) => print_hook_states(&project.name, &states),
    }
    let reviewers = store.reviewers_for_project(&project.id)?.into_iter().filter(|reviewer| reviewer.enabled).count();
    println!(
        "{}",
        ui::dim(&if reviewers == 0 {
            "No Reviewer judges this repo yet: `reviewers onboard` suggests some, `reviewers new` writes one.".to_string()
        } else {
            format!("{} will judge every commit here.", crate::util::plural(reviewers, "Reviewer"))
        })
    );
    Ok(0)
}

pub fn hooks(command: HooksCommand) -> Outcome {
    let store = Store::open_default()?;
    match command {
        HooksCommand::Install { global: true, .. } => {
            let installed = hooks::install_global(&store)?;
            for project in store.projects()?.iter().filter(|project| !project.ignored && Path::new(&project.root).exists()) {
                let _ = hooks::mark_judged(Path::new(&project.root), true);
            }
            let verb = if installed.updated { "Updated" } else { "Installed" };
            println!("{} {verb} the global hooks {}", ui::green("✓"), ui::dim(&format!("· {}", crate::util::home_path(&hooks::global_dir()))));
            println!("{}", ui::dim("Every repo on this machine now runs Reviewers, and each repo's own hooks still run first."));
            if let Some(previous) = installed.previous {
                println!("{}", ui::dim(&format!("The global hooks set before ({previous}) still run, as they did.")));
            }
            let own_folder: Vec<String> = store.projects()?.into_iter().filter(|project| git::local_hooks_path(Path::new(&project.root)).is_some()).map(|project| project.name).collect();
            if !own_folder.is_empty() {
                println!("{}", ui::dim(&format!("These set their own hooks folder, which git prefers, so they keep their own install: {}", own_folder.join(", "))));
            }
            Ok(0)
        }
        HooksCommand::Uninstall { global: true, .. } => {
            if hooks::uninstall_global(&store)? {
                println!("Removed the global hooks; git's global core.hooksPath is back to what it was. Repos with their own install keep it.");
            } else {
                println!("The global hooks weren't installed.");
            }
            Ok(0)
        }
        HooksCommand::Uninstall { path, .. } => {
            let start = path.unwrap_or_else(cwd);
            let root = git::root_of(&start).ok_or_else(|| format!("{} isn't inside a git repo", start.display()))?;
            let removed = hooks::uninstall(&git::main_checkout(&root))?;
            println!("{}", if removed.is_empty() { "No Reviewers hooks in this repo.".to_string() } else { format!("Removed {}.", removed.join(" and ")) });
            if hooks::global_installed() {
                println!("{}", ui::dim("The global hooks still reach it; `reviewers ignore` stops Reviewers judging it."));
            }
            Ok(0)
        }
        HooksCommand::Install { path, all, .. } => {
            let targets: Vec<(String, PathBuf)> = if all {
                store.projects()?.into_iter().filter(|project| !project.ignored).map(|project| (project.name, PathBuf::from(project.root))).collect()
            } else {
                let start = path.unwrap_or_else(cwd);
                let project = project_here(&store, &start)?.ok_or("this repo isn't judged by Reviewers yet; run `reviewers init`")?;
                vec![(project.name, PathBuf::from(project.root))]
            };
            for (name, root) in targets {
                if !root.exists() {
                    println!("{} {} {}", ui::dim("·"), name, ui::dim("· folder is gone"));
                    continue;
                }
                match hooks::install(&root) {
                    Ok(states) => print_hook_states(&name, &states),
                    Err(error) => println!("{} {name} {}", ui::red("✗"), ui::dim(&error)),
                }
            }
            Ok(0)
        }
        HooksCommand::Status { json } => {
            let global = hooks::global_installed();
            if !json {
                println!(
                    "{} {}",
                    if global { ui::green("✓") } else { ui::dim("·") },
                    if global { "Global hooks on: every repo runs Reviewers, unless it sets its own hooks folder.".to_string() } else { "No global hooks: only these repos run Reviewers.".to_string() }
                );
            }
            let mut rows = Vec::new();
            for project in store.projects()? {
                let states = hooks::state_of(Path::new(&project.root)).unwrap_or_default();
                rows.push((project.name, states));
            }
            if json {
                return print_json(&rows
                    .iter()
                    .map(|(name, states)| json!({ "repo": name, "hooks": states.iter().map(|(hook, state)| json!({ "hook": hook, "state": state.map(|state| format!("{state:?}").to_lowercase()) })).collect::<Vec<_>>() }))
                    .collect::<Vec<_>>()
                    .into_iter()
                    .chain(std::iter::once(json!({ "global": global })))
                    .collect::<Vec<_>>());
            }
            for (project, (name, states)) in store.projects()?.iter().zip(rows) {
                let root = Path::new(&project.root);
                if project.ignored {
                    println!("{} {} {}", ui::dim("·"), ui::bold(&name), ui::dim("ignored"));
                    continue;
                }
                if global && git::local_hooks_path(root).is_none() {
                    println!("{} {} {}", ui::green("✓"), ui::bold(&name), ui::dim("covered by the global hooks"));
                    continue;
                }
                let text: Vec<String> = states
                    .iter()
                    .map(|(hook, state)| format!("{hook} {}", state.map(hooks::describe).unwrap_or("missing")))
                    .collect();
                let ours = states.iter().all(|(_, state)| *state == Some(HookState::Installed));
                println!("{} {} {}", if ours { ui::green("✓") } else { ui::yellow("!") }, ui::bold(&name), ui::dim(&text.join(" · ").replace("up to date", "installed")));
            }
            Ok(0)
        }
    }
}

pub fn model(model: Option<String>, repo: bool) -> Outcome {
    let store = Store::open_default()?;
    let value = model.as_deref().map(|model| if model == "default" { None } else { Some(model) });
    if repo {
        let project = super::require_project_here(&store)?;
        match value {
            Some(value) => {
                store.set_project_model(&project.id, value)?;
                println!("{}: {}", project.name, value.unwrap_or("the default model"));
            }
            None => println!("{}", project.model.unwrap_or_else(|| "(the default model)".into())),
        }
        return Ok(0);
    }
    match value {
        Some(value) => {
            store.set_setting("default_model", value)?;
            println!("Default model: {}", value.unwrap_or("Claude Code's own default"));
        }
        None => println!("{}", store.setting("default_model")?.unwrap_or_else(|| "Claude Code's own default".into())),
    }
    Ok(0)
}

/// No Reviewer judges this repo (or PATH) any more, even under the global hooks. It's registered
/// if it wasn't, so the choice is remembered; `reviewers init` undoes it.
pub fn ignore(path: Option<PathBuf>) -> Outcome {
    let start = path.unwrap_or_else(cwd);
    let root = git::root_of(&start).ok_or_else(|| format!("{} isn't inside a git repo", start.display()))?;
    let main = git::main_checkout(&root);
    let store = Store::open_default()?;
    let project = store.ensure_project(&main.display().to_string(), git::remote_url(&main).as_deref())?;
    store.set_ignored(&project.id, true)?;
    hooks::mark_judged(&main, false)?;
    println!("Reviewers won't judge {} any more. `reviewers init` here turns them back on.", ui::bold(&project.name));
    Ok(0)
}
