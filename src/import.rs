use crate::commands::Outcome;
use crate::store::{ClassifierUse, Reviewer, Scope, Store};
use crate::{hooks, ui};
use clap::Args;
use rusqlite::params;
use serde_json::{Value, json};
use std::path::PathBuf;

#[derive(Args)]
pub struct ImportArgs {
    /// Personal Workspace's database. Defaults to ~/apps/personalworkspace/.data/workspace.sqlite.
    #[arg(long)]
    pub from: Option<PathBuf>,
    /// Also switch every imported repo's git hooks from Personal Workspace to reviewers.
    #[arg(long)]
    pub hooks: bool,
}

fn default_source() -> PathBuf {
    crate::util::home().join("apps/personalworkspace/.data/workspace.sqlite")
}

fn count(store: &Store, table: &str) -> Result<i64, String> {
    store.connection().query_row(&format!("SELECT count(*) FROM {table}"), [], |row| row.get(0)).map_err(|error| error.to_string())
}

/// Copies everything that still means something without the web app. Ids are
/// kept, so running it twice changes nothing. Screen settings have no
/// equivalent yet and are kept on each Reviewer's origin.
pub fn run(args: ImportArgs) -> Outcome {
    let source = args.from.unwrap_or_else(default_source);
    if !source.exists() {
        return Err(format!("no Personal Workspace database at {}", source.display()));
    }
    let store = Store::open_default()?;
    let before: Vec<i64> = ["projects", "reviewers", "runs", "decisions", "cases", "eval_batches"].iter().map(|table| count(&store, table)).collect::<Result<_, _>>()?;
    let connection = store.connection();
    connection
        .execute("ATTACH DATABASE ?1 AS old", [format!("file:{}?mode=ro", source.display())])
        .map_err(|error| format!("cannot open {}: {error}", source.display()))?;

    let result = (|| -> Result<(), String> {
        let sql = |statement: &str| connection.execute_batch(statement).map_err(|error| format!("import: {error}"));
        sql("BEGIN")?;
        sql(
            "INSERT OR IGNORE INTO settings (key, value)
               SELECT 'default_model', default_model_id FROM old.workspace WHERE default_harness_id = 'claude-code';
             INSERT OR IGNORE INTO projects (id, name, root, remote, model, created_at)
               SELECT id, name, local_path, github_repo, CASE WHEN harness_id = 'claude-code' THEN model_id END, created_at FROM old.projects;",
        )?;
        let mut statement = connection
            .prepare(
                "SELECT id, project_id, name, instruction, enabled, blocking, path_scope, context_files_json, model_id, harness_id, version, created_at, updated_at, screen_json, description
                 FROM old.reviewers",
            )
            .map_err(|error| error.to_string())?;
        let rows: Vec<Reviewer> = statement
            .query_map([], |row| {
                let path_scope: Option<String> = row.get("path_scope")?;
                let screen: Option<Value> = row.get::<_, Option<String>>("screen_json")?.and_then(|text| serde_json::from_str(&text).ok());
                let harness: Option<String> = row.get("harness_id")?;
                Ok(Reviewer {
                    id: row.get("id")?,
                    slug: String::new(),
                    name: row.get("name")?,
                    instruction: row.get("instruction")?,
                    scope: Scope::Projects,
                    project_ids: vec![row.get("project_id")?],
                    paths: path_scope.as_deref().map(crate::scope::parse_list).unwrap_or_default(),
                    context_files: serde_json::from_str(&row.get::<_, String>("context_files_json")?).unwrap_or_default(),
                    enabled: row.get::<_, i64>("enabled")? != 0,
                    blocking: row.get::<_, i64>("blocking")? != 0,
                    model: if harness.as_deref() == Some("claude-code") { row.get("model_id")? } else { None },
                    version: row.get("version")?,
                    // A screen's cutoff scored single lines it extracted, not a whole change, so it doesn't carry over.
                    classifier: ClassifierUse::Default,
                    origin: json!({
                        "kind": "import",
                        "from": "personal-workspace",
                        "description": row.get::<_, String>("description")?,
                        "screen": screen,
                    }),
                    created_at: row.get("created_at")?,
                    updated_at: row.get("updated_at")?,
                })
            })
            .map_err(|error| error.to_string())?
            .collect::<rusqlite::Result<_>>()
            .map_err(|error| error.to_string())?;
        drop(statement);
        for reviewer in &rows {
            store.insert_reviewer(reviewer)?;
        }
        sql(
            "INSERT OR IGNORE INTO reviewer_projects (reviewer_id, project_id)
               SELECT reviewer_id, project_id FROM old.reviewer_projects
               WHERE reviewer_id IN (SELECT id FROM reviewers) AND project_id IN (SELECT id FROM projects);
             INSERT OR IGNORE INTO runs (id, project_id, kind, verdict, failure, attempted_message, diff, diff_hash, started_at, ended_at, duration_ms,
                 commit_sha, commit_message, committed_at, commit_match)
               SELECT id, project_id, kind, overall_verdict, infrastructure_error, attempted_message, diff, diff_hash, started_at, ended_at, duration_ms,
                 commit_sha, committed_message, committed_at, commit_match
               FROM old.review_runs WHERE project_id IN (SELECT id FROM projects);
             INSERT OR IGNORE INTO decisions (id, run_id, reviewer_id, reviewer_name, reviewer_version, instruction, verdict, summary, reasoning, evidence, session,
                 model, duration_ms, tokens_read, tokens_written, turns, tool_calls)
               SELECT id, run_id, reviewer_id, reviewer_name, reviewer_version, reviewer_instruction, verdict, summary, reasoning, evidence_json, conversation_json,
                 model_id, duration_ms,
                 coalesce(json_extract(usage_json, '$.inputTokens'), 0) + coalesce(json_extract(usage_json, '$.cacheReadTokens'), 0)
                   + coalesce(json_extract(usage_json, '$.cacheCreationTokens'), 0),
                 coalesce(json_extract(usage_json, '$.outputTokens'), 0),
                 coalesce(json_extract(usage_json, '$.turns'), 0),
                 coalesce(json_extract(usage_json, '$.toolCalls'), 0)
               FROM old.reviewer_decisions WHERE run_id IN (SELECT id FROM runs);
             INSERT OR IGNORE INTO cases (id, reviewer_id, name, expected, diff, diff_hash, files, origin, reviewer_version, created_at)
               SELECT id, reviewer_id, name, expected, diff, diff_hash, files_json, origin_json, reviewer_version, created_at
               FROM old.reviewer_cases WHERE reviewer_id IN (SELECT id FROM reviewers);
             INSERT OR IGNORE INTO eval_batches (id, reviewer_id, reviewer_version, instruction, model, started_at, ended_at, duration_ms, passed, failed, outcomes)
               SELECT id, reviewer_id, reviewer_version, reviewer_instruction, model_id, started_at, ended_at, duration_ms, passed, failed, outcomes_json
               FROM old.eval_batches WHERE reviewer_id IN (SELECT id FROM reviewers);",
        )?;
        sql("COMMIT")
    })();
    if result.is_err() {
        let _ = connection.execute_batch("ROLLBACK");
    }
    let _ = connection.execute("DETACH DATABASE old", params![]);
    result?;

    let after: Vec<i64> = ["projects", "reviewers", "runs", "decisions", "cases", "eval_batches"].iter().map(|table| count(&store, table)).collect::<Result<_, _>>()?;
    let labels = ["repos", "Reviewers", "runs", "decisions", "eval cases", "eval batches"];
    println!("{}", ui::bold("Imported from Personal Workspace"));
    for ((label, old), new) in labels.iter().zip(&before).zip(&after) {
        println!("  {} {}", ui::pad(&crate::util::thousands((new - old) as u64), 7), ui::dim(&format!("new {label} · {} in total", crate::util::thousands(*new as u64))));
    }
    if args.hooks {
        println!();
        for project in store.projects()? {
            let root = PathBuf::from(&project.root);
            if !root.exists() {
                continue;
            }
            match hooks::install(&root, true) {
                Ok(states) => crate::commands::repos::print_hook_states(&project.name, &states),
                Err(error) => println!("{} {} {}", ui::red("✗"), project.name, ui::dim(&error)),
            }
        }
    } else {
        println!("{}", ui::dim("\nYour repos still run Personal Workspace's hooks. `reviewers import --hooks` switches them to reviewers."));
    }
    let screened = store.reviewers()?.iter().filter(|reviewer| !reviewer.origin["screen"].is_null()).count();
    if screened > 0 && crate::classifier::connected().is_none() {
        println!(
            "{}",
            ui::dim(&format!("{screened} Reviewers had a screen. Connect a classifier to skip the commits they can't concern: `reviewers help classifier`."))
        );
    }
    Ok(0)
}
