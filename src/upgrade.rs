use crate::commands::Outcome;
use crate::store::Store;
use crate::{hooks, skill, ui};
use clap::Args;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Args)]
pub struct UpgradeArgs {
    /// Only say whether a newer release exists, and its notes.
    #[arg(long)]
    pub check: bool,
    /// With --check: save the answer without printing it (the background check).
    #[arg(long, hide = true, requires = "check")]
    pub quiet: bool,
    /// Run by the new binary right after it's swapped in.
    #[arg(long, hide = true)]
    pub finish: bool,
}

/// Plain text, so `install.sh` can read it with nothing but POSIX shell:
/// `version 0.2.0`, any number of `note …` lines, and one `<target> <url> <sha256>` line per build.
const MANIFEST_URL: &str = "https://reviewers.sh/releases/latest.txt";
const MAX_BINARY_BYTES: u64 = 200 * 1024 * 1024;

fn manifest_url() -> String {
    std::env::var("REVIEWERS_RELEASES_URL").unwrap_or_else(|_| MANIFEST_URL.to_string())
}

pub fn target() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
        ("windows", "x86_64") => "x86_64-pc-windows-msvc",
        _ => "unsupported",
    }
}

fn version_parts(version: &str) -> Vec<u64> {
    version.trim_start_matches('v').split(['.', '-']).take(3).map(|part| part.parse().unwrap_or(0)).collect()
}

fn newer(candidate: &str, current: &str) -> bool {
    version_parts(candidate) > version_parts(current)
}

struct Manifest {
    version: String,
    notes: Vec<String>,
    /// `(target, url, sha256)`
    builds: Vec<(String, String, String)>,
}

fn parse_manifest(text: &str) -> Result<Manifest, String> {
    let mut manifest = Manifest { version: String::new(), notes: Vec::new(), builds: Vec::new() };
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty() && !line.starts_with('#')) {
        let mut words = line.split_whitespace();
        match (words.next(), words.next(), words.next()) {
            (Some("version"), Some(version), None) => manifest.version = version.to_string(),
            (Some("note"), _, _) => manifest.notes.push(line["note".len()..].trim().to_string()),
            (Some(target), Some(url), Some(sha256)) => manifest.builds.push((target.to_string(), url.to_string(), sha256.to_string())),
            _ => return Err(format!("the release manifest has a line it can't read: {line}")),
        }
    }
    if manifest.version.is_empty() {
        return Err("the release manifest has no version".into());
    }
    Ok(manifest)
}

/// How old the last look at the release list may be before another one starts in the background.
const CHECK_EVERY: Duration = Duration::from_secs(24 * 60 * 60);

pub struct Update {
    pub latest: String,
    pub notes: Vec<String>,
}

impl Update {
    /// One line for the hook's output and the guide.
    pub fn line(&self) -> String {
        format!("reviewers {} is out (this is {}): run `reviewers upgrade` between tasks.", self.latest, env!("CARGO_PKG_VERSION"))
    }
}

/// The newer release the last check found. Never waits on the network: when the last look is
/// more than a day old, a detached `reviewers upgrade --check --quiet` takes another, and a later
/// command sees what it found. `REVIEWERS_NO_UPDATE_CHECK=1` turns the checks off.
pub fn available(store: &Store) -> Option<Update> {
    if std::env::var_os("REVIEWERS_NO_UPDATE_CHECK").is_some() {
        return None;
    }
    let last = store.setting("update_checked_at").ok().flatten().and_then(|at| chrono::DateTime::parse_from_rfc3339(&at).ok());
    let stale = last.is_none_or(|at| chrono::Utc::now().signed_duration_since(at).to_std().unwrap_or_default() > CHECK_EVERY);
    if stale && store.set_setting("update_checked_at", Some(&crate::util::now_iso())).is_ok() {
        check_in_background();
    }
    let latest = store.setting("update_latest").ok().flatten()?;
    let notes = store.setting("update_notes").ok().flatten().map(|notes| notes.lines().map(str::to_string).collect()).unwrap_or_default();
    newer(&latest, env!("CARGO_PKG_VERSION")).then_some(Update { latest, notes })
}

fn check_in_background() {
    let Ok(exe) = std::env::current_exe() else { return };
    let _ = std::process::Command::new(exe)
        .args(["upgrade", "--check", "--quiet"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

/// Keeps what the release list says, for `available` to read without the network.
fn remember(manifest: &Manifest) {
    if let Ok(store) = Store::open_default() {
        let _ = store.set_setting("update_latest", Some(&manifest.version));
        let _ = store.set_setting("update_notes", Some(&manifest.notes.join("\n")));
        let _ = store.set_setting("update_checked_at", Some(&crate::util::now_iso()));
    }
}

fn fetch_manifest() -> Result<Manifest, String> {
    let mut response = ureq::get(&manifest_url()).call().map_err(|error| format!("cannot reach {}: {error}", manifest_url()))?;
    parse_manifest(&response.body_mut().read_to_string().map_err(|error| error.to_string())?)
}

/// The new binary's job: everything that depends on its version. The database migrates itself on open.
fn finish() -> Outcome {
    let placements = skill::install()?;
    let linked = placements.iter().filter(|placement| placement.state == "linked").count();
    println!("{} skill refreshed for {}", ui::green("✓"), crate::util::plural(linked, "agent"));
    let store = Store::open_default()?;
    let mut refreshed = 0;
    for project in store.projects()? {
        let root = PathBuf::from(&project.root);
        let ours = hooks::state_of(&root).is_ok_and(|states| states.iter().any(|(_, state)| *state == Some(hooks::HookState::Installed)));
        if ours && hooks::install(&root).is_ok() {
            refreshed += 1;
        }
    }
    println!("{} hooks refreshed in {}", ui::green("✓"), crate::util::plural(refreshed, "repo"));
    if hooks::global_installed() {
        hooks::install_global(&store)?;
        println!("{} global hooks refreshed", ui::green("✓"));
    }
    Ok(0)
}

pub fn run(args: UpgradeArgs) -> Outcome {
    if args.finish {
        return finish();
    }
    let current = env!("CARGO_PKG_VERSION");
    let manifest = fetch_manifest()?;
    remember(&manifest);
    let latest = manifest.version.as_str();
    if args.quiet {
        return Ok(0);
    }
    if !newer(latest, current) {
        println!("reviewers {current} is the latest.");
        return Ok(0);
    }
    if args.check {
        println!("reviewers {latest} is out (you have {current}). `reviewers upgrade` installs it.");
        for note in &manifest.notes {
            println!("  · {note}");
        }
        return Ok(0);
    }
    let (_, url, expected) = manifest.builds.iter().find(|(build, _, _)| build == target()).ok_or_else(|| format!("release {latest} has no build for {}", target()))?;
    println!("Downloading reviewers {latest}…");
    let mut response = ureq::get(url).call().map_err(|error| format!("download failed: {error}"))?;
    let bytes = response.body_mut().with_config().limit(MAX_BINARY_BYTES).read_to_vec().map_err(|error| format!("download failed: {error}"))?;
    let actual = crate::util::hex(&<sha2::Sha256 as sha2::Digest>::digest(&bytes));
    if !actual.eq_ignore_ascii_case(expected) {
        return Err(format!("the download doesn't match its checksum (expected {expected}, got {actual}); nothing was changed"));
    }
    let exe = std::env::current_exe().and_then(|path| path.canonicalize()).map_err(|error| format!("cannot find this binary: {error}"))?;
    let staged = exe.with_extension("new");
    std::fs::write(&staged, &bytes).map_err(|error| format!("cannot write {}: {error}", staged.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755)).map_err(|error| error.to_string())?;
    }
    // A rename over a running binary is safe on Unix: this process keeps the old file open.
    std::fs::rename(&staged, &exe).map_err(|error| format!("cannot replace {}: {error}", exe.display()))?;
    println!("{} reviewers {current} → {latest}", ui::green("✓"));
    for note in &manifest.notes {
        println!("  · {note}");
    }
    let status = std::process::Command::new(&exe).args(["upgrade", "--finish"]).status().map_err(|error| error.to_string())?;
    Ok(status.code().unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_newer_release_found_earlier_is_announced_without_the_network() {
        let directory = std::env::temp_dir().join(crate::util::new_id("reviewers-test"));
        let store = Store::open(&directory.join("reviewers.sqlite")).unwrap();
        // A check from just now: nothing starts in the background.
        store.set_setting("update_checked_at", Some(&crate::util::now_iso())).unwrap();
        assert!(available(&store).is_none(), "nothing found yet");
        store.set_setting("update_latest", Some("99.0.0")).unwrap();
        store.set_setting("update_notes", Some("Faster reviews\nA new command")).unwrap();
        let update = available(&store).expect("a newer release");
        assert_eq!((update.latest.as_str(), update.notes.len()), ("99.0.0", 2));
        assert!(update.line().contains("`reviewers upgrade`"));
        // The release this build already is, or an older one, isn't news.
        store.set_setting("update_latest", Some(env!("CARGO_PKG_VERSION"))).unwrap();
        assert!(available(&store).is_none());
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn reads_the_plain_manifest() {
        let manifest = super::parse_manifest("version 0.2.0\nnote Faster\naarch64-apple-darwin https://x/r abc123\n").expect("parses");
        assert_eq!(manifest.version, "0.2.0");
        assert_eq!(manifest.notes, vec!["Faster"]);
        assert_eq!(manifest.builds[0].2, "abc123");
    }

    #[test]
    fn compares_versions_by_number() {
        assert!(super::newer("0.10.0", "0.9.3"));
        assert!(super::newer("v1.0.0", "0.99.0"));
        assert!(!super::newer("0.1.0", "0.1.0"));
    }
}
