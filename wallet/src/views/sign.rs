//! An offline signer's page: a request read off the sender's screen, all it asks laid out to check,
//! and once signed, the signed transaction as a code for the sender to read back. Nothing here
//! talks to a node: the request says the nonce and fee, and the sender proves the rest. With
//! biometrics on, signing waits for one more check.
//!
//! MetaMask's requests read here too, once it paired with the signer as its QR hardware wallet:
//! a transaction, typed data or a message, for any chain, the chain named large, and the signature
//! back as a code for MetaMask to read.

use {
    super::Ctx,
    super::scan::Scan,
    crate::airgap::{self, Message, Request},
    crate::format,
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
    /// MetaMask's request, read, to check before signing; why it cannot be signed, if it cannot.
    MetaMask { request: Box<SignRequest>, trouble: Option<String> },
    /// Signed for MetaMask: the code it reads back.
    MetaMaskSigned { text: String },
}

pub fn show(ui: &mut Ui, ctx: &mut Ctx, state: &mut SignState) {
    let palette = *ctx.palette;
    theme::title(ui, &palette, "Sign", "A request from the watch-only sender: read it, check it, sign it, and show it back.");

    match &state.step {
        Step::Reading => reading(ui, ctx, state),
        Step::Review { .. } => review(ui, ctx, state),
        Step::MetaMask { .. } => metamask(ui, ctx, state),
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
                let trouble = match crate::keys::seed(ctx.book) {
                    None => Some("This wallet holds no words, only keys brought in: MetaMask signs with accounts words make.".to_string()),
                    Some(seed) if request.path.fingerprint.is_some_and(|asked| asked != seed.ethereum_account().master_fingerprint) => {
                        Some("A request for another wallet's keys: pair MetaMask with this one on Pair.".to_string())
                    }
                    Some(_) => request.account().and_then(|_| request.asked()).err(),
                };
                state.step = Step::MetaMask { request, trouble };
            }
            Some(Message::Sign(request)) => {
                let trouble = match ctx.holders.iter().find(|holder| holder.publics.contains(&request.signer)) {
                    Some(holder) if holder.keys.is_empty() => Some("This wallet holds that account's public keys alone.".to_string()),
                    Some(_) => None,
                    None => Some(format!("No account in this wallet signs for {}: pair the sender with this wallet's accounts.", request.signer.address())),
                };
                state.step = Step::Review { request: Box::new(request), trouble };
            }
            Some(other) => state.scan.refuse(&other, "a request to sign, this wallet's or MetaMask's"),
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
        ui.horizontal(|ui| {
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

/// MetaMask's request laid out: the chain, large; the account; and what it signs.
fn metamask(ui: &mut Ui, ctx: &mut Ctx, state: &mut SignState) {
    let palette = *ctx.palette;
    let Step::MetaMask { request, trouble } = &mut state.step else {
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

    let yours = ctx.settings.signer_chain;
    let chain = request.chain();
    let seed = crate::keys::seed(ctx.book);

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, &format!("MetaMask asks{}", request.origin.as_deref().map(|origin| format!(" · {origin}")).unwrap_or_default()));
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

        if let (Ok(account), Some(seed)) = (request.account(), &seed) {
            let address = seed.key(chainv4::crypto::keypair::KeyType::Secp256k1, account).public().address();
            super::row(ui, &palette, "Account", |ui| {
                ui.vertical(|ui| {
                    super::address(ui, &palette, &address.to_string());
                    ui.label(RichText::new(format!("m/{}", request.path.text())).small().color(palette.muted));
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
        ui.horizontal(|ui| {
            let tapped = theme::primary(ui, &palette, "Sign", trouble.is_none() && state.confirming.is_none()).clicked();
            let biometric = biometrics::enrolled(ctx.wake, ctx.settings);

            if tapped && !confirmed && let Some((enrolled, _)) = biometric {
                state.confirming = Some(Check::start(ctx.wake, &enrolled, "Sign what MetaMask asks"));
            } else if tapped || confirmed {
                let signed = seed
                    .as_ref()
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
