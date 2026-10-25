//! Typed parameters and results for #[export] functions. The macro reads each argument into its
//! parameter's type with FromValue, and turns what the function returns into return values with
//! IntoOutput.
//!
//! A call that passes too many arguments, leaves one out, or passes the wrong kind aborts with a
//! message naming the parameter, such as `amount: expected an amount`. Option parameters may be
//! left out or passed nil.

use {
    crate::{Address, I256, U256, Value, abort},
    alloc::{format, string::String, vec, vec::Vec},
    core::fmt::Display,
};

/// A type an exported function's parameter may have.
pub trait FromValue: Sized {
    /// What the argument must be, for the message a call that passes something else fails with.
    const EXPECTED: &'static str;

    /// The parameter from its argument, or None when the argument does not fit.
    fn from_value(value: &Value) -> Option<Self>;

    /// What the parameter is when the call leaves its argument out: nothing, so the call fails,
    /// except for an Option.
    fn missing() -> Option<Self> {
        None
    }
}

impl FromValue for Value {
    const EXPECTED: &'static str = "a value";

    fn from_value(value: &Value) -> Option<Value> {
        Some(value.clone())
    }
}

impl FromValue for Address {
    const EXPECTED: &'static str = "an address";

    fn from_value(value: &Value) -> Option<Address> {
        value.as_address()
    }
}

impl FromValue for U256 {
    const EXPECTED: &'static str = "an amount";

    fn from_value(value: &Value) -> Option<U256> {
        value.as_u256()
    }
}

impl FromValue for I256 {
    const EXPECTED: &'static str = "a whole number";

    fn from_value(value: &Value) -> Option<I256> {
        value.as_i256()
    }
}

impl FromValue for bool {
    const EXPECTED: &'static str = "true or false";

    fn from_value(value: &Value) -> Option<bool> {
        value.as_bool()
    }
}

impl FromValue for String {
    const EXPECTED: &'static str = "a string";

    fn from_value(value: &Value) -> Option<String> {
        value.as_str().map(String::from)
    }
}

impl FromValue for Vec<u8> {
    const EXPECTED: &'static str = "bytes";

    fn from_value(value: &Value) -> Option<Vec<u8>> {
        value.as_bytes().map(<[u8]>::to_vec)
    }
}

impl FromValue for i64 {
    const EXPECTED: &'static str = "a whole number";

    fn from_value(value: &Value) -> Option<i64> {
        value.as_i64()
    }
}

impl FromValue for u64 {
    const EXPECTED: &'static str = "a whole number from 0";

    fn from_value(value: &Value) -> Option<u64> {
        match *value {
            Value::U64(number) => Some(number),
            _ => u64::try_from(value.as_u128()?).ok(),
        }
    }
}

impl FromValue for u32 {
    const EXPECTED: &'static str = "a whole number from 0 to 4294967295";

    fn from_value(value: &Value) -> Option<u32> {
        u32::try_from(u64::from_value(value)?).ok()
    }
}

impl FromValue for u8 {
    const EXPECTED: &'static str = "a whole number from 0 to 255";

    fn from_value(value: &Value) -> Option<u8> {
        u8::try_from(u64::from_value(value)?).ok()
    }
}

impl FromValue for u128 {
    const EXPECTED: &'static str = "a whole number from 0";

    fn from_value(value: &Value) -> Option<u128> {
        value.as_u128()
    }
}

impl FromValue for i128 {
    const EXPECTED: &'static str = "a whole number";

    fn from_value(value: &Value) -> Option<i128> {
        match *value {
            Value::I128(number) => Some(number),
            _ => match value.as_i64() {
                Some(number) => Some(i128::from(number)),
                None => i128::try_from(value.as_u128()?).ok(),
            },
        }
    }
}

impl<T: FromValue> FromValue for Option<T> {
    const EXPECTED: &'static str = T::EXPECTED;

    fn from_value(value: &Value) -> Option<Option<T>> {
        match value {
            Value::Nil => Some(None),
            value => T::from_value(value).map(Some),
        }
    }

    fn missing() -> Option<Option<T>> {
        Some(None)
    }
}

/// One value an exported function may return.
pub trait IntoValue {
    fn into_value(self) -> Value;
}

impl IntoValue for Value {
    fn into_value(self) -> Value {
        self
    }
}

impl IntoValue for Address {
    fn into_value(self) -> Value {
        Value::Address(self)
    }
}

impl IntoValue for U256 {
    fn into_value(self) -> Value {
        self.into()
    }
}

impl IntoValue for I256 {
    fn into_value(self) -> Value {
        self.into()
    }
}

impl IntoValue for bool {
    fn into_value(self) -> Value {
        Value::Bool(self)
    }
}

impl IntoValue for String {
    fn into_value(self) -> Value {
        Value::String(self)
    }
}

impl IntoValue for &str {
    fn into_value(self) -> Value {
        Value::String(String::from(self))
    }
}

impl IntoValue for Vec<u8> {
    fn into_value(self) -> Value {
        Value::Bytes(self)
    }
}

impl IntoValue for i64 {
    fn into_value(self) -> Value {
        Value::I64(self)
    }
}

impl IntoValue for u64 {
    fn into_value(self) -> Value {
        Value::U64(self)
    }
}

impl IntoValue for u32 {
    fn into_value(self) -> Value {
        Value::U64(u64::from(self))
    }
}

impl IntoValue for u8 {
    fn into_value(self) -> Value {
        Value::U64(u64::from(self))
    }
}

impl IntoValue for i128 {
    fn into_value(self) -> Value {
        Value::I128(self)
    }
}

impl IntoValue for u128 {
    fn into_value(self) -> Value {
        Value::U128(self)
    }
}

/// None returns nil.
impl<T: IntoValue> IntoValue for Option<T> {
    fn into_value(self) -> Value {
        self.map_or(Value::Nil, IntoValue::into_value)
    }
}

/// What an exported function may return: nothing, one value, several, or a failure.
pub trait IntoOutput {
    fn into_output(self) -> Vec<Value>;
}

impl IntoOutput for () {
    fn into_output(self) -> Vec<Value> {
        Vec::new()
    }
}

impl<T: IntoValue> IntoOutput for T {
    fn into_output(self) -> Vec<Value> {
        vec![self.into_value()]
    }
}

/// Any number of values, as they are.
impl IntoOutput for Vec<Value> {
    fn into_output(self) -> Vec<Value> {
        self
    }
}

impl<A: IntoValue, B: IntoValue> IntoOutput for (A, B) {
    fn into_output(self) -> Vec<Value> {
        vec![self.0.into_value(), self.1.into_value()]
    }
}

impl<A: IntoValue, B: IntoValue, C: IntoValue> IntoOutput for (A, B, C) {
    fn into_output(self) -> Vec<Value> {
        vec![self.0.into_value(), self.1.into_value(), self.2.into_value()]
    }
}

impl<A: IntoValue, B: IntoValue, C: IntoValue, D: IntoValue> IntoOutput for (A, B, C, D) {
    fn into_output(self) -> Vec<Value> {
        vec![self.0.into_value(), self.1.into_value(), self.2.into_value(), self.3.into_value()]
    }
}

/// Err aborts the call with its message, so nothing it wrote is kept.
impl<T: IntoOutput, E: Display> IntoOutput for Result<T, E> {
    fn into_output(self) -> Vec<Value> {
        match self {
            Ok(value) => value.into_output(),
            Err(err) => abort(&format!("{err}")),
        }
    }
}

/// A call's arguments, as an exported function's parameters take them in order. Used by the code
/// #[export] writes; not meant to be named.
#[doc(hidden)]
pub struct Args<'a> {
    values: &'a [Value],
    next: usize,
}

impl<'a> Args<'a> {
    /// The arguments for a function with this many parameters; more than that aborts the call.
    pub fn new(values: &'a [Value], function: &str, parameters: usize) -> Args<'a> {
        if values.len() > parameters {
            abort(&format!("{function} takes {parameters} arguments, not {}", values.len()));
        }

        Args { values, next: 0 }
    }

    /// The next parameter, read from its argument, or the call aborts saying which and why.
    pub fn take<T: FromValue>(&mut self, name: &str) -> T {
        let value = self.values.get(self.next);
        self.next += 1;

        match value {
            Some(value) => T::from_value(value).unwrap_or_else(|| abort(&format!("{name}: expected {}", T::EXPECTED))),
            None => T::missing().unwrap_or_else(|| abort(&format!("{name}: missing"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_read_into_their_types() {
        let address = Address {
            kind: crate::AddressKind::Ed25519,
            bytes: [7; 32],
        };
        // Lua passes its numbers as floats' bits.
        let three = Value::Number(3.0f64.to_bits());

        assert_eq!(Address::from_value(&Value::Address(address)), Some(address));
        assert_eq!(U256::from_value(&three), Some(U256::from(3u64)));
        assert_eq!(u64::from_value(&Value::U64(u64::MAX)), Some(u64::MAX));
        assert_eq!(u8::from_value(&Value::I64(256)), None);
        assert_eq!(i128::from_value(&Value::I64(-5)), Some(-5));
        assert_eq!(String::from_value(&Value::Bool(true)), None);
        assert_eq!(Option::<u64>::from_value(&Value::Nil), Some(None));
        assert_eq!(Option::<u64>::from_value(&three), Some(Some(3)));
        assert_eq!(Option::<u64>::missing(), Some(None));
        assert_eq!(u64::missing(), None);
    }

    #[test]
    fn results_become_return_values() {
        assert!(().into_output().is_empty());
        assert_eq!(true.into_output(), [Value::Bool(true)]);
        assert_eq!(("a", 5u64).into_output(), [Value::String("a".into()), Value::U64(5)]);
        assert_eq!(Option::<u64>::None.into_output(), [Value::Nil]);
        assert_eq!(Ok::<_, &str>(vec![Value::I64(1), Value::I64(2)]).into_output(), [Value::I64(1), Value::I64(2)]);
    }
}
