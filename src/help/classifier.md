# The classifier

A cheap first pass before the Claude sessions. On each commit, one call to Jev, an evaluation model, asks about every Reviewer at once: does this change break the rule? Jev answers each with a probability. A Reviewer scored under its cutoff is approved without a session (cleared); the others run as usual.

The classifier never blocks a commit: a high score only means the Reviewer runs. When it can't answer (an error, a timeout, a file too large to read whole), every Reviewer runs in full.

## Connect

Jev is reachable two ways:

- `reviewers classifier connect gateway`, with a Vercel AI Gateway key.
- `reviewers classifier connect jev`, with a TypeSafe key.

The key comes from a hidden prompt, or from stdin: `printf %s "$KEY" | reviewers classifier connect gateway`. One small call checks it before it's saved in `~/.reviewers/classifier.json`, readable by the owner only. `reviewers classifier disconnect` forgets it. While connected, each commit's diff and the Reviewers' rules go to that provider.

## Cutoffs

The default cutoff is 15%: a Reviewer is cleared when Jev puts the chance of a broken rule under 15%. Replayed over 183 real commits, that skipped about half the sessions and missed none of 30 blocks; 25% missed 3. Change it with `reviewers classifier cutoff 0.2`. One Reviewer can have its own, `reviewers edit <name> --classifier 0.1`, or never be cleared, `--classifier off`. Use `off` for a rule the diff alone can't settle, one that depends on files the change doesn't show.

## Measure it

`reviewers classifier check` replays recent commits through the classifier and compares it with what the Reviewers decided: how many sessions it would have skipped, the tokens that saves, and any block it would have let through. "safe under" is the highest cutoff that misses none of a Reviewer's past blocks. A Reviewer with missed blocks needs a cutoff below that, or `--classifier off`.

Every cleared decision keeps its score: `reviewers run <id>` shows it, and `reviewers classifier` counts them.
