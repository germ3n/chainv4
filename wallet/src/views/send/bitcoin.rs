//! Sending Bitcoin from the account's own coins, as the wallet's light client found them: on the
//! Send page, Bitcoin or its test network picked under What. Confirmed coins alone are spent, not
//! ones a pending spend already takes. The fee rate is typed, suggested from Esplora's estimate
//! when one is set, otherwise from the tip block's average, read from Bitcoin's peers. The change
//! goes to a new further Bitcoin key of the account each time, which the light client then watches;
//! an account of one key brought in has no other, and takes its change back. Signed here, and sent
//! to Bitcoin's peers. Replaceable, so a stuck one can be bumped.

use {
    super::super::Ctx,
    crate::biometrics::{self, Check},
    crate::bitcoin::pay::{self, Built, Spendable},
    crate::format,
    crate::keys::{self, Holder},
    crate::theme,
    chainv4::crypto::bitcoin::hash160,
    chainv4::crypto::bitcoin::tx::{Network, address, p2wpkh, script_of},
    chainv4::crypto::keypair::{KeyType, Keypair, PublicKey},
    egui::{RichText, Ui},
    std::sync::{Arc, Mutex},
};

/// The form, the transaction under review, and what came of sending it.
#[derive(Default)]
pub struct BitcoinSend {
    /// The network sent on, when Bitcoin is what is sent: true for its test network.
    pub on: Option<bool>,
    to: String,
    amount: String,
    all: bool,
    rate: String,
    /// Where the rate shown came from, when it was suggested and not typed.
    rate_from: Option<&'static str>,
    asked: Option<bool>,
    estimate: Arc<Mutex<Option<Result<u64, String>>>>,
    review: Option<Review>,
    /// The broadcast under way or done, by the light client's number for it.
    broadcast: Option<(u64, Review)>,
    /// The coins sends from here spent this run, until a block shows it, so the next send does not
    /// spend them again.
    used: Vec<(String, u32)>,
    confirming: Option<Check>,
    trouble: Option<String>,
}

#[derive(Clone)]
struct Review {
    built: Built,
    testnet: bool,
    to: String,
    /// The further key the change goes to, by its index, when it is a new one; its address.
    change_key: Option<u32>,
    change_address: String,
}

impl BitcoinSend {
    /// Picks Bitcoin, or its test network, to send.
    pub fn pick(&mut self, testnet: bool) {
        if self.on != Some(testnet) {
            *self = BitcoinSend { on: Some(testnet), used: std::mem::take(&mut self.used), ..BitcoinSend::default() };
        }
    }
}

fn network(testnet: bool) -> Network {
    if testnet { Network::Test } else { Network::Main }
}

fn symbol(testnet: bool) -> &'static str {
    if testnet { "tBTC" } else { "BTC" }
}

/// The account's Bitcoin keys on a network: on Bitcoin, those it holds; on its test network, the
/// same paths under the test coin type, from its words.
fn bitcoin_keys(ctx: &Ctx, holder: &Holder, testnet: bool) -> Vec<Keypair> {
    match testnet {
        false => holder
            .publics
            .iter()
            .filter(|public| matches!(public, PublicKey::Bitcoin(_)))
            .filter_map(|public| holder.key(&public.address()).cloned())
            .collect(),
        true => match (keys::seed_of(ctx.book, holder), holder.number()) {
            (Some(seed), Some(number)) => std::iter::once(number)
                .chain(holder.extras().into_iter().filter(|(scheme, _)| *scheme == KeyType::Bitcoin).map(|(_, index)| index))
                .map(|index| seed.bitcoin_key(network(true).coin(), 0, index))
                .collect(),
            _ => Vec::new(),
        },
    }
}

fn script(key: &Keypair) -> Option<Vec<u8>> {
    match key.public() {
        PublicKey::Bitcoin(public) => Some(p2wpkh(&hash160(&public))),
        _ => None,
    }
}

/// The confirmed coins the account may spend: not spent in a block, nor by a pending spend, nor by
/// a send from here this run.
fn spendable(ctx: &Ctx, holder: &Holder, testnet: bool, used: &[(String, u32)]) -> Vec<Spendable> {
    let Some(light) = ctx.lights.on(testnet) else {
        return Vec::new();
    };
    let keys = bitcoin_keys(ctx, holder, testnet);
    let scripts = keys.iter().filter_map(script).collect();
    let pending: Vec<(String, u32)> = light.pending(&scripts).into_iter().flat_map(|pending| pending.spends).collect();

    keys.iter()
        .filter_map(|key| Some((key, script(key)?)))
        .flat_map(|(key, script)| light.coins(&script).into_iter().map(move |coin| (key, coin)))
        .filter(|(_, coin)| coin.spent.is_none())
        .filter(|(_, coin)| !pending.contains(&(coin.txid.clone(), coin.vout)) && !used.contains(&(coin.txid.clone(), coin.vout)))
        .map(|(key, coin)| Spendable { txid: coin.txid, vout: coin.vout, value: coin.value, key: key.clone() })
        .collect()
}

/// Where the change goes: a new further Bitcoin key of the account, by its index, or for an
/// account of one key brought in, that key's own address.
fn change_to(ctx: &Ctx, holder: &Holder, testnet: bool) -> Result<(Vec<u8>, Option<u32>), String> {
    match (keys::seed_of(ctx.book, holder), holder.number()) {
        (Some(seed), Some(account)) => {
            let at = keys::next_index(ctx.holders, account, holder.from(), KeyType::Bitcoin);
            let key = seed.bitcoin_key(network(testnet).coin(), 0, at);

            Ok((script(&key).expect("a bitcoin key"), Some(at)))
        }
        _ => bitcoin_keys(ctx, holder, testnet).first().and_then(script).map(|script| (script, None)).ok_or_else(|| "this account holds no Bitcoin key here".to_string()),
    }
}

/// Asks for a fee rate to suggest, once a form opens: Esplora's estimate for the next two blocks
/// when one is set, otherwise the tip block's average, from Bitcoin's peers.
fn suggest(ctx: &Ctx, state: &mut BitcoinSend, testnet: bool) {
    if state.asked == Some(testnet) {
        return;
    }

    state.asked = Some(testnet);
    let esplora = ctx.settings.esplora.trim().trim_end_matches('/').to_string();

    match (esplora.is_empty(), crate::app::client(ctx.settings)) {
        (false, Ok(client)) => {
            let (estimate, wake) = (state.estimate.clone(), ctx.wake.clone());
            client.fetch(format!("{esplora}/fee-estimates"), None, move |answer| {
                let rate = answer
                    .and_then(|answer| serde_json::from_slice::<serde_json::Value>(&answer.bytes).map_err(|err| err.to_string()))
                    .and_then(|rates| rates["2"].as_f64().ok_or_else(|| "no estimate for two blocks".to_string()))
                    .map(|rate| (rate.ceil() as u64).max(1));
                *estimate.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(rate);
                wake.request_repaint();
            });
        }
        _ => {
            if let Some(light) = ctx.lights.on(testnet) {
                light.ask_fee_rate();
            }
        }
    }
}

/// The rate suggested, once known, and where it came from.
fn suggested(ctx: &Ctx, state: &BitcoinSend, testnet: bool) -> Option<(u64, &'static str)> {
    if let Some(Ok(rate)) = *state.estimate.lock().unwrap_or_else(std::sync::PoisonError::into_inner) {
        return Some((rate, "Esplora's estimate for the next two blocks"));
    }

    match ctx.lights.on(testnet)?.fee_rate()? {
        Ok(rate) => Some((rate, "the tip block's average, from Bitcoin's peers")),
        Err(_) => None,
    }
}

/// The Bitcoin form, its review, or what came of sending: in place of the Send page's own, while
/// Bitcoin is what is sent. Whether to go back to the chain's coin.
pub fn show(ui: &mut Ui, ctx: &mut Ctx, state: &mut BitcoinSend) -> bool {
    let palette = *ctx.palette;
    let Some(testnet) = state.on else {
        return true;
    };
    let symbol = symbol(testnet);

    if ctx.lights.on(testnet).is_none() {
        let mut back = false;
        theme::card(ui, &palette, |ui| {
            ui.colored_label(palette.warn, "Bitcoin is sent from the coins the wallet's own light client found: turn it on in Settings, Bitcoin.");
            back = theme::quiet(ui, &palette, "Back").clicked();
        });

        return back;
    }

    if state.broadcast.is_some() {
        broadcasting(ui, ctx, state);

        return false;
    }

    if state.review.is_some() {
        review(ui, ctx, state);

        return false;
    }

    let Some(holder) = ctx.holders.get(ctx.current) else {
        return true;
    };
    let coins = spendable(ctx, holder, testnet, &state.used);
    let held: u64 = coins.iter().map(|coin| coin.value).sum();
    suggest(ctx, state, testnet);

    if (state.rate.trim().is_empty() || state.rate_from.is_some())
        && let Some((rate, from)) = suggested(ctx, state, testnet)
    {
        (state.rate, state.rate_from) = (rate.to_string(), Some(from));
    }

    let mut back = false;
    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "What");
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(format!("{symbol} · {}", if testnet { "Bitcoin's test network" } else { "Bitcoin" })).color(palette.ink));
            back = theme::quiet(ui, &palette, "Send the chain's coin instead").clicked();
        });
        ui.label(RichText::new(format!("{} {symbol} confirmed and spendable, in {} coin{}", format::units(&held.to_string(), 8), coins.len(), if coins.len() == 1 { "" } else { "s" })).small().color(palette.muted));

        ui.add_space(10.0);
        theme::caps(ui, &palette, "To");
        ui.add(egui::TextEdit::singleline(&mut state.to).hint_text(if testnet { "tb1q…" } else { "bc1q…" }).font(egui::TextStyle::Monospace).desired_width(f32::INFINITY));
        let to = (!state.to.trim().is_empty()).then(|| script_of(&state.to, network(testnet)));

        if let Some(Err(why)) = &to {
            ui.colored_label(palette.bad, format!("Not {why}."));
        }

        ui.add_space(10.0);
        theme::caps(ui, &palette, "Amount");
        ui.horizontal(|ui| {
            ui.add_enabled(!state.all, egui::TextEdit::singleline(&mut state.amount).hint_text(format!("0.0 {symbol}")).desired_width(160.0));
            ui.checkbox(&mut state.all, "All of it, less the fee");
        });

        ui.add_space(10.0);
        theme::caps(ui, &palette, "Fee rate");
        ui.horizontal(|ui| {
            if ui.add(egui::TextEdit::singleline(&mut state.rate).desired_width(60.0)).changed() {
                state.rate_from = None;
            }
            ui.label(RichText::new("sat/vB").color(palette.soft));
        });
        ui.label(RichText::new(match state.rate_from {
            Some(from) => format!("Suggested from {from}. Raise it to be mined sooner; it can be bumped later."),
            None if suggested(ctx, state, testnet).is_none() => "Asking for a rate to suggest…".to_string(),
            None => "As typed. It can be bumped later.".to_string(),
        }).small().color(palette.muted));

        ui.add_space(12.0);
        let ready = matches!(to, Some(Ok(_))) && (state.all || !state.amount.trim().is_empty()) && !coins.is_empty();

        if theme::primary(ui, &palette, "Review", ready).clicked()
            && let Some(Ok(to)) = to
        {
            state.trouble = None;
            let built = (|| {
                let amount = match state.all {
                    true => pay::Amount::All,
                    false => pay::Amount::Exactly(
                        format::parse_units(&state.amount, 8).and_then(|digits| digits.parse().ok()).filter(|sats| *sats > 0).ok_or(format!("an amount of {symbol}, such as 0.001"))?,
                    ),
                };
                let rate: u64 = state.rate.trim().parse().ok().filter(|rate| *rate > 0).ok_or("a fee rate of 1 or more sat/vB")?;
                let (change, change_key) = change_to(ctx, holder, testnet)?;
                let built = pay::pay(coins.clone(), &to, amount, rate, &change)?;

                Ok::<_, String>(Review { built, testnet, to: state.to.trim().to_string(), change_key, change_address: address(&change, network(testnet)) })
            })();

            match built {
                Ok(review) => state.review = Some(review),
                Err(why) => state.trouble = Some(why),
            }
        }

        if let Some(why) = &state.trouble {
            ui.colored_label(palette.bad, why.as_str());
        }
    });

    back
}

fn review(ui: &mut Ui, ctx: &mut Ctx, state: &mut BitcoinSend) {
    let palette = *ctx.palette;
    let Some(review) = state.review.clone() else {
        return;
    };
    let (built, symbol) = (&review.built, symbol(review.testnet));
    let sats = |value: u64| format!("{} {symbol}", format::units(&value.to_string(), 8));

    // What the check before sending came to: passed, it goes; turned down, nothing; the enrolled
    // fingers or faces changed, the wallet locks.
    let mut confirmed = false;

    if let Some(came) = state.confirming.as_ref().and_then(Check::take) {
        state.confirming = None;

        match came {
            Ok(_) => confirmed = true,
            Err(chainv4_ui::biometric::Failure::Canceled) => {}
            Err(chainv4_ui::biometric::Failure::Invalidated) => *ctx.lock = Some(biometrics::invalidated(ctx.wake, ctx.settings)),
            Err(chainv4_ui::biometric::Failure::Failed(why)) => state.trouble = Some(why),
        }
    }

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Review");
        ui.label(RichText::new(format!("Send {} to", sats(built.sent))).color(palette.ink));
        ui.label(RichText::new(&review.to).monospace().color(palette.ink));
        ui.add_space(6.0);
        ui.label(RichText::new(format!("Fee {}, {} virtual bytes at {} sat/vB or more", sats(built.fee), built.vsize, built.fee / built.vsize.max(1))).color(palette.soft));
        ui.label(RichText::new(match built.change {
            Some(change) => format!(
                "Change {} to {}, {}",
                sats(change),
                format::short(&review.change_address),
                if review.change_key.is_some() { "a new address of this account's" } else { "this key's own address" }
            ),
            None => "No change: what was left over was dust, which the fee keeps.".to_string(),
        }).color(palette.soft));
        ui.label(RichText::new(format!("Spends {} coin{}. Replaceable, so it can be bumped.", built.spends.len(), if built.spends.len() == 1 { "" } else { "s" })).small().color(palette.muted));
        ui.add_space(12.0);

        ui.horizontal(|ui| {
            if theme::primary(ui, &palette, "Send to Bitcoin's peers", state.confirming.is_none()).clicked() || confirmed {
                match biometrics::enrolled(ctx.wake, ctx.settings) {
                    Some((enrolled, _)) if !confirmed => state.confirming = Some(Check::start(ctx.wake, &enrolled, &format!("Send {}", sats(built.sent)))),
                    _ => send(ctx, state, review.clone()),
                }
            }

            if theme::quiet(ui, &palette, "Back").clicked() {
                (state.review, state.confirming) = (None, None);
            }
        });

        if state.confirming.is_some() {
            ui.label(RichText::new("Confirm it on the prompt to send it.").small().color(palette.muted));
        }

        if let Some(why) = &state.trouble {
            ui.colored_label(palette.bad, why.as_str());
        }
    });
}

/// Keeps the new key the change goes to, so it is found again from the words and watched, and
/// hands the transaction to the light client to broadcast.
fn send(ctx: &mut Ctx, state: &mut BitcoinSend, review: Review) {
    if let Some(at) = review.change_key {
        let holder = &ctx.holders[ctx.current];
        let (account, from) = (holder.number(), holder.from());

        // Another key taken since the review would be this one's index: review it again.
        if account.map(|account| keys::next_index(ctx.holders, account, from, KeyType::Bitcoin)) != Some(at) {
            (state.review, state.trouble) = (None, Some("The account's keys changed since: review it again.".to_string()));
            return;
        }

        let Some(seed) = ctx.book.phrase_of(from).and_then(|words| words.seed()) else {
            state.trouble = Some("The account's words are gone from this wallet.".to_string());
            return;
        };
        ctx.holders[ctx.current].add_key(seed, KeyType::Bitcoin, at);
        *ctx.changed = true;
    }

    let Some(light) = ctx.lights.on_mut(review.testnet) else {
        state.trouble = Some("The light client stopped.".to_string());
        return;
    };
    let id = light.broadcast(review.built.raw.clone());
    state.used.extend(review.built.spends.iter().cloned());
    (state.review, state.broadcast) = (None, Some((id, review)));
}

fn broadcasting(ui: &mut Ui, ctx: &mut Ctx, state: &mut BitcoinSend) {
    let palette = *ctx.palette;
    let Some((id, review)) = state.broadcast.clone() else {
        return;
    };
    let came = ctx.lights.on(review.testnet).and_then(|light| light.broadcasted(id));

    theme::card(ui, &palette, |ui| match came {
        None => {
            ui.label(RichText::new("Sending to Bitcoin's peers…").color(palette.soft));
        }
        Some(Ok(txid)) => {
            ui.label(RichText::new("Sent to Bitcoin's peers").strong().color(palette.ink));
            ui.label(RichText::new(&txid).monospace().small().color(palette.soft));
            ui.label(RichText::new("It shows as pending on Overview until a block has it; the change is found with it.").small().color(palette.muted));

            if theme::quiet(ui, &palette, "Send more Bitcoin").clicked() {
                let testnet = review.testnet;
                *state = BitcoinSend { on: Some(testnet), used: std::mem::take(&mut state.used), ..BitcoinSend::default() };
            }
        }
        Some(Err(why)) => {
            ui.colored_label(palette.bad, format!("Bitcoin's peers did not take it: {why}"));

            if theme::quiet(ui, &palette, "Back").clicked() {
                // Its coins were not spent after all.
                state.used.retain(|coin| !review.built.spends.contains(coin));
                state.broadcast = None;
            }
        }
    });
}
