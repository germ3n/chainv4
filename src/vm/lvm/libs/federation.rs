use {
    super::accounts::refused,
    super::crypto::whole,
    crate::registry::federation::Federation,
    crate::vm::lvm::{charge, costs},
    crate::vm::{Host},
    mlua::{Error, Lua, Table, Value},
    r#macro::document,
};

fn host(lua: &Lua) -> mlua::Result<Host> {
    lua.app_data_ref::<Host>()
        .map(|host| host.clone())
        .ok_or_else(|| Error::runtime("federation is only available inside a call"))
}

fn get(lua: &Lua, (): ()) -> mlua::Result<Option<Table>> {
    let host = host(lua)?;
    charge(lua, costs(lua).federation)?;
    let Some(decided) = host.federation().map_err(refused)? else {
        return Ok(None);
    };

    federation_table(lua, decided.version, &decided.federation).map(Some)
}

// A federation of any version, with when it was decided and when the next replaced it, each as a
// height and a block time.
fn at(lua: &Lua, version: Value) -> mlua::Result<Option<Table>> {
    let version = whole(&version, "a federation's version")?;
    let host = host(lua)?;
    charge(lua, costs(lua).federation)?;
    let Some(found) = host.federation_at(version).map_err(refused)? else {
        return Ok(None);
    };
    let table = federation_table(lua, found.version, &found.federation)?;

    // Whole numbers well inside the 2^53 a Lua number keeps exactly, as ctx.time is.
    if let Some(decided) = found.decided {
        table.set("decided_height", decided.height as f64)?;
        table.set("decided_time", decided.time as f64)?;
    }

    if let Some(replaced) = found.replaced {
        table.set("replaced_height", replaced.height as f64)?;
        table.set("replaced_time", replaced.time as f64)?;
    }

    Ok(Some(table))
}

fn federation_table(lua: &Lua, version: u64, federation: &Federation) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    table.set("version", version)?;
    table.set("threshold", federation.threshold)?;
    table.set("chain_depth", federation.chain_depth)?;
    table.set("evm_depth", federation.evm_depth)?;
    table.set("bitcoin_depth", federation.bitcoin_depth)?;
    let signers = lua.create_table()?;

    for key in &federation.signers {
        signers.push(lua.create_string(key)?)?;
    }

    table.set("signers", signers)?;

    if let Some(btc) = federation.btc {
        let policy = lua.create_table()?;
        policy.set("fee_cap", btc.fee_cap)?;
        policy.set("fee_floor", btc.fee_floor)?;
        policy.set("grace_ms", btc.grace_ms)?;
        policy.set("cancel_after_ms", btc.cancel_after_ms)?;
        policy.set("fee_part", btc.fee_part)?;
        policy.set("bump_after_ms", btc.bump_after_ms)?;
        policy.set("bump_below_pct", btc.bump_below_pct)?;
        policy.set("repay_pct", btc.repay_pct)?;
        policy.set("repay_most", btc.repay_most)?;
        table.set("btc", policy)?;
    }

    Ok(table)
}

#[document(
    lua,
    kind = "library",
    name = "federation",
    category = "Chain",
    summary = "The bridges' federation in force: the signers who vouch for what crossed from another chain and sign what leaves for one, as genesis named them or governance decided last; and every one before it, by its version, as the chain keeps them all.",
    note = "Reading one, in force or past, costs 200 fuel.",
    example = "local fed = federation.get()\nassert(fed, \"this chain has no federation\")\nfor _, key in ipairs(fed.signers) do\n    -- 33-byte compressed secp256k1 keys\nend",
    see_also = "crypto.recover",
)]
fn federation_lib() {}

#[document(
    lua,
    parent = "federation",
    name = "get",
    summary = "The federation in force.",
    returns = { ty = "table | nil", desc = "version, 0 for genesis's and one more for each decided since; threshold, how many must sign; signers, their 33-byte compressed secp256k1 keys, in order; chain_depth, evm_depth and bitcoin_depth, how deep each chain's locks must be; and btc, the Bitcoin policy, with fee_cap, fee_floor, grace_ms, cancel_after_ms, fee_part, bump_after_ms, bump_below_pct, repay_pct and repay_most, or nil while the federation names none. nil on a chain with none." },
)]
fn federation_get() {}

#[document(
    lua,
    parent = "federation",
    name = "at",
    summary = "The federation of a version, in force or past: the chain keeps every one it decided, so a contract reads one it never saw in force, and when the next replaced it. What the Bitcoin gateway mints a deposit to an old federation's address by, within its grace from that replacement.",
    params = { version = { ty = "number", desc = "Its version: 0 for genesis's, and one more for each decided since." } },
    returns = { ty = "table | nil", desc = "What federation.get gives, with decided_height and decided_time, the block that decided it, and once the next version was decided, replaced_height and replaced_time, that block's; it is in force from the block after one until the block after the other. nil for a version never decided." },
    example = "local old = federation.at(3)\nif old and old.replaced_time and ctx.time > old.replaced_time + grace then\n    error(\"that federation's addresses closed\")\nend",
)]
fn federation_at() {}

pub fn register_federation_lib(lua: &Lua) {
    let table = lua.create_table().expect("[federation] Failed to create federation table");
    table.set("get", lua.create_function(get).expect("[federation] Failed to create get")).expect("[federation] Failed to set get");
    table.set("at", lua.create_function(at).expect("[federation] Failed to create at")).expect("[federation] Failed to set at");
    lua.globals().set("federation", table).expect("[federation] Failed to set federation table");
}
