//! Sending the coin or a token. The wallet builds and signs the transaction itself, from what it
//! proved: the account's nonce and balance. The node only prices it, from a probe signed with no
//! fee offer, as `chainv4 send` does, and takes it in. Nothing is signed for real before the
//! review shows what it costs at most.

use {
    super::{Ctx, Page},
    crate::app::Sent,
    crate::format,
    crate::node::{Ask, Failure},
    crate::theme,
    chainv4::crypto::hasher::Hasher,
    chainv4::structs::address::Address,
    chainv4::structs::amount::Amount,
    chainv4::structs::hash::HashType,
    chainv4::structs::hex::to_hex,
    chainv4::structs::tx::{FeeOffer, SignedTx, Tx, TxCallArgumentType},
    chainv4::structs::u256::u256,
    egui::{RichText, Ui},
    serde_json::{Value, json},
};

/// How long a sent transaction counts as waiting, holding its nonce, before the next one may take
/// that nonce again, in milliseconds.
const PENDING_FOR: u64 = super::activity::UNSEEN_AFTER;

#[derive(Default)]
pub struct SendState {
    /// What to send: None for the coin, or a token's address.
    token: Option<String>,
    to: String,
    amount: String,
    /// What to add per gas for the producer, in whole units of the coin; empty for the node's
    /// choice.
    tip: String,
    step: Step,
}

/// A transaction built and not yet sent.
#[derive(Clone, Debug)]
struct Draft {
    from: Address,
    to: Address,
    tx: Tx,
    nonce: u64,
    /// What it sends, as shown: 1.5 DEV.
    what: String,
    /// What it moves of the coin, beside the fee: what the review checks the balance for.
    coin: u256,
}

#[derive(Debug, Default)]
enum Step {
    #[default]
    Edit,
    /// Asked the node to price the probe.
    Pricing { draft: Draft, ask: Ask },
    Review { draft: Draft, fee: FeeOffer },
    /// Submitted, signed with the fee.
    Sending { draft: Draft, ask: Ask },
    Sent { id: String },
    Failed(String),
}

impl SendState {
    /// Takes the node's answer to a pricing or a submission this page asked, and returns the
    /// transaction it sent, once the node took it, for the history to keep.
    pub fn answered(&mut self, ask: &Ask, result: &Result<Value, Failure>, now: u64) -> Option<Sent> {
        match (&self.step, result) {
            (Step::Pricing { draft, ask: asked }, Ok(value)) if asked == ask => {
                self.step = match offer(value) {
                    Ok(fee) => Step::Review { draft: draft.clone(), fee },
                    Err(why) => Step::Failed(why),
                };

                None
            }
            (Step::Sending { draft, ask: asked }, Ok(value)) if asked == ask => {
                let id = value["id"].as_str().unwrap_or_default().to_string();
                let sent = Sent {
                    id: id.clone(),
                    from: draft.from.to_string(),
                    to: draft.to.to_string(),
                    what: draft.what.clone(),
                    nonce: draft.nonce,
                    at: now,
                };
                self.step = Step::Sent { id };
                self.amount.clear();

                Some(sent)
            }
            (Step::Pricing { ask: asked, .. } | Step::Sending { ask: asked, .. }, Err(failure)) if asked == ask => {
                self.step = Step::Failed(match failure.status {
                    0 => format!("The node could not be reached: {}", failure.message),
                    _ => format!("The node refused it: {}", failure.message),
                });

                None
            }
            _ => None,
        }
    }
}

pub fn show(ui: &mut Ui, ctx: &mut Ctx, state: &mut SendState) {
    let palette = *ctx.palette;
    theme::title(ui, &palette, "Send", "The wallet signs it from what it proved; the node only prices it and passes it on.");

    if let Step::Review { draft, fee } = &state.step {
        let (draft, fee) = (draft.clone(), *fee);
        review(ui, ctx, state, draft, fee);

        return;
    }

    if let Step::Sent { id } = &state.step {
        let id = id.clone();
        sent(ui, ctx, state, &id);

        return;
    }

    let busy = matches!(state.step, Step::Pricing { .. } | Step::Sending { .. });
    let address = ctx.address();

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "What");
        let shown = match &state.token {
            None => ctx.coin.symbol.clone(),
            Some(token) => ctx.token(token).0,
        };
        egui::ComboBox::from_id_salt("asset").selected_text(shown).width(ui.available_width()).show_ui(ui, |ui| {
            ui.selectable_value(&mut state.token, None, format!("{} · the coin", ctx.coin.symbol));

            for token in &ctx.chain.listed {
                let (symbol, _) = ctx.token(token);
                let held = ctx.chain.balance(token, &address).map(|proven| proven.value.to_string());

                if held.as_deref().is_some_and(|held| held != "0") {
                    ui.selectable_value(&mut state.token, Some(token.clone()), format!("{symbol} · {}", format::short(token)));
                }
            }
        });

        let (balance, decimals, symbol) = available(ctx, state.token.as_deref(), &address);
        ui.label(
            RichText::new(match &balance {
                Some(raw) => format!("{} available, proven", format::amount(raw, decimals, &symbol)),
                None => "Proving what is available…".to_string(),
            })
            .small()
            .color(palette.muted),
        );

        ui.add_space(10.0);
        theme::caps(ui, &palette, "To");
        ui.add(egui::TextEdit::singleline(&mut state.to).hint_text("ed25519:0x…").font(egui::TextStyle::Monospace).desired_width(f32::INFINITY));

        if !state.to.trim().is_empty() && state.to.trim().parse::<Address>().is_err() {
            ui.colored_label(palette.bad, "Not an address: one starts with its kind, such as ed25519:0x, and has 32 bytes of hex.");
        }

        ui.add_space(10.0);
        theme::caps(ui, &palette, "Amount");
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut state.amount).hint_text("0.0").desired_width(ui.available_width() - 70.0));

            if let Some(raw) = &balance
                && state.token.is_some()
                && theme::quiet(ui, &palette, "Max").clicked()
            {
                state.amount = format::units(raw, decimals).replace(',', "");
            }
        });

        ui.add_space(10.0);
        egui::CollapsingHeader::new(RichText::new("Fee").color(palette.soft)).id_salt("fee").show(ui, |ui| {
            ui.label(RichText::new("The node offers a fee sure to cover it now. A tip per gas, in the coin, asks producers to take it sooner.").small().color(palette.muted));
            ui.add(egui::TextEdit::singleline(&mut state.tip).hint_text("tip per gas, such as 0.000000001").desired_width(f32::INFINITY));
        });

        ui.add_space(12.0);

        if let Step::Failed(why) = &state.step {
            ui.colored_label(palette.bad, why);
            ui.add_space(6.0);
        }

        ui.horizontal(|ui| {
            if theme::primary(ui, &palette, "Review", !busy).clicked() {
                state.step = match draft(ctx, state) {
                    Ok((draft, body)) => {
                        let ask = Ask::Offer(body);
                        ctx.node.ask(ask.clone());

                        Step::Pricing { draft, ask }
                    }
                    Err(why) => Step::Failed(why),
                };
            }

            if busy {
                ui.spinner();
                ui.label(RichText::new("Asking the node for a fee offer…").color(palette.muted));
            }
        });
    });
}

/// What can be sent, proven, with its decimals and symbol.
fn available(ctx: &Ctx, token: Option<&str>, address: &str) -> (Option<String>, u8, String) {
    match token {
        None => (
            ctx.chain.account(address).map(|proven| proven.value.balance.to_string()),
            ctx.coin.decimals,
            ctx.coin.symbol.clone(),
        ),
        Some(token) => {
            let (symbol, decimals) = ctx.token(token);

            (ctx.chain.balance(token, address).map(|proven| proven.value.to_string()), decimals, symbol)
        }
    }
}

/// The transaction the form asks for, and the body of POST /offer for its probe.
fn draft(ctx: &Ctx, state: &SendState) -> Result<(Draft, String), String> {
    let holder = ctx.holder().ok_or("there is no account to send from")?;
    let from = holder.address;
    let address = from.to_string();
    let account = ctx.chain.account(&address).ok_or("the account is not proven yet: wait for the light client")?;
    let to: Address = state.to.trim().parse().map_err(|_| "the address to send to does not read".to_string())?;
    let (balance, decimals, symbol) = available(ctx, state.token.as_deref(), &address);
    let units = format::parse_units(&state.amount, decimals).ok_or(format!("the amount does not read as {symbol}, with at most {decimals} decimals"))?;
    let units = u256::from_str_radix(&units, 10).map_err(|_| "the amount is too large".to_string())?;
    let held = balance.and_then(|raw| u256::from_str_radix(&raw, 10).ok()).ok_or("the balance is not proven yet")?;

    if units == u256::ZERO {
        return Err("send more than nothing".to_string());
    }

    if units > held {
        return Err(format!("that is more than the {} available", format::amount(&held.to_string(), decimals, &symbol)));
    }

    let (tx, coin) = match &state.token {
        None => (Tx::Transfer { from, to, amount: amount(units) }, units),
        Some(token) => {
            let token: Address = token.parse().map_err(|_| "the token's address does not read".to_string())?;
            let tx = Tx::ContractCall {
                to: token,
                func: Hasher::digest(HashType::Xxh3_64, b"transfer"),
                amount: Amount::U128(0),
                data: vec![TxCallArgumentType::Address(to), TxCallArgumentType::U256(units)],
            };

            (tx, u256::ZERO)
        }
    };
    let nonce = next_nonce(account.value.nonce, &ctx.settings.sent, &address, ctx.now);
    let probe = SignedTx::sign(tx.clone(), nonce, FeeOffer::NONE, &holder.key);
    let mut body = json!({ "tx": encode(&probe) });

    if !state.tip.trim().is_empty() {
        let tip = format::parse_units(&state.tip, ctx.coin.decimals).ok_or("the tip does not read as an amount of the coin")?;
        body["tip"] = tip.into();
    }

    let draft = Draft {
        from,
        to,
        tx,
        nonce,
        what: format::amount(&units.to_string(), decimals, &symbol),
        coin,
    };

    Ok((draft, body.to_string()))
}

/// What the transaction costs at most, and the button that signs and sends it.
fn review(ui: &mut Ui, ctx: &mut Ctx, state: &mut SendState, draft: Draft, fee: FeeOffer) {
    let palette = *ctx.palette;
    let most = u256::from(fee.gas_limit) * fee.max_price;
    let balance = ctx.chain.account(&draft.from.to_string()).map(|proven| proven.value.balance);
    let short = balance.is_some_and(|balance| balance < draft.coin.saturating_add(most));

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Review");
        ui.add_space(4.0);
        ui.label(RichText::new(&draft.what).font(egui::FontId::new(34.0, egui::FontFamily::Name(theme::SERIF.into()))).color(palette.ink));
        ui.add_space(8.0);
        super::row(ui, &palette, "From", |ui| super::address(ui, &palette, &draft.from.to_string()));
        super::row(ui, &palette, "To", |ui| super::address(ui, &palette, &draft.to.to_string()));
        super::row(ui, &palette, "Fee, at most", |ui| {
            ui.label(ctx.amount(None, &most.to_string()))
                .on_hover_text(format!("{} gas at up to {} per gas; what it does not use is refunded", fee.gas_limit, ctx.amount(None, &fee.max_price.to_string())));
        });

        if fee.tip != u256::ZERO {
            super::row(ui, &palette, "Tip per gas", |ui| {
                ui.label(ctx.amount(None, &fee.tip.to_string()));
            });
        }

        super::row(ui, &palette, "Nonce", |ui| {
            ui.label(draft.nonce.to_string());
        });

        if short {
            ui.add_space(6.0);
            ui.colored_label(palette.bad, "The balance does not cover this and its fee.");
        }

        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if theme::primary(ui, &palette, "Sign and send", !short).clicked()
                && let Some(holder) = ctx.holders.iter().find(|holder| holder.address == draft.from)
            {
                let signed = SignedTx::sign(draft.tx.clone(), draft.nonce, fee, &holder.key);
                let ask = Ask::Submit(json!({ "tx": encode(&signed) }).to_string());
                ctx.node.ask(ask.clone());
                state.step = Step::Sending { draft: draft.clone(), ask };
            }

            if theme::quiet(ui, &palette, "Back").clicked() {
                state.step = Step::Edit;
            }
        });
    });
}

/// What became of a transaction just sent.
fn sent(ui: &mut Ui, ctx: &mut Ctx, state: &mut SendState, id: &str) {
    let palette = *ctx.palette;

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Sent");
        ui.add_space(4.0);
        let (text, color) = match ctx.settings.checked.get(id) {
            Some(checked) if !checked.done => ("It landed, and failed: it used its nonce and paid its fee.".to_string(), palette.bad),
            Some(checked) if checked.final_ => (format!("Final at block {}.", checked.height), palette.good),
            Some(checked) => (format!("Proven in block {}, not final yet.", checked.height), palette.accent),
            None => ("The node took it. Waiting for a block to hold it…".to_string(), palette.warn),
        };
        ui.horizontal(|ui| {
            theme::dot(ui, color);
            ui.label(RichText::new(text).color(palette.ink));
        });
        ui.add_space(6.0);
        super::row(ui, &palette, "Transaction", |ui| super::address(ui, &palette, id));
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if theme::primary(ui, &palette, "Send another", true).clicked() {
                state.step = Step::Edit;
            }

            if theme::quiet(ui, &palette, "Activity").clicked() {
                *ctx.page = Page::Activity;
                state.step = Step::Edit;
            }
        });
    });
}

/// The nonce the next transaction takes: the proven one, past any this wallet sent that is still
/// waiting. One unseen for long is taken to be dropped, so its nonce is used again.
pub fn next_nonce(proven: u64, sent: &[Sent], from: &str, now: u64) -> u64 {
    sent.iter()
        .filter(|sent| sent.from == from && sent.nonce >= proven && now.saturating_sub(sent.at) < PENDING_FOR)
        .map(|sent| sent.nonce + 1)
        .max()
        .unwrap_or(proven)
        .max(proven)
}

/// The narrowest width an amount fits.
fn amount(units: u256) -> Amount {
    match u128::try_from(units) {
        Ok(narrow) => Amount::U128(narrow),
        Err(_) => Amount::U256(units),
    }
}

fn encode(signed: &SignedTx) -> String {
    to_hex(&wincode::serialize(signed).expect("a signed transaction always encodes"))
}

/// A fee offer as POST /offer answers it.
fn offer(value: &Value) -> Result<FeeOffer, String> {
    let wide = |name: &str| {
        value[name]
            .as_str()
            .and_then(|text| u256::from_str_radix(text, 10).ok())
            .ok_or_else(|| format!("the node sent a bad offer: no {name}"))
    };

    Ok(FeeOffer {
        gas_limit: value["gas_limit"].as_u64().ok_or("the node sent a bad offer: no gas_limit")?,
        max_price: wide("max_price")?,
        tip: wide("tip")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sent(nonce: u64, at: u64) -> Sent {
        Sent {
            id: format!("0x{nonce}"),
            from: "ed25519:0x01".to_string(),
            to: "ed25519:0x02".to_string(),
            what: "1 DEV".to_string(),
            nonce,
            at,
        }
    }

    #[test]
    fn the_next_nonce_skips_what_is_still_waiting() {
        let now = 1_000_000;
        let waiting = [sent(3, now - 1_000), sent(4, now - 500)];

        assert_eq!(next_nonce(3, &waiting, "ed25519:0x01", now), 5, "past both that wait");
        assert_eq!(next_nonce(5, &waiting, "ed25519:0x01", now), 5, "both landed");
        assert_eq!(next_nonce(3, &waiting, "ed25519:0x09", now), 3, "another account's");
        assert_eq!(next_nonce(3, &[sent(3, now - PENDING_FOR)], "ed25519:0x01", now), 3, "unseen for long, so dropped");
    }

    #[test]
    fn offers_and_amounts_read_as_the_node_and_the_chain_take_them() {
        let fee = offer(&json!({ "gas_limit": 21000, "max_price": "2000000000", "tip": "3" })).unwrap();

        assert_eq!((fee.gas_limit, fee.max_price, fee.tip), (21000, u256::new(2_000_000_000), u256::new(3)));
        assert!(offer(&json!({ "gas_limit": 1, "max_price": "x", "tip": "0" })).is_err());
        assert_eq!(amount(u256::new(5)), Amount::U128(5));
        assert!(matches!(amount(u256::MAX), Amount::U256(_)));
    }
}
