use {
    super::accounts::refused,
    super::address::LuaAddress,
    super::u256::{LuaU256, operand},
    crate::structs::u256::u256,
    crate::vm::lvm::{charge, value},
    crate::vm::{FunctionId, Host, VmError, cost, selector},
    mlua::{AnyUserData, Error, Lua, MultiValue, Value},
    r#macro::document,
};

fn call(lua: &Lua, (target, function, args): (AnyUserData, String, MultiValue)) -> mlua::Result<MultiValue> {
    call_with(lua, target, u256::ZERO, function, args)
}

fn call_value(
    lua: &Lua,
    (target, value, function, args): (AnyUserData, Value, String, MultiValue),
) -> mlua::Result<MultiValue> {
    call_with(lua, target, operand(&value)?, function, args)
}

fn delegate(lua: &Lua, (target, function, args): (AnyUserData, Value, MultiValue)) -> mlua::Result<MultiValue> {
    let target = *target.borrow::<LuaAddress>()?;
    let host = lua
        .app_data_ref::<Host>()
        .map(|host| host.clone())
        .ok_or_else(|| Error::runtime("contracts.delegate is only available inside a call"))?;
    let name = match &function {
        Value::String(name) => Some(name.to_str()?.to_string()),
        _ => None,
    };
    let id = match &name {
        Some(name) => FunctionId::Name(name),
        None => FunctionId::Selector(
            u64::try_from(operand(&function)?).map_err(|_| Error::runtime("a selector fits in 64 bits"))?,
        ),
    };
    let args = args
        .into_iter()
        .map(value::from_lua)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| Error::runtime(format!("contracts.delegate: {err}")))?;
    charge(lua, cost::CALL)?;
    let fuel = lua.get_fuel()?;
    let outcome = host.delegate(&target.0, id, &args, fuel);
    lua.set_fuel(fuel.saturating_sub(outcome.fuel_used))?;
    returned(lua, outcome.result, "delegate", &target)
}

fn selector_of(_: &Lua, name: String) -> mlua::Result<LuaU256> {
    Ok(LuaU256(u256::from(selector(&name))))
}

/// A call's return values as Lua values, or its failure as a Lua error.
fn returned(
    lua: &Lua,
    result: Result<Vec<crate::structs::tx::TxCallArgumentType>, VmError>,
    what: &str,
    target: &LuaAddress,
) -> mlua::Result<MultiValue> {
    match result {
        Ok(returns) => returns
            .iter()
            .map(|value| value::to_lua(lua, value))
            .collect::<Result<Vec<_>, _>>()
            .map(MultiValue::from_vec)
            .map_err(|err| Error::runtime(format!("contracts.{what}: {err}"))),
        Err(err @ VmError::Storage(_)) => Err(refused(err)),
        Err(err) => Err(Error::runtime(format!("{what} to {} failed: {err}", target.0))),
    }
}

fn call_with(lua: &Lua, target: AnyUserData, value: u256, function: String, args: MultiValue) -> mlua::Result<MultiValue> {
    let target = *target.borrow::<LuaAddress>()?;
    let host = lua
        .app_data_ref::<Host>()
        .map(|host| host.clone())
        .ok_or_else(|| Error::runtime("contracts.call is only available inside a call"))?;
    let args = args
        .into_iter()
        .map(value::from_lua)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| Error::runtime(format!("contracts.call: {err}")))?;
    let transfer = if value == u256::ZERO { 0 } else { cost::TRANSFER };
    charge(lua, cost::CALL + transfer)?;
    let fuel = lua.get_fuel()?;
    let outcome = host.call(&target.0, &function, &args, value, fuel);
    lua.set_fuel(fuel.saturating_sub(outcome.fuel_used))?;

    match outcome.result {
        Err(err @ VmError::Storage(_)) => Err(refused(err)),
        Err(err) => Err(Error::runtime(format!("call to {function} on {} failed: {err}", target.0))),
        ok => returned(lua, ok, "call", &target),
    }
}

#[document(
    lua,
    kind = "library",
    name = "contracts",
    category = "Chain",
    summary = "Calls other contracts, whichever engine runs them. The callee sees this contract as ctx.sender; what it writes, and any value sent with call_value, is kept only if it succeeds.",
    note = "A call costs 1000 fuel on top of what the callee burns, 1000 more when it sends value, which comes out of this call's fuel. Calls nest at most 8 deep. A contract already running further up the chain can only be entered again through a function marked with a --@reentrant line right above its definition; anything else fails with an error.",
    example = "local ok, total = pcall(contracts.call, token, \"balance_of\", ctx.sender)\nif not ok then error(\"token call failed: \" .. total) end",
)]
fn contracts_lib() {}

#[document(
    lua,
    parent = "contracts",
    name = "call",
    summary = "Calls an exported function on another contract and returns what it returns. Raises an error when the call fails; catch it with pcall to carry on.",
    params = {
        address = { ty = "Address", desc = "The contract to call." },
        name = { ty = "string", desc = "The exported function's name." },
        args = { ty = "...", desc = "The arguments: anything a contract function can return.", optional = true },
    },
    returns = { ty = "...", desc = "The callee's return values." },
    see_also = "ctx.sender",
)]
fn contracts_call() {}

#[document(
    lua,
    parent = "contracts",
    name = "call_value",
    summary = "Like call, but also sends funds from this contract's balance, which the callee sees in ctx.value. The funds go back if the call fails.",
    params = {
        address = { ty = "Address", desc = "The contract to call." },
        amount = { ty = "U256 | number", desc = "How much to send." },
        name = { ty = "string", desc = "The exported function's name." },
        args = { ty = "...", desc = "The arguments.", optional = true },
    },
    returns = { ty = "...", desc = "The callee's return values." },
    see_also = "accounts.transfer",
)]
fn contracts_call_value() {}

#[document(
    lua,
    parent = "contracts",
    name = "delegate",
    summary = "Runs another contract's function as this contract's own: on this contract's storage and balance, with ctx.sender, ctx.contract and ctx.value unchanged, and events in this contract's name. The EVM's delegatecall, for proxies. Raises an error when it fails, and nothing it did is kept.",
    params = {
        address = { ty = "Address", desc = "The contract whose code to run." },
        name = { ty = "string | U256", desc = "The function: its name, or its selector, as fallback receives it." },
        args = { ty = "...", desc = "The arguments.", optional = true },
    },
    returns = { ty = "...", desc = "The function's return values." },
    note = "Costs 1000 fuel on top of what the code burns. Only functions the target exports can be delegated to, never its fallback or a hook. The code has full access to this contract, so delegate only to code you trust, and keep storage layouts in agreement.",
    see_also = "contracts.selector",
)]
fn contracts_delegate() {}

#[document(
    lua,
    parent = "contracts",
    name = "selector",
    summary = "The selector a call names a function by: XXH3-64 of the name. What fallback receives, for comparing.",
    params = { name = { ty = "string", desc = "The function's name." } },
    returns = { ty = "U256", desc = "The selector." },
    example = "function M.fallback(selector, ...)\n    if selector == contracts.selector(\"admin_only\") then error(\"no\") end\n    return contracts.delegate(implementation(), selector, ...)\nend",
)]
fn contracts_selector() {}

pub fn register_contracts_lib(lua: &Lua) {
    let contracts_table = lua
        .create_table()
        .expect("[contracts] Failed to create contracts table");

    contracts_table
        .set(
            "call",
            lua.create_function(call)
                .expect("[contracts] Failed to create call"),
        )
        .expect("[contracts] Failed to set call");
    contracts_table
        .set(
            "call_value",
            lua.create_function(call_value)
                .expect("[contracts] Failed to create call_value"),
        )
        .expect("[contracts] Failed to set call_value");
    contracts_table
        .set(
            "delegate",
            lua.create_function(delegate)
                .expect("[contracts] Failed to create delegate"),
        )
        .expect("[contracts] Failed to set delegate");
    contracts_table
        .set(
            "selector",
            lua.create_function(selector_of)
                .expect("[contracts] Failed to create selector"),
        )
        .expect("[contracts] Failed to set selector");

    lua.globals()
        .set("contracts", contracts_table)
        .expect("[contracts] Failed to set contracts table");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::structs::address::Address;

    fn setup() -> Lua {
        let lua = Lua::new();
        lua.set_fuel(u64::MAX).unwrap();
        super::super::register_address_lib(&lua);
        register_contracts_lib(&lua);

        lua
    }

    #[test]
    fn outside_a_call_there_is_nothing_to_call_from() {
        let lua = setup();
        let target = LuaAddress(Address::Contract([1; 32]));
        lua.globals().set("target", target).unwrap();
        let err = lua.load("contracts.call(target, 'f')").exec().unwrap_err();

        assert!(err.to_string().contains("only available inside a call"), "{err}");
    }
}
