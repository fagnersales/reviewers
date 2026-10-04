# Onboarding: finding the rules you already hold

`reviewers onboard` (and bare `reviewers` the first time) reads the person's Claude Code and Codex sessions from the last 90 days, keeping only what the person typed. Claude sessions come from every config folder (`~/.claude`, `~/.claude-*`, `$CLAUDE_CONFIG_DIR`). Codex JSONL rollouts come from `sessions` and `archived_sessions` under `~/.codex` and `$CODEX_HOME`, including dated subfolders. Duplicate roots and duplicate Codex user-event/response-item representations are read once. Injected context, scripted `claude -p`/`codex exec` sessions and Codex subagent sessions are skipped. Sessions are matched to repos, worktrees included, even deleted ones.

The person picks the repos, whether to pool evidence across all selected projects or keep each project's context separate, and the model (the default is the one they used most lately). In the pooled mode, agents read the material in parallel, one share each: a big repo is cut into stretches of time, and small repos can share an agent. A final step merges the same rule said in different repos and decides which rules are personal (`everywhere`). In per-project mode, each extraction and merge sees only one project's evidence, and its suggestions belong to that project.

The picker offers models for installed CLIs. `--model codex` uses Codex's built-in default; `--model codex:<model-id>` selects a Codex model. Claude ids and aliases still work. Either provider can read both transcript sources, and the same selection runs extraction and merging. If no review default is configured, a Codex selection is also saved for subsequent reviews and evals; a Claude selection only reads the transcripts. `--dry-run` builds the material without requiring either CLI or sending anything to a model.

A rule must have come up at least twice, or have been stated as a standing rule ("always", "never"). Rules a linter could enforce are left out: a lint rule is instant and costs no tokens.

At the end the person picks which Reviewers start on, and the global hooks are installed so every repo runs them, with each repo's own hooks still running first. Everything found, and every piece of evidence, is kept in `onboard/<date>/` in the data folder (`~/.reviewers`, or `REVIEWERS_HOME`).

The merge is shown the relevant Reviewers that already exist and leaves out a rule one of them already checks, even in other words; a suggestion with the same name as an existing Reviewer in its scope, ignoring case, is skipped too. `--context all|project` sets the context for one run without a picker; `reviewers context all|project` sets a default for onboarding and `suggest`, and `reviewers context default` clears it. With neither, the person picks each time, and without a terminal `--context` is required. `--since`, `--repos` and `--model` narrow it; `reviewers onboard --help` lists the rest.

## Later: `reviewers suggest`

`reviewers suggest` does the same over only what was typed since the last read, by onboarding or by an earlier `suggest`, and shows each new Reviewer with why it's suggested and the person's words behind it. Then the person picks which to add; the ones they add start on.

How far each repo has been read is kept in `suggest/state.json` in the data folder and shared by onboarding and `suggest`, so a session is never sent to an agent twice. A repo whose agent failed is read again next time. `--reread` on either command reads the whole window again, and `--since` caps how far back reading goes (90 days).

A rule seen only once waits in the same file and is suggested once it comes up again, in any later run, within the window. Suggestions the person didn't pick are dropped; suggestions nobody was asked about (no terminal, or the picker cancelled) wait too. `--yes` adds every suggestion without asking.
