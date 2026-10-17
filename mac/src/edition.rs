//! Which ULTRA this is. The public and experimental builds differ only in this file, so they
//! can run side by side with separate data. Kept per branch: see .gitattributes.
#![allow(dead_code)]

pub const NAME: &str = "ULTRA";
/// Folder for settings, session, history, bookmarks and sign-ins.
pub const DATA_DIR: &str = "Ultra";
/// Shown next to the logo (empty for the public build).
pub const TAG: &str = "";
/// Must stay unique per build so macOS treats them as two apps.
pub const BUNDLE_ID: &str = "app.ultra.browser";
/// The chrome UI in inverted colors (light), to tell the builds apart at a glance.
pub const INVERT: bool = false;
