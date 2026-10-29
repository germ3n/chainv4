//! The pieces pages are built from. Each takes its colors from a palette, and its sizes from the
//! style apply set: a card is roomy in a comfortable look and tight in a compact one.

use {
    crate::theme::{DISPLAY, Palette, SEMIBOLD},
    egui::{Color32, FontFamily, FontId, Frame, Margin, Response, RichText, Stroke, TextStyle, Ui},
};

/// A page's title, in the display face, with a line under it saying what the page is for.
pub fn title(ui: &mut Ui, palette: &Palette, title: &str, lead: &str) {
    let size = ui.style().text_styles.get(&TextStyle::Heading).map_or(26.0, |font| font.size) * 1.15;
    ui.label(RichText::new(title).font(FontId::new(size, FontFamily::Name(DISPLAY.into()))).color(palette.ink));

    if !lead.is_empty() {
        ui.add_space(2.0);
        ui.label(RichText::new(lead).color(palette.muted));
    }

    ui.add_space(size * 0.55);
}

/// A small label in capitals, spaced out: a section's heading.
pub fn caps(ui: &mut Ui, palette: &Palette, text: &str) -> Response {
    let mut job = egui::text::LayoutJob::default();
    job.append(&text.to_uppercase(), 0.0, egui::TextFormat {
        font_id: FontId::new(10.5, FontFamily::Name(SEMIBOLD.into())),
        color: palette.heading,
        extra_letter_spacing: 1.6,
        ..Default::default()
    });

    ui.label(job)
}

/// One line of text that shrinks to fit the width there is, down to smallest, and is cut short past
/// that: a row never grows past the screen, however large what it shows.
pub fn fitted(ui: &mut Ui, text: &str, size: f32, smallest: f32, family: FontFamily, color: Color32) -> Response {
    let width = ui.available_width().max(1.0);
    let natural = ui.fonts_mut(|fonts| fonts.layout_no_wrap(text.to_string(), FontId::new(size, family.clone()), color).size().x);
    let size = match natural > width {
        true => (size * width / natural).max(smallest),
        false => size,
    };

    ui.add(egui::Label::new(RichText::new(text).font(FontId::new(size, family)).color(color)).truncate())
}

/// Text in the semibold face.
pub fn strong(text: impl Into<String>, size: f32) -> RichText {
    RichText::new(text.into()).font(FontId::new(size, FontFamily::Name(SEMIBOLD.into())))
}

/// Text in the monospace face, for addresses, hashes and numbers to compare.
pub fn mono(text: impl Into<String>) -> RichText {
    RichText::new(text.into()).monospace()
}

/// A card: what each part of a page sits in, with the look's radius and margin.
pub fn card<R>(ui: &mut Ui, palette: &Palette, add: impl FnOnce(&mut Ui) -> R) -> R {
    let (radius, margin) = (ui.visuals().window_corner_radius, ui.spacing().window_margin);

    Frame::new()
        .fill(palette.card)
        .stroke(Stroke::new(1.0, palette.line))
        .corner_radius(radius)
        .inner_margin(margin)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// The one button a step leads to, filled with the accent.
pub fn primary(ui: &mut Ui, palette: &Palette, text: &str, enabled: bool) -> Response {
    let (size, height) = (button_size(ui), ui.spacing().interact_size.y);
    let button = egui::Button::new(strong(text, size).color(if enabled { palette.on_accent } else { palette.muted }))
        .fill(if enabled { palette.accent } else { palette.row })
        .stroke(Stroke::NONE)
        .corner_radius(ui.visuals().widgets.inactive.corner_radius)
        .min_size(egui::vec2(height * 4.0, height + 4.0));

    ui.add_enabled(enabled, button)
}

/// A quiet button, outlined.
pub fn quiet(ui: &mut Ui, palette: &Palette, text: &str) -> Response {
    ui.add(
        egui::Button::new(RichText::new(text).color(palette.soft))
            .fill(Color32::TRANSPARENT)
            .stroke(Stroke::new(1.0, palette.line))
            .corner_radius(ui.visuals().widgets.inactive.corner_radius),
    )
}

/// A rounded tag in a color: a status at a glance.
pub fn pill(ui: &mut Ui, text: &str, color: Color32) -> Response {
    let font = ui.style().text_styles.get(&TextStyle::Small).cloned().unwrap_or_else(|| egui::FontId::proportional(11.5));
    let size = font.size;
    // Whole, never a letter a line: in a row that wraps, one that would not fit starts the next line.
    let measured = ui.fonts_mut(|fonts| fonts.layout_no_wrap(text.to_string(), font, color).size());
    let width = measured.x + 20.0;

    if ui.layout().main_wrap && ui.available_size_before_wrap().x < width {
        ui.end_row();
    }

    // Its own size, however tall the row: in a row beside taller things it would stretch to their
    // height, a pill grown into a circle.
    let height = measured.y + 6.0;
    ui.allocate_ui_with_layout(egui::vec2(width, height), egui::Layout::left_to_right(egui::Align::Center), |ui| {
        Frame::new()
            .fill(color.gamma_multiply(0.14))
            .stroke(Stroke::new(1.0, color.gamma_multiply(0.6)))
            .corner_radius(99.0)
            .inner_margin(Margin::symmetric(9, 2))
            .show(ui, |ui| {
                ui.add(egui::Label::new(RichText::new(text).size(size).color(color)).extend());
            })
            .response
    })
    .inner
}

/// In a row that wraps, starts the next line unless buttons with these texts all fit on this one:
/// a choice of a few moves to the next line whole, rather than one of them left behind.
pub fn together(ui: &mut Ui, texts: &[&str]) {
    let font = TextStyle::Button.resolve(ui.style());
    let (padding, gap) = (ui.spacing().button_padding.x, ui.spacing().item_spacing.x);
    let width: f32 = texts
        .iter()
        .map(|text| ui.fonts_mut(|fonts| fonts.layout_no_wrap(text.to_string(), font.clone(), Color32::WHITE).size().x) + 2.0 * padding + gap)
        .sum();

    if ui.layout().main_wrap && ui.available_size_before_wrap().x < width {
        ui.end_row();
    }
}

/// A small dot in a color, before a status.
pub fn dot(ui: &mut Ui, color: Color32) -> Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, color);

    response
}

/// A thin line across, between the parts of a card.
pub fn rule(ui: &mut Ui, palette: &Palette) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 1.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 0.0, palette.line);
}

/// How long a copy says it was done, in seconds.
const COPIED_FOR: f64 = 1.6;

/// A label that copies some text when clicked or tapped, and says so beside it for a moment. The
/// label may show less than it copies: an address cut short copies whole.
pub fn copy_on_click(ui: &mut Ui, palette: &Palette, label: egui::Label, text: &str) -> Response {
    let response = ui.add(label.sense(egui::Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand);
    let id = response.id.with("copied");
    let now = ui.input(|input| input.time);

    if response.clicked() {
        ui.ctx().copy_text(text.to_string());
        ui.data_mut(|data| data.insert_temp(id, now));
    }

    if ui.data(|data| data.get_temp::<f64>(id)).is_some_and(|at| now - at < COPIED_FOR) {
        ui.label(RichText::new("Copied").small().color(palette.good));
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
    }

    response
}

/// A text field with a button on its right: the button takes the room it needs, and the field the
/// rest, however narrow the screen. Returns whether the button was pressed.
pub fn beside(ui: &mut Ui, button: impl FnOnce(&mut Ui) -> bool, field: impl FnOnce(&mut Ui, f32)) -> bool {
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let pressed = button(ui);
            field(ui, ui.available_width());

            pressed
        })
        .inner
    })
    .inner
}

/// Where a proxy is set, in an app's settings: a field with Use, which takes what is typed, then
/// Orbot, which takes Orbot's port on the device, I2P, i2pd's, and Off. Gives the setting to keep, empty to go
/// direct, once one is chosen that reads as a proxy; why one does not waits in trouble. set is the
/// setting in use. In the browser, which makes every request itself, it says where to set one.
pub fn proxy(ui: &mut Ui, palette: &Palette, typed: &mut String, trouble: &mut Option<String>, set: &str) -> Option<String> {
    let note = |ui: &mut Ui, text: String| ui.add(egui::Label::new(RichText::new(text).small().color(palette.muted)).wrap());

    if cfg!(target_arch = "wasm32") {
        note(ui, "In the browser, the browser makes every request, through the proxy set in it: to use Tor, open this page in Tor Browser.".to_string());

        return None;
    }

    let shown = crate::net::Proxy::parse(set).map(|proxy| proxy.shown().to_string());
    note(ui, match shown {
        Ok(shown) => format!("Every request goes through the proxy at {shown}, and only through it: while it is down, nothing is sent."),
        Err(_) => "None: requests go straight to the node. Set one to send them all through it, such as Orbot's or Tor Browser's to reach the node over Tor, as an .onion node needs, or I2P's for a .i2p node.".to_string(),
    });
    ui.add_space(4.0);

    let mut chosen = None;
    let other = typed.trim() != set;
    let used = beside(
        ui,
        |ui| primary(ui, palette, "Use", other).clicked(),
        |ui, width| {
            ui.add(egui::TextEdit::singleline(typed).hint_text(crate::net::Proxy::ORBOT).font(TextStyle::Monospace).desired_width(width));
        },
    );

    if used {
        chosen = Some(typed.clone());
    }

    ui.horizontal(|ui| {
        if quiet(ui, palette, "Orbot").on_hover_text("Orbot's SOCKS port on this device, which is Tor's own too").clicked() {
            chosen = Some(crate::net::Proxy::ORBOT.to_string());
        }

        if quiet(ui, palette, "Tor Browser").on_hover_text("Tor Browser's Tor, on this machine while it runs").clicked() {
            chosen = Some(crate::net::Proxy::TOR_BROWSER.to_string());
        }

        if quiet(ui, palette, "I2P")
            .on_hover_text("i2pd's SOCKS port on this device, reaching .i2p nodes; for the Java I2P router, type its HTTP proxy, http://127.0.0.1:4444")
            .clicked()
        {
            chosen = Some(crate::net::Proxy::I2P.to_string());
        }

        if !set.is_empty() && quiet(ui, palette, "Off").on_hover_text("Go straight to the node").clicked() {
            chosen = Some(String::new());
        }
    });

    if let Some(trouble) = trouble {
        ui.colored_label(palette.bad, trouble.as_str());
    }

    match crate::net::setting(&chosen?) {
        Ok(setting) => {
            *typed = setting.clone();
            *trouble = None;

            Some(setting)
        }
        Err(why) => {
            *trouble = Some(why);

            None
        }
    }
}

fn button_size(ui: &Ui) -> f32 {
    ui.style().text_styles.get(&TextStyle::Button).map_or(14.0, |font| font.size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pill_at_the_end_of_a_full_row_moves_to_the_next_line_whole() {
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(220.0, 400.0));
        let mut pills = Vec::new();

        // A frame to settle fonts, then the one measured.
        for _ in 0..2 {
            let mut output = ctx.run_ui(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label("Account 1 · Ed25519 ed25519:0x06b6…318c");
                    pills.push(pill(ui, "may vote", Color32::GREEN).rect);
                });
            });
            output.textures_delta.clear();
        }

        let rect = pills.last().copied().unwrap();
        let line = ctx.fonts_mut(|fonts| fonts.row_height(&egui::FontId::proportional(11.5)));

        assert!(rect.height() < 3.0 * line, "one line, not a letter a line: {rect:?}");
        assert!(rect.width() > rect.height(), "wider than tall: {rect:?}");
        assert!(rect.right() <= screen.right() + 1.0, "within the row: {rect:?}");
    }

    #[test]
    fn a_pill_beside_taller_things_keeps_its_own_height() {
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(380.0, 400.0));
        let (mut pill_rect, mut row) = (egui::Rect::NOTHING, egui::Rect::NOTHING);

        for _ in 0..2 {
            let mut output = ctx.run_ui(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |ui| {
                row = ui
                    .horizontal_wrapped(|ui| {
                        ui.vertical(|ui| {
                            ui.label("DEV");
                            ui.label("the coin");
                            ui.label("nonce 0");
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            pill_rect = pill(ui, "Proven", Color32::GOLD).rect;
                            ui.label("0");
                        });
                    })
                    .response
                    .rect;
            });
            output.textures_delta.clear();
        }

        let line = ctx.fonts_mut(|fonts| fonts.row_height(&egui::FontId::proportional(11.5)));

        assert!(row.height() > 3.0 * line, "the row is tall: {row:?}");
        assert!(pill_rect.height() < 2.0 * line, "the pill is not: {pill_rect:?}");
        assert!(pill_rect.width() > pill_rect.height(), "wider than tall: {pill_rect:?}");
    }

    #[test]
    fn a_choice_that_does_not_fit_after_its_label_moves_to_the_next_line_whole() {
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(260.0, 400.0));
        let (mut label, mut first, mut second) = (egui::Rect::NOTHING, egui::Rect::NOTHING, egui::Rect::NOTHING);

        for _ in 0..2 {
            let mut output = ctx.run_ui(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |ui| {
                ui.horizontal_wrapped(|ui| {
                    label = ui.label("Follow its test network:").rect;
                    together(ui, &["testnet4", "signet"]);
                    first = ui.selectable_label(true, "testnet4").rect;
                    second = ui.selectable_label(false, "signet").rect;
                });
            });
            output.textures_delta.clear();
        }

        assert!(first.top() > label.bottom() - 1.0, "below the label: {first:?} {label:?}");
        assert!((first.top() - second.top()).abs() < 1.0, "side by side: {first:?} {second:?}");
    }
}
