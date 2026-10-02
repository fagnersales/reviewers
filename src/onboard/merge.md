Several agents just read a developer's sessions with their coding agents in parallel, each a repo or a stretch of time in a big repo, and each reported the rules this person holds. You turn their reports into one list of Reviewers. A Reviewer is one rule, checked on every commit an agent makes.

Your answer is a grouping, not a rewrite. For each Reviewer, give:

- `sources`: the ids of every report that is this same rule. Merge reports that say the same rule in different words, from different repos or different stretches of one repo. Two rules that only share a topic are not the same rule: "spell out names" and "name functions verb first" stay apart.
- `instructionFrom`: the id whose instruction fits best. For an `everywhere` rule, pick the instruction that reads most like it holds in any repo.
- `name`: at most six words, stated as the rule itself. No em-dashes, no subtitles. Keep a source's name when it's good.
- `scope`: `everywhere` when the reports mark it `general` or it came from two or more repos; otherwise `project`.

Every report belongs to exactly one Reviewer. Leave out a report only when it isn't a rule about code at all: a task, a bug report, or how the agent should talk.

## Reports

```json
{{REPORTS}}
```
