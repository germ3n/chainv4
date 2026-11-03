use {
    super::accounts::refused,
    crate::vm::lvm::{charge, costs},
    crate::vm::{Host},
    mlua::{Error, Lua, Table},
    r#macro::document,
};

fn get(lua: &Lua, (): ()) -> mlua::Result<Option<Table>> {
    let host = lua
        .app_data_ref::<Host>()
        .map(|host| host.clone())
        .ok_or_else(|| Error::runtime("federation is only available inside a call"))?;
    charge(lua, costs(lua).federation)?;
    let Some(decided) = host.federation().map_err(refused)? else {
        return Ok(None);
    };
    let table = lua.create_table()?;
    table.set("version", decided.version)?;
    table.set("threshold", decided.federation.threshold)?;
    table.set("chain_depth", decided.federation.chain_depth)?;
    table.set("evm_depth", decided.federation.evm_depth)?;
    table.set("bitcoin_depth", decided.federation.bitcoin_depth)?;
    let signers = lua.create_table()?;

    for key in &decided.federation.signers {
        signers.push(lua.create_string(key)?)?;
    }

    table.set("signers", signers)?;

    if let Some(btc) = decided.federation.btc {
        let policy = lua.create_table()?;
        policy.set("fee_cap", btc.fee_cap)?;
        policy.set("fee_floor", btc.fee_floor)?;
        policy.set("grace_ms", btc.grace_ms)?;
        policy.set("cancel_after_ms", btc.cancel_after_ms)?;
        policy.set("fee_part", btc.fee_part)?;
        table.set("btc", policy)?;
    }

    Ok(Some(table))
}

#[document(
    lua,
    kind = "library",
    name = "federation",
    category = "Chain",
    summary = "The bridges' federation in force: the signers who vouch for what crossed from another chain and sign what leaves for one, as genesis named them or governance decided last.",
    note = "Reading it costs 200 fuel.",
    example = "local fed = federation.get()\nassert(fed, \"this chain has no federation\")\nfor _, key in ipairs(fed.signers) do\n    -- 33-byte compressed secp256k1 keys\nend",
    see_also = "crypto.recover",
)]
fn federation_lib() {}

#[document(
    lua,
    parent = "federation",
    name = "get",
    summary = "The federation in force.",
    returns = { ty = "table | nil", desc = "version, 0 for genesis's and one more for each decided since; threshold, how many must sign; signers, their 33-byte compressed secp256k1 keys, in order; chain_depth, evm_depth and bitcoin_depth, how deep each chain's locks must be; and btc, the Bitcoin policy, with fee_cap, fee_floor, grace_ms, cancel_after_ms and fee_part, or nil while the federation names none. nil on a chain with none." },
)]
fn federation_get() {}

pub fn register_federation_lib(lua: &Lua) {
    let table = lua.create_table().expect("[federation] Failed to create federation table");
    table.set("get", lua.create_function(get).expect("[federation] Failed to create get")).expect("[federation] Failed to set get");
    lua.globals().set("federation", table).expect("[federation] Failed to set federation table");
}
