//! A signed 256-bit integer, two's complement, as the `i256` kind and argument type hold it.
//!
//! Like U256, the operators abort the call on overflow and division by zero, with the messages a
//! Lua contract's I256 raises, and the checked_, saturating_ and wrapping_ methods do what Rust's
//! integers' do. / rounds toward zero and % takes the dividend's sign, as Rust's do. An i64 works
//! on either side of an operator or a comparison: `debt - 5`, `debt < 0`, `0 > debt`.

use {
    crate::u256::{OutOfRange, U256, fail},
    alloc::string::String,
    core::{
        cmp::Ordering,
        fmt,
        ops::{Add, AddAssign, Div, DivAssign, Mul, MulAssign, Neg, Rem, RemAssign, Sub, SubAssign},
    },
};

/// The value's bits, as a U256 holds them.
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct I256(U256);

/// 2^255: the magnitude of MIN, one past MAX's.
const HALF: U256 = U256::from_limbs([0, 0, 0, 1 << 63]);

impl I256 {
    pub const ZERO: I256 = I256(U256::ZERO);
    pub const ONE: I256 = I256(U256::ONE);
    pub const MIN: I256 = I256(HALF);
    pub const MAX: I256 = I256(U256::from_limbs([u64::MAX, u64::MAX, u64::MAX, u64::MAX >> 1]));

    /// From 32 little-endian bytes, two's complement, as the host reads and writes them.
    pub fn from_le_bytes(bytes: [u8; 32]) -> I256 {
        I256(U256::from_le_bytes(bytes))
    }

    pub fn to_le_bytes(self) -> [u8; 32] {
        self.0.to_le_bytes()
    }

    pub fn is_negative(self) -> bool {
        self.0 >= HALF
    }

    pub fn is_zero(self) -> bool {
        self.0.is_zero()
    }

    /// The value without its sign, which fits a U256 even for MIN.
    pub fn unsigned_abs(self) -> U256 {
        if self.is_negative() { U256::ZERO.wrapping_sub(self.0) } else { self.0 }
    }

    /// The value with this sign and magnitude, if an I256 holds it.
    fn from_sign(negative: bool, magnitude: U256) -> Option<I256> {
        match negative {
            true if magnitude <= HALF => Some(I256(U256::ZERO.wrapping_sub(magnitude))),
            false if magnitude < HALF => Some(I256(magnitude)),
            _ => None,
        }
    }

    pub fn checked_add(self, other: I256) -> Option<I256> {
        let sum = self.wrapping_add(other);
        // only two values of one sign can overflow, and then the sum has the other
        let overflow = self.is_negative() == other.is_negative() && sum.is_negative() != self.is_negative();

        (!overflow).then_some(sum)
    }

    pub fn checked_sub(self, other: I256) -> Option<I256> {
        let difference = self.wrapping_sub(other);
        let overflow = self.is_negative() != other.is_negative() && difference.is_negative() != self.is_negative();

        (!overflow).then_some(difference)
    }

    pub fn checked_mul(self, other: I256) -> Option<I256> {
        let magnitude = self.unsigned_abs().checked_mul(other.unsigned_abs())?;

        I256::from_sign(self.is_negative() != other.is_negative(), magnitude)
    }

    /// Rounds toward zero. None for division by zero, and for MIN / -1.
    pub fn checked_div(self, other: I256) -> Option<I256> {
        let quotient = self.unsigned_abs().checked_div(other.unsigned_abs())?;

        I256::from_sign(self.is_negative() != other.is_negative(), quotient)
    }

    /// Takes the dividend's sign. None for division by zero, and for MIN % -1, as in Rust.
    pub fn checked_rem(self, other: I256) -> Option<I256> {
        if self == I256::MIN && other == I256::from(-1i64) {
            return None;
        }

        let remainder = self.unsigned_abs().checked_rem(other.unsigned_abs())?;

        I256::from_sign(self.is_negative(), remainder)
    }

    /// None for MIN, whose negation an I256 does not hold.
    pub fn checked_neg(self) -> Option<I256> {
        I256::from_sign(!self.is_negative(), self.unsigned_abs())
    }

    pub fn checked_abs(self) -> Option<I256> {
        I256::from_sign(false, self.unsigned_abs())
    }

    pub fn checked_pow(self, exponent: u32) -> Option<I256> {
        let magnitude = self.unsigned_abs().checked_pow(exponent)?;

        I256::from_sign(self.is_negative() && exponent % 2 == 1, magnitude)
    }

    pub fn saturating_add(self, other: I256) -> I256 {
        self.checked_add(other).unwrap_or(if other.is_negative() { I256::MIN } else { I256::MAX })
    }

    pub fn saturating_sub(self, other: I256) -> I256 {
        self.checked_sub(other).unwrap_or(if other.is_negative() { I256::MAX } else { I256::MIN })
    }

    pub fn saturating_mul(self, other: I256) -> I256 {
        let negative = self.is_negative() != other.is_negative();

        self.checked_mul(other).unwrap_or(if negative { I256::MIN } else { I256::MAX })
    }

    pub fn saturating_pow(self, exponent: u32) -> I256 {
        let negative = self.is_negative() && exponent % 2 == 1;

        self.checked_pow(exponent).unwrap_or(if negative { I256::MIN } else { I256::MAX })
    }

    // Two's complement adds, subtracts and multiplies the bits as unsigned ones.
    pub fn wrapping_add(self, other: I256) -> I256 {
        I256(self.0.wrapping_add(other.0))
    }

    pub fn wrapping_sub(self, other: I256) -> I256 {
        I256(self.0.wrapping_sub(other.0))
    }

    pub fn wrapping_mul(self, other: I256) -> I256 {
        I256(self.0.wrapping_mul(other.0))
    }

    pub fn wrapping_pow(self, exponent: u32) -> I256 {
        I256(self.0.wrapping_pow(exponent))
    }

    pub fn wrapping_neg(self) -> I256 {
        I256(U256::ZERO.wrapping_sub(self.0))
    }

    /// The negation, MIN becoming MAX.
    pub fn saturating_neg(self) -> I256 {
        self.checked_neg().unwrap_or(I256::MAX)
    }

    /// The absolute value, aborting the call for MIN.
    pub fn abs(self) -> I256 {
        self.checked_abs().unwrap_or_else(|| fail("i256 overflow"))
    }

    /// Raises to a power, aborting the call on overflow, as ^ does in Lua.
    pub fn pow(self, exponent: u32) -> I256 {
        self.checked_pow(exponent).unwrap_or_else(|| fail("i256 overflow"))
    }

    /// The value in decimal digits, after a minus below zero, as Lua's tostring writes an I256.
    pub fn to_decimal(self) -> String {
        let digits = self.unsigned_abs().to_decimal();

        if self.is_negative() { alloc::format!("-{digits}") } else { digits }
    }

    /// Reads decimal digits after an optional minus, as Lua's i256(text) does, grouped by `_` as
    /// U256::from_decimal takes them: None for anything else, or for a value past MIN or MAX.
    pub fn from_decimal(text: &str) -> Option<I256> {
        let (negative, digits) = match text.strip_prefix('-') {
            Some(digits) => (true, digits),
            None => (false, text),
        };

        I256::from_sign(negative, U256::from_decimal(digits)?)
    }
}

/// Why an operator failed: division by zero, or the overflow of MIN / -1 and the like.
fn failure(other: I256, division: bool) -> &'static str {
    if division && other.is_zero() { "division by zero" } else { "i256 overflow" }
}

macro_rules! operator {
    ($trait:ident, $method:ident, $assign:ident, $assign_method:ident, $checked:ident, $division:literal) => {
        impl $trait for I256 {
            type Output = I256;

            fn $method(self, other: I256) -> I256 {
                self.$checked(other).unwrap_or_else(|| fail(failure(other, $division)))
            }
        }

        impl $trait<i64> for I256 {
            type Output = I256;

            fn $method(self, other: i64) -> I256 {
                $trait::$method(self, I256::from(other))
            }
        }

        impl $trait<I256> for i64 {
            type Output = I256;

            fn $method(self, other: I256) -> I256 {
                $trait::$method(I256::from(self), other)
            }
        }

        impl $assign for I256 {
            fn $assign_method(&mut self, other: I256) {
                *self = $trait::$method(*self, other);
            }
        }

        impl $assign<i64> for I256 {
            fn $assign_method(&mut self, other: i64) {
                *self = $trait::$method(*self, other);
            }
        }
    };
}

operator!(Add, add, AddAssign, add_assign, checked_add, false);
operator!(Sub, sub, SubAssign, sub_assign, checked_sub, false);
operator!(Mul, mul, MulAssign, mul_assign, checked_mul, false);
operator!(Div, div, DivAssign, div_assign, checked_div, true);
operator!(Rem, rem, RemAssign, rem_assign, checked_rem, true);

impl Neg for I256 {
    type Output = I256;

    fn neg(self) -> I256 {
        self.checked_neg().unwrap_or_else(|| fail("i256 overflow"))
    }
}

impl From<i64> for I256 {
    fn from(value: i64) -> I256 {
        I256::from(i128::from(value))
    }
}

impl From<i128> for I256 {
    fn from(value: i128) -> I256 {
        // the sign fills the upper half
        let fill = if value < 0 { u64::MAX } else { 0 };

        I256(U256::from_limbs([value as u64, (value >> 64) as u64, fill, fill]))
    }
}

impl From<u64> for I256 {
    fn from(value: u64) -> I256 {
        I256(U256::from(value))
    }
}

impl From<u128> for I256 {
    fn from(value: u128) -> I256 {
        I256(U256::from(value))
    }
}

impl TryFrom<U256> for I256 {
    type Error = OutOfRange;

    fn try_from(value: U256) -> Result<I256, OutOfRange> {
        I256::from_sign(false, value).ok_or(OutOfRange)
    }
}

impl TryFrom<I256> for U256 {
    type Error = OutOfRange;

    fn try_from(value: I256) -> Result<U256, OutOfRange> {
        if value.is_negative() { Err(OutOfRange) } else { Ok(value.0) }
    }
}

impl TryFrom<I256> for i128 {
    type Error = OutOfRange;

    fn try_from(value: I256) -> Result<i128, OutOfRange> {
        if value < I256::from(i128::MIN) || value > I256::from(i128::MAX) {
            return Err(OutOfRange);
        }

        let bytes = value.to_le_bytes();

        Ok(i128::from_le_bytes(bytes[..16].try_into().expect("16 bytes")))
    }
}

impl TryFrom<I256> for i64 {
    type Error = OutOfRange;

    fn try_from(value: I256) -> Result<i64, OutOfRange> {
        i64::try_from(i128::try_from(value)?).map_err(|_| OutOfRange)
    }
}

impl Ord for I256 {
    fn cmp(&self, other: &I256) -> Ordering {
        // within one sign, two's complement orders as the bits do
        match (self.is_negative(), other.is_negative()) {
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            _ => self.0.cmp(&other.0),
        }
    }
}

impl PartialOrd for I256 {
    fn partial_cmp(&self, other: &I256) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq<i64> for I256 {
    fn eq(&self, other: &i64) -> bool {
        *self == I256::from(*other)
    }
}

impl PartialEq<I256> for i64 {
    fn eq(&self, other: &I256) -> bool {
        I256::from(*self) == *other
    }
}

impl PartialOrd<i64> for I256 {
    fn partial_cmp(&self, other: &i64) -> Option<Ordering> {
        Some(self.cmp(&I256::from(*other)))
    }
}

impl PartialOrd<I256> for i64 {
    fn partial_cmp(&self, other: &I256) -> Option<Ordering> {
        Some(I256::from(*self).cmp(other))
    }
}

impl fmt::Display for I256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad_integral(!self.is_negative(), "", &self.unsigned_abs().to_decimal())
    }
}

impl fmt::Debug for I256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "I256({})", self.to_decimal())
    }
}

#[cfg(test)]
mod tests;
