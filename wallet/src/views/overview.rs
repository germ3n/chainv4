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

#[cfg_attr(not(all(feature = "bitcoin-light", not(target_arch = "wasm32"))), allow(unused_variables))]
pub fn show(ui: &mut Ui, ctx: &mut Ctx, send: &mut super::send::SendState) {
    let palette = *ctx.palette;
    let Some(holder) = ctx.holder() else {
        return;
    };
    let (name, addresses) = (holder.name.clone(), holder.addresses());

    theme::title(ui, &palette, &name, "What this account holds at each of its addresses, every amount proven by the light client.");

    // Where there is no sidebar to say it, or no room in one.
    let no_sidebar = chainv4_ui::layout::is_narrow(ui.ctx().content_rect().width()) || chainv4_ui::layout::is_short(ui.ctx());

    if no_sidebar && let Some(said) = super::fast_resumed(ctx.chain) {
        ui.add(egui::Label::new(RichText::new(said).small().color(palette.warn)).wrap());
        theme::section_gap(ui);
    }

    // An Ethereum key's account that EIP-7702 delegated runs a contract's code when called: say so,
    // as proven, since a call to it, or what it signs, may then do what that code says.
    for address in &addresses {
        if let Some(proven) = ctx.chain.delegations.get(&address.to_string())
            && let Some(to) = proven.value
        {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(format!("{} runs the code of", format::short(&address.to_string()))).small().color(palette.warn));
                super::address(ui, &palette, &to.to_string());
                ui.label(RichText::new("as its own, by EIP-7702").small().color(palette.warn));
                super::assurance(ui, &palette, &proven.assurance);
            });
            theme::section_gap(ui);
        }
    }

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
        ui.horizontal_wrapped(|ui| {
            if theme::primary(ui, &palette, "Send", true).clicked() {
                *ctx.page = Page::Send;
            }

            if theme::quiet(ui, &palette, "Receive").clicked() {
                *ctx.page = Page::Receive;
            }
        });
    });

    for address in &addresses {
        held_at(ui, ctx, address);
    }

    #[cfg(all(feature = "bitcoin-light", not(target_arch = "wasm32")))]
    bitcoin(ui, ctx, send);

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

    ui.horizontal_wrapped(|ui| {
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

/// What this account holds on Bitcoin, as the light client found it, on each network it follows:
/// its balance, confirmed in blocks, and its latest coins.
#[cfg(all(feature = "bitcoin-light", not(target_arch = "wasm32")))]
fn bitcoin(ui: &mut Ui, ctx: &mut Ctx, send: &mut super::send::SendState) {
    let palette = *ctx.palette;
    let Some(holder) = ctx.holder() else {
        return;
    };

    let mut go = None;

    for testnet in [false, true] {
        let Some(light) = ctx.lights.on(testnet) else {
            continue;
        };
        let scripts = crate::light::account_scripts(std::slice::from_ref(holder), ctx.book, testnet);

        if scripts.is_empty() {
            continue;
        }

        let mut coins: Vec<crate::light::Coin> = scripts.iter().flat_map(|script| light.coins(script)).collect();
        coins.sort_by_key(|coin| std::cmp::Reverse(coin.height));
        let held: u64 = coins.iter().filter(|coin| coin.spent.is_none()).map(|coin| coin.value).sum();
        let symbol = if testnet { "tBTC" } else { "BTC" };
        let status = light.status();

        theme::card(ui, &palette, |ui| {
            theme::caps(ui, &palette, &format!("Bitcoin{}", if testnet { format!(" · {}", light.net.name()) } else { String::new() }));
            ui.add_space(4.0);
            theme::fitted(ui, &format!("{} {symbol}", format::units(&held.to_string(), 8)), 32.0, 16.0, egui::FontFamily::Name(theme::SERIF.into()), palette.ink);
            ui.label(RichText::new(match status.synced {
                true => format!("confirmed, to block {}", light.tip()),
                false => format!("so far: syncing, at block {} of {}", light.seen().height, status.headers),
            }).small().color(palette.muted));

            // What the peers gossiped, not yet in a block: shown, never counted.
            for pending in light.pending(&scripts) {
                let paid: u64 = pending.pays.iter().map(|(_, value, _)| value).sum();
                let line = match (paid, pending.spends.len()) {
                    (0, spends) => format!("a spend of {spends} coin{} · pending, not in a block yet", if spends == 1 { "" } else { "s" }),
                    (paid, _) => format!("+{} {symbol} · pending, not in a block yet", format::units(&paid.to_string(), 8)),
                };
                ui.label(RichText::new(line).color(palette.muted)).on_hover_text(format!(
                    "{}: heard from a peer's mempool. It can still be replaced or never mined, so it counts once it is in a block.",
                    pending.txid
                ));
            }

            for coin in coins.iter().take(5) {
                let depth = light.confirmations(coin.height);
                let line = format!(
                    "+{} {symbol} · {} confirmation{}{}",
                    format::units(&coin.value.to_string(), 8),
                    depth,
                    if depth == 1 { "" } else { "s" },
                    if coin.spent.is_some() { " · spent" } else { "" }
                );
                ui.label(RichText::new(line).color(if coin.spent.is_some() { palette.muted } else { palette.ink }));
            }

            ui.add_space(8.0);
            if theme::quiet(ui, &palette, &format!("Send {symbol}")).clicked() {
                go = Some(testnet);
            }
        });
    }

    if let Some(testnet) = go {
        send.send_bitcoin(testnet);
        *ctx.page = Page::Send;
    }
}

