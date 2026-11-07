//! A native multisig's contract on EVM chains, for one whose members name EVM owners: a
//! MultiSig.sol at one address on every chain, its owners the members' EVM keys, Ed25519 keys or
//! contracts. The page shows the address, and on a chain Settings reaches, whether it is deployed,
//! what it holds, its owners and its nonce; a call by it is a bundle the owners sign, here with the
//! account's key, on the offline signer, as EIP-712 typed data for an EVM key or as the bundle for
//! an Ed25519 one, or with chainv4 multisig evm, and an account with an EVM key sends it once
//! enough signed, deploying the contract first. An owner that is another multisig's contract signs
//! by that one's members approving the call on its page. When the members change, it offers the
//! matching change of owners, which the owners before sign.

use {
    super::super::Ctx,
    super::super::evm::{Link, number, request, urls},
    super::super::scan::Scan,
    crate::airgap::Message,
    crate::format,
    crate::hardware::{DataKind, SignRequest},
    crate::theme,
    chainv4::abi::multisig::{EvmBundle, EvmRequest, change_owners_input, execute_signer, multisig_contract, multisig_deployment, multisig_read, multisig_reads},
    chainv4::crypto::keypair::{Keypair, PrivateKey, PublicKey},
    chainv4::registry::multisig::{EvmKind, EvmStart},
    chainv4::structs::address::Address,
    chainv4::structs::hex::{from_hex, to_hex},
    chainv4::structs::u256::u256,
    chainv4::swap::evm::{Call, DEPLOYMENT_PROXY, chain_name, symbol},
    egui::{RichText, Ui},
    serde_json::{Value, json},
};

/// The contract as a chain answered: its code, coin, and owners, threshold and nonce once deployed.
#[derive(Clone, Debug, PartialEq)]
pub struct Looked {
    pub chain_id: u64,
    pub deployed: bool,
    pub balance: u256,
    pub standing: Option<(EvmStart, u256)>,
}

pub enum Read {
    Looked(Result<Looked, String>),
}

#[derive(Default)]
pub struct EvmState {
    /// The multisig all this is of: what is found is dropped when another is picked.
    of: String,
    link: Link<Read>,
    chain_id: Option<u64>,
    looked: Option<Looked>,
    to: String,
    amount: String,
    data: String,
    bundle: Option<EvmBundle>,
    /// The offline signer, asked to sign the bundle or approve another's.
    asking: Option<(Asking, Scan)>,
    pasted: String,
    /// Another multisig's call this one's contract owns a share of, pasted to approve.
    foreign: String,
    approving: Option<EvmBundle>,
    /// What the transaction on its way is, to say what came of it once mined.
    sending: Option<Sending>,
    /// Whether the chain is being asked about the contract.
    looking: bool,
    /// Calls another page asked it to make, such as a swap's approval and lock: each becomes the
    /// bundle in turn, at the nonce the one before leaves.
    queued: Vec<Call>,
    note: Option<String>,
    trouble: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sending {
    Deploy,
    Call,
}

/// What the offline signer was shown: an EVM key's typed data, for this multisig's call or as an
/// owner of another's, or the bundle itself for an Ed25519 key.
enum Asking {
    Typed { request: Box<SignRequest>, owner: Option<[u8; 20]> },
    Evm(Box<EvmRequest>),
}

/// How this account is one of the contract's owners.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mine {
    Key([u8; 20]),
    Ed25519(PublicKey),
}

/// A multisig's side on EVM chains, as the node says it: the owners its contract starts with,
/// which its address commits to, the ones its members name now, and chainv4's chain id, which the
/// contract's init code names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Side {
    pub start: EvmStart,
    pub now: Option<EvmStart>,
    pub home: u64,
}

/// The multisig's side on EVM chains, from what the node says of it: the owners its contract
/// starts with, which its address commits to, and the ones its members name now.
impl EvmState {
    /// Takes calls another page made for a multisig's contract on a chain, such as a swap's lock.
    pub fn queue(&mut self, account: &str, chain_id: u64, calls: Vec<Call>) {
        *self = EvmState { of: account.to_string(), chain_id: Some(chain_id), queued: calls, ..EvmState::default() };
    }
}

pub fn side_of(value: &Value) -> Option<Side> {
    let evm = &value["evm"];

    Some(Side { start: chainv4::structs::json::evm_start(&evm["start"])?, now: chainv4::structs::json::evm_start(&evm["now"]), home: evm["home"].as_u64()? })
}

/// The contract's state from the batch's answers: code, balance, and the three reads.
pub fn looked(chain_id: u64, replies: &Value) -> Result<Looked, String> {
    let reply = |id: u64| -> Result<Value, String> {
        let reply = replies.as_array().and_then(|replies| replies.iter().find(|reply| reply["id"] == id)).ok_or("the RPC left a question unanswered")?;

        match reply.get("error").filter(|error| !error.is_null()) {
            Some(error) => Err(error["message"].as_str().map_or_else(|| error.to_string(), str::to_string)),
            None => Ok(reply["result"].clone()),
        }
    };
    let deployed = reply(1)?.as_str().is_some_and(|code| code.len() > 2);
    let balance = number(&reply(2)?)?;
    let bytes = |id: u64| reply(id).ok().and_then(|value| value.as_str().and_then(from_hex)).unwrap_or_default();
    let standing = match deployed {
        true => Some(multisig_read(&bytes(3), &bytes(4), &bytes(5)).ok_or("the code there is not a MultiSig.sol")?),
        false => None,
    };

    Ok(Looked { chain_id, deployed, balance, standing })
}

/// The owners, threshold and nonce a call is checked against: the contract's, or before it is
/// deployed, the ones it starts with at nonce 0.
fn standing(looked: &Looked, start: &EvmStart) -> (EvmStart, u256) {
    looked.standing.clone().unwrap_or((start.clone(), u256::ZERO))
}

/// How this account is one of the owners, if it is: by its EVM key, or by its Ed25519 key.
fn mine(ctx: &Ctx, start: &EvmStart, now: Option<&EvmStart>) -> Option<Mine> {
    let owners: Vec<_> = start.owners.iter().chain(now.into_iter().flat_map(|now| now.owners.iter())).copied().collect();
    let holder = ctx.holder()?;

    holder.publics.iter().find_map(|public| match (public, public.address()) {
        (_, Address::Secp256k1(bytes)) if owners.iter().any(|owner| owner.address == bytes && owner.kind == EvmKind::Key) => Some(Mine::Key(bytes)),
        (PublicKey::Ed25519(key), _) if owners.iter().any(|owner| owner.kind == EvmKind::Ed25519(*key)) => Some(Mine::Ed25519(*public)),
        _ => None,
    })
}

/// This account's EVM address, whatever it owns: what pays a chain's gas.
fn payer(ctx: &Ctx) -> Option<[u8; 20]> {
    ctx.holder()?.publics.iter().find_map(|public| match public.address() {
        Address::Secp256k1(bytes) => Some(bytes),
        _ => None,
    })
}

/// The key this wallet holds for an owner, to sign with here; None when only the offline signer
/// holds it.
fn held(ctx: &Ctx, mine: Mine) -> Option<Keypair> {
    match mine {
        Mine::Key(address) => super::super::dapp::secret(ctx, &address).map(|secret| Keypair::from_private(PrivateKey::Secp256k1(secret))),
        Mine::Ed25519(public) => ctx.holders.iter().flat_map(|holder| holder.keys.iter()).find(|key| key.public() == public).cloned(),
    }
}

/// Asks the offline signer to sign a bundle as an owner, or, with owner, as an owner of that
/// other multisig's contract: typed data for an EVM key, the bundle for an Ed25519 one.
fn ask(ctx: &Ctx, bundle: &EvmBundle, mine: Mine, owner: Option<[u8; 20]>) -> Result<Asking, String> {
    match mine {
        Mine::Key(address) => {
            let origin = super::super::dapp::offline(ctx, &address).ok_or("This wallet neither holds the key nor knows where a signer derives it.")?;
            let typed = match owner {
                Some(owner) => bundle.nested_typed_data(&owner),
                None => bundle.typed_data(),
            };
            let request = SignRequest::asking(typed.to_string().into_bytes(), DataKind::TypedData, Some(bundle.chain_id), origin, address, "a multisig's call");

            Ok(Asking::Typed { request: Box::new(request), owner })
        }
        Mine::Ed25519(signer) => Ok(Asking::Evm(Box::new(EvmRequest { bundle: bundle.clone(), signer, owner }))),
    }
}

/// What the offline signer showed back, put into the bundle it was asked about: an EVM key's
/// signature over the typed data, or the bundle with an Ed25519 key's signature in.
fn answered(asking: &Asking, read: &Message, bundle: &mut EvmBundle) -> Option<Result<(), String>> {
    match (asking, read) {
        (Asking::Typed { owner, .. }, Message::EthSignature(signature)) => Some(match signature.parts() {
            Some((rs, parity)) => {
                let mut whole = [0; 65];
                whole[..64].copy_from_slice(&rs);
                whole[64] = 27 + parity;

                match owner {
                    None => bundle.add(whole).map(|_| ()),
                    Some(owner) => match execute_signer(&bundle.nested_digest(owner), &whole) {
                        Some(signer) => {
                            bundle.add_nested(*owner, signer, whole.to_vec());
                            Ok(())
                        }
                        None => Err("A signature that recovers no key.".to_string()),
                    },
                }
            }
            None => Err("A signature that does not read.".to_string()),
        }),
        (Asking::Evm(_), Message::Evm(signed)) => Some(bundle.merge(&signed.bundle)),
        _ => None,
    }
}

pub fn show(ui: &mut Ui, ctx: &mut Ctx, state: &mut EvmState, account: &str, side: &Side) {
    let palette = *ctx.palette;
    let (start, now) = (&side.start, side.now.as_ref());

    if state.of != account {
        *state = EvmState { of: account.to_string(), ..EvmState::default() };
    }

    let Ok(multisig) = account.parse::<Address>() else {
        return;
    };
    let contract = multisig_contract(&multisig, start, side.home);
    state.link.learn_chains(ctx);
    let (reads, mined) = state.link.take();

    for read in reads {
        state.looking = false;

        match read {
            Read::Looked(Ok(found)) => state.looked = Some(found),
            Read::Looked(Err(why)) => state.trouble = Some(format!("The chain: {why}")),
        }
    }

    // Once a deploy or a call of its was mined, asked again: its code, nonce or owners moved.
    if mined.is_some() {
        match state.sending.take() {
            Some(Sending::Call) => {
                state.bundle = None;
                state.note = Some("Its call was mined.".to_string());
            }
            Some(Sending::Deploy) => state.note = Some("Deployed: now send the call.".to_string()),
            None => {}
        }

        state.looked = None;
    }

    if let Some(chain_id) = state.chain_id
        && state.looked.is_none()
        && !state.looking
    {
        look(ctx, state, chain_id, &contract);
    }

    // The next call another page asked for, once the contract's nonce is known.
    if state.bundle.is_none()
        && !state.queued.is_empty()
        && let Some(looked) = state.looked.clone().filter(|looked| Some(looked.chain_id) == state.chain_id)
    {
        let call = state.queued.remove(0);
        state.bundle = Some(bundle_of(&looked, &contract, start, &call));
    }

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, &format!("On EVM chains, {} of {}", start.threshold, start.owners.len()));
        super::super::row(ui, &palette, "Its contract, on every chain", |ui| super::super::address(ui, &palette, &to_hex(&contract)));

        if mine(ctx, start, now).is_none() {
            ui.label(RichText::new("This account is none of its owners, by its EVM key or its Ed25519 key: it can look, not sign.").small().color(palette.muted));
        }

        let chains: Vec<u64> = urls(ctx).iter().filter_map(|url| state.link.chain_ids.get(url).and_then(|known| known.clone().ok())).collect();

        if chains.is_empty() {
            ui.label(RichText::new("Name an EVM chain's JSON-RPC URL in Settings to see it there and send from it.").small().color(palette.muted));

            return;
        }

        ui.horizontal_wrapped(|ui| {
            for chain_id in &chains {
                if ui.selectable_label(state.chain_id == Some(*chain_id), chain_name(*chain_id)).clicked() {
                    state.chain_id = Some(*chain_id);
                    state.looked = None;
                    state.bundle = None;
                    state.queued.clear();
                }
            }
        });

        let Some(looked) = state.looked.clone().filter(|looked| Some(looked.chain_id) == state.chain_id) else {
            return;
        };
        let coin = symbol(looked.chain_id);
        super::super::row(ui, &palette, "Holds", |ui| {
            ui.label(format::amount(&looked.balance.to_string(), 18, coin));
        });

        match &looked.standing {
            None => {
                ui.label(RichText::new("Not deployed here yet: coin sent to it waits, and the first call sent deploys it.").small().color(palette.soft));
            }
            Some((owners, nonce)) => {
                super::super::row(ui, &palette, "Owners", |ui| {
                    ui.label(format!("{} of {}, next nonce {nonce}", owners.threshold, owners.owners.len()));
                });

                if let Some(now) = now.filter(|now| *now != owners)
                    && state.bundle.is_none()
                    && theme::quiet(ui, &palette, "Its members changed: propose the same owners here").clicked()
                {
                    let call = Call { to: contract, value: u256::ZERO, input: change_owners_input(now), what: "Change its owners" };
                    state.bundle = Some(bundle_of(&looked, &contract, start, &call));
                }
            }
        }
    });

    let Some(looked) = state.looked.clone().filter(|looked| Some(looked.chain_id) == state.chain_id) else {
        return;
    };

    match state.bundle.is_some() {
        true => bundle(ui, ctx, state, &multisig, &contract, side, &looked),
        false => {
            propose(ui, ctx, state, &contract, start, &looked);
            approve(ui, ctx, state, &contract, start, now);
        }
    }

    state.link.show(ui, ctx, "the multisig's call", "a multisig's call on an EVM chain");
}

/// Asks the chain for the contract's code, coin, owners, threshold and nonce, in one batch.
fn look(ctx: &Ctx, state: &mut EvmState, chain_id: u64, contract: &[u8; 20]) {
    let Some(url) = state.link.url_for(ctx, chain_id) else {
        return;
    };
    state.looking = true;
    let [owners, threshold, nonce] = multisig_reads();
    let call = |id: u64, input: Vec<u8>| request(id, "eth_call", json!([{ "to": to_hex(contract), "data": to_hex(&input) }, "latest"]));
    let batch = json!([
        request(1, "eth_getCode", json!([to_hex(contract), "latest"])),
        request(2, "eth_getBalance", json!([to_hex(contract), "latest"])),
        call(3, owners),
        call(4, threshold),
        call(5, nonce),
    ]);

    state.link.ask_raw(ctx, &url, batch, move |answer| Read::Looked(answer.and_then(|replies| looked(chain_id, &replies))));
}

/// A call made into a bundle at the contract's next nonce.
fn bundle_of(looked: &Looked, contract: &[u8; 20], start: &EvmStart, call: &Call) -> EvmBundle {
    EvmBundle::new(looked.chain_id, *contract, call.to, call.value, call.input.clone(), standing(looked, start).1)
}

/// Another multisig's call, pasted, approved for this one: its contract is one of that one's
/// owners, and this account one of its own.
fn approve(ui: &mut Ui, ctx: &mut Ctx, state: &mut EvmState, contract: &[u8; 20], start: &EvmStart, now: Option<&EvmStart>) {
    let palette = *ctx.palette;
    let Some(me) = mine(ctx, start, now) else {
        return;
    };

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Approve another multisig's call");
        ui.label(RichText::new("When this multisig's contract is an owner of another's, that one's calls need this one's approval: its owners, this account among them, each sign it here. Paste the other's bundle.").small().color(palette.muted));
        ui.add(egui::TextEdit::multiline(&mut state.foreign).hint_text("chainv4-multisig-evm:0x…").font(egui::TextStyle::Monospace).desired_rows(2).desired_width(f32::INFINITY));

        if !state.foreign.trim().is_empty() {
            match EvmBundle::from_text(&state.foreign) {
                Some(theirs) if theirs.contract == *contract => {
                    ui.colored_label(palette.bad, "That is this multisig's own call: sign it above, not here.");
                }
                Some(theirs) => {
                    state.approving = Some(match state.approving.take() {
                        Some(mut held) if held.digest() == theirs.digest() => {
                            let _ = held.merge(&theirs);
                            held
                        }
                        _ => theirs,
                    });
                    state.foreign.clear();
                }
                None => {
                    ui.colored_label(palette.bad, "Not a bundle: it starts chainv4-multisig-evm:.");
                }
            }
        }

        let Some(mut theirs) = state.approving.clone() else {
            return;
        };
        let approvals = theirs.nested.iter().find(|nested| nested.owner == *contract).map_or(0, |nested| nested.signatures.len());
        super::super::row(ui, &palette, "Their contract", |ui| super::super::address(ui, &palette, &to_hex(&theirs.contract)));
        super::super::row(ui, &palette, "Calls", |ui| super::super::address(ui, &palette, &to_hex(&theirs.to)));

        if theirs.value != u256::ZERO {
            super::super::row(ui, &palette, "Sends", |ui| {
                ui.label(format::amount(&theirs.value.to_string(), 18, symbol(theirs.chain_id)));
            });
        }

        super::super::row(ui, &palette, "On", |ui| {
            ui.label(format!("{}, at nonce {}", chain_name(theirs.chain_id), theirs.nonce));
        });
        ui.label(RichText::new(format!("approved for this multisig by {approvals} of the {} it needs", now.unwrap_or(start).threshold)).small().color(palette.soft));

        if let Some((asking, scan)) = state.asking.as_mut().filter(|(asking, _)| matches!(asking, Asking::Typed { owner: Some(_), .. } | Asking::Evm(_))) {
            show_asking(ui, &palette, asking);

            if let Some(read) = scan.show(ui, &palette) {
                match answered(asking, &read, &mut theirs) {
                    Some(Ok(())) => state.asking = None,
                    Some(Err(why)) => scan.fail(why),
                    None => scan.refuse(&read, "the signer's signature"),
                }
            }
        }

        ui.horizontal_wrapped(|ui| {
            match held(ctx, me) {
                Some(keypair) if theme::primary(ui, &palette, "Approve", true).clicked() => match theirs.approve_with(*contract, &keypair) {
                    Ok(_) => {
                        ui.ctx().copy_text(theirs.to_text());
                        state.note = Some("Approved and copied: give it back to the other multisig's members.".to_string());
                    }
                    Err(why) => state.trouble = Some(why),
                },
                None if state.asking.is_none() && theme::quiet(ui, &palette, "Ask the signer").clicked() => match ask(ctx, &theirs, me, Some(*contract)) {
                    Ok(asking) => state.asking = Some((asking, Scan::default())),
                    Err(why) => state.trouble = Some(why),
                },
                _ => {}
            }

            if theme::quiet(ui, &palette, "Copy as text").clicked() {
                ui.ctx().copy_text(theirs.to_text());
                state.note = Some("Copied: give it back to the other multisig's members.".to_string());
            }

            if theme::quiet(ui, &palette, "Put away").clicked() {
                state.approving = None;
                state.asking = None;
            }
        });

        if state.approving.is_some() {
            state.approving = Some(theirs);
        }
    });
}

/// The code for the offline signer, and what to do with it.
fn show_asking(ui: &mut Ui, palette: &crate::theme::Palette, asking: &Asking) {
    match asking {
        Asking::Typed { request, .. } => {
            ui.label(RichText::new("On the offline signer, Sign, and read this code: it shows the call as typed data. Then read its signature back.").color(palette.soft));
            super::super::code(ui, palette, &Message::MetaMask(request.clone()));
        }
        Asking::Evm(request) => {
            ui.label(RichText::new("On the offline signer, Sign, and read this code: it shows the call and signs it with the account's Ed25519 key. Then read its code back.").color(palette.soft));
            super::super::code(ui, palette, &Message::Evm(request.clone()));
        }
    }
}

/// A call to propose: whom, how much, and its data.
fn propose(ui: &mut Ui, ctx: &mut Ctx, state: &mut EvmState, contract: &[u8; 20], start: &EvmStart, looked: &Looked) {
    let palette = *ctx.palette;

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, &format!("Send from it on {}", chain_name(looked.chain_id)));
        ui.add(egui::TextEdit::singleline(&mut state.to).hint_text("To: 0x…").font(egui::TextStyle::Monospace).desired_width(f32::INFINITY));
        ui.add(egui::TextEdit::singleline(&mut state.amount).hint_text(format!("Amount in {}", symbol(looked.chain_id))).desired_width(f32::INFINITY));
        ui.add(egui::TextEdit::singleline(&mut state.data).hint_text("Call data, 0x…, or none for a payment").font(egui::TextStyle::Monospace).desired_width(f32::INFINITY));

        if theme::primary(ui, &palette, "Make the bundle", true).clicked() {
            let made = (|| -> Result<Call, String> {
                let to = from_hex(state.to.trim()).and_then(|bytes| bytes.try_into().ok()).ok_or("To is an EVM address: 0x and 40 hex digits.")?;
                let value = match state.amount.trim() {
                    "" => u256::ZERO,
                    amount => format::parse_units(amount, 18).and_then(|raw| u256::from_str_radix(&raw, 10).ok()).ok_or("An amount, or none.")?,
                };
                let input = match state.data.trim() {
                    "" => Vec::new(),
                    data => from_hex(data).ok_or("Call data is 0x and hex.")?,
                };

                Ok(Call { to, value, input, what: "Its call" })
            })();

            match made {
                Ok(call) => {
                    state.bundle = Some(bundle_of(looked, contract, start, &call));
                    state.trouble = None;
                }
                Err(why) => state.trouble = Some(why.to_string()),
            }
        }

        notes(ui, &palette, state);
    });
}

/// The bundle on its way round the owners: what it does, who signed, signing, and sending it.
fn bundle(ui: &mut Ui, ctx: &mut Ctx, state: &mut EvmState, multisig: &Address, contract: &[u8; 20], side: &Side, looked: &Looked) {
    let palette = *ctx.palette;
    let (start, now) = (&side.start, side.now.as_ref());
    let Some(mut bundle) = state.bundle.clone() else {
        return;
    };
    let (owners, next) = standing(looked, start);
    // A contract owner's signature is counted as given: the chain asks the contract.
    let signed = bundle.signed_by(&owners);
    let enough = signed >= owners.threshold as usize;
    let me = mine(ctx, start, now);
    let gas = payer(ctx);
    let mut close = false;

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "A call for the owners to sign");
        super::super::row(ui, &palette, "To", |ui| super::super::address(ui, &palette, &to_hex(&bundle.to)));

        if bundle.value != u256::ZERO {
            super::super::row(ui, &palette, "Sends", |ui| {
                ui.label(format::amount(&bundle.value.to_string(), 18, symbol(bundle.chain_id)));
            });
        }

        if !bundle.data.is_empty() {
            super::super::row(ui, &palette, "Data", |ui| {
                ui.label(theme::mono(format!("{} · {} bytes", to_hex(&bundle.data[..bundle.data.len().min(4)]), bundle.data.len())));
            });
        }

        super::super::row(ui, &palette, "At nonce", |ui| {
            ui.label(bundle.nonce.to_string());
        });
        ui.label(RichText::new(format!("signed by {signed} of the {} it needs", owners.threshold)).small().color(palette.soft));

        if bundle.nonce < next {
            ui.colored_label(palette.bad, "Its nonce has gone: the contract ran another call. Put it away and propose it again.");
        }

        if let Some((asking, scan)) = state.asking.as_mut().filter(|(asking, _)| matches!(asking, Asking::Typed { owner: None, .. } | Asking::Evm(_))) {
            show_asking(ui, &palette, asking);

            if let Some(read) = scan.show(ui, &palette) {
                match answered(asking, &read, &mut bundle) {
                    Some(Ok(())) => state.asking = None,
                    Some(Err(why)) => scan.fail(why),
                    None => scan.refuse(&read, "the signer's signature"),
                }
            }
        }

        ui.horizontal_wrapped(|ui| {
            if let Some(me) = me {
                match held(ctx, me) {
                    Some(keypair) if theme::primary(ui, &palette, "Sign", !enough).clicked() => {
                        if let Err(why) = bundle.sign_with(&keypair) {
                            state.trouble = Some(why);
                        }
                    }
                    None if state.asking.is_none() && theme::quiet(ui, &palette, "Ask the signer").clicked() => match ask(ctx, &bundle, me, None) {
                        Ok(asking) => state.asking = Some((asking, Scan::default())),
                        Err(why) => state.trouble = Some(why),
                    },
                    _ => {}
                }
            }

            if theme::quiet(ui, &palette, "Copy as text").clicked() {
                ui.ctx().copy_text(bundle.to_text());
                state.note = Some("Copied: chainv4 multisig evm sign reads it, and a member's wallet pasting it here adds what it signed.".to_string());
            }

            let busy = state.link.pending.is_some();

            // Any EVM key pays: owner or not, it only sends what the owners signed.
            match (looked.deployed, gas) {
                (false, Some(gas)) if theme::primary(ui, &palette, "Deploy it here first", !busy).clicked() => {
                    let call = Call { to: DEPLOYMENT_PROXY, value: u256::ZERO, input: multisig_deployment(multisig, start, side.home), what: "Deploy the multisig's contract" };
                    start_call(ctx, state, call, looked.chain_id, gas, Sending::Deploy);
                }
                (true, Some(gas)) if enough && bundle.nonce == next && theme::primary(ui, &palette, "Send it", !busy).clicked() => {
                    let call = Call { to: *contract, value: u256::ZERO, input: bundle.execute_input(&owners.owners), what: "The multisig's call" };
                    start_call(ctx, state, call, looked.chain_id, gas, Sending::Call);
                }
                (_, None) if enough => {
                    ui.label(RichText::new("This account has no EVM key to pay the gas: copy it as text for one that has.").small().color(palette.muted));
                }
                _ => {}
            }

            close = theme::quiet(ui, &palette, "Put away").clicked();
        });

        ui.add(egui::TextEdit::multiline(&mut state.pasted).hint_text("Paste a member's bundle, chainv4-multisig-evm:0x…").font(egui::TextStyle::Monospace).desired_rows(2).desired_width(f32::INFINITY));

        if !state.pasted.trim().is_empty() {
            match EvmBundle::from_text(&state.pasted) {
                Some(theirs) if theirs.digest() == bundle.digest() => {
                    let _ = bundle.merge(&theirs);
                    state.pasted.clear();
                }
                Some(_) => {
                    ui.colored_label(palette.bad, "That bundle is another call, or for another chain or nonce.");
                }
                None => {
                    ui.colored_label(palette.bad, "Not a bundle: it starts chainv4-multisig-evm:.");
                }
            }
        }

        notes(ui, &palette, state);
    });

    state.bundle = match close {
        true => {
            state.asking = None;
            state.queued.clear();
            None
        }
        false => Some(bundle),
    };
}

fn start_call(ctx: &Ctx, state: &mut EvmState, call: Call, chain_id: u64, from: [u8; 20], sending: Sending) {
    match state.link.url_for(ctx, chain_id) {
        Some(url) => {
            state.link.start(ctx, state.of.clone(), call, chain_id, url, from);
            state.sending = Some(sending);
        }
        None => state.trouble = Some(format!("Settings names no JSON-RPC URL for {}.", chain_name(chain_id))),
    }
}

fn notes(ui: &mut Ui, palette: &crate::theme::Palette, state: &EvmState) {
    if let Some(note) = &state.note {
        ui.label(RichText::new(note).small().color(palette.good));
    }

    if let Some(trouble) = &state.trouble {
        ui.colored_label(palette.bad, trouble);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn its_side_reads_from_the_node_and_the_chain_s_answers_read_as_the_contract() {
        use chainv4::registry::multisig::EvmOwner;

        let key = |byte: u8| EvmOwner { address: [byte; 20], kind: EvmKind::Key };
        let value = json!({ "evm": { "contract": "0x00", "home": 9, "start": { "owners": ["0x1111111111111111111111111111111111111111", "0x2222222222222222222222222222222222222222"], "kinds": ["key", "contract"], "threshold": 2 }, "now": null } });
        let side = side_of(&value).unwrap();

        assert_eq!(side.start, EvmStart { owners: vec![key(0x11), EvmOwner { address: [0x22; 20], kind: EvmKind::Contract }], threshold: 2 });
        assert_eq!((side.now, side.home), (None, 9));
        assert!(side_of(&json!({ "evm": null })).is_none(), "no EVM owners, no side there");
        let start = side.start;

        // Not deployed: no code, coin waiting, and the start's owners at nonce 0.
        let replies = json!([
            { "id": 1, "result": "0x" },
            { "id": 2, "result": "0x2a" },
            { "id": 3, "result": "0x" },
            { "id": 4, "result": "0x" },
            { "id": 5, "result": "0x" },
        ]);
        let found = looked(5, &replies).unwrap();

        assert_eq!((found.deployed, found.balance, found.standing.clone()), (false, u256::new(42), None));
        assert_eq!(standing(&found, &start), (start.clone(), u256::ZERO));

        // Deployed: its owners and nonce, as getOwnerKeys, threshold and nonce answer.
        let word = |value: u64| format!("{value:064x}");
        let owners = format!("0x{}{}{}{}{}{}{}", word(64), word(128), word(1), word(0), word(1), "00".repeat(12), "33".repeat(20));
        let replies = json!([
            { "id": 1, "result": "0x6080" },
            { "id": 2, "result": "0x0" },
            { "id": 3, "result": owners },
            { "id": 4, "result": format!("0x{}", word(1)) },
            { "id": 5, "result": format!("0x{}", word(7)) },
        ]);

        assert_eq!(looked(5, &replies).unwrap().standing, Some((EvmStart { owners: vec![key(0x33)], threshold: 1 }, u256::new(7))));
    }

    #[test]
    fn an_ed25519_owner_s_request_goes_to_the_signer_and_comes_back_signed() {
        let keypair = Keypair::from_private(PrivateKey::Ed25519([4; 32]));
        let bundle = EvmBundle::new(1, [9; 20], [7; 20], u256::ZERO, Vec::new(), u256::ZERO);
        let asking = Asking::Evm(Box::new(EvmRequest { bundle: bundle.clone(), signer: keypair.public(), owner: None }));
        let Asking::Evm(request) = &asking else { unreachable!() };

        // The signer's answer, read back over the code, puts the signature in.
        let signed = request.sign(&keypair).unwrap();
        let message = Message::from_text(&Message::Evm(Box::new(EvmRequest { bundle: signed, ..(**request).clone() })).to_text()).unwrap();
        let mut ours = bundle.clone();
        assert_eq!(answered(&asking, &message, &mut ours), Some(Ok(())));
        assert_eq!(ours.signers().len(), 1);
        assert!(request.sign(&Keypair::from_private(PrivateKey::Ed25519([5; 32]))).is_err(), "another key than the one asked");

        // An approval for another multisig, whose contract owns this call.
        let approval = EvmRequest { bundle: bundle.clone(), signer: keypair.public(), owner: Some([3; 20]) }.sign(&keypair).unwrap();
        assert_eq!(approval.nested[0].owner, [3; 20]);
        assert!(approval.signatures.is_empty(), "not a signature of this contract's owners");
    }
}
