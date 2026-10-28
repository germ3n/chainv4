//! The wallet's look: the wiki's palette and type, dark or light, roomy. Manrope for text,
//! JetBrains Mono for addresses and hashes, and Newsreader only for page titles and the big
//! numbers, so it reads calmer than the wiki's pages. The widgets every page is built from are
//! chainv4-ui's, shared with the explorer.

pub use chainv4_ui::widgets::{card, caps, dot, fitted, mono, pill, primary, quiet, rule, strong, title};
pub use chainv4_ui::{DISPLAY as SERIF, Palette, SEMIBOLD};
use {
    chainv4_ui::{Density, Typeface},
    egui::Color32,
};

pub const DARK: Palette = Palette {
    paper: Color32::from_rgb(0x0c, 0x0d, 0x10),
    nav: Color32::from_rgb(0x0f, 0x10, 0x13),
    card: Color32::from_rgb(0x14, 0x16, 0x1a),
    row: Color32::from_rgb(0x1b, 0x1d, 0x22),
    line: Color32::from_rgb(0x24, 0x26, 0x2b),
    ink: Color32::from_rgb(0xec, 0xe8, 0xe1),
    soft: Color32::from_rgb(0xc4, 0xc0, 0xb7),
    muted: Color32::from_rgb(0x86, 0x83, 0x7c),
    heading: Color32::from_rgb(0xa7, 0xa3, 0x9a),
    accent: Color32::from_rgb(0xd6, 0xaa, 0x62),
    on_accent: Color32::from_rgb(0x15, 0x12, 0x0c),
    good: Color32::from_rgb(0xa3, 0xc9, 0x9c),
    warn: Color32::from_rgb(0xe8, 0xb8, 0x72),
    bad: Color32::from_rgb(0xec, 0x8d, 0x8d),
    dark: true,
};

pub const LIGHT: Palette = Palette {
    paper: Color32::from_rgb(0xf8, 0xf5, 0xef),
    nav: Color32::from_rgb(0xf3, 0xef, 0xe7),
    card: Color32::from_rgb(0xff, 0xfd, 0xf9),
    row: Color32::from_rgb(0xeb, 0xe5, 0xd8),
    line: Color32::from_rgb(0xe2, 0xdc, 0xcf),
    ink: Color32::from_rgb(0x1d, 0x1b, 0x17),
    soft: Color32::from_rgb(0x46, 0x42, 0x3a),
    muted: Color32::from_rgb(0x7b, 0x76, 0x6b),
    heading: Color32::from_rgb(0x6a, 0x65, 0x5b),
    accent: Color32::from_rgb(0x98, 0x68, 0x1b),
    on_accent: Color32::from_rgb(0xff, 0xfd, 0xf9),
    good: Color32::from_rgb(0x3f, 0x7a, 0x3a),
    warn: Color32::from_rgb(0xa8, 0x63, 0x1a),
    bad: Color32::from_rgb(0xb2, 0x3b, 0x3b),
    dark: false,
};

/// The wiki's fonts. Their licences are in assets/fonts.
const TYPEFACE: Typeface = Typeface {
    text: include_bytes!("../assets/fonts/Manrope-Regular.ttf"),
    semibold: include_bytes!("../assets/fonts/Manrope-Semibold.ttf"),
    mono: include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf"),
    display: Some(include_bytes!("../assets/fonts/Newsreader-Regular.ttf")),
};

/// Loads the wiki's fonts.
pub fn install_fonts(ctx: &egui::Context) {
    chainv4_ui::install_fonts(ctx, &TYPEFACE);
}

/// Dresses egui in a palette, roomy. Text wraps everywhere, in rows too, where egui would let it
/// run past the edge: a phone's screen is narrow, and nothing the wallet says may be cut off.
pub fn apply(ctx: &egui::Context, palette: &Palette) {
    chainv4_ui::apply(ctx, palette, &Density::COMFORTABLE);
    ctx.all_styles_mut(|style| style.wrap_mode = Some(egui::TextWrapMode::Wrap));
}
