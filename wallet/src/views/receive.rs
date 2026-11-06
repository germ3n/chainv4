//! Where to send to: one of the account's addresses, picked by its scheme, written out and as a QR
//! code to scan, and all of them listed.

use {
    super::Ctx,
    crate::theme,
    chainv4::structs::address::Address,
    egui::{Color32, RichText, Ui},
    qrcode::{Color, EcLevel, QrCode},
};

/// An address's scheme, numbered when the account has more than one in it: Bitcoin, Bitcoin 2.
fn numbered(addresses: &[Address], index: usize) -> String {
    let scheme = super::scheme(&addresses[index]);
    let nth = addresses[..=index].iter().filter(|address| super::scheme(address) == scheme).count();

    match nth {
        1 => scheme.to_string(),
        nth => format!("{scheme} {nth}"),
    }
}

/// The quiet border around the code, in modules, as scanners want it.
const QUIET: usize = 3;

/// Which of the account's addresses the code shows, by its place among them.
#[derive(Default)]
pub struct ReceiveState {
    shown: usize,
    /// When the Copy button was last pressed, to say so on it for a moment.
    copied_at: Option<f64>,
}

pub fn show(ui: &mut Ui, ctx: &mut Ctx, state: &mut ReceiveState) {
    let palette = *ctx.palette;
    let Some(holder) = ctx.holder() else {
        return;
    };
    let (name, addresses) = (holder.name.clone(), holder.addresses());
    let holder_words = holder.number().is_some() && !holder.keys.is_empty();
    let mut new_bitcoin = false;
    state.shown = state.shown.min(addresses.len().saturating_sub(1));
    let Some(shown) = addresses.get(state.shown).map(Address::to_string) else {
        return;
    };

    theme::title(ui, &palette, "Receive", "Share an address to be paid. What comes in shows once the light client proves it.");

    theme::card(ui, &palette, |ui| {
        ui.vertical_centered(|ui| {
            theme::caps(ui, &palette, &name);

            if addresses.len() > 1 {
                ui.add_space(6.0);
                let names: Vec<String> = (0..addresses.len()).map(|index| numbered(&addresses, index)).collect();
                let options: Vec<(usize, &str)> = names.iter().enumerate().map(|(index, name)| (index, name.as_str())).collect();
                // A few fit a line; more than four take what rows they need.
                match options.len() {
                    0..=4 => theme::segmented_fit(ui, &palette, &mut state.shown, &options),
                    _ => theme::segmented_rows(ui, &palette, &mut state.shown, &options),
                };
            }

            ui.add_space(10.0);
            qr(ui, &shown, 232.0);
            ui.add_space(14.0);
            let label = egui::Label::new(theme::mono(&shown).color(palette.ink).size(13.5)).wrap();
            chainv4_ui::widgets::copy_on_click(ui, &palette, label, &shown).on_hover_text("Click to copy");
            ui.add_space(8.0);
            let now = ui.input(|input| input.time);
            let copied = state.copied_at.is_some_and(|at| now - at < 1.6);

            if theme::primary(ui, &palette, if copied { "Copied" } else { "Copy address" }, true).clicked() {
                ui.ctx().copy_text(shown.clone());
                state.copied_at = Some(now);
            }

            if copied {
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
            }

            // Bitcoin's habit: a new address to each payer, from the words, one tap at a time.
            let bitcoin = matches!(addresses.get(state.shown), Some(Address::Bitcoin(_)));

            if bitcoin && holder_words {
                ui.add_space(6.0);

                if theme::quiet(ui, &palette, "New address").on_hover_text("The next Bitcoin address from the words, kept with this account").clicked() {
                    new_bitcoin = true;
                }
            }
        });
    });

    if new_bitcoin
        && let Some(address) = super::accounts::add_address(ctx, ctx.current, chainv4::crypto::keypair::KeyType::Bitcoin)
        && let Some(holder) = ctx.holder()
    {
        state.shown = holder.addresses().iter().position(|shown| *shown == address).unwrap_or(state.shown);
    }

    if addresses.len() > 1 {
        theme::card(ui, &palette, |ui| {
            theme::caps(ui, &palette, "All its addresses");
            ui.add_space(4.0);

            for (index, address) in addresses.iter().enumerate() {
                super::row(ui, &palette, &numbered(&addresses, index), |ui| super::address(ui, &palette, &address.to_string()));
            }
        });
    }

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Good to know");
        ui.add_space(4.0);

        let ethereum = addresses.iter().any(|address| matches!(address, Address::Secp256k1(_)));
        let bitcoin = addresses.iter().any(|address| matches!(address, Address::Bitcoin(_)));
        let lines = [
            Some("Each address holds its own coin and tokens. The coin and every token the chain's registry lists arrive at any of them."),
            ethereum.then_some("The Ethereum address is the one Ethereum wallets show for the same key, but what this chain sends to it lives on this chain alone."),
            bitcoin.then_some("The Bitcoin address is the one Bitcoin wallets show for the same words, but what this chain sends to it lives on this chain alone."),
            Some("A payment counts once the light client proves it, and cannot be undone once its block is final."),
            Some("Coins a contract pays out show in the balance; the activity lists them when the node keeps an address index."),
        ];

        for line in lines.into_iter().flatten() {
            ui.label(RichText::new(line).color(palette.soft));
        }
    });
}

/// The text as a QR code this wide, dark on white whatever the theme, so any camera reads it.
pub(super) fn qr(ui: &mut Ui, text: &str, side: f32) {
    let Ok(code) = QrCode::with_error_correction_level(text.as_bytes(), EcLevel::M) else {
        ui.label("This does not fit a QR code.");

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
