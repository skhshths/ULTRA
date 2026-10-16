//! Private-workspace helpers: password hashing, system idle time, and keeping
//! the window out of screenshots / screen sharing while private tabs are shown.

use objc2_app_kit::{NSWindow, NSWindowSharingType};
use sha2::{Digest, Sha256};

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventSourceSecondsSinceLastEventType(state: i32, event_type: u32) -> f64;
}

/// kCGEventSourceStateHIDSystemState: input from the hardware, whichever app it went to.
const HID_SYSTEM_STATE: i32 = 1;
/// kCGAnyInputEventType.
const ANY_INPUT: u32 = !0;

/// Key-stretching rounds; makes each guess against a stolen settings file expensive.
const ROUNDS: u32 = 200_000;

fn stretch(salt: &[u8], password: &str) -> [u8; 32] {
    let mut h: [u8; 32] = Sha256::new().chain_update(salt).chain_update(password.as_bytes()).finalize().into();
    for _ in 0..ROUNDS {
        h = Sha256::new().chain_update(h).chain_update(salt).finalize().into();
    }
    h
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// `salt$hash`, both hex. The password itself is never stored.
pub fn hash_password(password: &str) -> String {
    // Salt only needs to be unique: mix time, process id and an address.
    let seed = format!(
        "{:?}{}{:p}",
        std::time::SystemTime::now(),
        std::process::id(),
        &password as *const _
    );
    let salt: [u8; 16] = Sha256::digest(seed.as_bytes())[..16].try_into().unwrap_or([0; 16]);
    format!("{}${}", hex(&salt), hex(&stretch(&salt, password)))
}

pub fn verify_password(stored: &str, password: &str) -> bool {
    let Some((salt_hex, hash_hex)) = stored.split_once('$') else { return false };
    let salt: Vec<u8> = (0..salt_hex.len() / 2)
        .filter_map(|i| u8::from_str_radix(salt_hex.get(i * 2..i * 2 + 2)?, 16).ok())
        .collect();
    let got = hex(&stretch(&salt, password));
    // Constant-time comparison.
    got.len() == hash_hex.len() && got.bytes().zip(hash_hex.bytes()).fold(0u8, |a, (x, y)| a | (x ^ y)) == 0
}

/// Seconds since the last keyboard/mouse input anywhere on the system.
pub fn idle_secs() -> u64 {
    // SAFETY: a pure query; needs no permission (it reports time, not the events themselves).
    let secs = unsafe { CGEventSourceSecondsSinceLastEventType(HID_SYSTEM_STATE, ANY_INPUT) };
    if secs.is_finite() && secs > 0.0 {
        secs as u64
    } else {
        0
    }
}

/// Hide the window's contents from screenshots, recordings and screen sharing.
/// (Every pane is a subview of this one window, so this covers the pages too.)
pub fn exclude_from_capture(window: &NSWindow, on: bool) {
    let want = if on { NSWindowSharingType::None } else { NSWindowSharingType::ReadOnly };
    if window.sharingType() != want {
        window.setSharingType(want);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_roundtrip() {
        let h = hash_password("hunter22");
        assert!(verify_password(&h, "hunter22"));
        assert!(!verify_password(&h, "hunter23"));
        assert!(!verify_password("garbage", "hunter22"));
        assert_ne!(hash_password("a"), hash_password("a"));
    }
}
