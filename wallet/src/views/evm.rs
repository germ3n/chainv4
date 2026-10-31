//! Transactions on EVM chains, such as Ethereum, Sepolia or Base, for the pages that send there:
//! the Swaps page's locks, claims and refunds, and the Bridge page's sends and deliveries. Each
//! chain is reached through a JSON-RPC URL Settings gives, asked its chain id once. A call is
//! priced there in one batch, shown, then signed with the account's Ethereum key, after biometrics
//! when they are on, or for an account watched from the offline signer, asked of it as MetaMask
//! asks, ur:eth-sign-request. Then it is sent, and waited for until mined. What a page reads there
//! comes back to it as its own answers, R.

use {
    crate::airgap::{Message, Origin},
    crate::biometrics::{self, Check},
    crate::format,
    crate::hardware::{DataKind, Signature, SignRequest},
    crate::theme,
    crate::views::Ctx,
    crate::views::scan::Scan,
    chainv4::crypto::ethereum::EthereumTx,
    chainv4::crypto::secp256k1,
    chainv4::structs::hex::to_hex,
    chainv4::structs::u256::u256,
    chainv4::swap::evm::{Call, chain_name, symbol},
    chainv4_ui::biometric::Failure,
    egui::{RichText, Ui},
    serde_json::{Value, json},
    std::collections::HashMap,
    std::sync::{Arc, Mutex},
};

/// How often a mined transaction is asked after.
const RECEIPT_EVERY: f64 = 3.0;

/// An EVM transaction on its way: from pricing to mined.
pub struct Pending {
    /// What it is for, as the page that started it names it: a swap's hash, a transfer.
    pub tag: String,
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
    Signer { tx: Box<EthereumTx>, request: Box<SignRequest>, scan: Box<Scan> },
    Sending,
    /// Sent, by its hash, until mined.
    Sent(String),
    /// Mined, or failed on the way: what came of it.
    Done(Result<String, String>),
}

/// What an EVM chain's RPC answered, for the next frame to take: the link's own, or the page's.
pub enum Answer<R> {
    Chain { url: String, chain_id: Result<u64, String> },
    Priced(Result<Box<EthereumTx>, String>),
    Sent(Result<String, String>),
    Mined(Result<Option<bool>, String>),
    Read(R),
}

/// A page's way to EVM chains: which URL serves which chain, the one transaction on its way, and
/// what came back since the last frame.
pub struct Link<R> {
    pub chain_ids: HashMap<String, Result<u64, String>>,
    pub pending: Option<Pending>,
    answers: Arc<Mutex<Vec<Answer<R>>>>,
}

impl<R> Default for Link<R> {
    fn default() -> Link<R> {
        Link { chain_ids: HashMap::new(), pending: None, answers: Arc::default() }
    }
}

/// The URLs Settings gives.
pub fn urls(ctx: &Ctx) -> Vec<String> {
    ctx.settings.eth_rpc.lines().map(str::trim).filter(|url| !url.is_empty()).map(str::to_string).collect()
}

impl<R: Send + 'static> Link<R> {
    /// The URL serving a chain, once it said so.
    pub fn url_for(&self, ctx: &Ctx, chain_id: u64) -> Option<String> {
        urls(ctx).into_iter().find(|url| self.chain_ids.get(url).is_some_and(|known| *known == Ok(chain_id)))
    }

    /// Asks each URL not yet asked for its chain id.
    pub fn learn_chains(&mut self, ctx: &Ctx) {
        for url in urls(ctx) {
            if self.chain_ids.contains_key(&url) {
                continue;
            }

            self.chain_ids.insert(url.clone(), Err("asking".to_string()));
            let target = url.clone();
            self.ask_link(ctx, &url, request(1, "eth_chainId", json!([])), move |answer| Answer::Chain {
                url: target,
                chain_id: answer.and_then(|value| number(&value)).and_then(|id| u64::try_from(id).map_err(|_| "a chain id past 64 bits".to_string())),
            });
        }
    }

    /// Takes what came back since the last frame: the link's own answers, and the page's reads
    /// to take; and the tag of a transaction mined just now, for the page to ask again after.
    pub fn take(&mut self) -> (Vec<R>, Option<String>) {
        let answers: Vec<Answer<R>> = std::mem::take(&mut *self.answers.lock().expect("never poisoned"));
        let (mut reads, mut mined) = (Vec::new(), None);

        for answer in answers {
            match answer {
                Answer::Chain { url, chain_id } => {
                    self.chain_ids.insert(url, chain_id.map_err(|why| format!("no chain id: {why}")));
                }
                Answer::Priced(priced) => {
                    if let Some(pending) = self.pending.as_mut() {
                        pending.stage = match priced {
                            Ok(tx) => Stage::Priced(tx),
                            Err(why) => Stage::Done(Err(why)),
                        };
                    }
                }
                Answer::Sent(sent) => {
                    if let Some(pending) = self.pending.as_mut() {
                        pending.stage = match sent {
                            Ok(hash) => Stage::Sent(hash),
                            Err(why) => Stage::Done(Err(why)),
                        };
                    }
                }
                Answer::Mined(found) => {
                    if let Some(pending) = self.pending.as_mut()
                        && let Stage::Sent(hash) = &pending.stage
                    {
                        match found {
                            Ok(Some(true)) => {
                                pending.stage = Stage::Done(Ok(hash.clone()));
                                mined = Some(pending.tag.clone());
                            }
                            Ok(Some(false)) => pending.stage = Stage::Done(Err(format!("It was mined but failed: {hash}"))),
                            // Not mined yet, or the RPC did not say: asked again in a moment.
                            Ok(None) | Err(_) => {}
                        }
                    }
                }
                Answer::Read(read) => reads.push(read),
            }
        }

        (reads, mined)
    }

    /// Starts a call on its way: priced first, in one batch of questions.
    pub fn start(&mut self, ctx: &Ctx, tag: String, call: Call, chain_id: u64, url: String, from: [u8; 20]) {
        let estimate = json!({ "from": to_hex(&from), "to": to_hex(&call.to), "value": format!("{:#x}", call.value), "data": to_hex(&call.input) });
        let batch = json!([
            request(1, "eth_chainId", json!([])),
            request(2, "eth_getTransactionCount", json!([to_hex(&from), "pending"])),
            request(3, "eth_gasPrice", json!([])),
            request(4, "eth_maxPriorityFeePerGas", json!([])),
            request(5, "eth_estimateGas", json!([estimate])),
        ]);
        let priced = call.clone();
        self.ask_raw_link(ctx, &url, batch, move |answer| Answer::Priced(answer.and_then(|replies| price(&priced, chain_id, &replies)).map(Box::new)));
        self.pending = Some(Pending { tag, call, chain_id, url, from, stage: Stage::Pricing, asked: 0.0 });
    }

    /// Reads a contract there: eth_call at the latest block, its output as bytes.
    #[cfg_attr(not(feature = "bridge"), allow(dead_code))]
    pub fn call(&self, ctx: &Ctx, url: &str, to: &[u8; 20], input: &[u8], read: impl FnOnce(Result<Vec<u8>, String>) -> R + Send + 'static) {
        let body = request(1, "eth_call", json!([{ "to": to_hex(to), "data": to_hex(input) }, "latest"]));

        self.ask(ctx, url, body, move |answer| {
            read(answer.and_then(|value| value.as_str().and_then(chainv4::structs::hex::from_hex).ok_or_else(|| format!("eth_call answered {value}"))))
        });
    }

    /// Asks one question, taking its result, or its error, as the page's answer.
    pub fn ask(&self, ctx: &Ctx, url: &str, body: Value, read: impl FnOnce(Result<Value, String>) -> R + Send + 'static) {
        self.ask_link(ctx, url, body, move |answer| Answer::Read(read(answer)));
    }

    /// Posts a question, or a batch, taking the JSON that comes back as the page's answer.
    pub fn ask_raw(&self, ctx: &Ctx, url: &str, body: Value, read: impl FnOnce(Result<Value, String>) -> R + Send + 'static) {
        self.ask_raw_link(ctx, url, body, move |answer| Answer::Read(read(answer)));
    }

    fn ask_link(&self, ctx: &Ctx, url: &str, body: Value, answer: impl FnOnce(Result<Value, String>) -> Answer<R> + Send + 'static) {
        self.ask_raw_link(ctx, url, body, move |reply| {
            answer(reply.and_then(|reply| match reply.get("error").filter(|error| !error.is_null()) {
                Some(error) => Err(error["message"].as_str().map_or_else(|| error.to_string(), str::to_string)),
                None => Ok(reply["result"].clone()),
            }))
        });
    }

    fn ask_raw_link(&self, ctx: &Ctx, url: &str, body: Value, answer: impl FnOnce(Result<Value, String>) -> Answer<R> + Send + 'static) {
        let client = match crate::app::client(ctx.settings) {
            Ok(client) => client,
            Err(why) => {
                self.answers.lock().expect("never poisoned").push(answer(Err(why)));
                return;
            }
        };
        let (answers, wake) = (self.answers.clone(), ctx.wake.clone());

        client.fetch(url.to_string(), Some(body.to_string().into_bytes()), move |reply| {
            let reply = reply.and_then(|reply| serde_json::from_slice::<Value>(&reply.bytes).map_err(|_| format!("not JSON-RPC: {}", String::from_utf8_lossy(&reply.bytes).chars().take(120).collect::<String>())));
            answers.lock().expect("never poisoned").push(answer(reply));
            wake.request_repaint();
        });
    }

    /// The transaction on its way: its review, the signer's code, or where it stands. purpose
    /// names it to biometrics and the signer, such as "a swap's transaction".
    pub fn show(&mut self, ui: &mut Ui, ctx: &mut Ctx, purpose: &str, signer_note: &str) {
        let palette = *ctx.palette;
        let Some(pending) = self.pending.as_mut() else {
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
                    review(ui, ctx, &pending.call, tx, pending.chain_id);
                    ui.horizontal_wrapped(|ui| {
                        let away = secret(ctx, &pending.from).is_none();

                        if theme::primary(ui, &palette, if away { "Ask the signer" } else { "Sign and send" }, true).clicked() {
                            let tx = tx.clone();

                            match (secret(ctx, &pending.from), origin(ctx, &pending.from)) {
                                (Some(_), _) => match biometrics::enrolled(ctx.wake, ctx.settings) {
                                    Some((enrolled, _)) => next = Some(Stage::Confirming(tx, Check::start(ctx.wake, &enrolled, &format!("Send {purpose}")))),
                                    None => send = Some(*tx),
                                },
                                (None, Some(origin)) => {
                                    let request = SignRequest::asking(tx.signing_data(), DataKind::TypedTransaction, Some(tx.chain_id), origin, pending.from, signer_note);
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
                            *ctx.lock = Some(biometrics::invalidated(ctx.wake, ctx.settings));
                        }
                        Some(Err(Failure::Failed(why))) => next = Some(Stage::Done(Err(why))),
                        None => {}
                    }
                }
                Stage::Signer { tx, request, scan } => {
                    review(ui, ctx, &pending.call, tx, pending.chain_id);
                    ui.label(RichText::new("On the offline signer, Sign, and read this code: it shows what it signs. Then read the signature it shows back, here.").color(palette.soft));
                    super::code(ui, &palette, &Message::MetaMask(request.clone()));
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
            && let Some(secret) = secret(ctx, &self.pending.as_ref().map_or([0; 20], |pending| pending.from))
        {
            self.broadcast(ctx, tx.signed(&secret));
        }

        if let Some(tx) = broadcast_signed {
            self.broadcast(ctx, tx);
        }

        if let Some(pending) = self.pending.as_mut()
            && let Stage::Sent(hash) = &pending.stage
            && now - pending.asked > RECEIPT_EVERY
        {
            pending.asked = now;
            let (url, hash) = (pending.url.clone(), hash.clone());
            self.ask_link(ctx, &url, request(1, "eth_getTransactionReceipt", json!([hash])), |answer| {
                Answer::Mined(answer.map(|receipt| match receipt.is_null() {
                    true => None,
                    false => Some(receipt["status"] == "0x1"),
                }))
            });
        }

        if close {
            self.pending = None;
        }
    }

    /// Sends a signed transaction.
    fn broadcast(&mut self, ctx: &Ctx, tx: EthereumTx) {
        let Some(pending) = self.pending.as_mut() else {
            return;
        };
        pending.stage = Stage::Sending;
        let url = pending.url.clone();

        self.ask_link(ctx, &url, request(1, "eth_sendRawTransaction", json!([to_hex(&tx.encode())])), |answer| {
            Answer::Sent(answer.and_then(|hash| hash.as_str().map(str::to_string).ok_or_else(|| format!("eth_sendRawTransaction answered {hash}"))))
        });
    }
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

/// What a transaction does, and the most it may cost. Its call reads by name below what is sure of
/// it, and on another chain, its tokens' decimals are not known here, so its amounts stay plain.
fn review(ui: &mut Ui, ctx: &Ctx, call: &Call, tx: &EthereumTx, chain_id: u64) {
    let palette = ctx.palette;
    let coin = symbol(chain_id);
    super::row(ui, palette, "To", |ui| super::address(ui, palette, &to_hex(&call.to)));

    if call.value != u256::ZERO {
        super::row(ui, palette, "Sends", |ui| {
            ui.label(format::amount(&call.value.to_string(), 18, coin));
        });
    }

    if !call.input.is_empty() {
        super::row(ui, palette, "Data", |ui| {
            ui.label(theme::mono(format!("{} · {} bytes", to_hex(&call.input[..call.input.len().min(4)]), call.input.len())));
        });

        if let Some(read) = ctx.reader.call(super::decoded::kept_for(ctx.settings, &call.to), &call.input) {
            super::decoded::signing(ui, palette, None, &read);
        }

        super::decoded::raw(ui, palette, "evm data", &to_hex(&call.input));
    }

    super::row(ui, palette, "Fee, at most", |ui| {
        ui.label(format::amount(&(tx.max_price * u256::from(tx.gas_limit)).to_string(), 18, coin));
    });
}

/// Takes the signer's answer: refused if it answers another request, or is not this address's
/// key's over the transaction.
pub fn finish(tx: &EthereumTx, request: &SignRequest, signature: &Signature, from: [u8; 20]) -> Result<EthereumTx, String> {
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

pub fn request(id: u64, method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

/// A JSON-RPC answer's number: 0x and hex digits.
pub fn number(value: &Value) -> Result<u256, String> {
    let hex = value.as_str().and_then(|text| text.strip_prefix("0x")).ok_or_else(|| format!("{value} is not a number"))?;

    u256::from_str_radix(if hex.is_empty() { "0" } else { hex }, 16).map_err(|_| format!("{value} is not a number"))
}
