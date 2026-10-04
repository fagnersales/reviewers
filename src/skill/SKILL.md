---
name: reviewers
description: Reviewers check every commit against the person's rules and stop commits that break them. Use when a commit is blocked by Reviewers, when the person states a rule about code or asks to add, change, tune, remove or check a Reviewer, or asks how their Reviewers are doing. Also use every time the person rejects or corrects something you did, however short or soft, in any language ("no", "undo it", "start over", "use X, not Y", "why did you…", "I'd rather you didn't…"): keep working, and offer in your final message to make that correction a Reviewer. Not for other commit hooks (husky, lint-staged), lint or compiler setup, pull request reviews, or writing about code review.
---

<!-- managed by reviewers: `reviewers skill install` rewrites this file -->

# Reviewers

This machine runs Reviewers: the person's rules about code, each checked on every commit by a git hook. A blocked commit prints what broke the rule and how to fix it.

Before doing anything with Reviewers, run `reviewers help --agent`. It's the current guide and command reference, and it changes with every release, so don't work from memory.

## When the person rejects something you did

A correction is a rule the person may want checked on every commit. Don't stop to ask: do what the correction asks and finish the task. Then, in your final message, ask whether they want a Reviewer for it, with the rule named as it would be ("No type casts on API data"). If `reviewers list --all --json` shows one that already covers it, say which one instead. When they say yes, follow "When the person states a rule" in `reviewers help --agent`.
