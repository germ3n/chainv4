//! The wallet's look: the wiki's palette and type, dark or light, and the few pieces every page is
//! built from. Manrope for text, JetBrains Mono for addresses and hashes, and Newsreader only for
//! page titles, so it reads calmer than the wiki's pages.

use egui::{Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Frame, Margin, Response, RichText, Stroke, TextStyle, Ui};
use std::sync::Arc;

/// One set of colors, as the wiki names them.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub paper: Color32,
    pub nav: Color32,
    pub card: Color32,
    pub row: Color32,
    pub line: Color32,
    pub ink: Color32,
    pub soft: Color32,
    pub muted: Color32,
    pub heading: Color32,
    pub accent: Color32,
    /// What sits on the accent: the paper's own color.
    pub on_accent: Color32,
    pub good: Color32,
    pub warn: Color32,
    pub bad: Color32,
    pub dark: bool,
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

/// The families the pages ask for besides proportional and monospace.
pub const SEMIBOLD: &str = "semibold";
pub const SERIF: &str = "serif";

/// Loads the wiki's fonts, each falling back to egui's own for anything it lacks.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let mut add = |name: &str, bytes: &'static [u8]| {
        fonts.font_data.insert(name.to_string(), Arc::new(FontData::from_static(bytes)));
    };

    add("manrope", include_bytes!("../assets/fonts/Manrope-Regular.ttf"));
    add("manrope-semibold", include_bytes!("../assets/fonts/Manrope-Semibold.ttf"));
    add("jetbrains", include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf"));
    add("newsreader", include_bytes!("../assets/fonts/Newsreader-Regular.ttf"));

    let fallback = |fonts: &FontDefinitions, family: FontFamily| fonts.families.get(&family).cloned().unwrap_or_default();
    let proportional = fallback(&fonts, FontFamily::Proportional);
    let monospace = fallback(&fonts, FontFamily::Monospace);
    let with = |first: &str, rest: &[String]| [first.to_string()].into_iter().chain(rest.iter().cloned()).collect::<Vec<_>>();

    fonts.families.insert(FontFamily::Proportional, with("manrope", &proportional));
    fonts.families.insert(FontFamily::Monospace, with("jetbrains", &monospace));
    fonts.families.insert(FontFamily::Name(SEMIBOLD.into()), with("manrope-semibold", &proportional));
    fonts.families.insert(FontFamily::Name(SERIF.into()), with("newsreader", &proportional));
    ctx.set_fonts(fonts);
}

/// Dresses egui in a palette: its panels, widgets, text sizes and spacing. The palette picks the
/// theme, so the system's own preference never swaps it for egui's.
pub fn apply(ctx: &egui::Context, palette: &Palette) {
    let theme = if palette.dark { egui::Theme::Dark } else { egui::Theme::Light };
    let mut style = (*ctx.style_of(theme)).clone();
    let mut visuals = if palette.dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    visuals.panel_fill = palette.paper;
    visuals.window_fill = palette.card;
    visuals.extreme_bg_color = palette.row;
    visuals.faint_bg_color = palette.row;
    visuals.override_text_color = Some(palette.ink);
    visuals.hyperlink_color = palette.accent;
    visuals.selection.bg_fill = palette.accent.gamma_multiply(0.35);
    visuals.selection.stroke = Stroke::new(1.0, palette.accent);
    visuals.window_stroke = Stroke::new(1.0, palette.line);
    visuals.window_corner_radius = CornerRadius::same(10);

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
        widget.corner_radius = CornerRadius::same(8);
        widget.fg_stroke.color = palette.ink;
    }

    visuals.widgets.noninteractive.fg_stroke.color = palette.soft;
    style.visuals = visuals;
    style.spacing.item_spacing = egui::vec2(10.0, 8.0);
    style.spacing.button_padding = egui::vec2(14.0, 7.0);
    style.spacing.interact_size.y = 30.0;
    style.text_styles = [
        (TextStyle::Small, FontId::proportional(11.5)),
        (TextStyle::Body, FontId::proportional(14.5)),
        (TextStyle::Button, FontId::proportional(14.0)),
        (TextStyle::Monospace, FontId::monospace(13.0)),
        (TextStyle::Heading, FontId::new(26.0, FontFamily::Name(SERIF.into()))),
    ]
    .into();
    ctx.set_style_of(theme, style);
    ctx.set_theme(theme);
}

/// A page's title, in the wiki's serif, with a line under it saying what the page is for.
pub fn title(ui: &mut Ui, palette: &Palette, title: &str, lead: &str) {
    ui.label(RichText::new(title).font(FontId::new(30.0, FontFamily::Name(SERIF.into()))).color(palette.ink));
    ui.add_space(2.0);
    ui.label(RichText::new(lead).color(palette.muted));
    ui.add_space(16.0);
}

/// A small label in capitals, spaced out, as the wiki heads its sections.
pub fn caps(ui: &mut Ui, palette: &Palette, text: &str) {
    let mut job = egui::text::LayoutJob::default();
    job.append(&text.to_uppercase(), 0.0, egui::TextFormat {
        font_id: FontId::new(10.5, FontFamily::Name(SEMIBOLD.into())),
        color: palette.heading,
        extra_letter_spacing: 1.6,
        ..Default::default()
    });
    ui.label(job);
}

/// Text in the semibold face.
pub fn strong(text: impl Into<String>, size: f32) -> RichText {
    RichText::new(text.into()).font(FontId::new(size, FontFamily::Name(SEMIBOLD.into())))
}

/// Text in the monospace face, for addresses and hashes.
pub fn mono(text: impl Into<String>) -> RichText {
    RichText::new(text.into()).monospace()
}

/// A card: what each part of a page sits in.
pub fn card<R>(ui: &mut Ui, palette: &Palette, add: impl FnOnce(&mut Ui) -> R) -> R {
    Frame::new()
        .fill(palette.card)
        .stroke(Stroke::new(1.0, palette.line))
        .corner_radius(12.0)
        .inner_margin(Margin::same(20))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// The one button a step leads to, filled with the accent.
pub fn primary(ui: &mut Ui, palette: &Palette, text: &str, enabled: bool) -> Response {
    let button = egui::Button::new(strong(text, 14.0).color(if enabled { palette.on_accent } else { palette.muted }))
        .fill(if enabled { palette.accent } else { palette.row })
        .stroke(Stroke::NONE)
        .corner_radius(8.0)
        .min_size(egui::vec2(120.0, 34.0));

    ui.add_enabled(enabled, button)
}

/// A quiet button, outlined.
pub fn quiet(ui: &mut Ui, palette: &Palette, text: &str) -> Response {
    ui.add(
        egui::Button::new(RichText::new(text).color(palette.soft))
            .fill(Color32::TRANSPARENT)
            .stroke(Stroke::new(1.0, palette.line))
            .corner_radius(8.0),
    )
}

/// A rounded tag in a color: what the light client says about a value.
pub fn pill(ui: &mut Ui, text: &str, color: Color32) -> Response {
    Frame::new()
        .fill(color.gamma_multiply(0.14))
        .stroke(Stroke::new(1.0, color.gamma_multiply(0.6)))
        .corner_radius(99.0)
        .inner_margin(Margin::symmetric(9, 2))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(11.5).color(color));
        })
        .response
}

/// A small dot in a color, before a status.
pub fn dot(ui: &mut Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, color);
}

/// A thin line across a card, between its parts.
pub fn rule(ui: &mut Ui, palette: &Palette) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 1.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 0.0, palette.line);
}
