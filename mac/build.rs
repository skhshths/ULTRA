//! Stamps the build with the commit it comes from (see `stamp_commit`), and converts the
//! ad-block filter lists (shared with Windows) into WebKit content rules (see build_rules.rs).

use std::{env, fs, path::PathBuf};

mod build_rules;

/// The lists as WebKit rule lists, deflated into OUT_DIR/rules.bin: each list as its length on a
/// line, then its JSON.
fn pack_rules() {
    let dir = PathBuf::from("../windows/assets/filters");
    println!("cargo:rerun-if-changed=../windows/assets/filters");
    println!("cargo:rerun-if-changed=build_rules.rs");
    let lists: Vec<String> = build_rules::read_lists(&dir).into_iter().map(|(_, t)| t).collect();
    let (chunks, count, _) = build_rules::convert(&lists);
    let mut raw = Vec::new();
    for c in &chunks {
        raw.extend(format!("{}\n", c.len()).into_bytes());
        raw.extend(c.as_bytes());
    }
    let hash = raw.iter().fold(0xcbf29ce484222325u64, |h, &b| (h ^ b as u64).wrapping_mul(0x100000001b3));
    println!("cargo:rustc-env=ULTRA_RULES_HASH={hash:016x}");
    println!("cargo:rustc-env=ULTRA_RULES_COUNT={count}");
    fs::write(PathBuf::from(env::var("OUT_DIR").unwrap()).join("rules.bin"), miniz_oxide::deflate::compress_to_vec(&raw, 9)).unwrap();
}

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
    pack_rules();
}
