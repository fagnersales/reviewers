# reviewers

Your rules about code, checked on every commit your coding agent makes.

Each Reviewer is one rule ("no type casts", "errors reach users through the error map"). A git hook runs every Reviewer that applies to the staged diff, all at once, each as a read-only Claude Code session. When one blocks, the commit stops and the agent gets the file, the lines and what to do instead, then fixes it and commits again.

## Install

```sh
curl -fsSL https://reviewers.sh/install | sh
```

The first run reads your Claude Code sessions, finds the rules you keep repeating to your agents, and turns them into Reviewers. Run it again any time with `reviewers onboard`.

Requires `git` and [Claude Code](https://claude.com/claude-code). Reviewers run on your own Claude Code, and everything is stored in `~/.reviewers/`. Nothing is sent anywhere else, unless you connect the optional classifier (`reviewers help classifier`), which sends each commit's diff to Jev.

## Use

You mostly don't. Your agent commits, Reviewers judge, your agent fixes. The installer gives every coding agent on the machine a `reviewers` skill, which points it at `reviewers help --agent`, so it knows how to add, tune and check Reviewers when you ask.

```sh
reviewers               # this repo: its Reviewers and latest reviews
reviewers stats         # what each Reviewer catches, the wait it adds, the tokens it uses
reviewers help          # everything else
reviewers upgrade       # update to the latest release
```

## License

The Fira Code font in `site/fonts` is under the SIL Open Font License; see `site/fonts/OFL.txt`.

## Develop

```sh
cargo build
cargo test
scripts/release.sh      # release binaries and dist/latest.txt
```
