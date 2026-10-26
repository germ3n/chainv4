use {
    super::accounts::refused,
    super::address::LuaAddress,
    crate::vm::lvm::charge,
    crate::vm::{Host, cost},
    mlua::{AnyUserData, Error, Lua},
    r#macro::document,
};

fn host(lua: &Lua) -> mlua::Result<Host> {
    lua.app_data_ref::<Host>()
        .map(|host| host.clone())
        .ok_or_else(|| Error::runtime("names is only available inside a call"))
}

fn resolve(lua: &Lua, name: String) -> mlua::Result<Option<LuaAddress>> {
    let host = host(lua)?;
    charge(lua, cost::NAME)?;

    host.resolve_name(&name).map(|address| address.map(LuaAddress)).map_err(refused)
}

fn name_of(lua: &Lua, address: AnyUserData) -> mlua::Result<Option<String>> {
    let address = *address.borrow::<LuaAddress>()?;
    let host = host(lua)?;
    charge(lua, cost::NAME)?;

    host.name_of(&address.0).map_err(refused)
}

#[document(
    lua,
    kind = "library",
    name = "names",
    category = "Chain",
    summary = "The chain's names, such as alice.dev: what a name resolves to, and the name an address goes by. Judged at the block's time, so an expired name resolves to nothing.",
    note = "Each lookup costs 500 fuel. On a chain without names, both raise an error.",
    example = "function M.pay(name, amount)\n    local to = names.resolve(name)\n    assert(to, name .. \" resolves to nothing\")\n    accounts.transfer(to, u256(amount))\nend",
    see_also = "accounts.transfer",
)]
fn names_lib() {}

#[document(
    lua,
    parent = "names",
    name = "resolve",
    summary = "The address a name resolves to now, with or without the chain's suffix, in any case.",
    params = { name = { ty = "string", desc = "The name, such as alice.dev or pay.alice." } },
    returns = { ty = "Address | nil", desc = "Its target, or nil for a name nobody holds, one that expired, or text that is no name." },
)]
fn names_resolve() {}

#[document(
    lua,
    parent = "names",
    name = "name_of",
    summary = "The name an address goes by: its primary name, with the chain's suffix, while that name resolves to it.",
    params = { address = { ty = "Address", desc = "Any address." } },
    returns = { ty = "string | nil", desc = "Such as alice.dev, or nil." },
)]
fn names_name_of() {}

pub fn register_names_lib(lua: &Lua) {
    let names_table = lua
        .create_table()
        .expect("[names] Failed to create names table");

    names_table
        .set(
            "resolve",
            lua.create_function(resolve)
                .expect("[names] Failed to create resolve"),
        )
        .expect("[names] Failed to set resolve");
    names_table
        .set(
            "name_of",
            lua.create_function(name_of)
                .expect("[names] Failed to create name_of"),
        )
        .expect("[names] Failed to set name_of");

    lua.globals()
        .set("names", names_table)
        .expect("[names] Failed to set names table");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outside_a_call_there_are_no_names_to_look_up() {
        let lua = Lua::new();
        lua.set_fuel(u64::MAX).unwrap();
        register_names_lib(&lua);
        let err = lua.load("names.resolve('alice.dev')").exec().unwrap_err();

        assert!(err.to_string().contains("only available inside a call"), "{err}");
    }
}
