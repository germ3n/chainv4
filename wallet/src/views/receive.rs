//! Where to send to: the account's address, written out and as a QR code to scan.

use {
    super::Ctx,
    crate::theme,
    egui::{Color32, RichText, Ui},
    qrcode::{Color, EcLevel, QrCode},
};

/// The quiet border around the code, in modules, as scanners want it.
const QUIET: usize = 3;

pub fn show(ui: &mut Ui, ctx: &mut Ctx) {
    let palette = *ctx.palette;
    let Some(holder) = ctx.holder() else {
        return;
    };
    let (name, address) = (holder.name.clone(), holder.address.to_string());

    theme::title(ui, &palette, "Receive", "Share this address to be paid. What comes in shows once the light client proves it.");

    theme::card(ui, &palette, |ui| {
        ui.vertical_centered(|ui| {
            theme::caps(ui, &palette, &name);
            ui.add_space(10.0);
            qr(ui, &address, 232.0);
            ui.add_space(14.0);
            ui.label(theme::mono(&address).color(palette.ink).size(13.5));
            ui.add_space(8.0);

            if theme::primary(ui, &palette, "Copy address", true).clicked() {
                ui.ctx().copy_text(address.clone());
            }
        });
    });

    ui.add_space(14.0);
    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Good to know");
        ui.add_space(4.0);

        for line in [
            "The coin and every token the chain's registry lists arrive at this same address.",
            "A payment counts once the light client proves it, and cannot be undone once its block is final.",
            "Coins a contract pays out show in the balance; the activity lists them when the node keeps an address index.",
        ] {
            ui.label(RichText::new(line).color(palette.soft));
        }
    });
}

/// The text as a QR code this wide, dark on white whatever the theme, so any camera reads it.
fn qr(ui: &mut Ui, text: &str, side: f32) {
    let Ok(code) = QrCode::with_error_correction_level(text.as_bytes(), EcLevel::M) else {
        ui.label("This address does not fit a QR code.");

        return;
    };
    let width = code.width();
    let modules = code.to_colors();
    let cells = width + 2 * QUIET;
    let cell = (side / cells as f32).floor().max(1.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(cell * cells as f32, cell * cells as f32), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 10.0, Color32::WHITE);

    for (index, module) in modules.iter().enumerate() {
        if *module == Color::Dark {
            let (x, y) = ((index % width + QUIET) as f32, (index / width + QUIET) as f32);
            let min = rect.min + egui::vec2(x * cell, y * cell);
            painter.rect_filled(egui::Rect::from_min_size(min, egui::vec2(cell, cell)), 0.0, Color32::from_rgb(0x15, 0x12, 0x0c));
        }
    }
}
