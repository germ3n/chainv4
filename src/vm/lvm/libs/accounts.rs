use {
    super::address::LuaAddress,
    super::integers::{LuaInt, exact_as},
    crate::structs::u256::u256,
    crate::vm::lvm::{charge, costs},
    crate::vm::{Host, VmError},
    mlua::{AnyUserData, Error, Lua, Value},
    r#macro::document,
};

fn host(lua: &Lua) -> mlua::Result<Host> {
    lua.app_data_ref::<Host>()
        .map(|host| host.clone())
        .ok_or_else(|| Error::runtime("accounts is only available inside a call"))
}

/// A database failure is already recorded in the Host, so the script cannot hide it with pcall.
pub(super) fn refused(err: VmError) -> Error {
    match err {
        VmError::Storage(err) => Error::external(err),
        other => Error::runtime(other.to_string()),
    }
}

fn balance(lua: &Lua, address: AnyUserData) -> mlua::Result<LuaInt<u256>> {
    let address = *address.borrow::<LuaAddress>()?;
    let host = host(lua)?;
    charge(lua, costs(lua).balance)?;

    host.balance(&address.0).map(LuaInt).map_err(refused)
}

fn transfer(lua: &Lua, (to, amount): (AnyUserData, Value)) -> mlua::Result<()> {
    let to = *to.borrow::<LuaAddress>()?;
    let amount = exact_as::<u256>(&amount)?;
    let host = host(lua)?;
    charge(lua, costs(lua).transfer)?;

    host.transfer(&to.0, amount).map_err(refused)
}

#[document(
    lua,
    kind = "library",
    name = "accounts",
    category = "Chain",
    summary = "Balances, and sending funds out of this contract's own balance. A call that fails sends nothing.",
    note = "Reading a balance costs 100 fuel and a transfer 1000. Funds arrive with a transaction's contract call or another contract's contracts.call_value, and show in ctx.value.",
    example = "function M.withdraw(amount)\n    assert(ctx.sender == owner, \"only the owner\")\n    accounts.transfer(ctx.sender, u256(amount))\nend",
    see_also = "ctx.value, contracts.call_value",
)]
fn accounts_lib() {}

#[document(
    lua,
    parent = "accounts",
    name = "balance",
    summary = "What an address holds.",
    params = { address = { ty = "Address", desc = "Any address, this contract's included." } },
    returns = { ty = "U256", desc = "The balance." },
)]
fn accounts_balance() {}

#[document(
    lua,
    parent = "accounts",
    name = "transfer",
    summary = "Sends funds from this contract to an address. Raises an error when the contract holds less.",
    params = {
        to = { ty = "Address", desc = "Who receives them." },
        amount = { ty = "U256 | number", desc = "How much: a U256, or a non-negative whole number." },
    },
    returns = { ty = "nil", desc = "" },
)]
fn accounts_transfer() {}

pub fn register_accounts_lib(lua: &Lua) {
    let accounts_table = lua
        .create_table()
        .expect("[accounts] Failed to create accounts table");

    accounts_table
        .set(
            "balance",
            lua.create_function(balance)
                .expect("[accounts] Failed to create balance"),
        )
        .expect("[accounts] Failed to set balance");
    accounts_table
        .set(
            "transfer",
            lua.create_function(transfer)
                .expect("[accounts] Failed to create transfer"),
        )
        .expect("[accounts] Failed to set transfer");

    lua.globals()
        .set("accounts", accounts_table)
        .expect("[accounts] Failed to set accounts table");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outside_a_call_there_are_no_funds_to_touch() {
        let lua = Lua::new();
        lua.set_fuel(u64::MAX).unwrap();
        super::super::register_address_lib(&lua);
        register_accounts_lib(&lua);
        let err = lua
            .load("accounts.balance(address.from_bytes(string.rep('\\1', 32)))")
            .exec()
            .unwrap_err();

        assert!(err.to_string().contains("only available inside a call"), "{err}");
    }
}
