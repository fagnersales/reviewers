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
- `--advisory` (on `new` or `edit`) reports its blocks without stopping the commit, while a rule is being tuned; `edit --blocking` makes it enforce again. Its blocks count in its own block rate in `reviewers stats`, but a commit only advisory Reviewers blocked counts as passed.

## What a Reviewer sees

The staged diff, its instruction, and its context files. It can open and search any file in the working tree (unstaged edits included), but can't change anything or run commands. In an eval it sees the case's snapshot instead of the repo.

## Versions

A new instruction is a new version (`reviewers edit <name> --instruction "…"`). Nothing else makes one: name, paths, model, scope and context files change in place. `reviewers show <name> --json` lists every version that judged something, with its instruction, when it was first used and how many decisions it made (the text output lists the versions when there's more than one; the current instruction is always shown); every eval batch keeps its version too (`reviewers evals <name>`). There's no rollback command: to go back, edit the instruction to the earlier text, which makes a new version.

## Model

A Reviewer runs on its own model if it has one (`--model sonnet`), else the repo's (`reviewers model sonnet --repo`), else the default (`reviewers model sonnet`), else Claude Code's own. Any id or alias `claude --model` accepts works. `default` clears a level: `reviewers edit <name> --model default`, `reviewers model default --repo`, `reviewers model default`.

## Cost

Every Reviewer that applies is a Claude session on every commit, all of them in parallel. A commit waits for the slowest. Ten Reviewers that each take 30 seconds still cost ten sessions of tokens. A rule a linter can enforce should be a lint rule.
