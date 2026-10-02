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
}
