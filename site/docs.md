# Reviewers 0.5.1: guide for coding agents

Reviewers are the person's rules about code, checked on every commit. Each Reviewer is one rule. A git `commit-msg` hook runs every Reviewer that applies to the staged diff, all at once, each as its own read-only Claude Code or Codex session. If any blocks, the commit stops and the hook prints, for each block, the file and lines, what is wrong, and what the code should do instead.

You are the one who fixes, adds and tunes Reviewers. The person states rules; you do the work.

## When a commit is blocked

1. Read the hook output: every block names the file, the lines and what to do.
2. Fix the code, stage it, and commit again with the same message. Committing the same diff again gets the same verdicts back without running: only a change to the code or to the Reviewer is judged anew. `REVIEWERS_FRESH=1 git commit …` asks for a fresh judgement of the same code, only when the person asks.
3. If a block makes no sense, read the Reviewer's full reasoning: `reviewers run <run-id> --json` (the id is in the hook output). If the Reviewer is wrong, fix the Reviewer rather than working around it: capture the diff as an `approved` case and tune the instruction (`reviewers help evals`).
4. Never bypass with `--no-verify` or `REVIEWERS_BYPASS=1` unless the person asks.

A commit that ends "could not reach a verdict" means a Reviewer crashed or timed out, not that the code is wrong. Retry once: only the Reviewers without a verdict run again. If it fails again, tell the person.

## Before committing

`reviewers check` judges the change now, as the commit would: every Reviewer that applies, against what `git add -A && git commit` would commit (`--staged` for only what's staged). It prints the same blocks and exits the way the hook would. A commit of exactly that change afterwards gets the same verdicts back without running, so checking first moves the wait rather than adding to it. Name Reviewers to run only those: `reviewers check "No type casts"`, for example to see whether a fix satisfies the one that blocked. Checks show in `reviewers runs`, marked `check`; `reviewers stats` counts a verdict once, whether a check or a commit reached it.

## When the person states a rule

"Never cast types", "errors shown to users go through the error map", "dialogs, not pages":

1. Check it isn't already a Reviewer: `reviewers list --all --json`.
2. Check whether a linter can enforce it (an ESLint rule, a TypeScript flag). If so, propose that instead: it's instant and uses no tokens.
3. Otherwise write it: `reviewers new --name "…" --instruction "…"` (this repo) or with `--everywhere` (a personal rule for every repo). Add `--paths` when it only concerns part of the repo, and `--reads-text` when the rule is about prose (Markdown, docs): a change to only text files starts no Reviewer otherwise. `reviewers help writing` covers the instruction.
4. Give it cases, at least one diff it must block and one it must approve, and run the evals before calling it done (`reviewers help evals`).

## Text files

A commit that only changes text files (Markdown, `.txt` and the like; `reviewers text-files` prints the list and sets it) starts no Reviewer, to save the tokens: the hook approves and says `only text files changed`. A Reviewer made with `--reads-text` still judges them. When a Reviewer about docs or skill files never runs, that is why; `reviewers edit <name> --reads-text` fixes it.

## When the person rejects something you did

"No, don't cast that", "undo it", "use the Badge we have": a correction is a rule the person may want checked on every commit. Don't stop to ask. Do what the correction asks and finish the task; then, in your final message, ask whether they want a Reviewer for it, with the rule named as it would be ("No type casts on API data"). If `reviewers list --all --json` shows a Reviewer that already covers it, say which one instead. When they say yes, it's a stated rule: follow the steps above.

## When asked to find new Reviewers

`reviewers suggest` reads the person's sessions since the last read and suggests new Reviewers, each with why and the person's own words. It runs several agent sessions, so run it only when the person asks. Without a terminal it adds nothing: show the person the suggestions, and add the ones they pick with `reviewers new`; each suggestion's full instruction is in the run folder it prints. `--yes` adds them all.

## When asked how the Reviewers are doing

`reviewers stats --json` (this repo) or `--all`: commits judged, blocks, wait, tokens, and per Reviewer: block rate, time, tokens per catch, and `neverBlocks` for one that hasn't blocked in 50+ runs. A Reviewer that never blocks is a candidate for a lint rule or for turning off. Draw a chart when it helps; the data is all in the JSON.

## The classifier

The classifier is optional. It asks Jev, TypeSafe's evaluation model, which Reviewers a change can't concern, and clears them before any session starts; it's reached with a Vercel AI Gateway key or a TypeSafe key (`reviewers help classifier`). Whether one is connected: `reviewers classifier status --json`. After connecting it, run `reviewers classifier check --json`: for any Reviewer with `missedBlocks`, set a cutoff at or under its `lowestBlockScore` (`reviewers edit <name> --classifier 0.1`) or turn it off for that Reviewer (`--classifier off`). `reviewers help classifier` has the rest.

## Repos

`reviewers hooks status --json` says whether the global hooks are on. With them, every repo runs the Reviewers meant for every repo, and a new repo needs nothing; without them, `reviewers init` starts judging one. A repo that sets its own hooks folder (`git config core.hooksPath` prints one, as with husky) needs `reviewers init` either way, plus, where another tool owns the hooks, `reviewers hook commit-msg "$1"` in its commit-msg hook and `reviewers hook post-commit` in its post-commit hook (`reviewers help hooks` has husky and lefthook). `reviewers ignore` stops Reviewers in a repo the person doesn't want judged.

## Data

Everything lives in `~/.reviewers/reviewers.sqlite` on this machine. Diffs go to the model through the person's own Claude Code or Codex CLI, and, when a classifier is connected, to its provider.

## Updates

Once a day, in the background, `reviewers` looks for a newer release. When there is one, it says so after every commit review, at the top of this guide, and in `reviewers status` (`update` in `--json`). Then run `reviewers upgrade` between tasks, never while a commit is waiting on its review, and tell the person what changed: `reviewers upgrade --check` prints the release notes. An upgrade checks the download's checksum and keeps every Reviewer, setting and review; it refreshes the skill and the hooks itself.

## Environment, files and exit codes

`reviewers` with no command starts the first run when there are no Reviewers yet (in a terminal), shows this repo's status inside a judged repo, and prints the help anywhere else.

Environment variables:

- `REVIEWERS_BYPASS=1`: the hooks let the commit through without reviewing it. Only when the person asks.
- `REVIEWERS_FRESH=1`: judge again even code that was judged before, instead of giving the earlier verdict back. Only when the person asks.
- `REVIEWERS_HOME`: where everything is kept, instead of `~/.reviewers`. To move existing data: move the whole folder; set the variable wherever commits happen (the hooks read it from the committing shell); if `bin/reviewers` moved with it, fix the PATH line the installer added (marked `# added by reviewers`); then, if the global hooks were on, run `reviewers hooks install --global` again, and run `reviewers hooks install --all` for repos with their own install, so every hook calls the program where it now is. Settings, including the global hooks folder that was set before, live in `reviewers.sqlite` and move with it.
- `REVIEWERS_TRACE_DIR`: keeps every reviewer session's raw output stream there, one `.jsonl` file each, for a review that ended without an answer: `REVIEWERS_TRACE_DIR=/tmp/reviewers-trace git commit …`.
- `REVIEWERS_RELEASES_URL`: where `reviewers upgrade`, the daily check and the installer read the release manifest, instead of `https://reviewers.sh/releases/latest.txt`.
- `REVIEWERS_NO_UPDATE_CHECK=1`: no daily look for a newer release.
- `REVIEWERS_NO_ONBOARD=1`: the installer doesn't start the first run.
- `NO_COLOR`: plain text, no colors.
- `CLAUDE_CONFIG_DIR`, `CODEX_HOME`, `XDG_CONFIG_HOME`: where `reviewers skill install` finds Claude Code, Codex and OpenCode. Onboarding also reads transcripts from `CLAUDE_CONFIG_DIR` and `CODEX_HOME`.

Files, in `~/.reviewers` (or `REVIEWERS_HOME`):

- `reviewers.sqlite`: Reviewers, repos, every review with its diff and sessions, eval cases, settings.
- `bin/reviewers`: the program, when the installer put it there.
- `hooks/`: the global hooks, when `reviewers hooks install --global` is on.
- `classifier.json`: the classifier's provider and key, readable by the owner only.
- `onboard/<date>/`: what each first run read and found.
- `suggest/<date>/`: what each `reviewers suggest` read and found; `suggest/state.json`: how far each repo's sessions have been read, and the rules seen once that wait to come up again.

The skill itself is `~/.agents/skills/reviewers`, linked into each agent's skills folder (`reviewers skill install --json` lists them).

Exit codes: the commit hook exits 0 when every Reviewer approves, when only advisory ones block or fail, or when `REVIEWERS_BYPASS=1` skips it; 1 when a blocking Reviewer blocks; and 3 when the commit couldn't be reviewed (a blocking Reviewer reached no verdict, or Reviewers' data couldn't be read in a repo it judges), which wins over 1. Anything but 0 stops the commit (`reviewers help hooks`). `reviewers eval` exits 1 when a case doesn't match. Every other command exits 0, or 1 with the error on stderr. A usage error (an unknown flag, a missing argument) exits 2 and prints the usage, from any command.

## Commands

A command that has `--json` lists it in its arguments below; agents should always pass it where it exists. Commands without it print a sentence when they change something, and `reviewers model`, `reviewers classifier cutoff` and `reviewers upgrade --check` print a single value. A flag listed without `<VALUE>` takes no value: `--json`, never `--json true`.

### `reviewers onboard`

Read your agent sessions and turn the rules you keep repeating into Reviewers

  --since <SINCE>: How far back to read, in days (default: 90)
  --repos <REPOS>: Only these repos, by folder name, comma-separated (skips the repo picker)
  --context <CONTEXT>: Pool evidence from all selected repos, or keep each project's evidence separate, for this run (skips the picker). Defaults to `reviewers context`; one of `all`, `project`
  --reread: Read sessions in the window again, including ones already read
  --model <MODEL>: The model for extraction and merging: a Claude id/alias, `codex:<model>` or `codex` (skips the picker)
  --parallel <PARALLEL>: Agents running at once (default: 8)
  --max <MAX>: At most this many Reviewers
  --yes: Take the defaults without asking: every repo, your usual model, the strongest Reviewers on, hooks installed
  --dry-run: Build the material and stop before any agent runs
  --no-skill: Don't give your coding agents the reviewers skill

### `reviewers suggest`

Read only the sessions since the last read and suggest new Reviewers, each with why and the words behind it

  --since <SINCE>: How far back to read at most, in days (default: 90)
  --reread: Read every session in the window again, even the ones already read
  --context <CONTEXT>: Pool evidence from all selected repos, or keep each project's evidence separate, for this run (skips the picker). Defaults to `reviewers context`; one of `all`, `project`
  --repos <REPOS>: Only these repos, by folder name, comma-separated
  --model <MODEL>: The model the agents run on: a Claude id or alias, `codex`, or `codex:<model-id>`. Defaults to the one you used most lately
  --parallel <PARALLEL>: Agents running at once (default: 8)
  --max <MAX>: At most this many suggestions
  --yes: Add every suggestion without asking

### `reviewers status`

This repo: the Reviewers that judge it and its latest reviews

  --json: Print JSON instead of text, for agents and scripts

### `reviewers check`

Judge the change now, before committing it: what a commit of it would get. Committing the same change afterwards reuses these verdicts

  <REVIEWERS>: Only these Reviewers, by name, slug or id. Without any, every Reviewer the commit would run
  --staged: Judge only what's staged, as `git commit` would, instead of everything `git add -A` would stage
  --json: Print JSON instead of text, for agents and scripts

### `reviewers list`

List Reviewers: this repo's, or every one with --all

  --all: Every Reviewer, not just the ones judging this repo
  --json: Print JSON instead of text, for agents and scripts

### `reviewers show`

One Reviewer: its instruction, where it runs, and its record

  <REVIEWER> (required): Name, slug or id
  --decisions <DECISIONS>: How many recent decisions to list (default: 10)
  --json: Print JSON instead of text, for agents and scripts

### `reviewers new`

Create a Reviewer

  --name <NAME> (required): Short name, stated as the rule: "No type casts"
  --instruction <INSTRUCTION> (required): The rule, as a brief for the judge: what to block, what's allowed, real examples
  --everywhere: Judge every repo instead of just this one
  --repo <REPOS>: Repos to judge (paths); repeat it for several. Defaults to the repo you're in
  --paths <PATHS>: Only run when the diff touches these globs, comma-separated: `convex/**,shared/**`
  --context-files <CONTEXT_FILES>: Repo files attached to every review as reference, comma-separated
  --model <MODEL>: Model for this Reviewer: a Claude id/alias, `codex:<model>` or `codex`. Otherwise inherits the repo's or default model
  --classifier <CLASSIFIER>: When the classifier may skip it: `default`, `off`, or a cutoff from 0 to 1
  --disabled: Create it turned off
  --advisory: Report its blocks without stopping the commit
  --reads-text: Judge text files too (Markdown and the like, `reviewers text-files`), for a Reviewer about prose

### `reviewers edit`

Change a Reviewer. A new instruction is a new version

  <REVIEWER> (required): Name, slug or id
  --name <NAME>: A new name
  --instruction <INSTRUCTION>: A new instruction is a new version
  --paths <PATHS>: Comma-separated globs, or `none`
  --context-files <CONTEXT_FILES>: Comma-separated files, or `none`
  --model <MODEL>: A model id or alias, or `default` to use the repo's or the default model (`reviewers model`)
  --everywhere: Judge every repo
  --repos-only: Judge only linked repos
  --add-repo <ADD_REPO>: Link a repo (path)
  --remove-repo <REMOVE_REPO>: Unlink a repo (path)
  --advisory: Report its blocks without stopping the commit; `--blocking` undoes it. Can't be combined with `--blocking`
  --blocking: Stop the commit when it blocks (the default)
  --reads-text: Judge text files too (Markdown and the like, `reviewers text-files`); `--skips-text` undoes it. Can't be combined with `--skips-text`
  --skips-text: Leave text files out again (the default)
  --classifier <CLASSIFIER>: When the classifier may skip it: `default`, `off`, or a cutoff from 0 to 1

### `reviewers enable`

Turn Reviewers on

  <REVIEWERS> (required): Names, slugs or ids

### `reviewers disable`

Turn Reviewers off without deleting them or their history

  <REVIEWERS> (required): Names, slugs or ids

### `reviewers remove`

Delete a Reviewer and its eval cases. Past decisions stay in the history

  <REVIEWER> (required): Name, slug or id

### `reviewers runs`

Recent commit reviews: this repo's, or every repo's with --all

  --all: Every repo, not just this one
  --evals: Include eval runs
  --limit <LIMIT>: How many runs to list (default: 20)
  --prune-evals: Delete every eval run kept by `reviewers eval --record`. Eval scores (`reviewers evals`) stay; commit reviews are never deleted in bulk
  --json: Print JSON instead of text, for agents and scripts

### `reviewers run`

One review in full: every verdict, its evidence and reasoning

  <ID> (required): The run id, from the hook's output or `reviewers runs`
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
  --expect <EXPECT> (required): one of `approved`, `blocked`
  --staged: Only what's staged, not the whole working tree
  --from-run <FROM_RUN>: The diff a past review judged
  --patch <PATCH>: A patch file
  --repo <REPO>: The repo the diff belongs to. Defaults to the current directory
  --include <INCLUDE>: Extra globs to include in the snapshot, comma-separated
  --replace: Overwrite a case with the same name

### `reviewers case list`

A Reviewer's cases

  <REVIEWER> (required): Name, slug or id of the Reviewer
  --json: Print JSON instead of text, for agents and scripts

### `reviewers case update`

Rename a case or flip its expected verdict

  <CASE> (required): The case id, from `reviewers case list`
  --name <NAME>: A new name
  --expect <EXPECT>: one of `approved`, `blocked`

### `reviewers case remove`

Delete a case. Past eval batches keep their record of it

  <CASE> (required): The case id, from `reviewers case list`

### `reviewers eval`

Run a Reviewer's eval cases and record the score

  <REVIEWER> (required): Name, slug or id
  --only <ONLY>: Only cases whose name contains this
  --record: Also keep each judged case as an eval run, so `reviewers run <id> --session` can show it
  --json: Print JSON instead of text, for agents and scripts

### `reviewers evals`

A Reviewer's eval history, newest first

  <REVIEWER> (required): Name, slug or id
  --limit <LIMIT>: How many batches to list (default: 10)
  --delete <DELETE>: Delete one batch instead
  --json: Print JSON instead of text, for agents and scripts

### `reviewers repos`

Repos registered with Reviewers, and how many Reviewers judge each; ignored ones are marked

  --json: Print JSON instead of text, for agents and scripts

### `reviewers init`

Start judging this repo (or PATH): register it and install its hooks

  <PATH>: Any path inside the repo; defaults to the current directory

### `reviewers ignore`

Stop judging this repo (or PATH), even under the global hooks. `reviewers init` undoes it

  <PATH>: Any path inside the repo; defaults to the current directory

### `reviewers hooks install`

Install the hooks in this repo (or PATH), in every registered repo with --all, or for every repo on this machine with --global

  <PATH>: Any path inside the repo; defaults to the current directory
  --all: Every registered repo that isn't ignored
  --global: Through git's global core.hooksPath: every repo without its own hooks folder, new ones included, with no setup. Each repo's own hooks still run first (or, if another global hooks folder was set before, that folder's)

### `reviewers hooks uninstall`

Remove the hooks from this repo (or PATH), or the global ones with --global, setting back what was there before

  <PATH>: Any path inside the repo; defaults to the current directory
  --global: The global hooks, setting git's global core.hooksPath back to what it was

### `reviewers hooks status`

Whether the global hooks are on, and how each registered repo is covered

  --json: Print JSON instead of text, for agents and scripts

### `reviewers model`

Show or set the model Reviewers run on. A Reviewer's own model (`edit --model`) wins, then the repo's, then this default, then Claude Code's own

  <MODEL>: A Claude id/alias (sonnet, opus), `codex:<model>` or `codex` for Codex's default. `default` clears this level. Omit to print the current one
  --repo: For the repo you're in only

### `reviewers context`

Show or set the default context for onboarding and `suggest`: pool evidence from all selected repos, or keep each project's separate. Unset, they ask each time, and need --context without a terminal

  <CONTEXT>: `all`, `project`, or `default` to clear it. Omit to print the current one; one of `all`, `project`, `default`

### `reviewers text-files`

Show or set which files count as text (Markdown and the like). A change to only text files starts no Reviewer, except those made with `--reads-text`

  <GLOBS>: Comma-separated globs like `--paths`, `none` for no text files, or `default` to go back to the default. Omit to print the current ones
  --json: Print JSON instead of text, for agents and scripts

### `reviewers classifier status`

What's connected, the cutoffs, and what it cleared lately

  --json: Print JSON instead of text, for agents and scripts

### `reviewers classifier connect`

Connect Jev, TypeSafe's evaluation model, with a key read from a hidden prompt or from stdin. One small call checks it first

  <PROVIDER> (required): one of `jev` (TypeSafe's own API, with a TypeSafe key), `gateway` (Jev through the Vercel AI Gateway, with an AI Gateway key)
  --endpoint <ENDPOINT>: A SystemOne-compatible URL of your own, instead of the provider's

### `reviewers classifier disconnect`

Forget the key. Every Reviewer runs in full again

### `reviewers classifier cutoff`

Show or set the default cutoff, from 0 to 1. Under it, the classifier clears a Reviewer

  <VALUE>: From 0 to 1; leave it out to print the current one

### `reviewers classifier check`

Replay recent commits from every repo through the classifier: what it would have cleared, and any block it would have missed. Makes real calls on the key

  --runs <RUNS>: How many recent commits (default: 50)
  --json: Print JSON instead of text, for agents and scripts

### `reviewers skill install`

Find the coding agents on this machine and give each the skill

  --json: Print JSON instead of text, for agents and scripts

### `reviewers skill status`

Where the skill is, and which agents have it

  --json: Print JSON instead of text, for agents and scripts

### `reviewers skill uninstall`

Take the skill away from every agent

### `reviewers upgrade`

Update reviewers to the latest release

  --check: Only say whether a newer release exists, and its notes

### `reviewers help`

What you can do. Agents: `reviewers help --agent`

  <TOPIC>: A topic: writing, evals, onboarding, hooks, classifier, commands
  --agent: The full guide for coding agents: how Reviewers work and every command


## Topics

- `reviewers help writing`: How to write a Reviewer that judges well
- `reviewers help evals`: Cases and the tuning loop
- `reviewers help onboarding`: How the first run finds your rules
- `reviewers help hooks`: What happens on a commit, and how to skip it
- `reviewers help classifier`: Skipping the Reviewers a change can't concern

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
- `--reads-text` (on `new` or `edit`; `edit --skips-text` undoes it) lets a Reviewer about prose run on text files. See "What a Reviewer sees".
- `--context-files LAYOUT.md` attaches a file to every review as reference.
- `--advisory` (on `new` or `edit`) reports its blocks without stopping the commit, while a rule is being tuned; `edit --blocking` makes it enforce again. Its blocks count in its own block rate in `reviewers stats`, but a commit only advisory Reviewers blocked counts as passed.

## What a Reviewer sees

The staged diff, its instruction, and its context files. It can open and search any file in the working tree (unstaged edits included), but can't change anything or run commands. In an eval it sees the case's snapshot instead of the repo.

A file `.gitattributes` marks `linguist-generated` (`convex/_generated/** linguist-generated`) shows in the diff as one line, how many lines it added and removed, never its contents. GitHub collapses the same files in pull requests. A commit that only changes generated files isn't judged.

Text files (`**/*.{md,mdx,markdown,txt,rst,adoc}` unless changed) don't start Reviewers: a commit that only changes docs or skill files runs no session, and the hook says so. A Reviewer judges them only if it was made with `--reads-text`, which a Reviewer about prose needs (a docs style, a changelog format). A commit that changes code and text together still runs the other Reviewers, and the diff they see includes the text files. `reviewers text-files` prints which files count as text; `reviewers text-files "docs/**,**/*.md"` sets them, `none` makes every file count as code, `default` restores the list.

## Versions

A new instruction is a new version (`reviewers edit <name> --instruction "…"`). Nothing else makes one: name, paths, model, scope and context files change in place. `reviewers show <name> --json` lists every version that judged something, with its instruction, when it was first used and how many decisions it made (the text output lists the versions when there's more than one; the current instruction is always shown); every eval batch keeps its version too (`reviewers evals <name>`). There's no rollback command: to go back, edit the instruction to the earlier text, which makes a new version.

## Model

A Reviewer runs on its own model if it has one (`--model sonnet`), else the repo's (`reviewers model sonnet --repo`), else the default (`reviewers model sonnet`), else Claude Code's own. Any id or alias `claude --model` accepts works. `default` clears a level: `reviewers edit <name> --model default`, `reviewers model default --repo`, `reviewers model default`.

Use `codex` for Codex's built-in default or `codex:<model-id>` for a specific Codex model at any of those levels. Unqualified ids and aliases retain their Claude meaning; `claude:<model-id>` is also accepted. These settings apply to evals as well as commit reviews. The transcript source does not determine which provider runs a Reviewer.

Codex needs a recent CLI supporting `exec --json --output-schema --ephemeral --ignore-user-config --ignore-rules`, installed and signed in. Runs use its read-only sandbox with approvals, hooks, web search and subagents disabled. User configuration and execution rules are ignored; saved CLI authentication is reused. The merge step also disables shell tools. Custom model ids should be supplied explicitly; custom providers configured only in user settings are not loaded.

## Cost

Every Reviewer that applies is an agent session on every commit, all of them in parallel. A commit waits for the slowest. Ten Reviewers that each take 30 seconds still cost ten sessions of tokens. A rule a linter can enforce should be a lint rule.

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

A case that comes back `not run` fell outside the Reviewer's `--paths`, or holds only text files it doesn't read (`--reads-text`), so it was never judged. It fails whatever it expected: an approval nobody gave proves nothing. Widen the paths, or remove the case if it no longer belongs to this Reviewer.

---

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

---

# What happens on a commit

`reviewers check` runs the same judgement before the commit, and the commit of that same change gets its verdicts back without running: see `reviewers help --agent`.

## Which repos run Reviewers

- **Every repo, with the global hooks.** `reviewers hooks install --global` points git's global `core.hooksPath` at `hooks/` in the data folder (`~/.reviewers`, or `REVIEWERS_HOME`), so every repo on the machine runs Reviewers, new repos and fresh clones included. With a global hooks folder, git no longer runs a repo's own hooks in `.git/hooks`, so each script there runs the repo's own hook of the same name first (`pre-commit`, `pre-push` and the rest), and a commit its own `commit-msg` refuses never reaches Reviewers. If another global hooks folder was set before, its hooks run instead of the repo's own, exactly as git ran them; `reviewers hooks uninstall --global` puts that setting back. A repo nobody added is registered on its first commit, if a Reviewer applies to every repo.
- **One repo at a time, without them.** `reviewers init` registers the repo you're in and installs `commit-msg` and `post-commit` in its hooks folder.
- **A repo with its own hooks folder** (one where `git config core.hooksPath` prints something, as husky sets up) is never reached by the global hooks, because git prefers the repo's folder. Tools that install into `.git/hooks`, like lefthook by default, need nothing: the global hooks run them first. Run `reviewers init` there. It writes `commit-msg` and `post-commit` into that folder, except over a file another tool already has there: it prints `left alone: another tool's hook is there` for each one it skipped. For each skipped hook, add its line to that tool's own hook file:

  ```sh
  reviewers hook commit-msg "$1"   # in the commit-msg hook
  reviewers hook post-commit       # in the post-commit hook
  ```

  Husky keeps its own wrappers in `.husky/_`, so `init` skips both there: put the lines in `.husky/commit-msg` and `.husky/post-commit`, creating the files if they don't exist. With lefthook, when `init` skips its hooks (its own folder, or the global hooks off), add them to `lefthook.yml`; lefthook passes the message file as `{1}`:

  ```yaml
  commit-msg:
    commands:
      reviewers:
        run: reviewers hook commit-msg {1}
  post-commit:
    commands:
      reviewers:
        run: reviewers hook post-commit
  ```

  `reviewers hook` is the command every Reviewers hook runs. It's kept out of the command list because only hooks call it: Reviewers' own, and another tool's as above.

- **Not this repo:** `reviewers ignore` stops Reviewers judging it, even under the global hooks; `reviewers init` turns them back on.

`reviewers hooks status` says whether the global hooks are on, then one line per registered repo (`reviewers repos` lists them; a repo never added doesn't appear): `covered by the global hooks`, `ignored`, or, for a repo with its own install, each hook as `installed`, `missing` or `left alone: another tool's hook is there`.

## A commit

1. **commit-msg** takes the staged diff and picks every enabled Reviewer that applies: its scope (every repo, or this one) and its `--paths` match the files changed. Text files (`reviewers text-files`) count only for Reviewers made with `--reads-text`; a commit that changes nothing else is approved without starting a session.
2. A Reviewer already asked exactly this (the same diff, and the same name, instruction, model, and context files with the same contents) gets that verdict back without running: committing unchanged code after a block brings the same block back at once. Change the code, or the Reviewer if it's wrong. Only when the person asks for a fresh judgement of the same code: `REVIEWERS_FRESH=1 git commit …`.
3. With a classifier connected, one quick call clears the Reviewers the change can't concern (`reviewers help classifier`).
4. The rest run at once, each a read-only Claude Code or Codex session in the repo. Claude uses Read/Grep/Glob; Codex can run read-only shell commands inside its sandbox, with approvals disabled. Neither can edit the repository. The diff it judges is the staged change; files it opens are read from the working tree, unstaged edits included.
5. A block stops the commit and prints, for each block, the file, the lines and the fix. A block from an advisory Reviewer (`--advisory`) is printed as a note, and the commit goes through.
6. **post-commit** ties the landed commit to the review that let it through, so the history shows which commit each review became.

A Reviewer that doesn't answer within 5 minutes, crashes, or answers without a verdict stops the commit too: it fails closed. An advisory one doesn't: it never stops a commit. If Reviewers' own data can't be read, a repo it judges stops the commit, and every other repo lets it through with a warning: every judged repo carries `reviewers.judged=true` in its git config so the hook can tell without the data. The 5 minutes can't be changed. To see what went wrong, commit with `REVIEWERS_TRACE_DIR=/tmp/reviewers-trace git commit …`: every session's raw output is kept there, one `.jsonl` file each.

## Exit codes of `commit-msg`

- `0`: every Reviewer approved, only advisory Reviewers blocked or failed, or `REVIEWERS_BYPASS=1` was set.
- `1`: a Reviewer blocked.
- `3`: the commit couldn't be reviewed: a blocking Reviewer reached no verdict, or Reviewers' own data couldn't be read in a repo it judges. This wins over `1` when both happen in one commit.
- `2`: not from a review: `reviewers hook` was called wrong (a broken line in another tool's hook), and the usage is printed.

Anything but `0` stops the commit.

## Skipping

Skip Reviewers once with `REVIEWERS_BYPASS=1 git commit …`, or every hook with `git commit --no-verify`. Agents shouldn't do either unless the person asks. A skipped commit leaves no review in the history.

Every review is kept: `reviewers runs`, `reviewers run <id>`, `reviewers stats`.

---

# The classifier

Jev is an evaluation model by TypeSafe: it answers closed questions with a probability, cheaply and fast. SystemOne is TypeSafe's API for it.

A cheap first pass before the reviewer sessions. On each commit, one call to Jev, an evaluation model, asks about every Reviewer at once: does any line this change adds break the rule? A rule the change doesn't concern counts as kept, so a rule on TypeScript types is cleared for a change to a README, and so is one whose added code plainly keeps it. Jev answers each with a probability. A Reviewer scored under its cutoff is approved without a session (cleared); the others run as usual.

The classifier never blocks a commit: a high score only means the Reviewer runs. A file too large to read whole is read in pieces. When it can't answer (an error, a timeout, a change too large even in pieces), every Reviewer runs in full.

## Connect

Jev is reachable two ways:

- `reviewers classifier connect gateway`, with a Vercel AI Gateway key (Vercel dashboard, AI Gateway, API keys).
- `reviewers classifier connect jev`, with a TypeSafe key (console.typesafe.ai, Settings, Keys).

The key comes from a hidden prompt, or from stdin, best from a file or a password manager so it never sits in shell history: `reviewers classifier connect gateway < key.txt`. `--endpoint` points it at another service that speaks the chosen provider's API (SystemOne's for `jev`, the AI Gateway's evaluation-model API for `gateway`), with that service's key. One small call checks it before it's saved as `classifier.json` in the data folder (`~/.reviewers`, or `REVIEWERS_HOME`), readable by the owner only. `reviewers classifier disconnect` forgets it. While connected, each commit's diff and the Reviewers' rules go to that provider.

## Cutoffs

The default cutoff is 17%: a Reviewer is cleared when Jev puts the chance of a broken rule under 17%. Change it with `reviewers classifier cutoff 0.2`. One Reviewer can have its own, `reviewers edit <name> --classifier 0.1`, or never be cleared, `--classifier off`. Use `off` for a rule the diff alone can't settle, one that depends on files the change doesn't show.

## Measure it

`reviewers classifier check` replays recent commits from every repo (`--runs`, 50 by default) through the classifier and compares it with what the Reviewers decided. It needs a connection and makes real calls, one or more per commit, billed to the key like any other. It reports how many sessions it would have skipped, the tokens that saves, and any block it would have let through. "safe under" (`lowestBlockScore` in `--json`) is the lowest score any of a Reviewer's past blocks got: a cutoff at or under it misses none of them. `--json` also lists every decision's score, so any cutoff can be tried without calling again. A Reviewer with missed blocks needs a cutoff below that, or `--classifier off`.

Every cleared decision keeps its score: `reviewers run <id>` shows it, and `reviewers classifier status` counts them.
