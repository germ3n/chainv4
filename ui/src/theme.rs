//! The look both apps share, the wiki's: its palette, dark or light, and its type, Manrope for text,
//! JetBrains Mono for addresses, hashes and numbers, and Newsreader for page titles and the big
//! numbers. Each app picks only how dense it is. Applying a look dresses egui in it: its panels and
//! widgets, its text sizes and spacing, and the radius and margin the widgets here take for cards.

use {
    egui::{Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Stroke, TextStyle, Vec2, vec2},
    std::sync::Arc,
};

/// The font family for semibold text, besides proportional and monospace.
pub const SEMIBOLD: &str = "semibold";
/// The font family for titles and big numbers: the typeface's display face, or its semibold one.
pub const DISPLAY: &str = "display";

/// One set of colors.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    /// Behind everything.
    pub paper: Color32,
    /// Behind the sidebar and the bars.
    pub nav: Color32,
    /// Behind a card.
    pub card: Color32,
    /// Behind a field, a row picked out, a button.
    pub row: Color32,
    /// Lines between things, and around them.
    pub line: Color32,
    /// Text.
    pub ink: Color32,
    /// Text that matters less.
    pub soft: Color32,
    /// Text that matters least: notes and labels.
    pub muted: Color32,
    /// Section headings in capitals.
    pub heading: Color32,
    /// Links, what is picked, and the one button a step leads to.
    pub accent: Color32,
    /// What sits on the accent.
    pub on_accent: Color32,
    pub good: Color32,
    pub warn: Color32,
    pub bad: Color32,
    pub dark: bool,
}

/// How much room things take: spacing, text sizes, and the radius and margin of cards.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Density {
    /// Between widgets, across and down.
    pub spacing: Vec2,
    /// Inside a button.
    pub button_padding: Vec2,
    /// How tall a widget is at least: what a finger needs on a phone.
    pub interact: f32,
    pub small: f32,
    pub body: f32,
    pub button: f32,
    pub mono: f32,
    /// A heading, in the display face.
    pub heading: f32,
    /// A card's corners.
    pub radius: u8,
    /// Inside a card.
    pub margin: i8,
}

impl Density {
    /// Roomy: a few things at a time, each given room, as the wallet shows them.
    pub const COMFORTABLE: Density = Density {
        spacing: vec2(10.0, 8.0),
        button_padding: vec2(14.0, 7.0),
        interact: 30.0,
        small: 11.5,
        body: 14.5,
        button: 14.0,
        mono: 13.0,
        heading: 26.0,
        radius: 12,
        margin: 20,
    };

    /// Tight: many things at once, to be scanned, as the explorer shows them. Text a little smaller and
    /// less room between things, in the same cards with the same corners.
    pub const COMPACT: Density = Density {
        spacing: vec2(8.0, 5.0),
        button_padding: vec2(10.0, 4.0),
        interact: 26.0,
        small: 11.0,
        body: 13.5,
        button: 13.0,
        mono: 12.5,
        heading: 22.0,
        radius: 12,
        margin: 14,
    };
}

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

/// Loads the look's fonts, whose licences are in assets/fonts. These are small Latin subsets: each
/// family falls back on egui's own fonts for what it lacks, the arrows among them, which only its
/// mono face has, so that no text draws one as nothing.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let mut add = |name: &str, bytes: &'static [u8]| {
        fonts.font_data.insert(name.to_string(), Arc::new(FontData::from_static(bytes)));
    };

    add("text", include_bytes!("../assets/fonts/Manrope-Regular.ttf"));
    add("semibold", include_bytes!("../assets/fonts/Manrope-Semibold.ttf"));
    add("mono", include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf"));
    add("display", include_bytes!("../assets/fonts/Newsreader-Regular.ttf"));

    let fallback = |fonts: &FontDefinitions, family: FontFamily| fonts.families.get(&family).cloned().unwrap_or_default();
    let monospace = fallback(&fonts, FontFamily::Monospace);
    let mut proportional = fallback(&fonts, FontFamily::Proportional);

    for font in &monospace {
        if !proportional.contains(font) {
            proportional.push(font.clone());
        }
    }

    let with = |first: &str, rest: &[String]| [first.to_string()].into_iter().chain(rest.iter().cloned()).collect::<Vec<_>>();

    fonts.families.insert(FontFamily::Proportional, with("text", &proportional));
    fonts.families.insert(FontFamily::Monospace, with("mono", &monospace));
    fonts.families.insert(FontFamily::Name(SEMIBOLD.into()), with("semibold", &proportional));
    fonts.families.insert(FontFamily::Name(DISPLAY.into()), with("display", &proportional));
    ctx.set_fonts(fonts);
}

/// Says when a value differs from the one before, for telling a system only what it does not know.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
#[derive(Debug)]
pub(crate) struct Changed<T>(Option<T>);

impl<T> Default for Changed<T> {
    fn default() -> Self {
        Changed(None)
    }
}

#[cfg_attr(not(target_os = "android"), allow(dead_code))]
impl<T: PartialEq> Changed<T> {
    /// Whether this one is new, the first always; it is remembered either way.
    pub(crate) fn is_new(&mut self, now: T) -> bool {
        let new = self.0.as_ref() != Some(&now);
        self.0 = Some(now);

        new
    }
}

/// Dresses egui in a palette and a density. The palette picks the theme, so the system's own
/// preference never swaps it for egui's.
pub fn apply(ctx: &egui::Context, palette: &Palette, density: &Density) {
    let theme = if palette.dark { egui::Theme::Dark } else { egui::Theme::Light };
    let mut style = (*ctx.style_of(theme)).clone();
    let mut visuals = if palette.dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    let radius = CornerRadius::same(density.radius);
    let small = CornerRadius::same(density.radius.min(8));
    visuals.panel_fill = palette.paper;
    visuals.window_fill = palette.card;
    visuals.extreme_bg_color = palette.row;
    visuals.faint_bg_color = palette.row;
    visuals.code_bg_color = palette.row;
    visuals.override_text_color = Some(palette.ink);
    visuals.hyperlink_color = palette.accent;
    visuals.selection.bg_fill = palette.accent.gamma_multiply(0.35);
    visuals.selection.stroke = Stroke::new(1.0, palette.accent);
    visuals.window_stroke = Stroke::new(1.0, palette.line);
    visuals.window_corner_radius = radius;
    visuals.menu_corner_radius = small;
    visuals.striped = true;

    for (widget, fill, stroke) in [
        (&mut visuals.widgets.noninteractive, palette.card, palette.line),
        (&mut visuals.widgets.inactive, palette.row, palette.line),
        (&mut visuals.widgets.hovered, palette.row, palette.accent.gamma_multiply(0.7)),
        (&mut visuals.widgets.active, palette.row, palette.accent),
        (&mut visuals.widgets.open, palette.row, palette.accent),
    ] {
        widget.bg_fill = fill;
        widget.weak_bg_fill = fill;
        widget.bg_stroke = Stroke::new(1.0, stroke);
        widget.corner_radius = small;
        widget.fg_stroke.color = palette.ink;
    }

    visuals.widgets.noninteractive.fg_stroke.color = palette.soft;
    style.visuals = visuals;
    style.spacing.item_spacing = density.spacing;
    style.spacing.button_padding = density.button_padding;
    style.spacing.interact_size.y = density.interact;
    style.spacing.window_margin = Margin::same(density.margin);
    style.text_styles = [
        (TextStyle::Small, FontId::proportional(density.small)),
        (TextStyle::Body, FontId::proportional(density.body)),
        (TextStyle::Button, FontId::proportional(density.button)),
        (TextStyle::Monospace, FontId::monospace(density.mono)),
        (TextStyle::Heading, FontId::new(density.heading, FontFamily::Name(DISPLAY.into()))),
    ]
    .into();
    ctx.set_style_of(theme, style);
    ctx.set_theme(theme);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_first_value_and_a_different_one_are_new() {
        let mut told = Changed::default();

        assert!(told.is_new(true), "nothing was said before");
        assert!(!told.is_new(true));
        assert!(told.is_new(false));
        assert!(!told.is_new(false));
    }

    /// How far the two are apart in lightness, by the WCAG's measure, 1 to 21.
    fn contrast(one: Color32, other: Color32) -> f32 {
        let lightness = |color: Color32| {
            let [red, green, blue] = [color.r(), color.g(), color.b()].map(|channel| {
                let channel = f32::from(channel) / 255.0;

                if channel <= 0.04045 { channel / 12.92 } else { ((channel + 0.055) / 1.055).powf(2.4) }
            });

            0.2126 * red + 0.7152 * green + 0.0722 * blue
        };
        let (light, dark) = (lightness(one).max(lightness(other)), lightness(one).min(lightness(other)));

        (light + 0.05) / (dark + 0.05)
    }

    #[test]
    fn text_reads_in_both_palettes() {
        for palette in [DARK, LIGHT] {
            for ground in [palette.paper, palette.nav, palette.card] {
                assert!(contrast(palette.ink, ground) >= 7.0, "{palette:?}");
                assert!(contrast(palette.soft, ground) >= 4.5, "{palette:?}");
                assert!(contrast(palette.muted, ground) >= 3.0, "{palette:?}");
            }

            assert!(contrast(palette.on_accent, palette.accent) >= 4.5, "{palette:?}");
        }
    }

    #[test]
    fn a_look_sets_the_spacing_and_sizes_cards_take() {
        let ctx = egui::Context::default();
        let palette = Palette {
            paper: Color32::BLACK,
            nav: Color32::BLACK,
            card: Color32::from_gray(20),
            row: Color32::from_gray(30),
            line: Color32::from_gray(40),
            ink: Color32::WHITE,
            soft: Color32::LIGHT_GRAY,
            muted: Color32::GRAY,
            heading: Color32::GRAY,
            accent: Color32::LIGHT_BLUE,
            on_accent: Color32::BLACK,
            good: Color32::GREEN,
            warn: Color32::YELLOW,
            bad: Color32::RED,
            dark: true,
        };
        apply(&ctx, &palette, &Density::COMPACT);
        let style = ctx.style_of(egui::Theme::Dark);

        assert_eq!(style.spacing.window_margin, Margin::same(Density::COMPACT.margin));
        assert_eq!(style.visuals.window_corner_radius, CornerRadius::same(Density::COMFORTABLE.radius), "the same corners, however dense");
        assert_eq!(style.visuals.panel_fill, Color32::BLACK);
        assert_eq!(style.text_styles[&TextStyle::Body].size, 13.5);
        assert_eq!(ctx.theme(), egui::Theme::Dark);
    }
}
