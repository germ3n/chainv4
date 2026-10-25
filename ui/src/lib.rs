//! What chainv4's egui apps, the wallet and the explorer, share. Each has a look of its own: its
//! palette, its typeface and how dense it is. What they share is how a look is applied, the widgets
//! their pages are built from, which take their sizes from the look, picking files, and running on
//! Android, where an app keeps clear of the system's bars and keyboard, copies and pastes through
//! Android's clipboard, and picks files through a Java activity of its own.

pub mod files;
pub mod layout;
pub mod theme;
pub mod widgets;

#[cfg(target_os = "android")]
pub mod android;

#[cfg(any(target_os = "android", test))]
mod edit;

pub use theme::{DISPLAY, Density, Palette, SEMIBOLD, Typeface, apply, install_fonts};
