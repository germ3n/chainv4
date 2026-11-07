//! An offline signer's page: a request read off the sender's screen, all it asks laid out to check,
//! and once signed, the signed transaction as a code for the sender to read back. Nothing here
//! talks to a node: the request says the nonce, the fee and the chain, shown among the rest and held
//! against the one set in Settings, and the sender proves the rest. With biometrics on, signing waits
//! for one more check.
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
    chainv4::structs::tx::Tx,
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
    /// A multisig's call on an EVM chain, read, to sign as one of its contract's owners with the
    /// key it names, or as an owner of another multisig that owns it.
    Evm { request: Box<chainv4::abi::multisig::EvmRequest>, trouble: Option<String> },
}

pub fn show(ui: &mut Ui, ctx: &mut Ctx, state: &mut SignState) {
    let palette = *ctx.palette;
    theme::title(ui, &palette, "Sign", "A request from the watch-only sender: read it, check it, sign it, and show it back.");

    match &state.step {
        Step::Reading => {
            reading(ui, ctx, state);
            history(ui, ctx);
        }
        Step::Review { .. } => review(ui, ctx, state),
        Step::MetaMask { .. } => metamask(ui, ctx, state),
        Step::Bitcoin { .. } => bitcoin_review(ui, ctx, state),
        Step::Multisig { .. } => multisig_review(ui, ctx, state),
        Step::Evm { .. } => evm_review(ui, ctx, state),
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

/// Notes what was just signed in the wallet's sealed history, unless that is off.
fn note(ctx: &mut Ctx, kind: &str, what: &str, signer: &str) {
    if ctx.book.note_signed(ctx.now, kind, what, signer) {
        *ctx.changed = true;
    }
}

/// Which chain a signature here counts on, among the facts to check, as a hostile sender may name
/// another: its id, the one Settings takes, then the genesis id that comes from. Never one of the
/// names of Ethereum's chains MetaMask's requests go by: an id here is the first bytes of a genesis
/// id, which a sender could choose so as to read as one.
fn chain_row(ui: &mut Ui, palette: &theme::Palette, genesis: &chainv4::structs::hash::Hash, yours: Option<u64>) {
    let chain = chainv4::crypto::ethereum::chain_id(genesis);
    let name = if yours == Some(chain) { "your chain" } else { "another chain than yours" };

    super::row(ui, palette, "For the chain", |ui| {
        ui.vertical(|ui| {
            if yours.is_some() {
                ui.label(RichText::new(name).strong());
            }

            ui.label(RichText::new(format!("chain id {chain}")).strong());
            super::address(ui, palette, &genesis.to_string());

            // Nothing to hold it against: what was said above is the sender's word alone.
            if yours.is_none() {
                ui.label(RichText::new("The sender says so. Set your chain's id in Settings, and this signer refuses any other.").small().color(palette.warn));
            }
        });
    });
}

/// What a history line says of what was signed, with the chain it counts on.
fn on_chain(what: &str, chain: u64) -> String {
    format!("{what} on chain {chain}").trim().to_string()
}

/// How a native transaction reads in the history: its kind, and what it does.
fn native_line(described: &airgap::Described) -> (String, String) {
    // A batch's steps are too many for a line, and the page that signed it laid each out.
    let what = match &described.steps {
        Some(steps) if steps.count == 1 => format!("1 step, {} in all", steps.total.0),
        Some(steps) => format!("{} steps, {} in all", steps.count, steps.total.0),
        None => described.what.as_ref().map_or(String::new(), |(shown, _)| shown.clone()),
    };
    let to = described.to.as_deref().map_or(String::new(), |to| format!(" to {}", format::short(to)));

    (described.kind.clone(), format!("{what}{to}").trim().to_string())
}

/// How MetaMask's request reads in the history: only what kind it was and where, as a message or
/// typed data may hold anything, and a transaction's own words are on the page that signed it.
fn metamask_line(request: &SignRequest) -> String {
    let kind = match request.asked() {
        Ok(Asked::Transaction(_)) => "A transaction",
        Ok(Asked::TypedData(..)) => "Typed data",
        Ok(Asked::Message(_)) => "A message",
        Err(_) => "A request",
    };

    match request.chain() {
        Some(chain) => format!("{kind} on chain {chain}"),
        None => format!("{kind}, naming no chain"),
    }
}

/// How a PSBT reads in the history: what left the wallet, and the fee.
fn bitcoin_line(review: &Review) -> String {
    format!("{} sats sent, {} sats fee", format::thousands(&review.sent().to_string()), format::thousands(&review.fee.to_string()))
}

/// What this signer signed, newest first, folded until asked for: anyone holding it unlocked would
/// otherwise see it on the page they sign from.
fn history(ui: &mut Ui, ctx: &Ctx) {
    let palette = *ctx.palette;

    if ctx.book.signed.is_empty() {
        return;
    }

    ui.add_space(chainv4_ui::layout::SECTION);
    theme::card(ui, &palette, |ui| {
        egui::CollapsingHeader::new(RichText::new(format!("Signed here ({})", ctx.book.signed.len())).color(palette.soft)).id_salt("signed-here").show(ui, |ui| {
            for line in ctx.book.signed.iter().rev() {
                ui.add(egui::Label::new(RichText::new(if line.what.is_empty() { &line.kind } else { &line.what }).color(palette.ink)).wrap());
                ui.add(egui::Label::new(RichText::new(format!("{} · {} · {}", line.kind, format::short(&line.signer), format::utc(line.at))).small().color(palette.muted)).wrap());
                ui.add_space(6.0);
            }
        });
    });
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
                // A batch the chain would refuse is nothing to look over and sign.
                let trouble = airgap::refused(&request).or(trouble);
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
                // Checked as the multisig's own: its steps take from it alone.
                let held = Tx::Multisig { account: bundle.body.account, tx: Box::new(bundle.body.tx.clone()), signatures: Vec::new() };
                let refused = airgap::refused(&airgap::Request {
                    tx: held,
                    nonce: bundle.body.nonce,
                    fee: bundle.body.fee,
                    signer: chainv4::crypto::keypair::PublicKey::Ed25519([0; 32]),
                    coin: airgap::Shown { symbol: String::new(), decimals: 0 },
                    token: None,
                    collection: None,
                    genesis: bundle.genesis,
                });
                let trouble = refused.or(trouble);
                state.step = Step::Multisig { bundle, key, trouble };
            }
            Some(Message::Evm(request)) => {
                let held = ctx.holders.iter().flat_map(|holder| holder.keys.iter()).any(|key| key.public() == request.signer);
                let trouble = (!held).then(|| format!("This wallet holds no key {}: the request is for another's.", format::short(&request.signer.to_string())));
                state.step = Step::Evm { request, trouble };
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
    let yours = ctx.settings.signer_chain;
    // Held against the chain Settings names as it stands now, not as it stood when this was read.
    let wrong = airgap::wrong_chain(&request.genesis, yours);
    let mut next = None;

    // What the check before signing came to: passed, it signs; the fingers or faces enrolled
    // changed since, the wallet locks.
    let mut confirmed = false;

    if let Some(came) = state.confirming.as_ref().and_then(Check::take) {
        state.confirming = None;

        match came {
            Ok(_) => confirmed = true,
            Err(Failure::Canceled) => {}
            Err(Failure::Invalidated) => {
                *ctx.lock = Some(biometrics::invalidated(ctx.wake, ctx.settings));
            }
            Err(Failure::Failed(why)) => *trouble = Some(why),
        }
    }

    // A single transaction is laid out in the same frame a batch's step is, so its amount,
    // recipient and data all show, not a one-line summary that a compromised sender could lean on.
    let single = described.steps.is_none().then(|| airgap::laid(request));

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, &described.kind);
        ui.add_space(4.0);

        // A batch's count and total say what the steps come to, before any of them.
        if let Some(steps) = &described.steps {
            super::steps::heading(ui, ctx, steps);
        }

        chain_row(ui, &palette, &request.genesis, yours);
        super::row(ui, &palette, "From", |ui| super::address(ui, &palette, &described.from));
        super::row(ui, &palette, "Fee, at most", |ui| {
            ui.label(&described.fee).on_hover_text(&described.gas);
        });
        super::row(ui, &palette, "Nonce", |ui| {
            ui.label(described.nonce.to_string());
        });

        // Then the transaction itself: every step of a batch, or the lone one in its own frame.
        ui.add_space(6.0);

        match (&described.steps, &single) {
            (Some(steps), _) => super::steps::each(ui, ctx, steps),
            // A kind the step frame lays out in full: the common ones, with amount, recipient and data.
            (None, Some(single)) if single.steps[0].allowed => super::steps::single(ui, ctx, &single.steps[0]),
            // A kind it has no better layout for, such as a validator's or governance's: its
            // recipient if it names one, and the rest in "Everything it holds" below.
            _ => {
                if let Some(to) = &described.to {
                    super::row(ui, &palette, "To", |ui| super::address(ui, &palette, to));
                }
            }
        }

        ui.add_space(6.0);
        ui.label(
            RichText::new("Check each against what you meant to send: the chain runs what is signed here. Names and decimals are the sender's word; the units and addresses are what is signed.")
                .small()
                .color(palette.warn),
        );
        egui::CollapsingHeader::new(RichText::new("Everything it holds").color(palette.soft)).id_salt("everything").show(ui, |ui| {
            ui.add(egui::Label::new(theme::mono(&described.everything).color(palette.soft)).wrap());
        });

        if let Some(why) = wrong.as_ref().or(trouble.as_ref()) {
            ui.add_space(6.0);
            ui.colored_label(palette.bad, why.as_str());
        }

        ui.add_space(12.0);
        ui.horizontal_wrapped(|ui| {
            let tapped = theme::primary(ui, &palette, "Sign", wrong.is_none() && trouble.is_none() && state.confirming.is_none()).clicked();
            let biometric = biometrics::enrolled(ctx.wake, ctx.settings);

            if tapped && !confirmed && let Some((enrolled, _)) = biometric {
                state.confirming = Some(Check::start(ctx.wake, &enrolled, "Sign this transaction"));
            } else if tapped || confirmed {
                let signed = ctx
                    .holders
                    .iter()
                    .find_map(|holder| holder.keys.iter().find(|key| key.public() == request.signer))
                    .ok_or_else(|| "the key is gone from this wallet".to_string())
                    .and_then(|key| airgap::sign(request, key, yours));

                match signed {
                    Ok(signed) => {
                        let (kind, what) = native_line(&described);
                        note(ctx, &kind, &on_chain(&what, described.chain), &described.from);
                        next = Some(Step::Signed { message: Box::new(Message::Signed(signed)) });
                    }
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
/// A multisig's call on an EVM chain, laid out before the key it names signs it: the chain, the
/// contract, whom it calls with what, its nonce, and whose approval it is when it is another
/// multisig's.
fn evm_review(ui: &mut Ui, ctx: &mut Ctx, state: &mut SignState) {
    let palette = *ctx.palette;
    let Step::Evm { request, trouble } = &mut state.step else {
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
                *ctx.lock = Some(biometrics::invalidated(ctx.wake, ctx.settings));
            }
            Err(Failure::Failed(why)) => *trouble = Some(why),
        }
    }

    let bundle = &request.bundle;
    let hex = chainv4::structs::hex::to_hex;

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "A multisig's call on an EVM chain");
        super::row(ui, &palette, "Chain", |ui| {
            ui.label(chainv4::swap::evm::chain_name(bundle.chain_id));
        });
        super::row(ui, &palette, "Its contract", |ui| super::address(ui, &palette, &hex(&bundle.contract)));
        super::row(ui, &palette, "Calls", |ui| super::address(ui, &palette, &hex(&bundle.to)));
        super::row(ui, &palette, "Sends", |ui| {
            ui.label(format::amount(&bundle.value.to_string(), 18, chainv4::swap::evm::symbol(bundle.chain_id)));
        });

        if !bundle.data.is_empty() {
            super::row(ui, &palette, "Data", |ui| {
                ui.label(theme::mono(format!("{} · {} bytes", hex(&bundle.data[..bundle.data.len().min(4)]), bundle.data.len())));
            });
        }

        super::row(ui, &palette, "At nonce", |ui| {
            ui.label(bundle.nonce.to_string());
        });
        super::row(ui, &palette, "Signs as", |ui| super::address(ui, &palette, &request.signer.to_string()));

        if let Some(owner) = request.owner {
            ui.label(RichText::new(format!("As an owner of {}, another multisig's contract, which is one of this call's owners: the signature approves the call for it.", format::short(&hex(&owner)))).color(palette.warn));
        }

        egui::CollapsingHeader::new(RichText::new("The data in full").color(palette.soft)).id_salt("evm-call-data").show(ui, |ui| {
            ui.add(egui::Label::new(theme::mono(hex(&bundle.data)).color(palette.soft)).wrap());
        });

        if let Some(why) = trouble.as_ref() {
            ui.colored_label(palette.bad, why.as_str());
        }

        ui.add_space(12.0);
        ui.horizontal_wrapped(|ui| {
            let tapped = theme::primary(ui, &palette, "Sign", trouble.is_none() && state.confirming.is_none()).clicked();
            let biometric = biometrics::enrolled(ctx.wake, ctx.settings);

            if tapped && !confirmed && let Some((enrolled, _)) = biometric {
                state.confirming = Some(Check::start(ctx.wake, &enrolled, "Sign this multisig call"));
            } else if tapped || confirmed {
                match ctx.holders.iter().find_map(|holder| holder.keys.iter().find(|held| held.public() == request.signer)) {
                    Some(keypair) => match request.sign(keypair) {
                        Ok(signed) => {
                            note(ctx, &format!("Multisig on {} · call", chainv4::swap::evm::chain_name(bundle.chain_id)), &hex(&bundle.to), &request.signer.address().to_string());
                            let answer = chainv4::abi::multisig::EvmRequest { bundle: signed, ..(**request).clone() };
                            next = Some(Step::Signed { message: Box::new(Message::Evm(Box::new(answer))) });
                        }
                        Err(why) => *trouble = Some(why),
                    },
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
                *ctx.lock = Some(biometrics::invalidated(ctx.wake, ctx.settings));
            }
            Err(Failure::Failed(why)) => *trouble = Some(why),
        }
    }

    let yours = ctx.settings.signer_chain;
    let wrong = airgap::wrong_chain(&bundle.genesis, yours);
    let request = airgap::Request {
        tx: bundle.body.tx.clone(),
        nonce: bundle.body.nonce,
        fee: bundle.body.fee,
        signer: key.unwrap_or(chainv4::crypto::keypair::PublicKey::Ed25519([0; 32])),
        coin: airgap::Shown { symbol: "units".to_string(), decimals: 0 },
        token: None,
        collection: None,
        genesis: bundle.genesis,
    };
    let described = airgap::describe(&request);
    // A non-batch inner transaction is laid out in the step frame too, so a member sees its value,
    // recipient and calldata, not a one-line summary of a transaction that spends the multisig.
    let single = described.steps.is_none().then(|| airgap::laid(&request));

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, &format!("A multisig's transaction: {}", described.kind));

        if let Some(steps) = &described.steps {
            super::steps::heading(ui, ctx, steps);
        }

        super::row(ui, &palette, "Multisig", |ui| super::address(ui, &palette, &bundle.body.account.to_string()));
        super::row(ui, &palette, "Fee, at most", |ui| {
            ui.label(&described.fee).on_hover_text(&described.gas);
        });
        super::row(ui, &palette, "Nonce", |ui| {
            ui.label(bundle.body.nonce.to_string());
        });
        chain_row(ui, &palette, &bundle.genesis, yours);
        super::row(ui, &palette, "Signed by", |ui| {
            ui.label(format!("{} member{} so far", bundle.signatures.len(), if bundle.signatures.len() == 1 { "" } else { "s" }));
        });

        // The transaction itself, as the multisig will run it, before a member's key goes on it.
        ui.add_space(6.0);

        match (&described.steps, &single) {
            (Some(steps), _) => super::steps::each(ui, ctx, steps),
            (None, Some(single)) if single.steps[0].allowed => super::steps::single(ui, ctx, &single.steps[0]),
            _ => {
                if let Some(to) = &described.to {
                    super::row(ui, &palette, "To", |ui| super::address(ui, &palette, to));
                }
            }
        }

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

        if let Some(why) = wrong.as_ref().or(trouble.as_ref()) {
            ui.colored_label(palette.bad, why.as_str());
        }

        ui.add_space(12.0);
        ui.horizontal_wrapped(|ui| {
            let tapped = theme::primary(ui, &palette, "Sign", wrong.is_none() && trouble.is_none() && key.is_some() && state.confirming.is_none()).clicked();
            let biometric = biometrics::enrolled(ctx.wake, ctx.settings);

            if tapped && !confirmed && let Some((enrolled, _)) = biometric {
                state.confirming = Some(Check::start(ctx.wake, &enrolled, "Sign this multisig transaction"));
            } else if tapped || confirmed {
                match ctx.holders.iter().find_map(|holder| holder.keys.iter().find(|held| Some(held.public()) == *key)) {
                    Some(keypair) => {
                        let mut signed = (**bundle).clone();
                        signed.sign(keypair);
                        let (kind, what) = native_line(&described);
                        let member = keypair.public().address().to_string();
                        note(ctx, &format!("Multisig {} · {kind}", format::short(&bundle.body.account.to_string())), &on_chain(&what, described.chain), &member);
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
    let mut noted = None;
    let mut next = None;
    let mut confirmed = false;

    if let Some(came) = state.confirming.as_ref().and_then(Check::take) {
        state.confirming = None;

        match came {
            Ok(_) => confirmed = true,
            Err(Failure::Canceled) => {}
            Err(Failure::Invalidated) => {
                *ctx.lock = Some(biometrics::invalidated(ctx.wake, ctx.settings));
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

                    // This page talks to no node: what reads it is an ABI built in or saved here.
                    let kept = tx.to.as_ref().and_then(|to| super::decoded::kept_for(ctx.settings, to));

                    if let Some(read) = ctx.reader.call(kept, &tx.input) {
                        super::decoded::signing(ui, &palette, None, &read);
                    }

                    super::decoded::raw(ui, &palette, "signed data", &chainv4::structs::hex::to_hex(&tx.input));
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

                // Only the fields the type declares are signed; a site may add others to the
                // message that are not part of the digest, so show just the signed ones.
                super::text_block(ui, &palette, &serde_json::to_string_pretty(&data.signed_message()).unwrap_or_default());
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
                    Ok(cbor) => {
                        let account = request.address.map_or_else(|| format!("m/{}", request.path.text()), |address| chainv4::structs::hex::to_hex(&address));
                        noted = Some((request.asker(), metamask_line(request), account));
                        next = Some(Step::MetaMaskSigned { text: hardware::ur(hardware::SIGNATURE_TYPE, &cbor) });
                    }
                    Err(why) => *trouble = Some(why),
                }
            }

            if theme::quiet(ui, &palette, "Cancel").clicked() {
                next = Some(Step::Reading);
                state.confirming = None;
            }
        });
    });

    if let Some((kind, what, signer)) = noted {
        note(ctx, &kind, &what, &signer);
    }

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
                *ctx.lock = Some(biometrics::invalidated(ctx.wake, ctx.settings));
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
                    Ok(psbt) => {
                        note(ctx, "Bitcoin", &review.as_deref().map_or_else(|| "a transaction".to_string(), bitcoin_line), "this wallet's words");
                        next = Some(Step::BitcoinSigned { message: Box::new(Message::Bitcoin(Box::new(bitcoin::Request { psbt, kind: request.kind }))) });
                    }
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

#[cfg(test)]
mod signed_history {
    use {
        super::{super::drawn, *},
        crate::vault::Book,
    };

    #[test]
    fn what_a_transaction_did_reads_in_a_line_and_what_a_request_merely_says_stays_out() {
        let described = airgap::Described {
            kind: "Send the coin".to_string(),
            what: Some(("1.5 CV4".to_string(), "1500000000000000000".to_string())),
            from: "ed25519:0x01".to_string(),
            to: Some("ed25519:0x02020202020202020202".to_string()),
            token: None,
            fee: String::new(),
            gas: String::new(),
            nonce: 3,
            everything: "the words of a message".to_string(),
            steps: None,
            chain: 7,
        };
        let (kind, what) = native_line(&described);

        assert_eq!(kind, "Send the coin");
        assert!(what.starts_with("1.5 CV4 to "), "{what}");
        assert!(!what.contains("words of a message"));
    }

    #[test]
    fn the_sign_page_folds_what_was_signed_and_says_nothing_with_none() {
        let mut empty = Book::default();
        let mut holders = Vec::new();
        let said = drawn::said(&mut empty, &mut holders, (360.0, 740.0), |ui, ctx| history(ui, ctx));
        assert!(!said.contains("Signed here"), "{said}");

        let mut book = Book::default();
        book.note_signed(1_793_000_000_000, "Send the coin", "1.5 CV4 to bob", "ed25519:0x0101010101");
        let said = drawn::said(&mut book, &mut holders, (360.0, 740.0), |ui, ctx| history(ui, ctx));
        assert!(said.contains("Signed here (1)"), "{said}");
        assert!(!said.contains("1.5 CV4"), "folded until asked for: {said}");
    }
}

#[cfg(test)]
mod batch_tests {
    use {
        super::*,
        crate::keys,
        crate::views::drawn,
        chainv4::registry::multisig::{MultisigBody, MultisigBundle},
        chainv4::structs::address::Address,
        chainv4::structs::amount::Amount,
        chainv4::structs::hash::{Hash, HashType},
        chainv4::structs::tx::FeeOffer,
        chainv4::structs::u256::u256,
    };

    fn fee() -> FeeOffer {
        FeeOffer { gas_limit: 50_000, max_price: u256::new(2_000_000_000), tip: u256::ZERO }
    }

    /// The signer's page, on a screen this wide, showing this step, and what it drew.
    fn drawn_sign(width: f32, step: impl Fn(&[Address]) -> Step) -> Vec<(String, egui::Rect)> {
        let mut book = drawn::two_phrases();
        let mut holders = keys::open(&book).unwrap();
        let from = holders[0].addresses();
        let mut state = SignState { step: step(&from), ..SignState::default() };
        let (drawn_width, _, shown) = drawn::page_shown(&mut book, &mut holders, (width, 20_000.0), |ui, ctx| show(ui, ctx, &mut state));

        assert!(drawn_width <= width + 1.0, "{drawn_width} on {width}");

        shown
    }

    fn top(shown: &[(String, egui::Rect)], said: &str) -> f32 {
        shown.iter().find(|(text, _)| text.contains(said)).map_or_else(|| panic!("{said} in {shown:?}"), |(_, rect)| rect.top())
    }

    #[test]
    fn the_offline_signer_shows_every_step_above_its_sign_button_and_the_count_and_total_above_them() {
        for (width, _) in drawn::SCREENS {
            let shown = drawn_sign(width, |from| {
                let pay = Tx::Transfer { from: from[0], to: Address::Ed25519([9; 32]), amount: Amount::U128(1_000_000_000_000_000_000) };
                let key = chainv4::crypto::keypair::Keypair::from_seed([7; 32]).public();

                Step::Review {
                    request: Box::new(Request { tx: Tx::Batch { steps: vec![pay; 3] }, nonce: 4, fee: fee(), signer: key, coin: airgap::Shown { symbol: "DEV".to_string(), decimals: 18 }, token: None, collection: None, genesis: Hash::zero(HashType::default_chain()) }),
                    trouble: None,
                }
            });

            assert!(top(&shown, "3 STEPS") < top(&shown, "Sends 3 DEV in all"));
            assert!(top(&shown, "Sends 3 DEV in all") < top(&shown, "Step 1 of 3"));

            let (first, last) = (top(&shown, "Step 1 of 3"), top(&shown, "Step 3 of 3"));
            let sign = shown.iter().rfind(|(text, _)| text == "Sign").unwrap().1.top();

            assert!(first < top(&shown, "Step 2 of 3") && last > first && last < sign, "every step above the button");
        }
    }

    #[test]
    fn a_multisigs_bundle_lists_every_step_on_the_offline_signer_too() {
        let account = Address::Multisig([6; 32]);

        for (width, _) in drawn::SCREENS {
            let shown = drawn_sign(width, |_| {
                let pay = Tx::Transfer { from: account, to: Address::Ed25519([9; 32]), amount: Amount::U128(5) };
                let bundle = MultisigBundle { genesis: Hash::zero(HashType::default_chain()), body: MultisigBody { account, tx: Tx::Batch { steps: vec![pay; 2] }, nonce: 1, fee: fee() }, signatures: Vec::new() };

                Step::Multisig { bundle: Box::new(bundle), key: Some(chainv4::crypto::keypair::Keypair::from_seed([7; 32]).public()), trouble: None }
            });
            let sign = shown.iter().rfind(|(text, _)| text == "Sign").unwrap().1.top();

            assert!(top(&shown, "2 STEPS") < top(&shown, "Step 1 of 2"));
            assert!(top(&shown, "Step 2 of 2") < sign);
            assert!(shown.iter().any(|(text, _)| text == "Sends 10 units in all"), "{shown:?}");
        }
    }

    #[test]
    fn the_offline_signer_shows_a_lone_calls_value_recipient_and_data_not_a_one_line_summary() {
        let shown = drawn_sign(360.0, |_| {
            let tx = Tx::EvmCall { to: Address::Evm([0xab; 20]), value: Amount::U128(2_000_000_000_000_000_000), input: vec![0x12, 0x34, 0x56, 0x78, 0x9a] };
            let key = chainv4::crypto::keypair::Keypair::from_seed([7; 32]).public();

            Step::Review { request: Box::new(Request { tx, nonce: 0, fee: fee(), signer: key, coin: airgap::Shown { symbol: "DEV".to_string(), decimals: 18 }, token: None, collection: None, genesis: Hash::zero(HashType::default_chain()) }), trouble: None }
        });
        let sign = shown.iter().rfind(|(text, _)| text == "Sign").unwrap().1.top();

        assert!(shown.iter().any(|(text, _)| text.contains("2 DEV")), "the value it signs shows, not just the kind: {shown:?}");
        assert!(shown.iter().any(|(text, _)| text == "To"), "the recipient shows");
        assert!(shown.iter().any(|(text, _)| text.contains("bytes")), "the calldata and its size show");
        assert!(top(&shown, "2 DEV") < sign, "above the sign button, not folded into a collapsed section");
    }

    #[test]
    fn the_offline_signer_shows_a_multisig_bundles_fee_and_chain_and_its_lone_calls_detail() {
        let account = Address::Multisig([6; 32]);
        let shown = drawn_sign(360.0, |_| {
            let tx = Tx::EvmCall { to: Address::Evm([0xcd; 20]), value: Amount::U128(7), input: vec![0xde, 0xad, 0xbe, 0xef] };
            let bundle = MultisigBundle { genesis: Hash::zero(HashType::default_chain()), body: MultisigBody { account, tx, nonce: 1, fee: fee() }, signatures: Vec::new() };

            Step::Multisig { bundle: Box::new(bundle), key: Some(chainv4::crypto::keypair::Keypair::from_seed([7; 32]).public()), trouble: None }
        });

        assert!(shown.iter().any(|(text, _)| text == "Fee, at most"), "the fee a member signs for shows: {shown:?}");
        assert!(shown.iter().any(|(text, _)| text == "For the chain"), "the bundle's chain shows");
        assert!(shown.iter().any(|(text, _)| text.contains("7 units")), "the inner call's value shows");
        assert!(shown.iter().any(|(text, _)| text.contains("bytes")), "the inner call's data shows");
    }

    /// What the review of a request for this chain says, on a signer told this is its own, and
    /// where it stands once tapped on Sign, with what its history noted.
    fn tapped_sign(genesis: Hash, yours: Option<u64>) -> (String, SignState, Vec<crate::vault::SignedLine>) {
        let mut book = drawn::two_phrases();
        let mut holders = keys::open(&book).unwrap();
        let from = holders[0].addresses()[0];
        let signer = holders[0].public(&from).unwrap();
        let tx = Tx::Transfer { from, to: Address::Ed25519([9; 32]), amount: Amount::U128(1) };
        let request = Request { tx, nonce: 0, fee: fee(), signer, coin: airgap::Shown { symbol: "DEV".to_string(), decimals: 18 }, token: None, collection: None, genesis };
        let mut state = SignState { step: Step::Review { request: Box::new(request), trouble: None }, ..SignState::default() };
        let mut world = drawn::World::default();
        world.settings.signer_chain = yours;
        let said = drawn::said_in(&mut world, &mut book, &mut holders, (360.0, 4_000.0), |ui, ctx| review(ui, ctx, &mut state));
        drawn::tap_in(&mut world, &mut book, &mut holders, (360.0, 4_000.0), "Sign", |ui, ctx| review(ui, ctx, &mut state));

        (said, state, book.signed)
    }

    #[test]
    fn the_offline_signer_shows_the_chain_and_will_not_sign_for_another_it_was_told_is_not_its_own() {
        let genesis = Hash::zero(HashType::default_chain());
        let chain = chainv4::crypto::ethereum::chain_id(&genesis);

        // Told nothing, it shows the chain as the sender's word, and signs.
        let (said, state, noted) = tapped_sign(genesis, None);

        assert!(said.contains(&format!("chain id {chain}")) && said.contains("The sender says so"), "{said}");
        assert!(matches!(state.step, Step::Signed { .. }), "{said}");
        assert!(noted[0].what.ends_with(&format!("on chain {chain}")), "its history says which: {noted:?}");

        // Told this one, it names it so, and signs.
        let (said, state, _) = tapped_sign(genesis, Some(chain));

        assert!(said.contains("your chain") && !said.contains("The sender says so"), "{said}");
        assert!(matches!(state.step, Step::Signed { .. }), "{said}");

        // Told another, the tap does nothing, and it says why.
        let (said, state, noted) = tapped_sign(genesis, Some(chain + 1));

        assert!(said.contains(&format!("not yours, {}", chain + 1)) && said.contains("another chain than yours"), "{said}");
        assert!(matches!(state.step, Step::Review { .. }), "{said}");
        assert!(noted.is_empty(), "nothing signed, nothing noted");
    }

    #[test]
    fn a_chain_id_a_sender_chose_to_read_as_ethereums_is_not_named_so() {
        // Its first bytes are the sender's to choose: this one's chain id is 1.
        let genesis = Hash::zero(HashType::default_chain());

        assert_eq!(chainv4::crypto::ethereum::chain_id(&genesis), 1);

        let (said, ..) = tapped_sign(genesis, None);

        assert!(said.contains("chain id 1") && !said.contains("Ethereum"), "{said}");
    }

    #[test]
    fn the_chain_of_a_request_shows_above_its_sign_button_on_every_screen() {
        for (width, _) in drawn::SCREENS {
            let shown = drawn_sign(width, |from| {
                let tx = Tx::Transfer { from: from[0], to: Address::Ed25519([9; 32]), amount: Amount::U128(1) };
                let signer = chainv4::crypto::keypair::Keypair::from_seed([7; 32]).public();

                Step::Review { request: Box::new(Request { tx, nonce: 0, fee: fee(), signer, coin: airgap::Shown { symbol: "DEV".to_string(), decimals: 18 }, token: None, collection: None, genesis: Hash::zero(HashType::default_chain()) }), trouble: None }
            });
            let sign = shown.iter().rfind(|(text, _)| text == "Sign").unwrap().1.top();

            assert!(top(&shown, "chain id") < sign);
        }
    }

    #[test]
    fn a_multisig_bundle_for_a_chain_this_signer_was_told_is_not_its_own_is_refused() {
        let account = Address::Multisig([6; 32]);
        let genesis = Hash::zero(HashType::default_chain());
        let tx = Tx::Burn { from: account, amount: Amount::U128(1) };
        let bundle = MultisigBundle { genesis, body: MultisigBody { account, tx, nonce: 1, fee: fee() }, signatures: Vec::new() };
        let mut book = drawn::two_phrases();
        let mut holders = keys::open(&book).unwrap();
        let key = holders[0].keys.first().map(|key| key.public());
        let mut state = SignState { step: Step::Multisig { bundle: Box::new(bundle), key, trouble: None }, ..SignState::default() };
        let mut world = drawn::World::default();
        world.settings.signer_chain = Some(chainv4::crypto::ethereum::chain_id(&genesis) + 1);
        let said = drawn::said_in(&mut world, &mut book, &mut holders, (360.0, 4_000.0), |ui, ctx| multisig_review(ui, ctx, &mut state));
        drawn::tap_in(&mut world, &mut book, &mut holders, (360.0, 4_000.0), "Sign", |ui, ctx| multisig_review(ui, ctx, &mut state));

        assert!(said.contains("not yours"), "{said}");
        assert!(matches!(state.step, Step::Multisig { .. }), "{said}");
    }

    #[test]
    fn a_request_the_chain_would_refuse_says_so_and_a_signature_is_not_offered() {
        let shown = drawn_sign(360.0, |_| {
            let refused = Request {
                tx: Tx::Batch { steps: vec![Tx::Mint { to: Address::Ed25519([9; 32]), amount: Amount::U128(1) }] },
                nonce: 0,
                fee: fee(),
                signer: chainv4::crypto::keypair::Keypair::from_seed([7; 32]).public(),
                coin: airgap::Shown { symbol: "DEV".to_string(), decimals: 18 },
                token: None,
                collection: None,
                genesis: Hash::zero(HashType::default_chain()),
            };
            let trouble = airgap::refused(&refused);

            assert!(trouble.is_some());

            Step::Review { request: Box::new(refused), trouble }
        });

        assert!(shown.iter().any(|(text, _)| text.contains("Not allowed in a batch: Mint")));
        assert!(shown.iter().any(|(text, _)| text.contains("The chain would refuse this batch")));
    }
}

