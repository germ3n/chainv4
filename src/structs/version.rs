//! Release versions, as Cargo writes them (major.minor.patch), and the upgrade governance decides:
//! from a height on, nodes older than a version stop, so a release that changes the rules, such
//! as the block format, reaches a chain without old nodes following it onto a fork of their own.

use {
    crate::crypto::hashable::Hashable,
    r#macro::document,
    std::fmt,
    wincode::{SchemaRead, SchemaWrite},
};

#[document(
    summary = "A release's version, major.minor.patch, as Cargo.toml writes it. Versions order by major, then minor, then patch.",
    category = "Releases",
    see_also = "Version::ours, Upgrade",
)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, std::hash::Hash, SchemaWrite, SchemaRead)]
pub struct Version {
    #[document(summary = "The major version.")]
    pub major: u32,
    #[document(summary = "The minor version.")]
    pub minor: u32,
    #[document(summary = "The patch version.")]
    pub patch: u32,
}

impl Hashable for Version {}

impl Version {
    #[document(parent = "Version", summary = "A version from its three parts.")]
    pub const fn new(major: u32, minor: u32, patch: u32) -> Version {
        Version { major, minor, patch }
    }

    #[document(
        parent = "Version",
        summary = "This build of chainv4's version, from its Cargo.toml: what a node checks upgrades against, and what chainv4 update compares releases with.",
    )]
    pub fn ours() -> Version {
        env!("CARGO_PKG_VERSION").parse().expect("Cargo.toml's version is major.minor.patch")
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// Reads major.minor.patch, with an optional leading v; a pre-release or build suffix, such as
/// -beta.1 or +abc, is not a version a release names.
impl std::str::FromStr for Version {
    type Err = String;

    fn from_str(text: &str) -> Result<Version, String> {
        let trimmed = text.trim();
        let digits = trimmed.strip_prefix('v').unwrap_or(trimmed);
        let parts: Vec<&str> = digits.split('.').collect();
        let part = |text: &str| text.parse::<u32>().ok().filter(|_| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()));

        match parts.as_slice() {
            [major, minor, patch] => match (part(major), part(minor), part(patch)) {
                (Some(major), Some(minor), Some(patch)) => Ok(Version { major, minor, patch }),
                _ => Err(format!("{text} is not a version such as 1.2.3")),
            },
            _ => Err(format!("{text} is not a version such as 1.2.3")),
        }
    }
}

#[document(
    summary = "An upgrade governance decides with Change::Upgrade: from block from on, nodes older than least stop before importing it, and say to update. A later one replaces it.",
    category = "Releases",
    see_also = "Change::Upgrade, Changes::upgrade, Version",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct Upgrade {
    #[document(summary = "The oldest version that runs on past it.")]
    pub least: Version,
    #[document(summary = "The first block older nodes do not import.")]
    pub from: u64,
}

impl Hashable for Upgrade {}

impl Upgrade {
    #[document(
        parent = "Upgrade",
        summary = "Why a chain at this height cannot decide it: a height already reached, or a least version of 0.0.0, which stops nothing.",
        params = { height = "The height of the block deciding it." },
    )]
    pub fn problem(&self, height: u64) -> Option<&'static str> {
        if self.least == Version::default() {
            Some("an upgrade's least version is above 0.0.0")
        } else if self.from <= height {
            Some("an upgrade's height is one the chain has yet to reach")
        } else {
            None
        }
    }

    #[document(parent = "Upgrade", summary = "Whether it stops a node of this version: one older than least.")]
    pub fn stops(&self, version: Version) -> bool {
        version < self.least
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_read_order_and_print_as_cargo_writes_them() {
        assert_eq!("1.2.3".parse(), Ok(Version::new(1, 2, 3)));
        assert_eq!("v0.10.0".parse(), Ok(Version::new(0, 10, 0)));
        assert!("1.2".parse::<Version>().is_err());
        assert!("1.2.3-beta".parse::<Version>().is_err());
        assert!("1..3".parse::<Version>().is_err());
        assert!("+1.2.3".parse::<Version>().is_err());
        assert!(Version::new(0, 10, 0) > Version::new(0, 9, 12));
        assert!(Version::new(1, 0, 0) > Version::new(0, 99, 99));
        assert_eq!(Version::new(4, 5, 6).to_string(), "4.5.6");
        assert_eq!(Version::ours().to_string(), env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn an_upgrade_stops_older_versions_and_needs_a_height_ahead() {
        let upgrade = Upgrade { least: Version::new(0, 2, 0), from: 100 };

        assert!(upgrade.stops(Version::new(0, 1, 9)));
        assert!(!upgrade.stops(Version::new(0, 2, 0)));
        assert_eq!(upgrade.problem(99), None);
        assert!(upgrade.problem(100).is_some());
        assert!(Upgrade { least: Version::default(), from: 100 }.problem(1).is_some());
    }
}
