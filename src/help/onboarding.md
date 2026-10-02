# Onboarding: finding the rules you already hold

`reviewers onboard` (and bare `reviewers` the first time) reads the person's Claude Code sessions from the last 90 days, in every config folder (`~/.claude`, `~/.claude-*`, `$CLAUDE_CONFIG_DIR`), and keeps only what the person typed. Scripted `claude -p` sessions are skipped. Sessions are matched to repos, worktrees included, even deleted ones.

The person picks the repos and the model (the default is the one they used most lately). Then agents read the material in parallel, one share each: a big repo is cut into stretches of time, and small repos share an agent. Each agent reports the rules it finds with evidence: the person's own words, with dates. A final step merges the same rule said in different repos, and decides which rules are personal (`everywhere`) and which belong to one repo.

A rule must have come up at least twice, or have been stated as a standing rule ("always", "never"). Rules a linter could enforce are listed apart.

At the end the person picks which Reviewers start on, and the commit hook is installed in the picked repos. Everything found, and every piece of evidence, is kept in `~/.reviewers/onboard/<date>/`.
