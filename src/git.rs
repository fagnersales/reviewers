use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub struct GitOutput {
    pub stdout: String,
    pub stderr: String,
    pub ok: bool,
}

pub fn run(cwd: &Path, args: &[&str], env: &[(&str, &str)]) -> GitOutput {
    let mut command = Command::new("git");
    command.args(args).current_dir(cwd).env("GIT_TERMINAL_PROMPT", "0");
    for (key, value) in env {
        command.env(key, value);
    }
    match command.output() {
        Ok(output) => GitOutput {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            ok: output.status.success(),
        },
        Err(error) => GitOutput { stdout: String::new(), stderr: error.to_string(), ok: false },
    }
}

pub fn ok(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let output = run(cwd, args, &[]);
    if output.ok {
        Ok(output.stdout)
    } else {
        Err(format!("git {} failed: {}", args.join(" "), output.stderr.trim()))
    }
}

pub fn root_of(start: &Path) -> Option<PathBuf> {
    let output = run(start, &["rev-parse", "--show-toplevel"], &[]);
    output.ok.then(|| PathBuf::from(output.stdout.trim()))
}

/// A linked worktree has its own top level but shares the main checkout's
/// `.git`; repos are registered by the main checkout, so hooks running in a
/// worktree find their repo through this.
pub fn main_checkout(root: &Path) -> PathBuf {
    let output = run(root, &["rev-parse", "--path-format=absolute", "--git-common-dir"], &[]);
    if !output.ok {
        return root.to_path_buf();
    }
    let common = PathBuf::from(output.stdout.trim());
    if common.file_name().is_some_and(|name| name == ".git") {
        common.parent().map(Path::to_path_buf).unwrap_or_else(|| root.to_path_buf())
    } else {
        root.to_path_buf()
    }
}

/// The repo's own `core.hooksPath` (husky, lefthook), if it sets one. A relative path is taken
/// from the top of the working tree, where git runs hooks.
pub fn local_hooks_path(root: &Path) -> Option<PathBuf> {
    let output = run(root, &["config", "--local", "--get", "core.hooksPath"], &[]);
    let value = output.stdout.trim();
    if !output.ok || value.is_empty() {
        return None;
    }
    let path = match value.strip_prefix("~/") {
        Some(rest) => crate::util::home().join(rest),
        None => PathBuf::from(value),
    };
    Some(if path.is_absolute() { path } else { root.join(path) })
}

/// Where this repo's own hooks live: its own `core.hooksPath` if it sets one, else the hooks
/// folder its worktrees share. A global `core.hooksPath` isn't followed: that's every repo's
/// layer, not this one's.
pub fn hooks_dir(root: &Path) -> Result<PathBuf, String> {
    if let Some(path) = local_hooks_path(root) {
        return Ok(path);
    }
    let common = ok(root, &["rev-parse", "--path-format=absolute", "--git-common-dir"])?;
    Ok(PathBuf::from(common.trim()).join("hooks"))
}

pub fn local_config(root: &Path, key: &str) -> Option<String> {
    let output = run(root, &["config", "--local", "--get", key], &[]);
    let value = output.stdout.trim().to_string();
    (output.ok && !value.is_empty()).then_some(value)
}

/// Sets a key in the repo's own config (shared by its worktrees), or unsets it with `None`.
pub fn set_local_config(root: &Path, key: &str, value: Option<&str>) -> Result<(), String> {
    match value {
        Some(value) => ok(root, &["config", "--local", key, value]).map(|_| ()),
        None => {
            let output = run(root, &["config", "--local", "--unset", key], &[]);
            if output.ok || output.stderr.trim().is_empty() { Ok(()) } else { Err(format!("git config --unset {key} failed: {}", output.stderr.trim())) }
        }
    }
}

pub fn global_config(key: &str) -> Option<String> {
    let output = run(&crate::util::home(), &["config", "--global", "--get", key], &[]);
    let value = output.stdout.trim().to_string();
    (output.ok && !value.is_empty()).then_some(value)
}

/// Sets a key in the global git config, or unsets it with `None`.
pub fn set_global_config(key: &str, value: Option<&str>) -> Result<(), String> {
    let home = crate::util::home();
    match value {
        Some(value) => ok(&home, &["config", "--global", key, value]).map(|_| ()),
        // Exit 5 means it wasn't set, which is the goal anyway.
        None => {
            let output = run(&home, &["config", "--global", "--unset", key], &[]);
            if output.ok || output.stderr.trim().is_empty() { Ok(()) } else { Err(format!("git config --global --unset {key} failed: {}", output.stderr.trim())) }
        }
    }
}

pub fn remote_url(root: &Path) -> Option<String> {
    let output = run(root, &["remote", "get-url", "origin"], &[]);
    output.ok.then(|| output.stdout.trim().to_string()).filter(|url| !url.is_empty())
}

pub fn staged_diff(root: &Path) -> Result<String, String> {
    ok(root, &["diff", "--cached", "--no-ext-diff"]).map(|diff| without_generated(root, &diff))
}

/// Collapsed the way `staged_diff` collapses it, so a commit's diff hashes like the review of it.
pub fn commit_diff(root: &Path, reference: &str) -> Result<String, String> {
    ok(root, &["show", "--format=", "--no-ext-diff", reference]).map(|diff| without_generated(root, &diff))
}

/// The diff with the files `.gitattributes` marks `linguist-generated` collapsed to a line each.
pub fn without_generated(root: &Path, diff: &str) -> String {
    let generated = generated_paths(root, &crate::diff::changed_paths(diff));
    if generated.is_empty() { diff.to_string() } else { crate::diff::collapse_generated(diff, &generated) }
}

/// Read from the index, which matches HEAD right after a commit, so the hook and post-commit agree.
/// Any failure reads as no generated files: the Reviewers then see everything.
fn generated_paths(root: &Path, paths: &[String]) -> Vec<String> {
    if paths.is_empty() {
        return Vec::new();
    }
    let child = Command::new("git")
        .args(["check-attr", "--cached", "-z", "--stdin", "linguist-generated"])
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = child else {
        return Vec::new();
    };
    let input: String = paths.iter().map(|path| format!("{path}\0")).collect();
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(input.as_bytes());
    }
    let Ok(output) = child.wait_with_output() else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let fields: Vec<&str> = text.split('\0').collect();
    fields.chunks_exact(3).filter(|entry| matches!(entry[2], "set" | "true")).map(|entry| entry[0].to_string()).collect()
}

pub struct CommitInfo {
    pub sha: String,
    pub subject: String,
    pub committed_at: String,
}

pub fn commit_info(root: &Path, reference: &str) -> Result<CommitInfo, String> {
    let text = ok(root, &["log", "-1", "--format=%H%n%cI%n%s", reference])?;
    let mut lines = text.trim_end().lines();
    let sha = lines.next().unwrap_or_default().to_string();
    let committed = lines.next().unwrap_or_default();
    let subject = lines.collect::<Vec<_>>().join("\n");
    if sha.is_empty() {
        return Err(format!("cannot read commit {reference}"));
    }
    let committed_at = crate::util::parse_iso(committed).map(crate::util::to_iso).unwrap_or_else(crate::util::now_iso);
    Ok(CommitInfo { sha, subject, committed_at })
}

/// The subject line the author typed, skipping Git's `#` comment lines.
pub fn attempted_message(message_file: &Path) -> Option<String> {
    let text = std::fs::read_to_string(message_file).ok()?;
    text.lines().find(|line| !line.trim().is_empty() && !line.starts_with('#')).map(|line| line.trim().to_string())
}

pub fn head_sha(root: &Path) -> Result<String, String> {
    ok(root, &["rev-parse", "HEAD"]).map(|sha| sha.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staged_diff_collapses_files_marked_generated() {
        let repo = std::env::temp_dir().join(crate::util::new_id("generated"));
        std::fs::create_dir_all(repo.join("convex/_generated")).unwrap();
        ok(&repo, &["init", "-q"]).unwrap();
        std::fs::write(repo.join(".gitattributes"), "convex/_generated/** linguist-generated\n").unwrap();
        std::fs::write(repo.join("convex/_generated/api.d.ts"), "export type Api = {};\n".repeat(500)).unwrap();
        std::fs::write(repo.join("users.ts"), "export const user = 1;\n").unwrap();
        ok(&repo, &["add", "."]).unwrap();
        let diff = staged_diff(&repo).unwrap();
        let _ = std::fs::remove_dir_all(&repo);
        assert!(diff.contains("Generated file (linguist-generated in .gitattributes): 500 lines added, 0 removed, not shown."), "{diff}");
        assert!(diff.contains("+export const user = 1;") && diff.contains("+convex/_generated/** linguist-generated"));
        assert!(!diff.contains("export type Api"));
    }
}
