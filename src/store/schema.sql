CREATE TABLE settings (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE projects (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  root TEXT NOT NULL UNIQUE,
  remote TEXT,
  model TEXT,
  created_at TEXT NOT NULL,
  -- Set with `reviewers ignore`: no Reviewer judges this repo, even under the global hooks.
  ignored INTEGER NOT NULL DEFAULT 0
);

-- `everywhere` Reviewers judge every registered repo; `projects` ones only
-- the repos linked in reviewer_projects.
CREATE TABLE reviewers (
  id TEXT PRIMARY KEY,
  slug TEXT NOT NULL UNIQUE,
  name TEXT NOT NULL,
  instruction TEXT NOT NULL,
  scope TEXT NOT NULL CHECK (scope IN ('everywhere', 'projects')),
  paths TEXT NOT NULL DEFAULT '[]',
  context_files TEXT NOT NULL DEFAULT '[]',
  enabled INTEGER NOT NULL DEFAULT 1,
  blocking INTEGER NOT NULL DEFAULT 1,
  model TEXT,
  version INTEGER NOT NULL DEFAULT 1,
  origin TEXT NOT NULL DEFAULT '{}',
  -- `default`, `off`, or a cutoff from 0 to 1: under it, the classifier clears the Reviewer.
  classifier TEXT NOT NULL DEFAULT 'default',
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE reviewer_projects (
  reviewer_id TEXT NOT NULL REFERENCES reviewers(id) ON DELETE CASCADE,
  project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  PRIMARY KEY (reviewer_id, project_id)
);

-- `failure` is set when no verdict could be reached (the harness crashed,
-- timed out, ran out of quota); such a run blocks the commit too.
CREATE TABLE runs (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  kind TEXT NOT NULL CHECK (kind IN ('review', 'eval')),
  verdict TEXT NOT NULL CHECK (verdict IN ('approved', 'blocked')),
  failure TEXT,
  attempted_message TEXT,
  diff TEXT NOT NULL,
  diff_hash TEXT NOT NULL,
  started_at TEXT NOT NULL,
  ended_at TEXT NOT NULL,
  duration_ms INTEGER NOT NULL,
  commit_sha TEXT,
  commit_message TEXT,
  committed_at TEXT,
  commit_match TEXT
);
CREATE INDEX runs_project_started ON runs(project_id, started_at DESC);
CREATE INDEX runs_project_hash ON runs(project_id, diff_hash);

-- Decisions copy the Reviewer's name, version and instruction, so history
-- reads as it was judged even after the Reviewer changes or is removed.
CREATE TABLE decisions (
  id TEXT PRIMARY KEY,
  run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
  reviewer_id TEXT NOT NULL,
  reviewer_name TEXT NOT NULL,
  reviewer_version INTEGER NOT NULL,
  instruction TEXT NOT NULL,
  verdict TEXT NOT NULL CHECK (verdict IN ('approved', 'blocked')),
  summary TEXT NOT NULL,
  reasoning TEXT NOT NULL,
  evidence TEXT NOT NULL DEFAULT '[]',
  session TEXT NOT NULL DEFAULT '[]',
  model TEXT,
  duration_ms INTEGER NOT NULL,
  tokens_read INTEGER NOT NULL DEFAULT 0,
  tokens_written INTEGER NOT NULL DEFAULT 0,
  turns INTEGER NOT NULL DEFAULT 0,
  tool_calls INTEGER NOT NULL DEFAULT 0,
  -- A hash of exactly what the Reviewer was asked (prompt, model, schema).
  -- The same input gets the earlier verdict back without running: then
  -- `reused_from` is the run that judged it, and this row cost no tokens.
  input_hash TEXT,
  reused_from TEXT,
  -- What the classifier did before (or instead of) the session, as JSON.
  classifier TEXT
);
CREATE INDEX decisions_run ON decisions(run_id);
CREATE INDEX decisions_reviewer ON decisions(reviewer_id, run_id);
CREATE INDEX decisions_input ON decisions(input_hash);

CREATE TABLE cases (
  id TEXT PRIMARY KEY,
  reviewer_id TEXT NOT NULL REFERENCES reviewers(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  expected TEXT NOT NULL CHECK (expected IN ('approved', 'blocked')),
  diff TEXT NOT NULL,
  diff_hash TEXT NOT NULL,
  files TEXT NOT NULL,
  origin TEXT NOT NULL,
  reviewer_version INTEGER NOT NULL,
  created_at TEXT NOT NULL,
  UNIQUE (reviewer_id, name)
);

CREATE TABLE eval_batches (
  id TEXT PRIMARY KEY,
  reviewer_id TEXT NOT NULL REFERENCES reviewers(id) ON DELETE CASCADE,
  reviewer_version INTEGER NOT NULL,
  instruction TEXT NOT NULL,
  model TEXT,
  started_at TEXT NOT NULL,
  ended_at TEXT NOT NULL,
  duration_ms INTEGER NOT NULL,
  passed INTEGER NOT NULL,
  failed INTEGER NOT NULL,
  outcomes TEXT NOT NULL
);
CREATE INDEX eval_batches_reviewer ON eval_batches(reviewer_id, started_at DESC);
