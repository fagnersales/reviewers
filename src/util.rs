use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::PathBuf;

pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Everything Reviewers keeps: the database, the installed binary, onboarding output.
pub fn data_dir() -> PathBuf {
    std::env::var_os("REVIEWERS_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".reviewers"))
}

/// `2026-10-02T12:12:13.605Z`, the format every stored timestamp uses, so they sort as text.
pub fn now_iso() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

pub fn to_iso(time: DateTime<Utc>) -> String {
    time.to_rfc3339_opts(SecondsFormat::Millis, true)
}

pub fn parse_iso(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text).ok().map(|time| time.with_timezone(&Utc))
}

pub fn new_id(prefix: &str) -> String {
    let mut bytes = [0u8; 10];
    getrandom::fill(&mut bytes).expect("the OS random source is available");
    format!("{prefix}_{}", hex(&bytes))
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn sha256_hex(text: &str) -> String {
    hex(&Sha256::digest(text.as_bytes()))
}

/// `No type casts!` → `no-type-casts`
pub fn slugify(name: &str) -> String {
    let mut slug = String::new();
    for character in name.to_lowercase().chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character);
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
    }
    slug.trim_end_matches('-').chars().take(60).collect()
}

pub fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let kept: String = text.chars().take(max).collect();
    format!("{kept}\n…[{} more characters]", text.chars().count() - max)
}

pub fn str_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

pub fn u64_field(value: &Value, key: &str) -> u64 {
    value.get(key).and_then(Value::as_u64).unwrap_or(0)
}

/// 950, 9.5k, 52k, 1.24M
pub fn compact(value: u64) -> String {
    if value >= 1_000_000 {
        format!("{:.2}M", value as f64 / 1_000_000.0)
    } else if value >= 10_000 {
        format!("{}k", (value as f64 / 1000.0).round() as u64)
    } else if value >= 1000 {
        format!("{:.1}k", value as f64 / 1000.0)
    } else {
        value.to_string()
    }
}

pub fn duration(ms: u64) -> String {
    let seconds = (ms as f64 / 1000.0).round() as u64;
    if seconds >= 60 {
        format!("{}m {:02}s", seconds / 60, seconds % 60)
    } else {
        format!("{seconds}s")
    }
}

pub fn plural(count: usize, word: &str) -> String {
    format!("{} {word}{}", thousands(count as u64), if count == 1 { "" } else { "s" })
}

pub fn thousands(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::new();
    for (index, character) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            out.push(',');
        }
        out.push(character);
    }
    out
}

pub fn home_path(path: &std::path::Path) -> String {
    let home = home();
    match path.strip_prefix(&home) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_are_readable_handles() {
        assert_eq!(slugify("No type casts"), "no-type-casts");
        assert_eq!(slugify("  Errors reach users — via the map!"), "errors-reach-users-via-the-map");
        assert_eq!(slugify("Prices are never in $"), "prices-are-never-in");
    }

    #[test]
    fn counts_read_at_a_glance() {
        assert_eq!(compact(950), "950");
        assert_eq!(compact(9_500), "9.5k");
        assert_eq!(compact(52_300), "52k");
        assert_eq!(compact(1_240_000), "1.24M");
        assert_eq!(thousands(2_319), "2,319");
        assert_eq!(duration(128_000), "2m 08s");
    }
}
