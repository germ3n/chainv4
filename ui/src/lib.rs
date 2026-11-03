//! What chainv4's egui apps, the wallet and the explorer, share: one look, its palette and fonts,
//! with each app picking only how dense it is; the widgets their pages and their sidebars, bars and
//! tabs are built from, which take their sizes from the look; picking files; talking to a node,
//! through a proxy when one is set; and running on Android, where an app keeps clear of the
//! system's bars and keyboard, copies and pastes through Android's clipboard, and picks files
//! through a Java activity of its own, and on iOS, through UIKit and AVFoundation; and biometrics,
//! which keep a secret behind a fingerprint, a face, Touch ID, Face ID or a passkey; and, on a
//! phone, whether it is on its charger. And finding a ballot among many, as both Governance pages
//! do, and drawing their votes: tallies, voters, what a change moves and what was decided when;
//! what became of what was asked of a node, and asking it again; pulling a page down to reload it;
//! and filtering an address's transactions. And reading blocks a node sends, each checked, and
//! keeping what an app makes of them where it can be rebuilt from.

pub mod asks;
pub mod ballots;
pub mod biometric;
#[cfg(feature = "blocks")]
pub mod blocks;
#[cfg(feature = "camera")]
pub mod camera;
pub mod files;
pub mod filters;
#[cfg(not(target_arch = "wasm32"))]
pub mod kept;
pub mod layout;
pub mod net;
pub mod power;
pub mod pull;
pub mod secret;
#[cfg(feature = "blocks")]
pub mod shelf;
pub mod sse;
pub mod theme;
pub mod votes;
pub mod widgets;

#[cfg(target_os = "android")]
pub mod android;
#[cfg(target_os = "ios")]
pub mod ios;

#[cfg(any(target_os = "android", target_os = "ios", test))]
mod edit;

/// The node an app talks to until told otherwise: the public devnet's. A page a node serves talks to
/// that node instead, and a node asked for by flag, environment or Settings comes first.
pub const PUBLIC_NODE: &str = "https://chain.homepage.gs";

pub use theme::{DARK, DISPLAY, Density, LIGHT, Palette, SEMIBOLD, apply, install_fonts};
