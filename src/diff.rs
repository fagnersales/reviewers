/// Files a unified diff touches; a deleted file is named by its `---` side.
pub fn changed_paths(diff: &str) -> Vec<String> {
    let mut paths = Vec::new();
    let mut removed: Option<String> = None;
    for line in diff.lines() {
        if line.starts_with("diff --git ") {
            removed = None;
        } else if let Some(path) = line.strip_prefix("--- ") {
            let path = path.strip_prefix("a/").unwrap_or(path);
            removed = (path != "/dev/null").then(|| path.to_string());
        } else if let Some(path) = line.strip_prefix("+++ ") {
            let path = path.strip_prefix("b/").unwrap_or(path);
            let resolved = if path == "/dev/null" { removed.clone() } else { Some(path.to_string()) };
            if let Some(resolved) = resolved {
                if !paths.contains(&resolved) {
                    paths.push(resolved);
                }
            }
        }
    }
    paths
}

/// Mode changes, renames and binary blobs give a Reviewer nothing to read.
pub fn has_reviewable_content(diff: &str) -> bool {
    diff.lines()
        .any(|line| (line.starts_with('+') && !line.starts_with("+++")) || (line.starts_with('-') && !line.starts_with("---")))
}

pub fn hash(diff: &str) -> String {
    crate::util::sha256_hex(diff)
}

/// A file's hunks, each as its lines with the `+`, `-` or ` ` marker still on.
fn hunks<'a>(diff: &'a str, file: &str) -> Vec<Vec<&'a str>> {
    let mut hunks: Vec<Vec<&str>> = Vec::new();
    let (mut removed, mut current, mut in_hunk) = (None, None, false);
    for line in diff.lines() {
        if line.starts_with("diff --git ") {
            (removed, current, in_hunk) = (None, None, false);
        } else if line.starts_with("@@") {
            in_hunk = current.as_deref() == Some(file);
            if in_hunk {
                hunks.push(Vec::new());
            }
        } else if in_hunk {
            if let Some(hunk) = hunks.last_mut().filter(|_| line.starts_with([' ', '+', '-'])) {
                hunk.push(line);
            }
        } else if let Some(path) = line.strip_prefix("--- ") {
            removed = Some(path.strip_prefix("a/").unwrap_or(path).to_string());
        } else if let Some(path) = line.strip_prefix("+++ ") {
            let path = path.strip_prefix("b/").unwrap_or(path);
            current = if path == "/dev/null" { removed.clone() } else { Some(path.to_string()) };
        }
    }
    hunks
}

/// The quoted lines as the diff has them. Models quoting code often trim the start of the quote,
/// which shifts its first line left of the rest; matching on trimmed text finds the real lines,
/// added or removed. None when the quote isn't in the diff.
pub fn find_quote(diff: &str, file: &str, quote: &str) -> Option<Vec<String>> {
    let wanted: Vec<&str> = quote.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
    if wanted.is_empty() {
        return None;
    }
    let hunks = hunks(diff, file);
    for side in ['+', '-'] {
        for hunk in &hunks {
            let lines: Vec<&str> = hunk
                .iter()
                .filter(|line| line.starts_with([' ', side]))
                .map(|line| &line[1..])
                .filter(|line| !line.trim().is_empty())
                .collect();
            let found = lines.windows(wanted.len()).find(|window| window.iter().map(|line| line.trim()).eq(wanted.iter().copied()));
            if let Some(window) = found {
                return Some(window.iter().map(|line| line.to_string()).collect());
            }
        }
    }
    None
}

/// Code lines shifted left by the indentation they all share.
pub fn dedent(lines: &[String]) -> Vec<String> {
    let indent = lines.iter().filter(|line| !line.trim().is_empty()).map(|line| line.len() - line.trim_start().len()).min().unwrap_or(0);
    lines.iter().map(|line| line.get(indent..).unwrap_or_else(|| line.trim_start()).trim_end().to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIFF: &str = "diff --git a/src/a.ts b/src/a.ts\nindex 1..2 100644\n--- a/src/a.ts\n+++ b/src/a.ts\n@@ -1 +1 @@\n-old\n+new\ndiff --git a/gone.ts b/gone.ts\ndeleted file mode 100644\n--- a/gone.ts\n+++ /dev/null\n@@ -1 +0,0 @@\n-bye\n";

    #[test]
    fn names_changed_and_deleted_files() {
        assert_eq!(changed_paths(DIFF), vec!["src/a.ts", "gone.ts"]);
        assert!(has_reviewable_content(DIFF));
        assert!(!has_reviewable_content("diff --git a/x b/x\nold mode 100644\nnew mode 100755\n"));
    }

    #[test]
    fn quotes_keep_the_indentation_the_code_has() {
        let diff = "diff --git a/src/api/users.ts b/src/api/users.ts\nnew file mode 100644\n--- /dev/null\n+++ b/src/api/users.ts\n@@ -0,0 +1,4 @@\n+export async function fetchUser(id: string) {\n+  const body = await getJson(`/users/${id}`);\n+  return body as User;\n+}\n";
        // The model trimmed the start of its quote, so its first line lost the indentation.
        let quote = "const body = await getJson(`/users/${id}`);\n  return body as User;";
        let found = find_quote(diff, "src/api/users.ts", quote).unwrap();
        assert_eq!(dedent(&found), vec!["const body = await getJson(`/users/${id}`);", "return body as User;"]);

        let nested = find_quote(diff, "src/api/users.ts", "export async function fetchUser(id: string) {\nconst body = await getJson(`/users/${id}`);").unwrap();
        assert_eq!(dedent(&nested), vec!["export async function fetchUser(id: string) {", "  const body = await getJson(`/users/${id}`);"]);

        assert_eq!(find_quote(DIFF, "src/a.ts", "old").unwrap(), vec!["old"]);
        assert_eq!(find_quote(DIFF, "gone.ts", "bye").unwrap(), vec!["bye"]);
        assert!(find_quote(diff, "src/api/users.ts", "return body;").is_none());
        assert!(find_quote(diff, "src/other.ts", "return body as User;").is_none());
    }
}
