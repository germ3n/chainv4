//! A look: a palette of colors, a typeface and a density, which each app picks for itself. Applying
//! one dresses egui in it: its panels and widgets, its text sizes and spacing, and the radius and
//! margin the widgets here take for cards.

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

    /// Tight: many things at once, to be scanned, as the explorer shows them.
    pub const COMPACT: Density = Density {
        spacing: vec2(8.0, 5.0),
        button_padding: vec2(10.0, 4.0),
        interact: 26.0,
        small: 11.0,
        body: 13.5,
        button: 13.0,
        mono: 12.5,
        heading: 20.0,
        radius: 6,
        margin: 12,
    };
}

/// The fonts a look is set in.
#[derive(Clone, Copy, Debug)]
pub struct Typeface {
    pub text: &'static [u8],
    pub semibold: &'static [u8],
    pub mono: &'static [u8],
    /// For titles and big numbers; the semibold face when None.
    pub display: Option<&'static [u8]>,
}

/// Loads a typeface, each face falling back to egui's own fonts for anything it lacks.
pub fn install_fonts(ctx: &egui::Context, typeface: &Typeface) {
    let mut fonts = FontDefinitions::default();
    let mut add = |name: &str, bytes: &'static [u8]| {
        fonts.font_data.insert(name.to_string(), Arc::new(FontData::from_static(bytes)));
    };

    add("text", typeface.text);
    add("semibold", typeface.semibold);
    add("mono", typeface.mono);
    add("display", typeface.display.unwrap_or(typeface.semibold));

    let fallback = |fonts: &FontDefinitions, family: FontFamily| fonts.families.get(&family).cloned().unwrap_or_default();
    let proportional = fallback(&fonts, FontFamily::Proportional);
    let monospace = fallback(&fonts, FontFamily::Monospace);
    let with = |first: &str, rest: &[String]| [first.to_string()].into_iter().chain(rest.iter().cloned()).collect::<Vec<_>>();

    fonts.families.insert(FontFamily::Proportional, with("text", &proportional));
    fonts.families.insert(FontFamily::Monospace, with("mono", &monospace));
    fonts.families.insert(FontFamily::Name(SEMIBOLD.into()), with("semibold", &proportional));
    fonts.families.insert(FontFamily::Name(DISPLAY.into()), with("display", &proportional));
    ctx.set_fonts(fonts);
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

        assert_eq!(style.spacing.window_margin, Margin::same(12));
        assert_eq!(style.visuals.window_corner_radius, CornerRadius::same(6));
        assert_eq!(style.visuals.panel_fill, Color32::BLACK);
        assert_eq!(style.text_styles[&TextStyle::Body].size, 13.5);
        assert_eq!(ctx.theme(), egui::Theme::Dark);
    }
}
