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
