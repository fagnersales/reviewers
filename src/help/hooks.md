# What happens on a commit

## Which repos run Reviewers

- **Every repo, with the global hooks.** `reviewers hooks install --global` points git's global `core.hooksPath` at `hooks/` in the data folder (`~/.reviewers`, or `REVIEWERS_HOME`), so every repo on the machine runs Reviewers, new repos and fresh clones included. With a global hooks folder, git no longer runs a repo's own hooks in `.git/hooks`, so each script there runs the repo's own hook of the same name first (`pre-commit`, `pre-push` and the rest), and a commit its own `commit-msg` refuses never reaches Reviewers. If another global hooks folder was set before, its hooks run instead of the repo's own, exactly as git ran them; `reviewers hooks uninstall --global` puts that setting back. A repo nobody added is registered on its first commit, if a Reviewer applies to every repo.
- **One repo at a time, without them.** `reviewers init` registers the repo you're in and installs `commit-msg` and `post-commit` in its hooks folder.
- **A repo with its own hooks folder** (one where `git config core.hooksPath` prints something, as husky sets up) is never reached by the global hooks, because git prefers the repo's folder. Tools that install into `.git/hooks`, like lefthook by default, need nothing: the global hooks run them first. Run `reviewers init` there. It writes `commit-msg` and `post-commit` into that folder, except over a file another tool already has there: it prints `left alone: another tool's hook is there` for each one it skipped. For each skipped hook, add its line to that tool's own hook file:

  ```sh
  reviewers hook commit-msg "$1"   # in the commit-msg hook
  reviewers hook post-commit       # in the post-commit hook
  ```

  Husky keeps its own wrappers in `.husky/_`, so `init` skips both there: put the lines in `.husky/commit-msg` and `.husky/post-commit`, creating the files if they don't exist. With lefthook, when `init` skips its hooks (its own folder, or the global hooks off), add them to `lefthook.yml`; lefthook passes the message file as `{1}`:

  ```yaml
  commit-msg:
    commands:
      reviewers:
        run: reviewers hook commit-msg {1}
  post-commit:
    commands:
      reviewers:
        run: reviewers hook post-commit
  ```

  `reviewers hook` is the command every Reviewers hook runs. It's kept out of the command list because only hooks call it: Reviewers' own, and another tool's as above.

- **Not this repo:** `reviewers ignore` stops Reviewers judging it, even under the global hooks; `reviewers init` turns them back on.

`reviewers hooks status` says whether the global hooks are on, then one line per registered repo (`reviewers repos` lists them; a repo never added doesn't appear): `covered by the global hooks`, `ignored`, or, for a repo with its own install, each hook as `installed`, `missing` or `left alone: another tool's hook is there`.

## A commit

1. **commit-msg** takes the staged diff and picks every enabled Reviewer that applies: its scope (every repo, or this one) and its `--paths` match the files changed.
2. A Reviewer already asked exactly this (the same diff, and the same name, instruction, model, and context files with the same contents) gets that verdict back without running: committing unchanged code after a block brings the same block back at once. Change the code, or the Reviewer if it's wrong. Only when the person asks for a fresh judgement of the same code: `REVIEWERS_FRESH=1 git commit …`.
3. With a classifier connected, one quick call clears the Reviewers the change can't concern (`reviewers help classifier`).
4. The rest run at once, each a read-only Claude Code session in the repo. It can open and search any file, but can't change anything or run commands. The diff it judges is the staged change; files it opens are read from the working tree, unstaged edits included.
5. A block stops the commit and prints, for each block, the file, the lines and the fix. A block from an advisory Reviewer (`--advisory`) is printed as a note, and the commit goes through.
6. **post-commit** ties the landed commit to the review that let it through, so the history shows which commit each review became.

A Reviewer that doesn't answer within 5 minutes, crashes, or answers without a verdict stops the commit too: it fails closed. An advisory one doesn't: it never stops a commit. If Reviewers' own data can't be read, a repo it judges stops the commit, and every other repo lets it through with a warning: every judged repo carries `reviewers.judged=true` in its git config so the hook can tell without the data. The 5 minutes can't be changed. To see what went wrong, commit with `REVIEWERS_TRACE_DIR=/tmp/reviewers-trace git commit …`: every session's raw output is kept there, one `.jsonl` file each.

## Exit codes of `commit-msg`

- `0`: every Reviewer approved, only advisory Reviewers blocked or failed, or `REVIEWERS_BYPASS=1` was set.
- `1`: a Reviewer blocked.
- `3`: the commit couldn't be reviewed: a blocking Reviewer reached no verdict, or Reviewers' own data couldn't be read in a repo it judges. This wins over `1` when both happen in one commit.
- `2`: not from a review: `reviewers hook` was called wrong (a broken line in another tool's hook), and the usage is printed.

Anything but `0` stops the commit.

## Skipping

Skip Reviewers once with `REVIEWERS_BYPASS=1 git commit …`, or every hook with `git commit --no-verify`. Agents shouldn't do either unless the person asks. A skipped commit leaves no review in the history.

Every review is kept: `reviewers runs`, `reviewers run <id>`, `reviewers stats`.
