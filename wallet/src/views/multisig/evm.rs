//! A native multisig's contract on EVM chains, for one whose members name EVM addresses: a
//! MultiSig.sol at one address on every chain, its owners the members' EVM keys. The page shows
//! the address, and on a chain Settings reaches, whether it is deployed, what it holds, its owners
//! and its nonce; a call by it is a bundle the owners sign, here with the account's key, on the
//! offline signer as EIP-712 typed data, or with chainv4 multisig evm, and any key sends it once
//! enough signed, deploying the contract first. When the members change, it offers the matching
//! change of owners, which the owners before sign.

use {
    super::super::Ctx,
    super::super::evm::{Link, number, request, urls},
    super::super::scan::Scan,
    crate::airgap::Message,
    crate::format,
    crate::hardware::{DataKind, SignRequest},
    crate::theme,
    chainv4::abi::multisig::{EvmBundle, change_owners_input, multisig_contract, multisig_deployment, multisig_read, multisig_reads},
    chainv4::registry::multisig::EvmStart,
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
    /// The offline signer, asked to sign the bundle's typed data.
    asking: Option<(Box<SignRequest>, Scan)>,
    pasted: String,
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

/// The multisig's side on EVM chains, from what the node says of it: the owners its contract
/// starts with, which its address commits to, and the ones its members name now.
impl EvmState {
    /// Takes calls another page made for a multisig's contract on a chain, such as a swap's lock.
    pub fn queue(&mut self, account: &str, chain_id: u64, calls: Vec<Call>) {
        *self = EvmState { of: account.to_string(), chain_id: Some(chain_id), queued: calls, ..EvmState::default() };
    }
}

pub fn side_of(value: &Value) -> Option<(EvmStart, Option<EvmStart>)> {
    let owners = |value: &Value| -> Option<EvmStart> {
        let owners = value["owners"].as_array()?.iter().map(|owner| owner.as_str().and_then(from_hex).and_then(|bytes| bytes.try_into().ok())).collect::<Option<Vec<[u8; 20]>>>()?;

        Some(EvmStart { owners, threshold: u32::try_from(value["threshold"].as_u64()?).ok()? })
    };

    Some((owners(&value["evm"]["start"])?, owners(&value["evm"]["now"])))
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

/// This account's EVM address, when it is one of the members': the one it signs and pays gas with.
fn mine(ctx: &Ctx, start: &EvmStart, now: Option<&EvmStart>) -> Option<[u8; 20]> {
    let members: Vec<[u8; 20]> = start.owners.iter().chain(now.into_iter().flat_map(|now| now.owners.iter())).copied().collect();
    let holder = ctx.holder()?;

    holder.publics.iter().find_map(|public| match public.address() {
        Address::Secp256k1(bytes) if members.contains(&bytes) => Some(bytes),
        _ => None,
    })
}

pub fn show(ui: &mut Ui, ctx: &mut Ctx, state: &mut EvmState, account: &str, start: &EvmStart, now: Option<&EvmStart>) {
    let palette = *ctx.palette;

    if state.of != account {
        *state = EvmState { of: account.to_string(), ..EvmState::default() };
    }

    let Ok(multisig) = account.parse::<Address>() else {
        return;
    };
    let contract = multisig_contract(&multisig, start);
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
            ui.label(RichText::new("This account's EVM address is none of its members': it can look, not sign.").small().color(palette.muted));
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
        true => bundle(ui, ctx, state, &multisig, &contract, start, now, &looked),
        false => propose(ui, ctx, state, &contract, start, &looked),
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
    EvmBundle { chain_id: looked.chain_id, contract: *contract, to: call.to, value: call.value, data: call.input.clone(), nonce: standing(looked, start).1, signatures: Vec::new() }
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
#[allow(clippy::too_many_arguments)]
fn bundle(ui: &mut Ui, ctx: &mut Ctx, state: &mut EvmState, multisig: &Address, contract: &[u8; 20], start: &EvmStart, now: Option<&EvmStart>, looked: &Looked) {
    let palette = *ctx.palette;
    let Some(mut bundle) = state.bundle.clone() else {
        return;
    };
    let (owners, next) = standing(looked, start);
    let signed = bundle.signers().iter().filter(|(signer, _)| owners.owners.contains(signer)).count();
    let enough = signed >= owners.threshold as usize;
    let me = mine(ctx, start, now);
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

        if let Some((request, scan)) = state.asking.as_mut() {
            ui.label(RichText::new("On the offline signer, Sign, and read this code: it shows the call as typed data. Then read its signature back.").color(palette.soft));
            super::super::code(ui, &palette, &Message::MetaMask(request.clone()));

            match scan.show(ui, &palette) {
                Some(Message::EthSignature(signature)) => match signature.parts() {
                    Some((rs, parity)) => {
                        let mut whole = [0; 65];
                        whole[..64].copy_from_slice(&rs);
                        whole[64] = 27 + parity;

                        match bundle.add(whole) {
                            Ok(_) => state.asking = None,
                            Err(why) => scan.fail(why),
                        }
                    }
                    None => scan.fail("A signature that does not read.".to_string()),
                },
                Some(other) => scan.refuse(&other, "the signer's signature"),
                None => {}
            }
        }

        ui.horizontal_wrapped(|ui| {
            if let Some(me) = me {
                match super::super::dapp::secret(ctx, &me) {
                    Some(secret) if theme::primary(ui, &palette, "Sign", !enough).clicked() => bundle.sign(&secret),
                    None if state.asking.is_none() && theme::quiet(ui, &palette, "Ask the signer").clicked() => match super::super::dapp::offline(ctx, &me) {
                        Some(origin) => {
                            let request = SignRequest::asking(bundle.typed_data().to_string().into_bytes(), DataKind::TypedData, Some(bundle.chain_id), origin, me, "a multisig's call");
                            state.asking = Some((Box::new(request), Scan::default()));
                        }
                        None => state.trouble = Some("This wallet neither holds the key nor knows where a signer derives it.".to_string()),
                    },
                    _ => {}
                }
            }

            if theme::quiet(ui, &palette, "Copy as text").clicked() {
                ui.ctx().copy_text(bundle.to_text());
                state.note = Some("Copied: chainv4 multisig evm sign reads it, and a member's wallet pasting it here adds what it signed.".to_string());
            }

            let busy = state.link.pending.is_some();

            match (looked.deployed, me) {
                (false, Some(me)) if theme::primary(ui, &palette, "Deploy it here first", !busy).clicked() => {
                    let call = Call { to: DEPLOYMENT_PROXY, value: u256::ZERO, input: multisig_deployment(multisig, start), what: "Deploy the multisig's contract" };
                    start_call(ctx, state, call, looked.chain_id, me, Sending::Deploy);
                }
                (true, Some(me)) if enough && bundle.nonce == next && theme::primary(ui, &palette, "Send it", !busy).clicked() => {
                    let call = Call { to: *contract, value: u256::ZERO, input: bundle.execute_input(&owners.owners), what: "The multisig's call" };
                    start_call(ctx, state, call, looked.chain_id, me, Sending::Call);
                }
                _ => {}
            }

            close = theme::quiet(ui, &palette, "Put away").clicked();
        });

        ui.add(egui::TextEdit::multiline(&mut state.pasted).hint_text("Paste a member's bundle, chainv4-multisig-evm:0x…").font(egui::TextStyle::Monospace).desired_rows(2).desired_width(f32::INFINITY));

        if !state.pasted.trim().is_empty() {
            match EvmBundle::from_text(&state.pasted) {
                Some(theirs) if theirs.digest() == bundle.digest() => {
                    for signature in theirs.signatures {
                        let _ = bundle.add(signature);
                    }
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
        let value = json!({ "evm": { "contract": "0x00", "start": { "owners": ["0x1111111111111111111111111111111111111111", "0x2222222222222222222222222222222222222222"], "threshold": 2 }, "now": null } });
        let (start, now) = side_of(&value).unwrap();

        assert_eq!(start, EvmStart { owners: vec![[0x11; 20], [0x22; 20]], threshold: 2 });
        assert_eq!(now, None);
        assert!(side_of(&json!({ "evm": null })).is_none(), "no EVM addresses, no side there");

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

        // Deployed: its owners and nonce, as getOwners, threshold and nonce answer.
        let word = |value: u64| format!("{value:064x}");
        let owners = format!("0x{}{}{}{}", word(32), word(1), "00".repeat(12), "33".repeat(20));
        let replies = json!([
            { "id": 1, "result": "0x6080" },
            { "id": 2, "result": "0x0" },
            { "id": 3, "result": owners },
            { "id": 4, "result": format!("0x{}", word(1)) },
            { "id": 5, "result": format!("0x{}", word(7)) },
        ]);

        assert_eq!(looked(5, &replies).unwrap().standing, Some((EvmStart { owners: vec![[0x33; 20]], threshold: 1 }, u256::new(7))));
    }
}
