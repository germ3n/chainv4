use {
    super::accounts::refused,
    crate::vm::lvm::charge,
    crate::vm::{Host, cost},
    mlua::{Error, Lua, Table},
    r#macro::document,
};

fn get(lua: &Lua, (): ()) -> mlua::Result<Option<Table>> {
    let host = lua
        .app_data_ref::<Host>()
        .map(|host| host.clone())
        .ok_or_else(|| Error::runtime("federation is only available inside a call"))?;
    charge(lua, cost::FEDERATION)?;
    let Some(decided) = host.federation().map_err(refused)? else {
        return Ok(None);
    };
    let table = lua.create_table()?;
    table.set("version", decided.version)?;
    table.set("threshold", decided.federation.threshold)?;
    let signers = lua.create_table()?;

    for key in &decided.federation.signers {
        signers.push(lua.create_string(key)?)?;
    }

    table.set("signers", signers)?;

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
    returns = { ty = "table | nil", desc = "version, 0 for genesis's and one more for each decided since; threshold, how many must sign; and signers, their 33-byte compressed secp256k1 keys, in order. nil on a chain with none." },
)]
fn federation_get() {}

pub fn register_federation_lib(lua: &Lua) {
    let table = lua.create_table().expect("[federation] Failed to create federation table");
    table.set("get", lua.create_function(get).expect("[federation] Failed to create get")).expect("[federation] Failed to set get");
    lua.globals().set("federation", table).expect("[federation] Failed to set federation table");
}
