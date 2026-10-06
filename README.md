# reviewers

Your rules about code, checked on every commit your coding agent makes.

Each Reviewer is one rule ("no type casts", "errors reach users through the error map"). A git hook runs every Reviewer that applies to the staged diff, all at once, each as a read-only Claude Code or Codex session. When one blocks, the commit stops and the agent gets the file, the lines and what to do instead, then fixes it and commits again.

## Install

```sh
curl -fsSL https://reviewers.sh/install | sh
```

The first run reads your Claude Code and Codex sessions, finds the rules you keep repeating to your agents, and turns them into Reviewers. Onboarding and `reviewers suggest` either pool evidence from all selected projects or keep it separate for each project: pick each run, pass `--context all|project`, or set a default with `reviewers context`. Later runs read only sessions that have not already been sent to agents; `--reread` explicitly reads them again.

Requires `git` and either [Claude Code](https://claude.com/claude-code) or [Codex CLI](https://developers.openai.com/codex/cli/), installed and signed in. Reviewers run through your selected CLI, and everything is stored in `~/.reviewers/`. Nothing is sent anywhere else, unless you connect the optional classifier (`reviewers help classifier`), which sends each commit's diff to Jev.

For Codex, use a recent CLI supporting `exec --json --output-schema --ephemeral --ignore-user-config --ignore-rules`:

```sh
reviewers onboard --model codex          # learn from Claude and Codex transcripts using Codex
reviewers model codex                    # use Codex for reviews and evals
reviewers model 'codex:<model-id>' --repo # optional: choose a specific model for this repo
reviewers edit <reviewer> --model sonnet  # individual Reviewers can still use Claude
```

Onboarding discovers Codex JSONL rollouts in `~/.codex/{sessions,archived_sessions}` and `$CODEX_HOME`, alongside Claude transcripts. A Codex model chosen there becomes the review default if none is configured. Transcript source and reviewer model are independent. Existing unqualified model settings still use Claude; `codex` uses Codex's built-in default and `codex:<model-id>` selects a model explicitly.

Codex runs in a read-only sandbox with approvals, hooks and web search disabled. It uses saved CLI authentication, but ignores user configuration and execution rules to isolate reviews from personal tools and permission overrides; select custom models explicitly. Claude keeps its Read/Grep/Glob tool allowlist.

## Use

You mostly don't. Your agent commits, Reviewers judge, your agent fixes. The installer gives every coding agent on the machine a `reviewers` skill, which points it at `reviewers help --agent`, so it knows how to add, tune and check Reviewers when you ask.

```sh
reviewers               # this repo: its Reviewers and latest reviews
reviewers check         # judge the change now; the commit of it reuses these verdicts
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
scripts/skill-eval.py      # when agents open the skill: real Claude Code sessions on your account
scripts/release.sh "note" # tag the version in Cargo.toml; CI builds and publishes it
```
