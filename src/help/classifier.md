# The classifier

Jev is an evaluation model by TypeSafe: it answers closed questions with a probability, cheaply and fast. SystemOne is TypeSafe's API for it.

A cheap first pass before the reviewer sessions. On each commit, one call to Jev, an evaluation model, asks about every Reviewer at once: does this change break the rule? Jev answers each with a probability. A Reviewer scored under its cutoff is approved without a session (cleared); the others run as usual.

The classifier never blocks a commit: a high score only means the Reviewer runs. A file too large to read whole is read in pieces. When it can't answer (an error, a timeout, a change too large even in pieces), every Reviewer runs in full.

## Connect

Jev is reachable two ways:

- `reviewers classifier connect gateway`, with a Vercel AI Gateway key (Vercel dashboard, AI Gateway, API keys).
- `reviewers classifier connect jev`, with a TypeSafe key (console.typesafe.ai, Settings, Keys).

The key comes from a hidden prompt, or from stdin, best from a file or a password manager so it never sits in shell history: `reviewers classifier connect gateway < key.txt`. `--endpoint` points it at another service that speaks the chosen provider's API (SystemOne's for `jev`, the AI Gateway's evaluation-model API for `gateway`), with that service's key. One small call checks it before it's saved as `classifier.json` in the data folder (`~/.reviewers`, or `REVIEWERS_HOME`), readable by the owner only. `reviewers classifier disconnect` forgets it. While connected, each commit's diff and the Reviewers' rules go to that provider.

## Cutoffs

The default cutoff is 15%: a Reviewer is cleared when Jev puts the chance of a broken rule under 15%. Change it with `reviewers classifier cutoff 0.2`. One Reviewer can have its own, `reviewers edit <name> --classifier 0.1`, or never be cleared, `--classifier off`. Use `off` for a rule the diff alone can't settle, one that depends on files the change doesn't show.

## Measure it

`reviewers classifier check` replays recent commits from every repo (`--runs`, 50 by default) through the classifier and compares it with what the Reviewers decided. It needs a connection and makes real calls, one or more per commit, billed to the key like any other. It reports how many sessions it would have skipped, the tokens that saves, and any block it would have let through. "safe under" (`lowestBlockScore` in `--json`) is the lowest score any of a Reviewer's past blocks got: a cutoff at or under it misses none of them. `--json` also lists every decision's score, so any cutoff can be tried without calling again. A Reviewer with missed blocks needs a cutoff below that, or `--classifier off`.

Every cleared decision keeps its score: `reviewers run <id>` shows it, and `reviewers classifier status` counts them.
