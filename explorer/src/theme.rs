//! The explorer's look: cool slate and blue, dark or light, set tight in IBM Plex, for scanning a
//! lot of data at once. Plex Sans for text and Plex Mono for ids, addresses and numbers to compare.
//! The widgets pages are built from are chainv4-ui's, shared with the wallet, which looks unlike
//! this on purpose.

use {
    chainv4_ui::{Density, Palette, Typeface},
    egui::Color32,
};

pub const DARK: Palette = Palette {
    paper: Color32::from_rgb(0x0b, 0x0e, 0x13),
    nav: Color32::from_rgb(0x0f, 0x13, 0x19),
    card: Color32::from_rgb(0x12, 0x18, 0x20),
    row: Color32::from_rgb(0x19, 0x21, 0x2b),
    line: Color32::from_rgb(0x23, 0x2d, 0x39),
    ink: Color32::from_rgb(0xe3, 0xe9, 0xf0),
    soft: Color32::from_rgb(0xb3, 0xbe, 0xcb),
    muted: Color32::from_rgb(0x7b, 0x88, 0x97),
    heading: Color32::from_rgb(0x8b, 0x9b, 0xb0),
    accent: Color32::from_rgb(0x5a, 0xa9, 0xff),
    on_accent: Color32::from_rgb(0x06, 0x12, 0x20),
    good: Color32::from_rgb(0x4c, 0xc3, 0x8a),
    warn: Color32::from_rgb(0xe5, 0xa5, 0x4b),
    bad: Color32::from_rgb(0xf0, 0x6c, 0x6c),
    dark: true,
};

pub const LIGHT: Palette = Palette {
    paper: Color32::from_rgb(0xf5, 0xf7, 0xfa),
    nav: Color32::from_rgb(0xec, 0xef, 0xf4),
    card: Color32::from_rgb(0xff, 0xff, 0xff),
    row: Color32::from_rgb(0xee, 0xf1, 0xf5),
    line: Color32::from_rgb(0xd9, 0xdf, 0xe6),
    ink: Color32::from_rgb(0x18, 0x20, 0x2a),
    soft: Color32::from_rgb(0x3a, 0x45, 0x53),
    muted: Color32::from_rgb(0x68, 0x75, 0x87),
    heading: Color32::from_rgb(0x5a, 0x67, 0x78),
    accent: Color32::from_rgb(0x1f, 0x6f, 0xeb),
    on_accent: Color32::from_rgb(0xff, 0xff, 0xff),
    good: Color32::from_rgb(0x1f, 0x88, 0x3d),
    warn: Color32::from_rgb(0x9a, 0x67, 0x00),
    bad: Color32::from_rgb(0xcf, 0x22, 0x2e),
    dark: false,
};

/// What colors the chain's own ideas take, the same on every page: the main chain and side
/// branches, and how far finality reached.
#[derive(Clone, Copy, Debug)]
pub struct Roles {
    pub main: Color32,
    pub side: Color32,
    pub justified: Color32,
    pub final_: Color32,
}

impl Roles {
    pub fn of(palette: &Palette) -> Roles {
        Roles {
            main: palette.accent,
            side: palette.warn,
            justified: match palette.dark {
                true => Color32::from_rgb(0x3f, 0xc1, 0xc9),
                false => Color32::from_rgb(0x0e, 0x7c, 0x86),
            },
            final_: palette.good,
        }
    }
}

/// IBM Plex. Its licence is in assets/fonts.
const TYPEFACE: Typeface = Typeface {
    text: include_bytes!("../assets/fonts/IBMPlexSans-Regular.ttf"),
    semibold: include_bytes!("../assets/fonts/IBMPlexSans-SemiBold.ttf"),
    mono: include_bytes!("../assets/fonts/IBMPlexMono-Regular.ttf"),
    display: None,
};

pub fn install_fonts(ctx: &egui::Context) {
    chainv4_ui::install_fonts(ctx, &TYPEFACE);
}

/// Dresses egui in a palette, tight.
pub fn apply(ctx: &egui::Context, palette: &Palette) {
    chainv4_ui::apply(ctx, palette, &Density::COMPACT);
}
