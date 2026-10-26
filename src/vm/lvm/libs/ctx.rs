use {
    super::address::LuaAddress,
    super::integers::LuaInt,
    super::values::LuaValues,
    crate::structs::tx::TxCallArgumentType,
    crate::vm::{CallContext, FALLBACK, Host},
    mlua::{AnyUserData, Error, Lua, UserData, UserDataFields},
    r#macro::document,
};

/// The arguments of the call running, as they arrived, and the list ctx.args gives once it is
/// made: made once, so reading ctx.args again costs nothing.
pub struct CallArgs {
    args: Vec<TxCallArgumentType>,
    list: Option<AnyUserData>,
}

impl CallArgs {
    /// The arguments a function is called with. A fallback's selector, which the chain put in
    /// front, is not among them: they are what the caller sent.
    pub fn new(function: &str, args: &[TxCallArgumentType]) -> CallArgs {
        let args = match function == FALLBACK {
            true => args.get(1..).unwrap_or_default().to_vec(),
            false => args.to_vec(),
        };

        CallArgs { args, list: None }
    }
}

/// The ctx global. It holds nothing itself: every field reads the context of the call running
/// right now.
pub struct LuaCtx;

impl UserData for LuaCtx {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_function_get("sender", |lua, _| Ok(LuaAddress(current(lua)?.sender)));
        fields.add_field_function_get("contract", |lua, _| Ok(LuaAddress(current(lua)?.contract)));
        fields.add_field_function_get("value", |lua, _| Ok(LuaInt(current(lua)?.value)));
        fields.add_field_function_get("args", |lua, _| {
            let mut call = lua
                .app_data_mut::<CallArgs>()
                .ok_or_else(|| Error::runtime("ctx is only available inside a call"))?;

            if call.list.is_none() {
                call.list = Some(lua.create_userdata(LuaValues::new(lua, &call.args)?)?);
            }

            Ok(call.list.clone())
        });
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
    summary = "The call being run: who made it, to which contract, and how much it sent. Only available inside an exported function, not while the script's top level loads. When another contract calls this one, sender is that contract and value is what it sent with contracts.call_value, or zero.",
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

#[document(
    lua,
    parent = "ctx",
    name = "args",
    kind = "field",
    summary = "The arguments the call came with, exactly as they arrived, to pass on unchanged. In a fallback they are what the caller sent, without the selector the chain put in front.",
    returns = { ty = "Values", desc = "The same list each time it is read." },
    see_also = "Values",
)]
fn ctx_args() {}

pub fn register_ctx_lib(lua: &Lua) {
    lua.globals()
        .set("ctx", LuaCtx)
        .expect("[ctx] Failed to set ctx");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::structs::hash::HashType;
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
            block: crate::vm::BlockAt::at(0),
        };
        let state = Arc::new(State::new(Arc::new(MemoryDb::default())));
        lua.set_app_data(Host::root(state, context, 1 << 20, Arc::new(Vm::new(&[], HashType::default_chain()).unwrap())));
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
