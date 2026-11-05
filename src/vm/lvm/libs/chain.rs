use {
    crate::vm::lvm::{charge, costs},
    crate::vm::{Host},
    mlua::{Error, Lua},
    r#macro::document,
};

// A Lua number keeps it exactly: the id is six bytes of the genesis id, under 2^53.
fn id(lua: &Lua, (): ()) -> mlua::Result<f64> {
    let host = lua
        .app_data_ref::<Host>()
        .map(|host| host.clone())
        .ok_or_else(|| Error::runtime("chain is only available inside a call"))?;
    charge(lua, costs(lua).chain_id)?;

    Ok(host.chain_id() as f64)
}

#[document(
    lua,
    kind = "library",
    name = "chain",
    category = "Chain",
    summary = "The chain a contract runs on: read-only, and nothing a contract's owner sets, so what a contract signs, pays or takes can be bound to this one chain.",
    note = "Reading its id costs 20 fuel by default, its DEX fee and Ethereum network 200.",
    example = "assert(chain.id() == 11155111, \"this contract is for Sepolia\")",
    see_also = "federation",
)]
fn chain_lib() {}

#[document(
    lua,
    parent = "chain",
    name = "id",
    summary = "The chain's id: what its EVM answers CHAINID with and the bridges name it by, six bytes of its genesis id.",
    returns = { ty = "number", desc = "A whole number under 2^48, at least 1." },
)]
fn chain_id() {}

// The DEX's fee in basis points, as governance set it for the call's block.
fn dex_fee(lua: &Lua, (): ()) -> mlua::Result<f64> {
    let host = lua
        .app_data_ref::<Host>()
        .map(|host| host.clone())
        .ok_or_else(|| Error::runtime("chain is only available inside a call"))?;
    charge(lua, costs(lua).federation)?;

    Ok(f64::from(host.rules().dex.fee_bps))
}

#[document(
    lua,
    parent = "chain",
    name = "dex_fee",
    summary = "The DEX's fee, in hundredths of a percent: what a swap keeps for the pool, 30 being 0.3%. Genesis sets it and governance changes it, from the block after a decision; both DEXes charge it.",
    returns = { ty = "number", desc = "0 to 1000." },
)]
fn chain_dex_fee() {}

// The Ethereum network anchoring names, as SSZ.
fn eth_network(lua: &Lua, (): ()) -> mlua::Result<Option<mlua::String>> {
    let host = lua
        .app_data_ref::<Host>()
        .map(|host| host.clone())
        .ok_or_else(|| Error::runtime("chain is only available inside a call"))?;
    charge(lua, costs(lua).federation)?;

    match host.eth_network().map_err(super::accounts::refused)? {
        Some(bytes) => lua.create_string(&bytes).map(Some),
        None => Ok(None),
    }
}

#[document(
    lua,
    parent = "chain",
    name = "eth_network",
    summary = "The Ethereum beacon network anchoring's settings in force name, as eth_start's network: its whole definition, SSZ-encoded as ethlight's Network writes it, a preset's built-in one or the one governance carried. What an Ethereum light client follows, so a vote changes its fork schedule without a redeploy. Costs 200 fuel by default.",
    returns = { ty = "string | nil", desc = "The SSZ bytes, or nil while the settings name no Ethereum network." },
)]
fn chain_eth_network() {}

pub fn register_chain_lib(lua: &Lua) {
    let table = lua.create_table().expect("[chain] Failed to create chain table");
    table.set("id", lua.create_function(id).expect("[chain] Failed to create id")).expect("[chain] Failed to set id");
    table.set("dex_fee", lua.create_function(dex_fee).expect("[chain] Failed to create dex_fee")).expect("[chain] Failed to set dex_fee");
    table.set("eth_network", lua.create_function(eth_network).expect("[chain] Failed to create eth_network")).expect("[chain] Failed to set eth_network");
    lua.globals().set("chain", table).expect("[chain] Failed to set chain table");
}
