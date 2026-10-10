# Reviewers {{VERSION}}: guide for coding agents

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
