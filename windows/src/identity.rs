//! ULTRA ID: who you are to other ULTRA users (friends, chat and shared workspaces later).
//! It is made on this computer: a display name, a @handle, and an Ed25519 key pair. The private
//! key will prove it's you to ULTRA's servers and friends without a password; your shareable ID
//! code is derived from the public key. On Windows the private key is encrypted by Windows
//! (DPAPI, tied to your Windows login); on a Mac the file is readable by your user only.
//! Nothing is sent anywhere yet.

use std::path::Path;

use ed25519_dalek::{Signer, SigningKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const FILE: &str = "identity.json";

/// What's kept in identity.json.
#[derive(Serialize, Deserialize)]
struct Stored {
    name: String,
    handle: String,
    /// Public key, hex.
    public: String,
    /// Private key, hex: encrypted by Windows when `protected`.
    secret: String,
    protected: bool,
    created: u64,
}

/// Your ULTRA ID as the rest of ULTRA sees it (never the private key).
#[derive(Clone, Serialize)]
pub struct Identity {
    pub name: String,
    pub handle: String,
    /// The code friends will add you by, e.g. "ULTRA-7K2M-Q9XD-3FHP-W8RA".
    pub id: String,
    pub public: String,
    pub created: u64,
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    (s.len() % 2 == 0).then(|| (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect())?
}

/// The shareable code for a public key: 80 bits of its SHA-256 in an unambiguous base 32
/// (no I, L, O or U), in groups of four.
pub fn id_code(public: &[u8]) -> String {
    const ALPHA: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let h = Sha256::digest(public);
    let mut bits: u128 = 0;
    for b in &h[..10] {
        bits = (bits << 8) | *b as u128;
    }
    let chars: Vec<char> = (0..16).rev().map(|i| ALPHA[((bits >> (i * 5)) & 31) as usize] as char).collect();
    let groups: Vec<String> = chars.chunks(4).map(|c| c.iter().collect()).collect();
    format!("ULTRA-{}", groups.join("-"))
}

/// Handles: 3–20 characters, lowercase letters, digits and underscores, starting with a letter.
pub fn valid_handle(h: &str) -> bool {
    (3..=20).contains(&h.len())
        && h.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && h.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

pub fn load(dir: &Path) -> Option<Identity> {
    let s: Stored = serde_json::from_str(&std::fs::read_to_string(dir.join(FILE)).ok()?).ok()?;
    let public = unhex(&s.public)?;
    Some(Identity { id: id_code(&public), name: s.name, handle: s.handle, public: s.public, created: s.created })
}

/// Make a new ULTRA ID on this computer (refuses if one exists).
pub fn create(dir: &Path, name: &str, handle: &str) -> Result<Identity, String> {
    if load(dir).is_some() {
        return Err("this computer already has an ULTRA ID".into());
    }
    let name: String = name.trim().chars().filter(|c| !c.is_control()).take(40).collect();
    let handle = handle.trim().trim_start_matches('@').to_lowercase();
    if name.is_empty() {
        return Err("give your ULTRA ID a display name".into());
    }
    if !valid_handle(&handle) {
        return Err("handles are 3–20 characters: letters, numbers and _, starting with a letter".into());
    }
    let key = SigningKey::generate(&mut rand_core::OsRng);
    let public = key.verifying_key().to_bytes();
    let (secret, protected) = protect(&key.to_bytes());
    let created = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let stored = Stored { name: name.clone(), handle: handle.clone(), public: hex(&public), secret: hex(&secret), protected, created };
    let json = serde_json::to_string_pretty(&stored).map_err(|e| e.to_string())?;
    crate::library::write_atomic(&dir.join(FILE), json).map_err(|e| format!("couldn't save your ULTRA ID ({e})"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir.join(FILE), std::fs::Permissions::from_mode(0o600));
    }
    Ok(Identity { id: id_code(&public), name, handle, public: hex(&public), created })
}

/// Remove the ULTRA ID from this computer. (Once there are servers, a removed ID can't be
/// recovered: the private key exists nowhere else.)
pub fn remove(dir: &Path) {
    let _ = std::fs::remove_file(dir.join(FILE));
}

/// Sign a message with this computer's ULTRA ID key (for the servers to come).
#[allow(dead_code)]
pub fn sign(dir: &Path, msg: &[u8]) -> Option<Vec<u8>> {
    let s: Stored = serde_json::from_str(&std::fs::read_to_string(dir.join(FILE)).ok()?).ok()?;
    let raw = unhex(&s.secret)?;
    let bytes = if s.protected { unprotect(&raw)? } else { raw };
    let key = SigningKey::from_bytes(&bytes.try_into().ok()?);
    Some(key.sign(msg).to_bytes().to_vec())
}

/// Encrypt the private key for this Windows user (DPAPI). Elsewhere it's stored as is, in a
/// file only this user can read.
#[cfg(windows)]
pub(crate) fn protect(data: &[u8]) -> (Vec<u8>, bool) {
    use windows::Win32::{
        Foundation::{LocalFree, HLOCAL},
        Security::Cryptography::{CryptProtectData, CRYPT_INTEGER_BLOB},
    };
    let input = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 };
    let mut out = CRYPT_INTEGER_BLOB::default();
    unsafe {
        if CryptProtectData(&input, windows_core::w!("ULTRA ID"), None, None, None, 0, &mut out).is_err() {
            return (data.to_vec(), false);
        }
        let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        let _ = LocalFree(Some(HLOCAL(out.pbData as *mut _)));
        (v, true)
    }
}

#[cfg(windows)]
pub(crate) fn unprotect(data: &[u8]) -> Option<Vec<u8>> {
    use windows::Win32::{
        Foundation::{LocalFree, HLOCAL},
        Security::Cryptography::{CryptUnprotectData, CRYPT_INTEGER_BLOB},
    };
    let input = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 };
    let mut out = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptUnprotectData(&input, None, None, None, None, 0, &mut out).ok()?;
        let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        let _ = LocalFree(Some(HLOCAL(out.pbData as *mut _)));
        Some(v)
    }
}

#[cfg(not(windows))]
fn protect(data: &[u8]) -> (Vec<u8>, bool) {
    (data.to_vec(), false)
}

#[cfg(not(windows))]
fn unprotect(data: &[u8]) -> Option<Vec<u8>> {
    Some(data.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_load_sign_remove() {
        let dir = std::env::temp_dir().join(format!("ultra-id-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        assert!(create(&dir, "Olive", "no").is_err(), "handle too short");
        assert!(create(&dir, "Olive", "9lives").is_err(), "handle starts with a letter");
        let id = create(&dir, "  Olive ", "@Olive_B").unwrap();
        assert_eq!((id.name.as_str(), id.handle.as_str()), ("Olive", "olive_b"));
        assert!(id.id.starts_with("ULTRA-") && id.id.len() == 25, "{}", id.id);
        assert!(create(&dir, "Again", "again").is_err(), "one ID per computer");
        let back = load(&dir).unwrap();
        assert_eq!(back.id, id.id);
        // The stored secret is encrypted on Windows, and still signs correctly.
        let sig = sign(&dir, b"hello").unwrap();
        let vk = ed25519_dalek::VerifyingKey::from_bytes(&unhex(&back.public).unwrap().try_into().unwrap()).unwrap();
        assert!(vk.verify_strict(b"hello", &ed25519_dalek::Signature::from_slice(&sig).unwrap()).is_ok());
        remove(&dir);
        assert!(load(&dir).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn id_codes_are_stable_and_readable() {
        let a = id_code(&[1u8; 32]);
        assert_eq!(a, id_code(&[1u8; 32]));
        assert_ne!(a, id_code(&[2u8; 32]));
        assert!(!a[6..].contains(['I', 'L', 'O', 'U']));
    }
}
