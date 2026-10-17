//! Private-workspace helpers: password hashing, system idle time, and keeping
//! the window out of screenshots / screen sharing while private tabs are shown.

use sha2::{Digest, Sha256};
use windows::Win32::{
    Foundation::HWND,
    System::SystemInformation::GetTickCount,
    UI::{
        Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO},
        WindowsAndMessaging::{SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE, WDA_NONE},
    },
};

/// Rounds of the older password format, still accepted (and upgraded on the next unlock).
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

/// Argon2id (the current standard for passwords: memory-hard, so guessing with graphics cards
/// is slow) with a random salt, as a PHC string. The password itself is never stored.
pub fn hash_password(password: &str) -> String {
    use argon2::password_hash::{PasswordHasher, SaltString};
    let salt = SaltString::generate(&mut rand_core::OsRng);
    argon2::Argon2::default().hash_password(password.as_bytes(), &salt).map(|h| h.to_string()).unwrap_or_default()
}

/// Check a password against what's stored (Argon2id, or the older salted SHA-256 format).
pub fn verify_password(stored: &str, password: &str) -> bool {
    if stored.starts_with("$argon2") {
        use argon2::password_hash::{PasswordHash, PasswordVerifier};
        return PasswordHash::new(stored).is_ok_and(|h| argon2::Argon2::default().verify_password(password.as_bytes(), &h).is_ok());
    }
    let Some((salt_hex, hash_hex)) = stored.split_once('$') else { return false };
    let salt: Vec<u8> = (0..salt_hex.len() / 2)
        .filter_map(|i| u8::from_str_radix(salt_hex.get(i * 2..i * 2 + 2)?, 16).ok())
        .collect();
    let got = hex(&stretch(&salt, password));
    // Constant-time comparison.
    got.len() == hash_hex.len() && got.bytes().zip(hash_hex.bytes()).fold(0u8, |a, (x, y)| a | (x ^ y)) == 0
}

/// A stored password in the older format, to be re-hashed with Argon2id after a good unlock.
pub fn needs_upgrade(stored: &str) -> bool {
    !stored.is_empty() && !stored.starts_with("$argon2")
}

/// Seconds since the last keyboard/mouse input anywhere on the system.
pub fn idle_secs() -> u64 {
    let mut li = LASTINPUTINFO { cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
    unsafe {
        if !GetLastInputInfo(&mut li).as_bool() {
            return 0;
        }
        (GetTickCount().wrapping_sub(li.dwTime) / 1000) as u64
    }
}

/// Hide the window's contents from screenshots, recordings and screen sharing.
pub fn exclude_from_capture(hwnd: isize, on: bool) {
    unsafe {
        let _ = SetWindowDisplayAffinity(HWND(hwnd as *mut _), if on { WDA_EXCLUDEFROMCAPTURE } else { WDA_NONE });
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

    #[test]
    fn argon2_now_and_the_older_format_still_unlocks() {
        let h = hash_password("correct horse");
        assert!(h.starts_with("$argon2id$"), "{h}");
        assert!(!needs_upgrade(&h));
        let salt = [7u8; 16];
        let old = format!("{}${}", hex(&salt), hex(&stretch(&salt, "pw1234")));
        assert!(verify_password(&old, "pw1234") && !verify_password(&old, "pw1235"));
        assert!(needs_upgrade(&old));
    }
}
