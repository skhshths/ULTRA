//! Which ULTRA this is. The public and experimental builds differ only in this file, so they
//! can run side by side with separate data. Kept per branch: see .gitattributes.
#![allow(dead_code)]

pub const NAME: &str = "ULTRA Experimental";
/// Folder for settings, session, history, bookmarks and sign-ins.
pub const DATA_DIR: &str = "Ultra Experimental";
/// Shown next to the logo (empty for the public build).
pub const TAG: &str = "EXP";
/// Registry name of ULTRA's "web page" file type (default browser).
pub const PROGID: &str = "ULTRAEXPHTML";
/// The app icon in inverted colors, to tell the builds apart at a glance (the app itself looks
/// the same as the public one).
pub const INVERT: bool = true;
/// GitHub repository whose "latest" release this build updates from.
pub const REPO: &str = "skhshths/ULTRA-experimental";
/// Developer switches for test scripts (ULTRA_DEVTOOLS_PORT). Never in the public build.
pub const TEST_SWITCHES: bool = true;

/// What this build has that the public one doesn't yet, listed under the help hint on the
/// new-tab page. Add a line with every experimental feature; remove it once public ships it.
/// (Public's edition.rs has this list empty.)
pub const NEW_HERE: &[&str] = &[
    "updates install straight from the private repo (connect GitHub once in settings)",
];
