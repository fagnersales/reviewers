use crate::store::{Evidence, Run, RunKind, Verdict};
use crate::util::{compact, duration};
use std::io::IsTerminal;

const PREFIX: &str = "reviewers:";

/// Colors when a person commits in a terminal; plain text when an agent reads the hook's output.
pub struct Paint {
    on: bool,
}

impl Paint {
    pub fn for_stderr() -> Paint {
        Paint { on: std::io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none() }
    }

    pub fn for_stdout() -> Paint {
        Paint { on: std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none() }
    }

    #[cfg(test)]
    pub fn plain() -> Paint {
        Paint { on: false }
    }

    fn wrap(&self, code: &str, text: &str) -> String {
        if self.on { format!("\x1b[{code}m{text}\x1b[0m") } else { text.to_string() }
    }

    pub fn bold(&self, text: &str) -> String {
        self.wrap("1", text)
    }
    pub fn dim(&self, text: &str) -> String {
        self.wrap("2", text)
    }
    pub fn red(&self, text: &str) -> String {
        self.wrap("31", text)
    }
    pub fn green(&self, text: &str) -> String {
        self.wrap("32", text)
    }
    pub fn yellow(&self, text: &str) -> String {
        self.wrap("33", text)
    }
    pub fn cyan(&self, text: &str) -> String {
        self.wrap("36", text)
    }
}

fn location(evidence: &Evidence) -> String {
    match (evidence.start_line, evidence.end_line) {
        (Some(start), Some(end)) if end != start => format!("{}:{start}-{end}", evidence.file),
        (Some(start), _) => format!("{}:{start}", evidence.file),
        _ => evidence.file.clone(),
    }
}

/// One line as each Reviewer finishes: its verdict and what it took.
pub fn progress(judged: &super::Judged, paint: &Paint) -> String {
    match &judged.outcome {
        Ok(decision) => {
            let mark = match (decision.verdict, judged.reviewer.blocking) {
                (Verdict::Approved, _) => paint.green("✓"),
                (_, true) => paint.red("✗"),
                (_, false) => paint.yellow("✗"),
            };
            let cleared = decision.classifier.as_ref().filter(|note| note.outcome == crate::classifier::Outcome::Cleared);
            let timing = match (&decision.reused_from, cleared) {
                (Some(run), _) => format!("unchanged since {run}"),
                (None, Some(note)) => format!("cleared by the classifier · {}", super::percent(note.probability.unwrap_or_default())),
                (None, None) => duration(decision.duration_ms),
            };
            format!("  {mark} {} {}", judged.reviewer.name, paint.dim(&timing))
        }
        Err(_) => format!("  {} {} {}", paint.yellow("!"), judged.reviewer.name, paint.dim("no verdict")),
    }
}

/// Written for the agent that made the commit (or ran the check): where, what, what to do, and how to see the rest.
pub fn report(run: &Run, paint: &Paint) -> String {
    let check = run.kind == RunKind::Check;
    let tokens: u64 = run.decisions.iter().map(|decision| decision.usage.tokens_read + decision.usage.tokens_written).sum();
    let footer = paint.dim(&format!("· {} · {} tokens", duration(run.duration_ms), compact(tokens)));
    let (blocked, advisory): (Vec<_>, Vec<_>) = run.decisions.iter().filter(|decision| decision.verdict == Verdict::Blocked).partition(|decision| !decision.advisory);
    let blocked_before = blocked.iter().any(|decision| decision.reused_from.is_some());
    let mut lines = Vec::new();
    if let Some(failure) = &run.failure {
        lines.push(format!("{PREFIX} {}", paint.yellow(if check { "could not reach a verdict" } else { "could not reach a verdict, so the commit is stopped" })));
        lines.extend(failure.lines().map(|line| format!("  {line}")));
        lines.push(String::new());
        let again = if check { "Check again" } else { "Try the commit again" };
        lines.push(format!("{again}: only the Reviewers without a verdict run again. If it keeps failing: `reviewers run {}`.", run.id));
        return lines.join("\n");
    }
    let finding = |lines: &mut Vec<String>, decision: &crate::store::Decision| {
        lines.push(String::new());
        if decision.advisory {
            lines.push(format!("{} {} {}", paint.yellow("✗"), paint.bold(&decision.reviewer_name), paint.dim("(advisory: reported, not enforced)")));
        } else {
            lines.push(format!("{} {}", paint.red("✗"), paint.bold(&decision.reviewer_name)));
        }
        lines.push(format!("  {}", decision.summary));
        for evidence in &decision.evidence {
            lines.push(String::new());
            lines.push(format!("  {}", paint.cyan(&location(evidence))));
            let quoted = crate::diff::find_quote(&run.diff, &evidence.file, &evidence.excerpt)
                .unwrap_or_else(|| evidence.excerpt.lines().filter(|line| !line.trim().is_empty()).map(str::to_string).collect());
            for line in crate::diff::dedent(&quoted) {
                lines.push(format!("    {}", paint.dim(&line)));
            }
            lines.push(format!("  → {}", evidence.explanation));
        }
    };
    if blocked.is_empty() {
        let cleared = run.decisions.iter().filter(|decision| decision.classifier.as_ref().is_some_and(|note| note.outcome == crate::classifier::Outcome::Cleared)).count();
        let mut notes = Vec::new();
        if cleared > 0 {
            notes.push(format!("{cleared} cleared by the classifier"));
        }
        if !advisory.is_empty() {
            notes.push(crate::util::plural(advisory.len(), "advisory note"));
        }
        let note = if notes.is_empty() { String::new() } else { paint.dim(&format!(" ({})", notes.join(", "))) };
        lines.push(format!("{PREFIX} {}{note} {footer}", paint.green(&format!("{} passed", crate::util::plural(run.decisions.len(), "Reviewer")))));
        if advisory.is_empty() {
            return lines.join("\n");
        }
        for decision in advisory {
            finding(&mut lines, decision);
        }
        lines.push(String::new());
        let through = if check { "a commit of this change goes through" } else { "the commit went through" };
        lines.push(format!("Advisory only: {through}. Full reasoning: `reviewers run {}`.", run.id));
        return lines.join("\n");
    }
    lines.push(format!(
        "{PREFIX} {} {footer}",
        paint.red(&format!("blocked by {} of {}", blocked.len(), crate::util::plural(run.decisions.len(), "Reviewer")))
    ));
    for decision in blocked.iter().chain(&advisory) {
        finding(&mut lines, decision);
    }
    lines.push(String::new());
    if blocked_before {
        lines.push("The code and the Reviewer are unchanged since the last try, so it gave the same verdict without running again.".into());
    }
    let next = if check { "check again, or commit" } else { "commit again" };
    lines.push(format!("Fix the code above and {next}. Full reasoning: `reviewers run {}`.", run.id));
    lines.join("\n")
}
