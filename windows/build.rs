//! Embeds the exe icon (resource group 1) without needing rc.exe: the .ico is converted into
//! a binary .res file that link.exe accepts directly. The inverted-colors edition (experimental,
//! see src/edition.rs) gets assets/ultra-inv.ico, everything else assets/ultra.ico.

use std::{env, fs, path::PathBuf};

const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;
const LANG_EN_US: u16 = 0x0409;

fn push_resource(out: &mut Vec<u8>, kind: u16, id: u16, flags: u16, data: &[u8]) {
    out.extend((data.len() as u32).to_le_bytes());
    out.extend(32u32.to_le_bytes()); // header size
    out.extend([0xFFFF, kind, 0xFFFF, id].iter().flat_map(|w: &u16| w.to_le_bytes()));
    out.extend(0u32.to_le_bytes()); // data version
    out.extend(flags.to_le_bytes());
    out.extend(LANG_EN_US.to_le_bytes());
    out.extend(0u32.to_le_bytes()); // version
    out.extend(0u32.to_le_bytes()); // characteristics
    out.extend(data);
    while out.len() % 4 != 0 {
        out.push(0);
    }
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
    for f in ["assets/ultra.ico", "assets/ultra-inv.ico", "src/edition.rs"] {
        println!("cargo:rerun-if-changed={f}");
    }
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let inverted = fs::read_to_string("src/edition.rs").is_ok_and(|s| s.contains("pub const INVERT: bool = true;"));
    let file = if inverted { "assets/ultra-inv.ico" } else { "assets/ultra.ico" };
    let ico = fs::read(file).expect(file);
    let u16_at = |o: usize| u16::from_le_bytes([ico[o], ico[o + 1]]);
    let u32_at = |o: usize| u32::from_le_bytes([ico[o], ico[o + 1], ico[o + 2], ico[o + 3]]);
    let count = u16_at(4) as usize;

    // Empty leading resource marks a 32-bit .res file.
    let mut res = Vec::new();
    push_resource(&mut res, 0, 0, 0, &[]);

    let mut group = Vec::new();
    group.extend([0u16, 1, count as u16].iter().flat_map(|w| w.to_le_bytes()));
    for i in 0..count {
        let e = 6 + i * 16;
        let (size, offset) = (u32_at(e + 8) as usize, u32_at(e + 12) as usize);
        push_resource(&mut res, RT_ICON, i as u16 + 1, 0x1010, &ico[offset..offset + size]);
        group.extend(&ico[e..e + 12]); // width, height, colors, reserved, planes, bit count, bytes
        group.extend((i as u16 + 1).to_le_bytes());
    }
    push_resource(&mut res, RT_GROUP_ICON, 1, 0x1030, &group);

    let path = PathBuf::from(env::var("OUT_DIR").unwrap()).join("ultra.res");
    fs::write(&path, res).unwrap();
    println!("cargo:rustc-link-arg-bins={}", path.display());
}
