//! Being a browser for the rest of Windows: registering ULTRA so it shows up under
//! Settings → Default apps, and telling whether it is the default for web links.
//! Everything goes in HKEY_CURRENT_USER, so no admin rights are needed.

use std::path::Path;

use windows::Win32::{
    Foundation::ERROR_SUCCESS,
    System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegGetValueW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_WRITE,
        REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_SZ,
    },
    UI::Shell::{SHChangeNotify, SHCNE_ASSOCCHANGED, SHCNF_IDLIST},
};
use windows_core::{HSTRING, PCWSTR};

use crate::edition::{NAME, PROGID};

/// Set one string value (`None` = the key's default value), creating the key if needed.
fn set(key: &str, name: Option<&str>, value: &str) -> bool {
    unsafe {
        let mut h = HKEY::default();
        if RegCreateKeyExW(HKEY_CURRENT_USER, &HSTRING::from(key), None, PCWSTR::null(), REG_OPTION_NON_VOLATILE, KEY_WRITE, None, &mut h, None)
            != ERROR_SUCCESS
        {
            return false;
        }
        let data: Vec<u8> = value.encode_utf16().chain([0]).flat_map(|c| c.to_le_bytes()).collect();
        let name = name.map(HSTRING::from);
        let ok = RegSetValueExW(h, name.as_ref().map_or(PCWSTR::null(), |n| PCWSTR(n.as_ptr())), None, REG_SZ, Some(&data)) == ERROR_SUCCESS;
        let _ = RegCloseKey(h);
        ok
    }
}

/// Read one string value, if present.
fn get(key: &str, name: &str) -> Option<String> {
    unsafe {
        let mut buf = [0u16; 512];
        let mut len = (buf.len() * 2) as u32;
        let r = RegGetValueW(
            HKEY_CURRENT_USER,
            &HSTRING::from(key),
            &HSTRING::from(name),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut len),
        );
        (r == ERROR_SUCCESS).then(|| String::from_utf16_lossy(&buf[..(len as usize / 2).saturating_sub(1)]))
    }
}

/// Register ULTRA as a web browser for this user (links, .html files). Windows then lists it
/// as a choice; only the user can make it the default (Windows doesn't let apps do that).
pub fn register(exe: &Path) -> bool {
    let exe = exe.to_string_lossy();
    let open = format!("\"{exe}\" \"%1\"");
    let icon = format!("\"{exe}\",0");
    let client = format!(r"Software\Clients\StartMenuInternet\{NAME}");
    let caps = format!(r"{client}\Capabilities");
    let progid = format!(r"Software\Classes\{PROGID}");
    let ok = [
        set(&progid, None, &format!("{NAME} web page")),
        set(&progid, Some("FriendlyTypeName"), &format!("{NAME} web page")),
        set(&format!(r"{progid}\DefaultIcon"), None, &icon),
        set(&format!(r"{progid}\shell\open\command"), None, &open),
        set(&client, None, NAME),
        set(&format!(r"{client}\DefaultIcon"), None, &icon),
        set(&format!(r"{client}\shell\open\command"), None, &format!("\"{exe}\"")),
        set(&caps, Some("ApplicationName"), NAME),
        set(&caps, Some("ApplicationDescription"), "A fast, minimal, tiling web browser."),
        set(&caps, Some("ApplicationIcon"), &icon),
        set(&format!(r"{caps}\StartMenu"), Some("StartMenuInternet"), NAME),
        set(&format!(r"{caps}\URLAssociations"), Some("http"), PROGID),
        set(&format!(r"{caps}\URLAssociations"), Some("https"), PROGID),
        set(&format!(r"{caps}\FileAssociations"), Some(".htm"), PROGID),
        set(&format!(r"{caps}\FileAssociations"), Some(".html"), PROGID),
        set(r"Software\RegisteredApplications", Some(NAME), &caps),
    ]
    .iter()
    .all(|&b| b);
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
    ok
}

/// ULTRA is a portable exe: if it was registered before, point the registration at wherever
/// it runs from now.
pub fn refresh(exe: &Path) {
    let open = format!("\"{}\" \"%1\"", exe.to_string_lossy());
    if let Some(cur) = get(&format!(r"Software\Classes\{PROGID}\shell\open\command"), "") {
        if cur != open {
            register(exe);
        }
    }
}

/// Whether web links currently open in ULTRA.
pub fn is_default() -> bool {
    get(r"Software\Microsoft\Windows\Shell\Associations\UrlAssociations\https\UserChoice", "ProgId").as_deref() == Some(PROGID)
}

/// Windows' own Default apps page, opened at ULTRA.
pub fn open_settings() {
    // Through the shell, like a clicked link: handing an ms-settings: link with a query to
    // explorer.exe can open File Explorer instead of Settings.
    use windows::Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL};
    let uri = HSTRING::from(format!("ms-settings:defaultapps?registeredAppUser={}", NAME.replace(' ', "%20")));
    unsafe {
        ShellExecuteW(None, &HSTRING::from("open"), &uri, PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
    }
}

/// What a link handed to ULTRA by Windows should load: web addresses as they are, local
/// files (.html) as file:/// URLs. Anything else is ignored.
pub fn link_to_url(arg: &str) -> Option<String> {
    let a = arg.trim().trim_matches('"');
    let lower = a.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return Some(a.to_string());
    }
    let p = Path::new(a);
    if p.is_absolute() && p.exists() {
        return Some(format!("file:///{}", a.replace('\\', "/")));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_web_links_and_real_files_are_opened() {
        assert_eq!(link_to_url("https://example.com/a?b=1").as_deref(), Some("https://example.com/a?b=1"));
        assert_eq!(link_to_url("\"HTTP://x.test\"").as_deref(), Some("HTTP://x.test"));
        assert_eq!(link_to_url("javascript:alert(1)"), None);
        assert_eq!(link_to_url("C:\\definitely\\not\\here.html"), None);
        let f = std::env::current_exe().unwrap();
        assert!(link_to_url(&f.to_string_lossy()).unwrap().starts_with("file:///"));
    }

    #[test]
    fn registry_values_round_trip() {
        let key = format!(r"Software\ULTRA-test-{}", std::process::id());
        let cmd = r#""C:\a b\ultra.exe" "%1""#;
        assert!(set(&format!(r"{key}\shell"), None, cmd));
        assert!(set(&key, Some("Name"), "ULTRA"));
        assert_eq!(get(&format!(r"{key}\shell"), "").as_deref(), Some(cmd));
        assert_eq!(get(&key, "Name").as_deref(), Some("ULTRA"));
        assert_eq!(get(&key, "Missing"), None);
        unsafe {
            let _ = windows::Win32::System::Registry::RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(key.as_str()));
        }
        assert_eq!(get(&key, "Name"), None);
    }
}
