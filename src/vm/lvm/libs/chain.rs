use {
    crate::vm::lvm::charge,
    crate::vm::{Host, cost},
    mlua::{Error, Lua},
    r#macro::document,
};

// A Lua number keeps it exactly: the id is six bytes of the genesis id, under 2^53.
fn id(lua: &Lua, (): ()) -> mlua::Result<f64> {
    let host = lua
        .app_data_ref::<Host>()
        .map(|host| host.clone())
        .ok_or_else(|| Error::runtime("chain is only available inside a call"))?;
    charge(lua, cost::CHAIN_ID)?;

    Ok(host.chain_id() as f64)
}

#[document(
    lua,
    kind = "library",
    name = "chain",
    category = "Chain",
    summary = "The chain a contract runs on: read-only, and nothing a contract's owner sets, so what a contract signs, pays or takes can be bound to this one chain.",
    note = "Reading it costs 20 fuel.",
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

pub fn register_chain_lib(lua: &Lua) {
    let table = lua.create_table().expect("[chain] Failed to create chain table");
    table.set("id", lua.create_function(id).expect("[chain] Failed to create id")).expect("[chain] Failed to set id");
    lua.globals().set("chain", table).expect("[chain] Failed to set chain table");
}
