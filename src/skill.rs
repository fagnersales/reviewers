use crate::commands::{Outcome, print_json};
use crate::ui;
use crate::util::{home, home_path};
use clap::Subcommand;
use serde::Serialize;
use std::path::{Path, PathBuf};

const SKILL: &str = include_str!("skill/SKILL.md");
const MARKER: &str = "managed by reviewers";
const NAME: &str = "reviewers";

#[derive(Subcommand)]
pub enum SkillCommand {
    /// Find the coding agents on this machine and give each the skill.
    Install {
        #[arg(long)]
        json: bool,
    },
    /// Where the skill is, and which agents have it.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Take the skill away from every agent.
    Uninstall,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    pub agent: String,
    /// The agent's skills folder; the skill lands in `<dir>/reviewers`.
    pub dir: PathBuf,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Placement {
    pub agent: String,
    pub path: PathBuf,
    pub state: &'static str,
}

/// The one real copy; every agent's folder links here, the convention the `skills` CLI uses too.
fn canonical() -> PathBuf {
    home().join(".agents").join("skills").join(NAME)
}

fn config_home() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".config"))
}

/// A Claude Code config folder has settings or sessions; a stray `~/.claude-tmp` has neither.
fn is_claude_config(dir: &Path) -> bool {
    dir.join("settings.json").exists() || dir.join("projects").is_dir() || dir.join("skills").is_dir()
}

/// Every coding agent on this machine that reads skills from a folder.
pub fn targets() -> Vec<Target> {
    let mut claude_dirs: Vec<PathBuf> = vec![home().join(".claude")];
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR") {
        claude_dirs.push(PathBuf::from(dir));
    }
    if let Ok(entries) = std::fs::read_dir(home()) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(".claude-") && entry.path().is_dir() {
                claude_dirs.push(entry.path());
            }
        }
    }
    let mut targets = Vec::new();
    let mut seen = Vec::new();
    for dir in claude_dirs {
        let resolved = dir.canonicalize().unwrap_or(dir.clone());
        if is_claude_config(&dir) && !seen.contains(&resolved) {
            seen.push(resolved);
            targets.push(Target { agent: format!("Claude Code ({})", home_path(&dir)), dir: dir.join("skills") });
        }
    }
    let codex = std::env::var_os("CODEX_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".codex"));
    if codex.is_dir() {
        targets.push(Target { agent: "Codex".into(), dir: codex.join("skills") });
    }
    if home().join(".cursor").is_dir() {
        targets.push(Target { agent: "Cursor".into(), dir: home().join(".cursor").join("skills") });
    }
    if config_home().join("opencode").is_dir() {
        targets.push(Target { agent: "OpenCode".into(), dir: config_home().join("opencode").join("skills") });
    }
    targets
}

fn points_at_canonical(path: &Path) -> bool {
    std::fs::read_link(path).is_ok_and(|link| link == canonical() || path.canonicalize().ok() == canonical().canonicalize().ok())
}

fn is_ours(path: &Path) -> bool {
    std::fs::read_to_string(path.join("SKILL.md")).is_ok_and(|text| text.contains(MARKER))
}

fn link(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(canonical(), path)
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(path)?;
        std::fs::write(path.join("SKILL.md"), SKILL)
    }
}

/// Writes the real copy, then links it into every agent found. A `reviewers` skill someone else made is left alone.
pub fn install() -> Result<Vec<Placement>, String> {
    let source = canonical();
    std::fs::create_dir_all(&source).map_err(|error| format!("cannot create {}: {error}", source.display()))?;
    std::fs::write(source.join("SKILL.md"), SKILL).map_err(|error| format!("cannot write the skill: {error}"))?;
    let mut placements = vec![Placement { agent: "Shared skills folder".into(), path: source.clone(), state: "written" }];
    for target in targets() {
        let path = target.dir.join(NAME);
        let state = if points_at_canonical(&path) {
            "linked"
        } else if path.symlink_metadata().is_ok() && !is_ours(&path) {
            "left alone: another skill is named reviewers"
        } else {
            if path.symlink_metadata().is_ok() {
                let _ = std::fs::remove_file(&path).or_else(|_| std::fs::remove_dir_all(&path));
            }
            std::fs::create_dir_all(&target.dir).map_err(|error| format!("cannot create {}: {error}", target.dir.display()))?;
            match link(&path) {
                Ok(()) => "linked",
                Err(_) => "could not link",
            }
        };
        placements.push(Placement { agent: target.agent, path, state });
    }
    Ok(placements)
}

pub fn status() -> Vec<Placement> {
    let mut placements = vec![Placement {
        agent: "Shared skills folder".into(),
        path: canonical(),
        state: if canonical().join("SKILL.md").exists() { "written" } else { "missing" },
    }];
    for target in targets() {
        let path = target.dir.join(NAME);
        let state = if points_at_canonical(&path) || is_ours(&path) {
            "linked"
        } else if path.symlink_metadata().is_ok() {
            "another skill is named reviewers"
        } else {
            "missing"
        };
        placements.push(Placement { agent: target.agent, path, state });
    }
    placements
}

fn print(placements: &[Placement]) {
    for placement in placements {
        let mark = if placement.state == "linked" || placement.state == "written" { ui::green("✓") } else { ui::yellow("!") };
        println!("{mark} {} {}", placement.agent, ui::dim(&format!("· {} · {}", home_path(&placement.path), placement.state)));
    }
}

pub fn run(command: SkillCommand) -> Outcome {
    match command {
        SkillCommand::Install { json } => {
            let placements = install()?;
            if json {
                return print_json(&placements);
            }
            print(&placements);
            Ok(0)
        }
        SkillCommand::Status { json } => {
            let placements = status();
            if json {
                return print_json(&placements);
            }
            print(&placements);
            Ok(0)
        }
        SkillCommand::Uninstall => {
            for target in targets() {
                let path = target.dir.join(NAME);
                if points_at_canonical(&path) || is_ours(&path) {
                    let _ = std::fs::remove_file(&path).or_else(|_| std::fs::remove_dir_all(&path));
                    println!("{} {}", ui::dim("removed"), home_path(&path));
                }
            }
            let _ = std::fs::remove_dir_all(canonical());
            Ok(0)
        }
    }
}
