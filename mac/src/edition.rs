//! Which ULTRA this is. The public and experimental builds differ only in this file, so they
//! can run side by side with separate data. Kept per branch: see .gitattributes.
#![allow(dead_code)]

pub const NAME: &str = "ULTRA Experimental";
/// Folder for settings, session, history, bookmarks and sign-ins.
pub const DATA_DIR: &str = "Ultra Experimental";
/// Shown next to the logo (empty for the public build).
pub const TAG: &str = "EXP";
/// Must stay unique per build so macOS treats them as two apps.
pub const BUNDLE_ID: &str = "app.ultra.browser.experimental";
/// The chrome UI in inverted colors (light), to tell the builds apart at a glance.
pub const INVERT: bool = true;
