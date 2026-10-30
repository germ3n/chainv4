//! The explorer's look is the wallet's, chainv4-ui's: its palette, its fonts and the widgets pages are
//! built from. What is the explorer's own is how tight it sets them, for scanning a lot of data at
//! once, and the colors the chain's own ideas take on its charts and map.

pub use chainv4_ui::{DARK, LIGHT, install_fonts};
use {
    chainv4_ui::{Density, Palette},
    egui::Color32,
};

/// What colors the chain's own ideas take, the same on every page: the main chain and side
/// branches, and how far finality reached. Green is the palette's, and the rest are picked to tell
/// from it and from each other for a person who cannot tell red from green, or blue from yellow, as
/// the palette's gold accent and its amber warning could not be told from each other by anyone.
#[derive(Clone, Copy, Debug)]
pub struct Roles {
    pub main: Color32,
    pub side: Color32,
    pub justified: Color32,
    pub final_: Color32,
    /// A call made on another's behalf, among a transaction's hops.
    pub delegate: Color32,
}

impl Roles {
    pub fn of(palette: &Palette) -> Roles {
        let (main, side, justified, delegate) = match palette.dark {
            true => ([0x4a, 0x90, 0xe2], [0xe0, 0x89, 0x3c], [0x3f, 0xc1, 0xc9], [0xb3, 0x9d, 0xf0]),
            false => ([0x1f, 0x5f, 0xbf], [0xb4, 0x58, 0x0f], [0x0e, 0x7c, 0x86], [0x7a, 0x2f, 0x9a]),
        };
        let color = |[red, green, blue]: [u8; 3]| Color32::from_rgb(red, green, blue);

        Roles { main: color(main), side: color(side), justified: color(justified), final_: palette.good, delegate: color(delegate) }
    }
}

/// What to write on a fill laid over the paper, as a block on the map: black or white, whichever
/// reads.
pub fn ink_on(fill: Color32, palette: &Palette) -> Color32 {
    let over = 1.0 - f32::from(fill.a()) / 255.0;
    let channel = |fill: u8, paper: u8| f32::from(fill) + over * f32::from(paper);
    let shown = Color32::from_rgb(channel(fill.r(), palette.paper.r()) as u8, channel(fill.g(), palette.paper.g()) as u8, channel(fill.b(), palette.paper.b()) as u8);

    match contrast(Color32::BLACK, shown) >= contrast(Color32::WHITE, shown) {
        true => Color32::BLACK,
        false => Color32::WHITE,
    }
}

/// How far apart two colors are in lightness, by the WCAG's measure: 1 to 21.
fn contrast(one: Color32, other: Color32) -> f32 {
    let (one, other) = (lightness(one), lightness(other));

    (one.max(other) + 0.05) / (one.min(other) + 0.05)
}

/// A color's relative lightness, by the WCAG's measure.
fn lightness(color: Color32) -> f32 {
    let [red, green, blue] = linear(color);

    0.2126 * red + 0.7152 * green + 0.0722 * blue
}

/// A color's channels as light, rather than as stored.
fn linear(color: Color32) -> [f32; 3] {
    [color.r(), color.g(), color.b()].map(|channel| {
        let channel = f32::from(channel) / 255.0;

        if channel <= 0.04045 { channel / 12.92 } else { ((channel + 0.055) / 1.055).powf(2.4) }
    })
}

/// Dresses egui in a palette, tight.
pub fn apply(ctx: &egui::Context, palette: &Palette) {
    chainv4_ui::apply(ctx, palette, &Density::COMPACT);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How a color looks to a person with no red cones, no green ones or no blue ones, by Machado,
    /// Oliveira and Fernandes' model of 2009, at full severity, and to everyone else.
    const SEEN: [[[f32; 3]; 3]; 4] = [
        [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        [[0.152_286, 1.052_583, -0.204_868], [0.114_503, 0.786_281, 0.099_216], [-0.003_882, -0.048_116, 1.051_998]],
        [[0.367_322, 0.860_646, -0.227_968], [0.280_085, 0.672_501, 0.047_413], [-0.011_820, 0.042_940, 0.968_881]],
        [[1.255_528, -0.076_749, -0.178_779], [-0.078_411, 0.930_809, 0.147_602], [0.004_733, 0.691_367, 0.303_900]],
    ];

    /// How far apart two colors look through one of those, in OKLab, where 1 is black to white.
    fn apart(one: Color32, other: Color32, seen: &[[f32; 3]; 3]) -> f32 {
        let lab = |color: Color32| {
            let light = linear(color);
            let [red, green, blue] = seen.map(|row| (row[0] * light[0] + row[1] * light[1] + row[2] * light[2]).clamp(0.0, 1.0));
            let [long, middle, short] = [
                0.412_221_47 * red + 0.536_332_54 * green + 0.051_445_99 * blue,
                0.211_903_5 * red + 0.680_699_5 * green + 0.107_396_96 * blue,
                0.088_302_46 * red + 0.281_718_84 * green + 0.629_978_7 * blue,
            ]
            .map(f32::cbrt);

            [
                0.210_454_26 * long + 0.793_617_8 * middle - 0.004_072_05 * short,
                1.977_998_5 * long - 2.428_592_2 * middle + 0.450_593_7 * short,
                0.025_904_04 * long + 0.782_771_77 * middle - 0.808_675_77 * short,
            ]
        };
        let (one, other) = (lab(one), lab(other));

        one.iter().zip(other).map(|(one, other)| (one - other).powi(2)).sum::<f32>().sqrt()
    }

    #[test]
    fn the_chains_colors_read_on_a_card_and_tell_apart_however_a_person_sees_color_in_both_themes() {
        for palette in [DARK, LIGHT] {
            let roles = Roles::of(&palette);
            let colors = [roles.main, roles.side, roles.justified, roles.final_];

            for color in colors.into_iter().chain([roles.delegate]) {
                assert!(contrast(color, palette.card) >= 3.0, "{color:?} on {:?}", palette.card);
                assert!(contrast(color, palette.paper) >= 3.0, "{color:?} on {:?}", palette.paper);
            }

            for (index, one) in colors.iter().enumerate() {
                for other in &colors[index + 1..] {
                    for seen in &SEEN {
                        assert!(apart(*one, *other, seen) >= 0.035, "{one:?} and {other:?} through {seen:?}: {}", apart(*one, *other, seen));
                    }
                }
            }
        }
    }

    #[test]
    fn what_is_written_on_a_block_reads_however_busy_the_block() {
        for palette in [DARK, LIGHT] {
            let roles = Roles::of(&palette);

            for base in [roles.main, roles.side, roles.final_] {
                for busy in [0.0_f32, 0.5, 1.0] {
                    let fill = base.gamma_multiply(0.45 + 0.55 * busy);
                    let over = 1.0 - f32::from(fill.a()) / 255.0;
                    let channel = |fill: u8, paper: u8| (f32::from(fill) + over * f32::from(paper)) as u8;
                    let shown = Color32::from_rgb(channel(fill.r(), palette.paper.r()), channel(fill.g(), palette.paper.g()), channel(fill.b(), palette.paper.b()));

                    assert!(contrast(ink_on(fill, &palette), shown) >= 4.5, "{fill:?} over {:?}", palette.paper);
                }
            }
        }
    }

    #[test]
    fn every_family_draws_the_symbols_pages_use() {
        let ctx = egui::Context::default();
        install_fonts(&ctx);
        let mut output = ctx.run_ui(egui::RawInput::default(), |_| {});
        output.textures_delta.clear();
        let families = [egui::FontFamily::Proportional, egui::FontFamily::Monospace, egui::FontFamily::Name(chainv4_ui::SEMIBOLD.into()), egui::FontFamily::Name(chainv4_ui::DISPLAY.into())];

        for family in families {
            for symbol in ['→', '‹', '›', '×', '·', '…', '—', 'a', '0'] {
                assert!(ctx.fonts_mut(|fonts| fonts.has_glyph(&egui::FontId::new(14.0, family.clone()), symbol)), "{symbol} in {family:?}");
            }
        }
    }
}
