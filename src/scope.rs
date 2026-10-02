use regex::Regex;

/// `**` any depth, `*` within one segment, `?` one character, `{a,b}` either.
pub fn glob_to_regex(pattern: &str) -> Regex {
    let characters: Vec<char> = pattern.chars().collect();
    let mut source = String::from("^");
    let mut index = 0;
    while index < characters.len() {
        let character = characters[index];
        match character {
            '*' if characters.get(index + 1) == Some(&'*') => {
                if characters.get(index + 2) == Some(&'/') {
                    source.push_str("(?:.*/)?");
                    index += 2;
                } else {
                    source.push_str(".*");
                    index += 1;
                }
            }
            '*' => source.push_str("[^/]*"),
            '?' => source.push_str("[^/]"),
            '{' => match characters[index..].iter().position(|&c| c == '}') {
                Some(offset) => {
                    let inner: String = characters[index + 1..index + offset].iter().collect();
                    let options: Vec<String> = inner.split(',').map(regex::escape).collect();
                    source.push_str(&format!("(?:{})", options.join("|")));
                    index += offset;
                }
                None => source.push_str("\\{"),
            },
            other => source.push_str(&regex::escape(&other.to_string())),
        }
        index += 1;
    }
    source.push('$');
    Regex::new(&source).unwrap_or_else(|_| Regex::new("^$").expect("an empty pattern compiles"))
}

/// A pattern without glob characters is a directory prefix (`convex/`) or an exact file.
fn matches_pattern(pattern: &str, file: &str) -> bool {
    if pattern.contains(['*', '?', '{', '[']) {
        return glob_to_regex(pattern).is_match(file);
    }
    let prefix = pattern.strip_suffix("/**").unwrap_or(pattern);
    file == pattern || file.starts_with(prefix)
}

/// No patterns means every diff; otherwise any changed file matching any pattern.
pub fn matches(patterns: &[String], files: &[String]) -> bool {
    patterns.is_empty() || files.iter().any(|file| patterns.iter().any(|pattern| matches_pattern(pattern, file)))
}

/// `convex/**, shared/**` or `none`, as typed on the command line. Commas inside `{a,b}` belong to the glob.
pub fn parse_list(text: &str) -> Vec<String> {
    if text.trim() == "none" {
        return Vec::new();
    }
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    for character in text.chars() {
        match character {
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                parts.push(std::mem::take(&mut current));
                continue;
            }
            _ => {}
        }
        current.push(character);
    }
    parts.push(current);
    parts.into_iter().map(|part| part.trim().to_string()).filter(|part| !part.is_empty()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(list: &[&str]) -> Vec<String> {
        list.iter().map(|file| file.to_string()).collect()
    }

    #[test]
    fn globs_and_prefixes() {
        let scope = parse_list("convex/**/core.ts, app/**/*.tsx, shared/");
        assert!(matches(&scope, &files(&["convex/coupons/core.ts"])));
        assert!(matches(&scope, &files(&["convex/core.ts"])));
        assert!(matches(&scope, &files(&["app/a/b/page.tsx"])));
        assert!(matches(&scope, &files(&["shared/errors.ts"])));
        assert!(!matches(&scope, &files(&["lib/util.ts", "convex/coupons/rules.ts"])));
        assert!(matches(&[], &files(&["anything"])));
        assert!(matches(&parse_list("{app,components}/**"), &files(&["components/x.tsx"])));
    }
}
