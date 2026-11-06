//! What sites ask, through the browser extension, shown over whatever page is open until it is
//! answered: to connect, choosing which Ethereum accounts a site sees; to send, priced on the node
//! and reviewed as any send is; to sign a message or typed data, laid out to read. Nothing is
//! signed but from a tap here, after biometrics when they are on. Settings has the switch that
//! lets sites ask at all, and the sites connected.
//!
//! An account watched from the offline signer is offered too, once paired by a code that says
//! where the signer's words derive it: approving shows the request as the signer reads MetaMask's,
//! ur:eth-sign-request, and the signer's ur:eth-signature, read back off its screen, signs it.

use {
    super::Ctx,
    crate::biometrics::{self, Check},
    crate::airgap::{Message, Origin},
    crate::dapp::{self, AssetKind, Asked, Request, Wanted},
    crate::hardware::{self, DataKind, SignRequest},
    crate::format,
    crate::node::Ask,
    crate::theme,
    chainv4::crypto::ethereum::{EthereumKind, EthereumTx, chain_id},
    chainv4::crypto::keypair::{PrivateKey, PublicKey},
    chainv4::crypto::secp256k1,
    chainv4::structs::address::Address,
    chainv4::structs::hex::to_hex,
    chainv4::structs::u256::u256,
    chainv4_ui::biometric::Failure,
    egui::{RichText, Ui},
    serde_json::{Value, json},
    std::collections::HashSet,
};

/// How a send's questions to the node are told apart: the JSON-RPC id is the request's times this,
/// plus which.
const KINDS: u64 = 4;
const NONCE: u64 = 0;
const GAS: u64 = 1;
const PRICE: u64 = 2;
const SEND: u64 = 3;

#[derive(Default)]
pub struct DappState {
    /// What waits, in the order it came.
    shown: Vec<Shown>,
    /// Requests answered here, or refused as unreadable, so a late copy is not shown again.
    answered: HashSet<u64>,
    /// The biometric check before signing, with the request it is for.
    confirming: Option<(u64, Check)>,
}

struct Shown {
    request: Request,
    /// For a connect: the accounts ticked.
    chosen: HashSet<[u8; 20]>,
    /// For a send: what the node said it lacked.
    nonce: Option<u64>,
    gas: Option<u64>,
    price: Option<u256>,
    asked: bool,
    sending: bool,
    trouble: Option<String>,
    /// Asked of the offline signer, once approved, until its signature is read back.
    offline: Option<Box<Offline>>,
}

/// A request shown to the offline signer: what it signs, and the scanner its answer comes in by.
struct Offline {
    request: SignRequest,
    /// For a send, the transaction the signature completes.
    tx: Option<EthereumTx>,
    /// What is signed: the transaction's hash, or the message's or typed data's.
    digest: [u8; 32],
    scan: super::scan::Scan,
}

impl DappState {
    /// Takes what waits, as the extension lists it: new ones are read, and ones settled elsewhere,
    /// in another of the wallet's windows, go.
    pub fn take(&mut self, pending: Vec<Value>) {
        let ids: HashSet<u64> = pending.iter().filter_map(|json| json["id"].as_u64()).collect();
        self.shown.retain(|shown| ids.contains(&shown.request.id));

        for json in pending {
            let Some(id) = json["id"].as_u64() else { continue };

            if self.answered.contains(&id) || self.shown.iter().any(|shown| shown.request.id == id) {
                continue;
            }

            match Request::read(&json) {
                Ok(request) => self.shown.push(Shown {
                    request,
                    chosen: HashSet::new(),
                    nonce: None,
                    gas: None,
                    price: None,
                    asked: false,
                    sending: false,
                    trouble: None,
                    offline: None,
                }),
                Err((id, why)) => {
                    self.answered.insert(id);
                    dapp::answer(id, Err((-32602, why)));
                }
            }
        }
    }

    /// The node's answer to something a send asked at POST /rpc.
    pub fn answered(&mut self, body: &str, result: &Result<Value, crate::node::Failure>) {
        let Some(id) = serde_json::from_str::<Value>(body).ok().and_then(|body| body["id"].as_u64()) else {
            return;
        };
        let (request, kind) = (id / KINDS, id % KINDS);
        let Some(shown) = self.shown.iter_mut().find(|shown| shown.request.id == request) else {
            return;
        };
        let answer = match result {
            Ok(value) => match (&value["result"], value["error"]["message"].as_str()) {
                (_, Some(error)) => Err(error.to_string()),
                (result, None) => Ok(result.clone()),
            },
            Err(failure) => Err(failure.message.clone()),
        };
        let number = |value: &Value| value.as_str().and_then(|hex| u256::from_str_radix(hex.trim_start_matches("0x"), 16).ok());

        match (kind, answer) {
            (NONCE, Ok(value)) => shown.nonce = number(&value).and_then(|nonce| u64::try_from(nonce).ok()),
            (GAS, Ok(value)) => shown.gas = number(&value).and_then(|gas| u64::try_from(gas).ok()),
            (PRICE, Ok(value)) => shown.price = number(&value),
            (SEND, Ok(hash)) => {
                let id = shown.request.id;
                self.answered.insert(id);
                self.shown.retain(|shown| shown.request.id != id);
                dapp::answer(id, Ok(hash));
            }
            (SEND, Err(why)) => {
                shown.sending = false;
                shown.trouble = Some(format!("The node refused it: {why}"));
            }
            (_, Err(why)) => shown.trouble = Some(format!("The node could not price it: {why}")),
            (_, Ok(_)) => {}
        }
    }
}

/// The Ethereum accounts this wallet signs for, itself or by the offline signer: each its name and
/// 20 bytes.
pub fn accounts(ctx: &Ctx) -> Vec<(String, [u8; 20])> {
    offered(ctx.holders)
}

/// The same, of these accounts: those this wallet holds keys for, and those watched from an offline
/// signer that said where it derives them. MetaMask's are MetaMask's to offer.
pub fn offered(holders: &[crate::keys::Holder]) -> Vec<(String, [u8; 20])> {
    holders
        .iter()
        .flat_map(|holder| {
            let signs = !holder.keys.is_empty() || holder.origin().is_some();
            // A watched account's first Ethereum key alone: a request names it by the account's
            // place under the signer's words, which a further one is not at.
            let first_only = holder.keys.is_empty();

            holder
                .publics
                .iter()
                .filter(move |_| signs && !holder.by_metamask())
                .filter_map(move |public| match public.address() {
                    Address::Secp256k1(bytes) => Some((holder.name.clone(), bytes)),
                    _ => None,
                })
                .take(if first_only { 1 } else { usize::MAX })
        })
        .collect()
}

/// Where the offline signer derives one of them, for an account watched from it.
pub(super) fn offline(ctx: &Ctx, account: &[u8; 20]) -> Option<Origin> {
    ctx.holders
        .iter()
        .filter(|holder| holder.keys.is_empty())
        .find(|holder| holder.publics.iter().any(|public| matches!(public.address(), Address::Secp256k1(bytes) if bytes == *account)))
        .and_then(|holder| holder.origin())
}

/// The key to one of them.
pub(super) fn secret(ctx: &Ctx, account: &[u8; 20]) -> Option<[u8; 32]> {
    ctx.holders.iter().flat_map(|holder| &holder.keys).find_map(|key| match (key.public().address(), key.private()) {
        (Address::Secp256k1(bytes), PrivateKey::Secp256k1(secret)) if bytes == *account => Some(*secret),
        _ => None,
    })
}

/// Draws the first request waiting, over the page; false when none waits.
pub fn show(ui: &mut Ui, ctx: &mut Ctx, state: &mut DappState) -> bool {
    let palette = *ctx.palette;

    if state.shown.is_empty() {
        if dapp::approving() {
            theme::title(ui, &palette, "Sites", "Nothing waits for you to approve. This window closes once it has answered.");

            return true;
        }

        return false;
    }

    let chain = ctx.chain.genesis.map(|genesis| chain_id(&genesis));

    // A switch to the chain followed already needs no asking, and one to a chain never followed
    // cannot be done: either is answered at once, as EIP-3326 has it.
    let settled: Vec<(u64, dapp::Answer)> = state
        .shown
        .iter()
        .filter_map(|shown| match shown.request.asked {
            Asked::SwitchChain { chain_id } if Some(chain_id) == chain => Some((shown.request.id, Ok(Value::Null))),
            Asked::SwitchChain { chain_id } if known_node(ctx, chain_id).is_none() => {
                Some((shown.request.id, Err((4902, format!("This wallet has not followed chain {chain_id}: connect to its node under Settings first.")))))
            }
            _ => None,
        })
        .collect();

    for (id, answer) in settled {
        state.answered.insert(id);
        state.shown.retain(|shown| shown.request.id != id);
        dapp::answer(id, answer);
    }

    if state.shown.is_empty() {
        return false;
    }

    let count = state.shown.len();
    let accounts = accounts(ctx);
    let mut done: Option<(u64, dapp::Answer)> = None;
    let mut ask = Vec::new();
    let confirmed = take_check(ctx, state);
    let shown = &mut state.shown[0];
    let id = shown.request.id;
    theme::title(ui, &palette, "A site asks", &match count {
        1 => shown.request.origin.clone(),
        more => format!("{} · 1 of {more}", shown.request.origin),
    });

    // Any but a connect names its account, which must be one this wallet signs for.
    if let Some(signer) = shown.request.signer()
        && !accounts.iter().any(|(_, account)| *account == signer)
    {
        shown.trouble = Some(format!("This wallet holds no key for {}.", to_hex(&signer)));
    }

    let mut approve = false;
    let mut reject = false;
    // What it asks, apart from what this frame changes of it.
    let asked = shown.request.asked.clone();

    theme::card(ui, &palette, |ui| match &asked {
        Asked::Connect => {
            theme::caps(ui, &palette, "Connect");
            ui.label(RichText::new("It would see these accounts' addresses and balances, and could ask you to send and sign. Each send and each signature waits for you here.").color(palette.soft));
            ui.add_space(8.0);

            if accounts.is_empty() {
                shown.trouble = Some("This wallet has no Ethereum account it holds a key for: add one under Accounts.".to_string());
            }

            if shown.chosen.is_empty()
                && let Some(current) = ctx.holders.get(ctx.current).and_then(|holder| accounts.iter().find(|(name, _)| *name == holder.name))
            {
                shown.chosen.insert(current.1);
            }

            for (name, account) in &accounts {
                let mut ticked = shown.chosen.contains(account);

                if ui.checkbox(&mut ticked, RichText::new(format!("{name} · {}", format::short(&to_hex(account)))).color(palette.ink)).changed() {
                    match ticked {
                        true => shown.chosen.insert(*account),
                        false => shown.chosen.remove(account),
                    };
                }
            }
        }
        Asked::Send(wanted) => send(ui, ctx, &palette, wanted, shown, chain, &mut ask),
        Asked::SwitchChain { chain_id } => {
            theme::caps(ui, &palette, "Switch chains");

            match known_node(ctx, *chain_id) {
                Some(node) => {
                    ui.label(RichText::new(format!("It asks the wallet to follow chain {chain_id}, which it followed before at {node}. Switching starts afresh with that node, its light client checking from the genesis pinned there, for every page, not just this site.")).color(palette.soft));
                }
                None => shown.trouble = Some(format!("This wallet has not followed chain {chain_id}: connect to its node under Settings first.")),
            }
        }
        Asked::WatchAsset(watched) => {
            theme::caps(ui, &palette, &format!("Show {}", if watched.symbol.is_empty() { "a token" } else { watched.symbol.as_str() }));
            ui.label(RichText::new("The wallet would show it beside the chain's listed tokens, its balance read on its own EVM from state the node proves. A site names a token as it likes: the symbol is its word alone.").color(palette.soft));
            ui.add_space(8.0);
            super::row(ui, &palette, "kind", |ui| {
                ui.label(RichText::new(watched.kind.name()).color(palette.ink));
            });
            super::row(ui, &palette, "contract", |ui| {
                ui.label(theme::mono(format!("evm:{}", watched.address)).color(palette.ink));
            });

            if let Some(id) = &watched.token_id {
                super::row(ui, &palette, "token", |ui| {
                    ui.label(theme::mono(id).color(palette.ink));
                });
            }

            if watched.kind == AssetKind::Erc20 {
                super::row(ui, &palette, "decimals", |ui| {
                    ui.label(RichText::new(watched.decimals.to_string()).color(palette.ink));
                });
            }

            if chain.is_none() {
                shown.trouble = Some("The wallet follows no chain yet.".to_string());
            }
        }
        Asked::Message { message, .. } => {
            theme::caps(ui, &palette, "Sign a message");
            ui.label(RichText::new("Signing a message moves nothing, but a site may take it as you saying so: sign only what you mean.").color(palette.soft));
            ui.add_space(8.0);

            match std::str::from_utf8(message).ok().filter(|text| !text.chars().any(|c| c.is_control() && !c.is_whitespace())) {
                Some(text) => super::text_block(ui, &palette, text),
                None => super::text_block(ui, &palette, &to_hex(message)),
            }
        }
        Asked::Typed { data, json, .. } => {
            theme::caps(ui, &palette, &format!("Sign {}", data.primary));
            ui.label(RichText::new("Typed data can approve a token to be spent, or an order: read each field.").color(palette.soft));
            ui.add_space(8.0);

            for field in ["name", "version", "chainId", "verifyingContract"] {
                if let Some(value) = json["domain"].get(field).filter(|value| !value.is_null()) {
                    super::row(ui, &palette, field, |ui| {
                        ui.label(theme::mono(value.to_string().trim_matches('"')).color(palette.ink));
                    });
                }
            }

            let named = json["domain"]["chainId"].as_u64().or_else(|| json["domain"]["chainId"].as_str().and_then(|text| u64::from_str_radix(text.trim_start_matches("0x"), if text.starts_with("0x") { 16 } else { 10 }).ok()));

            if let (Some(named), Some(chain)) = (named, chain)
                && named != chain
            {
                ui.colored_label(palette.warn, format!("It names chain {named}, not this one, {chain}."));
            }

            ui.add_space(6.0);
            // Only the fields the type declares are signed; show just those, never decoy fields a
            // site slips into the message that the digest leaves out.
            super::text_block(ui, &palette, &serde_json::to_string_pretty(&data.signed_message()).unwrap_or_default());
        }
    });

    // An account whose key is on the offline signer, where the signer derives it.
    let signer = shown.request.signer();
    let away = signer.and_then(|signer| secret(ctx, &signer).is_none().then(|| offline(ctx, &signer)).flatten());
    let mut signed = None;

    if let Some(offline) = shown.offline.as_mut() {
        theme::card(ui, &palette, |ui| {
            theme::caps(ui, &palette, "On the offline signer");
            ui.label(RichText::new("On the signer, Sign, and read this code: it shows what it signs. Then read the signature it shows back, here.").color(palette.soft));
            ui.add_space(8.0);
            super::code(ui, &palette, &Message::MetaMask(Box::new(offline.request.clone())));
            ui.add_space(10.0);
            theme::caps(ui, &palette, "Read the signature");

            match offline.scan.show(ui, &palette) {
                Some(Message::EthSignature(signature)) => signed = Some(finish(offline, &signature, shown.request.signer())),
                Some(other) => offline.scan.refuse(&other, "the signer's signature"),
                None => {}
            }
        });
    }

    theme::card(ui, &palette, |ui| {
        if let Some(trouble) = &shown.trouble {
            ui.colored_label(palette.bad, trouble.as_str());
            ui.add_space(6.0);
        }

        ui.horizontal_wrapped(|ui| {
            let ready = shown.offline.is_none()
                && match &asked {
                Asked::Connect => !shown.chosen.is_empty(),
                Asked::Send(_) => shown.trouble.is_none() && !shown.sending && prepared(shown).is_some(),
                _ => shown.trouble.is_none(),
            };
            let label = match &asked {
                Asked::Connect => "Connect",
                Asked::SwitchChain { .. } => "Switch",
                Asked::WatchAsset(_) => "Show it",
                _ if away.is_some() && shown.offline.is_none() => "Ask the signer",
                Asked::Send(_) if shown.sending => "Sending…",
                Asked::Send(_) => "Send",
                _ => "Sign",
            };
            approve = theme::primary(ui, &palette, label, ready && state.confirming.is_none()).clicked();
            reject = theme::quiet(ui, &palette, "Reject").clicked();
        });
    });

    // A connect gives addresses alone; anything signed here waits for biometrics, when they are
    // on. The offline signer checks its own.
    let signs = !matches!(asked, Asked::Connect | Asked::SwitchChain { .. } | Asked::WatchAsset(_)) && away.is_none();

    if approve && let (Some(origin), Some(from)) = (away, signer) {
        match ask_offline(&asked, shown, origin, from, chain) {
            Ok(offline) => shown.offline = Some(Box::new(offline)),
            Err(why) => shown.trouble = Some(why),
        }

        approve = false;
    }

    match signed {
        Some(Ok(Finished::Send(tx))) => {
            shown.sending = true;
            ask.push(rpc(id, SEND, "eth_sendRawTransaction", json!([to_hex(&tx.encode())])));
        }
        Some(Ok(Finished::Signature(signature))) => done = Some((id, Ok(json!(signature)))),
        Some(Err(why)) => shown.trouble = Some(why),
        None => {}
    }

    if approve && signs && !confirmed && state.confirming.is_none() && let Some((enrolled, _)) = biometrics::enrolled(ctx.wake, ctx.settings) {
        state.confirming = Some((id, Check::start(ctx.wake, &enrolled, "Approve what the site asks")));
    } else if approve || confirmed {
        match &asked {
            Asked::Connect => {
                let chosen = accounts.iter().filter(|(_, account)| shown.chosen.contains(account)).map(|(_, account)| json!(to_hex(account))).collect();
                done = Some((id, Ok(Value::Array(chosen))));
            }
            Asked::SwitchChain { chain_id } => match known_node(ctx, *chain_id) {
                Some(node) => {
                    *ctx.connect = Some(node);
                    done = Some((id, Ok(Value::Null)));
                }
                None => done = Some((id, Err((4902, format!("This wallet has not followed chain {chain_id}."))))),
            },
            Asked::WatchAsset(watched) => {
                if let Some(genesis) = ctx.chain.genesis.map(|genesis| genesis.to_string()) {
                    let kept = (genesis, (**watched).clone());

                    if !ctx.settings.watched.iter().any(|(on, token)| *on == kept.0 && token.address == kept.1.address && token.token_id == kept.1.token_id) {
                        ctx.settings.watched.push(kept);
                    }
                }

                done = Some((id, Ok(json!(true))));
            }
            Asked::Send(wanted) => match (transaction(wanted, shown, chain), secret(ctx, &wanted.from)) {
                (Some(tx), Some(secret)) => {
                    let tx = tx.signed(&secret);
                    shown.sending = true;
                    ask.push(rpc(id, SEND, "eth_sendRawTransaction", json!([to_hex(&tx.encode())])));
                }
                _ => shown.trouble = Some("The wallet lacks the key, or the chain, to sign it.".to_string()),
            },
            Asked::Message { address, message } => match secret(ctx, address) {
                Some(secret) => done = Some((id, Ok(json!(dapp::signature(&secret, &dapp::personal_digest(message)))))),
                None => shown.trouble = Some("The key is gone from this wallet.".to_string()),
            },
            Asked::Typed { address, data, .. } => match (secret(ctx, address), data.digest()) {
                (Some(secret), Ok(digest)) => done = Some((id, Ok(json!(dapp::signature(&secret, &digest))))),
                (None, _) => shown.trouble = Some("The key is gone from this wallet.".to_string()),
                (_, Err(why)) => shown.trouble = Some(why),
            },
        }
    }

    if reject {
        state.confirming = None;
        done = Some((id, dapp::rejected()));
    }

    for asked in ask {
        ctx.node.ask(asked);
    }

    if let Some((id, answer)) = done {
        state.answered.insert(id);
        state.shown.retain(|shown| shown.request.id != id);
        dapp::answer(id, answer);
    }

    true
}

/// The node the wallet followed a chain at before, by the chain's Ethereum id: the first, by name,
/// of those whose pinned genesis gives it.
pub fn known_node(ctx: &Ctx, chain: u64) -> Option<String> {
    let mut nodes: Vec<&String> = ctx
        .settings
        .pinned
        .iter()
        .filter(|(_, genesis)| chain_of_genesis(genesis) == Some(chain))
        .map(|(node, _)| node)
        .collect();
    nodes.sort();

    nodes.first().map(|node| node.to_string())
}

/// The Ethereum chain id of a genesis id as the wallet keeps it, 0x and hex: chain_id's, from its
/// first six bytes.
pub fn chain_of_genesis(genesis: &str) -> Option<u64> {
    let bytes = chainv4::structs::hex::from_hex(genesis)?;
    let mut id = [0; 8];
    id[2..].copy_from_slice(bytes.get(..6)?);

    Some(u64::from_be_bytes(id).max(1))
}

/// What the biometric check came to, for the request it was started for.
fn take_check(ctx: &mut Ctx, state: &mut DappState) -> bool {
    let Some((id, check)) = &state.confirming else {
        return false;
    };
    let Some(came) = check.take() else {
        return false;
    };
    let still = state.shown.first().is_some_and(|shown| shown.request.id == *id);
    state.confirming = None;

    match came {
        Ok(_) => still,
        Err(Failure::Canceled) => false,
        Err(Failure::Invalidated) => {
            *ctx.lock = Some(biometrics::invalidated(ctx.wake, ctx.settings));

            false
        }
        Err(Failure::Failed(why)) => {
            if let Some(shown) = state.shown.first_mut() {
                shown.trouble = Some(why);
            }

            false
        }
    }
}

/// The transaction a send asks for, unsigned, once the node said what the site left out and the
/// light client met the chain.
fn transaction(wanted: &Wanted, shown: &Shown, chain: Option<u64>) -> Option<EthereumTx> {
    let (nonce, gas, price) = prepared(shown)?;

    Some(EthereumTx {
        kind: EthereumKind::DynamicFee,
        chain_id: chain?,
        nonce,
        max_price: price,
        tip: wanted.tip.unwrap_or(u256::ZERO).min(price),
        gas_limit: gas,
        to: wanted.to,
        value: wanted.value,
        input: wanted.input.clone(),
        access_list: Vec::new(),
        signature: [0; 64],
        parity: 0,
    })
}

/// What to ask the offline signer for an approved request: the request it reads, as MetaMask's
/// would be, by the account's place under the signer's seed.
fn ask_offline(asked: &Asked, shown: &Shown, origin: Origin, from: [u8; 20], chain: Option<u64>) -> Result<Offline, String> {
    let site = shown.request.origin.as_str();
    let (request, tx, digest) = match asked {
        Asked::Send(wanted) => {
            let tx = transaction(wanted, shown, chain).ok_or("It is not priced yet.")?;
            let request = SignRequest::asking(tx.signing_data(), DataKind::TypedTransaction, Some(tx.chain_id), origin, from, site);
            let digest = tx.signing_hash();

            (request, Some(tx), digest)
        }
        Asked::Message { message, .. } => (SignRequest::asking(message.clone(), DataKind::Message, chain, origin, from, site), None, dapp::personal_digest(message)),
        Asked::Typed { data, json, .. } => {
            let digest = data.digest()?;

            (SignRequest::asking(json.to_string().into_bytes(), DataKind::TypedData, chain, origin, from, site), None, digest)
        }
        Asked::Connect | Asked::SwitchChain { .. } | Asked::WatchAsset(_) => return Err("nothing to sign".to_string()),
    };

    Ok(Offline { request, tx, digest, scan: super::scan::Scan::default() })
}

/// What the signer's signature completes.
enum Finished {
    /// A send's transaction, signed, for the node.
    Send(Box<EthereumTx>),
    /// A message's or typed data's signature, as the site takes it: r, s and v, 27 or 28.
    Signature(String),
}

/// Takes the signer's answer: refused if it answers another request, or is not the account's key's
/// over what was asked.
fn finish(offline: &Offline, signature: &hardware::Signature, from: Option<[u8; 20]>) -> Result<Finished, String> {
    if signature.id.is_some() && signature.id != offline.request.id {
        return Err("That signature answers another request: read the one the signer shows for this.".to_string());
    }

    let (rs, parity) = signature.parts().ok_or("A signature that does not read.")?;
    let key = secp256k1::recover_strict(&offline.digest, &rs, parity).ok_or("A signature that recovers no key.")?;

    if secp256k1::address(&key) != from {
        let by = PublicKey::Secp256k1(key).address();

        return Err(format!("That signature is {by}'s, not this account's: was the signer paired with these words?"));
    }

    Ok(match &offline.tx {
        Some(tx) => {
            let mut tx = tx.clone();
            tx.signature = rs;
            tx.parity = parity;

            Finished::Send(Box::new(tx))
        }
        None => Finished::Signature(to_hex(&[rs.as_slice(), &[27 + parity]].concat())),
    })
}

/// A send's nonce, gas and price, once the node said what the site left out.
fn prepared(shown: &Shown) -> Option<(u64, u64, u256)> {
    let Asked::Send(wanted) = &shown.request.asked else {
        return None;
    };

    Some((wanted.nonce.or(shown.nonce)?, wanted.gas.or(shown.gas)?, wanted.max_price.or(shown.price)?))
}

fn rpc(request: u64, kind: u64, method: &str, params: Value) -> Ask {
    Ask::Rpc(json!({ "jsonrpc": "2.0", "id": request * KINDS + kind, "method": method, "params": params }).to_string())
}

fn send(ui: &mut Ui, ctx: &Ctx, palette: &theme::Palette, wanted: &Wanted, shown: &mut Shown, chain: Option<u64>, ask: &mut Vec<Ask>) {
    let id = shown.request.id;

    // What the site left out, from the node, once.
    if !shown.asked {
        shown.asked = true;
        let from = to_hex(&wanted.from);

        if wanted.nonce.is_none() {
            ask.push(rpc(id, NONCE, "eth_getTransactionCount", json!([from, "pending"])));
        }

        if wanted.gas.is_none() {
            let mut call = json!({ "from": from, "value": format!("{:#x}", wanted.value), "data": to_hex(&wanted.input) });

            if let Some(to) = wanted.to {
                call["to"] = json!(to_hex(&to));
            }

            ask.push(rpc(id, GAS, "eth_estimateGas", json!([call])));
        }

        if wanted.max_price.is_none() {
            ask.push(rpc(id, PRICE, "eth_gasPrice", json!([])));
        }
    }

    if chain.is_none() {
        shown.trouble = Some("The light client has not met the chain yet: wait for it, then try again.".to_string());
    }

    theme::caps(ui, palette, if wanted.to.is_none() { "Deploy a contract" } else { "Send" });
    let coin = ctx.coin;
    theme::fitted(ui, &format::amount(&wanted.value.to_string(), coin.decimals, &coin.symbol), 30.0, 16.0, egui::FontFamily::Name(theme::SERIF.into()), palette.ink);
    ui.add_space(6.0);
    super::row(ui, palette, "From", |ui| super::address(ui, palette, &format!("secp256k1:{}", to_hex(&wanted.from))));

    if let Some(to) = wanted.to {
        let kept = ctx.settings.contracts.iter().find(|kept| kept.address.eq_ignore_ascii_case(&format!("evm:{}", to_hex(&to))));
        super::row(ui, palette, "To", |ui| {
            ui.vertical(|ui| {
                if let Some(kept) = kept {
                    ui.label(RichText::new(&kept.name).color(palette.ink));
                }

                super::address(ui, palette, &to_hex(&to));
            });
        });

        if !wanted.input.is_empty() {
            // The site's call, read by the ABI kept for the contract or one built in, under what is
            // sure of it: that there is data, and how much, stays whatever this reads as.
            let read = ctx.reader.call(super::decoded::kept_for(ctx.settings, &to), &wanted.input);
            let raw = theme::mono(format!("{} with {} bytes", to_hex(&wanted.input[..wanted.input.len().min(4)]), wanted.input.len())).color(palette.soft);

            super::row(ui, palette, "Calls", |ui| match &read {
                Some(_) => {
                    ui.label(raw);
                }
                None => {
                    ui.label(raw).on_hover_text("Keep the contract with its ABI on the Contracts page to see this by name.");
                }
            });

            if let Some(read) = &read {
                let token = ctx.chain.tokens.get(&format!("evm:{}", to_hex(&to))).and_then(|token| Some((token.value.symbol.as_deref()?, token.value.decimals?)));

                super::decoded::signing(ui, palette, token, read);
            }

            super::decoded::raw(ui, palette, "site's data", &to_hex(&wanted.input));
        }
    } else {
        super::row(ui, palette, "Code", |ui| {
            ui.label(format!("{} bytes", wanted.input.len()));
        });
    }

    match prepared(shown) {
        Some((nonce, gas, price)) => {
            let most = (u256::from(gas) * price).to_string();
            super::row(ui, palette, "Fee, at most", |ui| {
                ui.label(format::amount(&most, coin.decimals, &coin.symbol)).on_hover_text(format!("{gas} gas at up to {price} units"));
            });
            super::row(ui, palette, "Nonce", |ui| {
                ui.label(nonce.to_string());
            });
        }
        None if shown.trouble.is_none() => super::waiting(ui, palette, "Pricing it on the node…"),
        None => {}
    }

    ui.add_space(6.0);
    ui.label(RichText::new("Sent as an Ethereum transaction, from this account's Ethereum key. Nothing the site says here is proven: check the address and the amount.").small().color(palette.warn));
}

/// Settings' part: whether sites may ask, and which are connected. Only in the browser extension.
pub fn settings(ui: &mut Ui, ctx: &Ctx) {
    if !dapp::available() {
        return;
    }

    let palette = *ctx.palette;
    let sites = dapp::sites();
    let on = sites["on"].as_bool().unwrap_or(false);

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Sites");
        ui.label(
            RichText::new("Sites that speak to Ethereum wallets can ask this one to connect, through window.ethereum: your Ethereum accounts, the ones you choose, and each send and signature approved here.")
                .color(palette.soft),
        );
        ui.add_space(8.0);
        let mut wanted = on;

        if ui.checkbox(&mut wanted, "Let sites ask to connect").changed() {
            dapp::let_sites(wanted);
        }

        if sites["wanted"].as_bool() == Some(true) && !on {
            ui.label(RichText::new("Allow it in the bar above: the browser asks for every site, once.").small().color(palette.warn));
        }

        let connected = sites["connected"].as_object().cloned().unwrap_or_default();

        if connected.is_empty() {
            return;
        }

        ui.add_space(8.0);
        theme::caps(ui, &palette, "Connected");

        for (origin, site) in &connected {
            let accounts = site["accounts"].as_array().map_or(0, Vec::len);
            theme::end_row(
                ui,
                |ui| {
                    ui.vertical(|ui| {
                        ui.label(RichText::new(origin).color(palette.ink));
                        ui.label(RichText::new(format!("{accounts} account{}", if accounts == 1 { "" } else { "s" })).small().color(palette.muted));
                    });
                },
                |ui| {
                    if theme::quiet(ui, &palette, "Disconnect").clicked() {
                        dapp::disconnect(origin);
                    }
                },
            );
        }
    });
}

#[cfg(test)]
mod tests;
