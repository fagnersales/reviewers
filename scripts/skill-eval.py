#!/usr/bin/env python3
"""Checks when coding agents pick up the reviewers skill: triggers only, not what they do after.

Each query in evals/skill/triggers.json goes to `claude -p` in an empty repo that holds only this
repo's src/skill/SKILL.md, a few times over, and counts how often the agent opens the skill. A
query passes when it opens the skill in at least half its runs and should, or in fewer and
shouldn't. Every run is a real Claude Code session on your own account.

    scripts/skill-eval.py                     # every query, 3 runs each
    scripts/skill-eval.py --runs 1 --only no  # queries containing "no"
"""
import argparse, json, os, select, shutil, subprocess, sys, tempfile, time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def opens_skill(query, skill_text, model, timeout):
    """Whether the agent opened the skill at any point in its run, not just its first step: a
    correction mid-task is meant to open it alongside the work."""
    workspace = Path(tempfile.mkdtemp(prefix="skill-eval-"))
    try:
        (workspace / ".claude/skills/reviewers").mkdir(parents=True)
        (workspace / ".claude/skills/reviewers/SKILL.md").write_text(skill_text)
        subprocess.run(["git", "init", "-q"], cwd=workspace, check=True)
        # Project settings only: the person's own skills, the installed reviewers skill among
        # them, stay out of the run.
        command = ["claude", "-p", query, "--setting-sources", "project", "--output-format", "stream-json", "--verbose", "--max-turns", "8"]
        if model:
            command += ["--model", model]
        environment = {key: value for key, value in os.environ.items() if key != "CLAUDECODE"}
        process = subprocess.Popen(command, cwd=workspace, env=environment, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
        deadline = time.time() + timeout
        try:
            while time.time() < deadline:
                ready, _, _ = select.select([process.stdout], [], [], 1.0)
                if not ready:
                    if process.poll() is not None:
                        return False
                    continue
                line = process.stdout.readline()
                if not line:
                    return False
                try:
                    event = json.loads(line)
                except json.JSONDecodeError:
                    continue
                if event.get("type") != "assistant":
                    continue
                for block in event["message"].get("content", []):
                    if block.get("type") != "tool_use":
                        continue
                    arguments = block.get("input", {})
                    if block["name"] == "Skill" and arguments.get("skill", "").split(":")[-1] == "reviewers":
                        return True
                    if block["name"] == "Read" and arguments.get("file_path", "").endswith("skills/reviewers/SKILL.md"):
                        return True
            return False
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
    finally:
        shutil.rmtree(workspace, ignore_errors=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--cases", default=ROOT / "evals/skill/triggers.json", type=Path)
    parser.add_argument("--skill", default=ROOT / "src/skill/SKILL.md", type=Path, help="The SKILL.md to test")
    parser.add_argument("--runs", default=3, type=int, help="Runs per query")
    parser.add_argument("--only", help="Only queries containing this")
    parser.add_argument("--model", help="The model for claude -p; your usual one by default")
    parser.add_argument("--parallel", default=8, type=int)
    parser.add_argument("--timeout", default=120, type=int, help="Seconds per run")
    parser.add_argument("--json", action="store_true", help="Print JSON instead of text")
    args = parser.parse_args()

    cases = [case for case in json.loads(args.cases.read_text()) if not args.only or args.only in case["query"]]
    skill_text = args.skill.read_text()
    jobs = [(index, case) for index, case in enumerate(cases) for _ in range(args.runs)]
    with ThreadPoolExecutor(args.parallel) as pool:
        opened = list(pool.map(lambda job: opens_skill(job[1]["query"], skill_text, args.model, args.timeout), jobs))

    results = []
    for index, case in enumerate(cases):
        hits = sum(opened[position] for position, (job_index, _) in enumerate(jobs) if job_index == index)
        passed = (hits * 2 >= args.runs) == case["should_trigger"]
        results.append({**case, "opened": hits, "runs": args.runs, "pass": passed})

    failed = [result for result in results if not result["pass"]]
    if args.json:
        print(json.dumps({"passed": len(results) - len(failed), "total": len(results), "results": results}, indent=2, ensure_ascii=False))
    else:
        for result in results:
            mark = "pass" if result["pass"] else "FAIL"
            expected = "should open" if result["should_trigger"] else "shouldn't open"
            print(f"{mark}  {result['opened']}/{result['runs']}  {expected:14}  {result['why']}")
        print(f"\n{len(results) - len(failed)}/{len(results)} passed")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
