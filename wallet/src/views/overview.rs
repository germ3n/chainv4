//! The account at a glance: its coin, as proven at the tip and at the final block, what it holds
//! of each listed token, and what happened lately.

use {
    super::{Ctx, Page},
    crate::format,
    crate::theme,
    chainv4::db::address_key,
    chainv4::structs::hex::to_hex,
    egui::{RichText, Ui},
};

pub fn show(ui: &mut Ui, ctx: &mut Ctx) {
    let palette = *ctx.palette;
    let Some(holder) = ctx.holder() else {
        return;
    };
    let (name, address) = (holder.name.clone(), holder.address.to_string());
    let key = to_hex(&address_key(&holder.address));

    theme::title(ui, &palette, &name, "What this account holds, each amount proven by the light client.");

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, &ctx.coin.symbol);
        ui.add_space(4.0);

        match ctx.chain.account(&address) {
            Some(proven) => {
                let raw = proven.value.balance.to_string();
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format::brief(&raw, ctx.coin.decimals, 6))
                            .font(egui::FontId::new(40.0, egui::FontFamily::Name(theme::SERIF.into())))
                            .color(palette.ink),
                    )
                    .on_hover_text(format::amount(&raw, ctx.coin.decimals, &ctx.coin.symbol));
                    ui.label(RichText::new(&ctx.coin.symbol).size(18.0).color(palette.muted));
                });
                ui.horizontal(|ui| {
                    super::assurance(ui, &palette, &proven.assurance);
                    ui.label(RichText::new(format!("nonce {}", proven.value.nonce)).small().color(palette.muted));
                });

                if !proven.assurance.final_
                    && let Some(final_) = ctx.chain.final_account(&address)
                    && final_.value.balance != proven.value.balance
                {
                    ui.add_space(2.0);
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

    ui.add_space(14.0);
    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Tokens");
        ui.add_space(6.0);
        let mut shown = 0;

        for token in &ctx.chain.listed {
            let Some(proven) = ctx.chain.balance(token, &address) else {
                continue;
            };
            let raw = proven.value.to_string();

            if raw == "0" {
                continue;
            }

            let (symbol, decimals) = ctx.token(token);
            let name = ctx.chain.tokens.get(token).and_then(|proven| proven.value.name.clone()).unwrap_or_else(|| format::short(token));

            if shown > 0 {
                theme::rule(ui, &palette);
            }

            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(theme::strong(&symbol, 15.0).color(palette.ink));
                    ui.label(RichText::new(name).small().color(palette.muted)).on_hover_text(token);
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    super::assurance(ui, &palette, &proven.assurance);
                    ui.label(theme::strong(format::brief(&raw, decimals, 6), 15.0).color(palette.ink)).on_hover_text(format::amount(&raw, decimals, &symbol));
                });
            });
            shown += 1;
        }

        if shown == 0 {
            let text = match ctx.chain.caught_up() {
                true => "None of the listed tokens yet.",
                false => "Proving token balances once the light client catches up…",
            };
            ui.label(RichText::new(text).color(palette.muted));
        }
    });

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
