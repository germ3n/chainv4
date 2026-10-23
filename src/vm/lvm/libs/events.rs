use {
    super::accounts::refused,
    crate::vm::lvm::{charge, value},
    crate::vm::{Host, cost},
    mlua::{Error, Lua, MultiValue},
    r#macro::document,
};

fn emit(lua: &Lua, (name, data): (String, MultiValue)) -> mlua::Result<()> {
    let host = lua
        .app_data_ref::<Host>()
        .map(|host| host.clone())
        .ok_or_else(|| Error::runtime("events is only available inside a call"))?;
    let data = data
        .into_iter()
        .map(value::from_lua)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| Error::runtime(format!("events.emit: {err}")))?;
    let size = host.emit(&name, data).map_err(refused)?;

    charge(lua, cost::EVENT + size as u64 / cost::EVENT_BYTES_PER_FUEL)
}

#[document(
    lua,
    kind = "library",
    name = "events",
    category = "Chain",
    summary = "Announces what a contract did, for clients to follow. Events land in the transaction's receipt, in the order they were emitted, and only if the call, and every call above it, succeeds.",
    note = "An event costs 200 fuel plus 1 for every 16 bytes of name and data. Names are 1 to 64 bytes, data at most 16 KiB encoded, and one call emits at most 256 events.",
    example = "function M.transfer(to, amount)\n    -- move the balance in storage, then:\n    events.emit(\"Transfer\", ctx.sender, to, amount)\nend",
)]
fn events_lib() {}

#[document(
    lua,
    parent = "events",
    name = "emit",
    summary = "Emits an event from this contract.",
    params = {
        name = { ty = "string", desc = "What happened, such as \"Transfer\"." },
        data = { ty = "...", desc = "The details: anything a contract function can return.", optional = true },
    },
    returns = { ty = "nil", desc = "" },
)]
fn events_emit() {}

pub fn register_events_lib(lua: &Lua) {
    let events_table = lua
        .create_table()
        .expect("[events] Failed to create events table");

    events_table
        .set(
            "emit",
            lua.create_function(emit)
                .expect("[events] Failed to create emit"),
        )
        .expect("[events] Failed to set emit");

    lua.globals()
        .set("events", events_table)
        .expect("[events] Failed to set events table");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outside_a_call_there_is_nowhere_to_emit_to() {
        let lua = Lua::new();
        lua.set_fuel(u64::MAX).unwrap();
        register_events_lib(&lua);
        let err = lua.load("events.emit('Nothing')").exec().unwrap_err();

        assert!(err.to_string().contains("only available inside a call"), "{err}");
    }
}
