//! A swap's legs on EVM chains, such as Ethereum, Sepolia or Base: Swap.sol there, read and sent
//! to through the chain's JSON-RPC, a URL Settings gives. Each URL is asked its chain id once, and
//! serves that chain's legs. A lock, claim or refund is priced there in one batch, shown, then
//! signed with the account's Ethereum key, after biometrics when they are on, or for an account
//! watched from the offline signer, asked of it as MetaMask asks, ur:eth-sign-request. Then it is
//! sent, and waited for until mined.

use {
    super::{Looked, SwapsState},
    crate::airgap::{Message, Origin},
    crate::biometrics::{self, Check},
    crate::format,
    crate::hardware::{DataKind, Signature, SignRequest},
    crate::theme,
    crate::views::Ctx,
    chainv4::crypto::ethereum::EthereumTx,
    chainv4::crypto::secp256k1,
    chainv4::structs::hex::{from_hex, to_hex},
    chainv4::structs::u256::u256,
    chainv4::swap::evm::{Call, chain_name, get_input, item_input, read_item, read_lock, symbol},
    chainv4::swap::{EvmLeg, Lock},
    chainv4_ui::biometric::Failure,
    egui::{RichText, Ui},
    serde_json::{Value, json},
};

/// How often a mined transaction is asked after.
const RECEIPT_EVERY: f64 = 3.0;

/// An EVM transaction on its way: from pricing to mined.
pub struct Pending {
    /// The swap it is for, by its hash.
    pub hash: String,
    pub call: Call,
    pub chain_id: u64,
    pub url: String,
    pub from: [u8; 20],
    pub stage: Stage,
    asked: f64,
}

pub enum Stage {
    Pricing,
    /// Priced, unsigned, to review.
    Priced(Box<EthereumTx>),
    /// Waiting for biometrics before signing it.
    Confirming(Box<EthereumTx>, Check),
    /// Asked of the offline signer, until its signature is read.
    Signer { tx: Box<EthereumTx>, request: Box<SignRequest>, scan: Box<super::Scan> },
    Sending,
    /// Sent, by its hash, until mined.
    Sent(String),
    /// Mined, or failed on the way: what came of it.
    Done(Result<String, String>),
}

/// What an EVM chain's RPC answered, for the next frame to take.
pub enum Answer {
    Chain { url: String, chain_id: Result<u64, String> },
    Lock { hash: String, chain_id: u64, lock: Result<Lock, String> },
    /// The chain's height, and the block its RPC calls finalized, where it names one.
    Heads { chain_id: u64, heads: Result<(u64, Option<u64>), String> },
    Priced(Result<EthereumTx, String>),
    Sent(Result<String, String>),
    Mined(Result<Option<bool>, String>),
}

/// The URLs Settings gives.
pub fn urls(ctx: &Ctx) -> Vec<String> {
    ctx.settings.eth_rpc.lines().map(str::trim).filter(|url| !url.is_empty()).map(str::to_string).collect()
}

/// The URL serving a chain, once it said so.
pub fn url_for(ctx: &Ctx, state: &SwapsState, chain_id: u64) -> Option<String> {
    urls(ctx).into_iter().find(|url| state.chain_ids.get(url).is_some_and(|known| *known == Ok(chain_id)))
}

/// Asks each URL not yet asked for its chain id.
pub fn learn_chains(ctx: &Ctx, state: &mut SwapsState) {
    for url in urls(ctx) {
        if state.chain_ids.contains_key(&url) {
            continue;
        }

        state.chain_ids.insert(url.clone(), Err("asking".to_string()));
        let target = url.clone();
        ask(ctx, state, &url, request(1, "eth_chainId", json!([])), move |answer| {
            Answer::Chain { url: target, chain_id: answer.and_then(|value| number(&value)).and_then(|id| u64::try_from(id).map_err(|_| "a chain id past 64 bits".to_string())) }
        });
    }
}

/// Asks Swap.sol where a leg's lock stands, and for a leg locking an NFT, which one it holds, in
/// one batch.
pub fn ask_lock(ctx: &Ctx, state: &SwapsState, url: &str, leg: &EvmLeg, hash: &[u8; 32]) {
    let (chain_id, key) = (leg.chain_id, to_hex(hash));
    let call = |id: u64, input: Vec<u8>| request(id, "eth_call", json!([{ "to": to_hex(&leg.contract), "data": to_hex(&input) }, "latest"]));
    let output = |value: &Value| value.as_str().and_then(from_hex).ok_or_else(|| format!("eth_call answered {value}"));

    if leg.item.is_none() {
        return ask(ctx, state, url, call(1, get_input(hash)), move |answer| Answer::Lock { hash: key, chain_id, lock: answer.and_then(|value| output(&value)).and_then(|output| read_lock(&output)) });
    }

    let batch = json!([call(1, get_input(hash)), call(2, item_input(hash))]);

    ask_raw(ctx, state, url, batch, move |answer| Answer::Lock {
        hash: key,
        chain_id,
        lock: answer.and_then(|replies| {
            let reply = |id: u64| -> Result<Vec<u8>, String> {
                let found = replies.as_array().and_then(|replies| replies.iter().find(|reply| reply["id"] == id)).ok_or("a batch without its answer")?;

                match found.get("error").filter(|error| !error.is_null()) {
                    Some(error) => Err(error["message"].as_str().map_or_else(|| error.to_string(), str::to_string)),
                    None => output(&found["result"]),
                }
            };
            let mut lock = read_lock(&reply(1)?)?;
            lock.item = read_item(&reply(2)?)?;

            Ok(lock)
        }),
    });
}

/// Asks a chain for its height and its finalized block, in one batch.
pub fn ask_heads(ctx: &Ctx, state: &SwapsState, url: &str, chain_id: u64) {
    let batch = json!([request(1, "eth_blockNumber", json!([])), request(2, "eth_getBlockByNumber", json!(["finalized", false]))]);

    ask_raw(ctx, state, url, batch, move |answer| Answer::Heads { chain_id, heads: answer.and_then(|replies| heads(&replies)) });
}

/// A chain's height, and its finalized block's, from the batch's answers: none where the RPC
/// names no finalized block.
pub fn heads(replies: &Value) -> Result<(u64, Option<u64>), String> {
    let reply = |id: u64| replies.as_array().and_then(|replies| replies.iter().find(|reply| reply["id"] == id)).cloned().unwrap_or(Value::Null);
    let tip = number(&reply(1)["result"]).and_then(|tip| u64::try_from(tip).map_err(|_| "a height past 64 bits".to_string()))?;
    let finalized = reply(2)["result"]["number"].as_str().and_then(|hex| u64::from_str_radix(hex.trim_start_matches("0x"), 16).ok());

    Ok((tip, finalized))
}

/// Starts a call on its way: priced first, in one batch of questions.
pub fn start(ctx: &Ctx, state: &mut SwapsState, hash: &[u8; 32], call: Call, chain_id: u64, url: String, from: [u8; 20]) {
    let estimate = json!({ "from": to_hex(&from), "to": to_hex(&call.to), "value": format!("{:#x}", call.value), "data": to_hex(&call.input) });
    let batch = json!([
        request(1, "eth_chainId", json!([])),
        request(2, "eth_getTransactionCount", json!([to_hex(&from), "pending"])),
        request(3, "eth_gasPrice", json!([])),
        request(4, "eth_maxPriorityFeePerGas", json!([])),
        request(5, "eth_estimateGas", json!([estimate])),
    ]);
    let priced = call.clone();
    ask_raw(ctx, state, &url, batch, move |answer| Answer::Priced(answer.and_then(|replies| price(&priced, chain_id, &replies))));
    state.evm = Some(Pending { hash: to_hex(hash), call, chain_id, url, from, stage: Stage::Pricing, asked: 0.0 });
}

/// The transaction the batch's answers price: the chain checked, the estimate with room above
/// it, and a most per gas of twice the price, for a base fee that rises for a few blocks.
pub fn price(call: &Call, chain_id: u64, replies: &Value) -> Result<EthereumTx, String> {
    let reply = |id: u64| -> Result<Value, String> {
        let reply = replies.as_array().and_then(|replies| replies.iter().find(|reply| reply["id"] == id)).ok_or("the RPC left a question unanswered")?;

        match reply.get("error").filter(|error| !error.is_null()) {
            Some(error) => Err(error["message"].as_str().map_or_else(|| error.to_string(), str::to_string)),
            None => Ok(reply["result"].clone()),
        }
    };
    let small = |id: u64| reply(id).and_then(|value| number(&value)).and_then(|value| u64::try_from(value).map_err(|_| "a number past 64 bits".to_string()));

    if small(1)? != chain_id {
        return Err(format!("that RPC serves another chain than {}", chain_name(chain_id)));
    }

    let (nonce, price) = (small(2)?, reply(3).and_then(|value| number(&value))?);
    let gas = small(5).map_err(|why| format!("it would fail: {why}"))?;
    // Not every chain's RPC suggests a tip; one at the whole price then pays no more.
    let tip = reply(4).and_then(|value| number(&value)).unwrap_or(price).min(price);

    Ok(call.transaction(chain_id, nonce, gas + gas / 4 + 10_000, price * u256::new(2), tip))
}

/// Takes what an EVM chain answered since the last frame.
pub fn take(ctx: &mut Ctx, state: &mut SwapsState, answer: Answer) {
    match answer {
        Answer::Chain { url, chain_id } => {
            state.chain_ids.insert(url, chain_id.map_err(|why| format!("no chain id: {why}")));
        }
        Answer::Lock { hash, chain_id, lock } => {
            // A claim there shows the secret: kept for the other leg's claim, once it is the
            // swap's, whatever the RPC says.
            if let Ok(Lock { secret: Some(secret), .. }) = &lock
                && to_hex(&chainv4::crypto::bitcoin::sha256(secret)) == hash
                && let Some(record) = ctx.book.swaps.iter_mut().find(|record| record.secret.is_none() && super::hash_of(record).as_deref() == Some(hash.as_str()))
            {
                record.secret = Some(to_hex(secret));
                *ctx.changed = true;
            }

            state.evm_locks.insert((hash, chain_id), lock);
        }
        Answer::Heads { chain_id, heads } => {
            if let Ok(heads) = heads {
                state.evm_heads.insert(chain_id, heads);
            }
        }
        Answer::Priced(priced) => {
            if let Some(pending) = state.evm.as_mut() {
                pending.stage = match priced {
                    Ok(tx) => Stage::Priced(Box::new(tx)),
                    Err(why) => Stage::Done(Err(why)),
                };
            }
        }
        Answer::Sent(sent) => {
            if let Some(pending) = state.evm.as_mut() {
                pending.stage = match sent {
                    Ok(hash) => Stage::Sent(hash),
                    Err(why) => Stage::Done(Err(why)),
                };
            }
        }
        Answer::Mined(mined) => {
            if let Some(pending) = state.evm.as_mut()
                && let Stage::Sent(hash) = &pending.stage
            {
                match mined {
                    Ok(Some(true)) => {
                        pending.stage = Stage::Done(Ok(hash.clone()));
                        // Asked again at once: the lock has moved.
                        state.evm_asked.remove(&pending.hash);
                    }
                    Ok(Some(false)) => pending.stage = Stage::Done(Err(format!("It was mined but failed: {hash}"))),
                    // Not mined yet, or the RPC did not say: asked again in a moment.
                    Ok(None) | Err(_) => {}
                }
            }
        }
    }
}

/// The transaction on its way, over the swap: its review, the signer's code, or where it stands.
pub fn show(ui: &mut Ui, ctx: &mut Ctx, state: &mut SwapsState) {
    let palette = *ctx.palette;
    let Some(pending) = state.evm.as_mut() else {
        return;
    };
    let chain = chain_name(pending.chain_id);
    let mut close = false;
    // The stage to move to, once this one is drawn.
    let mut next = None;
    // Signed here, after biometrics if they are on; or by the signer, signed already.
    let mut send = None;
    let mut broadcast_signed = None;

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, &format!("{} on {chain}", pending.call.what));

        match &mut pending.stage {
            Stage::Pricing => {
                ui.label(RichText::new(format!("Pricing it on {chain}…")).color(palette.soft));
            }
            Stage::Priced(tx) => {
                review(ui, &palette, &pending.call, tx, pending.chain_id);
                ui.horizontal(|ui| {
                    let away = secret(ctx, &pending.from).is_none();

                    if theme::primary(ui, &palette, if away { "Ask the signer" } else { "Sign and send" }, true).clicked() {
                        let tx = tx.clone();

                        match (secret(ctx, &pending.from), origin(ctx, &pending.from)) {
                            (Some(_), _) => match biometrics::enrolled(ctx.wake, ctx.settings) {
                                Some((enrolled, _)) => next = Some(Stage::Confirming(tx, Check::start(ctx.wake, &enrolled, "Send a swap's transaction"))),
                                None => send = Some(*tx),
                            },
                            (None, Some(origin)) => {
                                let request = SignRequest::asking(tx.signing_data(), DataKind::TypedTransaction, Some(tx.chain_id), origin, pending.from, "chainv4 · swap");
                                next = Some(Stage::Signer { tx, request: Box::new(request), scan: Box::default() });
                            }
                            (None, None) => next = Some(Stage::Done(Err("This wallet holds no key for that address, nor knows where a signer derives it.".to_string()))),
                        }
                    }

                    close = theme::quiet(ui, &palette, "Not now").clicked();
                });
            }
            Stage::Confirming(tx, check) => {
                ui.label(RichText::new("Confirm it…").color(palette.soft));

                match check.take() {
                    Some(Ok(_)) => send = Some((**tx).clone()),
                    Some(Err(Failure::Canceled)) => next = Some(Stage::Priced(tx.clone())),
                    Some(Err(Failure::Invalidated)) => {
                        biometrics::turn_off(ctx.wake, ctx.settings);
                        *ctx.lock = Some(biometrics::CHANGED.to_string());
                    }
                    Some(Err(Failure::Failed(why))) => next = Some(Stage::Done(Err(why))),
                    None => {}
                }
            }
            Stage::Signer { tx, request, scan } => {
                review(ui, &palette, &pending.call, tx, pending.chain_id);
                ui.label(RichText::new("On the offline signer, Sign, and read this code: it shows what it signs. Then read the signature it shows back, here.").color(palette.soft));
                super::super::code(ui, &palette, &Message::MetaMask(request.clone()));
                ui.add_space(8.0);

                match scan.show(ui, &palette) {
                    Some(Message::EthSignature(signature)) => match finish(tx, request, &signature, pending.from) {
                        Ok(signed) => {
                            next = Some(Stage::Sending);
                            broadcast_signed = Some(signed);
                        }
                        Err(why) => scan.fail(why),
                    },
                    Some(other) => scan.refuse(&other, "the signer's signature"),
                    None => {}
                }

                close = theme::quiet(ui, &palette, "Not now").clicked();
            }
            Stage::Sending => {
                ui.label(RichText::new("Sending…").color(palette.soft));
            }
            Stage::Sent(hash) => {
                ui.label(RichText::new("Sent; waiting for it to be mined.").color(palette.soft));
                ui.label(theme::mono(hash.as_str()).color(palette.muted));
            }
            Stage::Done(Ok(hash)) => {
                ui.colored_label(palette.good, "Mined.");
                ui.label(theme::mono(hash.as_str()).color(palette.muted));
                close = theme::quiet(ui, &palette, "Close").clicked();
            }
            Stage::Done(Err(why)) => {
                ui.colored_label(palette.bad, why.as_str());
                close = theme::quiet(ui, &palette, "Close").clicked();
            }
        }
    });

    if let Some(next) = next {
        pending.stage = next;
    }

    let now = ui.input(|input| input.time);

    if let Some(tx) = send.take()
        && let Some(secret) = secret(ctx, &state.evm.as_ref().map_or([0; 20], |pending| pending.from))
    {
        broadcast(ctx, state, tx.signed(&secret));
    }

    if let Some(tx) = broadcast_signed {
        broadcast(ctx, state, tx);
    }

    if let Some(pending) = state.evm.as_mut()
        && let Stage::Sent(hash) = &pending.stage
        && now - pending.asked > RECEIPT_EVERY
    {
        pending.asked = now;
        let (url, hash) = (pending.url.clone(), hash.clone());
        ask(ctx, state, &url, request(1, "eth_getTransactionReceipt", json!([hash])), |answer| {
            Answer::Mined(answer.map(|receipt| match receipt.is_null() {
                true => None,
                false => Some(receipt["status"] == "0x1"),
            }))
        });
    }

    if close {
        state.evm = None;
    }
}

/// What a transaction does, and the most it may cost.
fn review(ui: &mut Ui, palette: &theme::Palette, call: &Call, tx: &EthereumTx, chain_id: u64) {
    let coin = symbol(chain_id);
    super::super::row(ui, palette, "To", |ui| super::super::address(ui, palette, &to_hex(&call.to)));

    if call.value != u256::ZERO {
        super::super::row(ui, palette, "Sends", |ui| {
            ui.label(format::amount(&call.value.to_string(), 18, coin));
        });
    }

    super::super::row(ui, palette, "Fee, at most", |ui| {
        ui.label(format::amount(&(tx.max_price * u256::from(tx.gas_limit)).to_string(), 18, coin));
    });
}

/// Sends a signed transaction.
fn broadcast(ctx: &Ctx, state: &mut SwapsState, tx: EthereumTx) {
    let Some(pending) = state.evm.as_mut() else {
        return;
    };
    pending.stage = Stage::Sending;
    let url = pending.url.clone();

    ask(ctx, state, &url, request(1, "eth_sendRawTransaction", json!([to_hex(&tx.encode())])), |answer| {
        Answer::Sent(answer.and_then(|hash| hash.as_str().map(str::to_string).ok_or_else(|| format!("eth_sendRawTransaction answered {hash}"))))
    });
}

/// Takes the signer's answer: refused if it answers another request, or is not this address's
/// key's over the transaction.
pub(super) fn finish(tx: &EthereumTx, request: &SignRequest, signature: &Signature, from: [u8; 20]) -> Result<EthereumTx, String> {
    if signature.id.is_some() && signature.id != request.id {
        return Err("That signature answers another request: read the one the signer shows for this.".to_string());
    }

    let (rs, parity) = signature.parts().ok_or("A signature that does not read.")?;
    let key = secp256k1::recover_strict(&tx.signing_hash(), &rs, parity).ok_or("A signature that recovers no key.")?;

    if secp256k1::address(&key) != Some(from) {
        return Err("That signature is another key's: was the signer paired with these words?".to_string());
    }

    let mut signed = tx.clone();
    (signed.signature, signed.parity) = (rs, parity);

    Ok(signed)
}

/// The Ethereum key to an address, when this wallet holds it.
fn secret(ctx: &Ctx, address: &[u8; 20]) -> Option<[u8; 32]> {
    crate::views::dapp::secret(ctx, address)
}

/// Where the offline signer derives an address watched from it.
fn origin(ctx: &Ctx, address: &[u8; 20]) -> Option<Origin> {
    crate::views::dapp::offline(ctx, address)
}

fn request(id: u64, method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

/// A JSON-RPC answer's number: 0x and hex digits.
fn number(value: &Value) -> Result<u256, String> {
    let hex = value.as_str().and_then(|text| text.strip_prefix("0x")).ok_or_else(|| format!("{value} is not a number"))?;

    u256::from_str_radix(if hex.is_empty() { "0" } else { hex }, 16).map_err(|_| format!("{value} is not a number"))
}

/// Asks one question, taking its result, or its error, as the answer.
fn ask(ctx: &Ctx, state: &SwapsState, url: &str, body: Value, answer: impl FnOnce(Result<Value, String>) -> Answer + Send + 'static) {
    ask_raw(ctx, state, url, body, move |reply| {
        answer(reply.and_then(|reply| match reply.get("error").filter(|error| !error.is_null()) {
            Some(error) => Err(error["message"].as_str().map_or_else(|| error.to_string(), str::to_string)),
            None => Ok(reply["result"].clone()),
        }))
    });
}

/// Posts a question, or a batch, and takes the JSON that comes back.
fn ask_raw(ctx: &Ctx, state: &SwapsState, url: &str, body: Value, answer: impl FnOnce(Result<Value, String>) -> Answer + Send + 'static) {
    let client = match crate::app::client(ctx.settings) {
        Ok(client) => client,
        Err(why) => {
            state.looked.lock().expect("never poisoned").push(Looked::Evm(Box::new(answer(Err(why)))));
            return;
        }
    };
    let (looked, wake) = (state.looked.clone(), ctx.wake.clone());

    client.fetch(url.to_string(), Some(body.to_string().into_bytes()), move |reply| {
        let reply = reply.and_then(|reply| serde_json::from_slice::<Value>(&reply.bytes).map_err(|_| format!("not JSON-RPC: {}", String::from_utf8_lossy(&reply.bytes).chars().take(120).collect::<String>())));
        looked.lock().expect("never poisoned").push(Looked::Evm(Box::new(answer(reply))));
        wake.request_repaint();
    });
}
