---
name: reviewers
description: Reviewers check every commit against the person's rules and stop commits that break them. Use when a commit is blocked by Reviewers, when the person states a rule about code or asks to add, change, tune, remove or check a Reviewer, or asks how their Reviewers are doing.
---

<!-- managed by reviewers: `reviewers skill install` rewrites this file -->

# Reviewers

This machine runs Reviewers: the person's rules about code, each checked on every commit by a git hook. A blocked commit prints what broke the rule and how to fix it.

Before doing anything with Reviewers, run `reviewers help --agent`. It's the current guide and command reference, and it changes with every release, so don't work from memory.
