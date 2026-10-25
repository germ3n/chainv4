//! Values exactly as they crossed into a contract, or came back from another one. Lua has no
//! i64 or bytes of its own, so values converted into Lua and back can change type: an i64 comes
//! back a number, bytes that read as text come back a string. A values list keeps them encoded,
//! so passing it on changes nothing. That is what proxies and routers need.
//!
//! The encoding lives in a Lua string, so a list counts against the call's memory like any other
//! value.

use {
    crate::structs::tx::TxCallArgumentType,
    crate::vm::VmError,
    crate::vm::lvm::value,
    mlua::{AnyUserData, Error, Lua, MetaMethod, MultiValue, UserData, UserDataMethods, Value},
    r#macro::document,
};

/// A values list: its values encoded as the chain encodes them, and how many there are.
pub struct LuaValues {
    encoded: mlua::String,
    len: usize,
}

impl LuaValues {
    /// A list of these values, untouched.
    pub fn new(lua: &Lua, values: &[TxCallArgumentType]) -> mlua::Result<LuaValues> {
        let bytes = wincode::serialize(values).map_err(|err| Error::runtime(err.to_string()))?;

        Ok(LuaValues {
            encoded: lua.create_string(&bytes)?,
            len: values.len(),
        })
    }

    /// The values, as they were.
    pub fn values(&self) -> Result<Vec<TxCallArgumentType>, VmError> {
        wincode::deserialize(&self.encoded.as_bytes()).map_err(|err| VmError::BadArgument(err.to_string()))
    }
}

impl UserData for LuaValues {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("unpack", |lua, this, ()| {
            this.values()
                .and_then(|values| values.iter().map(|value| value::to_lua(lua, value)).collect::<Result<Vec<_>, _>>())
                .map(MultiValue::from_vec)
                .map_err(|err| Error::runtime(format!("values:unpack: {err}")))
        });
        methods.add_method("type", |_, this, index: usize| {
            let values = this.values().map_err(|err| Error::runtime(err.to_string()))?;

            Ok(index.checked_sub(1).and_then(|index| values.get(index)).map(type_name))
        });
        methods.add_meta_method(MetaMethod::Len, |_, this, ()| Ok(this.len));
        methods.add_meta_method(MetaMethod::ToString, |_, this, ()| Ok(format!("values({})", this.len)));
    }
}

/// A value's type, as the API names it.
fn type_name(value: &TxCallArgumentType) -> &'static str {
    match value {
        TxCallArgumentType::Address(_) => "address",
        TxCallArgumentType::Amount(_) => "amount",
        TxCallArgumentType::Bytes(_) => "bytes",
        TxCallArgumentType::Bool(_) => "bool",
        TxCallArgumentType::String(_) => "string",
        TxCallArgumentType::Number(_) => "number",
        TxCallArgumentType::I64(_) => "i64",
        TxCallArgumentType::U64(_) => "u64",
        TxCallArgumentType::I128(_) => "i128",
        TxCallArgumentType::U128(_) => "u128",
        TxCallArgumentType::U256(_) => "u256",
        TxCallArgumentType::I256(_) => "i256",
        TxCallArgumentType::Nil => "nil",
    }
}

/// What values a contract gave for a call, a delegate, an event or its own results: loose Lua
/// values, converted, or one values list, passed on untouched.
pub(crate) enum Given {
    Loose(Vec<TxCallArgumentType>),
    Untouched(Vec<TxCallArgumentType>),
}

impl Given {
    /// Reads them, refusing a values list among other values: it goes alone.
    pub(crate) fn read(values: MultiValue) -> Result<Given, VmError> {
        if values.len() == 1
            && let Some(Value::UserData(data)) = values.front()
            && let Ok(list) = data.borrow::<LuaValues>()
        {
            return list.values().map(Given::Untouched);
        }

        values.into_iter().map(value::from_lua).collect::<Result<_, _>>().map(Given::Loose)
    }

    /// Whether they were a values list.
    pub(crate) fn untouched(&self) -> bool {
        matches!(self, Given::Untouched(_))
    }

    pub(crate) fn into_values(self) -> Vec<TxCallArgumentType> {
        match self {
            Given::Loose(values) | Given::Untouched(values) => values,
        }
    }
}

/// Whether a Lua value is a values list.
pub(crate) fn is_list(data: &AnyUserData) -> bool {
    data.borrow::<LuaValues>().is_ok()
}

#[document(
    lua,
    kind = "class",
    name = "Values",
    category = "Chain",
    summary = "Values exactly as they arrived: a call's arguments from ctx.args, or what a call or delegate given a values list returned. They stay encoded, so passing them on to contracts.call, contracts.call_value, contracts.delegate or events.emit, or returning them, changes nothing: an i64 stays an i64, bytes stay bytes. Lua values converted on the way would not, as Lua has no i64 or bytes of its own.",
    note = "A values list goes alone, in place of every other argument or result. #list is how many values it holds, and tostring names it.",
    example = "-- A proxy that passes every call on exactly as it came, and its results back.\nfunction M.fallback(selector)\n    return contracts.delegate(implementation(), selector, ctx.args)\nend",
    see_also = "ctx.args",
)]
fn values_class() {}

#[document(
    lua,
    parent = "Values",
    name = "unpack",
    kind = "method",
    summary = "The values as Lua values, converted as arguments are: integers wider than a Lua number as U256, i64 and i128 as numbers, bytes and strings as strings.",
    returns = { ty = "...", desc = "One Lua value for each." },
)]
fn values_unpack() {}

#[document(
    lua,
    parent = "Values",
    name = "type",
    kind = "method",
    summary = "What type a value has, as the API names it: address, amount, bytes, bool, string, number, i64, u64, i128, u128, i256, u256 or nil.",
    params = { index = "Which value, counting from 1." },
    returns = { ty = "string?", desc = "Its type, or nil past the end." },
)]
fn values_type() {}
