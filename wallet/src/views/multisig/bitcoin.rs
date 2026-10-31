//! A native multisig's coins on Bitcoin, for one whose members name xpubs: its descriptor and
//! receiving addresses, what Esplora finds at them, and spends as PSBTs, signed with this account's
//! words, on the offline signer, or in another member's Bitcoin wallet, combined, finished and
//! broadcast. The members' xpubs are the node's word, as the rest of the page's; a PSBT says what
//! it spends and pays, and each signer checks that.

use {
    super::super::scan::Scan,
    super::super::Ctx,
    crate::airgap::Message,
    crate::bitcoin::{PSBT_TYPE, Request, Signs},
    crate::format,
    crate::theme,
    chainv4::crypto::bitcoin::multisig::{bitcoin_address, descriptor, parse_xpub},
    chainv4::crypto::bitcoin::psbt::{MultisigCoin, MultisigWallet, Psbt, Utxo},
    chainv4::crypto::bitcoin::tx::{Network, address, script_of},
    chainv4::structs::hex::{from_hex, to_hex},
    egui::{RichText, Ui},
    serde_json::Value,
    std::collections::BTreeMap,
    std::sync::{Arc, Mutex},
};

/// How many addresses on each chain one look covers.
const WINDOW: u32 = 20;

#[derive(Default)]
pub struct BitcoinState {
    /// The multisig all this is of: what is found is dropped when another is picked.
    of: String,
    receive: u32,
    window: u32,
    found: Arc<Mutex<Vec<Found>>>,
    /// What each address holds, and whether it was ever used, by chain and number.
    stats: BTreeMap<(u32, u32), (u64, bool)>,
    utxos: Vec<Utxo>,
    to: String,
    amount: String,
    fee_rate: String,
    psbt: Option<Psbt>,
    pasted: String,
    asking_signer: bool,
    scan: Scan,
    note: Option<String>,
    trouble: Option<String>,
}

/// What Esplora answered.
enum Found {
    Stats { chain: u32, index: u32, held: u64, used: bool },
    Utxos(Vec<Utxo>),
    Broadcast(Result<String, String>),
    Failed(String),
}

/// The multisig's side on Bitcoin, from what the node says of it: None when its members name no
/// xpubs.
pub fn wallet_of(value: &Value) -> Option<MultisigWallet> {
    let xpubs = value["members"].as_array()?.iter().map(|member| member["bitcoin"].as_str().and_then(|text| parse_xpub(text).ok()).map(|(xpub, _)| xpub)).collect::<Option<Vec<_>>>()?;
    let network = match value["testnet"].as_bool() {
        Some(true) => Network::Test,
        _ => Network::Main,
    };

    Some(MultisigWallet { threshold: u32::try_from(value["threshold"].as_u64()?).ok()?, xpubs, network })
}

pub fn show(ui: &mut Ui, ctx: &mut Ctx, state: &mut BitcoinState, account: &str, wallet: &MultisigWallet) {
    let palette = *ctx.palette;

    if state.of != account {
        *state = BitcoinState { of: account.to_string(), window: WINDOW, fee_rate: "2".to_string(), ..BitcoinState::default() };
    }

    take_found(state);

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, &format!("On {}, {} of {}", wallet.network.name(), wallet.threshold, wallet.xpubs.len()));
        let receiving = descriptor(wallet.threshold, &wallet.xpubs, wallet.network, 0);
        theme::end_row(
            ui,
            |ui| {
                ui.add(egui::Label::new(RichText::new("Its descriptor, for Sparrow, Nunchuk or Bitcoin Core to watch it").small().color(palette.soft)).wrap());
            },
            |ui| {
                if theme::quiet(ui, &palette, "Copy").clicked() {
                    ui.ctx().copy_text(format!("{receiving}\n{}", descriptor(wallet.threshold, &wallet.xpubs, wallet.network, 1)));
                }
            },
        );
        ui.add(egui::Label::new(theme::mono(&receiving).color(palette.muted)).wrap());
        ui.add_space(8.0);

        let next = next_unused(state, 0);
        let index = state.receive.max(next);

        if let Some(address) = bitcoin_address(wallet.threshold, &wallet.xpubs, wallet.network, 0, index) {
            super::super::row(ui, &palette, &format!("Receive at, {index}"), |ui| super::super::address(ui, &palette, &address));
        }

        if theme::quiet(ui, &palette, "A fresh address").clicked() {
            state.receive = index + 1;
        }

        ui.add_space(8.0);

        if ctx.settings.esplora.trim().is_empty() {
            ui.label(RichText::new("Name an Esplora API in Settings to see its coins and spend them.").small().color(palette.muted));

            return;
        }

        let held: u64 = state.stats.values().map(|(held, _)| held).sum();
        super::super::row(ui, &palette, "Holds", |ui| {
            ui.label(match state.stats.is_empty() {
                true => "not looked up yet".to_string(),
                false => format!("{} {}, {} coin{}, as Esplora says", format::units(&held.to_string(), 8), wallet.network.symbol(), state.utxos.len(), if state.utxos.len() == 1 { "" } else { "s" }),
            });
        });
        ui.horizontal_wrapped(|ui| {
            if theme::quiet(ui, &palette, "Look up its coins").clicked() {
                look(ctx, state, wallet);
            }

            if !state.stats.is_empty() && theme::quiet(ui, &palette, "Look further").clicked() {
                state.window += WINDOW;
                look(ctx, state, wallet);
            }
        });
    });

    if ctx.settings.esplora.trim().is_empty() {
        return;
    }

    match state.psbt.is_some() {
        true => spend(ui, ctx, state, wallet),
        false => pay(ui, ctx, state, wallet),
    }
}

/// The first number on a chain no address was used at, by what Esplora found.
fn next_unused(state: &BitcoinState, chain: u32) -> u32 {
    state.stats.iter().filter(|((at, _), (_, used))| *at == chain && *used).map(|((_, index), _)| index + 1).max().unwrap_or(0)
}

/// Asks Esplora what each address in the window holds, receiving and change.
fn look(ctx: &Ctx, state: &mut BitcoinState, wallet: &MultisigWallet) {
    let Ok(client) = crate::app::client(ctx.settings) else {
        return;
    };
    let base = ctx.settings.esplora.trim().trim_end_matches('/').to_string();

    for chain in 0..2 {
        for index in 0..state.window {
            let Some(address) = bitcoin_address(wallet.threshold, &wallet.xpubs, wallet.network, chain, index) else {
                continue;
            };
            let (found, wake, client_again, base_again, address_again) = (state.found.clone(), ctx.wake.clone(), client.clone(), base.clone(), address.clone());

            client.fetch(format!("{base}/address/{address}"), None, move |answer| {
                let stats = answer.and_then(|answer| serde_json::from_slice::<Value>(&answer.bytes).map_err(|err| err.to_string()));
                let found_now = match stats {
                    Ok(stats) => {
                        let side = |name: &str| stats[name]["funded_txo_sum"].as_u64().unwrap_or(0).saturating_sub(stats[name]["spent_txo_sum"].as_u64().unwrap_or(0));
                        let used = stats["chain_stats"]["tx_count"].as_u64().unwrap_or(0) + stats["mempool_stats"]["tx_count"].as_u64().unwrap_or(0) > 0;
                        let held = side("chain_stats") + side("mempool_stats");

                        if held > 0 {
                            utxos(&client_again, &base_again, &address_again, chain, index, &found, &wake);
                        }

                        Found::Stats { chain, index, held, used }
                    }
                    Err(why) => Found::Failed(why),
                };
                found.lock().expect("never poisoned").push(found_now);
                wake.request_repaint();
            });
        }
    }

    state.utxos.clear();
}

/// Asks Esplora for an address's coins.
fn utxos(client: &chainv4_ui::net::Client, base: &str, address: &str, chain: u32, index: u32, found: &Arc<Mutex<Vec<Found>>>, wake: &egui::Context) {
    let (found, wake) = (found.clone(), wake.clone());

    client.fetch(format!("{base}/address/{address}/utxo"), None, move |answer| {
        let read = answer.and_then(|answer| serde_json::from_slice::<Value>(&answer.bytes).map_err(|err| err.to_string())).map(|list| {
            list.as_array()
                .into_iter()
                .flatten()
                .filter_map(|utxo| {
                    let mut txid: [u8; 32] = from_hex(&format!("0x{}", utxo["txid"].as_str()?)).and_then(|bytes| bytes.try_into().ok())?;
                    // Explorers write a txid backwards.
                    txid.reverse();

                    Some(Utxo { txid, vout: u32::try_from(utxo["vout"].as_u64()?).ok()?, coin: MultisigCoin { value: utxo["value"].as_u64()?, chain, index } })
                })
                .collect()
        });
        found.lock().expect("never poisoned").push(match read {
            Ok(list) => Found::Utxos(list),
            Err(why) => Found::Failed(why),
        });
        wake.request_repaint();
    });
}

fn take_found(state: &mut BitcoinState) {
    let found: Vec<Found> = std::mem::take(&mut *state.found.lock().expect("never poisoned"));

    for found in found {
        match found {
            Found::Stats { chain, index, held, used } => {
                state.stats.insert((chain, index), (held, used));
            }
            Found::Utxos(list) => {
                for utxo in list {
                    if !state.utxos.iter().any(|known| known.txid == utxo.txid && known.vout == utxo.vout) {
                        state.utxos.push(utxo);
                    }
                }
            }
            Found::Broadcast(Ok(txid)) => {
                state.note = Some(format!("Broadcast {}", format::short(&txid)));
                state.psbt = None;
            }
            Found::Broadcast(Err(why)) => state.trouble = Some(format!("Esplora would not take it: {why}")),
            Found::Failed(why) => state.trouble = Some(format!("Esplora: {why}")),
        }
    }
}

/// A payment from the multisig's coins, made into a PSBT for the members.
fn pay(ui: &mut Ui, ctx: &mut Ctx, state: &mut BitcoinState, wallet: &MultisigWallet) {
    let palette = *ctx.palette;

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Pay from it on Bitcoin");
        ui.add(egui::TextEdit::singleline(&mut state.to).hint_text(format!("To: {}1q…", wallet.network.hrp())).font(egui::TextStyle::Monospace).desired_width(f32::INFINITY));
        ui.add(egui::TextEdit::singleline(&mut state.amount).hint_text(format!("Amount in {}", wallet.network.symbol())).desired_width(f32::INFINITY));
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Fee rate, sat/vB").color(palette.soft));
            ui.add(egui::TextEdit::singleline(&mut state.fee_rate).desired_width(48.0));
        });

        if theme::primary(ui, &palette, "Make the PSBT", !state.utxos.is_empty()).clicked() {
            let made = (|| -> Result<Psbt, String> {
                let to = script_of(&state.to, wallet.network)?;
                let sats = format::parse_units(state.amount.trim(), 8).and_then(|raw| raw.parse::<u64>().ok()).filter(|sats| *sats > 0).ok_or("an amount above zero")?;
                let rate = state.fee_rate.trim().parse::<u64>().ok().filter(|rate| *rate > 0).ok_or("a fee rate of 1 or more")?;

                wallet.spend(&state.utxos, to, sats, rate, next_unused(state, 1))
            })();

            match made {
                Ok(psbt) => {
                    state.psbt = Some(psbt);
                    state.trouble = None;
                }
                Err(why) => state.trouble = Some(why),
            }
        }

        notes(ui, &palette, state);
    });
}

/// A spend on its way round the members: what it pays, how far its signing came, and signing,
/// combining and broadcasting it.
fn spend(ui: &mut Ui, ctx: &mut Ctx, state: &mut BitcoinState, wallet: &MultisigWallet) {
    let palette = *ctx.palette;
    let Some(mut psbt) = state.psbt.clone() else {
        return;
    };

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "A spend for the members to sign");

        for output in &psbt.tx.outputs {
            ui.label(RichText::new(format!("{} {} to {}", format::units(&output.value.to_string(), 8), wallet.network.symbol(), format::short(&address(&output.script, wallet.network)))).color(palette.ink));
        }

        let progress = psbt.multisig_progress();
        let done = !progress.is_empty() && progress.iter().all(|(held, needed)| *held >= *needed as usize);
        ui.label(RichText::new(format!("signatures: {}", progress.iter().map(|(held, needed)| format!("{held} of {needed}")).collect::<Vec<_>>().join(", "))).small().color(palette.soft));
        ui.add_space(8.0);

        if state.asking_signer {
            ui.label(RichText::new("On the offline signer, open Sign and read this code; then read its code back.").color(palette.soft));
            super::super::code(ui, &palette, &Message::Bitcoin(Box::new(Request { psbt: psbt.clone(), kind: PSBT_TYPE })));

            match state.scan.show(ui, &palette) {
                Some(Message::Bitcoin(signed)) => match psbt.combine(&signed.psbt) {
                    Ok(()) => state.asking_signer = false,
                    Err(why) => state.scan.fail(why),
                },
                Some(other) => state.scan.refuse(&other, "the signed PSBT"),
                None => {}
            }
        }

        ui.horizontal_wrapped(|ui| {
            let seeds = crate::keys::seeds(ctx.book);

            if !seeds.is_empty() && theme::primary(ui, &palette, "Sign with these words", !done).clicked() {
                match psbt.sign_all(&seeds) {
                    Ok(signed) => psbt = signed,
                    Err(why) => state.trouble = Some(why),
                }
            }

            if !state.asking_signer && theme::quiet(ui, &palette, "Ask the signer").clicked() {
                state.asking_signer = true;
            }

            if theme::quiet(ui, &palette, "Copy as base64").clicked() {
                ui.ctx().copy_text(psbt.to_base64());
                state.note = Some("Copied: give it to the other members' wallets, then paste what they signed here.".to_string());
            }

            if done && theme::primary(ui, &palette, "Finish and broadcast", true).clicked() {
                match psbt.finish_multisig() {
                    Ok(tx) => broadcast(ctx, state, to_hex(&tx.encode()).trim_start_matches("0x").to_string()),
                    Err(why) => state.trouble = Some(why),
                }
            }

            if theme::quiet(ui, &palette, "Put away").clicked() {
                state.psbt = None;
                state.asking_signer = false;
            }
        });

        ui.add(egui::TextEdit::multiline(&mut state.pasted).hint_text("Paste a member's signed PSBT, in base64").font(egui::TextStyle::Monospace).desired_rows(2).desired_width(f32::INFINITY));

        if !state.pasted.trim().is_empty() {
            match Psbt::from_base64(&state.pasted).and_then(|theirs| psbt.combine(&theirs)) {
                Ok(()) => state.pasted.clear(),
                Err(why) => {
                    ui.colored_label(palette.bad, why);
                }
            }
        }

        notes(ui, &palette, state);
    });

    if state.psbt.is_some() {
        state.psbt = Some(psbt);
    }
}

/// Broadcasts a finished spend through Esplora.
fn broadcast(ctx: &Ctx, state: &BitcoinState, hex: String) {
    let Ok(client) = crate::app::client(ctx.settings) else {
        return;
    };
    let (found, wake) = (state.found.clone(), ctx.wake.clone());
    let url = format!("{}/tx", ctx.settings.esplora.trim().trim_end_matches('/'));

    client.fetch(url, Some(hex.into_bytes()), move |answer| {
        let result = answer.and_then(|answer| match answer.ok {
            true => Ok(String::from_utf8_lossy(&answer.bytes).trim().to_string()),
            false => Err(format!("{}: {}", answer.status, String::from_utf8_lossy(&answer.bytes).trim())),
        });
        found.lock().expect("never poisoned").push(Found::Broadcast(result));
        wake.request_repaint();
    });
}

fn notes(ui: &mut Ui, palette: &crate::theme::Palette, state: &BitcoinState) {
    if let Some(note) = &state.note {
        ui.label(RichText::new(note).small().color(palette.good));
    }

    if let Some(trouble) = &state.trouble {
        ui.colored_label(palette.bad, trouble);
    }
}

#[cfg(test)]
mod tests {
    use {super::*, chainv4::crypto::bitcoin::multisig::xpub_text, chainv4::crypto::mnemonic::Phrase, serde_json::json};

    #[test]
    fn its_side_on_bitcoin_reads_from_the_node_s_members_and_the_next_address_skips_the_used() {
        let seeds: Vec<_> = ["abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about", "legal winner thank year wave sausage worth useful legal winner thank yellow"]
            .iter()
            .map(|words| Phrase::parse(words).unwrap().seed(""))
            .collect();
        let members: Vec<Value> = seeds.iter().map(|seed| json!({ "key": "ed25519:0x00", "bitcoin": xpub_text(&seed.multisig_account(1), Network::Test), "evm": "0x11" })).collect();
        let wallet = wallet_of(&json!({ "members": members, "threshold": 2, "testnet": true })).unwrap();

        assert_eq!((wallet.threshold, wallet.network), (2, Network::Test));
        assert_eq!(wallet.xpubs, seeds.iter().map(|seed| seed.multisig_account(1)).collect::<Vec<_>>());
        assert!(wallet_of(&json!({ "members": [{ "key": "ed25519:0x00", "bitcoin": null }], "threshold": 1 })).is_none(), "no xpubs, no side on Bitcoin");

        let mut state = BitcoinState::default();
        state.stats.insert((0, 0), (0, true));
        state.stats.insert((0, 3), (5_000, true));
        state.stats.insert((0, 4), (0, false));
        state.stats.insert((1, 0), (0, true));

        assert_eq!((next_unused(&state, 0), next_unused(&state, 1)), (4, 1));
    }
}
