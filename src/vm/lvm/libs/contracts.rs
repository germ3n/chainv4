use {
    super::accounts::refused,
    super::address::LuaAddress,
    super::integers::{LuaInt, exact_as},
    super::values::{Given, LuaValues},
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
    call_with(lua, target, exact_as::<u256>(&value)?, function, args)
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
            exact_as::<u64>(&function).map_err(|_| Error::runtime("a selector is a whole number that fits in 64 bits"))?,
        ),
    };
    let given = Given::read(args).map_err(|err| Error::runtime(format!("contracts.delegate: {err}")))?;
    let untouched = given.untouched();
    let args = given.into_values();
    charge(lua, cost::CALL)?;
    let fuel = lua.get_fuel()?;
    let outcome = host.delegate(&target.0, id, &args, fuel);
    lua.set_fuel(fuel.saturating_sub(outcome.fuel_used))?;
    returned(lua, outcome.result, "delegate", &target, untouched)
}

fn check_layout(lua: &Lua, (from, to): (AnyUserData, AnyUserData)) -> mlua::Result<()> {
    let (from, to) = (*from.borrow::<LuaAddress>()?, *to.borrow::<LuaAddress>()?);
    let host = lua
        .app_data_ref::<Host>()
        .map(|host| host.clone())
        .ok_or_else(|| Error::runtime("contracts.check_layout is only available inside a call"))?;
    charge(lua, cost::CHECK_LAYOUT)?;

    host.check_layout(&from.0, &to.0).map_err(refused)
}

fn selector_of(_: &Lua, name: String) -> mlua::Result<LuaInt<u64>> {
    Ok(LuaInt(selector(&name)))
}

/// A call's return values as Lua values, or as one values list when it was given one, or its
/// failure as a Lua error.
fn returned(
    lua: &Lua,
    result: Result<Vec<crate::structs::tx::TxCallArgumentType>, VmError>,
    what: &str,
    target: &LuaAddress,
    untouched: bool,
) -> mlua::Result<MultiValue> {
    match result {
        Ok(returns) if untouched => {
            let list = lua.create_userdata(LuaValues::new(lua, &returns)?)?;

            Ok(MultiValue::from_vec(vec![Value::UserData(list)]))
        }
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
    let given = Given::read(args).map_err(|err| Error::runtime(format!("contracts.call: {err}")))?;
    let untouched = given.untouched();
    let args = given.into_values();
    let transfer = if value == u256::ZERO { 0 } else { cost::TRANSFER };
    charge(lua, cost::CALL + transfer)?;
    let fuel = lua.get_fuel()?;
    let outcome = host.call(&target.0, &function, &args, value, fuel);
    lua.set_fuel(fuel.saturating_sub(outcome.fuel_used))?;

    match outcome.result {
        Err(err @ VmError::Storage(_)) => Err(refused(err)),
        Err(err) => Err(Error::runtime(format!("call to {function} on {} failed: {err}", target.0))),
        ok => returned(lua, ok, "call", &target, untouched),
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
        name = { ty = "string | U64", desc = "The function: its name, or its selector, as fallback receives it." },
        args = { ty = "...", desc = "The arguments.", optional = true },
    },
    returns = { ty = "...", desc = "The function's return values." },
    note = "Costs 1000 fuel on top of what the code burns. Only functions the target exports can be delegated to, never its fallback or a hook. The code has full access to this contract but for storage.own, which it can only read, so delegate only to code you trust, and keep storage layouts in agreement: contracts.check_layout says whether new code keeps the old code's.",
    see_also = "contracts.selector, contracts.check_layout",
)]
fn contracts_delegate() {}

#[document(
    lua,
    parent = "contracts",
    name = "check_layout",
    summary = "Raises an error unless to's code keeps the storage layout of from's: every namespace from's code declares is declared by to's too, with the same kind, or retired, the rule Tx::Upgrade holds new code to. What a proxy checks before it points at a new implementation.",
    params = {
        from = { ty = "Address", desc = "The contract whose code would be replaced: a proxy's current implementation." },
        to = { ty = "Address", desc = "The contract whose code would replace it." },
    },
    returns = { ty = "nil", desc = "" },
    panics = "Errors saying which namespace breaks the layout, or when either address holds no contract.",
    note = "Costs 2000 fuel.",
    example = "function M.set_implementation(implementation)\n    local current = storage.own:get(\"implementation\")\n    if current then contracts.check_layout(address.parse(current), implementation) end\n    storage.own:set(\"implementation\", implementation:text())\nend",
    see_also = "contracts.delegate, storage.namespace",
)]
fn contracts_check_layout() {}

#[document(
    lua,
    parent = "contracts",
    name = "selector",
    summary = "The selector a call names a function by: XXH3-64 of the name. What fallback receives, for comparing.",
    params = { name = { ty = "string", desc = "The function's name." } },
    returns = { ty = "U64", desc = "The selector." },
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
            "check_layout",
            lua.create_function(check_layout)
                .expect("[contracts] Failed to create check_layout"),
        )
        .expect("[contracts] Failed to set check_layout");
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
