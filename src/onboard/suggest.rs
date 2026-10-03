//! `reviewers suggest`: onboarding again, over only what was typed since the
//! last read. A rule seen once waits in `pending` until it comes up again.

use super::*;
use crate::commands::suggest::SuggestArgs;
use crate::util::{data_dir, now_iso, parse_iso, to_iso};
use chrono::{DateTime, Utc};
use std::collections::{BTreeMap, HashSet};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Pending {
    root: PathBuf,
    /// When it was first found; past the reading window it's forgotten.
    found: String,
    #[serde(flatten)]
    rule: Rule,
}

/// What has been read, kept in `suggest/state.json` in the data folder.
#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    /// Every repo's sessions are read up to here.
    #[serde(default)]
    everything: Option<String>,
    /// Repos read further than `everything`, by root.
    #[serde(default)]
    repos: BTreeMap<String, String>,
    #[serde(default)]
    pending: Vec<Pending>,
}

impl State {
    fn path() -> PathBuf {
        data_dir().join("suggest").join("state.json")
    }

    pub fn load() -> State {
        std::fs::read_to_string(State::path()).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let path = State::path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
        }
        std::fs::write(&path, serde_json::to_string_pretty(self).unwrap_or_default()).map_err(|error| format!("cannot write {}: {error}", path.display()))
    }

    /// Marks these repos read up to `at`; with `every_repo`, every other repo too.
    pub fn read_through(mut self, roots: &[&Path], every_repo: bool, at: DateTime<Utc>) -> State {
        let at = to_iso(at);
        if every_repo {
            self.repos.retain(|_, through| *through > at);
            self.everything = Some(at);
        } else {
            for root in roots {
                self.repos.insert(root.display().to_string(), at.clone());
            }
        }
        self
    }

    /// Where reading starts for any repo: the latest of the window and the last full read.
    fn start(&self, window: &str) -> String {
        self.everything.as_deref().filter(|everything| *everything > window).unwrap_or(window).to_string()
    }

    /// Where reading starts for one repo, which may have been read further on its own.
    fn start_for(&self, root: &Path, window: &str) -> String {
        let start = self.start(window);
        self.repos.get(&root.display().to_string()).filter(|through| **through > start).cloned().unwrap_or(start)
    }
}

/// Each suggestion in full, for a log or an agent, where nothing can be opened.
fn explain(fresh: &[&Suggestion]) {
    for (index, suggestion) in fresh.iter().enumerate() {
        let place = if suggestion.everywhere { "everywhere".to_string() } else { suggestion.repos.join(", ") };
        ui::line("");
        ui::line(&format!("{} {}  {}", ui::green(&format!("{}.", index + 1)), ui::bold(&suggestion.name), ui::dim(&format!("{}× · {place}", suggestion.times_seen))));
        for (paragraph, text) in details(suggestion).iter().enumerate() {
            ui::paragraph(text, 3, if paragraph == 0 { |text: &str| text.to_string() } else { ui::dim });
        }
    }
}

pub fn run(args: SuggestArgs) -> Outcome {
    let interactive = ui::interactive() && !args.yes;
    let live = ui::stdout_is_tty();
    if !claude::is_installed() {
        return Err("suggestions come from Claude Code, and `claude` isn't on this machine's PATH".into());
    }
    let store = Store::open_default()?;
    let state = State::load();
    let started = Instant::now();
    let now = Utc::now();
    let window = to_iso(now - chrono::Duration::days(args.since as i64));
    let wanted: Option<Vec<String>> = args.repos.as_deref().map(crate::scope::parse_list);
    let from = if args.reread { window.clone() } else { state.start(&window) };

    ui::intro(&format!("{} {}", ui::bold(&format!("{} reviewers", ui::LOGO)), ui::dim("· suggest")));
    let spinner = ui::Spinner::start("Reading your new agent sessions");
    let progress = |done: usize, total: usize| {
        if done % 50 == 0 || done == total {
            spinner.message(&format!("Reading your new agent sessions · {} of {}", thousands(done as u64), thousands(total as u64)));
        }
    };
    let scan = transcripts::scan(parse_iso(&from).unwrap_or(now), &progress);
    let mut repos = scan.repos;
    for repo in &mut repos {
        if !args.reread {
            repo.since = state.start_for(&repo.root, &window);
        }
        let since = repo.since.clone();
        repo.messages.retain(|message| message.at > since);
    }
    repos.retain(|repo| !repo.messages.is_empty());
    spinner.message("Collecting commits");
    let summaries = digest::summarize(repos);
    let chosen: Vec<&RepoSummary> = summaries.iter().filter(|repo| wanted.as_ref().is_none_or(|wanted| wanted.contains(&repo.name))).collect();
    let messages: usize = chosen.iter().map(|repo| repo.messages.len()).sum();
    let earliest = chosen.iter().map(|repo| repo.since.as_str()).min().unwrap_or(&from);
    let since_label = if earliest > window.as_str() { format!("your last check, {}", digest::short_date(earliest)) } else { format!("the last {}", plural(args.since as usize, "day")) };
    spinner.stop(&format!("{} in {} since {since_label}", plural(messages, "new message"), plural(chosen.len(), "repo")));

    let chosen_roots: Vec<&Path> = chosen.iter().map(|repo| repo.root.as_path()).collect();
    let every_repo = wanted.is_none();
    let (pending, set_aside): (Vec<Pending>, Vec<Pending>) = state
        .pending
        .iter()
        .filter(|pending| pending.found >= window)
        .filter(|pending| !(args.reread && chosen_roots.contains(&pending.root.as_path())))
        .cloned()
        .partition(|pending| wanted.as_ref().is_none_or(|wanted| wanted.contains(&pending.rule.repo)));
    if chosen.is_empty() {
        state.read_through(&[], every_repo, now).save()?;
        ui::outro("Nothing new to read.");
        return Ok(0);
    }

    let model: Option<String> = args.model.clone().or_else(|| model_choices(&scan.models).first().map(|(id, _)| id.clone()));
    ui::step(&format!("{} · {}", plural(chosen.len(), "repo"), model.as_deref().map(model_label).unwrap_or_else(|| "your default model".into())));
    let directory = run_directory("suggest")?;
    let jobs = digest::write_jobs(&directory, &chosen, args.parallel.max(1))?;
    let (mut candidates, extract_tokens, failed) = extract(&jobs, args.parallel.max(1), model.as_deref(), live);

    // Where each report came from, so one that waits can be tied back to its repo.
    let mut origins: HashMap<String, (PathBuf, String)> = HashMap::new();
    let found_at = now_iso();
    for candidate in &candidates {
        if let Some(repo) = chosen.iter().find(|repo| repo.name == candidate.rule.repo) {
            origins.insert(candidate.id.clone(), (repo.root.clone(), found_at.clone()));
        }
    }
    let waiting = pending.len();
    for (index, pending) in pending.into_iter().enumerate() {
        let id = format!("p{}", index + 1);
        origins.insert(id.clone(), (pending.root, pending.found));
        candidates.push(Candidate { id, rule: pending.rule });
    }
    // A failed agent's repos are read again next time.
    let failed_roots: Vec<&Path> = failed.iter().flat_map(|&index| jobs[index].stretches.iter().map(|stretch| stretch.root.as_path())).collect();
    let read_roots: Vec<&Path> = chosen_roots.iter().copied().filter(|root| !failed_roots.contains(root)).collect();
    let state = state.read_through(&read_roots, every_repo && failed.is_empty(), now);
    let to_pending = |ids: &[&String], candidates: &[Candidate]| -> Vec<Pending> {
        ids.iter()
            .filter_map(|id| {
                let candidate = candidates.iter().find(|candidate| &candidate.id == *id)?;
                let (root, found) = origins.get(*id)?;
                Some(Pending { root: root.clone(), found: found.clone(), rule: candidate.rule.clone() })
            })
            .collect()
    };
    if candidates.len() == waiting {
        State { pending: set_aside.into_iter().chain(to_pending(&origins.keys().collect::<Vec<_>>(), &candidates)).collect(), ..state }.save()?;
        ui::outro(if failed.is_empty() { "No new rules in these sessions." } else { "No rules found; the sessions of the agents that failed are read again next time." });
        return Ok(if failed.is_empty() { 0 } else { 1 });
    }

    let existing = store.reviewers()?;
    let earlier = if waiting > 0 { format!(" with {waiting} waiting from earlier") } else { String::new() };
    let spinner = ui::Spinner::start(&format!("Merging {}{earlier}", plural(candidates.len() - waiting, "new rule")));
    let (planned, merge_tokens) = match plan_merge(&candidates, &existing, &directory, model.as_deref(), &spinner) {
        Ok((planned, tokens)) => {
            spinner.stop("Merged");
            (planned, tokens)
        }
        Err(error) => {
            spinner.fail(&format!("Couldn't merge ({error}); each rule stands alone"));
            (unmerged(&candidates), Tokens::default())
        }
    };
    let (suggestions, thin) = assemble(&planned, &candidates, args.max);
    write_outputs(&directory, &candidates, &planned, &suggestions)?;
    let names: HashSet<String> = existing.iter().map(|reviewer| reviewer.name.to_lowercase()).collect();
    let fresh: Vec<&Suggestion> = suggestions.iter().filter(|suggestion| !names.contains(&suggestion.name.to_lowercase())).collect();

    let mut waiting_ids: Vec<&String> = thin.iter().flatten().collect();
    if !fresh.is_empty() && !interactive {
        ui::line("");
        println!("{}  {}", ui::green(ui::STEP_DONE), ui::bold(&format!("{} to add", plural(fresh.len(), "Reviewer"))));
        explain(&fresh);
    }
    let picked: Vec<bool> = if fresh.is_empty() {
        Vec::new()
    } else if args.yes {
        vec![true; fresh.len()]
    } else if interactive {
        let rows: Vec<ui::PickRow> = fresh
            .iter()
            .map(|suggestion| ui::PickRow {
                label: suggestion.name.clone(),
                detail: format!("{:>4}  {}", format!("{}×", suggestion.times_seen), if suggestion.everywhere { "everywhere".to_string() } else { suggestion.repos.join(", ") }),
                selected: true,
                more: details(suggestion),
            })
            .collect();
        let question = format!("{} to add. Which should become Reviewers? The rest won't be suggested again.", plural(fresh.len(), "Reviewer"));
        match ui::multiselect(&question, &rows, "Reviewer", 0) {
            Some(indexes) => (0..fresh.len()).map(|index| indexes.contains(&index)).collect(),
            None => {
                waiting_ids.extend(fresh.iter().flat_map(|suggestion| &suggestion.sources));
                Vec::new()
            }
        }
    } else {
        waiting_ids.extend(fresh.iter().flat_map(|suggestion| &suggestion.sources));
        Vec::new()
    };
    State { pending: set_aside.into_iter().chain(to_pending(&waiting_ids, &candidates)).collect(), ..state }.save()?;

    let mut roots: HashMap<String, PathBuf> = chosen.iter().map(|repo| (repo.name.clone(), repo.root.clone())).collect();
    for (id, (root, _)) in &origins {
        if let Some(candidate) = candidates.iter().find(|candidate| &candidate.id == id) {
            roots.entry(candidate.rule.repo.clone()).or_insert_with(|| root.clone());
        }
    }
    let picks: Vec<(&Suggestion, bool)> = fresh.iter().copied().zip(picked.iter().copied()).filter(|(_, picked)| *picked).map(|(suggestion, _)| (suggestion, true)).collect();
    let added = save(&store, &picks, &roots, "suggest", &directory)?;

    ui::line("");
    if fresh.is_empty() {
        ui::paragraph("No new Reviewers to suggest: what came up is already checked, or not yet often enough.", 0, |text| text.to_string());
    } else if picked.is_empty() {
        ui::paragraph("Nothing added. These come back the next time `reviewers suggest` finds something new; `reviewers suggest --yes` adds them all.", 0, |text| text.to_string());
    } else {
        ui::paragraph(&format!("Added {}, on: {}", plural(added.len(), "Reviewer"), added.iter().map(|reviewer| reviewer.name.as_str()).collect::<Vec<_>>().join(", ")), 0, |text| text.to_string());
    }
    if !thin.is_empty() {
        ui::paragraph(&format!("{} seen once so far; suggested when it comes up again.", plural(thin.len(), "rule")), 0, ui::dim);
    }
    let tokens = extract_tokens + merge_tokens;
    ui::outro(&format!(
        "Saved to {}  {}",
        home_path(&directory),
        ui::dim(&format!("{} · {} tokens read · {} written", duration(started.elapsed().as_millis() as u64), compact(tokens.read), compact(tokens.written)))
    ));
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_where_the_last_read_stopped() {
        let window = "2026-07-01T00:00:00.000Z";
        let web = Path::new("/code/web");
        let api = Path::new("/code/api");
        let at = |text: &str| parse_iso(text).expect("a valid timestamp");

        let state = State::default();
        assert_eq!(state.start_for(web, window), window);

        let state = state.read_through(&[web], false, at("2026-09-01T00:00:00.000Z"));
        assert_eq!(state.start(window), window);
        assert_eq!(state.start_for(web, window), "2026-09-01T00:00:00.000Z");
        assert_eq!(state.start_for(api, window), window);

        let state = state.read_through(&[], true, at("2026-09-15T00:00:00.000Z"));
        assert!(state.repos.is_empty());
        assert_eq!(state.start_for(web, window), "2026-09-15T00:00:00.000Z");
        assert_eq!(state.start_for(api, window), "2026-09-15T00:00:00.000Z");
        assert_eq!(state.start("2026-09-20T00:00:00.000Z"), "2026-09-20T00:00:00.000Z");
    }
}
