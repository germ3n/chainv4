//! The bridges (docs/bridges.md): the federation that vouches for what crosses, and the contracts
//! the chain's directory names. The gateway here, contracts/lua/gateway.lua, is read by its events:
//! what left for an EVM chain and how many signers signed it, what arrived and was done, and the
//! wrapped assets it made, each with what it stands for. The Bitcoin gateway,
//! contracts/lua/btc_gateway.lua, is read by its calls and events: its relay's tip, wBTC minted
//! against the coins it holds, deposits, withdrawals, and the releases that pay them, each input
//! with the signatures posted for it. What happens on the other chains is theirs to show: a
//! transfer delivered to Gateway.sol, or a release broadcast to Bitcoin, shows here once it comes
//! back proven.

use {
    super::{Ctx, View},
    crate::api::{Ask, EventQuery},
    crate::format,
    crate::model::{Argument, Called, FoundEvent},
    chainv4::crypto::bitcoin::tx::{Network, Transaction, address as bitcoin_address},
    chainv4::structs::address::Address,
    chainv4::structs::hex::{from_hex, to_hex},
    chainv4_ui::widgets,
    egui::{RichText, Ui},
    serde_json::{Value, json},
    std::collections::HashMap,
};

/// How long an answer stands before it is asked for again, in milliseconds.
const REFRESH: u64 = 15_000;
/// How many of a gateway's events a page takes.
const EVENTS: u32 = 300;
/// The most withdrawals, releases and held outputs asked after, newest first.
const MOST: u64 = 30;

#[derive(Default)]
pub struct BridgeState {
    /// Bitcoin's test networks' addresses, tb1q…, rather than Bitcoin's own: as the relay follows.
    testnet: bool,
    asked: HashMap<Ask, u64>,
}

impl BridgeState {
    fn network(&self) -> Network {
        match self.testnet {
            true => Network::Test,
            false => Network::Main,
        }
    }
}

/// Which way a transfer went, as the gateway's events tell it.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Way {
    /// Left through this gateway, by its nonce, for a chain.
    Out { nonce: u64, target: u64 },
    /// Arrived and was done, from a chain, by its nonce there.
    In { source: u64, nonce: u64 },
    /// Signed here but never sent from here, nor done yet: one arriving, as its signers vouch.
    Arriving,
}

/// A transfer through the gateway, by its digest.
#[derive(Clone, Debug)]
struct Crossing {
    digest: String,
    way: Way,
    /// Who signed it, by key, with the transaction each signature came in, oldest first.
    signers: Vec<(String, String)>,
    /// Where it first showed: its Sent event, or its first signature.
    height: u64,
    tx: String,
    /// The transaction it was done in, for one that arrived.
    done_in: Option<String>,
}

impl Crossing {
    /// Where it stands, against the federation's threshold.
    fn state(&self, threshold: Option<u64>) -> String {
        let signed = self.signers.len() as u64;

        match (&self.way, threshold) {
            (Way::In { .. }, _) => "received".to_string(),
            (Way::Out { .. }, Some(threshold)) if signed >= threshold => "signed, to relay".to_string(),
            (_, Some(threshold)) => format!("signing, {signed} of {threshold}"),
            (_, None) => format!("{signed} signed"),
        }
    }
}

/// A wrapped asset the gateway made: what it stands for, and where it is here.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Wrapped {
    origin: u64,
    kind: u64,
    /// Its address on its origin chain, as hex: none for a coin.
    asset: String,
    here: String,
    tx: String,
}

/// What a value an event or call gave holds, as hex: bytes as they are, and text as its bytes, as
/// a Lua string of bytes that happen to be UTF-8 comes back as text.
fn bytes_hex(argument: &Argument) -> String {
    match argument.kind.as_str() {
        "bytes" => argument.text().to_ascii_lowercase(),
        _ => to_hex(argument.text().as_bytes()),
    }
}

fn number(argument: Option<&Argument>) -> u64 {
    argument.map_or(0, |argument| argument.text().parse().unwrap_or(0))
}

/// The gateway's transfers, newest first, from its events: Sent(nonce, target, digest, sender),
/// Attested(digest, signer) and Received(source, nonce, digest).
fn crossings(events: &[FoundEvent]) -> Vec<Crossing> {
    let mut by_digest: HashMap<String, Crossing> = HashMap::new();
    let first = |event: &FoundEvent, digest: &str, way: Way| Crossing { digest: digest.to_string(), way, signers: Vec::new(), height: event.height, tx: event.tx.clone(), done_in: None };

    // Oldest first, so a signature or a receipt lands on what it is for.
    for event in events.iter().rev() {
        match event.name.as_str() {
            "Sent" => {
                let digest = event.data.get(2).map(bytes_hex).unwrap_or_default();
                let way = Way::Out { nonce: number(event.data.first()), target: number(event.data.get(1)) };
                let crossing = by_digest.entry(digest.clone()).or_insert_with(|| first(event, &digest, way.clone()));
                (crossing.way, crossing.height, crossing.tx) = (way, event.height, event.tx.clone());
            }
            "Attested" => {
                let digest = event.data.first().map(bytes_hex).unwrap_or_default();
                let signer = event.data.get(1).map(bytes_hex).unwrap_or_default();
                let crossing = by_digest.entry(digest.clone()).or_insert_with(|| first(event, &digest, Way::Arriving));

                if !crossing.signers.iter().any(|(key, _)| *key == signer) {
                    crossing.signers.push((signer, event.tx.clone()));
                }
            }
            "Received" => {
                let digest = event.data.get(2).map(bytes_hex).unwrap_or_default();
                let way = Way::In { source: number(event.data.first()), nonce: number(event.data.get(1)) };
                let crossing = by_digest.entry(digest.clone()).or_insert_with(|| first(event, &digest, way.clone()));
                (crossing.way, crossing.done_in) = (way, Some(event.tx.clone()));
            }
            _ => {}
        }
    }

    let mut list: Vec<Crossing> = by_digest.into_values().collect();
    list.sort_by(|one, two| (two.height, &two.digest).cmp(&(one.height, &one.digest)));

    list
}

/// The wrapped assets the gateway made, newest first, from its Wrapped(origin, kind, asset, here)
/// events.
fn wrapped(events: &[FoundEvent]) -> Vec<Wrapped> {
    events
        .iter()
        .filter(|event| event.name == "Wrapped")
        .map(|event| Wrapped {
            origin: number(event.data.first()),
            kind: number(event.data.get(1)),
            asset: event.data.get(2).map(bytes_hex).unwrap_or_default(),
            here: event.data.get(3).map(Argument::text).unwrap_or_default(),
            tx: event.tx.clone(),
        })
        .collect()
}

fn kind_name(kind: u64) -> &'static str {
    match kind {
        0 => "coin",
        1 => "token",
        2 => "NFT",
        3 => "multi-token",
        _ => "?",
    }
}

/// An address the gateways write as bytes: an EVM chain's 20, this chain's 33, as each chain
/// writes it; none for the coin.
fn party(hex: &str) -> String {
    let bytes = from_hex(hex).unwrap_or_default();

    match bytes.len() {
        0 => String::new(),
        33 => Address::from_abi(&bytes).map_or_else(|| hex.to_string(), |address| address.to_string()),
        _ => to_hex(&bytes),
    }
}

/// A Bitcoin hash, held in Bitcoin's own byte order, as explorers write it: reversed, no 0x.
fn shown_hash(internal: &str) -> String {
    let mut bytes = from_hex(internal).unwrap_or_default();
    bytes.reverse();

    to_hex(&bytes).trim_start_matches("0x").to_string()
}

/// An output the Bitcoin gateway keys as hex:vout, in Bitcoin's byte order, as explorers write it.
fn outpoint(key: &str) -> String {
    match key.split_once(':') {
        Some((txid, vout)) => format!("{}:{vout}", shown_hash(&format!("0x{txid}"))),
        None => key.to_string(),
    }
}

/// How many signatures an input of a release holds, and how many it needs, from what
/// input_signatures returns: the script, the threshold, then a signer's place and signature each.
fn signed_input(values: &[String]) -> Option<(usize, u64)> {
    let threshold = values.get(1)?.parse().ok()?;

    Some((values.len().saturating_sub(2) / 2, threshold))
}

/// A release, as proposal returns it: its transaction, unsigned, its id, the withdrawals it pays,
/// the federation it was proposed under, and its state.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Release {
    raw: Vec<u8>,
    txid: String,
    withdrawals: Vec<u64>,
    version: u64,
    state: String,
}

fn release_from(values: &[Argument]) -> Option<Release> {
    let raw = from_hex(&bytes_hex(values.first()?))?;
    let withdrawals = values.get(2).map(Argument::text).unwrap_or_default().split(',').filter_map(|id| id.trim().parse().ok()).collect();

    Some(Release {
        raw,
        txid: shown_hash(&bytes_hex(values.get(1)?)),
        withdrawals,
        version: number(values.get(3)),
        state: values.get(4).map(Argument::text).unwrap_or_default(),
    })
}

fn btc(sats: &str) -> String {
    format::amount(sats, 8, "BTC")
}

fn ask(ctx: &mut Ctx, state: &mut BridgeState, asked: Ask) {
    let stale = state.asked.get(&asked).is_none_or(|when| ctx.now.saturating_sub(*when) > REFRESH);

    if stale && !ctx.api.asking(&asked) {
        state.asked.insert(asked.clone(), ctx.now);
        ctx.api.ask(asked);
    }
}

/// What a read-only call returned, once it answered: Err with the contract's refusal.
fn called(ctx: &mut Ctx, state: &mut BridgeState, contract: &str, function: &str, args: Value) -> Option<Result<Vec<Argument>, String>> {
    let asked = Ask::Call { contract: contract.to_string(), function: function.to_string(), args: args.to_string() };
    ask(ctx, state, asked.clone());
    let called: Called = serde_json::from_value(ctx.store.calls.get(&asked)?.clone()).ok()?;

    Some(match called.status.as_str() {
        "returned" => Ok(called.values),
        _ => Err(called.error.unwrap_or_else(|| "the contract would not say".to_string())),
    })
}

/// A call's values as text, once it answered without a refusal.
fn texts(ctx: &mut Ctx, state: &mut BridgeState, contract: &str, function: &str, args: Value) -> Option<Vec<String>> {
    Some(called(ctx, state, contract, function, args)?.ok()?.iter().map(Argument::text).collect())
}

fn whole(value: u64) -> Value {
    json!({ "type": "u64", "value": value.to_string() })
}

/// A contract's events, newest first, from the event index, asked for again once old; None,
/// having said why, while there are none to show.
fn indexed(ui: &mut Ui, ctx: &mut Ctx, contract: &str) -> Option<Vec<FoundEvent>> {
    let look = ctx.look;
    let query = EventQuery::contract(contract, None).limited(EVENTS);
    let newest = Ask::Events { query: query.clone(), cursor: None };
    let found = ctx.store.events.get(&query);

    if found.is_none_or(|found| ctx.now.saturating_sub(found.refreshed) > REFRESH) {
        ctx.api.ask(newest.clone());
    }

    match ctx.store.events.get(&query) {
        Some(found) => Some(found.items.clone()),
        None => {
            match ctx.store.failures.get(&newest) {
                Some(failure) if failure.status == 404 => super::note(ui, look, "This node keeps no event index: start it with --index-events to list what crossed."),
                _ => super::waiting(ui, ctx, &newest),
            }

            None
        }
    }
}

/// A chain by its id: this one, or an EVM chain by name.
fn chain(ctx: &Ctx, id: u64) -> String {
    match ctx.store.status.as_ref().and_then(|status| status.chain_id) {
        Some(here) if here == id => "this chain".to_string(),
        _ => chainv4::swap::evm::chain_name(id),
    }
}

/// The federation's threshold, as the node says it.
fn threshold(ctx: &Ctx) -> Option<u64> {
    ctx.store.status.as_ref()?.federation.as_ref()?.get("threshold")?.as_u64()
}

/// A link to a transfer's page, by its digest.
fn crossing_link(ui: &mut Ui, ctx: &mut Ctx, digest: &str) {
    let text = RichText::new(format::short(digest)).monospace().color(ctx.look.palette.accent);

    if ui.link(text).on_hover_text(digest).clicked() {
        ctx.go(View::Transfer(digest.to_string()));
    }
}

pub fn show(ui: &mut Ui, ctx: &mut Ctx, state: &mut BridgeState) {
    let look = ctx.look;
    super::heading(ui, look, "Bridges", "Coins, tokens and NFTs carried to other chains and back as wrapped assets: what leaves is held or burned here, and a federation of signers vouches for each side.", |_| {});
    federation(ui, ctx);
    let status = ctx.store.status.as_ref();
    let (gateway, btc_gateway) = (status.and_then(|status| status.announced("gateway")), status.and_then(|status| status.announced("btc_gateway")));

    if gateway.is_none() && btc_gateway.is_none() {
        super::note(ui, look, "This chain's directory names no gateway: governance has not announced one.");

        return;
    }

    if let Some(gateway) = gateway {
        evm_bridge(ui, ctx, &gateway);
    }

    if let Some(btc_gateway) = btc_gateway {
        bitcoin(ui, ctx, state, &btc_gateway);
    }
}

/// The federation in force, and where the chain's directory says the gateways are.
fn federation(ui: &mut Ui, ctx: &mut Ctx) {
    let look = ctx.look;
    let Some(status) = ctx.store.status.clone() else {
        return;
    };

    super::section(ui, look, "Federation", |ui| {
        let Some(federation) = &status.federation else {
            super::note(ui, look, "This chain has no federation: neither its genesis nor its governance named one, so nothing crosses its bridges.");

            return;
        };
        let signers: Vec<String> = federation["signers"].as_array().into_iter().flatten().filter_map(|key| key.as_str().map(str::to_string)).collect();
        let number = |key: &str| federation[key].as_u64().map_or_else(|| "—".to_string(), |value| value.to_string());

        super::facts(ui, look, "bridge federation", |facts| {
            facts.text("version", number("version"));
            facts.text("signs", format!("{} of {}", number("threshold"), signers.len()));
            facts.row("signers", |ui| {
                ui.vertical(|ui| {
                    for signer in &signers {
                        ui.monospace(format::short(signer)).on_hover_text(signer);
                    }
                });
            });
            facts.text("waits", format!("{} blocks here, {} on an EVM chain, {} Bitcoin confirmations", number("chain_depth"), number("evm_depth"), number("bitcoin_depth")));
        });
        ui.add_space(6.0);
        let gateways: Vec<(u64, String)> = status
            .directory
            .as_ref()
            .and_then(|directory| directory["evm_gateways"].as_array().cloned())
            .unwrap_or_default()
            .iter()
            .filter_map(|evm| Some((evm["chain_id"].as_u64()?, evm["gateway"].as_str()?.to_string())))
            .collect();

        super::facts(ui, look, "bridge directory", |facts| {
            for (name, key) in [("gateway", "gateway"), ("Bitcoin gateway", "btc_gateway")] {
                match status.announced(key) {
                    Some(address) => facts.row(name, |ui| super::address(ui, ctx, &address)),
                    None => facts.text(name, "none announced"),
                }
            }

            for (chain_id, gateway) in &gateways {
                facts.row(&format!("on {}", chain(ctx, *chain_id)), |ui| super::copyable(ui, gateway));
            }
        });
    });
}

/// The gateway with EVM chains: what crossed it, and the wrapped assets it made.
fn evm_bridge(ui: &mut Ui, ctx: &mut Ctx, gateway: &str) {
    let look = ctx.look;

    super::section(ui, look, "EVM chains", |ui| {
        let Some(events) = indexed(ui, ctx, gateway) else {
            return;
        };
        let (crossings, made) = (crossings(&events), wrapped(&events));
        let count = |matches: &dyn Fn(&Way) -> bool| crossings.iter().filter(|crossing| matches(&crossing.way)).count().to_string();
        super::stats(ui, look, &[
            ("left".to_string(), count(&|way| matches!(way, Way::Out { .. })), "sent from here".to_string()),
            ("arrived".to_string(), count(&|way| matches!(way, Way::In { .. })), "done here".to_string()),
            ("arriving".to_string(), count(&|way| *way == Way::Arriving), "being signed".to_string()),
            ("wrapped".to_string(), made.len().to_string(), "assets made here".to_string()),
        ]);

        if crossings.is_empty() {
            super::note(ui, look, "Nothing has crossed yet.");
        } else {
            transfers_table(ui, ctx, gateway, &crossings);
        }

        if !made.is_empty() {
            ui.add_space(8.0);
            widgets::caps(ui, &look.palette, "Wrapped assets");
            if look.narrow {
                for asset in &made {
                    let original = party(&asset.asset);
                    super::list_row(ui, look, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(widgets::strong(kind_name(asset.kind), 13.5).color(look.palette.ink));
                            super::address(ui, ctx, &asset.here);
                        });
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new("stands for").small().color(look.palette.muted));

                            match original.is_empty() {
                                true => ui.label(format!("the coin, {}", chainv4::swap::evm::symbol(asset.origin))),
                                false => ui.monospace(format::short(&original)).on_hover_text(&original),
                            };
                            ui.label(RichText::new(format!("on {} · made in", chain(ctx, asset.origin))).small().color(look.palette.muted));
                            super::tx(ui, ctx, &asset.tx);
                        });
                    });
                }

                return;
            }

            super::table(ui, look, ("bridge wrapped", gateway.to_string()), &["here", "stands for", "kind", "from", "made in"], |ui| {
                for asset in &made {
                    super::address(ui, ctx, &asset.here);
                    let original = party(&asset.asset);

                    match original.is_empty() {
                        true => ui.label(format!("the coin, {}", chainv4::swap::evm::symbol(asset.origin))),
                        false => ui.monospace(format::short(&original)).on_hover_text(&original),
                    };
                    ui.label(kind_name(asset.kind));
                    ui.label(chain(ctx, asset.origin));
                    super::tx(ui, ctx, &asset.tx);
                    super::row_end(ui);
                }
            });
        }
    });
}

fn transfers_table(ui: &mut Ui, ctx: &mut Ctx, gateway: &str, crossings: &[Crossing]) {
    let look = ctx.look;
    let threshold = threshold(ctx);

    // Each crossing's way, chain, nonce, and state in its color.
    let shown = |ctx: &Ctx, crossing: &Crossing| {
        let (way, chain_id, nonce) = match crossing.way {
            Way::Out { nonce, target } => ("out", Some(target), Some(nonce)),
            Way::In { source, nonce } => ("in", Some(source), Some(nonce)),
            Way::Arriving => ("in", None, None),
        };
        let state = crossing.state(threshold);
        let color = match crossing.way {
            Way::In { .. } => look.palette.good,
            _ if state.starts_with("signed") => look.palette.accent,
            _ => look.palette.warn,
        };

        (way, chain_id.map_or_else(|| "—".to_string(), |id| chain(ctx, id)), nonce.map_or_else(|| "—".to_string(), |nonce| nonce.to_string()), state, color)
    };

    if look.narrow {
        for crossing in crossings.iter().take(50) {
            let (way, chain, nonce, state, color) = shown(ctx, crossing);
            super::list_row(ui, look, |ui| {
                ui.horizontal(|ui| {
                    ui.label(widgets::strong(way, 13.5).color(look.palette.ink));
                    crossing_link(ui, ctx, &crossing.digest);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| ui.colored_label(color, &state));
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(format!("{chain} · nonce {nonce} · in")).small().color(look.palette.muted));
                    super::tx(ui, ctx, crossing.done_in.as_deref().unwrap_or(&crossing.tx));
                });
            });
        }

        return;
    }

    super::table(ui, look, ("bridge transfers", gateway.to_string()), &["digest", "way", "chain", "nonce", "state", "in"], |ui| {
        for crossing in crossings.iter().take(50) {
            crossing_link(ui, ctx, &crossing.digest);
            let (way, chain, nonce, state, color) = shown(ctx, crossing);
            ui.label(way);
            ui.label(chain);
            ui.monospace(nonce);
            ui.colored_label(color, state);
            super::tx(ui, ctx, crossing.done_in.as_deref().unwrap_or(&crossing.tx));
            super::row_end(ui);
        }
    });
}

/// A transfer's page, by its digest: which way it went, what it carries, and who signed it.
pub fn transfer(ui: &mut Ui, ctx: &mut Ctx, state: &mut BridgeState, digest: &str) {
    let look = ctx.look;
    super::heading(ui, look, "Transfer", "One transfer through the gateway, as the federation signs it: both chains' gateways compute this digest alike.", |_| {});
    let Some(gateway) = ctx.store.status.as_ref().and_then(|status| status.announced("gateway")) else {
        super::note(ui, look, "This chain's directory names no gateway.");

        return;
    };
    let Some(events) = indexed(ui, ctx, &gateway) else {
        return;
    };
    let Some(crossing) = crossings(&events).into_iter().find(|crossing| crossing.digest == digest.to_ascii_lowercase()) else {
        super::note(ui, look, "The gateway's latest events hold no transfer with that digest.");

        return;
    };
    let threshold = threshold(ctx);

    super::section(ui, look, "Transfer", |ui| {
        super::facts(ui, look, "bridge transfer", |facts| {
            facts.row("digest", |ui| super::copyable(ui, &crossing.digest));

            match crossing.way {
                Way::Out { nonce, target } => facts.text("way", format!("out to {}, nonce {nonce}", chain(ctx, target))),
                Way::In { source, nonce } => facts.text("way", format!("in from {}, nonce {nonce}", chain(ctx, source))),
                Way::Arriving => facts.text("way", "in, being signed"),
            }

            facts.text("state", crossing.state(threshold));
            facts.row("first seen", |ui| super::tx(ui, ctx, &crossing.tx));

            if let Some(done_in) = &crossing.done_in {
                facts.row("done in", |ui| super::tx(ui, ctx, done_in));
            }
        });
    });

    if let Way::Out { nonce, target } = crossing.way {
        sent(ui, ctx, state, &gateway, nonce, target);
    }

    super::section(ui, look, "Signatures", |ui| {
        // The board's own count, the federation in force's, rather than every key the events name.
        if let Some(Ok(values)) = called(ctx, state, &gateway, "signatures", json!([{ "type": "bytes", "value": crossing.digest }])) {
            let needed = number(values.first());
            let posted = values.len().saturating_sub(1);
            super::meter(ui, look, posted as f32 / needed.max(1) as f32, &format!("{posted} of {needed} the federation in force needs"));
            ui.add_space(6.0);
        }

        if crossing.signers.is_empty() {
            super::note(ui, look, "No signer has signed it yet.");
        }

        for (signer, tx) in &crossing.signers {
            ui.horizontal(|ui| {
                ui.monospace(format::short(signer)).on_hover_text(signer);
                super::tx(ui, ctx, tx);
            });
        }

        if matches!(crossing.way, Way::Out { .. }) && threshold.is_some_and(|threshold| crossing.signers.len() as u64 >= threshold) {
            super::note(ui, look, "Signed enough: anyone may relay the signatures to Gateway.sol there, whose delivered(digest) says whether it arrived.");
        }
    });
}

/// What a transfer that left carries, as the gateway's sent(nonce) keeps it.
fn sent(ui: &mut Ui, ctx: &mut Ctx, state: &mut BridgeState, gateway: &str, nonce: u64, target: u64) {
    let look = ctx.look;
    let Some(Ok(values)) = called(ctx, state, gateway, "sent", json!([whole(nonce)])) else {
        return;
    };
    let text = |at: usize| values.get(at).map(Argument::text).unwrap_or_default();
    let (kind, origin, id, amount) = (number(values.get(1)), number(values.get(2)), text(4), text(5));
    let (name, symbol, decimals) = (text(7), text(8), number(values.get(9)).min(u64::from(u8::MAX)) as u8);
    let symbol = match (kind, symbol.is_empty()) {
        (0, true) => ctx.coin().1,
        _ => symbol,
    };
    let what = match kind {
        2 => format!("#{id} of {name}"),
        3 => format!("{amount} of #{id} of {name}"),
        _ => format::amount(&amount, decimals, &symbol),
    };
    let (asset, recipient) = (values.get(3).map(bytes_hex).map(|hex| party(&hex)).unwrap_or_default(), values.get(6).map(bytes_hex).map(|hex| party(&hex)).unwrap_or_default());
    let (uri, sender) = (text(10), text(12));

    super::section(ui, look, "Carries", |ui| {
        super::facts(ui, look, "bridge sent", |facts| {
            facts.text("what", format!("{what} ({})", kind_name(kind)));

            // Gateways from before it kept no sender.
            if !sender.is_empty() && sender != "null" {
                facts.row("from", |ui| super::address(ui, ctx, &sender));
            }

            if !asset.is_empty() {
                facts.row("asset", |ui| super::address(ui, ctx, &asset));
            }

            facts.text("native to", chain(ctx, origin));
            facts.row(&format!("to, on {}", chain(ctx, target)), |ui| super::copyable(ui, &recipient));

            if !uri.is_empty() {
                facts.row("URI", |ui| super::copyable(ui, &uri));
            }
        });
    });
}

/// The Bitcoin gateway: its relay, wBTC against what it holds, deposits, withdrawals and releases.
fn bitcoin(ui: &mut Ui, ctx: &mut Ctx, state: &mut BridgeState, gateway: &str) {
    let look = ctx.look;

    super::section(ui, look, "Bitcoin", |ui| {
        networks(ctx, state, gateway);
        relay(ui, ctx, state, gateway);
        let held = held(ctx, state, gateway);
        let minted = texts(ctx, state, gateway, "wbtc", json!([])).and_then(|values| values.into_iter().next()).and_then(|wbtc| {
            let supply = texts(ctx, state, &wbtc, "total_supply", json!([]))?.into_iter().next()?;

            Some((wbtc, supply))
        });
        let unspent: u64 = held.iter().flatten().filter(|utxo| utxo.state != "spent").map(|utxo| utxo.sats).sum();
        super::stats(ui, look, &[
            ("wBTC minted".to_string(), minted.as_ref().map_or_else(|| "—".to_string(), |(_, supply)| format::units(supply, 8)), "in circulation".to_string()),
            ("BTC held".to_string(), format::units(&unspent.to_string(), 8), "unspent or pending".to_string()),
        ]);

        if let Some((wbtc, _)) = &minted {
            ui.horizontal(|ui| {
                ui.label(RichText::new("wBTC").color(look.palette.muted));
                super::address(ui, ctx, wbtc);
            });
        }

        if let Some(held) = held {
            held_table(ui, ctx, gateway, &held);
        }

        if let Some(events) = indexed(ui, ctx, gateway) {
            deposits(ui, ctx, gateway, &events);
            withdrawals(ui, ctx, state, gateway, &events);
        }

        releases(ui, ctx, state, gateway);
    });
}

/// Which network the gateway's Bitcoin scripts are written as addresses for: the one its relay
/// follows, main as Bitcoin's own and regtest or test as the test networks'.
fn networks(ctx: &mut Ctx, state: &mut BridgeState, gateway: &str) {
    let relay = texts(ctx, state, gateway, "relay", json!([])).and_then(|values| values.into_iter().next());
    let name = relay.and_then(|relay| texts(ctx, state, &relay, "network_name", json!([]))).and_then(|values| values.into_iter().next());

    if let Some(name) = name {
        state.testnet = name != "main";
    }
}

/// The relay's tip, by the relay the gateway names.
fn relay(ui: &mut Ui, ctx: &mut Ctx, state: &mut BridgeState, gateway: &str) {
    let look = ctx.look;

    match called(ctx, state, gateway, "relay", json!([])) {
        Some(Ok(values)) => {
            let relay = values.first().map(Argument::text).unwrap_or_default();
            let tip = called(ctx, state, &relay, "tip", json!([])).and_then(Result::ok);

            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("relay").color(look.palette.muted));
                super::address(ui, ctx, &relay);

                match tip {
                    Some(tip) => {
                        let hash = tip.first().map(bytes_hex).map(|hash| shown_hash(&hash)).unwrap_or_default();
                        ui.label(format!("at Bitcoin height {},", format::thousands(&number(tip.get(1)).to_string())));
                        ui.label(RichText::new("block").color(look.palette.muted));
                        ui.monospace(format::short(&hash)).on_hover_text(hash);
                    }
                    None => {
                        ui.spinner();
                    }
                }
            });
        }
        Some(Err(_)) => super::note(ui, look, "The gateway does not say which relay it checks deposits against."),
        None => {
            ui.spinner();
        }
    }

    ui.add_space(6.0);
}

/// An output the Bitcoin gateway holds, as utxo returns it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Held {
    outpoint: String,
    sats: u64,
    version: u64,
    /// The 33 bytes of the address it was a deposit for, as hex, or none for change.
    recipient: String,
    state: String,
}

fn held_from(values: &[Argument]) -> Option<Held> {
    Some(Held {
        outpoint: outpoint(&values.first()?.text()),
        sats: number(values.get(1)),
        version: number(values.get(2)),
        recipient: values.get(3).map(bytes_hex).unwrap_or_default(),
        state: values.get(4).map(Argument::text).unwrap_or_default(),
    })
}

/// The latest outputs the gateway holds, MOST of them at most, newest first, once all answered.
fn held(ctx: &mut Ctx, state: &mut BridgeState, gateway: &str) -> Option<Vec<Held>> {
    let count = texts(ctx, state, gateway, "utxo_count", json!([]))?.first()?.parse::<u64>().ok()?;
    // Every one asked at once, the list shown once all answered.
    let found: Vec<Option<Held>> = (count.saturating_sub(MOST)..count)
        .rev()
        .map(|n| called(ctx, state, gateway, "utxo", json!([whole(n)])).and_then(Result::ok).and_then(|values| held_from(&values)))
        .collect();

    found.into_iter().collect()
}

fn held_table(ui: &mut Ui, ctx: &mut Ctx, gateway: &str, held: &[Held]) {
    let look = ctx.look;

    if held.is_empty() {
        return;
    }

    ui.add_space(8.0);
    widgets::caps(ui, &look.palette, "Held on Bitcoin");
    super::table(ui, look, ("bridge held", gateway.to_string()), &["output", "amount", "for", "federation", "state"], |ui| {
        for utxo in held {
            ui.monospace(format::short(&utxo.outpoint)).on_hover_text(&utxo.outpoint);
            ui.monospace(btc(&utxo.sats.to_string()));
            let recipient = party(&utxo.recipient);

            if recipient.is_empty() {
                ui.label(RichText::new("change").color(look.palette.muted));
            } else {
                super::address(ui, ctx, &recipient);
            }

            ui.monospace(utxo.version.to_string());
            ui.label(&utxo.state);
            super::row_end(ui);
        }
    });
}

/// Deposits shown and minted, from DepositedBtc(txid, vout, sats, recipient) events.
fn deposits(ui: &mut Ui, ctx: &mut Ctx, gateway: &str, events: &[FoundEvent]) {
    let look = ctx.look;
    let found: Vec<&FoundEvent> = events.iter().filter(|event| event.name == "DepositedBtc").take(50).collect();

    ui.add_space(8.0);
    widgets::caps(ui, &look.palette, "Deposits");

    if found.is_empty() {
        super::note(ui, look, "No deposit shown yet.");

        return;
    }

    super::table(ui, look, ("bridge deposits", gateway.to_string()), &["Bitcoin transaction", "amount", "minted to", "shown in"], |ui| {
        for event in found {
            let txid = format!("{}:{}", shown_hash(&event.data.first().map(bytes_hex).unwrap_or_default()), number(event.data.get(1)));
            ui.monospace(format::short(&txid)).on_hover_text(&txid);
            ui.monospace(btc(&event.data.get(2).map(Argument::text).unwrap_or_default()));
            super::address(ui, ctx, &event.data.get(3).map(Argument::text).unwrap_or_default());
            super::tx(ui, ctx, &event.tx);
            super::row_end(ui);
        }
    });
}

/// A withdrawal's fee, from what withdrawal returns: its share once a release took it, or the most
/// it may lose until then.
fn withdrawal_fee(values: &[String]) -> String {
    match (values.get(5).filter(|share| share.parse::<u64>().is_ok()), values.get(4).filter(|most| most.parse::<u64>().is_ok())) {
        (Some(share), _) => btc(share),
        (None, Some(most)) => format!("up to {}", btc(most)),
        (None, None) => "—".to_string(),
    }
}

/// Withdrawals, from WithdrawRequested(id, script, sats, sender) events, each with where it stands
/// now.
fn withdrawals(ui: &mut Ui, ctx: &mut Ctx, state: &mut BridgeState, gateway: &str, events: &[FoundEvent]) {
    let look = ctx.look;
    let found: Vec<FoundEvent> = events.iter().filter(|event| event.name == "WithdrawRequested").take(MOST as usize).cloned().collect();

    ui.add_space(8.0);
    widgets::caps(ui, &look.palette, "Withdrawals");

    if found.is_empty() {
        super::note(ui, look, "No withdrawal yet.");

        return;
    }

    let network = state.network();
    let rows: Vec<(u64, String, String, String, String, String, String)> = found
        .iter()
        .map(|event| {
            let id = number(event.data.first());
            let script = from_hex(&event.data.get(1).map(bytes_hex).unwrap_or_default()).unwrap_or_default();
            let said = texts(ctx, state, gateway, "withdrawal", json!([whole(id)]));
            let standing = said.as_ref().and_then(|values| values.get(2).cloned()).unwrap_or_else(|| "…".to_string());
            let sats = event.data.get(2).map(Argument::text).unwrap_or_default();
            let fee = said.as_ref().map_or("…".to_string(), |values| withdrawal_fee(values));
            let by = event.data.get(3).map(Argument::text).unwrap_or_default();

            (id, by, bitcoin_address(&script, network), sats, fee, standing, event.tx.clone())
        })
        .collect();

    super::note(ui, look, "Each arrives as what it burned, less its share of its release's fee, up to the most it agreed to lose: the fee comes off the withdrawals, not off what backs wBTC.");
    super::table(ui, look, ("bridge withdrawals", gateway.to_string()), &["#", "by", "to", "burned", "fee", "state", "asked in"], |ui| {
        for (id, by, to, sats, fee, standing, tx) in &rows {
            ui.monospace(id.to_string());

            // Gateways from before it named no sender.
            match by.is_empty() {
                true => {
                    ui.label(RichText::new("—").color(look.palette.muted));
                }
                false => super::address(ui, ctx, by),
            }

            ui.monospace(format::short(to)).on_hover_text(to);
            ui.monospace(btc(sats));
            ui.monospace(fee);
            let color = match standing.as_str() {
                "paid" => look.palette.good,
                "queued" => look.palette.warn,
                _ => look.palette.soft,
            };
            ui.colored_label(color, standing);
            super::tx(ui, ctx, tx);
            super::row_end(ui);
        }
    });
}

/// The latest releases, newest first, each opening its page.
fn releases(ui: &mut Ui, ctx: &mut Ctx, state: &mut BridgeState, gateway: &str) {
    let look = ctx.look;
    let Some(count) = texts(ctx, state, gateway, "proposal_count", json!([])).and_then(|values| values.first()?.parse::<u64>().ok()) else {
        return;
    };

    ui.add_space(8.0);
    widgets::caps(ui, &look.palette, "Releases");

    if count == 0 {
        super::note(ui, look, "No release proposed yet.");

        return;
    }

    let found: Vec<(u64, Option<Release>)> =
        (count.saturating_sub(MOST) + 1..=count).rev().map(|id| (id, called(ctx, state, gateway, "proposal", json!([whole(id)])).and_then(Result::ok).and_then(|values| release_from(&values)))).collect();

    super::table(ui, look, ("bridge releases", gateway.to_string()), &["#", "Bitcoin transaction", "pays", "state"], |ui| {
        for (id, release) in &found {
            if ui.link(RichText::new(format!("#{id}")).monospace().color(look.palette.accent)).clicked() {
                ctx.go(View::Release(*id));
            }

            match release {
                Some(release) => {
                    ui.monospace(format::short(&release.txid)).on_hover_text(&release.txid);
                    ui.label(match release.withdrawals.len() {
                        0 => "a sweep".to_string(),
                        1 => "1 withdrawal".to_string(),
                        many => format!("{many} withdrawals"),
                    });
                    ui.label(&release.state);
                }
                None => {
                    ui.spinner();
                    ui.label("");
                    ui.label("");
                }
            }

            super::row_end(ui);
        }
    });
}

/// A release's page: its transaction, each input with the signatures posted for it, and each
/// output with what it pays.
pub fn release(ui: &mut Ui, ctx: &mut Ctx, state: &mut BridgeState, id: u64) {
    let look = ctx.look;
    super::heading(ui, look, &format!("Release #{id}"), "A Bitcoin transaction paying withdrawals from what the gateway holds, which the federation signs input by input before anyone broadcasts it.", |_| {});
    let Some(gateway) = ctx.store.status.as_ref().and_then(|status| status.announced("btc_gateway")) else {
        super::note(ui, look, "This chain's directory names no Bitcoin gateway.");

        return;
    };
    let asked = called(ctx, state, &gateway, "proposal", json!([whole(id)]));
    let Some(release) = asked.as_ref().map(|answer| answer.as_ref().ok().and_then(|values| release_from(values))) else {
        ui.spinner();

        return;
    };
    let Some(release) = release else {
        super::note(ui, look, "The gateway holds no release by that number.");

        return;
    };
    let tx = match Transaction::read(&release.raw) {
        Ok((tx, _)) => tx,
        Err(why) => {
            ui.colored_label(look.palette.bad, format!("Its transaction does not read: {why}"));

            return;
        }
    };

    networks(ctx, state, &gateway);

    super::section(ui, look, "Release", |ui| {
        super::facts(ui, look, "bridge release", |facts| {
            facts.row("transaction", |ui| super::copyable(ui, &release.txid));
            facts.text("state", &release.state);
            facts.text("federation", format!("version {}", release.version));
            facts.text("pays", match release.withdrawals.is_empty() {
                true => "a sweep to the federation in force".to_string(),
                false => release.withdrawals.iter().map(|id| format!("#{id}")).collect::<Vec<_>>().join(", "),
            });
        });
    });

    super::section(ui, look, "Inputs", |ui| {
        super::table(ui, look, ("release inputs", id), &["#", "spends", "signatures"], |ui| {
            for (index, input) in tx.inputs.iter().enumerate() {
                let spends = format!("{}:{}", shown_hash(&to_hex(&input.txid)), input.vout);
                ui.monospace(index.to_string());
                ui.monospace(format::short(&spends)).on_hover_text(&spends);

                match texts(ctx, state, &gateway, "input_signatures", json!([whole(id), whole(index as u64)])).as_deref().and_then(signed_input) {
                    Some((signed, needed)) => {
                        let color = if signed as u64 >= needed { look.palette.good } else { look.palette.warn };
                        ui.colored_label(color, format!("{signed} of {needed}"));
                    }
                    None => {
                        ui.spinner();
                    }
                }

                super::row_end(ui);
            }
        });
    });

    let network = state.network();

    super::section(ui, look, "Outputs", |ui| {
        super::table(ui, look, ("release outputs", id), &["#", "to", "amount", "for"], |ui| {
            for (index, output) in tx.outputs.iter().enumerate() {
                let to = bitcoin_address(&output.script, network);
                ui.monospace(index.to_string());
                ui.monospace(format::short(&to)).on_hover_text(&to);
                ui.monospace(btc(&output.value.to_string()));
                ui.label(release.withdrawals.get(index).map_or_else(|| "change, to the federation".to_string(), |withdrawal| format!("withdrawal #{withdrawal}")));
                super::row_end(ui);
            }
        });
    });
}

#[cfg(test)]
mod tests;
