//! The wallet's look, the same as the explorer's, is chainv4-ui's: its palette, its fonts and the
//! widgets every page is built from. What is the wallet's own is how roomy it is, and that its text
//! wraps everywhere.

pub use chainv4_ui::layout::section_gap;
pub use chainv4_ui::widgets::{arrowed, beside, caps, dot, end_row, fitted, mono, page_card as card, pill, primary, quiet, reload, retry, rule, segmented, segmented_rows, strong, title};
pub use chainv4_ui::{DARK, DISPLAY as SERIF, LIGHT, Palette, SEMIBOLD, install_fonts};
use {chainv4_ui::Density, egui::Ui};

/// A note in small muted text that wraps, with a command's flags in the mono face.
#[cfg_attr(not(feature = "mining"), allow(dead_code))]
pub fn note(ui: &mut Ui, palette: &Palette, text: &str) -> egui::Response {
    chainv4_ui::widgets::flagged_label(ui, text, egui::TextStyle::Small, palette.muted)
}

/// Dresses egui in a palette, roomy. Text wraps everywhere, in rows too, where egui would let it
/// run past the edge: a phone's screen is narrow, and nothing the wallet says may be cut off.
pub fn apply(ctx: &egui::Context, palette: &Palette) {
    chainv4_ui::apply(ctx, palette, &Density::COMFORTABLE);
    ctx.all_styles_mut(|style| style.wrap_mode = Some(egui::TextWrapMode::Wrap));
}
