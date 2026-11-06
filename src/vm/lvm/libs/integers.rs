//! The integer types: U64, U128, U256, I64, I128 and I256, for whole numbers a Lua number would
//! round, or that should overflow where Rust's would. Each width is its own Lua type, holding only
//! what it needs; they meet through Int, and an Exact sign and magnitude that holds every value of
//! every one of them.

use {
    crate::structs::u256::{i256, u256},
    mlua::{AnyUserData, Error, IntoLua, Lua, MetaMethod, Table, UserData, UserDataFields, UserDataMethods, Value},
    r#macro::document,
    std::{cmp::Ordering, fmt},
};

/// 2^53: past it a number no longer holds every whole value.
// The largest whole number every step of arithmetic keeps exact, as the Luau fork's VM has it:
// past it a sum or product may already have rounded.
const MAX_EXACT: u64 = (1 << 53) - 1;

/// A value of one of the integer types, as a script holds it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LuaInt<T>(pub T);

/// The integer types, unsigned then signed, each narrowest first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntKind {
    U64,
    U128,
    U256,
    I64,
    I128,
    I256,
}

impl IntKind {
    pub const ALL: [IntKind; 6] = [IntKind::U64, IntKind::U128, IntKind::U256, IntKind::I64, IntKind::I128, IntKind::I256];

    pub fn signed(self) -> bool {
        matches!(self, IntKind::I64 | IntKind::I128 | IntKind::I256)
    }

    pub fn bits(self) -> u32 {
        match self {
            IntKind::U64 | IntKind::I64 => 64,
            IntKind::U128 | IntKind::I128 => 128,
            IntKind::U256 | IntKind::I256 => 256,
        }
    }

    /// The type's name, as typeof gives it: U256.
    pub fn name(self) -> &'static str {
        match self {
            IntKind::U64 => "U64",
            IntKind::U128 => "U128",
            IntKind::U256 => "U256",
            IntKind::I64 => "I64",
            IntKind::I128 => "I128",
            IntKind::I256 => "I256",
        }
    }

    /// Its library's name, which makes one: u256.
    pub fn library(self) -> &'static str {
        match self {
            IntKind::U64 => "u64",
            IntKind::U128 => "u128",
            IntKind::U256 => "u256",
            IntKind::I64 => "i64",
            IntKind::I128 => "i128",
            IntKind::I256 => "i256",
        }
    }

    /// "a U256", "an I64".
    fn article(self) -> String {
        format!("{} {}", self.a(), self.name())
    }

    /// "a u256", "an i64", as an error names what a value is not.
    fn library_article(self) -> String {
        format!("{} {}", self.a(), self.library())
    }

    fn a(self) -> &'static str {
        if self.signed() { "an" } else { "a" }
    }
}

/// Any integer type's value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Int {
    U64(u64),
    U128(u128),
    U256(u256),
    I64(i64),
    I128(i128),
    I256(i256),
}

impl Int {
    pub fn kind(self) -> IntKind {
        match self {
            Int::U64(_) => IntKind::U64,
            Int::U128(_) => IntKind::U128,
            Int::U256(_) => IntKind::U256,
            Int::I64(_) => IntKind::I64,
            Int::I128(_) => IntKind::I128,
            Int::I256(_) => IntKind::I256,
        }
    }

    fn exact(self) -> Exact {
        match self {
            Int::U64(value) => Exact::new(false, u256::from(value)),
            Int::U128(value) => Exact::new(false, u256::from(value)),
            Int::U256(value) => Exact::new(false, value),
            Int::I64(value) => Exact::new(value < 0, u256::from(value.unsigned_abs())),
            Int::I128(value) => Exact::new(value < 0, u256::from(value.unsigned_abs())),
            Int::I256(value) => Exact::new(value < 0, value.unsigned_abs()),
        }
    }

    /// The same value as another type, if that type holds it.
    pub fn to(self, kind: IntKind) -> Option<Int> {
        self.exact().to(kind)
    }
}

impl fmt::Display for Int {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Int::U64(value) => value.fmt(f),
            Int::U128(value) => value.fmt(f),
            Int::U256(value) => value.fmt(f),
            Int::I64(value) => value.fmt(f),
            Int::I128(value) => value.fmt(f),
            Int::I256(value) => value.fmt(f),
        }
    }
}

impl IntoLua for Int {
    fn into_lua(self, lua: &Lua) -> mlua::Result<Value> {
        let data = match self {
            Int::U64(value) => lua.create_userdata(LuaInt(value))?,
            Int::U128(value) => lua.create_userdata(LuaInt(value))?,
            Int::U256(value) => lua.create_userdata(LuaInt(value))?,
            Int::I64(value) => lua.create_userdata(LuaInt(value))?,
            Int::I128(value) => lua.create_userdata(LuaInt(value))?,
            Int::I256(value) => lua.create_userdata(LuaInt(value))?,
        };

        Ok(Value::UserData(data))
    }
}

/// A whole value as a sign and a magnitude, which every integer type's values fit, -2^255 too.
/// Zero is never negative, so each value has one Exact.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Exact {
    negative: bool,
    magnitude: u256,
}

impl Exact {
    fn new(negative: bool, magnitude: u256) -> Exact {
        Exact { negative: negative && magnitude != u256::ZERO, magnitude }
    }

    fn to(self, kind: IntKind) -> Option<Int> {
        let bits = kind.bits();
        let fits = if kind.signed() {
            let half = u256::ONE << (bits - 1);

            if self.negative { self.magnitude <= half } else { self.magnitude < half }
        } else {
            !self.negative && (bits == 256 || self.magnitude >> bits == u256::ZERO)
        };

        if !fits {
            return None;
        }

        // two's complement: -2^255's magnitude, as an i256, is already -2^255
        let signed = if self.negative { self.magnitude.as_i256().wrapping_neg() } else { self.magnitude.as_i256() };

        Some(match kind {
            IntKind::U64 => Int::U64(self.magnitude.as_u64()),
            IntKind::U128 => Int::U128(self.magnitude.as_u128()),
            IntKind::U256 => Int::U256(self.magnitude),
            IntKind::I64 => Int::I64(signed.as_i64()),
            IntKind::I128 => Int::I128(signed.as_i128()),
            IntKind::I256 => Int::I256(signed),
        })
    }
}

impl Ord for Exact {
    fn cmp(&self, other: &Exact) -> Ordering {
        match (self.negative, other.negative) {
            (false, false) => self.magnitude.cmp(&other.magnitude),
            (true, true) => other.magnitude.cmp(&self.magnitude),
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
        }
    }
}

impl PartialOrd for Exact {
    fn partial_cmp(&self, other: &Exact) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// What every integer type does, under the names Rust gives it. Unsigned types negate only 0.
pub trait Prim: Copy + Ord + fmt::Display + Send + Sync + 'static {
    const KIND: IntKind;
    const ZERO: Self;
    const MIN: Self;
    const MAX: Self;

    fn int(self) -> Int;
    /// From an Int of this type's kind.
    fn of(int: Int) -> Self;
    fn checked_add(self, other: Self) -> Option<Self>;
    fn checked_sub(self, other: Self) -> Option<Self>;
    fn checked_mul(self, other: Self) -> Option<Self>;
    fn checked_div(self, other: Self) -> Option<Self>;
    fn checked_rem(self, other: Self) -> Option<Self>;
    fn checked_pow(self, exponent: u32) -> Option<Self>;
    fn checked_neg(self) -> Option<Self>;
    fn checked_abs(self) -> Option<Self>;
    /// Division rounding down, as Lua's // does, rather than toward zero.
    fn floor_div(self, other: Self) -> Option<Self>;
    fn saturating_add(self, other: Self) -> Self;
    fn saturating_sub(self, other: Self) -> Self;
    fn saturating_mul(self, other: Self) -> Self;
    fn saturating_pow(self, exponent: u32) -> Self;
    fn wrapping_add(self, other: Self) -> Self;
    fn wrapping_sub(self, other: Self) -> Self;
    fn wrapping_mul(self, other: Self) -> Self;
    fn wrapping_pow(self, exponent: u32) -> Self;
    fn wrapping_neg(self) -> Self;
}

macro_rules! prim {
    ($ty:ty, $kind:ident, $zero:expr, $one:expr, $signed:tt) => {
        impl Prim for $ty {
            const KIND: IntKind = IntKind::$kind;
            const ZERO: $ty = $zero;
            const MIN: $ty = <$ty>::MIN;
            const MAX: $ty = <$ty>::MAX;

            fn int(self) -> Int {
                Int::$kind(self)
            }

            fn of(int: Int) -> $ty {
                match int {
                    Int::$kind(value) => value,
                    other => unreachable!("{other:?} is not a {}", stringify!($kind)),
                }
            }

            fn checked_add(self, other: $ty) -> Option<$ty> {
                <$ty>::checked_add(self, other)
            }

            fn checked_sub(self, other: $ty) -> Option<$ty> {
                <$ty>::checked_sub(self, other)
            }

            fn checked_mul(self, other: $ty) -> Option<$ty> {
                <$ty>::checked_mul(self, other)
            }

            fn checked_div(self, other: $ty) -> Option<$ty> {
                <$ty>::checked_div(self, other)
            }

            fn checked_rem(self, other: $ty) -> Option<$ty> {
                <$ty>::checked_rem(self, other)
            }

            fn checked_pow(self, exponent: u32) -> Option<$ty> {
                <$ty>::checked_pow(self, exponent)
            }

            fn saturating_add(self, other: $ty) -> $ty {
                <$ty>::saturating_add(self, other)
            }

            fn saturating_sub(self, other: $ty) -> $ty {
                <$ty>::saturating_sub(self, other)
            }

            fn saturating_mul(self, other: $ty) -> $ty {
                <$ty>::saturating_mul(self, other)
            }

            fn saturating_pow(self, exponent: u32) -> $ty {
                <$ty>::saturating_pow(self, exponent)
            }

            fn wrapping_add(self, other: $ty) -> $ty {
                <$ty>::wrapping_add(self, other)
            }

            fn wrapping_sub(self, other: $ty) -> $ty {
                <$ty>::wrapping_sub(self, other)
            }

            fn wrapping_mul(self, other: $ty) -> $ty {
                <$ty>::wrapping_mul(self, other)
            }

            fn wrapping_pow(self, exponent: u32) -> $ty {
                <$ty>::wrapping_pow(self, exponent)
            }

            fn wrapping_neg(self) -> $ty {
                <$ty>::wrapping_neg(self)
            }

            prim!(@signed $ty, $zero, $one, $signed);
        }
    };
    (@signed $ty:ty, $zero:expr, $one:expr, unsigned) => {
        fn checked_neg(self) -> Option<$ty> {
            (self == $zero).then_some(self)
        }

        fn checked_abs(self) -> Option<$ty> {
            Some(self)
        }

        fn floor_div(self, other: $ty) -> Option<$ty> {
            <$ty>::checked_div(self, other)
        }
    };
    (@signed $ty:ty, $zero:expr, $one:expr, signed) => {
        fn checked_neg(self) -> Option<$ty> {
            <$ty>::checked_neg(self)
        }

        fn checked_abs(self) -> Option<$ty> {
            <$ty>::checked_abs(self)
        }

        fn floor_div(self, other: $ty) -> Option<$ty> {
            let quotient = <$ty>::checked_div(self, other)?;
            let remainder = <$ty>::checked_rem(self, other)?;

            // toward zero rounded up a quotient below zero: take it one further down
            if remainder != $zero && ((remainder < $zero) != (other < $zero)) {
                quotient.checked_sub($one)
            } else {
                Some(quotient)
            }
        }
    };
}

prim!(u64, U64, 0, 1, unsigned);
prim!(u128, U128, 0, 1, unsigned);
prim!(u256, U256, u256::ZERO, u256::ONE, unsigned);
prim!(i64, I64, 0, 1, signed);
prim!(i128, I128, 0, 1, signed);
prim!(i256, I256, i256::ZERO, i256::ONE, signed);

/// Runs `$body` with `$x` and `$y` as one type's values: both sides of an operator, already made
/// the same kind.
macro_rules! both {
    ($a:expr, $b:expr, |$x:ident, $y:ident| $body:expr) => {
        match ($a, $b) {
            (Int::U64($x), Int::U64($y)) => $body,
            (Int::U128($x), Int::U128($y)) => $body,
            (Int::U256($x), Int::U256($y)) => $body,
            (Int::I64($x), Int::I64($y)) => $body,
            (Int::I128($x), Int::I128($y)) => $body,
            (Int::I256($x), Int::I256($y)) => $body,
            (a, b) => unreachable!("{a:?} and {b:?} were made one type"),
        }
    };
}

/// Runs `$body` with `$x` as the value, whatever its type.
macro_rules! each {
    ($a:expr, |$x:ident| $body:expr) => {
        match $a {
            Int::U64($x) => $body,
            Int::U128($x) => $body,
            Int::U256($x) => $body,
            Int::I64($x) => $body,
            Int::I128($x) => $body,
            Int::I256($x) => $body,
        }
    };
}

/// The integer in a Lua value, if it holds one.
pub fn int_of(value: &Value) -> Option<Int> {
    fn get<T: Prim>(data: &AnyUserData) -> Option<Int> {
        data.borrow::<LuaInt<T>>().ok().map(|value| value.0.int())
    }

    let Value::UserData(data) = value else {
        return None;
    };

    get::<u256>(data)
        .or_else(|| get::<u64>(data))
        .or_else(|| get::<u128>(data))
        .or_else(|| get::<i64>(data))
        .or_else(|| get::<i128>(data))
        .or_else(|| get::<i256>(data))
}

/// A number that holds a whole value exactly.
fn whole_number(value: &Value) -> Option<Exact> {
    match value {
        // mlua hands over any whole number that fits an i64 as an Integer, but past 2^53 a number
        // may already have been rounded, so it is refused the same as a Number there.
        Value::Integer(number) if number.unsigned_abs() <= MAX_EXACT => Some(Exact::new(*number < 0, u256::from(number.unsigned_abs()))),
        Value::Number(number) if number.fract() == 0.0 && number.abs() <= MAX_EXACT as f64 => {
            Some(Exact::new(*number < 0.0, u256::from(number.abs() as u64)))
        }
        _ => None,
    }
}

/// Why a value cannot be used as this type.
fn needs(kind: IntKind) -> Error {
    let number = if kind.signed() { "a whole number within 2^53 either way" } else { "a non-negative whole number up to 2^53" };

    Error::runtime(format!("{} arithmetic needs {} or {number}", kind.library(), kind.article()))
}

/// A number as this type, if it is whole and the type holds it.
fn number_as(value: &Value, kind: IntKind) -> mlua::Result<Int> {
    whole_number(value).and_then(|exact| exact.to(kind)).ok_or_else(|| needs(kind))
}

/// Any integer, or a whole number, as this type, when it holds the value: what storage writes,
/// transfers and selectors take.
pub fn exact_as<T: Prim>(value: &Value) -> mlua::Result<T> {
    let kind = T::KIND;

    match int_of(value) {
        Some(int) => int.to(kind).map(T::of).ok_or_else(|| {
            Error::runtime(format!("{int} is outside what {} holds, {} to {}", kind.article(), T::MIN, T::MAX))
        }),
        None => number_as(value, kind).map(T::of),
    }
}

/// Two integers as one type: the wider, when both are signed or both unsigned. Signed and
/// unsigned do not mix, as no type holds both U256 and I256.
fn unify(x: Int, y: Int) -> mlua::Result<(Int, Int)> {
    let (left, right) = (x.kind(), y.kind());

    if left.signed() != right.signed() {
        return Err(Error::runtime(format!(
            "{} and {} do not mix: convert one side, such as i256(x) or u256(x)",
            left.name(),
            right.name()
        )));
    }

    let kind = if left.bits() >= right.bits() { left } else { right };

    Ok((x.to(kind).expect("a wider type holds it"), y.to(kind).expect("a wider type holds it")))
}

/// Both sides of an operator as one type: unify's, or the integer's for a plain number.
fn operands(a: &Value, b: &Value) -> mlua::Result<(Int, Int)> {
    match (int_of(a), int_of(b)) {
        (Some(x), Some(y)) => unify(x, y),
        (Some(x), None) => Ok((x, number_as(b, x.kind())?)),
        (None, Some(y)) => Ok((number_as(a, y.kind())?, y)),
        (None, None) => Err(Error::runtime("integer arithmetic needs an integer on one side")),
    }
}

/// A method's receiver and its other side, as operands makes them.
fn with(this: Int, other: &Value) -> mlua::Result<(Int, Int)> {
    match int_of(other) {
        Some(other) => unify(this, other),
        None => Ok((this, number_as(other, this.kind())?)),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Add,
    Sub,
    Mul,
    /// Rounds toward zero, as Rust's / does.
    Div,
    /// Rounds down, as Lua's // does.
    FloorDiv,
    /// Takes the dividend's sign, as Rust's % does.
    Rem,
}

fn checked<T: Prim>(x: T, y: T, op: Op) -> Option<T> {
    match op {
        Op::Add => x.checked_add(y),
        Op::Sub => x.checked_sub(y),
        Op::Mul => x.checked_mul(y),
        Op::Div => x.checked_div(y),
        Op::FloorDiv => x.floor_div(y),
        Op::Rem => x.checked_rem(y),
    }
}

/// Why an operator failed.
fn failure<T: Prim>(y: T, op: Op) -> Error {
    let library = T::KIND.library();

    match op {
        Op::Div | Op::FloorDiv | Op::Rem if y == T::ZERO => Error::runtime("division by zero"),
        Op::Sub if !T::KIND.signed() => Error::runtime(format!("{library} underflow")),
        _ => Error::runtime(format!("{library} overflow")),
    }
}

fn arithmetic(a: &Value, b: &Value, op: Op) -> mlua::Result<Int> {
    let (x, y) = operands(a, b)?;

    both!(x, y, |x, y| checked(x, y, op).map(Prim::int).ok_or_else(|| failure(y, op)))
}

fn checked_method(this: Int, other: &Value, op: Op) -> mlua::Result<Option<Int>> {
    let (x, y) = with(this, other)?;

    Ok(both!(x, y, |x, y| checked(x, y, op).map(Prim::int)))
}

/// An exponent: a whole number from 0 to 2^32 - 1, as an integer or a number.
fn exponent_of(value: &Value) -> mlua::Result<u32> {
    let exact = int_of(value).map(Int::exact).or_else(|| whole_number(value));

    match exact {
        Some(exact) if exact.negative => Err(Error::runtime("a negative exponent would make a fraction")),
        Some(exact) => u32::try_from(exact.magnitude).map_err(|_| Error::runtime("an exponent is at most 2^32 - 1")),
        None => Err(Error::runtime("an exponent is a whole number from 0 to 2^32 - 1")),
    }
}

/// The base and exponent of ^: the base's type, or the exponent's for a plain number base.
fn power(a: &Value, b: &Value) -> mlua::Result<Int> {
    let base = match int_of(a) {
        Some(base) => base,
        None => number_as(a, int_of(b).expect("one side of ^ is an integer").kind())?,
    };
    let exponent = exponent_of(b)?;

    each!(base, |x| Prim::checked_pow(x, exponent)
        .map(Prim::int)
        .ok_or_else(|| Error::runtime(format!("{} overflow", base.kind().library()))))
}

/// One side of a comparison: an integer, or any number, by its true value.
enum Side {
    Int(Exact),
    Number(f64),
}

fn side(value: &Value) -> Option<Side> {
    match (int_of(value), value) {
        (Some(int), _) => Some(Side::Int(int.exact())),
        (None, Value::Integer(number)) if number.unsigned_abs() <= MAX_EXACT => Some(Side::Int(Exact::new(*number < 0, u256::from(number.unsigned_abs())))),
        // Past 2^53 a number is a plain one, held exactly by an f64 as it came from one.
        (None, Value::Integer(number)) => Some(Side::Number(*number as f64)),
        (None, Value::Number(number)) => Some(Side::Number(*number)),
        _ => None,
    }
}

/// Orders two values when both are integers or numbers, exactly: -1 is below every unsigned
/// value, 2.5 sits between 2 and 3, and NaN is unordered. None when they are not both.
fn order(a: &Value, b: &Value) -> Option<Option<Ordering>> {
    Some(match (side(a)?, side(b)?) {
        (Side::Int(a), Side::Int(b)) => Some(a.cmp(&b)),
        (Side::Int(a), Side::Number(b)) => against_number(a, b),
        (Side::Number(a), Side::Int(b)) => against_number(b, a).map(Ordering::reverse),
        (Side::Number(a), Side::Number(b)) => a.partial_cmp(&b),
    })
}

fn ordered(a: &Value, b: &Value) -> mlua::Result<Option<Ordering>> {
    order(a, b).ok_or_else(|| Error::runtime(format!("attempt to compare {} with {}", type_name(a), type_name(b))))
}

fn type_name(value: &Value) -> &'static str {
    int_of(value).map_or_else(|| value.type_name(), |int| int.kind().name())
}

/// An integer against a number. A finite number's whole part is a whole number times a power of
/// two, so it converts without rounding; its fraction then breaks a tie.
fn against_number(value: Exact, number: f64) -> Option<Ordering> {
    if number.is_nan() {
        return None;
    }

    let negative = number < 0.0;
    let whole = number.abs().trunc();
    let fraction = number.abs() > whole;
    // past 2^256 either way, infinities too, the number is past every integer
    let past = if negative { Ordering::Greater } else { Ordering::Less };

    let Some(magnitude) = whole_u256(whole) else {
        return Some(past);
    };

    match value.cmp(&Exact::new(negative, magnitude)) {
        Ordering::Equal if fraction => Some(past),
        ordering => Some(ordering),
    }
}

/// A non-negative whole f64 as a u256, or None at 2^256 and past it.
fn whole_u256(whole: f64) -> Option<u256> {
    if whole < (1u64 << 53) as f64 {
        return Some(u256::from(whole as u64));
    }

    if !whole.is_finite() {
        return None;
    }

    let bits = whole.to_bits();
    let exponent = ((bits >> 52) & 0x7ff) as u32 - 1075; // whole >= 2^53, so this is at least 1
    let mantissa = (bits & ((1 << 52) - 1)) | (1 << 52);

    // the mantissa holds 53 bits, so a shift past 203 leaves 256
    (exponent <= 203).then(|| u256::from(mantissa) << exponent)
}

fn hex(int: Int) -> String {
    let exact = int.exact();
    let sign = if exact.negative { "-" } else { "" };

    format!("{sign}{:#x}", exact.magnitude)
}

fn tonumber(int: Int) -> mlua::Result<f64> {
    let exact = int.exact();

    if exact.magnitude > u256::from(MAX_EXACT) {
        return Err(Error::runtime(format!("{int} is past 2^53, where numbers round")));
    }

    let number = exact.magnitude.as_u64() as f64;

    Ok(if exact.negative { -number } else { number })
}

fn text(value: &Value) -> mlua::Result<String> {
    match (int_of(value), value) {
        (Some(int), _) => Ok(int.to_string()),
        (None, Value::String(text)) => Ok(text.to_str()?.to_string()),
        (None, Value::Integer(number)) => Ok(number.to_string()),
        (None, Value::Number(number)) => Ok(number.to_string()),
        (None, other) => Err(Error::runtime(format!("attempt to concatenate {}", other.type_name()))),
    }
}

/// Makes a value of this type from a whole number, decimal or 0x hex text, with a minus for a
/// signed one and its digits grouped by `_` as `structs::digits::ungrouped` takes them, or any
/// integer that it holds.
fn new_int(value: &Value, kind: IntKind) -> mlua::Result<Int> {
    if let Some(int) = int_of(value) {
        return int.to(kind).ok_or_else(|| Error::runtime(format!("not {}: {int}", kind.library_article())));
    }

    let Value::String(text) = value else {
        return number_as(value, kind);
    };
    let text = text.to_str()?;
    let refused = || Error::runtime(format!("not {}: {}", kind.library_article(), &*text));
    let (negative, digits) = match text.strip_prefix('-') {
        Some(rest) if kind.signed() => (true, rest),
        _ => (false, &*text),
    };
    // Grouped as people write them, 1_000_000 or 0xff_ff, each _ between two digits.
    let digits = crate::structs::digits::ungrouped(digits).ok_or_else(refused)?;
    let digits = &*digits;
    let magnitude = match digits.strip_prefix("0x") {
        Some(hex) => u256::from_str_radix(hex, 16),
        None => u256::from_str_radix(digits, 10),
    };

    magnitude
        .ok()
        .filter(|_| !digits.starts_with(['+', '-']))
        .and_then(|magnitude| Exact::new(negative, magnitude).to(kind))
        .ok_or_else(refused)
}

/// The checked_ methods, and the operator each stands for.
const CHECKED: [(&str, Op); 5] = [
    ("checked_add", Op::Add),
    ("checked_sub", Op::Sub),
    ("checked_mul", Op::Mul),
    ("checked_div", Op::Div),
    ("checked_rem", Op::Rem),
];

impl<T: Prim> UserData for LuaInt<T> {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_meta_field(MetaMethod::Type, T::KIND.name());
    }

    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("hex", |_, this, ()| Ok(hex(this.0.int())));
        methods.add_method("tonumber", |_, this, ()| tonumber(this.0.int()));

        for (name, op) in CHECKED {
            methods.add_method(name, move |_, this, other: Value| checked_method(this.0.int(), &other, op));
        }

        methods.add_method("checked_pow", |_, this, exponent: Value| {
            Ok(this.0.checked_pow(exponent_of(&exponent)?).map(LuaInt))
        });
        methods.add_method("saturating_add", |_, this, other: Value| {
            let (x, y) = with(this.0.int(), &other)?;

            Ok(both!(x, y, |x, y| Prim::saturating_add(x, y).int()))
        });
        methods.add_method("saturating_sub", |_, this, other: Value| {
            let (x, y) = with(this.0.int(), &other)?;

            Ok(both!(x, y, |x, y| Prim::saturating_sub(x, y).int()))
        });
        methods.add_method("saturating_mul", |_, this, other: Value| {
            let (x, y) = with(this.0.int(), &other)?;

            Ok(both!(x, y, |x, y| Prim::saturating_mul(x, y).int()))
        });
        methods.add_method("saturating_pow", |_, this, exponent: Value| {
            Ok(LuaInt(this.0.saturating_pow(exponent_of(&exponent)?)))
        });
        methods.add_method("wrapping_add", |_, this, other: Value| {
            let (x, y) = with(this.0.int(), &other)?;

            Ok(both!(x, y, |x, y| Prim::wrapping_add(x, y).int()))
        });
        methods.add_method("wrapping_sub", |_, this, other: Value| {
            let (x, y) = with(this.0.int(), &other)?;

            Ok(both!(x, y, |x, y| Prim::wrapping_sub(x, y).int()))
        });
        methods.add_method("wrapping_mul", |_, this, other: Value| {
            let (x, y) = with(this.0.int(), &other)?;

            Ok(both!(x, y, |x, y| Prim::wrapping_mul(x, y).int()))
        });
        methods.add_method("wrapping_pow", |_, this, exponent: Value| {
            Ok(LuaInt(this.0.wrapping_pow(exponent_of(&exponent)?)))
        });
        methods.add_method("min", |_, this, other: Value| {
            let (x, y) = with(this.0.int(), &other)?;

            Ok(both!(x, y, |x, y| x.min(y).int()))
        });
        methods.add_method("max", |_, this, other: Value| {
            let (x, y) = with(this.0.int(), &other)?;

            Ok(both!(x, y, |x, y| x.max(y).int()))
        });

        methods.add_method("checked_neg", |_, this, ()| Ok(this.0.checked_neg().map(LuaInt)));
        methods.add_method("wrapping_neg", |_, this, ()| Ok(LuaInt(this.0.wrapping_neg())));

        if T::KIND.signed() {
            methods.add_method("abs", |_, this, ()| {
                this.0.checked_abs().map(LuaInt).ok_or_else(|| Error::runtime(format!("{} overflow", T::KIND.library())))
            });
            // only the min overflows, toward the max
            methods.add_method("saturating_neg", |_, this, ()| Ok(LuaInt(this.0.checked_neg().unwrap_or(T::MAX))));
        }

        methods.add_meta_method(MetaMethod::ToString, |_, this, ()| Ok(this.0.to_string()));
        methods.add_meta_function(MetaMethod::Add, |_, (a, b): (Value, Value)| arithmetic(&a, &b, Op::Add));
        methods.add_meta_function(MetaMethod::Sub, |_, (a, b): (Value, Value)| arithmetic(&a, &b, Op::Sub));
        methods.add_meta_function(MetaMethod::Mul, |_, (a, b): (Value, Value)| arithmetic(&a, &b, Op::Mul));
        methods.add_meta_function(MetaMethod::Div, |_, (a, b): (Value, Value)| arithmetic(&a, &b, Op::Div));
        methods.add_meta_function(MetaMethod::IDiv, |_, (a, b): (Value, Value)| arithmetic(&a, &b, Op::FloorDiv));
        methods.add_meta_function(MetaMethod::Mod, |_, (a, b): (Value, Value)| arithmetic(&a, &b, Op::Rem));
        methods.add_meta_function(MetaMethod::Pow, |_, (a, b): (Value, Value)| power(&a, &b));
        methods.add_meta_method(MetaMethod::Unm, |_, this, ()| {
            let failure = if T::KIND.signed() { "overflow" } else { "underflow" };

            this.0.checked_neg().map(LuaInt).ok_or_else(|| Error::runtime(format!("{} {failure}", T::KIND.library())))
        });
        methods.add_meta_function(MetaMethod::Eq, |_, (a, b): (Value, Value)| {
            Ok(order(&a, &b) == Some(Some(Ordering::Equal)))
        });
        methods.add_meta_function(MetaMethod::Lt, |_, (a, b): (Value, Value)| {
            Ok(ordered(&a, &b)? == Some(Ordering::Less))
        });
        methods.add_meta_function(MetaMethod::Le, |_, (a, b): (Value, Value)| {
            Ok(matches!(ordered(&a, &b)?, Some(Ordering::Less | Ordering::Equal)))
        });
        methods.add_meta_function(MetaMethod::Concat, |_, (a, b): (Value, Value)| {
            Ok(format!("{}{}", text(&a)?, text(&b)?))
        });
    }
}

pub fn register_integer_libs(lua: &Lua) {
    register::<u64>(lua);
    register::<u128>(lua);
    register::<u256>(lua);
    register::<i64>(lua);
    register::<i128>(lua);
    register::<i256>(lua);
}

/// The type's library: new, called as the library itself too, and zero, min and max.
fn register<T: Prim>(lua: &Lua) {
    let kind = T::KIND;
    let name = kind.library();
    let failed = |what: &str, err: Error| -> ! { panic!("[{name}] Failed to {what}: {err}") };
    let table = lua.create_table().unwrap_or_else(|err| failed("create its table", err));
    let new = lua
        .create_function(move |_, value: Value| new_int(&value, kind))
        .unwrap_or_else(|err| failed("create new", err));
    table.set("new", new).unwrap_or_else(|err| failed("set new", err));

    for (field, value) in [("zero", T::ZERO), ("min", T::MIN), ("max", T::MAX)] {
        table.set(field, LuaInt(value)).unwrap_or_else(|err| failed("set a constant", err));
    }

    let meta = lua.create_table().unwrap_or_else(|err| failed("create its metatable", err));
    let call = lua
        .create_function(move |_, (_, value): (Table, Value)| new_int(&value, kind))
        .unwrap_or_else(|err| failed("create __call", err));
    meta.set("__call", call).unwrap_or_else(|err| failed("set __call", err));
    table.set_metatable(Some(meta)).unwrap_or_else(|err| failed("set its metatable", err));
    lua.globals().set(name, table).unwrap_or_else(|err| failed("set its global", err));
}

#[document(
    lua,
    kind = "class",
    name = "Integer",
    category = "Numbers",
    summary = "What every integer type has: U64, U128, U256, I64, I128 and I256. Operators: + - * / // % ^ and unary -, with another integer or a whole number; == ~= < <= > >= with any integer or any number; .. and tostring.",
    note = "The operators never wrap: overflow, underflow and division by zero raise an error. The checked_ methods return nil instead, the saturating_ ones stop at the type's min and max, and the wrapping_ ones wrap around, as Rust's do. Two integers of one signedness make the wider type, u64(1) + u256(2) a U256, and a number takes the integer's; a signed and an unsigned one do not mix, so convert one side with i256(x) or u256(x). / rounds toward zero and % takes the dividend's sign, as in Rust, so i64(-7) / 2 is -3 and i64(-7) % 2 is -1; // rounds down, as Lua's does, so i64(-7) // 2 is -4. Comparisons go by the true value across every type and number: u256(5) == 5, u64(0) > i64(-1) and u128(2) < 2.5.",
    example = "local a = u128(\"340282366920938463463374607431768211455\")\nlocal b = a:checked_add(1) -- nil: past u128.max\nlocal c = u256(a) + 1 -- fits a U256\nassert(c > a and a == u128.max)",
    see_also = "U256, I256",
)]
fn integer_class() {}

#[document(
    lua,
    parent = "Integer",
    name = "hex",
    kind = "method",
    summary = "The value in hex with a 0x prefix and no leading zeros, after a minus below zero.",
    returns = { ty = "string", desc = "For example 0xff or -0x10." },
)]
fn integer_hex() {}

#[document(
    lua,
    parent = "Integer",
    name = "tonumber",
    kind = "method",
    summary = "The value as a plain number, for math a number does exactly, such as a for loop.",
    returns = { ty = "number", desc = "The same value." },
    panics = "Errors past 2^53 either way, where a number would round.",
    example = "for i = 1, count:tonumber() do end",
)]
fn integer_tonumber() {}

#[document(
    lua,
    parent = "Integer",
    name = "checked_add",
    kind = "method",
    summary = "Adds, giving nil where + would overflow.",
    params = {
        other = { ty = "U256|number", desc = "Another integer of the same signedness, or a whole number." },
    },
    returns = { ty = "Integer?", desc = "The sum, or nil." },
    example = "local total = balance:checked_add(amount)\nif not total then error(\"too much\") end",
)]
fn integer_checked_add() {}

#[document(
    lua,
    parent = "Integer",
    name = "checked_sub",
    kind = "method",
    summary = "Subtracts, giving nil where - would overflow or go below an unsigned type's 0.",
    params = {
        other = { ty = "U256|number", desc = "Another integer of the same signedness, or a whole number." },
    },
    returns = { ty = "Integer?", desc = "The difference, or nil." },
    example = "local left = balance:checked_sub(amount)\nif not left then error(\"not enough\") end",
)]
fn integer_checked_sub() {}

#[document(
    lua,
    parent = "Integer",
    name = "checked_mul",
    kind = "method",
    summary = "Multiplies, giving nil where * would overflow.",
    params = {
        other = { ty = "U256|number", desc = "Another integer of the same signedness, or a whole number." },
    },
    returns = { ty = "Integer?", desc = "The product, or nil." },
)]
fn integer_checked_mul() {}

#[document(
    lua,
    parent = "Integer",
    name = "checked_div",
    kind = "method",
    summary = "Divides as / does, toward zero, giving nil for division by zero or the signed min divided by -1.",
    params = {
        other = { ty = "U256|number", desc = "Another integer of the same signedness, or a whole number." },
    },
    returns = { ty = "Integer?", desc = "The quotient, or nil." },
)]
fn integer_checked_div() {}

#[document(
    lua,
    parent = "Integer",
    name = "checked_rem",
    kind = "method",
    summary = "The remainder as % gives it, with the dividend's sign, or nil for division by zero or the signed min by -1.",
    params = {
        other = { ty = "U256|number", desc = "Another integer of the same signedness, or a whole number." },
    },
    returns = { ty = "Integer?", desc = "The remainder, or nil." },
)]
fn integer_checked_rem() {}

#[document(
    lua,
    parent = "Integer",
    name = "checked_pow",
    kind = "method",
    summary = "Raises to a power, giving nil where ^ would overflow.",
    params = {
        exponent = { ty = "U64|number", desc = "A whole number from 0 to 2^32 - 1." },
    },
    returns = { ty = "Integer?", desc = "The power, or nil." },
    panics = "Errors on an exponent below 0 or past 2^32 - 1.",
)]
fn integer_checked_pow() {}

#[document(
    lua,
    parent = "Integer",
    name = "saturating_add",
    kind = "method",
    summary = "Adds, stopping at the type's max, or its min below zero.",
    params = {
        other = { ty = "U256|number", desc = "Another integer of the same signedness, or a whole number." },
    },
    returns = { ty = "Integer", desc = "The sum, or the limit it reached." },
)]
fn integer_saturating_add() {}

#[document(
    lua,
    parent = "Integer",
    name = "saturating_sub",
    kind = "method",
    summary = "Subtracts, stopping at an unsigned type's 0 or a signed one's min and max.",
    params = {
        other = { ty = "U256|number", desc = "Another integer of the same signedness, or a whole number." },
    },
    returns = { ty = "Integer", desc = "The difference, or the limit it reached." },
    example = "local left = allowance:saturating_sub(spent)",
)]
fn integer_saturating_sub() {}

#[document(
    lua,
    parent = "Integer",
    name = "saturating_mul",
    kind = "method",
    summary = "Multiplies, stopping at the type's min and max.",
    params = {
        other = { ty = "U256|number", desc = "Another integer of the same signedness, or a whole number." },
    },
    returns = { ty = "Integer", desc = "The product, or the limit it reached." },
)]
fn integer_saturating_mul() {}

#[document(
    lua,
    parent = "Integer",
    name = "saturating_pow",
    kind = "method",
    summary = "Raises to a power, stopping at the type's min and max.",
    params = {
        exponent = { ty = "U64|number", desc = "A whole number from 0 to 2^32 - 1." },
    },
    returns = { ty = "Integer", desc = "The power, or the limit it reached." },
    panics = "Errors on an exponent below 0 or past 2^32 - 1.",
)]
fn integer_saturating_pow() {}

#[document(
    lua,
    parent = "Integer",
    name = "wrapping_add",
    kind = "method",
    summary = "Adds modulo 2^bits: past the max it wraps around to the min.",
    params = {
        other = { ty = "U256|number", desc = "Another integer of the same signedness, or a whole number." },
    },
    returns = { ty = "Integer", desc = "The sum, wrapped." },
    example = "assert(u256.max:wrapping_add(1) == 0)",
)]
fn integer_wrapping_add() {}

#[document(
    lua,
    parent = "Integer",
    name = "wrapping_sub",
    kind = "method",
    summary = "Subtracts modulo 2^bits: below the min it wraps around to the max.",
    params = {
        other = { ty = "U256|number", desc = "Another integer of the same signedness, or a whole number." },
    },
    returns = { ty = "Integer", desc = "The difference, wrapped." },
    example = "assert(u64(0):wrapping_sub(1) == u64.max)",
)]
fn integer_wrapping_sub() {}

#[document(
    lua,
    parent = "Integer",
    name = "wrapping_mul",
    kind = "method",
    summary = "Multiplies modulo 2^bits, keeping the low bits of the product.",
    params = {
        other = { ty = "U256|number", desc = "Another integer of the same signedness, or a whole number." },
    },
    returns = { ty = "Integer", desc = "The product, wrapped." },
)]
fn integer_wrapping_mul() {}

#[document(
    lua,
    parent = "Integer",
    name = "wrapping_pow",
    kind = "method",
    summary = "Raises to a power modulo 2^bits.",
    params = {
        exponent = { ty = "U64|number", desc = "A whole number from 0 to 2^32 - 1." },
    },
    returns = { ty = "Integer", desc = "The power, wrapped." },
    panics = "Errors on an exponent below 0 or past 2^32 - 1.",
    example = "assert(u256(2):wrapping_pow(256) == 0)",
)]
fn integer_wrapping_pow() {}

#[document(
    lua,
    parent = "Integer",
    name = "min",
    kind = "method",
    summary = "The smaller of the two, as the wider type when they differ.",
    params = {
        other = { ty = "U256|number", desc = "Another integer of the same signedness, or a whole number." },
    },
    returns = { ty = "Integer", desc = "The smaller value." },
    example = "local paid = amount:min(limit)",
)]
fn integer_min() {}

#[document(
    lua,
    parent = "Integer",
    name = "max",
    kind = "method",
    summary = "The larger of the two, as the wider type when they differ.",
    params = {
        other = { ty = "U256|number", desc = "Another integer of the same signedness, or a whole number." },
    },
    returns = { ty = "Integer", desc = "The larger value." },
)]
fn integer_max() {}

#[document(
    lua,
    parent = "Integer",
    name = "checked_neg",
    kind = "method",
    summary = "The value with its sign turned, or nil where the type does not hold it: the signed min, or any unsigned value but 0.",
    returns = { ty = "Integer?", desc = "-x, or nil." },
)]
fn integer_checked_neg() {}

#[document(
    lua,
    parent = "Integer",
    name = "wrapping_neg",
    kind = "method",
    summary = "The value with its sign turned, modulo 2^bits: the signed min stays the min, and an unsigned x becomes 2^bits - x.",
    returns = { ty = "Integer", desc = "-x, wrapped." },
    example = "assert(u64(1):wrapping_neg() == u64.max)",
)]
fn integer_wrapping_neg() {}

#[document(
    lua,
    parent = "Integer",
    name = "saturating_neg",
    kind = "method",
    summary = "The value with its sign turned, the signed min becoming the max. Only the signed types have it.",
    returns = { ty = "Integer", desc = "-x, or the max." },
)]
fn integer_saturating_neg() {}

#[document(
    lua,
    parent = "Integer",
    name = "abs",
    kind = "method",
    summary = "The value without its sign. Only the signed types have it.",
    returns = { ty = "Integer", desc = "The same type's absolute value." },
    panics = "Errors on the type's min, whose absolute value it does not hold: i64.min:abs().",
)]
fn integer_abs() {}

#[document(
    lua,
    kind = "class",
    name = "U64",
    category = "Numbers",
    summary = "An unsigned 64-bit integer: 0 to 2^64 - 1. u64 arguments and the u64 storage kind arrive as one. It has every Integer method and operator.",
    see_also = "Integer, u64.new",
)]
fn u64_class() {}

#[document(
    lua,
    kind = "library",
    name = "u64",
    category = "Numbers",
    summary = "Makes U64 values. u64(value) is short for u64.new(value).",
    example = "local total = u64(\"1000000000\") * 3\nassert(total > 0)",
)]
fn u64_lib() {}

#[document(
    lua,
    parent = "u64",
    name = "new",
    summary = "Makes a U64 from a whole number, decimal or 0x hex text, or any integer it holds.",
    params = {
        value = { ty = "number|string|Integer", desc = "Text above 2^53, which a number cannot hold exactly: decimal digits or 0x hex." },
    },
    returns = { ty = "U64", desc = "The value." },
    panics = "Errors on a fraction, a number past 2^53 either way, text that is not a number, and anything the type does not hold, such as a negative.",
    example = "local big = u64(\"18446744073709551615\")",
)]
fn u64_new() {}

#[document(
    lua,
    parent = "u64",
    name = "zero",
    kind = "field",
    summary = "Zero.",
    returns = { ty = "U64", desc = "0" },
)]
fn u64_zero() {}

#[document(
    lua,
    parent = "u64",
    name = "min",
    kind = "field",
    summary = "The smallest U64.",
    returns = { ty = "U64", desc = "0" },
)]
fn u64_min() {}

#[document(
    lua,
    parent = "u64",
    name = "max",
    kind = "field",
    summary = "The largest U64.",
    returns = { ty = "U64", desc = "18446744073709551615" },
)]
fn u64_max() {}

#[document(
    lua,
    kind = "class",
    name = "U128",
    category = "Numbers",
    summary = "An unsigned 128-bit integer: 0 to 2^128 - 1. u128 arguments and the u128 storage kind arrive as one. It has every Integer method and operator.",
    see_also = "Integer, u128.new",
)]
fn u128_class() {}

#[document(
    lua,
    kind = "library",
    name = "u128",
    category = "Numbers",
    summary = "Makes U128 values. u128(value) is short for u128.new(value).",
    example = "local total = u128(\"1000000000\") * 3\nassert(total > 0)",
)]
fn u128_lib() {}

#[document(
    lua,
    parent = "u128",
    name = "new",
    summary = "Makes a U128 from a whole number, decimal or 0x hex text, or any integer it holds.",
    params = {
        value = { ty = "number|string|Integer", desc = "Text above 2^53, which a number cannot hold exactly: decimal digits or 0x hex." },
    },
    returns = { ty = "U128", desc = "The value." },
    panics = "Errors on a fraction, a number past 2^53 either way, text that is not a number, and anything the type does not hold, such as a negative.",
    example = "local big = u128(\"340282366920938463463374607431768211455\")",
)]
fn u128_new() {}

#[document(
    lua,
    parent = "u128",
    name = "zero",
    kind = "field",
    summary = "Zero.",
    returns = { ty = "U128", desc = "0" },
)]
fn u128_zero() {}

#[document(
    lua,
    parent = "u128",
    name = "min",
    kind = "field",
    summary = "The smallest U128.",
    returns = { ty = "U128", desc = "0" },
)]
fn u128_min() {}

#[document(
    lua,
    parent = "u128",
    name = "max",
    kind = "field",
    summary = "The largest U128.",
    returns = { ty = "U128", desc = "340282366920938463463374607431768211455" },
)]
fn u128_max() {}

#[document(
    lua,
    kind = "class",
    name = "U256",
    category = "Numbers",
    summary = "An unsigned 256-bit integer: 0 to 2^256 - 1, for amounts and anything a number would round. u256 arguments, amounts, ctx.value and the u256 storage kind arrive as one. It has every Integer method and operator.",
    see_also = "Integer, u256.new",
)]
fn u256_class() {}

#[document(
    lua,
    kind = "library",
    name = "u256",
    category = "Numbers",
    summary = "Makes U256 values. u256(value) is short for u256.new(value).",
    example = "local total = u256(\"1000000000000000000000\") + 5\nassert(total > 0)",
)]
fn u256_lib() {}

#[document(
    lua,
    parent = "u256",
    name = "new",
    summary = "Makes a U256 from a whole number, decimal or 0x hex text, or any integer it holds.",
    params = {
        value = { ty = "number|string|Integer", desc = "Text above 2^53, which a number cannot hold exactly: decimal digits or 0x hex." },
    },
    returns = { ty = "U256", desc = "The value." },
    panics = "Errors on a fraction, a number past 2^53 either way, text that is not a number, and anything the type does not hold, such as a negative.",
    example = "local big = u256(\"115792089237316195423570985008687907853269984665640564039457584007913129639935\")",
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
    name = "min",
    kind = "field",
    summary = "The smallest U256.",
    returns = { ty = "U256", desc = "0" },
)]
fn u256_min() {}

#[document(
    lua,
    parent = "u256",
    name = "max",
    kind = "field",
    summary = "The largest U256.",
    returns = { ty = "U256", desc = "115792089237316195423570985008687907853269984665640564039457584007913129639935" },
)]
fn u256_max() {}

#[document(
    lua,
    kind = "class",
    name = "I64",
    category = "Numbers",
    summary = "A signed 64-bit integer: -2^63 to 2^63 - 1. i64 arguments and the i64 storage kind arrive as one. It has every Integer method and operator.",
    see_also = "Integer, i64.new",
)]
fn i64_class() {}

#[document(
    lua,
    kind = "library",
    name = "i64",
    category = "Numbers",
    summary = "Makes I64 values. i64(value) is short for i64.new(value).",
    example = "local debt = i64(\"-5000\")\nassert(debt < 0 and -debt == 5000)",
)]
fn i64_lib() {}

#[document(
    lua,
    parent = "i64",
    name = "new",
    summary = "Makes an I64 from a whole number, decimal or 0x hex text, or any integer it holds.",
    params = {
        value = { ty = "number|string|Integer", desc = "Text above 2^53, which a number cannot hold exactly: decimal digits or 0x hex, after a minus below zero." },
    },
    returns = { ty = "I64", desc = "The value." },
    panics = "Errors on a fraction, a number past 2^53 either way, text that is not a number, and anything the type does not hold.",
    example = "local big = i64(\"9223372036854775807\")",
)]
fn i64_new() {}

#[document(
    lua,
    parent = "i64",
    name = "zero",
    kind = "field",
    summary = "Zero.",
    returns = { ty = "I64", desc = "0" },
)]
fn i64_zero() {}

#[document(
    lua,
    parent = "i64",
    name = "min",
    kind = "field",
    summary = "The smallest I64.",
    returns = { ty = "I64", desc = "-9223372036854775808" },
)]
fn i64_min() {}

#[document(
    lua,
    parent = "i64",
    name = "max",
    kind = "field",
    summary = "The largest I64.",
    returns = { ty = "I64", desc = "9223372036854775807" },
)]
fn i64_max() {}

#[document(
    lua,
    kind = "class",
    name = "I128",
    category = "Numbers",
    summary = "A signed 128-bit integer: -2^127 to 2^127 - 1. i128 arguments and the i128 storage kind arrive as one. It has every Integer method and operator.",
    see_also = "Integer, i128.new",
)]
fn i128_class() {}

#[document(
    lua,
    kind = "library",
    name = "i128",
    category = "Numbers",
    summary = "Makes I128 values. i128(value) is short for i128.new(value).",
    example = "local debt = i128(\"-5000\")\nassert(debt < 0 and -debt == 5000)",
)]
fn i128_lib() {}

#[document(
    lua,
    parent = "i128",
    name = "new",
    summary = "Makes an I128 from a whole number, decimal or 0x hex text, or any integer it holds.",
    params = {
        value = { ty = "number|string|Integer", desc = "Text above 2^53, which a number cannot hold exactly: decimal digits or 0x hex, after a minus below zero." },
    },
    returns = { ty = "I128", desc = "The value." },
    panics = "Errors on a fraction, a number past 2^53 either way, text that is not a number, and anything the type does not hold.",
    example = "local big = i128(\"170141183460469231731687303715884105727\")",
)]
fn i128_new() {}

#[document(
    lua,
    parent = "i128",
    name = "zero",
    kind = "field",
    summary = "Zero.",
    returns = { ty = "I128", desc = "0" },
)]
fn i128_zero() {}

#[document(
    lua,
    parent = "i128",
    name = "min",
    kind = "field",
    summary = "The smallest I128.",
    returns = { ty = "I128", desc = "-170141183460469231731687303715884105728" },
)]
fn i128_min() {}

#[document(
    lua,
    parent = "i128",
    name = "max",
    kind = "field",
    summary = "The largest I128.",
    returns = { ty = "I128", desc = "170141183460469231731687303715884105727" },
)]
fn i128_max() {}

#[document(
    lua,
    kind = "class",
    name = "I256",
    category = "Numbers",
    summary = "A signed 256-bit integer: -2^255 to 2^255 - 1. i256 arguments and the i256 storage kind arrive as one. It has every Integer method and operator.",
    see_also = "Integer, i256.new",
)]
fn i256_class() {}

#[document(
    lua,
    kind = "library",
    name = "i256",
    category = "Numbers",
    summary = "Makes I256 values. i256(value) is short for i256.new(value).",
    example = "local debt = i256(\"-5000\")\nassert(debt < 0 and -debt == 5000)",
)]
fn i256_lib() {}

#[document(
    lua,
    parent = "i256",
    name = "new",
    summary = "Makes an I256 from a whole number, decimal or 0x hex text, or any integer it holds.",
    params = {
        value = { ty = "number|string|Integer", desc = "Text above 2^53, which a number cannot hold exactly: decimal digits or 0x hex, after a minus below zero." },
    },
    returns = { ty = "I256", desc = "The value." },
    panics = "Errors on a fraction, a number past 2^53 either way, text that is not a number, and anything the type does not hold.",
    example = "local big = i256(\"57896044618658097711785492504343953926634992332820282019728792003956564819967\")",
)]
fn i256_new() {}

#[document(
    lua,
    parent = "i256",
    name = "zero",
    kind = "field",
    summary = "Zero.",
    returns = { ty = "I256", desc = "0" },
)]
fn i256_zero() {}

#[document(
    lua,
    parent = "i256",
    name = "min",
    kind = "field",
    summary = "The smallest I256.",
    returns = { ty = "I256", desc = "-57896044618658097711785492504343953926634992332820282019728792003956564819968" },
)]
fn i256_min() {}

#[document(
    lua,
    parent = "i256",
    name = "max",
    kind = "field",
    summary = "The largest I256.",
    returns = { ty = "I256", desc = "57896044618658097711785492504343953926634992332820282019728792003956564819967" },
)]
fn i256_max() {}

#[cfg(test)]
mod tests;
