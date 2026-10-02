use crate::store::{Evidence, Run, Verdict};
use crate::util::{compact, duration};

const PREFIX: &str = "reviewers:";

fn location(evidence: &Evidence) -> String {
    match (evidence.start_line, evidence.end_line) {
        (Some(start), Some(end)) if end != start => format!("{}:{start}-{end}", evidence.file),
        (Some(start), _) => format!("{}:{start}", evidence.file),
        _ => evidence.file.clone(),
    }
}

/// Written for the agent that made the commit: where, what, what to do, and how to see the rest.
pub fn report(run: &Run) -> String {
    let tokens: u64 = run.decisions.iter().map(|decision| decision.usage.tokens_read + decision.usage.tokens_written).sum();
    let footer = format!("{} · {} tokens", duration(run.duration_ms), compact(tokens));
    let blocked: Vec<_> = run.decisions.iter().filter(|decision| decision.verdict == Verdict::Blocked).collect();
    let mut lines = Vec::new();
    if let Some(failure) = &run.failure {
        lines.push(format!("{PREFIX} could not reach a verdict, so the commit is stopped"));
        lines.extend(failure.lines().map(|line| format!("  {line}")));
        lines.push(String::new());
        lines.push("Try the commit again. If it keeps failing: `reviewers run ".to_string() + &run.id + "`.");
        return lines.join("\n");
    }
    if blocked.is_empty() {
        return format!("{PREFIX} {} passed · {footer}", crate::util::plural(run.decisions.len(), "Reviewer"));
    }
    lines.push(format!("{PREFIX} blocked by {} of {} · {footer}", blocked.len(), crate::util::plural(run.decisions.len(), "Reviewer")));
    for decision in blocked {
        lines.push(String::new());
        lines.push(format!("✗ {}", decision.reviewer_name));
        lines.push(format!("  {}", decision.summary));
        for evidence in &decision.evidence {
            lines.push(String::new());
            lines.push(format!("  {}", location(evidence)));
            for excerpt in evidence.excerpt.lines().filter(|line| !line.trim().is_empty()) {
                lines.push(format!("    {}", excerpt.trim_end()));
            }
            lines.push(format!("  → {}", evidence.explanation));
        }
    }
    lines.push(String::new());
    lines.push(format!("Fix the code above and commit again. Full reasoning: `reviewers run {}`.", run.id));
    lines.join("\n")
}
