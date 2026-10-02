use crate::classifier::Classified;
use crate::util::{new_id, now_iso, slugify};
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub type Result<T> = std::result::Result<T, String>;

fn db_error(error: rusqlite::Error) -> String {
    format!("database: {error}")
}

const SCHEMA: &str = include_str!("schema.sql");
const SCHEMA_VERSION: i64 = 5;

/// Each step takes a database from the version before it to the next one.
const MIGRATIONS: &[(i64, &str)] = &[
    (
        2,
        "ALTER TABLE decisions ADD COLUMN input_hash TEXT;
         ALTER TABLE decisions ADD COLUMN reused_from TEXT;
         CREATE INDEX decisions_input ON decisions(input_hash);",
    ),
    (
        3,
        "ALTER TABLE reviewers ADD COLUMN classifier TEXT NOT NULL DEFAULT 'default';
         ALTER TABLE decisions ADD COLUMN classifier TEXT;",
    ),
    (4, "ALTER TABLE projects ADD COLUMN ignored INTEGER NOT NULL DEFAULT 0;"),
    (5, "ALTER TABLE decisions ADD COLUMN advisory INTEGER NOT NULL DEFAULT 0;"),
];

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Approved,
    Blocked,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Approved => "approved",
            Verdict::Blocked => "blocked",
        }
    }

    pub fn parse(text: &str) -> Option<Verdict> {
        match text {
            "approved" => Some(Verdict::Approved),
            "blocked" => Some(Verdict::Blocked),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Everywhere,
    Projects,
}

impl Scope {
    fn as_str(self) -> &'static str {
        match self {
            Scope::Everywhere => "everywhere",
            Scope::Projects => "projects",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RunKind {
    Review,
    Eval,
}

impl RunKind {
    fn as_str(self) -> &'static str {
        match self {
            RunKind::Review => "review",
            RunKind::Eval => "eval",
        }
    }
}

/// Whether the classifier may clear a Reviewer, and under what chance of a broken rule.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ClassifierUse {
    /// The cutoff set with `reviewers classifier cutoff`.
    Default,
    Off,
    Cutoff(f64),
}

impl ClassifierUse {
    pub fn parse(text: &str) -> Option<ClassifierUse> {
        match text.trim() {
            "default" => Some(ClassifierUse::Default),
            "off" => Some(ClassifierUse::Off),
            number => number.parse::<f64>().ok().filter(|cutoff| (0.0..=1.0).contains(cutoff)).map(ClassifierUse::Cutoff),
        }
    }

    pub fn as_text(self) -> String {
        match self {
            ClassifierUse::Default => "default".into(),
            ClassifierUse::Off => "off".into(),
            ClassifierUse::Cutoff(cutoff) => cutoff.to_string(),
        }
    }
}

impl Serialize for ClassifierUse {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.as_text())
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub name: String,
    pub root: String,
    pub remote: Option<String>,
    pub model: Option<String>,
    pub created_at: String,
    /// No Reviewer judges it (`reviewers ignore`).
    pub ignored: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Reviewer {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub instruction: String,
    pub scope: Scope,
    /// Linked repos; empty for `everywhere`.
    pub project_ids: Vec<String>,
    /// Globs; the Reviewer runs only when the diff touches one. Empty means every diff.
    pub paths: Vec<String>,
    pub context_files: Vec<String>,
    pub enabled: bool,
    pub blocking: bool,
    pub model: Option<String>,
    pub version: i64,
    pub classifier: ClassifierUse,
    /// Where it came from: onboarding evidence, an import, or a person.
    pub origin: Value,
    pub created_at: String,
    pub updated_at: String,
}

pub struct NewReviewer {
    pub name: String,
    pub instruction: String,
    pub scope: Scope,
    pub project_ids: Vec<String>,
    pub paths: Vec<String>,
    pub context_files: Vec<String>,
    pub enabled: bool,
    pub blocking: bool,
    pub model: Option<String>,
    pub classifier: ClassifierUse,
    pub origin: Value,
}

#[derive(Default)]
pub struct ReviewerChanges {
    pub name: Option<String>,
    pub instruction: Option<String>,
    pub paths: Option<Vec<String>>,
    pub context_files: Option<Vec<String>>,
    pub model: Option<Option<String>>,
    pub blocking: Option<bool>,
    pub scope: Option<Scope>,
    pub classifier: Option<ClassifierUse>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Evidence {
    pub file: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_line: Option<u32>,
    pub excerpt: String,
    pub explanation: String,
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    /// Everything the model took in, cache hits included.
    pub tokens_read: u64,
    pub tokens_written: u64,
    pub turns: u32,
    pub tool_calls: u32,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Decision {
    pub id: String,
    pub run_id: String,
    pub reviewer_id: String,
    pub reviewer_name: String,
    pub reviewer_version: i64,
    pub instruction: String,
    pub verdict: Verdict,
    pub summary: String,
    pub reasoning: String,
    pub evidence: Vec<Evidence>,
    /// The Reviewer's whole session: prompt, files it read, thinking, verdict.
    #[serde(skip_serializing_if = "Value::is_null")]
    pub session: Value,
    pub model: Option<String>,
    pub duration_ms: u64,
    pub usage: Usage,
    #[serde(skip)]
    pub input_hash: Option<String>,
    /// The run whose verdict this is, when the same input was judged there and the verdict was given back instead of running again.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reused_from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub classifier: Option<Classified>,
    /// Its Reviewer was advisory: a block was reported, and the commit went through.
    #[serde(skip_serializing_if = "is_false")]
    pub advisory: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    pub id: String,
    pub project_id: String,
    pub kind: RunKind,
    pub verdict: Verdict,
    pub failure: Option<String>,
    pub attempted_message: Option<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub diff: String,
    pub diff_hash: String,
    pub started_at: String,
    pub ended_at: String,
    pub duration_ms: u64,
    pub commit_sha: Option<String>,
    pub commit_message: Option<String>,
    pub committed_at: Option<String>,
    pub decisions: Vec<Decision>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunSummary {
    pub id: String,
    pub project: String,
    pub kind: RunKind,
    pub verdict: Verdict,
    pub failure: Option<String>,
    pub attempted_message: Option<String>,
    pub commit_sha: Option<String>,
    pub started_at: String,
    pub duration_ms: u64,
    pub reviewers: u32,
    pub blocked_by: Vec<String>,
    pub tokens: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotFile {
    pub path: String,
    pub content: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Case {
    pub id: String,
    pub reviewer_id: String,
    pub name: String,
    pub expected: Verdict,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub diff: String,
    pub diff_hash: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<SnapshotFile>,
    pub origin: Value,
    pub reviewer_version: i64,
    pub created_at: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvalBatch {
    pub id: String,
    pub reviewer_id: String,
    pub reviewer_version: i64,
    pub instruction: String,
    pub model: Option<String>,
    pub started_at: String,
    pub ended_at: String,
    pub duration_ms: u64,
    pub passed: u32,
    pub failed: u32,
    pub outcomes: Value,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewerRecord {
    pub reviewer_id: String,
    pub reviewer_name: String,
    pub runs: u64,
    pub blocked: u64,
    pub avg_duration_ms: u64,
    pub tokens_read: u64,
    pub tokens_written: u64,
}

pub enum StampResult {
    AlreadyStamped,
    Stamped,
    NoMatch,
}

/// A commit's own timestamp lands a little before the hook's run is recorded.
const STAMP_TIME_SLOP_MS: i64 = 5_000;

pub struct Store {
    connection: Connection,
}

pub fn default_path() -> PathBuf {
    crate::util::data_dir().join("reviewers.sqlite")
}

fn json_list(text: String) -> Vec<String> {
    serde_json::from_str(&text).unwrap_or_default()
}

fn to_json<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "null".to_string())
}

impl Store {
    pub fn open_default() -> Result<Store> {
        Store::open(&default_path())
    }

    pub fn open(path: &Path) -> Result<Store> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
        }
        let connection = Connection::open(path).map_err(db_error)?;
        // Hooks in several worktrees can commit at the same moment.
        connection.busy_timeout(Duration::from_secs(15)).map_err(db_error)?;
        connection
            .execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")
            .map_err(db_error)?;
        let store = Store { connection };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> Result<()> {
        let version: i64 = self
            .connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(db_error)?;
        if version == 0 {
            self.connection.execute_batch(SCHEMA).map_err(db_error)?;
            self.connection
                .execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION}"))
                .map_err(db_error)?;
            return Ok(());
        }
        if version > SCHEMA_VERSION {
            return Err(format!(
                "this database was written by a newer reviewers (schema {version}); run `reviewers upgrade`"
            ));
        }
        for (target, statements) in MIGRATIONS.iter().filter(|(target, _)| *target > version) {
            self.connection
                .execute_batch(&format!("BEGIN; {statements} PRAGMA user_version = {target}; COMMIT;"))
                .map_err(db_error)?;
        }
        Ok(())
    }

    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    // ── settings ──────────────────────────────────────────────────────────

    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        self.connection
            .query_row("SELECT value FROM settings WHERE key = ?", [key], |row| row.get(0))
            .optional()
            .map_err(db_error)
    }

    pub fn set_setting(&self, key: &str, value: Option<&str>) -> Result<()> {
        match value {
            Some(value) => self.connection.execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = ?2",
                params![key, value],
            ),
            None => self.connection.execute("DELETE FROM settings WHERE key = ?", [key]),
        }
        .map(|_| ())
        .map_err(db_error)
    }

    // ── projects ──────────────────────────────────────────────────────────

    fn project_row(row: &Row) -> rusqlite::Result<Project> {
        Ok(Project {
            id: row.get("id")?,
            name: row.get("name")?,
            root: row.get("root")?,
            remote: row.get("remote")?,
            model: row.get("model")?,
            created_at: row.get("created_at")?,
            ignored: row.get::<_, i64>("ignored")? != 0,
        })
    }

    pub fn projects(&self) -> Result<Vec<Project>> {
        let mut statement = self.connection.prepare("SELECT * FROM projects ORDER BY name").map_err(db_error)?;
        let rows = statement.query_map([], Store::project_row).map_err(db_error)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db_error)
    }

    pub fn project(&self, id: &str) -> Result<Option<Project>> {
        self.connection
            .query_row("SELECT * FROM projects WHERE id = ?", [id], Store::project_row)
            .optional()
            .map_err(db_error)
    }

    pub fn project_by_root(&self, root: &str) -> Result<Option<Project>> {
        self.connection
            .query_row("SELECT * FROM projects WHERE root = ?", [root], Store::project_row)
            .optional()
            .map_err(db_error)
    }

    /// Registers a repo, or returns it when it already is.
    pub fn ensure_project(&self, root: &str, remote: Option<&str>) -> Result<Project> {
        if let Some(existing) = self.project_by_root(root)? {
            return Ok(existing);
        }
        let name = Path::new(root).file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_else(|| root.to_string());
        let project = Project {
            id: new_id("prj"),
            name,
            root: root.to_string(),
            remote: remote.map(str::to_string),
            model: None,
            created_at: now_iso(),
            ignored: false,
        };
        self.insert_project(&project)?;
        Ok(project)
    }

    pub fn insert_project(&self, project: &Project) -> Result<()> {
        self.connection
            .execute(
                "INSERT OR IGNORE INTO projects (id, name, root, remote, model, created_at, ignored) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![project.id, project.name, project.root, project.remote, project.model, project.created_at, project.ignored as i64],
            )
            .map(|_| ())
            .map_err(db_error)
    }

    pub fn set_ignored(&self, id: &str, ignored: bool) -> Result<()> {
        self.connection.execute("UPDATE projects SET ignored = ?2 WHERE id = ?1", params![id, ignored as i64]).map(|_| ()).map_err(db_error)
    }

    pub fn set_project_model(&self, id: &str, model: Option<&str>) -> Result<()> {
        self.connection
            .execute("UPDATE projects SET model = ?2 WHERE id = ?1", params![id, model])
            .map(|_| ())
            .map_err(db_error)
    }

    // ── reviewers ─────────────────────────────────────────────────────────

    fn reviewer_row(row: &Row) -> rusqlite::Result<Reviewer> {
        let scope: String = row.get("scope")?;
        let origin: String = row.get("origin")?;
        Ok(Reviewer {
            id: row.get("id")?,
            slug: row.get("slug")?,
            name: row.get("name")?,
            instruction: row.get("instruction")?,
            scope: if scope == "everywhere" { Scope::Everywhere } else { Scope::Projects },
            project_ids: Vec::new(),
            paths: json_list(row.get("paths")?),
            context_files: json_list(row.get("context_files")?),
            enabled: row.get::<_, i64>("enabled")? != 0,
            blocking: row.get::<_, i64>("blocking")? != 0,
            model: row.get("model")?,
            version: row.get("version")?,
            classifier: ClassifierUse::parse(&row.get::<_, String>("classifier")?).unwrap_or(ClassifierUse::Default),
            origin: serde_json::from_str(&origin).unwrap_or(Value::Null),
            created_at: row.get("created_at")?,
            updated_at: row.get("updated_at")?,
        })
    }

    fn with_links(&self, mut reviewers: Vec<Reviewer>) -> Result<Vec<Reviewer>> {
        let mut statement = self
            .connection
            .prepare("SELECT project_id FROM reviewer_projects WHERE reviewer_id = ? ORDER BY project_id")
            .map_err(db_error)?;
        for reviewer in &mut reviewers {
            let rows = statement.query_map([&reviewer.id], |row| row.get::<_, String>(0)).map_err(db_error)?;
            reviewer.project_ids = rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db_error)?;
        }
        Ok(reviewers)
    }

    pub fn reviewers(&self) -> Result<Vec<Reviewer>> {
        let mut statement = self.connection.prepare("SELECT * FROM reviewers ORDER BY name COLLATE NOCASE").map_err(db_error)?;
        let rows = statement.query_map([], Store::reviewer_row).map_err(db_error)?;
        let reviewers = rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db_error)?;
        self.with_links(reviewers)
    }

    /// Everything that judges this repo: its own Reviewers and the `everywhere` ones.
    pub fn reviewers_for_project(&self, project_id: &str) -> Result<Vec<Reviewer>> {
        Ok(self
            .reviewers()?
            .into_iter()
            .filter(|reviewer| reviewer.scope == Scope::Everywhere || reviewer.project_ids.iter().any(|id| id == project_id))
            .collect())
    }

    pub fn reviewer(&self, id: &str) -> Result<Option<Reviewer>> {
        let found = self
            .connection
            .query_row("SELECT * FROM reviewers WHERE id = ?", [id], Store::reviewer_row)
            .optional()
            .map_err(db_error)?;
        Ok(self.with_links(found.into_iter().collect())?.pop())
    }

    /// An id, a slug, a name, or the unique start of a slug.
    pub fn find_reviewer(&self, handle: &str) -> Result<Reviewer> {
        let all = self.reviewers()?;
        let wanted = handle.trim();
        let lowered = wanted.to_lowercase();
        if let Some(found) = all
            .iter()
            .find(|reviewer| reviewer.id == wanted || reviewer.slug == lowered || reviewer.name.to_lowercase() == lowered)
        {
            return Ok(found.clone());
        }
        let slug = slugify(wanted);
        let prefixed: Vec<&Reviewer> = all.iter().filter(|reviewer| !slug.is_empty() && reviewer.slug.starts_with(&slug)).collect();
        // Then any unique part of the name: `punctuation` finds "Don't mark punctuation as errors".
        let matches = if prefixed.is_empty() {
            all.iter().filter(|reviewer| !slug.is_empty() && reviewer.slug.contains(&slug)).collect()
        } else {
            prefixed
        };
        match matches.as_slice() {
            [only] => Ok((*only).clone()),
            [] => Err(format!("no Reviewer matches \"{wanted}\"; `reviewers list --all` shows them")),
            many => Err(format!(
                "\"{wanted}\" matches {} Reviewers: {}",
                many.len(),
                many.iter().map(|reviewer| reviewer.slug.as_str()).collect::<Vec<_>>().join(", ")
            )),
        }
    }

    fn free_slug(&self, name: &str, except: Option<&str>) -> Result<String> {
        let base = match slugify(name) {
            slug if slug.is_empty() => "reviewer".to_string(),
            slug => slug,
        };
        for attempt in 1.. {
            let candidate = if attempt == 1 { base.clone() } else { format!("{base}-{attempt}") };
            let owner: Option<String> = self
                .connection
                .query_row("SELECT id FROM reviewers WHERE slug = ?", [&candidate], |row| row.get(0))
                .optional()
                .map_err(db_error)?;
            if owner.is_none() || owner.as_deref() == except {
                return Ok(candidate);
            }
        }
        unreachable!("the loop returns once a slug is free")
    }

    pub fn create_reviewer(&self, input: NewReviewer) -> Result<Reviewer> {
        let id = new_id("rev");
        let now = now_iso();
        let slug = self.free_slug(&input.name, None)?;
        self.connection
            .execute(
                "INSERT INTO reviewers (id, slug, name, instruction, scope, paths, context_files, enabled, blocking, model, version, origin, created_at, updated_at, classifier)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?13, ?9, 1, ?10, ?11, ?11, ?12)",
                params![
                    id,
                    slug,
                    input.name,
                    input.instruction,
                    input.scope.as_str(),
                    to_json(&input.paths),
                    to_json(&input.context_files),
                    input.enabled as i64,
                    input.model,
                    to_json(&input.origin),
                    now,
                    input.classifier.as_text(),
                    input.blocking as i64
                ],
            )
            .map_err(db_error)?;
        for project_id in &input.project_ids {
            self.link(&id, project_id)?;
        }
        self.reviewer(&id)?.ok_or_else(|| "the new Reviewer could not be read back".to_string())
    }

    /// Keeps an imported Reviewer's id and history intact.
    pub fn insert_reviewer(&self, reviewer: &Reviewer) -> Result<bool> {
        let slug = self.free_slug(&reviewer.name, Some(&reviewer.id))?;
        let inserted = self
            .connection
            .execute(
                "INSERT OR IGNORE INTO reviewers (id, slug, name, instruction, scope, paths, context_files, enabled, blocking, model, version, origin, created_at, updated_at, classifier)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
                params![
                    reviewer.id,
                    slug,
                    reviewer.name,
                    reviewer.instruction,
                    reviewer.scope.as_str(),
                    to_json(&reviewer.paths),
                    to_json(&reviewer.context_files),
                    reviewer.enabled as i64,
                    reviewer.blocking as i64,
                    reviewer.model,
                    reviewer.version,
                    to_json(&reviewer.origin),
                    reviewer.created_at,
                    reviewer.updated_at,
                    reviewer.classifier.as_text()
                ],
            )
            .map_err(db_error)?;
        for project_id in &reviewer.project_ids {
            self.link(&reviewer.id, project_id)?;
        }
        Ok(inserted > 0)
    }

    /// A new instruction is a new version, so evals and decisions say which wording judged them.
    pub fn update_reviewer(&self, id: &str, changes: ReviewerChanges) -> Result<Reviewer> {
        let current = self.reviewer(id)?.ok_or_else(|| format!("no Reviewer {id}"))?;
        let name = changes.name.unwrap_or_else(|| current.name.clone());
        let slug = if name == current.name { current.slug.clone() } else { self.free_slug(&name, Some(id))? };
        let instruction_changed = changes.instruction.as_ref().is_some_and(|instruction| *instruction != current.instruction);
        let instruction = changes.instruction.unwrap_or_else(|| current.instruction.clone());
        let version = if instruction_changed { current.version + 1 } else { current.version };
        let scope = changes.scope.unwrap_or(current.scope);
        self.connection
            .execute(
                "UPDATE reviewers SET slug = ?2, name = ?3, instruction = ?4, paths = ?5, context_files = ?6, model = ?7,
                   blocking = ?8, scope = ?9, version = ?10, updated_at = ?11, classifier = ?12 WHERE id = ?1",
                params![
                    id,
                    slug,
                    name,
                    instruction,
                    to_json(&changes.paths.unwrap_or(current.paths)),
                    to_json(&changes.context_files.unwrap_or(current.context_files)),
                    changes.model.unwrap_or(current.model),
                    changes.blocking.unwrap_or(current.blocking) as i64,
                    scope.as_str(),
                    version,
                    now_iso(),
                    changes.classifier.unwrap_or(current.classifier).as_text()
                ],
            )
            .map_err(db_error)?;
        self.reviewer(id)?.ok_or_else(|| format!("no Reviewer {id}"))
    }

    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<()> {
        self.connection
            .execute("UPDATE reviewers SET enabled = ?2, updated_at = ?3 WHERE id = ?1", params![id, enabled as i64, now_iso()])
            .map(|_| ())
            .map_err(db_error)
    }

    pub fn delete_reviewer(&self, id: &str) -> Result<()> {
        self.connection.execute("DELETE FROM reviewers WHERE id = ?", [id]).map(|_| ()).map_err(db_error)
    }

    pub fn link(&self, reviewer_id: &str, project_id: &str) -> Result<()> {
        self.connection
            .execute(
                "INSERT OR IGNORE INTO reviewer_projects (reviewer_id, project_id) VALUES (?1, ?2)",
                params![reviewer_id, project_id],
            )
            .map(|_| ())
            .map_err(db_error)
    }

    pub fn unlink(&self, reviewer_id: &str, project_id: &str) -> Result<()> {
        self.connection
            .execute("DELETE FROM reviewer_projects WHERE reviewer_id = ?1 AND project_id = ?2", params![reviewer_id, project_id])
            .map(|_| ())
            .map_err(db_error)
    }

    // ── runs ──────────────────────────────────────────────────────────────

    pub fn record_run(&mut self, run: &Run) -> Result<()> {
        let transaction = self.connection.transaction().map_err(db_error)?;
        transaction
            .execute(
                "INSERT OR IGNORE INTO runs (id, project_id, kind, verdict, failure, attempted_message, diff, diff_hash, started_at, ended_at, duration_ms,
                   commit_sha, commit_message, committed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                params![
                    run.id,
                    run.project_id,
                    run.kind.as_str(),
                    run.verdict.as_str(),
                    run.failure,
                    run.attempted_message,
                    run.diff,
                    run.diff_hash,
                    run.started_at,
                    run.ended_at,
                    run.duration_ms as i64,
                    run.commit_sha,
                    run.commit_message,
                    run.committed_at
                ],
            )
            .map_err(db_error)?;
        for decision in &run.decisions {
            transaction
                .execute(
                    "INSERT OR IGNORE INTO decisions (id, run_id, reviewer_id, reviewer_name, reviewer_version, instruction, verdict, summary, reasoning,
                       evidence, session, model, duration_ms, tokens_read, tokens_written, turns, tool_calls, input_hash, reused_from, classifier, advisory)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21)",
                    params![
                        decision.id,
                        run.id,
                        decision.reviewer_id,
                        decision.reviewer_name,
                        decision.reviewer_version,
                        decision.instruction,
                        decision.verdict.as_str(),
                        decision.summary,
                        decision.reasoning,
                        to_json(&decision.evidence),
                        to_json(&decision.session),
                        decision.model,
                        decision.duration_ms as i64,
                        decision.usage.tokens_read as i64,
                        decision.usage.tokens_written as i64,
                        decision.usage.turns,
                        decision.usage.tool_calls,
                        decision.input_hash,
                        decision.reused_from,
                        decision.classifier.as_ref().map(to_json),
                        decision.advisory as i64
                    ],
                )
                .map_err(db_error)?;
        }
        transaction.commit().map_err(db_error)
    }

    fn decision_row(row: &Row, with_session: bool) -> rusqlite::Result<Decision> {
        let evidence: String = row.get("evidence")?;
        let verdict: String = row.get("verdict")?;
        Ok(Decision {
            id: row.get("id")?,
            run_id: row.get("run_id")?,
            reviewer_id: row.get("reviewer_id")?,
            reviewer_name: row.get("reviewer_name")?,
            reviewer_version: row.get("reviewer_version")?,
            instruction: row.get("instruction")?,
            verdict: Verdict::parse(&verdict).unwrap_or(Verdict::Approved),
            summary: row.get("summary")?,
            reasoning: row.get("reasoning")?,
            evidence: serde_json::from_str(&evidence).unwrap_or_default(),
            session: if with_session {
                serde_json::from_str(&row.get::<_, String>("session")?).unwrap_or(Value::Null)
            } else {
                Value::Null
            },
            model: row.get("model")?,
            duration_ms: row.get::<_, i64>("duration_ms")? as u64,
            usage: Usage {
                tokens_read: row.get::<_, i64>("tokens_read")? as u64,
                tokens_written: row.get::<_, i64>("tokens_written")? as u64,
                turns: row.get::<_, i64>("turns")? as u32,
                tool_calls: row.get::<_, i64>("tool_calls")? as u32,
            },
            input_hash: row.get("input_hash")?,
            reused_from: row.get("reused_from")?,
            classifier: row.get::<_, Option<String>>("classifier")?.and_then(|text| serde_json::from_str(&text).ok()),
            advisory: row.get::<_, i64>("advisory")? != 0,
        })
    }

    /// How many commit-hook decisions the classifier cleared, escalated or couldn't answer since a date.
    pub fn classifier_outcomes(&self, since: &str) -> Result<Vec<(String, u64)>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT json_extract(d.classifier, '$.outcome') AS outcome, count(*) AS count FROM decisions d JOIN runs r ON r.id = d.run_id
                 WHERE r.kind = 'review' AND d.classifier IS NOT NULL AND r.started_at >= ?1 GROUP BY outcome",
            )
            .map_err(db_error)?;
        let rows = statement
            .query_map([since], |row| Ok((row.get::<_, Option<String>>("outcome")?.unwrap_or_default(), row.get::<_, i64>("count")? as u64)))
            .map_err(db_error)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db_error)
    }

    /// `(version, instruction, first judged at, decisions)` for every version that judged something, newest first.
    pub fn reviewer_versions(&self, reviewer_id: &str) -> Result<Vec<(i64, String, String, u64)>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT d.reviewer_version AS version, max(d.instruction) AS instruction, min(r.started_at) AS first, count(*) AS decisions
                 FROM decisions d JOIN runs r ON r.id = d.run_id WHERE d.reviewer_id = ?1
                 GROUP BY d.reviewer_version ORDER BY d.reviewer_version DESC",
            )
            .map_err(db_error)?;
        let rows = statement
            .query_map([reviewer_id], |row| Ok((row.get("version")?, row.get("instruction")?, row.get("first")?, row.get::<_, i64>("decisions")? as u64)))
            .map_err(db_error)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db_error)
    }

    /// The latest verdict a commit review in this repo reached on exactly this input.
    pub fn judged_before(&self, project_id: &str, input_hash: &str) -> Result<Option<Decision>> {
        self.connection
            .query_row(
                "SELECT d.* FROM decisions d JOIN runs r ON r.id = d.run_id
                 WHERE r.project_id = ?1 AND r.kind = 'review' AND d.input_hash = ?2
                 ORDER BY r.started_at DESC LIMIT 1",
                [project_id, input_hash],
                |row| Store::decision_row(row, true),
            )
            .optional()
            .map_err(db_error)
    }

    fn run_row(row: &Row, with_diff: bool) -> rusqlite::Result<Run> {
        let kind: String = row.get("kind")?;
        let verdict: String = row.get("verdict")?;
        Ok(Run {
            id: row.get("id")?,
            project_id: row.get("project_id")?,
            kind: if kind == "eval" { RunKind::Eval } else { RunKind::Review },
            verdict: Verdict::parse(&verdict).unwrap_or(Verdict::Blocked),
            failure: row.get("failure")?,
            attempted_message: row.get("attempted_message")?,
            diff: if with_diff { row.get("diff")? } else { String::new() },
            diff_hash: row.get("diff_hash")?,
            started_at: row.get("started_at")?,
            ended_at: row.get("ended_at")?,
            duration_ms: row.get::<_, i64>("duration_ms")? as u64,
            commit_sha: row.get("commit_sha")?,
            commit_message: row.get("commit_message")?,
            committed_at: row.get("committed_at")?,
            decisions: Vec::new(),
        })
    }

    pub fn run(&self, id: &str, with_sessions: bool) -> Result<Option<Run>> {
        let found = self
            .connection
            .query_row("SELECT * FROM runs WHERE id = ?", [id], |row| Store::run_row(row, true))
            .optional()
            .map_err(db_error)?;
        let Some(mut run) = found else {
            return Ok(None);
        };
        let mut statement = self.connection.prepare("SELECT * FROM decisions WHERE run_id = ? ORDER BY reviewer_name").map_err(db_error)?;
        let rows = statement.query_map([id], |row| Store::decision_row(row, with_sessions)).map_err(db_error)?;
        run.decisions = rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db_error)?;
        Ok(Some(run))
    }

    pub fn run_summaries(&self, project_id: Option<&str>, include_evals: bool, limit: u32) -> Result<Vec<RunSummary>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT r.id, p.name AS project, r.kind, r.verdict, r.failure, r.attempted_message, r.commit_sha, r.started_at, r.duration_ms,
                   (SELECT count(*) FROM decisions d WHERE d.run_id = r.id) AS reviewers,
                   (SELECT group_concat(d.reviewer_name, char(31)) FROM decisions d WHERE d.run_id = r.id AND d.verdict = 'blocked') AS blocked_by,
                   (SELECT coalesce(sum(d.tokens_read + d.tokens_written), 0) FROM decisions d WHERE d.run_id = r.id) AS tokens
                 FROM runs r JOIN projects p ON p.id = r.project_id
                 WHERE (?1 IS NULL OR r.project_id = ?1) AND (?2 OR r.kind = 'review')
                 ORDER BY r.started_at DESC LIMIT ?3",
            )
            .map_err(db_error)?;
        let rows = statement
            .query_map(params![project_id, include_evals, limit], |row| {
                let kind: String = row.get("kind")?;
                let verdict: String = row.get("verdict")?;
                let blocked: Option<String> = row.get("blocked_by")?;
                Ok(RunSummary {
                    id: row.get("id")?,
                    project: row.get("project")?,
                    kind: if kind == "eval" { RunKind::Eval } else { RunKind::Review },
                    verdict: Verdict::parse(&verdict).unwrap_or(Verdict::Blocked),
                    failure: row.get("failure")?,
                    attempted_message: row.get("attempted_message")?,
                    commit_sha: row.get("commit_sha")?,
                    started_at: row.get("started_at")?,
                    duration_ms: row.get::<_, i64>("duration_ms")? as u64,
                    reviewers: row.get::<_, i64>("reviewers")? as u32,
                    blocked_by: blocked.map(|names| names.split('\u{1f}').map(str::to_string).collect()).unwrap_or_default(),
                    tokens: row.get::<_, i64>("tokens")? as u64,
                })
            })
            .map_err(db_error)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db_error)
    }

    pub fn decisions_for(&self, reviewer_id: &str, verdict: Option<Verdict>, limit: u32) -> Result<Vec<(Decision, String, Option<String>)>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT d.*, r.started_at AS run_started, r.commit_sha AS run_commit FROM decisions d JOIN runs r ON r.id = d.run_id
                 WHERE d.reviewer_id = ?1 AND r.kind = 'review' AND (?2 IS NULL OR d.verdict = ?2)
                 ORDER BY r.started_at DESC LIMIT ?3",
            )
            .map_err(db_error)?;
        let rows = statement
            .query_map(params![reviewer_id, verdict.map(Verdict::as_str), limit], |row| {
                Ok((Store::decision_row(row, false)?, row.get("run_started")?, row.get("run_commit")?))
            })
            .map_err(db_error)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db_error)
    }

    /// Commit-hook runs only, per Reviewer, counting only verdicts that ran (not ones given back for the same input).
    pub fn reviewer_records(&self, project_id: Option<&str>, since: Option<&str>) -> Result<Vec<ReviewerRecord>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT d.reviewer_id,
                   (SELECT d2.reviewer_name FROM decisions d2 WHERE d2.reviewer_id = d.reviewer_id ORDER BY d2.rowid DESC LIMIT 1) AS name,
                   count(*) AS runs, sum(d.verdict = 'blocked') AS blocked, avg(d.duration_ms) AS avg_ms,
                   sum(d.tokens_read) AS tokens_read, sum(d.tokens_written) AS tokens_written
                 FROM decisions d JOIN runs r ON r.id = d.run_id
                 WHERE r.kind = 'review' AND d.reused_from IS NULL AND (?1 IS NULL OR r.project_id = ?1) AND (?2 IS NULL OR r.started_at >= ?2)
                 GROUP BY d.reviewer_id ORDER BY tokens_read + tokens_written DESC",
            )
            .map_err(db_error)?;
        let rows = statement
            .query_map(params![project_id, since], |row| {
                Ok(ReviewerRecord {
                    reviewer_id: row.get("reviewer_id")?,
                    reviewer_name: row.get("name")?,
                    runs: row.get::<_, i64>("runs")? as u64,
                    blocked: row.get::<_, i64>("blocked")? as u64,
                    avg_duration_ms: row.get::<_, f64>("avg_ms")? as u64,
                    tokens_read: row.get::<_, i64>("tokens_read")? as u64,
                    tokens_written: row.get::<_, i64>("tokens_written")? as u64,
                })
            })
            .map_err(db_error)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db_error)
    }

    /// `(duration_ms, verdict, failed)` for every commit-hook run, oldest first.
    pub fn run_outcomes(&self, project_id: Option<&str>, since: Option<&str>) -> Result<Vec<(u64, Verdict, bool)>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT duration_ms, verdict, failure IS NOT NULL FROM runs
                 WHERE kind = 'review' AND (?1 IS NULL OR project_id = ?1) AND (?2 IS NULL OR started_at >= ?2) ORDER BY started_at",
            )
            .map_err(db_error)?;
        let rows = statement
            .query_map(params![project_id, since], |row| {
                let verdict: String = row.get(1)?;
                Ok((row.get::<_, i64>(0)? as u64, Verdict::parse(&verdict).unwrap_or(Verdict::Blocked), row.get::<_, bool>(2)?))
            })
            .map_err(db_error)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db_error)
    }

    /// Ties the landed commit to the approved run that let it through: same diff first, else the latest approval just before it.
    pub fn stamp_commit(&self, project_id: &str, diff_hash: &str, sha: &str, message: &str, committed_at: &str) -> Result<StampResult> {
        let already: Option<String> = self
            .connection
            .query_row("SELECT id FROM runs WHERE project_id = ?1 AND commit_sha = ?2", params![project_id, sha], |row| row.get(0))
            .optional()
            .map_err(db_error)?;
        if already.is_some() {
            return Ok(StampResult::AlreadyStamped);
        }
        let by_diff: Option<String> = self
            .connection
            .query_row(
                "SELECT id FROM runs WHERE project_id = ?1 AND diff_hash = ?2 AND kind = 'review' AND verdict = 'approved' AND commit_sha IS NULL
                 ORDER BY started_at DESC LIMIT 1",
                params![project_id, diff_hash],
                |row| row.get(0),
            )
            .optional()
            .map_err(db_error)?;
        let (run_id, matched_by) = match by_diff {
            Some(id) => (id, "diff"),
            None => {
                let not_before = crate::util::parse_iso(committed_at)
                    .map(|time| crate::util::to_iso(time - chrono::Duration::milliseconds(STAMP_TIME_SLOP_MS)))
                    .unwrap_or_else(|| committed_at.to_string());
                let by_time: Option<String> = self
                    .connection
                    .query_row(
                        "SELECT id FROM runs WHERE project_id = ?1 AND kind = 'review' AND verdict = 'approved' AND commit_sha IS NULL
                           AND started_at >= ?2 ORDER BY started_at DESC LIMIT 1",
                        params![project_id, not_before],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(db_error)?;
                match by_time {
                    Some(id) => (id, "time"),
                    None => return Ok(StampResult::NoMatch),
                }
            }
        };
        self.connection
            .execute(
                "UPDATE runs SET commit_sha = ?2, commit_message = ?3, committed_at = ?4, commit_match = ?5 WHERE id = ?1",
                params![run_id, sha, message, committed_at, matched_by],
            )
            .map_err(db_error)?;
        Ok(StampResult::Stamped)
    }

    pub fn delete_run(&self, id: &str) -> Result<bool> {
        self.connection.execute("DELETE FROM runs WHERE id = ?", [id]).map(|rows| rows > 0).map_err(db_error)
    }

    pub fn prune_eval_runs(&self) -> Result<usize> {
        self.connection.execute("DELETE FROM runs WHERE kind = 'eval'", []).map_err(db_error)
    }

    // ── cases and evals ───────────────────────────────────────────────────

    fn case_row(row: &Row, with_body: bool) -> rusqlite::Result<Case> {
        let expected: String = row.get("expected")?;
        let origin: String = row.get("origin")?;
        Ok(Case {
            id: row.get("id")?,
            reviewer_id: row.get("reviewer_id")?,
            name: row.get("name")?,
            expected: Verdict::parse(&expected).unwrap_or(Verdict::Blocked),
            diff: if with_body { row.get("diff")? } else { String::new() },
            diff_hash: row.get("diff_hash")?,
            files: if with_body { serde_json::from_str(&row.get::<_, String>("files")?).unwrap_or_default() } else { Vec::new() },
            origin: serde_json::from_str(&origin).unwrap_or(Value::Null),
            reviewer_version: row.get("reviewer_version")?,
            created_at: row.get("created_at")?,
        })
    }

    pub fn cases(&self, reviewer_id: &str, with_body: bool) -> Result<Vec<Case>> {
        let mut statement = self.connection.prepare("SELECT * FROM cases WHERE reviewer_id = ? ORDER BY name").map_err(db_error)?;
        let rows = statement.query_map([reviewer_id], |row| Store::case_row(row, with_body)).map_err(db_error)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db_error)
    }

    pub fn case(&self, id: &str) -> Result<Option<Case>> {
        self.connection
            .query_row("SELECT * FROM cases WHERE id = ?", [id], |row| Store::case_row(row, true))
            .optional()
            .map_err(db_error)
    }

    pub fn case_named(&self, reviewer_id: &str, name: &str) -> Result<Option<Case>> {
        self.connection
            .query_row("SELECT * FROM cases WHERE reviewer_id = ?1 AND name = ?2", params![reviewer_id, name], |row| Store::case_row(row, true))
            .optional()
            .map_err(db_error)
    }

    pub fn save_case(&self, case: &Case, replace: bool) -> Result<()> {
        let verb = if replace { "INSERT OR REPLACE" } else { "INSERT OR IGNORE" };
        self.connection
            .execute(
                &format!(
                    "{verb} INTO cases (id, reviewer_id, name, expected, diff, diff_hash, files, origin, reviewer_version, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)"
                ),
                params![
                    case.id,
                    case.reviewer_id,
                    case.name,
                    case.expected.as_str(),
                    case.diff,
                    case.diff_hash,
                    to_json(&case.files),
                    to_json(&case.origin),
                    case.reviewer_version,
                    case.created_at
                ],
            )
            .map(|_| ())
            .map_err(db_error)
    }

    pub fn update_case(&self, id: &str, name: Option<&str>, expected: Option<Verdict>) -> Result<()> {
        self.connection
            .execute(
                "UPDATE cases SET name = coalesce(?2, name), expected = coalesce(?3, expected) WHERE id = ?1",
                params![id, name, expected.map(Verdict::as_str)],
            )
            .map(|_| ())
            .map_err(db_error)
    }

    pub fn delete_case(&self, id: &str) -> Result<bool> {
        self.connection.execute("DELETE FROM cases WHERE id = ?", [id]).map(|rows| rows > 0).map_err(db_error)
    }

    fn batch_row(row: &Row) -> rusqlite::Result<EvalBatch> {
        let outcomes: String = row.get("outcomes")?;
        Ok(EvalBatch {
            id: row.get("id")?,
            reviewer_id: row.get("reviewer_id")?,
            reviewer_version: row.get("reviewer_version")?,
            instruction: row.get("instruction")?,
            model: row.get("model")?,
            started_at: row.get("started_at")?,
            ended_at: row.get("ended_at")?,
            duration_ms: row.get::<_, i64>("duration_ms")? as u64,
            passed: row.get::<_, i64>("passed")? as u32,
            failed: row.get::<_, i64>("failed")? as u32,
            outcomes: serde_json::from_str(&outcomes).unwrap_or(Value::Null),
        })
    }

    pub fn save_batch(&self, batch: &EvalBatch) -> Result<()> {
        self.connection
            .execute(
                "INSERT OR IGNORE INTO eval_batches (id, reviewer_id, reviewer_version, instruction, model, started_at, ended_at, duration_ms, passed, failed, outcomes)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    batch.id,
                    batch.reviewer_id,
                    batch.reviewer_version,
                    batch.instruction,
                    batch.model,
                    batch.started_at,
                    batch.ended_at,
                    batch.duration_ms as i64,
                    batch.passed,
                    batch.failed,
                    to_json(&batch.outcomes)
                ],
            )
            .map(|_| ())
            .map_err(db_error)
    }

    pub fn batches(&self, reviewer_id: &str, limit: u32) -> Result<Vec<EvalBatch>> {
        let mut statement = self
            .connection
            .prepare("SELECT * FROM eval_batches WHERE reviewer_id = ?1 ORDER BY started_at DESC LIMIT ?2")
            .map_err(db_error)?;
        let rows = statement.query_map(params![reviewer_id, limit], Store::batch_row).map_err(db_error)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db_error)
    }

    pub fn delete_batch(&self, id: &str) -> Result<bool> {
        self.connection.execute("DELETE FROM eval_batches WHERE id = ?", [id]).map(|rows| rows > 0).map_err(db_error)
    }
}

pub fn new_decision_id() -> String {
    new_id("dec")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upgrades_a_schema_1_database() {
        let directory = std::env::temp_dir().join(new_id("reviewers-test"));
        let path = directory.join("reviewers.sqlite");
        Store::open(&path)
            .unwrap()
            .connection()
            .execute_batch(
                "DROP INDEX decisions_input;
                 ALTER TABLE decisions DROP COLUMN input_hash;
                 ALTER TABLE decisions DROP COLUMN reused_from;
                 ALTER TABLE decisions DROP COLUMN classifier;
                 ALTER TABLE reviewers DROP COLUMN classifier;
                 ALTER TABLE projects DROP COLUMN ignored;
                 ALTER TABLE decisions DROP COLUMN advisory;
                 PRAGMA user_version = 1;",
            )
            .unwrap();
        let store = Store::open(&path).unwrap();
        let version: i64 = store.connection().query_row("PRAGMA user_version", [], |row| row.get(0)).unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        assert!(store.judged_before("prj_none", "hash").unwrap().is_none());
        drop(store);
        // Opening it again is a no-op, not a second migration.
        assert!(Store::open(&path).is_ok());
        let _ = std::fs::remove_dir_all(&directory);
    }
}
