use {
    crate::vm::VmError,
    super::libs::address::LuaAddress,
    super::libs::integers::{Int, int_of},
    super::libs::values::is_list,
    crate::structs::amount::Amount,
    crate::structs::tx::TxCallArgumentType,
    crate::structs::u256::u256,
    mlua::{IntoLua, Lua, Value},
};

/// The largest integer a Lua number holds exactly. Numbers cross a contract's edges whole and
/// within it either way: Luau has floats inside a script, but none get in or out.
const MAX_EXACT: i128 = 1 << 53;

/// Integers arrive as their own types, U64 to I256, and amounts as U256, so no value is ever
/// rounded. Only the number type arrives as a plain number, and must fit one exactly.
pub(super) fn to_lua(lua: &Lua, arg: &TxCallArgumentType) -> Result<Value, VmError> {
    let value = match arg {
        TxCallArgumentType::Address(address) => {
            Value::UserData(lua.create_userdata(LuaAddress(*address))?)
        }
        TxCallArgumentType::Amount(Amount::U128(amount)) => Int::U256(u256::from(*amount)).into_lua(lua)?,
        TxCallArgumentType::Amount(Amount::U256(amount)) => Int::U256(*amount).into_lua(lua)?,
        TxCallArgumentType::Bytes(bytes) => Value::String(lua.create_string(bytes)?),
        TxCallArgumentType::String(text) => Value::String(lua.create_string(text)?),
        TxCallArgumentType::Bool(flag) => Value::Boolean(*flag),
        TxCallArgumentType::Number(number) => Value::Number(whole(*number).ok_or_else(|| {
            VmError::BadArgument(format!("{number}: a Lua contract takes whole numbers, within 2^53 either way"))
        })? as f64),
        TxCallArgumentType::I64(number) => Int::I64(*number).into_lua(lua)?,
        TxCallArgumentType::I128(number) => Int::I128(*number).into_lua(lua)?,
        TxCallArgumentType::I256(number) => Int::I256(*number).into_lua(lua)?,
        TxCallArgumentType::U64(number) => Int::U64(*number).into_lua(lua)?,
        TxCallArgumentType::U128(number) => Int::U128(*number).into_lua(lua)?,
        TxCallArgumentType::U256(number) => Int::U256(*number).into_lua(lua)?,
        TxCallArgumentType::Nil => Value::Nil,
    };

    Ok(value)
}

/// A whole number within 2^53 either way, as an i64; None for a fraction, inf, nan or one past it.
fn whole(number: f64) -> Option<i64> {
    (number.fract() == 0.0 && number.abs() <= MAX_EXACT as f64).then_some(number as i64)
}

fn numbers_leave_whole(number: f64) -> VmError {
    VmError::Runtime(format!("{number}: numbers leave a contract whole, within 2^53 either way, as i64"))
}

/// A string comes back as String when it is valid UTF-8 and as Bytes otherwise, and an integer as
/// its own type.
pub(super) fn from_lua(value: Value) -> Result<TxCallArgumentType, VmError> {
    if let Some(int) = int_of(&value) {
        return Ok(match int {
            Int::U64(number) => TxCallArgumentType::U64(number),
            Int::U128(number) => TxCallArgumentType::U128(number),
            Int::U256(number) => TxCallArgumentType::U256(number),
            Int::I64(number) => TxCallArgumentType::I64(number),
            Int::I128(number) => TxCallArgumentType::I128(number),
            Int::I256(number) => TxCallArgumentType::I256(number),
        });
    }

    let arg = match value {
        Value::Nil => TxCallArgumentType::Nil,
        Value::Boolean(flag) => TxCallArgumentType::Bool(flag),
        Value::Integer(number) => TxCallArgumentType::I64(whole(number as f64).ok_or_else(|| numbers_leave_whole(number as f64))?),
        Value::Number(number) => TxCallArgumentType::I64(whole(number).ok_or_else(|| numbers_leave_whole(number))?),
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
            } else if is_list(&data) {
                return Err(VmError::BadArgument("a values list goes alone, in place of every other value".to_string()));
            } else {
                return Err(VmError::BadReturn("this userdata".to_string()));
            }
        }
        other => return Err(VmError::BadReturn(other.type_name().to_string())),
    };

    Ok(arg)
}
