//! The bridges (docs/bridges.md): the coin, tokens and NFTs to and from EVM chains through the
//! gateways, and BTC in and out as wBTC. Where the gateways are is the chain's directory, and who
//! signs for them its federation, each proven by the light client, never taken from a node or a
//! name.
//!
//! - **To an EVM chain:** the gateway here may move the asset, then takes it: two calls the Send
//!   page reviews and signs. The federation signs what left once it is final; once the board
//!   holds enough signatures, the transfer is delivered to Gateway.sol, by anyone: here, with the
//!   account's Ethereum key, if no one did yet.
//! - **From an EVM chain:** Gateway.sol may move the asset, then takes it, signed with the
//!   account's Ethereum key as the Swaps page signs there. It arrives here as the federation
//!   signs it, as the wrapped token or the coin.
//! - **Bitcoin:** the account's deposit address is computed here, from the proven federation, so
//!   no node can hand it another. A payment to it, once deep enough, is shown to the Bitcoin
//!   gateway with its proof, from the light client's block or Esplora's, and minted as wBTC.
//!   wBTC is withdrawn to a Bitcoin address; the federation pays it in its next release.

use {
    super::send::{Prepared, SendState},
    super::{Ctx, Page},
    crate::format,
    crate::node::Ask,
    crate::theme,
    crate::views::evm::Link,
    chainv4::abi::{AbiType, AbiValue, encode_values},
    chainv4::bridge::{AssetKind, Transfer, btc_deposit_script, send_input},
    chainv4::crypto::bitcoin::tx::{Network, Transaction, address, p2wsh},
    chainv4::crypto::hasher::Hasher,
    chainv4::registry::directory::Directory,
    chainv4::registry::federation::DecidedFederation,
    chainv4::structs::address::Address,
    chainv4::structs::amount::Amount,
    chainv4::structs::hash::HashType,
    chainv4::structs::hex::{from_hex, to_hex},
    chainv4::structs::tx::{Tx, TxCallArgumentType},
    chainv4::structs::u256::u256,
    chainv4::swap::evm::{Call, chain_name},
    egui::{RichText, Ui},
    serde_json::{Value, json},
    std::collections::HashMap,
    std::sync::{Arc, Mutex},
};

/// How often what the gateways say is asked again, in seconds.
const ASK_EVERY: f64 = 15.0;
/// How many of the latest transfers and withdrawals are looked through for the account's.
const RECENT: u64 = 20;

/// Which way the page sends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Way {
    Out,
    In,
    Bitcoin,
}

/// What an EVM chain answered the page.
pub enum Read {
    /// Whether Gateway.sol took a transfer from here, by its chain and the transfer's digest.
    Delivered { chain_id: u64, digest: String, delivered: Result<bool, String> },
}

/// A deposit's proof, as Esplora gave it: its block, the transaction without its witness, its
/// place and Merkle path, each hash in Bitcoin's own byte order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proof {
    pub block: Vec<u8>,
    pub raw: Vec<u8>,
    pub index: u64,
    pub path: Vec<u8>,
    /// The block's coinbase without its witness, and its path, which show how tall its tree is.
    pub coinbase: Vec<u8>,
    pub coinbase_path: Vec<u8>,
}

pub struct BridgeState {
    way: Way,
    /// What the gateways answered, by contract, function and arguments.
    answers: HashMap<String, Result<Vec<Value>, String>>,
    asked: HashMap<String, f64>,
    kind: AssetKind,
    asset: String,
    id: String,
    amount: String,
    recipient: String,
    chain: Option<u64>,
    evm: Link<Read>,
    delivered: HashMap<(u64, String), Result<bool, String>>,
    delivered_asked: HashMap<(u64, String), f64>,
    withdraw_to: String,
    withdraw_sats: String,
    txid: String,
    vout: String,
    /// What Esplora proved, for the next frame to take.
    looked: Arc<Mutex<Vec<Result<Proof, String>>>>,
    found: Option<Result<Proof, String>>,
}

impl Default for BridgeState {
    fn default() -> BridgeState {
        BridgeState {
            way: Way::Out,
            answers: HashMap::new(),
            asked: HashMap::new(),
            kind: AssetKind::Coin,
            asset: String::new(),
            id: String::new(),
            amount: String::new(),
            recipient: String::new(),
            chain: None,
            evm: Link::default(),
            delivered: HashMap::new(),
            delivered_asked: HashMap::new(),
            withdraw_to: String::new(),
            withdraw_sats: String::new(),
            txid: String::new(),
            vout: "0".to_string(),
            looked: Arc::default(),
            found: None,
        }
    }
}

fn key_of(contract: &str, function: &str, args: &Value) -> String {
    format!("{contract}|{function}|{args}")
}

/// A value as text: a string as it is, anything else as JSON writes it.
fn text(value: &Value) -> String {
    value["value"].as_str().map_or_else(|| value["value"].to_string(), str::to_string)
}

/// A value's bytes: bytes as they are, text as its own.
fn raw(value: &Value) -> Vec<u8> {
    match value["type"].as_str() {
        Some("bytes") => value["value"].as_str().and_then(from_hex).unwrap_or_default(),
        _ => value["value"].as_str().unwrap_or_default().as_bytes().to_vec(),
    }
}

fn whole(value: &Value) -> Option<u64> {
    text(value).parse().ok()
}

fn wide(value: &Value) -> Option<u256> {
    u256::from_str_radix(&text(value), 10).ok()
}

impl BridgeState {
    /// Takes the gateways' answers; false for anyone else's.
    pub fn answered(&mut self, ask: &Ask, result: &Result<Value, crate::node::Failure>) -> bool {
        let Ask::Call(body) = ask else {
            return false;
        };
        let Ok(body) = serde_json::from_str::<Value>(body) else {
            return false;
        };

        if body["purpose"] != "bridge" {
            return false;
        }

        let key = key_of(body["contract"].as_str().unwrap_or_default(), body["function"].as_str().unwrap_or_default(), &body["args"]);
        let answer = match result {
            Ok(value) if value["status"] == "returned" => Ok(value["values"].as_array().cloned().unwrap_or_default()),
            Ok(value) => Err(value["error"].as_str().unwrap_or("the contract would not say").to_string()),
            Err(failure) => Err(failure.message.clone()),
        };
        self.answers.insert(key, answer);

        true
    }

    /// What a contract here answers, asked when it never was or not lately.
    fn read(&mut self, ctx: &mut Ctx, contract: &Address, function: &str, args: Value) -> Option<Result<Vec<Value>, String>> {
        let key = key_of(&contract.to_string(), function, &args);
        let now = ctx.now as f64 / 1_000.0;

        if self.asked.get(&key).is_none_or(|when| now - when > ASK_EVERY) {
            self.asked.insert(key.clone(), now);
            ctx.node.ask(Ask::Call(json!({ "contract": contract.to_string(), "function": function, "args": args, "purpose": "bridge" }).to_string()));
        }

        self.answers.get(&key).cloned()
    }

    /// Asks again at once: a transaction moved what was read.
    fn forget(&mut self) {
        self.asked.clear();
    }
}

/// A transfer that left the gateway here, as its sent(nonce) gives it, checked against the digest
/// the gateway keeps, so it is the one the signers signed. identity is the gateway as transfers
/// write it: its address and when it was set up.
pub fn left(values: &[Value], identity: &[u8], evm_gateway: [u8; 20], here: u64, nonce: u64) -> Option<Transfer> {
    let transfer = Transfer {
        from: identity.to_vec(),
        to: evm_gateway.to_vec(),
        source: here,
        target: whole(values.first()?)?,
        nonce,
        kind: AssetKind::from_number(whole(values.get(1)?)?)?,
        origin: whole(values.get(2)?)?,
        asset: raw(values.get(3)?),
        id: wide(values.get(4)?)?,
        amount: wide(values.get(5)?)?,
        recipient: raw(values.get(6)?),
        name: text(values.get(7)?),
        symbol: text(values.get(8)?),
        decimals: u8::try_from(whole(values.get(9)?)?).ok()?,
        uri: text(values.get(10)?),
    };

    (raw(values.get(11)?) == transfer.digest()).then_some(transfer)
}

/// A Solidity call's input: its selector, then its arguments.
fn calldata(signature: &str, types: &[AbiType], values: &[AbiValue]) -> Vec<u8> {
    let selector = Hasher::digest(HashType::Keccak256, signature.as_bytes()).to_bytes();

    [&selector[..4], &encode_values(types, values).expect("the arguments fit their types")].concat()
}

/// What lets a gateway take an asset: approve for a token or an NFT, set_approval_for_all for a
/// multi-token collection; nothing for the coin.
pub fn evm_approval(kind: AssetKind, spender: [u8; 20], id: u256, amount: u256) -> Option<Vec<u8>> {
    match kind {
        AssetKind::Coin => None,
        AssetKind::Token => Some(calldata("approve(address,uint256)", &[AbiType::Address, AbiType::Uint(256)], &[AbiValue::Address(spender), AbiValue::Uint(amount)])),
        AssetKind::Nft => Some(calldata("approve(address,uint256)", &[AbiType::Address, AbiType::Uint(256)], &[AbiValue::Address(spender), AbiValue::Uint(id)])),
        AssetKind::Multi => Some(calldata("setApprovalForAll(address,bool)", &[AbiType::Address, AbiType::Bool], &[AbiValue::Address(spender), AbiValue::Bool(true)])),
    }
}

/// A Bitcoin address, bc1q… or tb1q…, or an output script, 0x…, as the script it pays.
pub fn output_script(text: &str) -> Option<Vec<u8>> {
    let text = text.trim();

    match text.strip_prefix("0x") {
        Some(_) => from_hex(text).filter(|script| (1..=80).contains(&script.len())),
        None => {
            let (_, program) = chainv4::structs::bech32::decode_segwit(text)?;

            Some([&[0x00, program.len() as u8][..], &program].concat())
        }
    }
}

/// A deposit's proof from Esplora's answers: the transaction as it holds it, witness and all, its
/// Merkle proof, the hash of the block at the proof's height, each hash as explorers write it, and
/// the same two for the block's coinbase.
pub fn esplora_proof(tx_hex: &str, merkle: &Value, block_hash: &str, coinbase_hex: &str, coinbase_merkle: &Value) -> Result<Proof, String> {
    let internal = |shown: &str| -> Option<Vec<u8>> {
        let mut bytes = from_hex(&format!("0x{}", shown.trim()))?;
        bytes.reverse();

        (bytes.len() == 32).then_some(bytes)
    };
    let base = |hex: &str| -> Result<Vec<u8>, String> { Ok(Transaction::read(&from_hex(&format!("0x{}", hex.trim())).ok_or("Esplora gave a transaction that is not hex")?)?.0.base()) };
    let path_of = |merkle: &Value| -> Result<Vec<u8>, String> {
        let path = merkle["merkle"]
            .as_array()
            .ok_or("Esplora gave no Merkle path: is it mined?")?
            .iter()
            .map(|sibling| internal(sibling.as_str().unwrap_or_default()).ok_or("Esplora gave a Merkle path that does not read"))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(path.concat())
    };

    Ok(Proof {
        block: internal(block_hash).ok_or("Esplora gave a block hash that does not read")?,
        raw: base(tx_hex)?,
        index: merkle["pos"].as_u64().ok_or("Esplora gave no place")?,
        path: path_of(merkle)?,
        coinbase: base(coinbase_hex)?,
        coinbase_path: path_of(coinbase_merkle)?,
    })
}

/// What a withdrawal of this much arrives as at the least: all but a tenth, the most the wallet lets
/// it lose to its share of a release's fee.
pub fn least_arriving(sats: u64) -> u64 {
    sats - sats / 10
}

/// The account's deposit address's script on Bitcoin, under the federation in force, for this
/// chain and this gateway: its P2WSH.
pub fn deposit_output(recipient: &Address, chain_id: u64, gateway: &Address, federation: &DecidedFederation) -> Vec<u8> {
    p2wsh(&btc_deposit_script(recipient, chain_id, gateway, federation.federation.threshold, &federation.federation.signers))
}

/// Every account's deposit output, for the light client to watch, once the chain proves its
/// directory names a Bitcoin gateway and its federation.
#[cfg(all(feature = "bitcoin-light", not(target_arch = "wasm32")))]
pub fn deposit_outputs(chain: &crate::chain::Chain, holders: &[crate::keys::Holder]) -> Vec<Vec<u8>> {
    let (Some(gateway), Some(federation), Some(genesis)) = (chain.announced().and_then(|directory| directory.btc_gateway), chain.federation_in_force(), chain.genesis.as_ref()) else {
        return Vec::new();
    };
    let chain_id = chainv4::crypto::ethereum::chain_id(genesis);

    holders.iter().filter_map(|holder| holder.addresses().first().map(|address| deposit_output(address, chain_id, &gateway, federation))).collect()
}

/// A call to a contract here, for the Send page.
fn native_call(from: Address, to: Address, function: &str, args: Vec<TxCallArgumentType>, coin: u256, what: String) -> Prepared {
    let amount = match u128::try_from(coin) {
        Ok(narrow) => Amount::U128(narrow),
        Err(_) => Amount::U256(coin),
    };

    Prepared {
        from,
        tx: Tx::ContractCall { to, func: Hasher::digest(HashType::Xxh3_64, function.as_bytes()), amount, data: args },
        what,
        about: to.to_string(),
        coin,
    }
}

/// The account's Ethereum address: its secp256k1 key's.
fn evm_address(ctx: &Ctx) -> Option<[u8; 20]> {
    ctx.holder()?.addresses().into_iter().find_map(|address| match address {
        Address::Secp256k1(bytes) => Some(bytes),
        _ => None,
    })
}

/// This chain's id, as the gateways name it: what its EVM answers CHAINID with.
fn here(ctx: &Ctx) -> Option<u64> {
    ctx.chain.genesis.as_ref().map(chainv4::crypto::ethereum::chain_id)
}

pub fn show(ui: &mut Ui, ctx: &mut Ctx, state: &mut BridgeState, send: &mut SendState) {
    let palette = *ctx.palette;
    theme::title(ui, &palette, "Bridge", "The coin, tokens and NFTs to and from EVM chains, and bitcoin in and out as wBTC, through the gateways the chain names.");

    let (reads, mined) = state.evm.take();

    if mined.is_some() {
        state.delivered_asked.clear();
    }

    for read in reads {
        match read {
            Read::Delivered { chain_id, digest, delivered } => {
                state.delivered.insert((chain_id, digest), delivered);
            }
        }
    }

    if let Some(found) = state.looked.lock().expect("never poisoned").pop() {
        state.found = Some(found);
    }

    state.evm.learn_chains(ctx);

    let (Some(directory), federation) = (ctx.chain.announced().cloned(), ctx.chain.federation_in_force().cloned()) else {
        theme::card(ui, &palette, |ui| {
            let why = match ctx.chain.directory {
                None => "Proving where the chain's bridges are…",
                Some(_) => "This chain names no bridges: its directory has no gateways.",
            };
            ui.label(RichText::new(why).color(palette.soft));
        });

        return;
    };

    overview(ui, ctx, &directory, federation.as_ref());
    ui.horizontal_wrapped(|ui| {
        ui.selectable_value(&mut state.way, Way::Out, "To an EVM chain");
        ui.selectable_value(&mut state.way, Way::In, "From an EVM chain");
        ui.selectable_value(&mut state.way, Way::Bitcoin, "Bitcoin");
    });
    theme::section_gap(ui);

    let prepared = match state.way {
        Way::Out => out(ui, ctx, state, &directory),
        Way::In => {
            into(ui, ctx, state, &directory);
            None
        }
        Way::Bitcoin => bitcoin(ui, ctx, state, &directory, federation.as_ref()),
    };

    if state.evm.pending.is_some() {
        ui.add_space(10.0);
        state.evm.show(ui, ctx, "a bridge's transaction", "chainv4 · bridge");
    }

    if let Some(prepared) = prepared {
        state.forget();
        send.prepare(prepared);
        *ctx.page = Page::Send;
    }
}

/// The federation and the gateways, as proven.
fn overview(ui: &mut Ui, ctx: &Ctx, directory: &Directory, federation: Option<&DecidedFederation>) {
    let palette = *ctx.palette;

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Who bridges");

        match federation {
            Some(decided) => super::row(ui, &palette, "Federation", |ui| {
                ui.label(format!("{} of {} signers, version {}", decided.federation.threshold, decided.federation.signers.len(), decided.version));
            }),
            None => super::row(ui, &palette, "Federation", |ui| {
                ui.label(RichText::new("none yet: nothing crosses until governance names one").color(palette.warn));
            }),
        }

        if let Some(gateway) = directory.gateway {
            super::row(ui, &palette, "Gateway", |ui| super::address(ui, &palette, &gateway.to_string()));
        }

        if let Some(gateway) = directory.btc_gateway {
            super::row(ui, &palette, "Bitcoin gateway", |ui| super::address(ui, &palette, &gateway.to_string()));
        }

        for evm in &directory.evm_gateways {
            super::row(ui, &palette, &chain_name(evm.chain_id), |ui| super::address(ui, &palette, &to_hex(&evm.gateway)));
        }

        ui.label(RichText::new("Proven by the light client from the chain's own state, as every balance here is.").small().color(palette.muted));
    });
}

/// The asset form both ways share: what, which, how much.
fn asset_form(ui: &mut Ui, ctx: &Ctx, state: &mut BridgeState, hint: &str) {
    let palette = *ctx.palette;

    super::row(ui, &palette, "What", |ui| {
        for (kind, label) in [(AssetKind::Coin, "The coin"), (AssetKind::Token, "A token"), (AssetKind::Nft, "An NFT"), (AssetKind::Multi, "A multi-token")] {
            ui.selectable_value(&mut state.kind, kind, label);
        }
    });

    if state.kind != AssetKind::Coin {
        super::row(ui, &palette, "Asset", |ui| {
            ui.add(egui::TextEdit::singleline(&mut state.asset).hint_text(hint).font(egui::TextStyle::Monospace).desired_width(f32::INFINITY));
        });
    }

    if matches!(state.kind, AssetKind::Nft | AssetKind::Multi) {
        super::row(ui, &palette, "Id", |ui| {
            ui.add(egui::TextEdit::singleline(&mut state.id).desired_width(160.0));
        });
    }

    if state.kind != AssetKind::Nft {
        super::row(ui, &palette, "Amount", |ui| {
            ui.add(egui::TextEdit::singleline(&mut state.amount).desired_width(160.0));
        });
    }
}

/// The chain chosen among those the directory names, offered where Settings gives its RPC.
fn chain_choice(ui: &mut Ui, ctx: &Ctx, state: &mut BridgeState, directory: &Directory) -> Option<(u64, [u8; 20])> {
    let palette = *ctx.palette;

    if directory.evm_gateways.is_empty() {
        ui.label(RichText::new("The directory names Gateway.sol on no EVM chain yet.").color(palette.soft));

        return None;
    }

    super::row(ui, &palette, "Chain", |ui| {
        for evm in &directory.evm_gateways {
            ui.selectable_value(&mut state.chain, Some(evm.chain_id), chain_name(evm.chain_id));
        }
    });

    directory.evm_gateways.iter().find(|evm| Some(evm.chain_id) == state.chain).map(|evm| (evm.chain_id, evm.gateway))
}

/// The id and amount the form says, in the asset's own units: the coin's and a token's by their
/// decimals, an NFT's amount 1.
fn id_and_amount(ctx: &Ctx, state: &BridgeState, decimals: u8) -> Option<(u256, u256)> {
    let id = match state.kind {
        AssetKind::Nft | AssetKind::Multi => u256::from_str_radix(state.id.trim(), 10).ok()?,
        _ => u256::ZERO,
    };
    let amount = match state.kind {
        AssetKind::Nft => u256::from(1u64),
        AssetKind::Multi => u256::from_str_radix(state.amount.trim(), 10).ok()?,
        AssetKind::Coin => u256::from_str_radix(&format::parse_units(&state.amount, ctx.coin.decimals)?, 10).ok()?,
        AssetKind::Token => u256::from_str_radix(&format::parse_units(&state.amount, decimals)?, 10).ok()?,
    };

    (amount > u256::ZERO).then_some((id, amount))
}

/// Sending from here to an EVM chain, and the account's transfers on their way.
fn out(ui: &mut Ui, ctx: &mut Ctx, state: &mut BridgeState, directory: &Directory) -> Option<Prepared> {
    let palette = *ctx.palette;
    let Some(gateway) = directory.gateway else {
        ui.label(RichText::new("The directory names no gateway here.").color(palette.soft));

        return None;
    };
    let me = ctx.holder().and_then(|holder| holder.addresses().first().copied())?;
    let mut prepared = None;

    if state.recipient.is_empty()
        && let Some(mine) = evm_address(ctx)
    {
        state.recipient = to_hex(&mine);
    }

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "To an EVM chain");
        let Some((chain_id, _)) = chain_choice(ui, ctx, state, directory) else {
            return;
        };
        asset_form(ui, ctx, state, "contract:0x…");
        super::row(ui, &palette, "To", |ui| {
            ui.add(egui::TextEdit::singleline(&mut state.recipient).hint_text("0x…, an address there").font(egui::TextStyle::Monospace).desired_width(f32::INFINITY));
        });

        let asset = state.asset.trim().parse::<Address>().ok();
        let decimals = asset.map_or(ctx.coin.decimals, |asset| ctx.token(&asset.to_string()).1);
        let recipient = from_hex(state.recipient.trim()).filter(|bytes| bytes.len() == 20);
        let ready = id_and_amount(ctx, state, decimals).zip(recipient).filter(|_| asset.is_some() || state.kind == AssetKind::Coin);

        let Some(((id, amount), recipient)) = ready else {
            ui.label(RichText::new("Fill in what to send and to whom.").small().color(palette.muted));

            return;
        };
        let send_call = |coin| {
            let args = vec![
                TxCallArgumentType::U64(chain_id),
                TxCallArgumentType::Bytes(recipient.clone()),
                TxCallArgumentType::U64(state.kind as u64),
                asset.map_or(TxCallArgumentType::Nil, TxCallArgumentType::Address),
                TxCallArgumentType::U256(id),
                TxCallArgumentType::U256(amount),
            ];
            native_call(me, gateway, "send", args, coin, format!("Send to {}", chain_name(chain_id)))
        };

        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| match (state.kind, asset) {
            (AssetKind::Coin, _) => {
                if theme::primary(ui, &palette, "Send", true).clicked() {
                    prepared = Some(send_call(amount));
                }
            }
            (kind, Some(asset)) => {
                // What the gateway wrapped it burns; approving it first costs only the call.
                let (function, args) = match kind {
                    AssetKind::Multi => ("set_approval_for_all", vec![TxCallArgumentType::Address(gateway), TxCallArgumentType::Bool(true)]),
                    AssetKind::Nft => ("approve", vec![TxCallArgumentType::Address(gateway), TxCallArgumentType::U256(id)]),
                    _ => ("approve", vec![TxCallArgumentType::Address(gateway), TxCallArgumentType::U256(amount)]),
                };

                if theme::quiet(ui, &palette, "1. Let the gateway take it").clicked() {
                    prepared = Some(native_call(me, asset, function, args, u256::ZERO, "Let the gateway take it".to_string()));
                }

                if theme::primary(ui, &palette, "2. Send", true).clicked() {
                    prepared = Some(send_call(u256::ZERO));
                }
            }
            _ => {}
        });
        ui.label(RichText::new("It arrives once the federation signed it, as the chain finalizes it, and someone delivered it there: anyone may, you too, below.").small().color(palette.muted));
    });

    sent(ui, ctx, state, directory, gateway, me);

    prepared
}

/// The account's latest transfers out: the board's count, whether Gateway.sol took each, and a
/// way to deliver one that waits.
fn sent(ui: &mut Ui, ctx: &mut Ctx, state: &mut BridgeState, directory: &Directory, gateway: Address, me: Address) {
    let palette = *ctx.palette;
    let Some(here) = here(ctx) else {
        return;
    };
    let count = state.read(ctx, &gateway, "sent_count", json!([])).and_then(|answer| answer.ok()?.first().and_then(whole)).unwrap_or(0);
    let identity = state.read(ctx, &gateway, "identity", json!([])).and_then(|answer| answer.ok()?.first().map(raw)).unwrap_or_default();
    let mut mine = Vec::new();

    for nonce in (count.saturating_sub(RECENT) + 1..=count).rev() {
        let Some(Ok(values)) = state.read(ctx, &gateway, "sent", json!([{ "type": "u64", "value": nonce.to_string() }])) else {
            continue;
        };

        if values.get(12).map(text) != Some(me.to_string()) {
            continue;
        }

        let target = values.first().and_then(whole).unwrap_or(0);

        if let Some(evm) = directory.evm_gateway(target)
            && let Some(transfer) = left(&values, &identity, evm, here, nonce)
        {
            mine.push(transfer);
        }
    }

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Your transfers out");

        if mine.is_empty() {
            ui.label(RichText::new("None among the latest.").color(palette.soft));
        }

        for transfer in mine {
            let digest = to_hex(&transfer.digest());
            let posted = state.read(ctx, &gateway, "signatures", json!([{ "type": "bytes", "value": digest }])).and_then(Result::ok).unwrap_or_default();
            let threshold = posted.first().and_then(whole).unwrap_or(u64::MAX) as usize;
            let signatures: Vec<[u8; 65]> = posted.iter().skip(1).filter_map(|value| raw(value).try_into().ok()).collect();
            let url = state.evm.url_for(ctx, transfer.target);
            let key = (transfer.target, digest.clone());

            if let Some(url) = &url
                && state.delivered_asked.get(&key).is_none_or(|when| ui.input(|input| input.time) - when > ASK_EVERY)
            {
                state.delivered_asked.insert(key.clone(), ui.input(|input| input.time));
                let input = calldata("delivered(bytes32)", &[AbiType::FixedBytes(32)], &[AbiValue::FixedBytes(transfer.delivered_key())]);
                let (chain_id, digest) = key.clone();
                let to: [u8; 20] = transfer.to.clone().try_into().unwrap_or([0; 20]);
                state.evm.call(ctx, url, &to, &input, move |answer| Read::Delivered { chain_id, digest, delivered: answer.map(|output| output.last() == Some(&1)) });
            }

            theme::end_row(ui, |ui| {
                ui.label(theme::mono(format!("#{}", transfer.nonce)).color(palette.muted));
                ui.add(egui::Label::new(format!("{} · {} to {}", transfer.symbol, format::units(&transfer.amount.to_string(), transfer.decimals), chain_name(transfer.target))).wrap());
            }, |ui| {
                match state.delivered.get(&key) {
                    Some(Ok(true)) => {
                        ui.colored_label(palette.good, "arrived");
                    }
                    _ if signatures.len() < threshold => {
                        ui.colored_label(palette.warn, format!("signing, {} of {}", signatures.len(), if threshold == usize::MAX { 0 } else { threshold }));
                    }
                    _ => match (&url, evm_address(ctx)) {
                        (Some(url), Some(from)) if state.evm.pending.is_none() => {
                            if theme::quiet(ui, &palette, &format!("Deliver on {}", chain_name(transfer.target))).clicked() {
                                let call = Call { to: transfer.to.clone().try_into().unwrap_or([0; 20]), value: u256::ZERO, input: transfer.deliver_input(&signatures), what: "Deliver a transfer" };
                                state.evm.start(ctx, digest.clone(), call, transfer.target, url.clone(), from);
                            }
                        }
                        (None, _) => {
                            ui.colored_label(palette.soft, "signed: to deliver, give its RPC in Settings");
                        }
                        _ => {
                            ui.colored_label(palette.soft, "signed, to deliver");
                        }
                    },
                }
            });
        }
    });
}

/// Sending from an EVM chain to here, signed there with the account's Ethereum key.
fn into(ui: &mut Ui, ctx: &mut Ctx, state: &mut BridgeState, directory: &Directory) {
    let palette = *ctx.palette;
    let Some(here) = here(ctx) else {
        return;
    };

    if state.recipient.trim().parse::<Address>().is_err()
        && let Some(me) = ctx.holder().and_then(|holder| holder.addresses().first().copied())
    {
        state.recipient = me.to_string();
    }

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "From an EVM chain");
        let Some((chain_id, evm_gateway)) = chain_choice(ui, ctx, state, directory) else {
            return;
        };
        asset_form(ui, ctx, state, "0x…, the token or collection there");
        super::row(ui, &palette, "To", |ui| {
            ui.add(egui::TextEdit::singleline(&mut state.recipient).hint_text("an address here").font(egui::TextStyle::Monospace).desired_width(f32::INFINITY));
        });

        let (Some(url), Some(from)) = (state.evm.url_for(ctx, chain_id), evm_address(ctx)) else {
            ui.label(RichText::new(format!("Give {}'s RPC in Settings, and use an account with an Ethereum key, to send from there.", chain_name(chain_id))).color(palette.soft));

            return;
        };
        let asset: Option<[u8; 20]> = from_hex(state.asset.trim()).and_then(|bytes| bytes.try_into().ok());
        // A token's decimals there are its own: the form takes its smallest units, as the coin's 18.
        let decimals = if state.kind == AssetKind::Coin { 18 } else { 0 };
        let recipient = state.recipient.trim().parse::<Address>().ok();
        let amounts = match state.kind {
            AssetKind::Coin => format::parse_units(&state.amount, decimals).and_then(|raw| u256::from_str_radix(&raw, 10).ok()).map(|amount| (u256::ZERO, amount)),
            _ => id_and_amount(ctx, state, decimals),
        };
        let ready = amounts.zip(recipient).filter(|_| asset.is_some() || state.kind == AssetKind::Coin);

        let Some(((id, amount), recipient)) = ready else {
            ui.label(RichText::new("Fill in what to send and to whom; a token's amount in its smallest units.").small().color(palette.muted));

            return;
        };
        let asset = asset.unwrap_or([0; 20]);
        let busy = state.evm.pending.is_some();

        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            if let Some(input) = evm_approval(state.kind, evm_gateway, id, amount)
                && theme::quiet(ui, &palette, "1. Let Gateway.sol take it").clicked()
                && !busy
            {
                state.evm.start(ctx, "approve".to_string(), Call { to: asset, value: u256::ZERO, input, what: "Let the gateway take it" }, chain_id, url.clone(), from);
            }

            let label = if state.kind == AssetKind::Coin { "Send" } else { "2. Send" };

            if theme::primary(ui, &palette, label, !busy).clicked() {
                let value = if state.kind == AssetKind::Coin { amount } else { u256::ZERO };
                let input = send_input(here, &recipient.to_abi(), state.kind, asset, id, amount);
                state.evm.start(ctx, "send".to_string(), Call { to: evm_gateway, value, input, what: "Send through the gateway" }, chain_id, url.clone(), from);
            }
        });
        ui.label(RichText::new("It arrives here once the federation signed it, as the chain there finalizes it: the coin as its wrapped token, a wrapped one as what it wraps.").small().color(palette.muted));
    });
}

/// Bitcoin in and out: the deposit address, deposits to show, withdrawals.
fn bitcoin(ui: &mut Ui, ctx: &mut Ctx, state: &mut BridgeState, directory: &Directory, federation: Option<&DecidedFederation>) -> Option<Prepared> {
    let palette = *ctx.palette;
    let (Some(gateway), Some(federation)) = (directory.btc_gateway, federation) else {
        ui.label(RichText::new("The directory names no Bitcoin gateway, or there is no federation yet.").color(palette.soft));

        return None;
    };
    let me = ctx.holder().and_then(|holder| holder.addresses().first().copied())?;
    let chain_id = here(ctx)?;
    let relay = state.read(ctx, &gateway, "relay", json!([])).and_then(|answer| answer.ok()?.first().map(text)).and_then(|text| text.parse::<Address>().ok());
    let network_name = relay.and_then(|relay| state.read(ctx, &relay, "network_name", json!([]))).and_then(|answer| answer.ok()?.first().map(text));
    let network = if network_name.as_deref() == Some("main") { Network::Main } else { Network::Test };
    let output = deposit_output(&me, chain_id, &gateway, federation);
    let deposit_address = address(&output, network);
    let mut prepared = None;
    let mint = |proof: &Proof, vout: u32| {
        let args = vec![
            TxCallArgumentType::Bytes(proof.block.clone()),
            TxCallArgumentType::Bytes(proof.raw.clone()),
            TxCallArgumentType::U64(proof.index),
            TxCallArgumentType::Bytes(proof.path.clone()),
            TxCallArgumentType::Bytes(proof.coinbase.clone()),
            TxCallArgumentType::Bytes(proof.coinbase_path.clone()),
            TxCallArgumentType::U64(u64::from(vout)),
            TxCallArgumentType::Address(me),
            TxCallArgumentType::U64(federation.version),
        ];
        native_call(me, gateway, "deposit", args, u256::ZERO, "Mint wBTC for a deposit".to_string())
    };

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Deposit bitcoin");
        ui.label(RichText::new("Pay this address from any Bitcoin wallet. Once it is buried deep enough, show it here, and its value is minted to this account as wBTC.").color(palette.soft));
        ui.add_space(6.0);
        super::receive::qr(ui, &deposit_address, 200.0);
        chainv4_ui::widgets::copy_on_click(ui, &palette, egui::Label::new(theme::mono(deposit_address.as_str()).color(palette.ink)), &deposit_address);
        ui.label(
            RichText::new(format!(
                "Computed here from the federation in force, {} of {}, version {}: no node told the wallet where to pay. It needs {} confirmations.",
                federation.federation.threshold,
                federation.federation.signers.len(),
                federation.version,
                federation.federation.bitcoin_depth
            ))
            .small()
            .color(palette.muted),
        );
        ui.add_space(8.0);

        #[cfg(all(feature = "bitcoin-light", not(target_arch = "wasm32")))]
        for testnet in [false, true] {
            let Some(light) = ctx.lights.on(testnet) else {
                continue;
            };

            for coin in light.coins(&output) {
                let depth = light.confirmations(coin.height);
                theme::end_row(ui, |ui| {
                    ui.vertical(|ui| {
                        ui.label(format::amount(&coin.value.to_string(), 8, "BTC"));
                        ui.label(RichText::new(format!("{depth} confirmations")).color(palette.muted));
                    });
                }, |ui| {
                    match &coin.proof {
                        Some(proof) if !proof.coinbase.is_empty() && depth >= federation.federation.bitcoin_depth => {
                            let bytes = |text: &str| from_hex(&format!("0x{text}")).unwrap_or_default();
                            let proof = Proof { block: bytes(&proof.block), raw: bytes(&proof.raw), index: u64::from(proof.index), path: bytes(&proof.path), coinbase: bytes(&proof.coinbase), coinbase_path: bytes(&proof.coinbase_path) };

                            if theme::primary(ui, &palette, "Mint wBTC", true).clicked() {
                                prepared = Some(mint(&proof, coin.vout));
                            }
                        }
                        Some(proof) if !proof.coinbase.is_empty() => {
                            ui.colored_label(palette.warn, "not deep enough yet");
                        }
                        _ => {
                            ui.colored_label(palette.soft, "found before proofs were kept: show it through Esplora below");
                        }
                    }
                });
            }
        }

        // Without the light client: the deposit looked up on Esplora.
        ui.add_space(6.0);
        ui.collapsing("Show a deposit through Esplora", |ui| {
            super::row(ui, &palette, "Transaction", |ui| {
                ui.add(egui::TextEdit::singleline(&mut state.txid).hint_text("its id, as explorers write it").font(egui::TextStyle::Monospace).desired_width(f32::INFINITY));
            });
            super::row(ui, &palette, "Output", |ui| {
                ui.add(egui::TextEdit::singleline(&mut state.vout).desired_width(60.0));
            });
            let esplora = ctx.settings.esplora.trim().trim_end_matches('/').to_string();

            if esplora.is_empty() {
                ui.label(RichText::new("Give an Esplora API in Settings.").small().color(palette.muted));
            } else if theme::quiet(ui, &palette, "Look it up").clicked() {
                look_up(ctx, state, &esplora);
            }

            match (&state.found, state.vout.trim().parse::<u32>()) {
                (Some(Ok(proof)), Ok(vout)) => {
                    if theme::primary(ui, &palette, "Mint wBTC", true).clicked() {
                        prepared = Some(mint(proof, vout));
                    }
                }
                (Some(Err(why)), _) => {
                    ui.colored_label(palette.bad, why.as_str());
                }
                _ => {}
            }
        });
    });

    let wbtc = state.read(ctx, &gateway, "wbtc", json!([])).and_then(|answer| answer.ok()?.first().map(text)).and_then(|text| text.parse::<Address>().ok());

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Withdraw bitcoin");

        if let Some(held) = wbtc.and_then(|wbtc| ctx.chain.balance(&wbtc.to_string(), &me.to_string())) {
            super::row(ui, &palette, "You hold", |ui| {
                ui.label(format::amount(&held.value.to_string(), 8, "wBTC"));
            });
        }

        super::row(ui, &palette, "To", |ui| {
            ui.add(egui::TextEdit::singleline(&mut state.withdraw_to).hint_text("bc1q…, or an output script 0x…").font(egui::TextStyle::Monospace).desired_width(f32::INFINITY));
        });
        super::row(ui, &palette, "Amount", |ui| {
            ui.add(egui::TextEdit::singleline(&mut state.withdraw_sats).hint_text("BTC").desired_width(160.0));
        });
        let script = output_script(&state.withdraw_to);
        let sats = format::parse_units(&state.withdraw_sats, 8).and_then(|raw| raw.parse::<u64>().ok());
        let least = state.read(ctx, &gateway, "least_withdrawal", json!([])).and_then(|answer| answer.ok()?.first().and_then(whole)).unwrap_or(10_000);

        if let Some((burned, arrives)) = sats.filter(|sats| *sats >= least).map(|sats| (sats, least_arriving(sats))) {
            ui.label(format!("You burn {} and get {} up to {}: the amount less its share of the release's fee, at most a tenth of it.", format::amount(&burned.to_string(), 8, "wBTC"), format::amount(&arrives.to_string(), 8, "BTC"), format::amount(&burned.to_string(), 8, "BTC")));
        }

        if theme::primary(ui, &palette, "Withdraw", script.is_some() && sats.is_some_and(|sats| sats >= least)).clicked()
            && let (Some(script), Some(sats)) = (script, sats)
        {
            prepared = Some(native_call(me, gateway, "withdraw", vec![TxCallArgumentType::Bytes(script), TxCallArgumentType::U64(sats), TxCallArgumentType::U64(sats - least_arriving(sats))], u256::ZERO, "Withdraw wBTC to Bitcoin".to_string()));
        }

        ui.label(RichText::new(format!("Burned here, it is paid in the federation's next release, at least {} satoshis: a Taproot address as its 0x5120… script. A release's fee comes off the withdrawals it pays, split evenly.", format::thousands(&least.to_string()))).small().color(palette.muted));
        ui.add_space(8.0);
        theme::caps(ui, &palette, "Your withdrawals");
        let count = state.read(ctx, &gateway, "withdrawal_count", json!([])).and_then(|answer| answer.ok()?.first().and_then(whole)).unwrap_or(0);
        let mut any = false;

        for id in (count.saturating_sub(RECENT) + 1..=count).rev() {
            let Some(Ok(values)) = state.read(ctx, &gateway, "withdrawal", json!([{ "type": "u64", "value": id.to_string() }])) else {
                continue;
            };

            if values.get(3).map(text) != Some(me.to_string()) {
                continue;
            }

            any = true;
            let paid = values.get(2).map(text).unwrap_or_default();
            ui.horizontal_wrapped(|ui| {
                ui.label(theme::mono(format!("#{id}")).color(palette.muted));
                let burned = values.get(1).and_then(whole).unwrap_or(0);

                // Once a release takes it, what arrives is known; until then, up to what it may lose.
                match values.get(5).and_then(whole) {
                    Some(share) => {
                        ui.label(format!("{}, after {} in fees", format::amount(&burned.saturating_sub(share).to_string(), 8, "BTC"), format::amount(&share.to_string(), 8, "BTC")));
                    }
                    None => {
                        ui.label(format!("{}, up to {} less in fees", format::amount(&burned.to_string(), 8, "BTC"), format::amount(&values.get(4).and_then(whole).unwrap_or(0).to_string(), 8, "BTC")));
                    }
                }

                ui.label(theme::mono(format::short(&address(&values.first().map(raw).unwrap_or_default(), network))).color(palette.soft));
                ui.colored_label(if paid == "paid" { palette.good } else { palette.warn }, paid);
            });
        }

        if !any {
            ui.label(RichText::new("None among the latest.").color(palette.soft));
        }
    });

    prepared
}

/// Looks a deposit's proof up on Esplora: the transaction, its Merkle proof, and its block.
fn look_up(ctx: &Ctx, state: &mut BridgeState, esplora: &str) {
    let Ok(client) = crate::app::client(ctx.settings) else {
        return;
    };
    let (looked, wake, txid, base) = (state.looked.clone(), ctx.wake.clone(), state.txid.trim().to_string(), esplora.to_string());
    state.found = None;
    let get = |answer: Result<chainv4_ui::net::Answer, String>| answer.map(|answer| String::from_utf8_lossy(&answer.bytes).into_owned());
    let (client_two, client_three, client_four, client_five, client_six) = (client.clone(), client.clone(), client.clone(), client.clone(), client.clone());

    client.fetch(format!("{base}/tx/{txid}/hex"), None, move |answer| {
        let tx = match get(answer) {
            Ok(tx) => tx,
            Err(why) => return finish(&looked, &wake, Err(why)),
        };

        client_two.fetch(format!("{base}/tx/{txid}/merkle-proof"), None, move |answer| {
            let merkle = match get(answer).and_then(|text| serde_json::from_str::<Value>(&text).map_err(|_| "Esplora gave a proof that is not JSON".to_string())) {
                Ok(merkle) => merkle,
                Err(why) => return finish(&looked, &wake, Err(why)),
            };
            let Some(height) = merkle["block_height"].as_u64() else {
                return finish(&looked, &wake, Err("the transaction is not in a block yet".to_string()));
            };

            client_three.fetch(format!("{base}/block-height/{height}"), None, move |answer| {
                let block = match get(answer) {
                    Ok(block) => block,
                    Err(why) => return finish(&looked, &wake, Err(why)),
                };

                // The block's coinbase, which shows how tall its tree is.
                client_four.fetch(format!("{base}/block/{}/txid/0", block.trim()), None, move |answer| {
                    let coinbase = match get(answer) {
                        Ok(coinbase) => coinbase.trim().to_string(),
                        Err(why) => return finish(&looked, &wake, Err(why)),
                    };

                    client_five.fetch(format!("{base}/tx/{coinbase}/hex"), None, move |answer| {
                        let coinbase_hex = match get(answer) {
                            Ok(coinbase_hex) => coinbase_hex,
                            Err(why) => return finish(&looked, &wake, Err(why)),
                        };

                        client_six.fetch(format!("{base}/tx/{coinbase}/merkle-proof"), None, move |answer| {
                            let found = get(answer)
                                .and_then(|text| serde_json::from_str::<Value>(&text).map_err(|_| "Esplora gave a proof that is not JSON".to_string()))
                                .and_then(|coinbase_merkle| esplora_proof(&tx, &merkle, &block, &coinbase_hex, &coinbase_merkle));

                            finish(&looked, &wake, found);
                        });
                    });
                });
            });
        });
    });
}

fn finish(looked: &Arc<Mutex<Vec<Result<Proof, String>>>>, wake: &egui::Context, found: Result<Proof, String>) {
    looked.lock().expect("never poisoned").push(found);
    wake.request_repaint();
}

#[cfg(test)]
mod tests;
