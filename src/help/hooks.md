# What happens on a commit

`reviewers hooks install --global` covers every repo on the machine through git's global `core.hooksPath`, new repos and fresh clones included, with no setup. Under a global hooks folder git stops running each repo's own hooks, so every hook in it runs the repo's own first (`pre-commit`, `pre-push` and the rest). A global hooks folder set before keeps running in their place, as git did, and `reviewers hooks uninstall --global` sets it back. A repo nobody added is registered on its first commit when a Reviewer applies to every repo. `reviewers ignore` stops Reviewers judging a repo; `reviewers init` turns them back on.

Without the global hooks, `reviewers init` registers one repo and installs two hooks in it. A repo that sets its own `core.hooksPath` (husky, lefthook) always needs this, since git prefers its own folder over the global one. A hook another tool already owns is left alone; to run Reviewers from it, add this line to that tool's `commit-msg` hook:

```sh
reviewers hook commit-msg "$1"
```

- **commit-msg** takes the staged diff and runs every enabled Reviewer whose scope and `--paths` match, all at once. Each is a read-only Claude Code session in the repo: it can open any file, but can't change anything or run commands. If any blocks, the commit stops with the file, lines and fix for each block. If a Reviewer can't reach a verdict (a crash, a timeout), the commit stops too: it fails closed. With a classifier connected, one quick call first clears the Reviewers the change can't concern, and only the rest start a session (`reviewers help classifier`).
- **post-commit** ties the landed commit to the review that let it through, so the history shows which commit each review became.

Skip the hooks once with `REVIEWERS_BYPASS=1 git commit …`. Agents shouldn't, unless the person asks.

Every review is kept: `reviewers runs`, `reviewers run <id>`, `reviewers stats`.
