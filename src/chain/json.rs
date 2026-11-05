//! Genesis as JSON: what genesis files are read and written as. Every field must be present, and
//! unknown fields are refused, so a misspelled parameter is an error rather than a silent
//! default. A change under governance, and each part of genesis one may replace, are
//! params::json's, which this re-exports, so the apps read them without a node.

use {
    crate::chain::{Allocation, Coin, Genesis, Reward},
    crate::registry::names::NamesConfig,
    crate::structs::code::Engine,
    crate::structs::u256::u256,
    crate::vm::Limits,
    serde_json::{Value, json},
};

pub use crate::params::json::*;

pub fn to_json(genesis: &Genesis) -> Value {
    json!({
        "timestamp": genesis.timestamp,
        "hash": genesis.hash.name(),
        "coin": coin_json(&genesis.coin),
        "consensus": consensus_json(&genesis.consensus, genesis.pow_window),
        "reward": {
            "initial": genesis.reward.initial.to_string(),
            "halving_interval": genesis.reward.halving_interval,
        },
        "minter": genesis.minter.map(|minter| minter.to_string()),
        "allocations": genesis.allocations.iter().map(|allocation| json!({
            "address": allocation.address.to_string(),
            "amount": allocation.amount.to_string(),
        })).collect::<Vec<_>>(),
        "engines": genesis.engines.iter().map(|engine| engine.name()).collect::<Vec<_>>(),
        "tx_limits": {
            "fuel": genesis.tx_limits.fuel,
            "memory": genesis.tx_limits.memory,
        },
        "token_functions": genesis.token_functions,
        "max_reorg_depth": genesis.max_reorg_depth,
        "finality": genesis.finality.as_ref().map(finality_json),
        "fees": genesis.fees.as_ref().map(fees_json),
        "max_block_bytes": genesis.max_block_bytes,
        "vote_reward_source": genesis.vote_reward_source.map(|source| source.name()),
        "governance": genesis.governance.as_ref().map(governance_json),
        "bootnodes": genesis.bootnodes,
        "names": genesis.names.as_ref().map(names_json),
        "collections": genesis.collections.as_ref().map(collections_json),
        "federation": genesis.federation.as_ref().map(federation_json),
        "directory": genesis.directory.as_ref().map(directory_json),
        "name": genesis.name,
        "directory_admin": genesis.directory_admin.map(|admin| admin.to_string()),
        "contract_limits": genesis.contract_limits.as_ref().map(crate::params::json::contract_limits_json),
        "max_tx_bytes": genesis.max_tx_bytes,
        "multisig_members": genesis.multisig_members,
        "evm_spec": genesis.evm_spec.map(|spec| spec.name()),
        "dex": genesis.dex.as_ref().map(crate::params::json::dex_json),
        "anchoring": genesis.anchoring.as_ref().map(crate::params::json::anchoring_json),
        "locked": genesis.locked,
    })
}

/// A chain's names as JSON: the shape genesis files and GET /names use, prices as decimal strings.
pub fn names_json(names: &NamesConfig) -> Value {
    json!({
        "suffix": names.suffix,
        "prices": names.prices.iter().map(u256::to_string).collect::<Vec<_>>(),
        "grace_ms": names.grace_ms,
        "max_years": names.max_years,
    })
}

fn names_from_json(mut names: Fields) -> Result<NamesConfig, String> {
    let config = NamesConfig {
        suffix: names.text("suffix")?,
        prices: prices_from_json(names.get("prices")?, "names.prices")?,
        grace_ms: names.whole("grace_ms")?,
        max_years: names.small("max_years")?,
    };
    names.done()?;

    Ok(config)
}

/// The coin as JSON: the shape genesis files and GET /status use.
pub fn coin_json(coin: &Coin) -> Value {
    json!({ "symbol": coin.symbol, "decimals": coin.decimals })
}

pub fn from_json(value: &Value) -> Result<Genesis, String> {
    let mut root = Fields::of(value, "genesis")?;
    let mut coin = root.object("coin")?;
    let coin_value = Coin {
        symbol: coin.text("symbol")?,
        decimals: u8::try_from(coin.whole("decimals")?).map_err(|_| "coin.decimals is too large".to_string())?,
    };
    coin.done()?;
    let (consensus, pow_window) = consensus_from_json(root.get("consensus")?, "consensus")?;
    let mut reward = root.object("reward")?;
    let reward_value = Reward {
        initial: reward.amount("initial")?,
        halving_interval: reward.optional_whole("halving_interval")?,
    };
    reward.done()?;
    let minter = root.optional_parse("minter")?;
    let allocations = root
        .list("allocations")?
        .into_iter()
        .map(|mut item| {
            let allocation = Allocation {
                address: item.parse("address")?,
                amount: item.amount("amount")?,
            };
            item.done()?;

            Ok(allocation)
        })
        .collect::<Result<Vec<_>, String>>()?;
    let engines = root
        .strings("engines")?
        .iter()
        .map(|name| match name.as_str() {
            "lua" => Ok(Engine::Lua),
            "wasm" => Ok(Engine::Wasm),
            "evm" => Ok(Engine::Evm),
            other => Err(format!("engines: unknown engine {other}; use lua, wasm or evm")),
        })
        .collect::<Result<Vec<_>, String>>()?;
    let mut limits = root.object("tx_limits")?;
    let tx_limits = Limits {
        fuel: limits.whole("fuel")?,
        memory: usize::try_from(limits.whole("memory")?).map_err(|_| "tx_limits.memory is too large".to_string())?,
    };
    limits.done()?;
    let token_functions = root.strings("token_functions")?;
    let max_reorg_depth = root.whole("max_reorg_depth")?;
    let finality = root.optional_object("finality")?.map(finality_from_json).transpose()?;
    let fees = root.optional_object("fees")?.map(fees_from_fields).transpose()?;
    // Absent from files written before it: those chains keep their id, bounded by gas alone.
    let max_block_bytes = root.later_whole("max_block_bytes")?;
    let vote_reward_source = match root.map.contains_key("vote_reward_source") {
        true => root.optional_parse("vote_reward_source")?,
        false => None,
    };
    // Absent from files written before it: those chains keep their id, and nothing changes them.
    let governance = match root.map.contains_key("governance") {
        true => root.optional_object("governance")?.map(governance_from_json).transpose()?,
        false => None,
    };
    // Absent from files written before it: those chains keep their id, with no bootnodes.
    let bootnodes = match root.map.contains_key("bootnodes") {
        true => root.strings("bootnodes")?,
        false => Vec::new(),
    };
    // Absent from files written before it: those chains keep their id, with no names.
    let names = match root.map.contains_key("names") {
        true => root.optional_object("names")?.map(names_from_json).transpose()?,
        false => None,
    };
    // Absent from files written before it: those chains keep their id, with no collections.
    let collections = match root.map.contains_key("collections") {
        true => root.optional_object("collections")?.map(collections_from_json).transpose()?,
        false => None,
    };
    // Absent from files written before it: those chains keep their id, with no federation.
    let federation = match root.map.contains_key("federation") {
        true => root.optional_object("federation")?.map(federation_from_json).transpose()?,
        false => None,
    };
    // Absent from files written before it: those chains keep their id, announcing no directory.
    let directory = match root.map.contains_key("directory") {
        true => root.optional_object("directory")?.map(directory_from_json).transpose()?,
        false => None,
    };
    // Absent from files written before it too: those chains keep their id, and are called chainv4.
    let name = match root.map.contains_key("name") {
        true => root.optional_text("name")?,
        false => None,
    };
    // And so is who sets the directory without governance.
    let directory_admin = match root.map.contains_key("directory_admin") {
        true => root.optional_parse("directory_admin")?,
        false => None,
    };
    // Each absent from files written before it: those chains run by the defaults.
    let contract_limits = match root.map.contains_key("contract_limits") {
        true => root.optional_object("contract_limits")?.map(crate::params::json::contract_limits_from_json).transpose()?,
        false => None,
    };
    let max_tx_bytes = root.later_whole("max_tx_bytes")?;
    let multisig_members = root.later_whole("multisig_members")?.map(|members| u32::try_from(members).map_err(|_| "multisig_members is too large".to_string())).transpose()?;
    let evm_spec = match root.map.contains_key("evm_spec") {
        true => root.optional_parse("evm_spec")?,
        false => None,
    };
    let dex = match root.map.contains_key("dex") {
        true => root.optional_object("dex")?.map(crate::params::json::dex_from_json).transpose()?,
        false => None,
    };
    let anchoring = match root.map.contains_key("anchoring") {
        true => root.optional_object("anchoring")?.map(crate::params::json::anchoring_from_json).transpose()?,
        false => None,
    };
    // Absent from files written before it: those chains keep their id, with nothing locked.
    let locked = match root.map.contains_key("locked") {
        true => root.strings("locked")?,
        false => Vec::new(),
    };

    if let Some(problem) = crate::governance::locks::genesis_problem(&locked) {
        return Err(problem);
    }

    let genesis = Genesis {
        timestamp: root.whole("timestamp")?,
        hash: root.hash("hash")?,
        coin: coin_value,
        consensus,
        reward: reward_value,
        minter,
        allocations,
        engines,
        tx_limits,
        token_functions,
        max_reorg_depth,
        finality,
        fees,
        max_block_bytes,
        pow_window,
        governance,
        vote_reward_source,
        bootnodes,
        names,
        collections,
        federation,
        directory,
        name,
        directory_admin,
        contract_limits,
        max_tx_bytes,
        multisig_members,
        evm_spec,
        dex,
        anchoring,
        locked,
    };
    root.done()?;

    Ok(genesis)
}
