# Reviewers 0.1.0: guide for coding agents

Reviewers are the person's rules about code, checked on every commit. Each Reviewer is one rule. A git `commit-msg` hook runs every Reviewer that applies to the staged diff, all at once, each as its own read-only Claude Code session. If any blocks, the commit stops and the hook prints, for each block, the file and lines, what is wrong, and what the code should do instead.

You are the one who fixes, adds and tunes Reviewers. The person states rules; you do the work.

## When a commit is blocked

1. Read the hook output: every block names the file, the lines and what to do.
2. Fix the code, stage it, and commit again with the same message. Committing the same diff again gets the same verdicts back without running: only a change to the code or to the Reviewer is judged anew.
3. If a block makes no sense, read the Reviewer's full reasoning: `reviewers run <run-id> --json` (the id is in the hook output). If the Reviewer is wrong, fix the Reviewer rather than working around it: capture the diff as an `approved` case and tune the instruction (`reviewers help evals`).
4. Never bypass with `--no-verify` or `REVIEWERS_BYPASS=1` unless the person asks.

A commit that ends "could not reach a verdict" means a Reviewer crashed or timed out, not that the code is wrong. Retry once: only the Reviewers without a verdict run again. If it fails again, tell the person.

## When the person states a rule

"Never cast types", "errors shown to users go through the error map", "dialogs, not pages":

1. Check it isn't already a Reviewer: `reviewers list --all --json`.
2. Check whether a linter can enforce it (an ESLint rule, a TypeScript flag). If so, propose that instead: it's instant and uses no tokens.
3. Otherwise write it: `reviewers new --name "…" --instruction "…"` (this repo) or with `--everywhere` (a personal rule for every repo). Add `--paths` when it only concerns part of the repo. `reviewers help writing` covers the instruction.
4. Give it cases, at least one diff it must block and one it must approve, and run the evals before calling it done (`reviewers help evals`).

## When asked how the Reviewers are doing

`reviewers stats --json` (this repo) or `--all`: commits judged, blocks, wait, tokens, and per Reviewer: block rate, time, tokens per catch, and `neverBlocks` for one that hasn't blocked in 50+ runs. A Reviewer that never blocks is a candidate for a lint rule or for turning off. Draw a chart when it helps; the data is all in the JSON.

## Data

Everything lives in `~/.reviewers/reviewers.sqlite` on this machine. Nothing is sent anywhere except to the model, through the person's own Claude Code.

## Commands

Every read command takes `--json`; agents should always use it.

### `reviewers onboard`

Read your agent sessions and turn the rules you keep repeating into Reviewers

  --since <SINCE>: How far back to read, in days
  --repos <REPOS>: Only these repos, by folder name, comma-separated (skips the repo picker)
  --model <MODEL>: The model the agents run on (skips the model picker)
  --parallel <PARALLEL>: Agents running at once
  --max <MAX>: At most this many Reviewers
  --yes: Take the defaults without asking: every repo, your usual model, the strongest Reviewers on, hooks installed
  --dry-run: Build the material and stop before any agent runs
  --no-skill: Don't give your coding agents the reviewers skill

### `reviewers status`

This repo: the Reviewers that judge it and its latest reviews

  --json: Print JSON instead of text, for agents and scripts

### `reviewers list`

List Reviewers: this repo's, or every one with --all

  --all: Every Reviewer, not just the ones judging this repo
  --json: Print JSON instead of text, for agents and scripts

### `reviewers show`

One Reviewer: its instruction, where it runs, and its record

  <REVIEWER> (required): Name, slug or id
  --decisions <DECISIONS>: How many recent decisions to list
  --json: Print JSON instead of text, for agents and scripts

### `reviewers new`

Create a Reviewer

  --name <NAME> (required): Short name, stated as the rule: "No type casts"
  --instruction <INSTRUCTION> (required): The rule, as a brief for the judge: what to block, what's allowed, real examples
  --everywhere: Judge every repo instead of just this one
  --repo <REPOS>: Repos to judge (paths). Defaults to the repo you're in
  --paths <PATHS>: Only run when the diff touches these globs, comma-separated: `convex/**,shared/**`
  --context-files <CONTEXT_FILES>: Repo files attached to every review as reference, comma-separated
  --model <MODEL>: Model for this Reviewer only
  --disabled: Create it turned off

### `reviewers edit`

Change a Reviewer. A new instruction is a new version

  <REVIEWER> (required): Name, slug or id
  --name <NAME>
  --instruction <INSTRUCTION>: A new instruction is a new version
  --paths <PATHS>: Comma-separated globs, or `none`
  --context-files <CONTEXT_FILES>: Comma-separated files, or `none`
  --model <MODEL>: A model id, or `default` to inherit
  --everywhere: Judge every repo
  --repos-only: Judge only linked repos
  --add-repo <ADD_REPO>: Link a repo (path)
  --remove-repo <REMOVE_REPO>: Unlink a repo (path)
  --advisory: Report without stopping the commit
  --blocking: Stop the commit when it blocks (the default)

### `reviewers enable`

Turn Reviewers on

  <REVIEWERS> (required)

### `reviewers disable`

Turn Reviewers off without deleting them or their history

  <REVIEWERS> (required)

### `reviewers remove`

Delete a Reviewer and its eval cases. Past decisions stay in the history

  <REVIEWER> (required)

### `reviewers runs`

Recent commit reviews: this repo's, or every repo's with --all

  --all
  --evals: Include eval runs
  --limit <LIMIT>
  --prune-evals: Delete every eval run kept by `reviewers eval --record`. Commit reviews are never deleted in bulk
  --json: Print JSON instead of text, for agents and scripts

### `reviewers run`

One review in full: every verdict, its evidence and reasoning

  <ID> (required)
  --session: Also print each Reviewer's whole session: files read, thinking, tools
  --delete: Delete this run from the history
  --json: Print JSON instead of text, for agents and scripts

### `reviewers stats`

What each Reviewer catches, how long it adds to a commit, and the tokens it uses

  --all: Every repo, not just this one
  --days <DAYS>: Only the last N days
  --json: Print JSON instead of text, for agents and scripts

### `reviewers case add`

Capture a diff as a case: the working tree by default, or --staged, --from-run, --patch

  <REVIEWER> (required): Name, slug or id of the Reviewer
  --name <NAME> (required): Kebab-case, describing the situation: `refactor-moves-guard`
  --expect <EXPECT> (required)
  --staged: Only what's staged, not the whole working tree
  --from-run <FROM_RUN>: The diff a past review judged
  --patch <PATCH>: A patch file
  --repo <REPO>: The repo the diff belongs to. Defaults to the current directory
  --include <INCLUDE>: Extra globs to include in the snapshot, comma-separated
  --replace: Overwrite a case with the same name

### `reviewers case list`

A Reviewer's cases

  <REVIEWER> (required)
  --json

### `reviewers case update`

Rename a case or flip its expected verdict

  <CASE> (required)
  --name <NAME>
  --expect <EXPECT>

### `reviewers case remove`

Delete a case. Past eval batches keep their record of it

  <CASE> (required)

### `reviewers eval`

Run a Reviewer's eval cases and record the score

  <REVIEWER> (required): Name, slug or id
  --only <ONLY>: Only cases whose name contains this
  --record: Also keep each judged case as an eval run, so `reviewers run <id> --session` can show it
  --json

### `reviewers evals`

A Reviewer's eval history, newest first

  <REVIEWER> (required)
  --limit <LIMIT>
  --delete <DELETE>: Delete one batch instead
  --json: Print JSON instead of text, for agents and scripts

### `reviewers repos`

Repos Reviewers judge

  --json: Print JSON instead of text, for agents and scripts

### `reviewers init`

Start judging this repo (or PATH): register it and install its hooks

  <PATH>

### `reviewers hooks install`

Install the hooks in this repo (or PATH), or in every repo with --all

  <PATH>
  --all
  --take-over: Replace Personal Workspace's hooks

### `reviewers hooks status`

Which repos have the hooks

  --json

### `reviewers model`

Show or set the default model Reviewers run on

  <MODEL>: A model id or alias, or `default` to use Claude Code's own default
  --repo: Set it for this repo only

### `reviewers import`

Bring over Reviewers, history and evals from Personal Workspace

  --from <FROM>: Personal Workspace's database. Defaults to ~/apps/personalworkspace/.data/workspace.sqlite
  --hooks: Also switch every imported repo's git hooks from Personal Workspace to reviewers

### `reviewers skill install`

Find the coding agents on this machine and give each the skill

  --json

### `reviewers skill status`

Where the skill is, and which agents have it

  --json

### `reviewers skill uninstall`

Take the skill away from every agent

### `reviewers upgrade`

Update reviewers to the latest release

  --check: Only say whether a newer release exists

### `reviewers help`

What you can do. Agents: `reviewers help --agent`

  <TOPIC>: A topic: writing, evals, onboarding, hooks, commands
  --agent: The full guide for coding agents: how Reviewers work and every command


## Topics

- `reviewers help writing`: How to write a Reviewer that judges well
- `reviewers help evals`: Cases and the tuning loop
- `reviewers help onboarding`: How the first run finds your rules
- `reviewers help hooks`: What happens on a commit, and how to skip it

---

# Writing a Reviewer

A Reviewer reads one diff and answers `approved` or `blocked` for one rule. The instruction is everything it knows about the rule, so write it as a brief for a judge.

## Name

At most six words, stated as the rule itself: "No type casts", "Errors reach users through the error map", "Dialogs, not pages". It's how the person recognizes the rule in a list.

## Instruction

- **Start with what to block**, in one sentence.
- **Then what is allowed**, so it doesn't block the legitimate exceptions: a cast in a test fixture, a raw `Error` for an invariant that can't happen.
- **Use real examples**: the code the person corrected, the names they rejected, the pattern they asked for instead.
- **Point to files that show the right way** when they exist ("follow `convex/coupons/rules.ts`"). The Reviewer can open any file in the repo.
- **Under 200 words. One rule per Reviewer.** Two rules are two Reviewers; a Reviewer that checks three things blocks for the wrong one.

## Scope

- `--everywhere` for the person's habits in any repo (naming, comments, error handling). Without it, the Reviewer judges the repo you're in, or the ones given with `--repo`.
- `--paths "convex/**,shared/**"` runs it only when the diff touches those files. Scoping keeps every commit fast: a Reviewer that can't apply shouldn't run.
- `--context-files LAYOUT.md` attaches a file to every review as reference.

## Cost

Every Reviewer that applies is a Claude session on every commit, all of them in parallel. A commit waits for the slowest. Ten Reviewers that each take 30 seconds still cost ten sessions of tokens. A rule a linter can enforce should be a lint rule.

---

# Evals: tuning a Reviewer with cases

Instructions drift: too strict and the Reviewer blocks good commits, too loose and it waves bad ones through. Cases make tuning a measurement instead of a guess.

## What a case is

A diff, the verdict it must get (`approved` or `blocked`), and a snapshot of the files around it. When an eval runs, each case becomes its own throwaway git repo (snapshot committed, diff staged) and the Reviewer judges it exactly as the hook would. The source repo isn't needed, so cases never go stale.

The snapshot holds the directories the diff touches and the Reviewer's context files. If the instruction sends the Reviewer to an example elsewhere, add it with `--include "convex/coupons/**"`.

## Adding cases

- **From a change you make on purpose**: edit, stage, then `reviewers case add <reviewer> --name <kebab-name> --expect blocked --staged`, then revert the edit.
- **From a real review**: when the hook blocked something it shouldn't have (or let something through), pin it: `reviewers case add <reviewer> --name <name> --expect approved --from-run <run-id>`. This is how a Reviewer learns from real use.
- **From a patch file**: `--patch change.diff`, run inside the repo it came from.

Name cases for the situation, not the verdict. A good set has both kinds, and the most useful `approved` cases sit right at the edge of the rule.

## The loop

1. `reviewers eval <reviewer>`: every case, pass or fail, with the Reviewer's reasoning. Exits 1 if any case fails.
2. Read the failures. The reasoning shows which sentence of the instruction it applied or ignored.
3. `reviewers edit <reviewer> --instruction "…"`. This makes a new version; every batch records the version it ran.
4. Run again until everything passes. Keep the cases: they're the regression suite for the next edit.

`--only <text>` re-runs the matching cases while iterating. `reviewers evals <reviewer>` lists past batches by version.

A case that comes back `not run` fell outside the Reviewer's `--paths`, so it was never judged. It fails whatever it expected: an approval nobody gave proves nothing. Widen the paths, or remove the case if it no longer belongs to this Reviewer.

---

# Onboarding: finding the rules you already hold

`reviewers onboard` (and bare `reviewers` the first time) reads the person's Claude Code sessions from the last 90 days, in every config folder (`~/.claude`, `~/.claude-*`, `$CLAUDE_CONFIG_DIR`), and keeps only what the person typed. Scripted `claude -p` sessions are skipped. Sessions are matched to repos, worktrees included, even deleted ones.

The person picks the repos and the model (the default is the one they used most lately). Then agents read the material in parallel, one share each: a big repo is cut into stretches of time, and small repos share an agent. Each agent reports the rules it finds with evidence: the person's own words, with dates. A final step merges the same rule said in different repos, and decides which rules are personal (`everywhere`) and which belong to one repo.

A rule must have come up at least twice, or have been stated as a standing rule ("always", "never"). Rules a linter could enforce are listed apart.

At the end the person picks which Reviewers start on, and the commit hook is installed in the picked repos. Everything found, and every piece of evidence, is kept in `~/.reviewers/onboard/<date>/`.

---

# What happens on a commit

`reviewers init` registers a repo and installs two git hooks, honoring `core.hooksPath` (husky, lefthook). A hook another tool already owns is left alone; to run Reviewers from it, add this line to that tool's `commit-msg` hook:

```sh
reviewers hook commit-msg "$1"
```

- **commit-msg** takes the staged diff and runs every enabled Reviewer whose scope and `--paths` match, all at once. Each is a read-only Claude Code session in the repo: it can open any file, but can't change anything or run commands. If any blocks, the commit stops with the file, lines and fix for each block. If a Reviewer can't reach a verdict (a crash, a timeout), the commit stops too: it fails closed.
- **post-commit** ties the landed commit to the review that let it through, so the history shows which commit each review became.

Skip the hooks once with `REVIEWERS_BYPASS=1 git commit …`. Agents shouldn't, unless the person asks.

Every review is kept: `reviewers runs`, `reviewers run <id>`, `reviewers stats`.
