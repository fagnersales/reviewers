# Onboarding: finding the rules you already hold

`reviewers onboard` (and bare `reviewers` the first time) reads the person's Claude Code and Codex sessions from the last 90 days, keeping only what the person typed. Claude sessions come from every config folder (`~/.claude`, `~/.claude-*`, `$CLAUDE_CONFIG_DIR`). Codex JSONL rollouts come from `sessions` and `archived_sessions` under `~/.codex` and `$CODEX_HOME`, including dated subfolders. Duplicate roots and duplicate Codex user-event/response-item representations are read once. Injected context, scripted `claude -p`/`codex exec` sessions and Codex subagent sessions are skipped. Sessions are matched to repos, worktrees included, even deleted ones.

The person picks the repos and the model (the default is the one they used most lately). Then agents read the material in parallel, one share each: a big repo is cut into stretches of time, and small repos share an agent. Each agent reports the rules it finds with evidence: the person's own words, with dates. A final step merges the same rule said in different repos, and decides which rules are personal (`everywhere`) and which belong to one repo.

The picker offers models for installed CLIs. `--model codex` uses Codex's built-in default; `--model codex:<model-id>` selects a Codex model. Claude ids and aliases still work. Either provider can read both transcript sources, and the same selection runs extraction and merging. If no review default is configured, onboarding saves its selection for subsequent reviews and evals. `--dry-run` builds the material without requiring either CLI or sending anything to a model.

A rule must have come up at least twice, or have been stated as a standing rule ("always", "never"). Rules a linter could enforce are listed apart.

At the end the person picks which Reviewers start on, and the global hooks are installed so every repo runs them, with each repo's own hooks still running first. Everything found, and every piece of evidence, is kept in `onboard/<date>/` in the data folder (`~/.reviewers`, or `REVIEWERS_HOME`).

Running it again skips any suggestion with the same name as an existing Reviewer, ignoring case. The check is by name only, so the same rule under another name would be suggested again. `--since`, `--repos` and `--model` narrow it; `reviewers onboard --help` lists the rest.
