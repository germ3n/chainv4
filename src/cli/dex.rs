//! `chainv4 dex`: the chain's DEXes from the command line. deploy sets them up, contracts/lua/dex.lua
//! with its share token template, or with --evm the EVM's WDEV, DexFactory and DexRouter, from
//! sources built into the binary. pools lists them, quote prices a trade through the best route,
//! and swap, add and remove send what the wallet's Trade page sends: to the native DEX the chain's
//! directory names, or else the node, or --dex; with --evm to the router found the same way, or
//! --router. Each approves a token only when what the DEX may move of it already is short.

use {
    super::send::{SendArgs, TlsArgs, TxArgs, agent, ok, send},
    super::{CliResult, deployed_at, parse_amount, signer},
    crate::abi::{AbiType, AbiValue, decode_values, encode_values, evm_address},
    crate::crypto::hasher::Hasher,
    crate::dex::{COIN, DexPool, add_amounts, best_route, least_out},
    crate::structs::address::Address,
    crate::structs::hash::HashType,
    crate::structs::hex::{from_hex, to_hex},
    crate::structs::tx::TxCallArgumentType,
    crate::structs::u256::u256,
    serde_json::{Value, json},
    std::path::PathBuf,
};

const DEX_LUA: &str = include_str!("../../contracts/lua/dex.lua");
const LP_TOKEN_LUA: &str = include_str!("../../contracts/lua/lp_token.lua");
const WDEV_BIN: &str = include_str!("../../contracts/evm/WDEV.bin");
const FACTORY_BIN: &str = include_str!("../../contracts/evm/DexFactory.bin");
const ROUTER_BIN: &str = include_str!("../../contracts/evm/DexRouter.bin");

#[derive(clap::Args)]
pub struct DexArgs {
    #[command(subcommand)]
    pub action: DexAction,
    /// The node's HTTP API: http://, or https:// behind TLS.
    #[arg(long, global = true, default_value = "http://127.0.0.1:8080")]
    pub node: String,
    #[command(flatten)]
    pub tls: TlsArgs,
    /// The key file to sign with: the development key when none is given.
    #[arg(long, global = true)]
    pub key: Option<PathBuf>,
    /// The EVM DEX, through its router, rather than the native one.
    #[arg(long, global = true)]
    pub evm: bool,
    /// The native DEX, dex.lua, to use: contract:0x…. Without it, the one the chain's directory
    /// names as dex, or else the node's --dex-contract, as GET /status says. Not used with --evm.
    #[arg(long, global = true)]
    pub dex: Option<Address>,
    /// The EVM DEX's router to use, with --evm: 0x and 40 hex digits. Without it, the one the
    /// chain's directory names as dex_router, or else the node's --dex-router.
    #[arg(long, global = true)]
    pub router: Option<String>,
}

#[derive(clap::Subcommand)]
pub enum DexAction {
    /// Deploys and sets up a DEX, printing where: the native one, dex.lua and its share token
    /// template; or with --evm, WDEV, the factory and the router.
    Deploy,
    /// Lists the pools: their assets, what they hold of each, and their share tokens.
    Pools,
    /// What a trade would pay now, through the best route, one pool or two.
    Quote {
        /// The asset paid: coin, or a token's address (contract:0x…, evm:0x… or 0x…).
        from: String,
        /// The asset got.
        to: String,
        /// How much, in the paid asset's smallest units.
        #[arg(value_parser = parse_amount)]
        amount: u256,
    },
    /// Trades through the best route, for at least the quote less the slippage, approving the DEX
    /// to move a token first when what it may move already is short.
    Swap {
        /// The asset paid: coin, or a token's address (contract:0x…, evm:0x… or 0x…).
        from: String,
        /// The asset got.
        to: String,
        /// How much is paid, in the paid asset's smallest units.
        #[arg(value_parser = parse_amount)]
        amount: u256,
        /// The slippage allowed, in percent, from 0 to 50: the trade fails rather than pay less
        /// than the quote by more.
        #[arg(long, default_value_t = 0.5)]
        slippage: f64,
    },
    /// Adds to a pool at most these amounts: the largest pair at the pool's ratio that fits both,
    /// worked out from what it holds and said before it is sent, as Uniswap V2's router does. A new
    /// pool opens at the amounts as given, its price. Approves a token first when what the DEX may
    /// move of it already is short.
    Add {
        /// One asset: coin, or a token's address.
        a: String,
        /// The other.
        b: String,
        /// The most of a to put in, in its smallest units.
        #[arg(value_parser = parse_amount)]
        amount_a: u256,
        /// The most of b to put in.
        #[arg(value_parser = parse_amount)]
        amount_b: u256,
        /// How far the pool's price may move before it lands, in percent, from 0 to 50: it takes
        /// at least the pair worked out, less this, or nothing.
        #[arg(long, default_value_t = 0.5)]
        slippage: f64,
    },
    /// Takes shares out of a pool, for its part of each asset. On the EVM, the router takes the
    /// shares, approved first when what it may move already is short.
    Remove {
        /// One asset: coin, or a token's address.
        a: String,
        /// The other.
        b: String,
        /// The shares, in their smallest units.
        #[arg(value_parser = parse_amount)]
        shares: u256,
    },
}

/// What a command reaches the node with.
struct Dex {
    agent: ureq::Agent,
    node: String,
    args: DexArgs,
}

pub fn dex(args: DexArgs) -> CliResult<()> {
    let dex = Dex { agent: agent(&args.tls)?, node: args.node.trim_end_matches('/').to_string(), args };
    let deadline = crate::node::now_ms() + 20 * 60_000;

    match &dex.args.action {
        DexAction::Deploy if dex.args.evm => {
            let me = signer(dex.args.key.as_deref())?.public().address();
            let wdev = dex.create(&me, WDEV_BIN, Vec::new())?;
            let factory = dex.create(&me, FACTORY_BIN, Vec::new())?;
            let router = dex.create(&me, ROUTER_BIN, encode_values(&[AbiType::Address, AbiType::Address], &[AbiValue::Address(evm(&factory)?), AbiValue::Address(evm(&wdev)?)])?)?;
            println!("WDEV {}", to_hex(&evm(&wdev)?));
            println!("factory {}", to_hex(&evm(&factory)?));
            println!("router {}", to_hex(&evm(&router)?));
        }
        DexAction::Deploy => {
            let me = signer(dex.args.key.as_deref())?.public().address();
            let template = dex.deploy_lua(&me, "lp_token.lua", LP_TOKEN_LUA)?;
            let contract = dex.deploy_lua(&me, "dex.lua", DEX_LUA)?;
            dex.call(contract, "setup", u256::ZERO, vec![TxCallArgumentType::Address(template)])?;
            println!("share token template {template}");
            println!("dex {contract}");
        }
        DexAction::Pools => {
            for pool in dex.pools()? {
                println!("{} / {}: {} and {}, shares {}", pool.a, pool.b, pool.reserve_a, pool.reserve_b, pool.share);
            }
        }
        DexAction::Quote { from, to, amount } => {
            let (from, to) = (asset(from)?, asset(to)?);
            let route = best_route(&dex.pools()?, &from, &to, *amount, dex.fee_bps()?).ok_or("no pool, nor two, trades these")?;
            println!("{} out, price impact {:.2}%", route.out, f64::from(route.impact) / 100.0);

            if let Some(middle) = route.middle {
                println!("through {middle}");
            }
        }
        DexAction::Swap { from, to, amount, slippage } => {
            let (from, to) = (asset(from)?, asset(to)?);
            let route = best_route(&dex.pools()?, &from, &to, *amount, dex.fee_bps()?).ok_or("no pool, nor two, trades these")?;
            let least = least_out(route.out, percent(*slippage)?);
            dex.approve(&from, *amount)?;

            match dex.args.evm {
                false => {
                    let mut args = vec![TxCallArgumentType::Address(from), TxCallArgumentType::Address(to), TxCallArgumentType::U256(*amount), TxCallArgumentType::U256(least), TxCallArgumentType::I64(deadline as i64)];
                    args.extend(route.middle.map(TxCallArgumentType::Address));
                    dex.call(dex.native()?, "swap", if from == COIN { *amount } else { u256::ZERO }, args)?;
                }
                true => {
                    let me = signer(dex.args.key.as_deref())?.public().address();
                    let path = [Some(from), route.middle, Some(to)].into_iter().flatten().map(|asset| dex.token(&asset)).collect::<CliResult<Vec<_>>>()?;
                    let path = format!("[{}]", path.iter().map(|token| format!("\"{}\"", to_hex(token))).collect::<Vec<_>>().join(","));
                    let (me, deadline) = (to_hex(&evm_address(&me)), (deadline / 1_000).to_string());
                    let (function, args, value) = match (from == COIN, to == COIN) {
                        (true, _) => ("swapExactDEVForTokens(uint256,address[],address,uint256)", vec![least.to_string(), path, me, deadline], *amount),
                        (_, true) => ("swapExactTokensForDEV(uint256,uint256,address[],address,uint256)", vec![amount.to_string(), least.to_string(), path, me, deadline], u256::ZERO),
                        _ => ("swapExactTokensForTokens(uint256,uint256,address[],address,uint256)", vec![amount.to_string(), least.to_string(), path, me, deadline], u256::ZERO),
                    };
                    dex.evm_call(dex.router()?, function, args, value)?;
                }
            }

            println!("traded for at least {least}, quoted {}", route.out);
        }
        DexAction::Add { a, b, amount_a, amount_b, slippage } => {
            let (a, b, slip) = (asset(a)?, asset(b)?, percent(*slippage)?);
            let held = dex.pools()?.into_iter().find(|pool| pool.pairs(&a, &b)).and_then(|pool| pool.facing(&a));
            let plan = plan_add(held, *amount_a, *amount_b, slip)?;
            println!("{}", plan.said(&a, &b, *amount_a, *amount_b, *slippage));
            dex.approve(&a, *amount_a)?;
            dex.approve(&b, *amount_b)?;

            // The amounts given go as the most, and the pair worked out from the reserves, less
            // the slippage, as the least: the contract takes that same pair at the same reserves,
            // and gives back what is sent of the coin beyond it.
            let (least_a, least_b) = (plan.least_a, plan.least_b);

            match dex.args.evm {
                false => {
                    let coin = if a == COIN { *amount_a } else if b == COIN { *amount_b } else { u256::ZERO };
                    let args = [a, b].map(TxCallArgumentType::Address).into_iter().chain([*amount_a, *amount_b, least_a, least_b].map(TxCallArgumentType::U256)).chain([TxCallArgumentType::I64(deadline as i64)]).collect();
                    dex.call(dex.native()?, "add_liquidity", coin, args)?;
                }
                true => {
                    let me = to_hex(&evm_address(&signer(dex.args.key.as_deref())?.public().address()));
                    let deadline = (deadline / 1_000).to_string();

                    match (a == COIN, b == COIN) {
                        (false, false) => {
                            let args = vec![to_hex(&dex.token(&a)?), to_hex(&dex.token(&b)?), amount_a.to_string(), amount_b.to_string(), least_a.to_string(), least_b.to_string(), me, deadline];
                            dex.evm_call(dex.router()?, "addLiquidity(address,address,uint256,uint256,uint256,uint256,address,uint256)", args, u256::ZERO)?;
                        }
                        _ => {
                            let (token, amount_token, amount_coin, least_token, least_coin) = if a == COIN { (b, *amount_b, *amount_a, least_b, least_a) } else { (a, *amount_a, *amount_b, least_a, least_b) };
                            let args = vec![to_hex(&dex.token(&token)?), amount_token.to_string(), least_token.to_string(), least_coin.to_string(), me, deadline];
                            dex.evm_call(dex.router()?, "addLiquidityDEV(address,uint256,uint256,uint256,address,uint256)", args, amount_coin)?;
                        }
                    }
                }
            }
        }
        DexAction::Remove { a, b, shares } => {
            let (a, b) = (asset(a)?, asset(b)?);

            match dex.args.evm {
                false => {
                    let args = vec![TxCallArgumentType::Address(a), TxCallArgumentType::Address(b), TxCallArgumentType::U256(*shares), TxCallArgumentType::U256(u256::ZERO), TxCallArgumentType::U256(u256::ZERO), TxCallArgumentType::I64(deadline as i64)];
                    dex.call(dex.native()?, "remove_liquidity", u256::ZERO, args)?;
                }
                true => {
                    let pool = dex.pools()?.into_iter().find(|pool| pool.pairs(&a, &b)).ok_or("no such pair")?;
                    let me = to_hex(&evm_address(&signer(dex.args.key.as_deref())?.public().address()));
                    let deadline = (deadline / 1_000).to_string();
                    dex.approve(&pool.share, *shares)?;

                    match (a == COIN, b == COIN) {
                        (false, false) => dex.evm_call(dex.router()?, "removeLiquidity(address,address,uint256,uint256,uint256,address,uint256)", vec![to_hex(&dex.token(&a)?), to_hex(&dex.token(&b)?), shares.to_string(), "0".into(), "0".into(), me, deadline], u256::ZERO)?,
                        _ => {
                            let token = if a == COIN { b } else { a };
                            dex.evm_call(dex.router()?, "removeLiquidityDEV(address,uint256,uint256,uint256,address,uint256)", vec![to_hex(&dex.token(&token)?), shares.to_string(), "0".into(), "0".into(), me, deadline], u256::ZERO)?;
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

/// An asset as the commands take it: coin, or a token's address, with 0x and 40 hex digits for
/// an ERC-20.
fn asset(text: &str) -> CliResult<Address> {
    match text.trim() {
        "coin" | "dev" | "DEV" => Ok(COIN),
        text => Ok(super::evm::evm_address(text)?),
    }
}

fn percent(slippage: f64) -> CliResult<u32> {
    match (0.0..=50.0).contains(&slippage) {
        true => Ok((slippage * 100.0).round() as u32),
        false => Err("--slippage is a percent from 0 to 50".into()),
    }
}

/// What an add puts in: the pair at the pool's ratio within the amounts given, and the least of
/// each it takes should the price move before it lands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AddPlan {
    pub(crate) amount_a: u256,
    pub(crate) amount_b: u256,
    pub(crate) least_a: u256,
    pub(crate) least_b: u256,
    /// Whether the pool holds nothing yet, so this opens it at its price.
    pub(crate) opens: bool,
}

/// An add of at most amount_a and amount_b to a pool holding what held says, in a's order then
/// b's, or nothing for no pool: as Uniswap V2's router does it, the largest pair at the pool's ratio
/// that fits both, by the contracts' own rounding, at least that less the slippage in parts in
/// 10,000. A new pool takes the amounts as given.
pub(crate) fn plan_add(held: Option<(u256, u256)>, amount_a: u256, amount_b: u256, slippage: u32) -> CliResult<AddPlan> {
    let (reserve_a, reserve_b) = held.unwrap_or((u256::ZERO, u256::ZERO));
    let (put_a, put_b) = add_amounts(reserve_a, reserve_b, amount_a, amount_b).ok_or("the pool holds one of its assets alone, or the amounts are too large to reckon: nothing is added")?;
    let opens = reserve_a == u256::ZERO && reserve_b == u256::ZERO;

    if put_a == u256::ZERO || put_b == u256::ZERO {
        return Err(format!("at the pool's ratio, {reserve_a} to {reserve_b}, those amounts put in {put_a} and {put_b}: add more of each").into());
    }

    Ok(match opens {
        true => AddPlan { amount_a: put_a, amount_b: put_b, least_a: put_a, least_b: put_b, opens },
        false => AddPlan { amount_a: put_a, amount_b: put_b, least_a: least_out(put_a, slippage), least_b: least_out(put_b, slippage), opens },
    })
}

impl AddPlan {
    /// What it says before it sends.
    pub(crate) fn said(&self, a: &Address, b: &Address, given_a: u256, given_b: u256, slippage: f64) -> String {
        let name = |asset: &Address| if *asset == COIN { "coin".to_string() } else { asset.to_string() };

        match self.opens {
            true => format!("opening the pool of {} and {} at {} and {}: its price", name(a), name(b), self.amount_a, self.amount_b),
            false => {
                let kept = match (self.amount_a < given_a, self.amount_b < given_b) {
                    (true, _) => format!(", {} of the {} given", self.amount_a, given_a),
                    (_, true) => format!(", {} of the {} given", self.amount_b, given_b),
                    _ => String::new(),
                };

                format!(
                    "adding {} {} and {} {} at the pool's ratio{kept}; at least {} and {} should its price move ({slippage}%)",
                    self.amount_a,
                    name(a),
                    self.amount_b,
                    name(b),
                    self.least_a,
                    self.least_b
                )
            }
        }
    }
}

fn evm(address: &Address) -> CliResult<[u8; 20]> {
    match address {
        Address::Evm(bytes) => Ok(*bytes),
        other => Err(format!("{other} is not an EVM contract").into()),
    }
}

fn calldata(signature: &str, types: &[AbiType], values: &[AbiValue]) -> CliResult<Vec<u8>> {
    Ok([Hasher::digest(HashType::Keccak256, signature.as_bytes()).to_bytes()[..4].to_vec(), encode_values(types, values)?].concat())
}

impl Dex {
    fn send(&self, tx: TxArgs) -> CliResult<()> {
        send(SendArgs {
            node: self.node.clone(),
            tls: TlsArgs { ca_cert: self.args.tls.ca_cert.clone(), insecure: self.args.tls.insecure },
            key: self.args.key.clone(),
            no_wait: false,
            timeout: 120,
            confirmations: 1,
            finalized: false,
            gas_limit: None,
            anyway: false,
            genesis: None,
            eip712: false,
            max_price: None,
            tip: None,
            multisig: None,
            out: None,
            tx,
        })
    }

    fn call(&self, to: Address, function: &str, amount: u256, args: Vec<TxCallArgumentType>) -> CliResult<()> {
        self.send(TxArgs::Call { to, function: function.to_string(), amount, args })
    }

    fn evm_call(&self, to: Address, function: &str, args: Vec<String>, value: u256) -> CliResult<()> {
        self.send(TxArgs::EvmCall { to, function: Some(function.to_string()), data: None, value, read: false, args })
    }

    /// The sender's nonce, which says where its next deploy lands.
    fn nonce(&self, me: &Address) -> CliResult<u64> {
        let account = ok(self.agent.get(format!("{}/accounts/{me}", self.node)).call())?;

        Ok(account["nonce"].as_u64().unwrap_or(0))
    }

    fn deploy_lua(&self, me: &Address, name: &str, source: &str) -> CliResult<Address> {
        let at = deployed_at(me, self.nonce(me)?, crate::structs::hash::HashType::default_chain());
        let dir = std::env::temp_dir().join(format!("chainv4-dex-{}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(name);
        std::fs::write(&path, source)?;
        let sent = self.send(TxArgs::Deploy { lua: Some(path), wasm: None, evm: None, constructor: None, constructor_args: Vec::new() });
        let _ = std::fs::remove_dir_all(&dir);
        sent?;

        Ok(at)
    }

    /// Deploys EVM init code with its constructor's arguments after it.
    fn create(&self, me: &Address, bin: &str, constructor: Vec<u8>) -> CliResult<Address> {
        let at = crate::vm::evm::create_address(me, self.nonce(me)?);
        let dir = std::env::temp_dir().join(format!("chainv4-dex-{}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("code.bin");
        std::fs::write(&path, to_hex(&[from_hex(&format!("0x{}", bin.trim())).ok_or("built-in code is hex")?, constructor].concat()))?;
        let sent = self.send(TxArgs::Deploy { lua: None, wasm: None, evm: Some(path), constructor: None, constructor_args: Vec::new() });
        let _ = std::fs::remove_dir_all(&dir);
        sent?;

        Ok(at)
    }

    /// The DEX's fee in force, as the node says, which both DEXes charge: what a quote takes off.
    fn fee_bps(&self) -> CliResult<u32> {
        Ok(self.status()?["dex_fee_bps"].as_u64().and_then(|bps| u32::try_from(bps).ok()).unwrap_or(crate::dex::DEFAULT_FEE_BPS))
    }

    fn status(&self) -> CliResult<Value> {
        ok(self.agent.get(format!("{}/status", self.node)).call())
    }

    /// The native DEX: given, or the one the chain's directory names, or the one the node names.
    fn native(&self) -> CliResult<Address> {
        if let Some(dex) = self.args.dex {
            return Ok(dex);
        }

        let status = self.status()?;

        status["directory"]["dex"]
            .as_str()
            .or_else(|| status["dex"]["contract"].as_str())
            .ok_or("no DEX: the chain's directory names none, nor does the node. Give --dex")?
            .parse()
            .map_err(|_| "the node named a DEX that is not an address".into())
    }

    fn router_bytes(&self) -> CliResult<[u8; 20]> {
        let text = match &self.args.router {
            Some(router) => router.clone(),
            None => {
                let status = self.status()?;

                status["directory"]["dex_router"]
                    .as_str()
                    .or_else(|| status["dex"]["router"].as_str())
                    .ok_or("no EVM DEX: the chain's directory names no router, nor does the node. Give --router")?
                    .to_string()
            }
        };

        evm(&super::evm::evm_address(&text)?)
    }

    fn router(&self) -> CliResult<Address> {
        Ok(Address::Evm(self.router_bytes()?))
    }

    /// An EVM read, through the node's JSON-RPC.
    fn read(&self, to: [u8; 20], signature: &str, types: &[AbiType], values: &[AbiValue], returns: &str) -> CliResult<Vec<AbiValue>> {
        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": "eth_call", "params": [{ "to": to_hex(&to), "data": to_hex(&calldata(signature, types, values)?) }, "latest"] });
        let answer = ok(self.agent.post(format!("{}/rpc", self.node)).header("content-type", "application/json").send(body.to_string()))?;
        let output = answer["result"].as_str().and_then(from_hex).ok_or_else(|| format!("{signature}: {}", answer["error"]))?;
        let (_, types) = crate::abi::parse_signature(returns)?;

        Ok(decode_values(&types, &output)?)
    }

    fn wdev(&self) -> CliResult<[u8; 20]> {
        match self.read(self.router_bytes()?, "wdev()", &[], &[], "(address)")?.as_slice() {
            [AbiValue::Address(wdev)] => Ok(*wdev),
            _ => Err("the router names no WDEV".into()),
        }
    }

    /// An asset's ERC-20 on the EVM DEX: WDEV for the coin.
    fn token(&self, asset: &Address) -> CliResult<[u8; 20]> {
        match asset {
            asset if *asset == COIN => self.wdev(),
            other => evm(other),
        }
    }

    /// Approves the DEX to move a token, when what it may move already is short of amount: a native
    /// token by its approve, an ERC-20 at the native DEX's EVM address or the router's. Nothing for
    /// the coin. An allowance that cannot be read is taken as none.
    fn approve(&self, asset: &Address, amount: u256) -> CliResult<()> {
        if *asset == COIN {
            return Ok(());
        }

        let allowed = self.allowance(asset).unwrap_or(u256::ZERO);

        if allowed >= amount {
            println!("{asset}: the DEX may move {allowed} already, no approval sent");

            return Ok(());
        }

        match (asset, self.args.evm) {
            (Address::Evm(_), true) => self.evm_call(*asset, "approve(address,uint256)", vec![to_hex(&self.router_bytes()?), amount.to_string()], u256::ZERO),
            (Address::Evm(_), false) => self.evm_call(*asset, "approve(address,uint256)", vec![to_hex(&evm_address(&self.native()?)), amount.to_string()], u256::ZERO),
            (_, true) => Err(format!("{asset} is no ERC-20: the EVM DEX trades ERC-20s and the coin").into()),
            (token, false) => self.call(*token, "approve", u256::ZERO, vec![TxCallArgumentType::Address(self.native()?), TxCallArgumentType::U256(amount)]),
        }
    }

    /// What the DEX in use may move of a token for the signer now.
    fn allowance(&self, asset: &Address) -> CliResult<u256> {
        let me = signer(self.args.key.as_deref())?.public().address();

        match (asset, self.args.evm) {
            (Address::Evm(token), evm) => {
                let spender = if evm { self.router_bytes()? } else { evm_address(&self.native()?) };

                match self.read(*token, "allowance(address,address)", &[AbiType::Address, AbiType::Address], &[AbiValue::Address(evm_address(&me)), AbiValue::Address(spender)], "(uint256)")?[..] {
                    [AbiValue::Uint(allowed)] => Ok(allowed),
                    _ => Err("the token's allowance does not read".into()),
                }
            }
            (_, true) => Err(format!("{asset} is no ERC-20").into()),
            (token, false) => {
                let args = json!([{ "type": "address", "value": me.to_string() }, { "type": "address", "value": self.native()?.to_string() }]);
                let allowed = self.native_read(token, "allowance", args)?;

                Ok(allowed.first().and_then(|text| u256::from_str_radix(text, 10).ok()).ok_or("the token's allowance does not read")?)
            }
        }
    }

    /// The pools of the DEX in use, the EVM's with WDEV as the coin.
    fn pools(&self) -> CliResult<Vec<DexPool>> {
        match self.args.evm {
            false => {
                let dex = self.native()?;
                let count = self.native_read(&dex, "pool_count", json!([]))?.first().and_then(|count| count.parse::<u64>().ok()).unwrap_or(0);

                (0..count).map(|index| DexPool::from_values(&self.native_read(&dex, "pool_at", json!([{ "type": "i64", "value": index.to_string() }]))?).map_err(Into::into)).collect()
            }
            true => {
                let router = self.router_bytes()?;
                let [AbiValue::Address(factory)] = self.read(router, "factory()", &[], &[], "(address)")?[..] else {
                    return Err("the router names no factory".into());
                };
                let wdev = self.wdev()?;
                let [AbiValue::Uint(count)] = self.read(factory, "allPairsLength()", &[], &[], "(uint256)")?[..] else {
                    return Err("the factory does not say how many pairs".into());
                };
                let coin = |token: [u8; 20]| if token == wdev { COIN } else { Address::Evm(token) };
                let mut pools = Vec::new();

                for index in 0..u64::try_from(count).unwrap_or(0) {
                    let [AbiValue::Address(pair)] = self.read(factory, "allPairs(uint256)", &[AbiType::Uint(256)], &[AbiValue::Uint(u256::from(index))], "(address)")?[..] else {
                        continue;
                    };
                    let (token0, token1) = match (&self.read(pair, "token0()", &[], &[], "(address)")?[..], &self.read(pair, "token1()", &[], &[], "(address)")?[..]) {
                        ([AbiValue::Address(zero)], [AbiValue::Address(one)]) => (*zero, *one),
                        _ => continue,
                    };
                    if let [AbiValue::Uint(held0), AbiValue::Uint(held1), _] = self.read(pair, "getReserves()", &[], &[], "(uint112,uint112,uint32)")?[..] {
                        pools.push(DexPool { a: coin(token0), b: coin(token1), reserve_a: held0, reserve_b: held1, share: Address::Evm(pair) });
                    }
                }

                Ok(pools)
            }
        }
    }

    fn native_read(&self, contract: &Address, function: &str, args: Value) -> CliResult<Vec<String>> {
        let body = json!({ "contract": contract.to_string(), "function": function, "args": args });
        let answer = ok(self.agent.post(format!("{}/call", self.node)).header("content-type", "application/json").send(body.to_string()))?;

        match answer["status"].as_str() {
            Some("returned") => Ok(answer["values"].as_array().into_iter().flatten().map(|value| value["value"].as_str().map_or_else(|| value["value"].to_string(), str::to_string)).collect()),
            _ => Err(format!("{function}: {}", answer["error"]).into()),
        }
    }
}
