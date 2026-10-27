//! An offline signer's page: the account in use, as a code for the watch-only sender to read, so it
//! can prove what the account holds and ask for what to sign; and the wallet's Ethereum accounts,
//! as a code MetaMask pairs with, as it would a Keystone, to use the signer as its QR hardware
//! wallet; and the wallet's Bitcoin account, as a code a Bitcoin wallet such as Sparrow or
//! BlueWallet pairs with the same way, on Bitcoin or its testnet. The codes hold public keys
//! alone: nothing in them can spend.

use {
    super::Ctx,
    crate::airgap::{Message, Pairing},
    crate::bitcoin::Network,
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
    let message = Message::Pair(pairing);

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, &holder.name);
        ui.label(
            RichText::new("On the sender, choose Pair with a signer, or Accounts and Add from the signer, and read this code. It holds the account's public keys alone.")
                .color(palette.soft),
        );
        ui.add_space(10.0);
        super::code(ui, &palette, &message);
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

    ui.add_space(14.0);
    metamask(ui, ctx);
    ui.add_space(14.0);
    bitcoin(ui, ctx);
}

/// The wallet's Ethereum accounts, for MetaMask: the extended key at m/44'/60'/0', which every
/// account the words make derives below.
fn metamask(ui: &mut Ui, ctx: &mut Ctx) {
    let palette = *ctx.palette;
    let id = egui::Id::unique("metamask pairing shown");
    let shown = ui.data(|data| data.get_temp::<bool>(id)).unwrap_or(false);

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "MetaMask");
        ui.label(
            RichText::new("Use this signer as MetaMask's QR hardware wallet. In MetaMask: Add account or hardware wallet, Add hardware wallet, QR-based, and read this code; then tick the accounts to add. Each send and signature it asks for, read here on Sign.")
                .color(palette.soft),
        );
        ui.add_space(8.0);

        let Some(seed) = crate::keys::seed(ctx.book) else {
            ui.label(RichText::new("This wallet holds no words, only keys brought in: MetaMask pairs with the accounts words make.").small().color(palette.muted));

            return;
        };

        if !shown {
            ui.label(RichText::new("The code shows every Ethereum address these words make, but no key.").small().color(palette.muted));

            if theme::primary(ui, &palette, "Show MetaMask's code", true).clicked() {
                ui.data_mut(|data| data.insert_temp(id, true));
            }

            return;
        }

        match crate::hardware::pairing(&seed) {
            Ok(code) => {
                let side = ui.available_width().clamp(200.0, 420.0);
                ui.vertical_centered(|ui| super::receive::qr(ui, &code, side));
            }
            Err(why) => {
                ui.colored_label(palette.bad, why);
            }
        }

        ui.add_space(6.0);
        let account = seed.ethereum_account();
        ui.label(
            RichText::new(format!(
                "m/44'/60'/0', fingerprint {}: MetaMask lists account n at 0/n, as this wallet derives it.",
                chainv4::structs::hex::to_hex(&account.master_fingerprint).trim_start_matches("0x")
            ))
            .small()
            .color(palette.muted),
        );

        if theme::quiet(ui, &palette, "Hide").clicked() {
            ui.data_mut(|data| data.insert_temp(id, false));
        }
    });
}

/// The wallet's Bitcoin account, for a Bitcoin wallet: the extended key at m/84'/0'/0', below
/// which every account's bc1q address derives, or at m/84'/1'/0' for Bitcoin's test networks.
fn bitcoin(ui: &mut Ui, ctx: &mut Ctx) {
    let palette = *ctx.palette;
    let (shown_id, network_id) = (egui::Id::unique("bitcoin pairing shown"), egui::Id::unique("bitcoin pairing network"));
    let shown = ui.data(|data| data.get_temp::<bool>(shown_id)).unwrap_or(false);
    let mut network = ui.data(|data| data.get_temp::<Network>(network_id)).unwrap_or_default();

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Bitcoin");
        ui.label(
            RichText::new("Use this signer as a Bitcoin wallet's QR hardware wallet: Sparrow, BlueWallet, Nunchuk or another that pairs with an airgapped one by its account. In it, add a QR or airgapped hardware wallet and read this code; each transaction it asks to sign, read here on Sign.")
                .color(palette.soft),
        );
        ui.add_space(8.0);

        let Some(seed) = crate::keys::seed(ctx.book) else {
            ui.label(RichText::new("This wallet holds no words, only keys brought in: a Bitcoin wallet pairs with the account words make.").small().color(palette.muted));

            return;
        };

        ui.horizontal(|ui| {
            for each in Network::ALL {
                ui.selectable_value(&mut network, each, each.short());
            }
        });
        ui.data_mut(|data| data.insert_temp(network_id, network));
        ui.add_space(6.0);

        if !shown {
            ui.label(RichText::new("The code shows every Bitcoin address these words make on the network, but no key.").small().color(palette.muted));

            if theme::primary(ui, &palette, "Show the Bitcoin code", true).clicked() {
                ui.data_mut(|data| data.insert_temp(shown_id, true));
            }

            return;
        }

        theme::fitted(ui, network.name(), 30.0, 16.0, egui::FontFamily::Name(theme::SERIF.into()), palette.ink);

        if network == Network::Test {
            ui.colored_label(palette.warn, "For testnet, signet and regtest: pair a wallet set to a test network, as their keys are others.");
        }

        match crate::bitcoin::pairing(&seed, network) {
            Ok(code) => {
                let side = ui.available_width().clamp(200.0, 420.0);
                ui.vertical_centered(|ui| super::receive::qr(ui, &code, side));
            }
            Err(why) => {
                ui.colored_label(palette.bad, why);
            }
        }

        ui.add_space(6.0);
        ui.label(
            RichText::new(format!(
                "{}, fingerprint {}: native segwit. Its first address should read {}.",
                network.path(),
                chainv4::structs::hex::to_hex(&seed.bitcoin_account(network.coin()).master_fingerprint).trim_start_matches("0x"),
                crate::bitcoin::receiving(&seed, network, 0),
            ))
            .small()
            .color(palette.muted),
        );

        if theme::quiet(ui, &palette, "Hide").clicked() {
            ui.data_mut(|data| data.insert_temp(shown_id, false));
        }
    });
}
