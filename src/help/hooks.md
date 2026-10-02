# What happens on a commit

`reviewers init` registers a repo and installs two git hooks, honoring `core.hooksPath` (husky, lefthook). A hook another tool already owns is left alone.

- **commit-msg** takes the staged diff and runs every enabled Reviewer whose scope and `--paths` match, all at once. Each is a read-only Claude Code session in the repo: it can open any file, but can't change anything or run commands. If any blocks, the commit stops with the file, lines and fix for each block. If a Reviewer can't reach a verdict (a crash, a timeout), the commit stops too: it fails closed.
- **post-commit** ties the landed commit to the review that let it through, so the history shows which commit each review became.

Skip the hooks once with `REVIEWERS_BYPASS=1 git commit …`. Agents shouldn't, unless the person asks.

Every review is kept: `reviewers runs`, `reviewers run <id>`, `reviewers stats`.
