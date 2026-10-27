//! Trading on the chain's DEXes: contracts/lua/dex.lua, whose pools hold the coin, token-standard
//! tokens and ERC-20s, and the EVM's DexRouter, whose pairs hold ERC-20s and the coin as WDEV.
//! Both keep a constant product, so chainv4::dex quotes them alike from what their pools hold:
//! the best route, through one pool or two, its price impact, and the least it takes at the
//! slippage allowed. Pools are listed with what this account holds of each, to add to or take
//! out of, a new one opens with its first deposit, and a native pool's price is drawn from its
//! Sync events. Every transaction is a call the Send page reviews and signs.

use {
    super::send::{Prepared, SendState},
    super::{Ctx, Page},
    crate::format,
    crate::node::Ask,
    crate::theme,
    chainv4::abi::{AbiType, AbiValue, encode_values, evm_address},
    chainv4::crypto::hasher::Hasher,
    chainv4::crypto::secp256k1::keccak,
    chainv4::dex::{COIN, DexPool, best_route, least_out, shares_worth},
    chainv4::structs::address::Address,
    chainv4::structs::amount::Amount,
    chainv4::structs::hash::HashType,
    chainv4::structs::hex::{from_hex, to_hex},
    chainv4::structs::tx::{Tx, TxCallArgumentType},
    chainv4::structs::u256::u256,
    egui::{RichText, Ui},
    serde_json::{Value, json},
    std::collections::HashMap,
};

/// How often the pools are asked again.
const ASK_EVERY: f64 = 15.0;
/// The most pools listed.
const MAX_POOLS: u64 = 40;
/// Where the Trade page's JSON-RPC ids start, apart from the dApp page's.
const RPC_BASE: u64 = 1 << 40;
/// How long a trade or a deposit may wait to land.
const DEADLINE_MS: u64 = 20 * 60_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Venue {
    Native,
    Evm,
}

/// What an EVM read asked, by its JSON-RPC id.
#[derive(Clone, Debug)]
enum Read {
    Factory,
    Wdev,
    Count,
    Pair(u64),
    Token0([u8; 20]),
    Token1([u8; 20]),
    Reserves([u8; 20]),
    Held([u8; 20]),
    Supply([u8; 20]),
}

/// The EVM DEX, as its router and factory say.
#[derive(Default)]
struct EvmDex {
    factory: Option<[u8; 20]>,
    wdev: Option<[u8; 20]>,
    pairs: Vec<[u8; 20]>,
    tokens: HashMap<[u8; 20], ([u8; 20], [u8; 20])>,
    reserves: HashMap<[u8; 20], (u256, u256)>,
}

pub struct TradeState {
    venue: Venue,
    /// The native DEX's pools, by the order they opened in.
    pools: Vec<Option<DexPool>>,
    /// How many pools to ask for, once pool_count answered.
    wanted: Option<u64>,
    asked: f64,
    evm: EvmDex,
    reads: HashMap<u64, Read>,
    next_id: u64,
    evm_asked: f64,
    /// What this account holds of each share token, and each one's supply, by its address text.
    held: HashMap<String, u256>,
    supply: HashMap<String, u256>,
    from: Address,
    to: Option<Address>,
    amount: String,
    slippage: String,
    /// The pool picked to add to or take out of, by its share token.
    chosen: Option<Address>,
    add: String,
    remove: String,
    new_a: String,
    new_b: String,
    new_amount_a: String,
    new_amount_b: String,
    /// The chosen native pool's prices, by height, oldest first.
    chart: Vec<(u64, f64)>,
    syncs: Vec<Value>,
    charted: f64,
    trouble: Option<String>,
}

impl Default for TradeState {
    fn default() -> TradeState {
        TradeState {
            venue: Venue::Native,
            pools: Vec::new(),
            wanted: None,
            asked: f64::NEG_INFINITY,
            evm: EvmDex::default(),
            reads: HashMap::new(),
            next_id: RPC_BASE,
            evm_asked: f64::NEG_INFINITY,
            held: HashMap::new(),
            supply: HashMap::new(),
            from: COIN,
            to: None,
            amount: String::new(),
            slippage: "0.5".to_string(),
            chosen: None,
            add: String::new(),
            remove: String::new(),
            new_a: String::new(),
            new_b: String::new(),
            new_amount_a: String::new(),
            new_amount_b: String::new(),
            chart: Vec::new(),
            syncs: Vec::new(),
            charted: f64::NEG_INFINITY,
            trouble: None,
        }
    }
}

/// The name the DEX is found by on this chain, such as dex.dev.
pub fn dex_name(chain: &crate::chain::Chain) -> Option<String> {
    chain.names.as_ref().map(|names| format!("{}.{}", chainv4::dex::DEX_NAME, names.suffix))
}

/// The native DEX: the one its name resolves to, proven, or else the one the node names.
fn dex(ctx: &Ctx) -> Option<Address> {
    let named = dex_name(ctx.chain).and_then(|name| ctx.chain.resolve(&name)).and_then(|resolved| resolved.target());

    named.or_else(|| ctx.dex.0.and_then(|text| text.parse().ok()))
}

/// The EVM DEX's router, as the node names it.
fn router(ctx: &Ctx) -> Option<[u8; 20]> {
    ctx.dex.1.and_then(from_hex).and_then(|bytes| bytes.try_into().ok())
}

fn values(result: &Result<Value, crate::node::Failure>) -> Option<Vec<String>> {
    let value = result.as_ref().ok().filter(|value| value["status"] == "returned")?;
    let text = |value: &Value| value["value"].as_str().map_or_else(|| value["value"].to_string(), str::to_string);

    Some(value["values"].as_array()?.iter().map(text).collect())
}

fn word(output: &[u8], index: usize) -> Option<&[u8]> {
    output.get(index * 32..(index + 1) * 32)
}

fn address_word(output: &[u8], index: usize) -> Option<[u8; 20]> {
    word(output, index)?[12..].try_into().ok()
}

fn number_word(output: &[u8], index: usize) -> Option<u256> {
    Some(u256::from_be_bytes(word(output, index)?.try_into().ok()?))
}

impl TradeState {
    /// Takes the native DEX's answers, and the share tokens'; false for anyone else's.
    pub fn answered(&mut self, ask: &Ask, result: &Result<Value, crate::node::Failure>) -> bool {
        let Ask::Call(body) = ask else {
            return false;
        };
        let Ok(body) = serde_json::from_str::<Value>(body) else {
            return false;
        };

        if body["purpose"] != "trade" {
            return false;
        }

        let contract = body["contract"].as_str().unwrap_or_default().to_string();

        match (body["function"].as_str(), values(result)) {
            (Some("pool_count"), Some(values)) => {
                let count = values.first().and_then(|count| count.parse::<u64>().ok()).unwrap_or(0).min(MAX_POOLS);
                self.wanted = Some(count);
                self.pools.resize(count as usize, None);
            }
            (Some("pool_at"), Some(values)) => {
                let index = body["args"][0]["value"].as_str().and_then(|index| index.parse::<usize>().ok());

                if let (Some(index), Ok(pool)) = (index, DexPool::from_values(&values))
                    && index < self.pools.len()
                {
                    self.pools[index] = Some(pool);
                }
            }
            (Some("balance_of"), Some(values)) => {
                if let Some(held) = values.first().and_then(|held| u256::from_str_radix(held, 10).ok()) {
                    self.held.insert(contract, held);
                }
            }
            (Some("total_supply"), Some(values)) => {
                if let Some(supply) = values.first().and_then(|supply| u256::from_str_radix(supply, 10).ok()) {
                    self.supply.insert(contract, supply);
                }
            }
            (_, None) => {
                if let Ok(value) = result
                    && let Some(error) = value["error"].as_str()
                {
                    self.trouble = Some(format!("The DEX answered: {error}"));
                }
            }
            _ => {}
        }

        true
    }

    /// Takes an EVM read's answer; false for one the Trade page did not ask.
    pub fn answered_rpc(&mut self, body: &str, result: &Result<Value, crate::node::Failure>) -> bool {
        let Some(id) = serde_json::from_str::<Value>(body).ok().and_then(|body| body["id"].as_u64()) else {
            return false;
        };
        let Some(read) = self.reads.remove(&id) else {
            return id >= RPC_BASE;
        };
        let Some(output) = result.as_ref().ok().and_then(|value| value["result"].as_str()).and_then(from_hex) else {
            return true;
        };
        let evm = &mut self.evm;

        match read {
            Read::Factory => evm.factory = address_word(&output, 0),
            Read::Wdev => evm.wdev = address_word(&output, 0),
            Read::Count => {
                let count = number_word(&output, 0).map_or(0, |count| u64::try_from(count).unwrap_or(0)).min(MAX_POOLS);
                evm.pairs.truncate(count as usize);
            }
            Read::Pair(index) => {
                if let Some(pair) = address_word(&output, 0)
                    && !evm.pairs.contains(&pair)
                    && index as usize >= evm.pairs.len()
                {
                    evm.pairs.push(pair);
                }
            }
            Read::Token0(pair) => {
                let entry = evm.tokens.entry(pair).or_insert(([0; 20], [0; 20]));
                entry.0 = address_word(&output, 0).unwrap_or_default();
            }
            Read::Token1(pair) => {
                let entry = evm.tokens.entry(pair).or_insert(([0; 20], [0; 20]));
                entry.1 = address_word(&output, 0).unwrap_or_default();
            }
            Read::Reserves(pair) => {
                if let (Some(zero), Some(one)) = (number_word(&output, 0), number_word(&output, 1)) {
                    evm.reserves.insert(pair, (zero, one));
                }
            }
            Read::Held(pair) => {
                if let Some(held) = number_word(&output, 0) {
                    self.held.insert(Address::Evm(pair).to_string(), held);
                }
            }
            Read::Supply(pair) => {
                if let Some(supply) = number_word(&output, 0) {
                    self.supply.insert(Address::Evm(pair).to_string(), supply);
                }
            }
        }

        true
    }

    /// Takes the native DEX's Sync events, for the chosen pool's chart.
    pub fn events(&mut self, result: &Result<Value, crate::node::Failure>) {
        if let Ok(value) = result
            && let Some(events) = value["events"].as_array()
        {
            self.syncs = events.clone();
        }
    }

    /// The pools of the venue shown: the EVM's with WDEV as the coin.
    fn listed(&self) -> Vec<DexPool> {
        match self.venue {
            Venue::Native => self.pools.iter().flatten().cloned().collect(),
            Venue::Evm => {
                let coin = |token: [u8; 20]| match Some(token) == self.evm.wdev {
                    true => COIN,
                    false => Address::Evm(token),
                };

                self.evm
                    .pairs
                    .iter()
                    .filter_map(|pair| {
                        let (zero, one) = self.evm.tokens.get(pair).filter(|(zero, one)| *zero != [0; 20] && *one != [0; 20])?;
                        let (held0, held1) = self.evm.reserves.get(pair)?;

                        Some(DexPool { a: coin(*zero), b: coin(*one), reserve_a: *held0, reserve_b: *held1, share: Address::Evm(*pair) })
                    })
                    .collect()
            }
        }
    }

    /// Asks an EVM contract a read, as the node's JSON-RPC eth_call.
    fn read(&mut self, ctx: &mut Ctx, to: [u8; 20], input: Vec<u8>, read: Read) {
        let id = self.next_id;
        self.next_id += 1;
        self.reads.insert(id, read);
        ctx.node.ask(Ask::Rpc(json!({ "jsonrpc": "2.0", "id": id, "method": "eth_call", "params": [{ "to": to_hex(&to), "data": to_hex(&input) }, "latest"] }).to_string()));
    }
}

/// A native contract's function read through POST /call, marked as the Trade page's.
fn call_ask(contract: &Address, function: &str, args: Value) -> Ask {
    Ask::Call(json!({ "contract": contract.to_string(), "function": function, "args": args, "purpose": "trade" }).to_string())
}

fn calldata(signature: &str, types: &[AbiType], values: &[AbiValue]) -> Vec<u8> {
    [keccak(signature.as_bytes())[..4].to_vec(), encode_values(types, values).expect("values of their types")].concat()
}

/// Asks the pools again, now and then: the native DEX's through its pool list, the EVM's through
/// its router's factory, with what this account holds of each.
fn refresh(ctx: &mut Ctx, state: &mut TradeState, time: f64) {
    let me = ctx.holder().and_then(|holder| holder.addresses().first().copied());

    if let Some(dex) = dex(ctx) {
        if time - state.asked > ASK_EVERY {
            state.asked = time;
            ctx.node.ask(call_ask(&dex, "pool_count", json!([])));

            for pool in state.pools.iter().flatten() {
                ctx.node.ask(call_ask(&pool.share, "total_supply", json!([])));

                if let Some(me) = me {
                    ctx.node.ask(call_ask(&pool.share, "balance_of", json!([{ "type": "address", "value": me.to_string() }])));
                }
            }
        }

        // The pools pool_count said there are, once it said.
        if let Some(count) = state.wanted.take() {
            for index in 0..count {
                ctx.node.ask(call_ask(&dex, "pool_at", json!([{ "type": "i64", "value": index.to_string() }])));
            }
        }
    }

    let Some(router) = router(ctx) else {
        return;
    };

    if time - state.evm_asked <= ASK_EVERY {
        return;
    }

    state.evm_asked = time;
    let none: &[AbiType] = &[];

    match (state.evm.factory, state.evm.wdev) {
        (Some(factory), Some(_)) => {
            state.read(ctx, factory, calldata("allPairsLength()", none, &[]), Read::Count);

            for index in 0..MAX_POOLS.min(state.evm.pairs.len() as u64 + 4) {
                state.read(ctx, factory, calldata("allPairs(uint256)", &[AbiType::Uint(256)], &[AbiValue::Uint(u256::from(index))]), Read::Pair(index));
            }

            for pair in state.evm.pairs.clone() {
                if !state.evm.tokens.contains_key(&pair) {
                    state.read(ctx, pair, calldata("token0()", none, &[]), Read::Token0(pair));
                    state.read(ctx, pair, calldata("token1()", none, &[]), Read::Token1(pair));
                }

                state.read(ctx, pair, calldata("getReserves()", none, &[]), Read::Reserves(pair));
                state.read(ctx, pair, calldata("totalSupply()", none, &[]), Read::Supply(pair));

                if let Some(me) = me {
                    state.read(ctx, pair, calldata("balanceOf(address)", &[AbiType::Address], &[AbiValue::Address(evm_address(&me))]), Read::Held(pair));
                }
            }
        }
        _ => {
            state.read(ctx, router, calldata("factory()", none, &[]), Read::Factory);
            state.read(ctx, router, calldata("wdev()", none, &[]), Read::Wdev);
            // Soon again, for the pairs.
            state.evm_asked = time - ASK_EVERY + 1.0;
        }
    }
}

/// An asset's symbol: the coin's, a registered token's, or its address, short.
fn symbol(ctx: &Ctx, asset: &Address) -> String {
    match asset {
        asset if *asset == COIN => ctx.coin.symbol.clone(),
        asset => ctx
            .chain
            .tokens
            .get(&asset.to_string())
            .and_then(|token| token.value.symbol.clone())
            .unwrap_or_else(|| format::short(&asset.to_string())),
    }
}

/// An asset's decimals: the coin's, a registered token's, or 18, as ERC-20s mostly have.
fn decimals(ctx: &Ctx, asset: &Address) -> u8 {
    match asset {
        asset if *asset == COIN => ctx.coin.decimals,
        asset => ctx.chain.tokens.get(&asset.to_string()).and_then(|token| token.value.decimals).unwrap_or(18),
    }
}

fn shown(ctx: &Ctx, asset: &Address, amount: u256) -> String {
    format::amount(&amount.to_string(), decimals(ctx, asset), &symbol(ctx, asset))
}

fn parse_amount(ctx: &Ctx, asset: &Address, text: &str) -> Option<u256> {
    format::parse_units(text, decimals(ctx, asset)).and_then(|digits| u256::from_str_radix(&digits, 10).ok()).filter(|amount| *amount != u256::ZERO)
}

/// Slippage, as typed in percent, in parts in 10,000.
fn slippage(state: &TradeState) -> u32 {
    state.slippage.trim().parse::<f64>().ok().filter(|percent| (0.0..=50.0).contains(percent)).map_or(50, |percent| (percent * 100.0).round() as u32)
}

pub fn show(ui: &mut Ui, ctx: &mut Ctx, state: &mut TradeState, send: &mut SendState) {
    let palette = *ctx.palette;
    theme::title(ui, &palette, "Trade", "Tokens for tokens at the pools' price, and pools to add to: the DEX keeps a constant product, and 0.3% of each trade for who provided.");

    let time = ui.input(|input| input.time);
    refresh(ctx, state, time);
    let (native, evm) = (dex(ctx), router(ctx));

    if native.is_none() && evm.is_none() {
        theme::card(ui, &palette, |ui| {
            ui.colored_label(palette.warn, "No DEX found: the name dex does not resolve here, and the node names none.");
        });

        return;
    }

    if native.is_some() && evm.is_some() {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut state.venue, Venue::Native, "Native pools");
            ui.selectable_value(&mut state.venue, Venue::Evm, "EVM pools");
        });
        ui.add_space(8.0);
    } else {
        state.venue = if native.is_some() { Venue::Native } else { Venue::Evm };
    }

    let mut prepared = None;
    let pools = state.listed();
    trade(ui, ctx, state, &pools, &mut prepared);
    ui.add_space(14.0);
    liquidity(ui, ctx, state, &pools, &mut prepared);
    ui.add_space(14.0);
    create(ui, ctx, state, &mut prepared);

    if let Some(trouble) = &state.trouble {
        ui.add_space(8.0);
        ui.colored_label(palette.bad, trouble);
    }

    if let Some(prepared) = prepared {
        send.prepare(prepared);
        *ctx.page = Page::Send;
    }
}

/// The assets the pools hold, the coin first.
fn assets(pools: &[DexPool]) -> Vec<Address> {
    let mut assets = vec![COIN];

    for pool in pools {
        for asset in [pool.a, pool.b] {
            if !assets.contains(&asset) {
                assets.push(asset);
            }
        }
    }

    assets
}

fn pick(ui: &mut Ui, ctx: &Ctx, salt: &str, chosen: &mut Address, assets: &[Address]) {
    egui::ComboBox::from_id_salt(("trade", salt)).selected_text(symbol(ctx, chosen)).show_ui(ui, |ui| {
        for asset in assets {
            ui.selectable_value(chosen, *asset, symbol(ctx, asset));
        }
    });
}

fn trade(ui: &mut Ui, ctx: &Ctx, state: &mut TradeState, pools: &[DexPool], prepared: &mut Option<Prepared>) {
    let palette = *ctx.palette;
    let assets = assets(pools);

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Trade");

        if pools.is_empty() {
            ui.label(RichText::new("No pools yet: open one below with its first deposit.").color(palette.soft));
            return;
        }

        let mut to = state.to.filter(|to| assets.contains(to)).unwrap_or_else(|| *assets.iter().find(|asset| **asset != state.from).unwrap_or(&COIN));
        super::row(ui, &palette, "You pay", |ui| {
            pick(ui, ctx, "from", &mut state.from, &assets);
            ui.add(egui::TextEdit::singleline(&mut state.amount).hint_text("0.0").desired_width(140.0));
        });
        super::row(ui, &palette, "You get", |ui| pick(ui, ctx, "to", &mut to, &assets));
        state.to = Some(to);
        super::row(ui, &palette, "Slippage", |ui| {
            ui.add(egui::TextEdit::singleline(&mut state.slippage).desired_width(50.0));
            ui.label("%");
        });

        let Some(amount) = parse_amount(ctx, &state.from, &state.amount) else {
            return;
        };

        if state.from == to {
            ui.colored_label(palette.warn, "Pick two different assets.");
            return;
        }

        let Some(route) = best_route(pools, &state.from, &to, amount) else {
            ui.colored_label(palette.warn, "No pool, nor two, trades these.");
            return;
        };
        let least = least_out(route.out, slippage(state));
        ui.add_space(6.0);
        super::row(ui, &palette, "Quote", |ui| {
            ui.label(RichText::new(shown(ctx, &to, route.out)).strong().color(palette.ink));
        });
        super::row(ui, &palette, "Route", |ui| {
            ui.label(match route.middle {
                None => format!("{} → {}", symbol(ctx, &state.from), symbol(ctx, &to)),
                Some(middle) => format!("{} → {} → {}", symbol(ctx, &state.from), symbol(ctx, &middle), symbol(ctx, &to)),
            });
        });
        super::row(ui, &palette, "Price impact", |ui| {
            let percent = f64::from(route.impact) / 100.0;
            ui.colored_label(if route.impact > 500 { palette.bad } else if route.impact > 100 { palette.warn } else { palette.soft }, format!("{percent:.2}%"));
        });
        super::row(ui, &palette, "At least", |ui| {
            ui.label(shown(ctx, &to, least));
        });

        let Some(me) = ctx.holder().and_then(|holder| holder.addresses().first().copied()) else {
            return;
        };
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if state.from != COIN && theme::quiet(ui, &palette, &format!("1. Let the DEX move {}", symbol(ctx, &state.from))).clicked() {
                *prepared = approve(ctx, state.venue, me, &state.from, amount);
            }

            if theme::primary(ui, &palette, if state.from == COIN { "Trade" } else { "2. Trade" }, true).clicked() {
                *prepared = swap(ctx, state, me, amount, to, route.middle, least);
            }
        });
    });
}

/// Approving the DEX to move an asset: a token by its approve, an ERC-20 at the DEX's EVM
/// address, or the router's.
fn approve(ctx: &Ctx, venue: Venue, me: Address, asset: &Address, amount: u256) -> Option<Prepared> {
    let what = format!("Let the DEX move {}", symbol(ctx, asset));

    match (venue, asset) {
        (Venue::Native, Address::Evm(token)) => Some(evm_call(me, *token, approve_input(evm_address(&dex(ctx)?), amount), u256::ZERO, what)),
        (Venue::Native, token) => Some(native_call(me, *token, "approve", vec![TxCallArgumentType::Address(dex(ctx)?), TxCallArgumentType::U256(amount)], u256::ZERO, what)),
        (Venue::Evm, Address::Evm(token)) => Some(evm_call(me, *token, approve_input(router(ctx)?, amount), u256::ZERO, what)),
        (Venue::Evm, _) => None,
    }
}

fn approve_input(spender: [u8; 20], amount: u256) -> Vec<u8> {
    calldata("approve(address,uint256)", &[AbiType::Address, AbiType::Uint(256)], &[AbiValue::Address(spender), AbiValue::Uint(amount)])
}

/// The trade itself, on the venue shown.
fn swap(ctx: &Ctx, state: &TradeState, me: Address, amount: u256, to: Address, middle: Option<Address>, least: u256) -> Option<Prepared> {
    let what = format!("Trade {} for {}", symbol(ctx, &state.from), symbol(ctx, &to));

    match state.venue {
        Venue::Native => {
            let mut args = vec![
                TxCallArgumentType::Address(state.from),
                TxCallArgumentType::Address(to),
                TxCallArgumentType::U256(amount),
                TxCallArgumentType::U256(least),
                TxCallArgumentType::I64((ctx.now + DEADLINE_MS) as i64),
            ];
            args.extend(middle.map(TxCallArgumentType::Address));
            let coin = if state.from == COIN { amount } else { u256::ZERO };

            Some(native_call(me, dex(ctx)?, "swap", args, coin, what))
        }
        Venue::Evm => {
            let wdev = state.evm.wdev?;
            let token = |asset: &Address| match asset {
                asset if *asset == COIN => Some(wdev),
                Address::Evm(token) => Some(*token),
                _ => None,
            };
            let path = AbiValue::Array([Some(state.from), middle, Some(to)].into_iter().flatten().map(|asset| token(&asset).map(AbiValue::Address)).collect::<Option<Vec<_>>>()?);
            let (recipient, deadline) = (AbiValue::Address(evm_address(&me)), AbiValue::Uint(u256::from((ctx.now + DEADLINE_MS) / 1_000)));
            let addresses = AbiType::Array(Box::new(AbiType::Address));
            let (input, value) = match (state.from == COIN, to == COIN) {
                (true, _) => (calldata("swapExactDEVForTokens(uint256,address[],address,uint256)", &[AbiType::Uint(256), addresses, AbiType::Address, AbiType::Uint(256)], &[AbiValue::Uint(least), path, recipient, deadline]), amount),
                (_, true) => (calldata("swapExactTokensForDEV(uint256,uint256,address[],address,uint256)", &[AbiType::Uint(256), AbiType::Uint(256), addresses, AbiType::Address, AbiType::Uint(256)], &[AbiValue::Uint(amount), AbiValue::Uint(least), path, recipient, deadline]), u256::ZERO),
                _ => (calldata("swapExactTokensForTokens(uint256,uint256,address[],address,uint256)", &[AbiType::Uint(256), AbiType::Uint(256), addresses, AbiType::Address, AbiType::Uint(256)], &[AbiValue::Uint(amount), AbiValue::Uint(least), path, recipient, deadline]), u256::ZERO),
            };

            Some(evm_call(me, router(ctx)?, input, value, what))
        }
    }
}

fn liquidity(ui: &mut Ui, ctx: &mut Ctx, state: &mut TradeState, pools: &[DexPool], prepared: &mut Option<Prepared>) {
    let palette = *ctx.palette;

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Pools");

        if pools.is_empty() {
            ui.label(RichText::new("None yet.").color(palette.soft));
            return;
        }

        for pool in pools {
            let held = state.held.get(&pool.share.to_string()).copied().unwrap_or(u256::ZERO);
            let line = format!(
                "{} / {} · {} and {}{}",
                symbol(ctx, &pool.a),
                symbol(ctx, &pool.b),
                shown(ctx, &pool.a, pool.reserve_a),
                shown(ctx, &pool.b, pool.reserve_b),
                if held > u256::ZERO { " · yours: some" } else { "" }
            );

            if ui.selectable_label(state.chosen == Some(pool.share), RichText::new(line).color(palette.ink)).clicked() {
                state.chosen = Some(pool.share);
                (state.add, state.remove, state.chart) = (String::new(), String::new(), Vec::new());
                state.charted = f64::NEG_INFINITY;
            }
        }

        let Some(pool) = state.chosen.and_then(|share| pools.iter().find(|pool| pool.share == share)).cloned() else {
            return;
        };
        let Some(me) = ctx.holder().and_then(|holder| holder.addresses().first().copied()) else {
            return;
        };
        ui.add_space(10.0);
        theme::caps(ui, &palette, &format!("{} / {}", symbol(ctx, &pool.a), symbol(ctx, &pool.b)));

        if let Some(price) = pool.price(&pool.a, decimals(ctx, &pool.a), decimals(ctx, &pool.b)) {
            super::row(ui, &palette, "Price", |ui| {
                ui.label(format!("1 {} = {price:.6} {}", symbol(ctx, &pool.a), symbol(ctx, &pool.b)));
            });
        }

        if state.venue == Venue::Native {
            chart(ui, ctx, state, &pool);
        }

        let (held, supply) = (state.held.get(&pool.share.to_string()).copied().unwrap_or(u256::ZERO), state.supply.get(&pool.share.to_string()).copied());

        if let Some((a, b)) = supply.and_then(|supply| shares_worth(held, supply, pool.reserve_a, pool.reserve_b)) {
            super::row(ui, &palette, "Yours", |ui| {
                ui.label(format!("{} shares: {} and {}", format::units(&held.to_string(), 18), shown(ctx, &pool.a, a), shown(ctx, &pool.b, b)));
            });
        }

        // Adding, at the pool's ratio: the other side follows from the first.
        ui.add_space(6.0);
        super::row(ui, &palette, &format!("Add {}", symbol(ctx, &pool.a)), |ui| {
            ui.add(egui::TextEdit::singleline(&mut state.add).hint_text("0.0").desired_width(140.0));
        });

        if let Some(amount_a) = parse_amount(ctx, &pool.a, &state.add)
            && let Some(amount_b) = pool.matching(&pool.a, amount_a)
        {
            super::row(ui, &palette, "With", |ui| {
                ui.label(shown(ctx, &pool.b, amount_b));
            });
            ui.horizontal(|ui| {
                for (asset, amount) in [(pool.a, amount_a), (pool.b, amount_b)] {
                    if asset != COIN && theme::quiet(ui, &palette, &format!("Let it move {}", symbol(ctx, &asset))).clicked() {
                        *prepared = approve(ctx, state.venue, me, &asset, amount);
                    }
                }

                if theme::primary(ui, &palette, "Add", true).clicked() {
                    *prepared = add(ctx, state, me, &pool.a, &pool.b, amount_a, amount_b);
                }
            });
        }

        // Taking shares out.
        if held > u256::ZERO {
            ui.add_space(6.0);
            super::row(ui, &palette, "Take out", |ui| {
                ui.add(egui::TextEdit::singleline(&mut state.remove).hint_text("shares").desired_width(140.0));

                if ui.small_button("All").clicked() {
                    state.remove = format::units(&held.to_string(), 18);
                }
            });

            if let Some(shares) = format::parse_units(&state.remove, 18).and_then(|digits| u256::from_str_radix(&digits, 10).ok()).filter(|shares| *shares != u256::ZERO && *shares <= held)
                && let Some((a, b)) = supply.and_then(|supply| shares_worth(shares, supply, pool.reserve_a, pool.reserve_b))
            {
                let slip = slippage(state);
                super::row(ui, &palette, "For", |ui| {
                    ui.label(format!("{} and {}", shown(ctx, &pool.a, a), shown(ctx, &pool.b, b)));
                });
                ui.horizontal(|ui| {
                    if state.venue == Venue::Evm && theme::quiet(ui, &palette, "1. Let the router take the shares").clicked() {
                        *prepared = router(ctx).and_then(|router| match pool.share {
                            Address::Evm(pair) => Some(evm_call(me, pair, approve_input(router, shares), u256::ZERO, "Let the router take the shares".to_string())),
                            _ => None,
                        });
                    }

                    if theme::primary(ui, &palette, "Take out", true).clicked() {
                        *prepared = remove(ctx, state, me, &pool, shares, least_out(a, slip), least_out(b, slip));
                    }
                });
            }
        }
    });
}

/// Adding to a pair, opening it if it is new: at the slippage allowed below what is offered.
fn add(ctx: &Ctx, state: &TradeState, me: Address, a: &Address, b: &Address, amount_a: u256, amount_b: u256) -> Option<Prepared> {
    let slip = slippage(state);
    let what = format!("Add to {} / {}", symbol(ctx, a), symbol(ctx, b));

    match state.venue {
        Venue::Native => {
            let coin = if *a == COIN { amount_a } else if *b == COIN { amount_b } else { u256::ZERO };
            let args = vec![
                TxCallArgumentType::Address(*a),
                TxCallArgumentType::Address(*b),
                TxCallArgumentType::U256(amount_a),
                TxCallArgumentType::U256(amount_b),
                TxCallArgumentType::U256(least_out(amount_a, slip)),
                TxCallArgumentType::U256(least_out(amount_b, slip)),
                TxCallArgumentType::I64((ctx.now + DEADLINE_MS) as i64),
            ];

            Some(native_call(me, dex(ctx)?, "add_liquidity", args, coin, what))
        }
        Venue::Evm => {
            let (recipient, deadline) = (AbiValue::Address(evm_address(&me)), AbiValue::Uint(u256::from((ctx.now + DEADLINE_MS) / 1_000)));
            let n = AbiValue::Uint;
            let (input, value) = match (a, b) {
                (coin, Address::Evm(token)) | (Address::Evm(token), coin) if *coin == COIN => {
                    let (amount_token, amount_coin) = if *a == COIN { (amount_b, amount_a) } else { (amount_a, amount_b) };
                    let types = [AbiType::Address, AbiType::Uint(256), AbiType::Uint(256), AbiType::Uint(256), AbiType::Address, AbiType::Uint(256)];
                    let values = [AbiValue::Address(*token), n(amount_token), n(least_out(amount_token, slip)), n(least_out(amount_coin, slip)), recipient, deadline];

                    (calldata("addLiquidityDEV(address,uint256,uint256,uint256,address,uint256)", &types, &values), amount_coin)
                }
                (Address::Evm(token_a), Address::Evm(token_b)) => {
                    let types = [AbiType::Address, AbiType::Address, AbiType::Uint(256), AbiType::Uint(256), AbiType::Uint(256), AbiType::Uint(256), AbiType::Address, AbiType::Uint(256)];
                    let values = [AbiValue::Address(*token_a), AbiValue::Address(*token_b), n(amount_a), n(amount_b), n(least_out(amount_a, slip)), n(least_out(amount_b, slip)), recipient, deadline];

                    (calldata("addLiquidity(address,address,uint256,uint256,uint256,uint256,address,uint256)", &types, &values), u256::ZERO)
                }
                _ => return None,
            };

            Some(evm_call(me, router(ctx)?, input, value, what))
        }
    }
}

fn remove(ctx: &Ctx, state: &TradeState, me: Address, pool: &DexPool, shares: u256, least_a: u256, least_b: u256) -> Option<Prepared> {
    let what = format!("Take out of {} / {}", symbol(ctx, &pool.a), symbol(ctx, &pool.b));

    match state.venue {
        Venue::Native => {
            let args = vec![
                TxCallArgumentType::Address(pool.a),
                TxCallArgumentType::Address(pool.b),
                TxCallArgumentType::U256(shares),
                TxCallArgumentType::U256(least_a),
                TxCallArgumentType::U256(least_b),
                TxCallArgumentType::I64((ctx.now + DEADLINE_MS) as i64),
            ];

            Some(native_call(me, dex(ctx)?, "remove_liquidity", args, u256::ZERO, what))
        }
        Venue::Evm => {
            let (recipient, deadline) = (AbiValue::Address(evm_address(&me)), AbiValue::Uint(u256::from((ctx.now + DEADLINE_MS) / 1_000)));
            let n = AbiValue::Uint;
            let input = match (pool.a, pool.b) {
                (coin, Address::Evm(token)) | (Address::Evm(token), coin) if coin == COIN => {
                    let (least_token, least_coin) = if pool.a == COIN { (least_b, least_a) } else { (least_a, least_b) };
                    let types = [AbiType::Address, AbiType::Uint(256), AbiType::Uint(256), AbiType::Uint(256), AbiType::Address, AbiType::Uint(256)];

                    calldata("removeLiquidityDEV(address,uint256,uint256,uint256,address,uint256)", &types, &[AbiValue::Address(token), n(shares), n(least_token), n(least_coin), recipient, deadline])
                }
                (Address::Evm(a), Address::Evm(b)) => {
                    let types = [AbiType::Address, AbiType::Address, AbiType::Uint(256), AbiType::Uint(256), AbiType::Uint(256), AbiType::Address, AbiType::Uint(256)];

                    calldata("removeLiquidity(address,address,uint256,uint256,uint256,address,uint256)", &types, &[AbiValue::Address(a), AbiValue::Address(b), n(shares), n(least_a), n(least_b), recipient, deadline])
                }
                _ => return None,
            };

            Some(evm_call(me, router(ctx)?, input, u256::ZERO, what))
        }
    }
}

/// Opening a pool: two assets, by address or the coin, and the first deposit, which sets its
/// price.
fn create(ui: &mut Ui, ctx: &Ctx, state: &mut TradeState, prepared: &mut Option<Prepared>) {
    let palette = *ctx.palette;

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Open a pool");
        ui.label(RichText::new(format!("Two assets, each a token's address or empty for {}, and the first deposit of each: their ratio is the price the pool opens at.", ctx.coin.symbol)).color(palette.soft));
        ui.add_space(6.0);

        for (label, asset, amount) in [("First", &mut state.new_a, &mut state.new_amount_a), ("Second", &mut state.new_b, &mut state.new_amount_b)] {
            super::row(ui, &palette, label, |ui| {
                ui.add(egui::TextEdit::singleline(asset).hint_text(if state.venue == Venue::Evm { "0x… or empty" } else { "contract:0x…, evm:0x… or empty" }).desired_width(260.0));
                ui.add(egui::TextEdit::singleline(amount).hint_text("0.0").desired_width(100.0));
            });
        }

        let asset = |text: &str| -> Option<Address> {
            match text.trim() {
                "" => Some(COIN),
                text if text.len() == 42 && text.starts_with("0x") => from_hex(text).and_then(|bytes| bytes.try_into().ok()).map(Address::Evm),
                text => text.parse().ok(),
            }
        };
        let (Some(a), Some(b)) = (asset(&state.new_a), asset(&state.new_b)) else {
            ui.colored_label(palette.warn, "An asset that is no address.");
            return;
        };
        let (Some(amount_a), Some(amount_b)) = (parse_amount(ctx, &a, &state.new_amount_a), parse_amount(ctx, &b, &state.new_amount_b)) else {
            return;
        };
        let Some(me) = ctx.holder().and_then(|holder| holder.addresses().first().copied()) else {
            return;
        };

        if a == b {
            ui.colored_label(palette.warn, "Two different assets.");
            return;
        }

        super::row(ui, &palette, "Opens at", |ui| {
            let price = amount_b.as_f64() / 10f64.powi(i32::from(decimals(ctx, &b))) / (amount_a.as_f64() / 10f64.powi(i32::from(decimals(ctx, &a))));
            ui.label(format!("1 {} = {price:.6} {}", symbol(ctx, &a), symbol(ctx, &b)));
        });
        ui.horizontal(|ui| {
            for (asset, amount) in [(a, amount_a), (b, amount_b)] {
                if asset != COIN && theme::quiet(ui, &palette, &format!("Let it move {}", symbol(ctx, &asset))).clicked() {
                    *prepared = approve(ctx, state.venue, me, &asset, amount);
                }
            }

            if theme::primary(ui, &palette, "Open it", true).clicked() {
                *prepared = add(ctx, state, me, &a, &b, amount_a, amount_b);
            }
        });
    });
}

/// The chosen native pool's price over time, from its Sync events: what it held of each after
/// every change.
fn chart(ui: &mut Ui, ctx: &mut Ctx, state: &mut TradeState, pool: &DexPool) {
    let palette = *ctx.palette;
    let time = ui.input(|input| input.time);

    if time - state.charted > ASK_EVERY
        && let Some(dex) = dex(ctx)
    {
        state.charted = time;
        ctx.node.ask(Ask::Named { contract: dex.to_string(), name: "Sync".to_string() });
    }

    state.chart = prices(&state.syncs, pool, decimals(ctx, &pool.a), decimals(ctx, &pool.b));

    if state.chart.len() < 2 {
        ui.label(RichText::new("Its price shows here once it has moved, on a node that keeps an event index.").small().color(palette.muted));
        return;
    }

    let (low, high) = state.chart.iter().fold((f64::MAX, f64::MIN), |(low, high), (_, price)| (low.min(*price), high.max(*price)));
    let (first, last) = (state.chart[0].0 as f64, state.chart[state.chart.len() - 1].0 as f64);
    let (response, painter) = ui.allocate_painter(egui::vec2(ui.available_width(), 90.0), egui::Sense::hover());
    let rect = response.rect;
    let span = (high - low).max(high.abs() * 1e-9).max(f64::MIN_POSITIVE);
    let points: Vec<egui::Pos2> = state
        .chart
        .iter()
        .map(|(height, price)| {
            let x = if last > first { (*height as f64 - first) / (last - first) } else { 1.0 };
            egui::pos2(rect.left() + x as f32 * rect.width(), rect.bottom() - ((price - low) / span) as f32 * rect.height())
        })
        .collect();
    painter.add(egui::Shape::line(points, egui::Stroke::new(1.5, palette.ink)));
    ui.label(RichText::new(format!("{low:.6} to {high:.6} {} per {}, over the last {} changes", symbol(ctx, &pool.b), symbol(ctx, &pool.a), state.chart.len())).small().color(palette.muted));
}

/// A pool's prices from Sync events, newest first as the node gives them, oldest first after:
/// pool.b's per pool.a, by block height.
fn prices(events: &[Value], pool: &DexPool, decimals_a: u8, decimals_b: u8) -> Vec<(u64, f64)> {
    let text = |value: &Value| value["value"].as_str().map_or_else(|| value["value"].to_string(), str::to_string);
    let mut points: Vec<(u64, f64)> = events
        .iter()
        .filter_map(|event| {
            let data: Vec<String> = event["data"].as_array()?.iter().map(text).collect();
            let [a, b, reserve_a, reserve_b] = data.as_slice() else {
                return None;
            };
            let (a, b): (Address, Address) = (a.parse().ok()?, b.parse().ok()?);
            let (reserve_a, reserve_b) = (u256::from_str_radix(reserve_a, 10).ok()?, u256::from_str_radix(reserve_b, 10).ok()?);
            let synced = DexPool { a, b, reserve_a, reserve_b, share: pool.share };

            if !synced.pairs(&pool.a, &pool.b) {
                return None;
            }

            Some((event["height"].as_u64()?, synced.price(&pool.a, decimals_a, decimals_b)?))
        })
        .collect();
    points.reverse();

    points
}

/// A call to a native contract, for the Send page.
fn native_call(from: Address, to: Address, function: &str, args: Vec<TxCallArgumentType>, coin: u256, what: String) -> Prepared {
    Prepared {
        from,
        tx: Tx::ContractCall { to, func: Hasher::digest(HashType::Xxh3_64, function.as_bytes()), amount: amount(coin), data: args },
        what,
        about: to.to_string(),
        coin,
    }
}

/// A call to an EVM contract, for the Send page.
fn evm_call(from: Address, to: [u8; 20], input: Vec<u8>, coin: u256, what: String) -> Prepared {
    Prepared {
        from,
        tx: Tx::EvmCall { to: Address::Evm(to), value: amount(coin), input },
        what,
        about: to_hex(&to),
        coin,
    }
}

fn amount(coin: u256) -> Amount {
    match u128::try_from(coin) {
        Ok(narrow) => Amount::U128(narrow),
        Err(_) => Amount::U256(coin),
    }
}

#[cfg(test)]
mod tests;
