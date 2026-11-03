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
    note = "Reading its id costs 20 fuel by default, its DEX fee 200.",
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

pub fn register_chain_lib(lua: &Lua) {
    let table = lua.create_table().expect("[chain] Failed to create chain table");
    table.set("id", lua.create_function(id).expect("[chain] Failed to create id")).expect("[chain] Failed to set id");
    table.set("dex_fee", lua.create_function(dex_fee).expect("[chain] Failed to create dex_fee")).expect("[chain] Failed to set dex_fee");
    lua.globals().set("chain", table).expect("[chain] Failed to set chain table");
}
