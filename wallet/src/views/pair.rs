//! An offline signer's page: the account in use, as a code for the watch-only sender to read, so it
//! can prove what the account holds and ask for what to sign. The code holds public keys alone:
//! nothing in it can spend.

use {
    super::Ctx,
    crate::airgap::{Message, Pairing},
    crate::theme,
    egui::{RichText, Ui},
};

pub fn show(ui: &mut Ui, ctx: &mut Ctx) {
    let palette = *ctx.palette;
    theme::title(ui, &palette, "Pair", "The account in use, for the watch-only sender to follow and ask signatures of.");

    let Some(holder) = ctx.holder() else {
        return;
    };
    let pairing = Pairing {
        name: holder.name.clone(),
        keys: holder.publics.clone(),
    };
    let addresses = holder.addresses();
    let text = Message::Pair(pairing).to_text();

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, &holder.name);
        ui.label(
            RichText::new("On the sender, choose Pair with a signer, or Accounts and Add from the signer, and read this code. It holds the account's public keys alone.")
                .color(palette.soft),
        );
        ui.add_space(10.0);
        super::code(ui, &palette, &text);
        ui.add_space(10.0);

        for address in &addresses {
            ui.horizontal(|ui| {
                ui.add_sized([64.0, 18.0], egui::Label::new(RichText::new(super::scheme(address)).small().color(palette.muted)));
                super::address(ui, &palette, &address.to_string());
            });
        }

        ui.add_space(6.0);
        ui.label(RichText::new("Another account pairs on its own: pick it as the one in use, and read its code too.").small().color(palette.muted));
    });
}
