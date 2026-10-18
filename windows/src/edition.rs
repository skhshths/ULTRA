//! Which ULTRA this is. The public and experimental builds differ only in this file, so they
//! can run side by side with separate data. Kept per branch: see .gitattributes.
#![allow(dead_code)]

pub const NAME: &str = "ULTRA";
/// Folder for settings, session, history, bookmarks and sign-ins.
pub const DATA_DIR: &str = "Ultra";
/// Shown next to the logo (empty for the public build).
pub const TAG: &str = "";
/// Registry name of ULTRA's "web page" file type (default browser).
pub const PROGID: &str = "ULTRAHTML";
/// The chrome UI in inverted colors (light), to tell the builds apart at a glance.
pub const INVERT: bool = false;
/// GitHub repository whose "latest" release this build updates from.
pub const REPO: &str = "skhshths/ULTRA";
/// Developer switches for test scripts (ULTRA_DEVTOOLS_PORT). Never in the public build.
pub const TEST_SWITCHES: bool = false;

/// What this build has that the public one doesn't yet (listed on the new-tab page). The
/// public build has nothing extra.
pub const NEW_HERE: &[&str] = &[];
