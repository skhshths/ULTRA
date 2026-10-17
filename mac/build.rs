//! Stamps the build with the commit it comes from (see `stamp_commit`).

use std::env;

/// The commit this build comes from (CI sets GITHUB_SHA; local builds ask git), so ULTRA can
/// tell whether the latest release is a different build.
fn stamp_commit() {
    println!("cargo:rerun-if-env-changed=GITHUB_SHA");
    let sha = env::var("GITHUB_SHA").ok().filter(|s| !s.is_empty()).or_else(|| {
        let out = std::process::Command::new("git").args(["rev-parse", "HEAD"]).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    });
    println!("cargo:rustc-env=ULTRA_COMMIT={}", sha.unwrap_or_default());
    // The release this build becomes (CI names each release after a food, e.g. "ULTRA 1 · Apple").
    println!("cargo:rerun-if-env-changed=ULTRA_RELEASE");
    println!("cargo:rustc-env=ULTRA_RELEASE={}", env::var("ULTRA_RELEASE").unwrap_or_default());
    // Rebuild the stamp when the checkout moves to another commit.
    if let Ok(out) = std::process::Command::new("git").args(["rev-parse", "--git-dir"]).output() {
        let dir = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !dir.is_empty() {
            println!("cargo:rerun-if-changed={dir}/HEAD");
            println!("cargo:rerun-if-changed={dir}/logs/HEAD");
        }
    }
}

fn main() {
    stamp_commit();
}
