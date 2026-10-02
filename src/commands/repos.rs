use super::{Outcome, cwd, print_json, project_here};
use crate::hooks::{self, HookState};
use crate::store::Store;
use crate::{git, ui};
use clap::Subcommand;
use serde_json::json;
use std::path::{Path, PathBuf};

#[derive(Subcommand)]
pub enum HooksCommand {
    /// Install the hooks in this repo (or PATH), or in every repo with --all.
    Install {
        path: Option<PathBuf>,
        #[arg(long)]
        all: bool,
        /// Replace Personal Workspace's hooks.
        #[arg(long)]
        take_over: bool,
    },
    /// Which repos have the hooks.
    Status {
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
        println!("{} {}", ui::bold(&project.name), ui::dim(&format!("· {} · {}", crate::util::home_path(Path::new(&project.root)), crate::util::plural(reviewers, "Reviewer"))));
    }
    Ok(0)
}

pub fn print_hook_states(name: &str, states: &[(&str, HookState)]) {
    let ours = states.iter().all(|(_, state)| matches!(state, HookState::Installed | HookState::Updated | HookState::TookOver));
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
    let states = hooks::install(&main, false)?;
    print_hook_states(&project.name, &states);
    let reviewers = store.reviewers_for_project(&project.id)?.into_iter().filter(|reviewer| reviewer.enabled).count();
    if states.iter().any(|(_, state)| *state == HookState::Workspace) {
        println!("{}", ui::dim("Personal Workspace still runs here. `reviewers hooks install --take-over` switches this repo to reviewers."));
    }
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
        HooksCommand::Install { path, all, take_over } => {
            let targets: Vec<(String, PathBuf)> = if all {
                store.projects()?.into_iter().map(|project| (project.name, PathBuf::from(project.root))).collect()
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
                match hooks::install(&root, take_over) {
                    Ok(states) => print_hook_states(&name, &states),
                    Err(error) => println!("{} {name} {}", ui::red("✗"), ui::dim(&error)),
                }
            }
            Ok(0)
        }
        HooksCommand::Status { json } => {
            let mut rows = Vec::new();
            for project in store.projects()? {
                let states = hooks::state_of(Path::new(&project.root)).unwrap_or_default();
                rows.push((project.name, states));
            }
            if json {
                return print_json(&rows
                    .iter()
                    .map(|(name, states)| json!({ "repo": name, "hooks": states.iter().map(|(hook, state)| json!({ "hook": hook, "state": state.map(|state| format!("{state:?}").to_lowercase()) })).collect::<Vec<_>>() }))
                    .collect::<Vec<_>>());
            }
            for (name, states) in rows {
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
