//! One ULTRA per profile. A second launch (a link clicked in another app, or the exe
//! double-clicked again) hands its link to the running ULTRA through a named pipe and exits,
//! instead of opening a second window over the same session.

use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    os::windows::io::FromRawHandle,
    path::Path,
};

use windows::Win32::{
    Storage::FileSystem::PIPE_ACCESS_INBOUND,
    System::Pipes::{ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT},
    UI::WindowsAndMessaging::{AllowSetForegroundWindow, ASFW_ANY},
};
use windows_core::HSTRING;

/// The pipe for one data folder (so a test copy with its own data runs side by side).
pub fn pipe_name(data_dir: &Path) -> String {
    let key = data_dir.to_string_lossy().to_lowercase();
    // FNV-1a: stable across runs, unlike std's hasher.
    let h = key.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3));
    format!(r"\\.\pipe\ultra-{h:016x}")
}

/// If an ULTRA is already running, give it `msg` (a URL, or empty to just bring it forward)
/// and return true.
pub fn forward(pipe: &str, msg: &str) -> bool {
    let Ok(mut f) = OpenOptions::new().write(true).open(pipe) else { return false };
    // This process was just started by the user, so it may hand the foreground on.
    unsafe {
        let _ = AllowSetForegroundWindow(ASFW_ANY);
    }
    f.write_all(msg.as_bytes()).is_ok()
}

/// Serve the pipe for the rest of the run: `on_msg` gets each message a later launch sends.
pub fn listen(pipe: &str, on_msg: impl Fn(String) + Send + 'static) {
    let name = HSTRING::from(pipe);
    std::thread::spawn(move || loop {
        let h = unsafe {
            CreateNamedPipeW(
                &name,
                PIPE_ACCESS_INBOUND,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                0,
                64 * 1024,
                0,
                None,
            )
        };
        if h.is_invalid() {
            return;
        }
        // A client that connected between create and connect reports "already connected".
        let connected = match unsafe { ConnectNamedPipe(h, None) } {
            Ok(()) => true,
            Err(e) => e.code() == windows::Win32::Foundation::ERROR_PIPE_CONNECTED.to_hresult(),
        };
        // The File owns the handle from here and closes it when dropped.
        let mut f = unsafe { File::from_raw_handle(h.0) };
        if connected {
            let mut msg = String::new();
            let _ = (&mut f).take(64 * 1024).read_to_string(&mut msg);
            on_msg(msg);
        }
    });
}
