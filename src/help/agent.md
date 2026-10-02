# Reviewers {{VERSION}}: guide for coding agents

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
