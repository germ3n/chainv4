use {
    crate::vm::VmError,
    super::libs::address::LuaAddress,
    super::libs::u256::LuaU256,
    crate::structs::amount::Amount,
    crate::structs::tx::TxCallArgumentType,
    crate::structs::u256::u256,
    mlua::{Lua, Value},
};

/// The largest integer a Lua number holds exactly.
const MAX_EXACT: i128 = 1 << 53;

/// Unsigned integers and amounts arrive as u256, so no value is ever rounded. Signed integers
/// arrive as plain numbers and must fit one exactly.
pub(super) fn to_lua(lua: &Lua, arg: &TxCallArgumentType) -> Result<Value, VmError> {
    let value = match arg {
        TxCallArgumentType::Address(address) => {
            Value::UserData(lua.create_userdata(LuaAddress(*address))?)
        }
        TxCallArgumentType::Amount(Amount::U128(amount)) => wide(lua, u256::from(*amount))?,
        TxCallArgumentType::Amount(Amount::U256(amount)) => wide(lua, *amount)?,
        TxCallArgumentType::Bytes(bytes) => Value::String(lua.create_string(bytes)?),
        TxCallArgumentType::String(text) => Value::String(lua.create_string(text)?),
        TxCallArgumentType::Bool(flag) => Value::Boolean(*flag),
        TxCallArgumentType::Number(number) => Value::Number(*number),
        TxCallArgumentType::I64(number) => exact(i128::from(*number))?,
        TxCallArgumentType::I128(number) => exact(*number)?,
        TxCallArgumentType::U64(number) => wide(lua, u256::from(*number))?,
        TxCallArgumentType::U128(number) => wide(lua, u256::from(*number))?,
        TxCallArgumentType::U256(number) => wide(lua, *number)?,
    };

    Ok(value)
}

fn wide(lua: &Lua, number: u256) -> Result<Value, VmError> {
    Ok(Value::UserData(lua.create_userdata(LuaU256(number))?))
}

fn exact(number: i128) -> Result<Value, VmError> {
    if !(-MAX_EXACT..=MAX_EXACT).contains(&number) {
        return Err(VmError::BadArgument(format!(
            "{number} does not fit a Lua number exactly"
        )));
    }

    Ok(Value::Number(number as f64))
}

/// A string comes back as String when it is valid UTF-8 and as Bytes otherwise.
pub(super) fn from_lua(value: Value) -> Result<TxCallArgumentType, VmError> {
    let arg = match value {
        Value::Boolean(flag) => TxCallArgumentType::Bool(flag),
        Value::Integer(number) => TxCallArgumentType::Number(f64::from(number)),
        Value::Number(number) => TxCallArgumentType::Number(number),
        Value::String(text) => {
            let bytes = text.as_bytes().to_vec();

            match String::from_utf8(bytes) {
                Ok(text) => TxCallArgumentType::String(text),
                Err(err) => TxCallArgumentType::Bytes(err.into_bytes()),
            }
        }
        Value::UserData(data) => {
            if let Ok(address) = data.borrow::<LuaAddress>() {
                TxCallArgumentType::Address(address.0)
            } else if let Ok(number) = data.borrow::<LuaU256>() {
                TxCallArgumentType::U256(number.0)
            } else {
                return Err(VmError::BadReturn("this userdata".to_string()));
            }
        }
        other => return Err(VmError::BadReturn(other.type_name().to_string())),
    };

    Ok(arg)
}
