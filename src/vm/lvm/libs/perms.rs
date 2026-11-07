use {
    super::address::LuaAddress,
    crate::vm::{CHECK_PERMS, Host},
    mlua::{AnyUserData, Error, Function, Lua, MultiValue, Table, Value},
    r#macro::document,
};

/// The table the running script's top level returned, which perms asks its check_perms through.
pub struct Module(pub Table);

/// Asks the running contract's own check_perms: whether who, the caller unless named, has the
/// permission, for what detail says. A contract without check_perms permits nothing.
fn ask(lua: &Lua, (permission, who, detail): (String, Option<AnyUserData>, Value)) -> mlua::Result<bool> {
    let who = match who {
        Some(who) => LuaAddress(who.borrow::<LuaAddress>()?.0),
        None => LuaAddress(
            lua.app_data_ref::<Host>()
                .map(|host| host.context().sender)
                .ok_or_else(|| Error::runtime("perms is only available inside a call"))?,
        ),
    };
    let module = lua
        .app_data_ref::<Module>()
        .map(|module| module.0.clone())
        .ok_or_else(|| Error::runtime("perms is only available inside a call"))?;
    let Value::Function(check) = module.raw_get::<Value>(CHECK_PERMS)? else {
        return Ok(false);
    };
    let check: Function = check;
    let answer: MultiValue = check.call((permission, who, detail))?;

    // A single true, as the chain asks of it.
    Ok(answer.len() == 1 && matches!(answer.front(), Some(Value::Boolean(true))))
}

#[document(
    lua,
    kind = "library",
    name = "perms",
    category = "Chain",
    summary = "Asks the contract's own check_perms about its own privileged functions, as the chain asks it about \"upgrade\" and \"remove\": so who may mint, pause or set a price is decided in one place, which the apps can ask too, by calling check_perms.",
    note = "check_perms runs as part of this call, what it writes kept with the rest; keep it to reading. The chain's own questions run it on a throwaway copy. Costs what check_perms burns.",
    example = "function M.check_perms(permission, who, detail)\n    if permission == \"mint\" then\n        return storage.get(\"minter:\" .. who:hex()) ~= nil\n    end\n\n    return who:hex() == storage.get(\"owner\")\nend\n\nfunction M.mint(to, amount)\n    perms.require(\"mint\")\n    -- ...\nend",
)]
fn perms_lib() {}

#[document(
    lua,
    parent = "perms",
    name = "check",
    summary = "Whether the contract's check_perms gives who the permission, for detail: true only for a single true, false for a contract without check_perms.",
    params = {
        permission = { ty = "string", desc = "Its name, such as \"mint\"." },
        who = { ty = "address?", desc = "Who asks: ctx.sender unless given." },
        detail = { ty = "any?", desc = "What it is for, passed on as check_perms's third argument: an amount, an address, or nothing." },
    },
    returns = { ty = "boolean", desc = "Whether it is permitted." },
)]
fn perms_check() {}

#[document(
    lua,
    parent = "perms",
    name = "require",
    summary = "perms.check, raising \"not permitted: <permission>\" unless it is permitted.",
    params = {
        permission = { ty = "string", desc = "Its name." },
        who = { ty = "address?", desc = "Who asks: ctx.sender unless given." },
        detail = { ty = "any?", desc = "What it is for." },
    },
    returns = { ty = "nil", desc = "" },
)]
fn perms_require() {}

pub fn register_perms_lib(lua: &Lua) {
    let perms = lua.create_table().expect("[perms] Failed to create the perms table");
    perms.set("check", lua.create_function(ask).expect("[perms] Failed to create check")).expect("[perms] Failed to set check");
    perms
        .set(
            "require",
            lua.create_function(|lua, (permission, who, detail): (String, Option<AnyUserData>, Value)| {
                match ask(lua, (permission.clone(), who, detail))? {
                    true => Ok(()),
                    false => Err(Error::runtime(format!("not permitted: {permission}"))),
                }
            })
            .expect("[perms] Failed to create require"),
        )
        .expect("[perms] Failed to set require");
    lua.globals().set("perms", perms).expect("[perms] Failed to set the perms table");
}
