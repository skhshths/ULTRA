//! Embeds assets/ultra.ico as the exe icon (resource group 1) without needing rc.exe:
//! the .ico is converted into a binary .res file that link.exe accepts directly.

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

fn main() {
    println!("cargo:rerun-if-changed=assets/ultra.ico");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let ico = fs::read("assets/ultra.ico").expect("assets/ultra.ico");
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
