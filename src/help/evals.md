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
