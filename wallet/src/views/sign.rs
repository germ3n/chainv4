//! An offline signer's page: a request read off the sender's screen, all it asks laid out to check,
//! and once signed, the signed transaction as a code for the sender to read back. Nothing here
//! talks to a node: the request says the nonce and fee, and the sender proves the rest. With
//! biometrics on, signing waits for one more check.
//!
//! MetaMask's requests read here too, once it paired with the signer as its QR hardware wallet:
//! a transaction, typed data or a message, for any chain, the chain named large, and the signature
//! back as a code for MetaMask to read. So do a Bitcoin wallet's PSBTs, once it paired on Pair:
//! the network named large, each output and whether it comes back as change, and the fee; the PSBT
//! back signed, for the wallet to send.

use {
    super::Ctx,
    super::scan::Scan,
    crate::airgap::{self, Message, Request},
    crate::format,
    crate::bitcoin::{self, Review, Signs},
    crate::hardware::{self, Asked, SignRequest},
    crate::biometrics::{self, Check},
    crate::theme,
    chainv4_ui::biometric::Failure,
    egui::{RichText, Ui},
};

#[derive(Default)]
pub struct SignState {
    scan: Scan,
    step: Step,
    /// The biometric check before signing, while it is under way.
    confirming: Option<Check>,
}

#[derive(Default)]
enum Step {
    #[default]
    Reading,
    /// Read, to check before signing; why it cannot be signed, if it cannot.
    Review { request: Box<Request>, trouble: Option<String> },
    /// Signed: the code for the sender.
    Signed { message: Box<Message> },
    /// MetaMask's request, read, to check before signing; the seed it was matched to, by its
    /// fingerprint, as the wallet may hold several; why it cannot be signed, if it cannot.
    MetaMask { request: Box<SignRequest>, seed: Option<crate::vault::Fingerprint>, trouble: Option<String> },
    /// Signed for MetaMask: the code it reads back.
    MetaMaskSigned { text: String },
    /// A Bitcoin wallet's PSBT, read, laid out to check; why it cannot be signed, if it cannot.
    Bitcoin { request: Box<bitcoin::Request>, review: Option<Box<Review>>, trouble: Option<String> },
    /// Signed for the Bitcoin wallet: the PSBT it reads back.
    BitcoinSigned { message: Box<Message> },
    /// A multisig's transaction, read, with the key to sign it with as a member.
    Multisig { bundle: Box<chainv4::registry::multisig::MultisigBundle>, key: Option<chainv4::crypto::keypair::PublicKey>, trouble: Option<String> },
}

pub fn show(ui: &mut Ui, ctx: &mut Ctx, state: &mut SignState) {
    let palette = *ctx.palette;
    theme::title(ui, &palette, "Sign", "A request from the watch-only sender: read it, check it, sign it, and show it back.");

    match &state.step {
        Step::Reading => reading(ui, ctx, state),
        Step::Review { .. } => review(ui, ctx, state),
        Step::MetaMask { .. } => metamask(ui, ctx, state),
        Step::Bitcoin { .. } => bitcoin_review(ui, ctx, state),
        Step::Multisig { .. } => multisig_review(ui, ctx, state),
        Step::BitcoinSigned { message } => {
            let message = message.clone();
            theme::card(ui, &palette, |ui| {
                theme::caps(ui, &palette, "Signed for the Bitcoin wallet");
                ui.label(RichText::new("In the Bitcoin wallet, read this code back: it holds the transaction with this signer's signatures, for the wallet to finish and send.").color(palette.soft));
                ui.add_space(10.0);
                super::code(ui, &palette, &message);
                ui.add_space(12.0);

                if theme::primary(ui, &palette, "Sign another", true).clicked() {
                    state.step = Step::Reading;
                }
            });
        }
        Step::MetaMaskSigned { text } => {
            let text = text.clone();
            theme::card(ui, &palette, |ui| {
                theme::caps(ui, &palette, "Signed for MetaMask");
                ui.label(RichText::new("In MetaMask, Get signature, and read this code: it checks the signature against the account before it sends.").color(palette.soft));
                ui.add_space(10.0);
                let side = ui.available_width().clamp(200.0, 420.0);
                ui.vertical_centered(|ui| super::receive::qr(ui, &text, side));
                ui.add_space(12.0);

                if theme::primary(ui, &palette, "Sign another", true).clicked() {
                    state.step = Step::Reading;
                }
            });
        }
        Step::Signed { message } => {
            let message = message.clone();
            theme::card(ui, &palette, |ui| {
                theme::caps(ui, &palette, "Signed");
                ui.label(RichText::new("Show this to the sender: it checks this is what it asked for, and sends it.").color(palette.soft));
                ui.add_space(10.0);
                super::code(ui, &palette, &message);
                ui.add_space(12.0);

                if theme::primary(ui, &palette, "Sign another", true).clicked() {
                    state.step = Step::Reading;
                }
            });
        }
    }
}

fn reading(ui: &mut Ui, ctx: &mut Ctx, state: &mut SignState) {
    let palette = *ctx.palette;

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Read the request");
        ui.label(RichText::new("On the sender, Send shows a code once it is priced. Read it here.").color(palette.soft));
        ui.add_space(10.0);

        match state.scan.show(ui, &palette) {
            Some(Message::MetaMask(request)) => {
                let seeds = crate::keys::seeds(ctx.book);
                let (seed, trouble) = match seeds.as_slice() {
                    [] => (None, Some("This wallet holds no words, only keys brought in: MetaMask signs with accounts words make.".to_string())),
                    seeds => match request.choose(seeds) {
                        Ok(seed) => (Some(seed.ethereum_account().master_fingerprint), request.account().and_then(|_| request.asked()).err()),
                        Err(why) => (None, Some(why)),
                    },
                };
                state.step = Step::MetaMask { request, seed, trouble };
            }
            Some(Message::Sign(request)) => {
                let trouble = match ctx.holders.iter().find(|holder| holder.publics.contains(&request.signer)) {
                    Some(holder) if holder.keys.is_empty() => Some("This wallet holds that account's public keys alone.".to_string()),
                    Some(_) => None,
                    None => Some(format!("No account in this wallet signs for {}: pair the sender with this wallet's accounts.", request.signer.address())),
                };
                state.step = Step::Review { request: Box::new(request), trouble };
            }
            Some(Message::Bitcoin(request)) => {
                let seeds = crate::keys::seeds(ctx.book);
                let read = match seeds.is_empty() {
                    true => Err("This wallet holds no words, only keys brought in: a Bitcoin wallet signs with the account words make.".to_string()),
                    false => request.psbt.review_all(&seeds),
                };
                state.step = match read {
                    Ok(review) => Step::Bitcoin { request, review: Some(Box::new(review)), trouble: None },
                    Err(why) => Step::Bitcoin { request, review: None, trouble: Some(why) },
                };
            }
            Some(Message::Multisig(bundle)) => {
                let key = ctx.holders.iter().flat_map(|holder| holder.keys.iter()).map(|key| key.public()).find(|key| !matches!(key, chainv4::crypto::keypair::PublicKey::Bitcoin(_)));
                let trouble = key.is_none().then(|| "This wallet holds no keys to sign with, only public ones.".to_string());
                state.step = Step::Multisig { bundle, key, trouble };
            }
            Some(other) => state.scan.refuse(&other, "a request to sign, this wallet's, MetaMask's or a Bitcoin wallet's"),
            None => {}
        }
    });
}

fn review(ui: &mut Ui, ctx: &mut Ctx, state: &mut SignState) {
    let palette = *ctx.palette;
    let Step::Review { request, trouble } = &mut state.step else {
        return;
    };
    let described = airgap::describe(request);
    let mut next = None;

    // What the check before signing came to: passed, it signs; the fingers or faces enrolled
    // changed since, the wallet locks, for its password.
    let mut confirmed = false;

    if let Some(came) = state.confirming.as_ref().and_then(Check::take) {
        state.confirming = None;

        match came {
            Ok(_) => confirmed = true,
            Err(Failure::Canceled) => {}
            Err(Failure::Invalidated) => {
                biometrics::turn_off(ctx.wake, ctx.settings);
                *ctx.lock = Some(biometrics::CHANGED.to_string());
            }
            Err(Failure::Failed(why)) => *trouble = Some(why),
        }
    }

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, &described.kind);
        ui.add_space(4.0);

        if let Some((shown, raw)) = &described.what {
            theme::fitted(ui, shown, 34.0, 16.0, egui::FontFamily::Name(theme::SERIF.into()), palette.ink);
            ui.label(RichText::new(format!("{raw} units, signed as such")).small().color(palette.muted));
            ui.add_space(8.0);
        }

        super::row(ui, &palette, "From", |ui| super::address(ui, &palette, &described.from));

        if let Some(to) = &described.to {
            super::row(ui, &palette, "To", |ui| super::address(ui, &palette, to));
        }

        if let Some(token) = &described.token {
            super::row(ui, &palette, "Token", |ui| super::address(ui, &palette, token));
        }

        super::row(ui, &palette, "Fee, at most", |ui| {
            ui.label(&described.fee).on_hover_text(&described.gas);
        });
        super::row(ui, &palette, "Nonce", |ui| {
            ui.label(described.nonce.to_string());
        });
        ui.add_space(6.0);
        ui.label(
            RichText::new("Check each against what you meant to send: the chain runs what is signed here. Names and decimals are the sender's word; the units and addresses are what is signed.")
                .small()
                .color(palette.warn),
        );
        egui::CollapsingHeader::new(RichText::new("Everything it holds").color(palette.soft)).id_salt("everything").show(ui, |ui| {
            ui.add(egui::Label::new(theme::mono(&described.everything).color(palette.soft)).wrap());
        });

        if let Some(trouble) = trouble.as_ref() {
            ui.add_space(6.0);
            ui.colored_label(palette.bad, trouble.as_str());
        }

        ui.add_space(12.0);
        ui.horizontal_wrapped(|ui| {
            let tapped = theme::primary(ui, &palette, "Sign", trouble.is_none() && state.confirming.is_none()).clicked();
            let biometric = biometrics::enrolled(ctx.wake, ctx.settings);

            if tapped && !confirmed && let Some((enrolled, _)) = biometric {
                state.confirming = Some(Check::start(ctx.wake, &enrolled, "Sign this transaction"));
            } else if tapped || confirmed {
                let signed = ctx
                    .holders
                    .iter()
                    .find_map(|holder| holder.keys.iter().find(|key| key.public() == request.signer))
                    .ok_or_else(|| "the key is gone from this wallet".to_string())
                    .and_then(|key| airgap::sign(request, key));

                match signed {
                    Ok(signed) => next = Some(Step::Signed { message: Box::new(Message::Signed(signed)) }),
                    Err(why) => *trouble = Some(why),
                }
            }

            if theme::quiet(ui, &palette, "Cancel").clicked() {
                next = Some(Step::Reading);
                state.confirming = None;
            }
        });
    });

    if let Some(next) = next {
        state.step = next;
    }
}

/// A multisig's transaction laid out: what it does, from which multisig, at which nonce and fee,
/// who signed already, and the key to sign with as a member. Whether that key is one of the
/// multisig's members the signer cannot know: the chain checks it.
fn multisig_review(ui: &mut Ui, ctx: &mut Ctx, state: &mut SignState) {
    let palette = *ctx.palette;
    let Step::Multisig { bundle, key, trouble } = &mut state.step else {
        return;
    };
    let mut next = None;
    let mut confirmed = false;

    if let Some(came) = state.confirming.as_ref().and_then(Check::take) {
        state.confirming = None;

        match came {
            Ok(_) => confirmed = true,
            Err(Failure::Canceled) => {}
            Err(Failure::Invalidated) => {
                biometrics::turn_off(ctx.wake, ctx.settings);
                *ctx.lock = Some(biometrics::CHANGED.to_string());
            }
            Err(Failure::Failed(why)) => *trouble = Some(why),
        }
    }

    let request = airgap::Request {
        tx: bundle.body.tx.clone(),
        nonce: bundle.body.nonce,
        fee: bundle.body.fee,
        signer: key.unwrap_or(chainv4::crypto::keypair::PublicKey::Ed25519([0; 32])),
        coin: airgap::Shown { symbol: "units".to_string(), decimals: 0 },
        token: None,
    };
    let described = airgap::describe(&request);

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, &format!("A multisig's transaction: {}", described.kind));
        super::row(ui, &palette, "Multisig", |ui| super::address(ui, &palette, &bundle.body.account.to_string()));

        if let Some(to) = &described.to {
            super::row(ui, &palette, "To", |ui| super::address(ui, &palette, to));
        }

        if let Some((_, raw)) = &described.what {
            super::row(ui, &palette, "Amount", |ui| {
                ui.label(format!("{raw} units"));
            });
        }

        super::row(ui, &palette, "Nonce", |ui| {
            ui.label(bundle.body.nonce.to_string());
        });
        super::row(ui, &palette, "Signed by", |ui| {
            ui.label(format!("{} member{} so far", bundle.signatures.len(), if bundle.signatures.len() == 1 { "" } else { "s" }));
        });

        let keys: Vec<chainv4::crypto::keypair::PublicKey> = ctx.holders.iter().flat_map(|holder| holder.keys.iter()).map(|key| key.public()).filter(|key| !matches!(key, chainv4::crypto::keypair::PublicKey::Bitcoin(_))).collect();

        if keys.len() > 1 {
            theme::caps(ui, &palette, "Sign as");
            egui::ComboBox::from_id_salt("multisig-key").selected_text(key.map(|key| format::short(&key.to_string())).unwrap_or_default()).width(ui.available_width()).show_ui(ui, |ui| {
                for candidate in &keys {
                    ui.selectable_value(key, Some(*candidate), format::short(&candidate.to_string()));
                }
            });
        }

        ui.label(RichText::new("Amounts are the chain's units: the multisig's transaction names no coin. The chain counts this signature only if the key is one of its members'.").small().color(palette.warn));
        egui::CollapsingHeader::new(RichText::new("Everything it holds").color(palette.soft)).id_salt("multisig-everything").show(ui, |ui| {
            ui.add(egui::Label::new(theme::mono(&described.everything).color(palette.soft)).wrap());
        });

        if let Some(trouble) = trouble.as_ref() {
            ui.colored_label(palette.bad, trouble.as_str());
        }

        ui.add_space(12.0);
        ui.horizontal_wrapped(|ui| {
            let tapped = theme::primary(ui, &palette, "Sign", trouble.is_none() && key.is_some() && state.confirming.is_none()).clicked();
            let biometric = biometrics::enrolled(ctx.wake, ctx.settings);

            if tapped && !confirmed && let Some((enrolled, _)) = biometric {
                state.confirming = Some(Check::start(ctx.wake, &enrolled, "Sign this multisig transaction"));
            } else if tapped || confirmed {
                match ctx.holders.iter().find_map(|holder| holder.keys.iter().find(|held| Some(held.public()) == *key)) {
                    Some(keypair) => {
                        let mut signed = (**bundle).clone();
                        signed.sign(keypair);
                        next = Some(Step::Signed { message: Box::new(Message::Multisig(Box::new(signed))) });
                    }
                    None => *trouble = Some("the key is gone from this wallet".to_string()),
                }
            }

            if theme::quiet(ui, &palette, "Cancel").clicked() {
                next = Some(Step::Reading);
                state.confirming = None;
            }
        });
    });

    if let Some(next) = next {
        state.step = next;
    }
}

/// MetaMask's request laid out: the chain, large; the account; and what it signs.
fn metamask(ui: &mut Ui, ctx: &mut Ctx, state: &mut SignState) {
    let palette = *ctx.palette;
    let Step::MetaMask { request, seed, trouble } = &mut state.step else {
        return;
    };
    // The words it was matched to when read, and if those have gone since, none: never others.
    let chosen = *seed;
    let words = chosen.and_then(|fingerprint| ctx.book.holding(fingerprint));
    let seed = words.and_then(crate::vault::Words::seed);
    let mut next = None;
    let mut confirmed = false;

    if let Some(came) = state.confirming.as_ref().and_then(Check::take) {
        state.confirming = None;

        match came {
            Ok(_) => confirmed = true,
            Err(Failure::Canceled) => {}
            Err(Failure::Invalidated) => {
                biometrics::turn_off(ctx.wake, ctx.settings);
                *ctx.lock = Some(biometrics::CHANGED.to_string());
            }
            Err(Failure::Failed(why)) => *trouble = Some(why),
        }
    }

    let yours = ctx.settings.signer_chain;
    let chain = request.chain();

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, &request.asker());
        ui.add_space(4.0);

        match chain {
            Some(chain) => {
                theme::fitted(ui, &hardware::chain_name(chain, yours), 34.0, 16.0, egui::FontFamily::Name(theme::SERIF.into()), palette.ink);
                ui.label(RichText::new(format!("chain id {chain}")).small().color(palette.muted));

                match yours {
                    Some(yours) if yours != chain => {
                        ui.colored_label(palette.warn, format!("Not your chain, {yours}: what this signs counts on chain {chain} alone."));
                    }
                    None => {
                        ui.label(RichText::new("Set your chain's id in Settings, and this names it.").small().color(palette.muted));
                    }
                    Some(_) => {}
                }
            }
            None => {
                ui.colored_label(palette.warn, "It names no chain: a signature that may count on any.");
            }
        }

        ui.add_space(8.0);

        if let (Ok(account), Some(seed)) = (request.account(), seed) {
            let address = seed.key(chainv4::crypto::keypair::KeyType::Secp256k1, account).public().address();
            super::row(ui, &palette, "Account", |ui| {
                ui.vertical(|ui| {
                    super::address(ui, &palette, &address.to_string());
                    ui.label(RichText::new(format!("m/{}", request.path.text())).small().color(palette.muted));

                    // With several sets of words, which of them signs.
                    if let Some(index) = ctx.book.phrases.iter().position(|held| chosen.is_some() && held.fingerprint() == chosen).filter(|_| ctx.book.phrases.len() > 1) {
                        ui.label(RichText::new(format!("from {}", ctx.book.name_of(index))).small().color(palette.soft));
                    }
                });
            });
        }

        match request.asked() {
            Ok(Asked::Transaction(tx)) => {
                let units = |value: chainv4::structs::u256::u256| format::units(&value.to_string(), 18);
                super::row(ui, &palette, "Sends", |ui| {
                    ui.label(RichText::new(units(tx.value)).strong()).on_hover_text(format!("{} units, at 18 decimals", tx.value));
                });

                match tx.to {
                    Some(to) => super::row(ui, &palette, "To", |ui| super::address(ui, &palette, &chainv4::structs::hex::to_hex(&to))),
                    None => super::row(ui, &palette, "To", |ui| {
                        ui.label("a new contract, from its code");
                    }),
                }

                if !tx.input.is_empty() {
                    super::row(ui, &palette, "Data", |ui| {
                        ui.label(theme::mono(format!("{} · {} bytes", chainv4::structs::hex::to_hex(&tx.input[..tx.input.len().min(4)]), tx.input.len())));
                    });
                }

                let most = chainv4::structs::u256::u256::from(tx.gas_limit) * tx.max_price;
                super::row(ui, &palette, "Fee, at most", |ui| {
                    ui.label(units(most)).on_hover_text(format!("{} gas at up to {} units, tip {}", tx.gas_limit, tx.max_price, tx.tip));
                });
                super::row(ui, &palette, "Nonce", |ui| {
                    ui.label(tx.nonce.to_string());
                });
            }
            Ok(Asked::TypedData(data, json)) => {
                super::row(ui, &palette, "Signs", |ui| {
                    ui.label(format!("typed data: {}", data.primary));
                });

                for field in ["name", "version", "verifyingContract"] {
                    if let Some(value) = json["domain"].get(field).filter(|value| !value.is_null()) {
                        super::row(ui, &palette, field, |ui| {
                            ui.label(theme::mono(value.to_string().trim_matches('"')));
                        });
                    }
                }

                super::text_block(ui, &palette, &serde_json::to_string_pretty(&json["message"]).unwrap_or_default());
            }
            Ok(Asked::Message(message)) => {
                super::row(ui, &palette, "Signs", |ui| {
                    ui.label("a message");
                });

                match std::str::from_utf8(&message).ok().filter(|text| !text.chars().any(|c| c.is_control() && !c.is_whitespace())) {
                    Some(text) => {
                        ui.label(RichText::new(text).color(palette.ink));
                    }
                    None => {
                        ui.label(theme::mono(chainv4::structs::hex::to_hex(&message)));
                    }
                }
            }
            Err(_) => {}
        }

        ui.add_space(6.0);
        ui.label(RichText::new("Check each against what you meant: what is signed here counts wherever the chain it names counts it.").small().color(palette.warn));

        if let Some(trouble) = trouble.as_ref() {
            ui.add_space(6.0);
            ui.colored_label(palette.bad, trouble.as_str());
        }

        ui.add_space(12.0);
        ui.horizontal_wrapped(|ui| {
            let tapped = theme::primary(ui, &palette, "Sign", trouble.is_none() && state.confirming.is_none()).clicked();
            let biometric = biometrics::enrolled(ctx.wake, ctx.settings);

            if tapped && !confirmed && let Some((enrolled, _)) = biometric {
                state.confirming = Some(Check::start(ctx.wake, &enrolled, "Sign what MetaMask asks"));
            } else if tapped || confirmed {
                let signed = seed
                    .ok_or_else(|| "the words are gone from this wallet".to_string())
                    .and_then(|seed| request.sign(seed))
                    .and_then(|signature| signature.to_cbor());

                match signed {
                    Ok(cbor) => next = Some(Step::MetaMaskSigned { text: hardware::ur(hardware::SIGNATURE_TYPE, &cbor) }),
                    Err(why) => *trouble = Some(why),
                }
            }

            if theme::quiet(ui, &palette, "Cancel").clicked() {
                next = Some(Step::Reading);
                state.confirming = None;
            }
        });
    });

    if let Some(next) = next {
        state.step = next;
    }
}

/// A Bitcoin wallet's PSBT laid out: the network, large; each output, its amount and address, and
/// whether it pays this wallet back; and the fee.
fn bitcoin_review(ui: &mut Ui, ctx: &mut Ctx, state: &mut SignState) {
    let palette = *ctx.palette;
    let Step::Bitcoin { request, review, trouble } = &mut state.step else {
        return;
    };
    let mut next = None;
    let mut confirmed = false;

    if let Some(came) = state.confirming.as_ref().and_then(Check::take) {
        state.confirming = None;

        match came {
            Ok(_) => confirmed = true,
            Err(Failure::Canceled) => {}
            Err(Failure::Invalidated) => {
                biometrics::turn_off(ctx.wake, ctx.settings);
                *ctx.lock = Some(biometrics::CHANGED.to_string());
            }
            Err(Failure::Failed(why)) => *trouble = Some(why),
        }
    }

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "A Bitcoin wallet asks");
        ui.add_space(4.0);

        if let Some(review) = review.as_deref() {
            let network = review.network;
            let coins = |value: u64| format::amount(&value.to_string(), 8, network.symbol());
            theme::fitted(ui, network.name(), 34.0, 16.0, egui::FontFamily::Name(theme::SERIF.into()), palette.ink);

            match network {
                bitcoin::Network::Main => ui.label(RichText::new("Real bitcoin: what this signs spends it.").small().color(palette.muted)),
                bitcoin::Network::Test => ui.colored_label(palette.warn, "A test network's coins, worth nothing: testnet, signet or regtest."),
            };
            ui.add_space(8.0);

            for paid in &review.outputs {
                let label = match paid.yours {
                    Some(_) => "Change",
                    None => "Sends",
                };
                super::row(ui, &palette, label, |ui| {
                    ui.vertical(|ui| {
                        ui.label(RichText::new(coins(paid.value)).strong()).on_hover_text(format!("{} satoshis", paid.value));
                        super::address(ui, &palette, &paid.to);

                        if let Some(path) = &paid.yours {
                            ui.label(RichText::new(format!("back to this wallet, {path}")).small().color(palette.muted));
                        }
                    });
                });
            }

            super::row(ui, &palette, "Fee", |ui| {
                let rate = review.vsize.map(|vsize| format!(" · about {} sat/vB", review.fee.div_ceil(vsize))).unwrap_or_default();
                ui.label(format!("{}{rate}", coins(review.fee))).on_hover_text(format!("{} satoshis", review.fee));
            });

            let yours = review.inputs.iter().filter(|spent| spent.yours.is_some()).count();
            super::row(ui, &palette, "Spends", |ui| {
                let spent: u64 = review.inputs.iter().map(|spent| spent.value).sum();
                let others = match review.inputs.len() - yours {
                    0 => String::new(),
                    others => format!(", {others} not this wallet's to sign"),
                };
                ui.label(format!("{} from {} input{}{others}", coins(spent), review.inputs.len(), if review.inputs.len() == 1 { "" } else { "s" }));
            });

            // A swap's HTLC, claimed with its secret or taken back after its lock time.
            for spent in &review.inputs {
                match spent.swap {
                    Some(bitcoin::Swapped::Claim) => {
                        ui.label(RichText::new("It claims a swap's bitcoin for this wallet's key, with the swap's secret.").color(palette.soft));
                    }
                    Some(bitcoin::Swapped::Refund { locktime }) => {
                        ui.label(RichText::new(format!("It takes back a swap's bitcoin, which Bitcoin mines once its time is past {locktime}, in seconds since 1970.")).color(palette.soft));
                    }
                    None => {}
                }
            }

            if review.lock_time != 0 {
                super::row(ui, &palette, "Not before", |ui| {
                    ui.label(match review.lock_time < 500_000_000 {
                        true => format!("block {}", review.lock_time),
                        false => format!("time {}, in seconds since 1970", review.lock_time),
                    });
                });
            }

            if review.costly() {
                ui.add_space(4.0);
                ui.colored_label(palette.bad, format!("The fee is more than a tenth of what it spends, while it sends {} to others: check it.", coins(review.sent())));
            }

            if review.unproven {
                ui.add_space(4.0);
                ui.colored_label(
                    palette.warn,
                    "The wallet left out the transactions these coins came in, which prove their amounts: the fee shown is its word. Sign the same coins once only, or a wallet that lies could make the fee more.",
                );
            }

            ui.add_space(6.0);
            ui.label(RichText::new("Check each address against what you meant to pay: Bitcoin runs what is signed here, and none of it comes back.").small().color(palette.warn));
        }

        if let Some(trouble) = trouble.as_ref() {
            ui.add_space(6.0);
            ui.colored_label(palette.bad, trouble.as_str());
        }

        ui.add_space(12.0);
        ui.horizontal_wrapped(|ui| {
            let tapped = theme::primary(ui, &palette, "Sign", review.is_some() && trouble.is_none() && state.confirming.is_none()).clicked();
            let biometric = biometrics::enrolled(ctx.wake, ctx.settings);

            if tapped && !confirmed && let Some((enrolled, _)) = biometric {
                state.confirming = Some(Check::start(ctx.wake, &enrolled, "Sign what the Bitcoin wallet asks"));
            } else if tapped || confirmed {
                let seeds = crate::keys::seeds(ctx.book);
                let signed = match seeds.is_empty() {
                    true => Err("the words are gone from this wallet".to_string()),
                    false => request.psbt.sign_all(&seeds),
                };

                match signed {
                    Ok(psbt) => next = Some(Step::BitcoinSigned { message: Box::new(Message::Bitcoin(Box::new(bitcoin::Request { psbt, kind: request.kind }))) }),
                    Err(why) => *trouble = Some(why),
                }
            }

            if theme::quiet(ui, &palette, "Cancel").clicked() {
                next = Some(Step::Reading);
                state.confirming = None;
            }
        });
    });

    if let Some(next) = next {
        state.step = next;
    }
}
