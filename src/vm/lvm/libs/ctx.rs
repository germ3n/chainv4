use {
    super::address::LuaAddress,
    super::u256::LuaU256,
    crate::vm::{CallContext, Host},
    mlua::{Error, Lua, UserData, UserDataFields},
    r#macro::document,
};

/// The ctx global. It holds nothing itself: every field reads the context of the call running
/// right now.
pub struct LuaCtx;

impl UserData for LuaCtx {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_function_get("sender", |lua, _| Ok(LuaAddress(current(lua)?.sender)));
        fields.add_field_function_get("contract", |lua, _| Ok(LuaAddress(current(lua)?.contract)));
        fields.add_field_function_get("value", |lua, _| Ok(LuaU256(current(lua)?.value)));
    }
}

fn current(lua: &Lua) -> mlua::Result<CallContext> {
    lua.app_data_ref::<Host>()
        .map(|host| *host.context())
        .ok_or_else(|| Error::runtime("ctx is only available inside a call"))
}

#[document(
    lua,
    kind = "library",
    name = "ctx",
    category = "Chain",
    summary = "The call being run: who made it, to which contract, and how much it sent. Only available inside an exported function, not while the script's top level loads. When another contract calls this one, sender is that contract and value is zero.",
    example = "function M.deposit()\n    balances[ctx.sender:hex()] = ctx.value\nend",
)]
fn ctx_lib() {}

#[document(
    lua,
    parent = "ctx",
    name = "sender",
    kind = "field",
    summary = "The address that made the call.",
    returns = { ty = "Address", desc = "The caller." },
)]
fn ctx_sender() {}

#[document(
    lua,
    parent = "ctx",
    name = "contract",
    kind = "field",
    summary = "The address of the contract being called.",
    returns = { ty = "Address", desc = "This contract." },
)]
fn ctx_contract() {}

#[document(
    lua,
    parent = "ctx",
    name = "value",
    kind = "field",
    summary = "The amount the call sent to the contract.",
    returns = { ty = "U256", desc = "Zero when nothing was sent." },
)]
fn ctx_value() {}

pub fn register_ctx_lib(lua: &Lua) {
    lua.globals()
        .set("ctx", LuaCtx)
        .expect("[ctx] Failed to set ctx");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{MemoryDb, State};
    use crate::vm::Vm;
    use crate::structs::address::Address;
    use crate::structs::u256::u256;
    use std::sync::Arc;

    fn setup() -> Lua {
        let lua = Lua::new();
        lua.set_fuel(u64::MAX).unwrap();
        super::super::register_address_lib(&lua);
        register_ctx_lib(&lua);

        lua
    }

    #[test]
    fn fields_read_the_running_call() {
        let lua = setup();
        let context = CallContext {
            sender: Address::Ed25519([1; 32]),
            contract: Address::Ed25519([2; 32]),
            value: u256::new(7),
        };
        let state = Arc::new(State::new(Arc::new(MemoryDb::default())));
        lua.set_app_data(Host::root(state, context, 1 << 20, Arc::new(Vm::new(&[]).unwrap())));
        let code = "return ctx.sender == address.from_bytes(string.rep('\\1', 32)), ctx.contract:hex(), tostring(ctx.value)";
        let (sender, contract, value): (bool, String, String) = lua.load(code).eval().unwrap();

        assert!(sender);
        assert_eq!(contract, format!("0x{}", "02".repeat(32)));
        assert_eq!(value, "7");
    }

    #[test]
    fn outside_a_call_there_is_no_context() {
        let lua = setup();
        let err = lua.load("local _ = ctx.sender").exec().unwrap_err();

        assert!(err.to_string().contains("only available inside a call"), "{err}");
    }
}
