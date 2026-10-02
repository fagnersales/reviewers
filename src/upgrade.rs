use crate::commands::Outcome;
use crate::store::Store;
use crate::{hooks, skill, ui};
use clap::Args;
use serde_json::Value;
use std::path::PathBuf;

#[derive(Args)]
pub struct UpgradeArgs {
    /// Only say whether a newer release exists.
    #[arg(long)]
    pub check: bool,
    /// Run by the new binary right after it's swapped in.
    #[arg(long, hide = true)]
    pub finish: bool,
}

/// `{ "version": "0.2.0", "notes": ["…"], "assets": { "<target>": { "url": "…", "sha256": "…" } } }`
const MANIFEST_URL: &str = "https://reviewers.sh/releases/latest.json";
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

fn fetch_manifest() -> Result<Value, String> {
    let mut response = ureq::get(&manifest_url()).call().map_err(|error| format!("cannot reach {}: {error}", manifest_url()))?;
    let text = response.body_mut().read_to_string().map_err(|error| error.to_string())?;
    serde_json::from_str(&text).map_err(|error| format!("the release manifest is malformed: {error}"))
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
        if ours && hooks::install(&root, false).is_ok() {
            refreshed += 1;
        }
    }
    println!("{} hooks refreshed in {}", ui::green("✓"), crate::util::plural(refreshed, "repo"));
    Ok(0)
}

pub fn run(args: UpgradeArgs) -> Outcome {
    if args.finish {
        return finish();
    }
    let current = env!("CARGO_PKG_VERSION");
    let manifest = fetch_manifest()?;
    let latest = manifest["version"].as_str().ok_or("the release manifest has no version")?;
    if !newer(latest, current) {
        println!("reviewers {current} is the latest.");
        return Ok(0);
    }
    if args.check {
        println!("reviewers {latest} is out (you have {current}). `reviewers upgrade` installs it.");
        return Ok(0);
    }
    let asset = &manifest["assets"][target()];
    let url = asset["url"].as_str().ok_or_else(|| format!("release {latest} has no build for {}", target()))?;
    let expected = asset["sha256"].as_str().ok_or("the release manifest has no checksum")?;
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
    if let Some(notes) = manifest["notes"].as_array() {
        for note in notes.iter().filter_map(Value::as_str) {
            println!("  · {note}");
        }
    }
    let status = std::process::Command::new(&exe).args(["upgrade", "--finish"]).status().map_err(|error| error.to_string())?;
    Ok(status.code().unwrap_or(0))
}

#[cfg(test)]
mod tests {
    #[test]
    fn compares_versions_by_number() {
        assert!(super::newer("0.10.0", "0.9.3"));
        assert!(super::newer("v1.0.0", "0.99.0"));
        assert!(!super::newer("0.1.0", "0.1.0"));
    }
}
