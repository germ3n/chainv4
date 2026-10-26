//! The account at a glance: its coin, all its addresses together, and then each address with what
//! it holds of the coin and of each listed token, and what happened lately.

use {
    super::{Ctx, Page},
    crate::format,
    crate::theme,
    chainv4::db::address_key,
    chainv4::structs::address::Address,
    chainv4::structs::hex::to_hex,
    chainv4::structs::u256::u256,
    egui::{RichText, Ui},
};

pub fn show(ui: &mut Ui, ctx: &mut Ctx) {
    let palette = *ctx.palette;
    let Some(holder) = ctx.holder() else {
        return;
    };
    let (name, addresses) = (holder.name.clone(), holder.addresses());

    theme::title(ui, &palette, &name, "What this account holds at each of its addresses, every amount proven by the light client.");

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, &ctx.coin.symbol);
        ui.add_space(4.0);
        let proven: Vec<_> = addresses.iter().filter_map(|address| ctx.chain.account(&address.to_string())).collect();

        match proven.len() == addresses.len() {
            true => {
                let raw = proven.iter().fold(u256::ZERO, |sum, proven| sum.saturating_add(proven.value.balance)).to_string();
                let shown = format!("{} {}", format::compact(&raw, ctx.coin.decimals, 6), ctx.coin.symbol);
                theme::fitted(ui, &shown, 40.0, 18.0, egui::FontFamily::Name(theme::SERIF.into()), palette.ink)
                    .on_hover_text(format::amount(&raw, ctx.coin.decimals, &ctx.coin.symbol));

                if addresses.len() > 1 {
                    let text = match proven.iter().all(|proven| proven.assurance.final_) {
                        true => format!("across its {} addresses, all final", addresses.len()),
                        false => format!("across its {} addresses, proven", addresses.len()),
                    };
                    ui.label(RichText::new(text).small().color(palette.muted));
                }
            }
            false => super::waiting(ui, &palette, "Proving the balance…"),
        }

        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if theme::primary(ui, &palette, "Send", true).clicked() {
                *ctx.page = Page::Send;
            }

            if theme::quiet(ui, &palette, "Receive").clicked() {
                *ctx.page = Page::Receive;
            }
        });
    });

    for address in &addresses {
        ui.add_space(14.0);
        held_at(ui, ctx, address);
    }

    ui.add_space(14.0);
    theme::card(ui, &palette, |ui| {
        ui.horizontal(|ui| {
            theme::caps(ui, &palette, "Lately");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.link("All activity").clicked() {
                    *ctx.page = Page::Activity;
                }
            });
        });
        ui.add_space(6.0);
        super::activity::list(ui, ctx, 5);
    });
}

/// One address's card: the coin it holds, and each listed token it holds any of.
fn held_at(ui: &mut Ui, ctx: &Ctx, address: &Address) {
    let palette = *ctx.palette;
    let text = address.to_string();
    let key = to_hex(&address_key(address));

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, super::scheme(address));

        // The name it goes by, proven: it still resolves to this address.
        if let Some(name) = ctx.chain.primary(&text) {
            ui.label(RichText::new(name).color(palette.ink).size(17.0)).on_hover_text("This address's primary name, proven to resolve to it");
        }

        super::address(ui, &palette, &text);
        ui.add_space(6.0);

        match ctx.chain.account(&text) {
            Some(proven) => {
                let raw = proven.value.balance.to_string();
                held(ui, ctx, &ctx.coin.symbol, &format!("the coin · nonce {}", proven.value.nonce), None, &raw, &proven.assurance);

                if !proven.assurance.final_
                    && let Some(final_) = ctx.chain.final_account(&text)
                    && final_.value.balance != proven.value.balance
                {
                    ui.label(
                        RichText::new(format!(
                            "{} at the final block {}",
                            format::amount(&final_.value.balance.to_string(), ctx.coin.decimals, &ctx.coin.symbol),
                            final_.assurance.height
                        ))
                        .small()
                        .color(palette.muted),
                    );
                }
            }
            None => super::waiting(ui, &palette, "Proving the balance…"),
        }

        if let Some(why) = ctx.chain.refused.get(&key) {
            ui.colored_label(palette.bad, format!("The node's proof did not check: {why}"));
        }

        for token in &ctx.chain.listed {
            let Some(proven) = ctx.chain.balance(token, &text) else {
                continue;
            };
            let raw = proven.value.to_string();

            if raw == "0" {
                continue;
            }

            let (symbol, _) = ctx.token(token);
            let name = ctx.chain.tokens.get(token).and_then(|proven| proven.value.name.clone()).unwrap_or_else(|| format::short(token));
            theme::rule(ui, &palette);
            held(ui, ctx, &symbol, &name, Some(token), &raw, &proven.assurance);
        }
    });
}

/// A line of what an address holds: the symbol and a word under it, and the amount with how far it
/// is trusted.
fn held(ui: &mut Ui, ctx: &Ctx, symbol: &str, under: &str, token: Option<&str>, raw: &str, assurance: &chainv4::light::Assurance) {
    let palette = *ctx.palette;
    let decimals = match token {
        None => ctx.coin.decimals,
        Some(token) => ctx.token(token).1,
    };
    // The name keeps a third of the row; the amount shrinks into the rest beside its pill.
    let third = ui.available_width() / 3.0;

    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.set_max_width(third);
            ui.add(egui::Label::new(theme::strong(symbol, 15.0).color(palette.ink)).truncate());
            let under = ui.add(egui::Label::new(RichText::new(under).small().color(palette.muted)).truncate());

            if let Some(token) = token {
                under.on_hover_text(token);
            }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            super::assurance(ui, &palette, assurance);
            theme::fitted(ui, &format::compact(raw, decimals, 6), 15.0, 10.0, egui::FontFamily::Name(theme::SEMIBOLD.into()), palette.ink)
                .on_hover_text(format::amount(raw, decimals, symbol));
        });
    });
}
