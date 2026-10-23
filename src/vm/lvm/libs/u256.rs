use {
    crate::structs::u256::u256,
    mlua::{Error, Lua, MetaMethod, Table, UserData, UserDataMethods, Value},
    r#macro::document,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LuaU256(pub u256);

impl UserData for LuaU256 {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("hex", |_, this, ()| Ok(format!("{:#x}", this.0)));
        methods.add_meta_method(MetaMethod::ToString, |_, this, ()| Ok(this.0.to_string()));
        methods.add_meta_function(MetaMethod::Add, |_, (a, b): (Value, Value)| {
            checked(operand(&a)?.checked_add(operand(&b)?), "u256 overflow")
        });
        methods.add_meta_function(MetaMethod::Sub, |_, (a, b): (Value, Value)| {
            checked(operand(&a)?.checked_sub(operand(&b)?), "u256 underflow")
        });
        methods.add_meta_function(MetaMethod::Mul, |_, (a, b): (Value, Value)| {
            checked(operand(&a)?.checked_mul(operand(&b)?), "u256 overflow")
        });
        methods.add_meta_function(MetaMethod::Div, |_, (a, b): (Value, Value)| {
            checked(operand(&a)?.checked_div(operand(&b)?), "division by zero")
        });
        methods.add_meta_function(MetaMethod::Mod, |_, (a, b): (Value, Value)| {
            checked(operand(&a)?.checked_rem(operand(&b)?), "division by zero")
        });
        methods.add_meta_function(MetaMethod::Pow, |_, (a, b): (Value, Value)| {
            let exponent = u32::try_from(operand(&b)?).map_err(|_| Error::runtime("u256 overflow"))?;

            checked(operand(&a)?.checked_pow(exponent), "u256 overflow")
        });
        methods.add_meta_function(MetaMethod::Eq, |_, (a, b): (Value, Value)| {
            Ok(operand(&a)? == operand(&b)?)
        });
        methods.add_meta_function(MetaMethod::Lt, |_, (a, b): (Value, Value)| {
            Ok(operand(&a)? < operand(&b)?)
        });
        methods.add_meta_function(MetaMethod::Le, |_, (a, b): (Value, Value)| {
            Ok(operand(&a)? <= operand(&b)?)
        });
        methods.add_meta_function(MetaMethod::Concat, |_, (a, b): (Value, Value)| {
            Ok(format!("{}{}", text(&a)?, text(&b)?))
        });
    }
}

fn checked(result: Option<u256>, failure: &str) -> mlua::Result<LuaU256> {
    result.map(LuaU256).ok_or_else(|| Error::runtime(failure))
}

/// Either side of an operator: another U256, or a non-negative whole number small enough to
/// be exact.
fn operand(value: &Value) -> mlua::Result<u256> {
    match value {
        Value::UserData(data) => {
            if let Ok(number) = data.borrow::<LuaU256>() {
                return Ok(number.0);
            }
        }
        Value::Integer(number) if *number >= 0 => return Ok(u256::from(*number as u64)),
        Value::Number(number) if exact_whole(*number) => return Ok(u256::from(*number as u64)),
        _ => {}
    }

    Err(Error::runtime(
        "u256 arithmetic needs a U256 or a non-negative whole number",
    ))
}

fn exact_whole(number: f64) -> bool {
    (0.0..=9_007_199_254_740_992.0).contains(&number) && number.fract() == 0.0
}

fn text(value: &Value) -> mlua::Result<String> {
    match value {
        Value::String(text) => Ok(text.to_str()?.to_string()),
        Value::Integer(number) => Ok(number.to_string()),
        Value::Number(number) => Ok(number.to_string()),
        other => Ok(operand(other)?.to_string()),
    }
}

fn new_u256(value: &Value) -> mlua::Result<LuaU256> {
    let Value::String(text) = value else {
        return operand(value).map(LuaU256);
    };
    let text = text.to_str()?;
    let parsed = match text.strip_prefix("0x") {
        Some(digits) => u256::from_str_radix(digits, 16),
        None => u256::from_str_radix(&text, 10),
    };

    parsed
        .map(LuaU256)
        .map_err(|_| Error::runtime(format!("not a u256: {}", &*text)))
}

#[document(
    lua,
    kind = "library",
    name = "u256",
    category = "Numbers",
    summary = "Makes U256 values. u256(value) is short for u256.new(value).",
    example = "local total = u256(\"1000000000000000000000\") + 5\nassert(total > u256(0))",
)]
fn u256_lib() {}

#[document(
    lua,
    parent = "u256",
    name = "new",
    summary = "Makes a U256 from a non-negative whole number, a decimal string, a 0x hex string or another U256.",
    params = {
        value = { ty = "number|string|U256", desc = "Use a string above 2^53, which a number cannot hold exactly." },
    },
    returns = { ty = "U256", desc = "The value." },
    panics = "Errors on negative, fractional or too-large numbers, and on strings that are not a number in range.",
    example = "local one_eth = u256(\"1000000000000000000\")",
)]
fn u256_new() {}

#[document(
    lua,
    parent = "u256",
    name = "zero",
    kind = "field",
    summary = "Zero.",
    returns = { ty = "U256", desc = "0" },
)]
fn u256_zero() {}

#[document(
    lua,
    parent = "u256",
    name = "max",
    kind = "field",
    summary = "The largest U256, 2^256 - 1.",
    returns = { ty = "U256", desc = "2^256 - 1" },
)]
fn u256_max() {}

#[document(
    lua,
    kind = "class",
    name = "U256",
    category = "Numbers",
    summary = "An unsigned 256-bit integer, for amounts and anything else a Lua number would round. Operators: + - * / % ^ with another U256 or a non-negative whole number, == < <= with another U256, .. and tostring. / rounds down.",
    note = "Arithmetic never wraps: overflow, underflow and division by zero raise an error. Compare against another U256, not a number: Lua never calls == between a number and a userdata, so u256(5) == 5 is false.",
    see_also = "u256.new",
)]
fn u256_class() {}

#[document(
    lua,
    parent = "U256",
    name = "hex",
    kind = "method",
    summary = "The value in hex with a 0x prefix and no leading zeros.",
    returns = { ty = "string", desc = "For example 0xff." },
)]
fn u256_hex() {}

pub fn register_u256_lib(lua: &Lua) {
    let u256_table = lua.create_table().expect("[u256] Failed to create u256 table");
    u256_table
        .set(
            "new",
            lua.create_function(|_, value: Value| new_u256(&value))
                .expect("[u256] Failed to create new"),
        )
        .expect("[u256] Failed to set new");
    u256_table
        .set("zero", LuaU256(u256::ZERO))
        .expect("[u256] Failed to set zero");
    u256_table
        .set("max", LuaU256(u256::MAX))
        .expect("[u256] Failed to set max");

    let meta = lua.create_table().expect("[u256] Failed to create metatable");
    meta.set(
        "__call",
        lua.create_function(|_, (_, value): (Table, Value)| new_u256(&value))
            .expect("[u256] Failed to create __call"),
    )
    .expect("[u256] Failed to set __call");
    u256_table.set_metatable(Some(meta));
    lua.globals()
        .set("u256", u256_table)
        .expect("[u256] Failed to set u256 table");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> Lua {
        let lua = Lua::new();
        lua.set_fuel(u64::MAX).unwrap();
        register_u256_lib(&lua);

        lua
    }

    fn eval(lua: &Lua, code: &str) -> mlua::Result<String> {
        lua.load(code).eval()
    }

    fn error(lua: &Lua, code: &str) -> String {
        lua.load(code).exec().unwrap_err().to_string()
    }

    #[test]
    fn arithmetic_is_exact_past_2_to_the_53() {
        let lua = setup();

        assert_eq!(
            eval(&lua, "return tostring(u256('9007199254740993') + 1)").unwrap(),
            "9007199254740994"
        );
        assert_eq!(eval(&lua, "return tostring(u256(2) ^ 200 / u256(2) ^ 199)").unwrap(), "2");
        assert_eq!(eval(&lua, "return tostring(u256(17) % 5)").unwrap(), "2");
        assert_eq!(eval(&lua, "return tostring(10 - u256(4))").unwrap(), "6");
        assert_eq!(eval(&lua, "return u256.max:hex()").unwrap(), format!("{:#x}", u256::MAX));
        assert_eq!(eval(&lua, "return u256('0xff'):hex()").unwrap(), "0xff");
        assert_eq!(eval(&lua, "return 'total: ' .. u256(5)").unwrap(), "total: 5");
    }

    #[test]
    fn comparisons_work_between_u256_values() {
        let lua = setup();

        assert_eq!(
            eval(&lua, "return tostring(u256(5) == u256.new(5) and u256(4) < u256(5) and u256(5) <= u256(5))").unwrap(),
            "true"
        );
        assert_eq!(eval(&lua, "return tostring(u256(5) == 5)").unwrap(), "false");
    }

    #[test]
    fn arithmetic_never_wraps() {
        let lua = setup();

        assert!(error(&lua, "local _ = u256.max + 1").contains("u256 overflow"));
        assert!(error(&lua, "local _ = u256(1) - 2").contains("u256 underflow"));
        assert!(error(&lua, "local _ = u256(1) / 0").contains("division by zero"));
        assert!(error(&lua, "local _ = u256(2) ^ 256").contains("u256 overflow"));
    }

    #[test]
    fn bad_values_are_rejected() {
        let lua = setup();

        assert!(error(&lua, "local _ = u256(-1)").contains("non-negative whole number"));
        assert!(error(&lua, "local _ = u256(1.5)").contains("non-negative whole number"));
        assert!(error(&lua, "local _ = u256(2 ^ 60)").contains("non-negative whole number"));
        assert!(error(&lua, "local _ = u256('12abc')").contains("not a u256"));
        assert!(error(&lua, "local _ = u256(1) + 'x'").contains("non-negative whole number"));
    }
}
