//! The chain's DEXes: contracts/lua/dex.lua, whose pools hold the coin, tokens and ERC-20s, and the
//! EVM's DexRouter, whose pairs hold ERC-20s and the coin as WDEV. Every pool with what it holds, its
//! price, its shares, and what traded through it over the last day and week, with the fees it kept;
//! a pool's price over time and its latest trades, adds and removals, each linked to its
//! transaction. The native DEX is read with read-only calls and its events; the EVM's through the
//! node's JSON-RPC and its pairs' logs.

use {
    super::chart::{self, Kind, Series},
    super::Ctx,
    crate::api::{Ask, EventQuery},
    crate::format,
    crate::model::{Called, FoundEvent},
    chainv4::crypto::secp256k1::keccak,
    chainv4::dex::{COIN, DexPool},
    chainv4::structs::hex::{from_hex, to_hex},
    chainv4::structs::u256::u256,
    chainv4_ui::widgets,
    egui::{RichText, Ui},
    serde_json::{Value, json},
    std::collections::HashMap,
};

/// How long an answer stands before it is asked for again, in milliseconds.
const REFRESH: u64 = 15_000;
/// The most pools listed.
const MAX_POOLS: u64 = 40;
/// How many events a page of them takes, for volumes and charts.
const EVENTS: u32 = 500;

const DAY_MS: u64 = 86_400_000;

#[derive(Default)]
pub struct DexState {
    evm: bool,
    /// The pool open, by its share token.
    chosen: Option<String>,
    /// When each question was last asked.
    asked: HashMap<Ask, u64>,
}

pub fn show(ui: &mut Ui, ctx: &mut Ctx, state: &mut DexState) {
    let look = ctx.look;
    super::heading(ui, look, "DEX", "Constant-product pools: each trades its two assets at the price that keeps the product of what it holds, less the fee the chain sets, 0.3% by default, for who provided.", |_| {});
    let (native, router) = (native(ctx), router(ctx));

    match (&native, router) {
        (None, None) => {
            super::note(ui, look, "This chain names no DEX: its directory names none, and neither does the node.");

            return;
        }
        (Some(_), Some(_)) => {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut state.evm, false, "Native pools");
                ui.selectable_value(&mut state.evm, true, "EVM pools");
            });
            ui.add_space(6.0);
        }
        (Some(_), None) => state.evm = false,
        (None, Some(_)) => state.evm = true,
    }

    let pools = match (state.evm, &native, router) {
        (false, Some(dex), _) => native_pools(ctx, state, dex),
        (true, _, Some(router)) => evm_pools(ctx, state, router),
        _ => None,
    };
    let Some(pools) = pools else {
        super::section(ui, look, "Pools", |ui| {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("Reading the pools…").color(look.palette.muted));
            });
        });

        return;
    };
    let trades = match (state.evm, &native) {
        (false, Some(dex)) => native_trades(ctx, dex),
        _ => pools.iter().flat_map(|pool| evm_trades(ctx, pool)).collect(),
    };
    let ms_per_block = block_ms(ctx);
    let tip = ctx.store.status.as_ref().map_or(0, |status| status.height);

    super::section(ui, look, "Pools", |ui| {
        if pools.is_empty() {
            super::note(ui, look, "No pools yet: the first deposit of two assets opens one.");

            return;
        }

        // Each pool as its row shows it: its pair, what it holds, its price, and its volumes and fees.
        let rows: Vec<[String; 6]> = pools
            .iter()
            .map(|pool| {
                let (a, b) = (symbol(ctx, &pool.a, state.evm), symbol(ctx, &pool.b, state.evm));
                let (day, week, fees) = (volume(&trades, pool, tip, DAY_MS / ms_per_block), volume(&trades, pool, tip, 7 * DAY_MS / ms_per_block), fees(&trades, pool, tip, 7 * DAY_MS / ms_per_block));

                [
                    // A pair may have a pool at each tier.
                    format!("{a} / {b} · {}%", f64::from(pool.fee_bps) / 100.0),
                    format!("{} and {}", shown(ctx, &pool.a, pool.reserve_a, state.evm), shown(ctx, &pool.b, pool.reserve_b, state.evm)),
                    price(ctx, pool, state.evm).map_or("—".to_string(), |price| format!("{price:.6} {b}")),
                    shown(ctx, &pool.a, day, state.evm),
                    shown(ctx, &pool.a, week, state.evm),
                    shown(ctx, &pool.a, fees, state.evm),
                ]
            })
            .collect();
        let mut pick = |ui: &mut Ui, pool: &DexPool, pair: &str| {
            let chosen = state.chosen.as_deref() == Some(pool.share.to_string().as_str());

            if ui.selectable_label(chosen, widgets::strong(pair, 13.5)).clicked() {
                state.chosen = Some(pool.share.to_string());
            }
        };

        match look.narrow {
            true => {
                for (pool, [pair, holds, price, day, week, fees]) in pools.iter().zip(&rows) {
                    super::list_row(ui, look, |ui| {
                        widgets::end_row(ui, |ui| pick(ui, pool, pair), |ui| ui.monospace(price));
                        ui.label(holds);
                        ui.label(RichText::new(format!("volume {day} a day, {week} a week · fees {fees} a week")).small().color(look.palette.muted));
                    });
                }
            }
            false => {
                super::table(ui, look, ("dex pools", state.evm), &["pool", "holds", "price", "volume, day", "volume, week", "fees, week"], |ui| {
                    for (pool, [pair, holds, price, day, week, fees]) in pools.iter().zip(&rows) {
                        pick(ui, pool, pair);
                        ui.label(holds);

                        for number in [price, day, week, fees] {
                            ui.monospace(number);
                        }

                        super::row_end(ui);
                    }
                });
            }
        }
        ui.label(
            RichText::new(format!(
                "Volumes and fees in each pool's first asset, from its last {EVENTS} trades at most, over about a day and a week of blocks at {} each.",
                format::span(ms_per_block)
            ))
            .small()
            .color(look.palette.muted),
        );
    });

    if let Some(pool) = state.chosen.as_deref().and_then(|chosen| pools.iter().find(|pool| pool.share.to_string() == chosen)).cloned() {
        ui.add_space(10.0);
        pool_page(ui, ctx, state, &pool, &native, &trades);
    }
}

/// The native DEX: the one the chain's directory names, or the one the node names.
fn native(ctx: &Ctx) -> Option<String> {
    let status = ctx.store.status.as_ref()?;

    status.announced("dex").or_else(|| status.dex.as_ref()?.get("contract")?.as_str().map(str::to_string))
}

/// The EVM DEX's router: the directory's, or the one the node names.
fn router(ctx: &Ctx) -> Option<[u8; 20]> {
    let status = ctx.store.status.as_ref()?;
    let text = status.announced("dex_router").or_else(|| status.dex.as_ref()?.get("router")?.as_str().map(str::to_string))?;

    from_hex(&text)?.try_into().ok()
}

/// Asks a question when it was never asked, or not lately.
fn ask(ctx: &mut Ctx, state: &mut DexState, asked: Ask) {
    let stale = state.asked.get(&asked).is_none_or(|when| ctx.now.saturating_sub(*when) > ctx.settings.pace().every_ms(REFRESH));

    if stale && !ctx.api.asking(&asked) {
        state.asked.insert(asked.clone(), ctx.now);
        ctx.api.ask(asked);
    }
}

/// What a native contract's function returned, as text, once it answered.
fn called(ctx: &mut Ctx, state: &mut DexState, contract: &str, function: &str, args: Value) -> Option<Vec<String>> {
    let asked = Ask::Call { contract: contract.to_string(), function: function.to_string(), args: args.to_string() };
    ask(ctx, state, asked.clone());
    let called: Called = serde_json::from_value(ctx.store.calls.get(&asked)?.clone()).ok()?;

    (called.status == "returned").then(|| called.values.iter().map(|value| value.text()).collect())
}

fn native_pools(ctx: &mut Ctx, state: &mut DexState, dex: &str) -> Option<Vec<DexPool>> {
    let count = called(ctx, state, dex, "pool_count", json!([]))?.first()?.parse::<u64>().ok()?.min(MAX_POOLS);
    let mut pools = Vec::new();

    for index in 0..count {
        if let Some(pool) = called(ctx, state, dex, "pool_at", json!([{ "type": "i64", "value": index.to_string() }])).and_then(|values| DexPool::from_values(&values).ok()) {
            pools.push(pool);
        }
    }

    Some(pools)
}

/// An EVM read through the node's JSON-RPC: what it returned, once it answered.
fn rpc(ctx: &mut Ctx, state: &mut DexState, to: &[u8; 20], signature: &str, words: &[[u8; 32]]) -> Option<Vec<u8>> {
    let input = [keccak(signature.as_bytes())[..4].to_vec(), words.concat()].concat();
    let body = json!({ "jsonrpc": "2.0", "id": 1, "method": "eth_call", "params": [{ "to": to_hex(to), "data": to_hex(&input) }, "latest"] });
    let asked = Ask::Rpc(body.to_string());
    ask(ctx, state, asked.clone());

    from_hex(ctx.store.calls.get(&asked)?.get("result")?.as_str()?)
}

fn address_at(output: &[u8], index: usize) -> Option<[u8; 20]> {
    output.get(index * 32 + 12..(index + 1) * 32)?.try_into().ok()
}

fn number_at(output: &[u8], index: usize) -> Option<u256> {
    Some(u256::from_be_bytes(output.get(index * 32..(index + 1) * 32)?.try_into().ok()?))
}

fn word(number: u64) -> [u8; 32] {
    u256::from(number).to_be_bytes()
}

/// The EVM DEX's pairs, with WDEV as the coin.
fn evm_pools(ctx: &mut Ctx, state: &mut DexState, router: [u8; 20]) -> Option<Vec<DexPool>> {
    let factory = address_at(&rpc(ctx, state, &router, "factory()", &[])?, 0)?;
    let wdev = address_at(&rpc(ctx, state, &router, "wdev()", &[])?, 0)?;
    let count = u64::try_from(number_at(&rpc(ctx, state, &factory, "allPairsLength()", &[])?, 0)?).ok()?.min(MAX_POOLS);
    let coin = |token: [u8; 20]| if token == wdev { COIN } else { chainv4::structs::address::Address::Evm(token) };
    let mut pools = Vec::new();

    for index in 0..count {
        let Some(pair) = rpc(ctx, state, &factory, "allPairs(uint256)", &[word(index)]).and_then(|output| address_at(&output, 0)) else {
            continue;
        };
        let parts = (
            rpc(ctx, state, &pair, "token0()", &[]).and_then(|output| address_at(&output, 0)),
            rpc(ctx, state, &pair, "token1()", &[]).and_then(|output| address_at(&output, 0)),
            rpc(ctx, state, &pair, "getReserves()", &[]),
            rpc(ctx, state, &pair, "fee()", &[]).and_then(|output| u32::try_from(number_at(&output, 0)?).ok()),
        );

        if let (Some(zero), Some(one), Some(reserves), Some(fee_bps)) = parts
            && let (Some(held0), Some(held1)) = (number_at(&reserves, 0), number_at(&reserves, 1))
        {
            let share = chainv4::structs::address::Address::Evm(pair);
            pools.push(DexPool { a: coin(zero), b: coin(one), fee_bps, reserve_a: held0, reserve_b: held1, share, venue: chainv4::dex::Venue::Evm });
        }
    }

    Some(pools)
}

/// A trade, add or removal, as either DEX's events say it.
#[derive(Clone, Debug)]
struct Activity {
    kind: &'static str,
    /// The pool's assets, in this pool's order, and the amounts of each: in, then out, for a trade.
    a: chainv4::structs::address::Address,
    b: chainv4::structs::address::Address,
    /// For a trade: what went in of a, of b, came out of a, of b.
    amounts: [u256; 4],
    /// The pool's tier; none from the native DEX's events before tiers, which every pool of the
    /// pair matches.
    fee_bps: Option<u32>,
    height: u64,
    tx: String,
    block: String,
}

fn amount(event: &FoundEvent, index: usize) -> u256 {
    event.data.get(index).and_then(|value| u256::from_str_radix(&value.text(), 10).ok()).unwrap_or(u256::ZERO)
}

fn asset(event: &FoundEvent, index: usize) -> Option<chainv4::structs::address::Address> {
    event.data.get(index)?.text().parse().ok()
}

/// The native DEX's trades, adds and removals, newest first, from its event index.
fn native_trades(ctx: &mut Ctx, dex: &str) -> Vec<Activity> {
    events_of(ctx, EventQuery::contract(dex, None).limited(EVENTS)).iter().filter_map(native_activity).collect()
}

/// One of the native DEX's events as a trade, add or removal.
fn native_activity(event: &FoundEvent) -> Option<Activity> {
    let (kind, amounts, tier) = match event.name.as_str() {
        // Swap(asset_in, asset_out, sender, amount_in, amount_out, fee)
        "Swap" => ("trade", [amount(event, 3), u256::ZERO, u256::ZERO, amount(event, 4)], 5),
        // Mint and Burn(a, b, sender, amount_a, amount_b, shares, fee)
        "Mint" => ("add", [amount(event, 3), amount(event, 4), u256::ZERO, u256::ZERO], 6),
        "Burn" => ("take out", [u256::ZERO, u256::ZERO, amount(event, 3), amount(event, 4)], 6),
        _ => return None,
    };
    let fee_bps = event.data.get(tier).and_then(|value| value.text().parse().ok());

    Some(Activity { kind, a: asset(event, 0)?, b: asset(event, 1)?, amounts, fee_bps, height: event.height, tx: event.tx.clone(), block: event.block.clone() })
}

/// What asks the node's index for an EVM pair's logs.
fn pair_logs(pool: &DexPool) -> EventQuery {
    EventQuery::contract(&pool.share.to_string(), Some(chainv4::structs::event::EVM_LOG)).limited(EVENTS)
}

/// An EVM pair's trades, adds and removals, from its logs.
fn evm_trades(ctx: &mut Ctx, pool: &DexPool) -> Vec<Activity> {
    events_of(ctx, pair_logs(pool)).iter().filter_map(|event| evm_activity(pool, event)).collect()
}

/// One of an EVM pair's logs, topics first and then its data, as a trade, add or removal.
fn evm_activity(pool: &DexPool, event: &FoundEvent) -> Option<Activity> {
    let topic = |signature: &str| to_hex(&keccak(signature.as_bytes()));
    let first = event.data.first()?.text();
    let data = from_hex(&event.data.last()?.text())?;
    let word = |index: usize| number_at(&data, index).unwrap_or(u256::ZERO);
    let (kind, amounts) = match first {
        topic_ if topic_ == topic("Swap(address,uint256,uint256,uint256,uint256,address)") => ("trade", [word(0), word(1), word(2), word(3)]),
        topic_ if topic_ == topic("Mint(address,uint256,uint256)") => ("add", [word(0), word(1), u256::ZERO, u256::ZERO]),
        topic_ if topic_ == topic("Burn(address,uint256,uint256,address)") => ("take out", [u256::ZERO, u256::ZERO, word(0), word(1)]),
        _ => return None,
    };

    Some(Activity { kind, a: pool.a, b: pool.b, amounts, fee_bps: Some(pool.fee_bps), height: event.height, tx: event.tx.clone(), block: event.block.clone() })
}

fn events_of(ctx: &mut Ctx, query: EventQuery) -> Vec<FoundEvent> {
    let newest = Ask::Events { query: query.clone(), cursor: None };
    let found = ctx.store.events.get(&query);

    if found.is_none_or(|found| ctx.now.saturating_sub(found.refreshed) > ctx.settings.pace().every_ms(REFRESH)) {
        ctx.api.ask(newest);
    }

    ctx.store.events.get(&query).map(|found| found.items.clone()).unwrap_or_default()
}

/// A pool's trades, in its order of assets: what went in of a and b, and came out of each.
fn in_pool(pool: &DexPool, activity: &Activity) -> Option<[u256; 4]> {
    let [in_first, in_second, out_first, out_second] = activity.amounts;

    if activity.fee_bps.is_some_and(|fee| fee != pool.fee_bps) {
        return None;
    }

    match (activity.a == pool.a && activity.b == pool.b, activity.a == pool.b && activity.b == pool.a) {
        (true, _) => Some([in_first, in_second, out_first, out_second]),
        (_, true) => Some([in_second, in_first, out_second, out_first]),
        _ => None,
    }
}

/// What traded through a pool over the last so many blocks, in its first asset: what went in of
/// it, and what came out of it.
fn volume(trades: &[Activity], pool: &DexPool, tip: u64, blocks: u64) -> u256 {
    trades
        .iter()
        .filter(|trade| trade.kind == "trade" && trade.height + blocks > tip)
        .filter_map(|trade| in_pool(pool, trade))
        .fold(u256::ZERO, |sum, [in_a, _, out_a, _]| sum + in_a + out_a)
}

/// The fees a pool's trades paid over the last so many blocks, in its first asset: its tier's fee on
/// what came in, of the second asset at today's price.
fn fees(trades: &[Activity], pool: &DexPool, tip: u64, blocks: u64) -> u256 {
    trades
        .iter()
        .filter(|trade| trade.kind == "trade" && trade.height + blocks > tip)
        .filter_map(|trade| in_pool(pool, trade))
        .fold(u256::ZERO, |sum, [in_a, in_b, _, _]| {
            let in_b_as_a = pool.matching(&pool.b, in_b).unwrap_or(u256::ZERO);

            sum + chainv4::dex::fee_of(in_a + in_b_as_a, pool.fee_bps)
        })
}

/// About how long a block takes, from the blocks loaded, or two seconds.
fn block_ms(ctx: &Ctx) -> u64 {
    let store = ctx.store;
    let (Some((low, low_id)), Some((high, high_id))) = (store.main.iter().next(), store.main.iter().next_back()) else {
        return 2_000;
    };
    let time = |id: &String| store.blocks.get(id).map(|block| block.timestamp);

    match (time(low_id), time(high_id)) {
        (Some(early), Some(late)) if high > low && late > early => ((late - early) / (high - low)).max(1),
        _ => 2_000,
    }
}

fn symbol(ctx: &mut Ctx, asset: &chainv4::structs::address::Address, evm: bool) -> String {
    if *asset == COIN {
        return ctx.store.status.as_ref().map_or("coin".to_string(), |status| status.coin.symbol.clone());
    }

    let text = asset.to_string();

    match ctx.store.tokens.get(&text).and_then(|token| token.symbol.clone()) {
        Some(symbol) => symbol,
        None => {
            // A native token's registry entry comes with its contract's page.
            if !evm && !ctx.store.contracts.contains_key(&text) && !ctx.api.asking(&Ask::Contract(text.clone())) {
                ctx.api.ask(Ask::Contract(text.clone()));
            }

            format::short(&text)
        }
    }
}

fn decimals(ctx: &Ctx, asset: &chainv4::structs::address::Address, evm: bool) -> u8 {
    match asset {
        asset if *asset == COIN => ctx.store.status.as_ref().map_or(18, |status| status.coin.decimals),
        asset => ctx.store.tokens.get(&asset.to_string()).and_then(|token| token.decimals).unwrap_or(if evm { 18 } else { 0 }),
    }
}

fn shown(ctx: &mut Ctx, asset: &chainv4::structs::address::Address, amount: u256, evm: bool) -> String {
    let symbol = symbol(ctx, asset, evm);

    format::amount(&amount.to_string(), decimals(ctx, asset, evm), &symbol)
}

/// The price of a pool's first asset in its second.
fn price(ctx: &Ctx, pool: &DexPool, evm: bool) -> Option<f64> {
    pool.price(&pool.a, decimals(ctx, &pool.a, evm), decimals(ctx, &pool.b, evm))
}

fn pool_page(ui: &mut Ui, ctx: &mut Ctx, state: &mut DexState, pool: &DexPool, native: &Option<String>, trades: &[Activity]) {
    let look = ctx.look;
    let evm = state.evm;
    let (a, b) = (symbol(ctx, &pool.a, evm), symbol(ctx, &pool.b, evm));

    super::section(ui, look, &format!("{a} / {b}"), |ui| {
        let supply = match evm {
            false => called(ctx, state, &pool.share.to_string(), "total_supply", json!([])).and_then(|values| u256::from_str_radix(values.first()?, 10).ok()),
            true => match pool.share {
                chainv4::structs::address::Address::Evm(pair) => rpc(ctx, state, &pair, "totalSupply()", &[]).and_then(|output| number_at(&output, 0)),
                _ => None,
            },
        };
        let (held_a, held_b) = (shown(ctx, &pool.a, pool.reserve_a, evm), shown(ctx, &pool.b, pool.reserve_b, evm));
        let shown_price = price(ctx, pool, evm);
        let share = pool.share.to_string();

        super::facts(ui, look, "dex pool", |facts| {
            facts.row("holds", |ui| {
                ui.label(format!("{held_a} and {held_b}"));
            });
            facts.row("price", |ui| {
                ui.monospace(shown_price.map_or("—".to_string(), |price| format!("1 {a} = {price:.6} {b}")));
            });
            facts.row("shares", |ui| {
                ui.monospace(supply.map_or("—".to_string(), |supply| format::units(&supply.to_string(), 18)));
            });
            facts.row("share token", |ui| {
                ui.monospace(format::short(&share));
            });
        });

        // Its price over time: from Sync events, what it held after every change.
        let points = prices(ctx, pool, native, evm);

        if points.len() >= 2 {
            ui.add_space(8.0);
            let series = Series {
                name: format!("{b} per {a}"),
                points,
                kind: Kind::Line,
                color: look.roles.main,
                limit: None,
                unit: |value| format!("{value:.6}"),
                across: chart::block,
            };
            chart::chart(ui, look, &series, 140.0);
        }

        ui.add_space(8.0);
        let mine: Vec<&Activity> = trades.iter().filter(|activity| in_pool(pool, activity).is_some()).take(25).collect();

        if mine.is_empty() {
            super::note(ui, look, "No trades, adds or removals among the latest events.");
            return;
        }

        let side = |ctx: &mut Ctx, first: u256, second: u256| match (first == u256::ZERO, second == u256::ZERO) {
            (true, true) => "—".to_string(),
            (false, true) => shown(ctx, &pool.a, first, evm),
            (true, false) => shown(ctx, &pool.b, second, evm),
            (false, false) => format!("{} and {}", shown(ctx, &pool.a, first, evm), shown(ctx, &pool.b, second, evm)),
        };

        if look.narrow {
            for activity in mine {
                let [in_a, in_b, out_a, out_b] = in_pool(pool, activity).expect("filtered");
                let (put, took) = (side(ctx, in_a, in_b), side(ctx, out_a, out_b));
                super::list_row(ui, look, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(widgets::strong(activity.kind, 13.5).color(look.palette.ink));
                        super::tx(ui, ctx, &activity.tx);
                    });
                    ui.label(format!("in {put} · out {took}"));
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("block").small().color(look.palette.muted));
                        super::block_link(ui, ctx, &activity.block, activity.height);
                    });
                });
            }

            return;
        }

        super::table(ui, look, ("dex activity", share.clone()), &["what", "in", "out", "block", "transaction"], |ui| {
            for activity in mine {
                let [in_a, in_b, out_a, out_b] = in_pool(pool, activity).expect("filtered");
                ui.label(widgets::strong(activity.kind, 13.0));
                ui.label(side(ctx, in_a, in_b));
                ui.label(side(ctx, out_a, out_b));
                super::block_link(ui, ctx, &activity.block, activity.height);
                super::tx(ui, ctx, &activity.tx);
                super::row_end(ui);
            }
        });
    });
}

/// A pool's price at each change, by block height, oldest first: the native DEX's Sync events, or
/// an EVM pair's Sync logs.
fn prices(ctx: &mut Ctx, pool: &DexPool, native: &Option<String>, evm: bool) -> Vec<[f64; 2]> {
    let (decimals_a, decimals_b) = (decimals(ctx, &pool.a, evm), decimals(ctx, &pool.b, evm));
    let mut points: Vec<[f64; 2]> = match (evm, native) {
        (false, Some(dex)) => events_of(ctx, EventQuery::contract(dex, Some("Sync")).limited(EVENTS))
            .iter()
            .filter_map(|event| {
                let (a, b) = (asset(event, 0)?, asset(event, 1)?);
                // The tier last, since tiers; before, every pool of a pair was the one.
                let fee_bps = event.data.get(4).map_or(Some(pool.fee_bps), |fee| fee.text().parse().ok())?;
                let synced = DexPool { a, b, fee_bps, reserve_a: amount(event, 2), reserve_b: amount(event, 3), share: pool.share, venue: pool.venue };

                (fee_bps == pool.fee_bps && synced.pairs(&pool.a, &pool.b)).then(|| synced.price(&pool.a, decimals_a, decimals_b)).flatten().map(|price| [event.height as f64, price])
            })
            .collect(),
        (true, _) => {
            let sync = to_hex(&keccak(b"Sync(uint112,uint112)"));

            events_of(ctx, pair_logs(pool))
                .iter()
                .filter(|event| event.data.first().is_some_and(|topic| topic.text() == sync))
                .filter_map(|event| {
                    let data = from_hex(&event.data.last()?.text())?;
                    let synced = DexPool { reserve_a: number_at(&data, 0)?, reserve_b: number_at(&data, 1)?, ..pool.clone() };

                    synced.price(&pool.a, decimals_a, decimals_b).map(|price| [event.height as f64, price])
                })
                .collect()
        }
        _ => Vec::new(),
    };
    points.reverse();

    points
}

#[cfg(test)]
mod tests;
