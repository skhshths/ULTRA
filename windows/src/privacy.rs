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
}
