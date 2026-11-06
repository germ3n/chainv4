//! A 256-bit unsigned integer, for amounts. Integer instructions only: contracts may not use
//! floating point.
//!
//! The operators abort the call on overflow, underflow and division by zero, with the messages a
//! Lua contract's U256 raises; the checked_ methods return None instead, the saturating_ ones stop
//! at 0 and MAX, and the wrapping_ ones wrap around, as Rust's integers' do. A u64 works on
//! either side of an operator or a comparison: `amount * 3`, `amount > 1000`, `1000 < amount`.

use {
    alloc::{string::String, vec::Vec},
    core::{
        cmp::Ordering,
        fmt,
        ops::{Add, AddAssign, Div, DivAssign, Mul, MulAssign, Rem, RemAssign, Sub, SubAssign},
    },
};

/// Four 64-bit limbs, least significant first.
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct U256([u64; 4]);

/// A value too large, or below zero, for the type it was converted to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutOfRange;

impl fmt::Display for OutOfRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("out of range for the type")
    }
}

impl U256 {
    pub const ZERO: U256 = U256([0; 4]);
    pub const ONE: U256 = U256([1, 0, 0, 0]);
    pub const MIN: U256 = U256::ZERO;
    pub const MAX: U256 = U256([u64::MAX; 4]);

    pub(crate) const fn from_limbs(limbs: [u64; 4]) -> U256 {
        U256(limbs)
    }

    /// From the 32 little-endian bytes the host reads and writes.
    pub fn from_le_bytes(bytes: [u8; 32]) -> U256 {
        let mut limbs = [0; 4];

        for (index, limb) in limbs.iter_mut().enumerate() {
            *limb = u64::from_le_bytes(bytes[index * 8..index * 8 + 8].try_into().expect("8 bytes"));
        }

        U256(limbs)
    }

    pub fn to_le_bytes(self) -> [u8; 32] {
        let mut bytes = [0; 32];

        for (index, limb) in self.0.iter().enumerate() {
            bytes[index * 8..index * 8 + 8].copy_from_slice(&limb.to_le_bytes());
        }

        bytes
    }

    pub fn is_zero(self) -> bool {
        self == U256::ZERO
    }

    pub fn leading_zeros(self) -> u32 {
        let mut zeros = 0;

        for limb in self.0.iter().rev() {
            zeros += limb.leading_zeros();

            if *limb != 0 {
                break;
            }
        }

        zeros
    }

    /// The sum, and whether it wrapped past MAX.
    pub fn overflowing_add(self, other: U256) -> (U256, bool) {
        let mut out = [0; 4];
        let mut carry = false;

        for (index, limb) in out.iter_mut().enumerate() {
            let (sum, first) = self.0[index].overflowing_add(other.0[index]);
            let (sum, second) = sum.overflowing_add(u64::from(carry));
            *limb = sum;
            carry = first || second;
        }

        (U256(out), carry)
    }

    /// The difference, and whether it wrapped below zero.
    pub fn overflowing_sub(self, other: U256) -> (U256, bool) {
        let mut out = [0; 4];
        let mut borrow = false;

        for (index, limb) in out.iter_mut().enumerate() {
            let (difference, first) = self.0[index].overflowing_sub(other.0[index]);
            let (difference, second) = difference.overflowing_sub(u64::from(borrow));
            *limb = difference;
            borrow = first || second;
        }

        (U256(out), borrow)
    }

    /// The low 256 bits of the product, and whether any were cut off.
    pub fn overflowing_mul(self, other: U256) -> (U256, bool) {
        let mut out = [0u64; 8];

        for (i, left) in self.0.iter().enumerate() {
            let mut carry = 0u128;

            for (j, right) in other.0.iter().enumerate() {
                // at most (2^64 - 1)^2 + 2 * (2^64 - 1), which is 2^128 - 1
                let current = u128::from(*left) * u128::from(*right) + u128::from(out[i + j]) + carry;
                out[i + j] = current as u64;
                carry = current >> 64;
            }

            out[i + 4] = carry as u64;
        }

        (U256([out[0], out[1], out[2], out[3]]), out[4..].iter().any(|limb| *limb != 0))
    }

    pub fn checked_add(self, other: U256) -> Option<U256> {
        let (sum, overflow) = self.overflowing_add(other);

        (!overflow).then_some(sum)
    }

    pub fn checked_sub(self, other: U256) -> Option<U256> {
        let (difference, overflow) = self.overflowing_sub(other);

        (!overflow).then_some(difference)
    }

    pub fn checked_mul(self, other: U256) -> Option<U256> {
        let (product, overflow) = self.overflowing_mul(other);

        (!overflow).then_some(product)
    }

    /// Rounds down. None for division by zero.
    pub fn checked_div(self, other: U256) -> Option<U256> {
        self.div_rem(other).map(|(quotient, _)| quotient)
    }

    /// None for division by zero.
    pub fn checked_rem(self, other: U256) -> Option<U256> {
        self.div_rem(other).map(|(_, remainder)| remainder)
    }

    pub fn checked_pow(self, exponent: u32) -> Option<U256> {
        let mut base = self;
        let mut result = U256::ONE;
        let mut exponent = exponent;

        while exponent > 0 {
            if exponent & 1 == 1 {
                result = result.checked_mul(base)?;
            }

            exponent >>= 1;

            // squaring past the last bit could overflow where the result does not
            if exponent > 0 {
                base = base.checked_mul(base)?;
            }
        }

        Some(result)
    }

    pub fn saturating_add(self, other: U256) -> U256 {
        self.checked_add(other).unwrap_or(U256::MAX)
    }

    pub fn saturating_sub(self, other: U256) -> U256 {
        self.checked_sub(other).unwrap_or(U256::ZERO)
    }

    pub fn saturating_mul(self, other: U256) -> U256 {
        self.checked_mul(other).unwrap_or(U256::MAX)
    }

    pub fn saturating_pow(self, exponent: u32) -> U256 {
        self.checked_pow(exponent).unwrap_or(U256::MAX)
    }

    pub fn wrapping_add(self, other: U256) -> U256 {
        self.overflowing_add(other).0
    }

    pub fn wrapping_sub(self, other: U256) -> U256 {
        self.overflowing_sub(other).0
    }

    pub fn wrapping_mul(self, other: U256) -> U256 {
        self.overflowing_mul(other).0
    }

    pub fn wrapping_pow(self, exponent: u32) -> U256 {
        let mut base = self;
        let mut result = U256::ONE;
        let mut exponent = exponent;

        while exponent > 0 {
            if exponent & 1 == 1 {
                result = result.wrapping_mul(base);
            }

            exponent >>= 1;
            base = base.wrapping_mul(base);
        }

        result
    }

    /// Some only for zero: no other U256 has a negation.
    pub fn checked_neg(self) -> Option<U256> {
        self.is_zero().then_some(self)
    }

    /// 2^256 - self: the negation, modulo 2^256.
    pub fn wrapping_neg(self) -> U256 {
        U256::ZERO.wrapping_sub(self)
    }

    /// Raises to a power, aborting the call on overflow, as ^ does in Lua.
    pub fn pow(self, exponent: u32) -> U256 {
        self.checked_pow(exponent).unwrap_or_else(|| fail("u256 overflow"))
    }

    /// The value in decimal digits, as Lua's tostring writes a U256: what the `decimal` kind
    /// holds.
    pub fn to_decimal(self) -> String {
        let mut digits = Vec::new();
        let mut rest = self;

        loop {
            let (quotient, digit) = rest.div_rem_small(10);
            digits.push(b'0' + digit as u8);
            rest = quotient;

            if rest.is_zero() {
                break;
            }
        }

        digits.reverse();

        String::from_utf8(digits).expect("ASCII digits")
    }

    /// Reads decimal digits, as Lua's u256(text) does, grouped by `_` as in 1_000_000, each `_`
    /// between two digits: None for anything else, such as _1, 1_ or 1__0, or for more than MAX.
    pub fn from_decimal(text: &str) -> Option<U256> {
        let bytes = text.as_bytes();

        if bytes.is_empty() {
            return None;
        }

        let mut value = U256::ZERO;

        for (at, byte) in bytes.iter().enumerate() {
            if *byte == b'_' {
                let between = at > 0 && bytes[at - 1].is_ascii_digit() && bytes.get(at + 1).is_some_and(u8::is_ascii_digit);

                if !between {
                    return None;
                }

                continue;
            }

            if !byte.is_ascii_digit() {
                return None;
            }

            value = value.checked_mul_small(10)?.checked_add(U256::from(u64::from(byte - b'0')))?;
        }

        Some(value)
    }

    /// The quotient and remainder, or None for division by zero.
    fn div_rem(self, divisor: U256) -> Option<(U256, U256)> {
        if divisor.is_zero() {
            return None;
        }

        if self < divisor {
            return Some((U256::ZERO, self));
        }

        if divisor.0[1..].iter().all(|limb| *limb == 0) {
            let (quotient, remainder) = self.div_rem_small(divisor.0[0]);

            return Some((quotient, U256::from(remainder)));
        }

        // Long division a bit at a time, from the divisor lined up under the dividend's top bit:
        // a divisor past 64 bits leaves at most 192 steps.
        let shift = divisor.leading_zeros() - self.leading_zeros();
        let mut remainder = self;
        let mut quotient = U256::ZERO;
        let mut shifted = divisor.shl(shift);

        for bit in (0..=shift).rev() {
            if remainder >= shifted {
                remainder = remainder.wrapping_sub(shifted);
                quotient.0[bit as usize / 64] |= 1 << (bit % 64);
            }

            shifted = shifted.shr(1);
        }

        Some((quotient, remainder))
    }

    fn div_rem_small(self, divisor: u64) -> (U256, u64) {
        let mut out = [0; 4];
        let mut remainder = 0u128;

        for index in (0..4).rev() {
            let current = (remainder << 64) | u128::from(self.0[index]);
            out[index] = (current / u128::from(divisor)) as u64;
            remainder = current % u128::from(divisor);
        }

        (U256(out), remainder as u64)
    }

    fn checked_mul_small(self, factor: u64) -> Option<U256> {
        let mut out = [0; 4];
        let mut carry = 0u128;

        for (index, limb) in out.iter_mut().enumerate() {
            let product = u128::from(self.0[index]) * u128::from(factor) + carry;
            *limb = product as u64;
            carry = product >> 64;
        }

        (carry == 0).then_some(U256(out))
    }

    /// Shifts left by fewer than 256 bits, dropping what goes past the top.
    fn shl(self, bits: u32) -> U256 {
        let (limbs, bits) = ((bits / 64) as usize, bits % 64);
        let mut out = [0; 4];

        for (index, limb) in out.iter_mut().enumerate().skip(limbs) {
            *limb = self.0[index - limbs] << bits;

            if bits > 0 && index > limbs {
                *limb |= self.0[index - limbs - 1] >> (64 - bits);
            }
        }

        U256(out)
    }

    /// Shifts right by fewer than 256 bits.
    fn shr(self, bits: u32) -> U256 {
        let (limbs, bits) = ((bits / 64) as usize, bits % 64);
        let mut out = [0; 4];

        for (index, limb) in out.iter_mut().enumerate().take(4 - limbs) {
            *limb = self.0[index + limbs] >> bits;

            if bits > 0 && index + limbs + 1 < 4 {
                *limb |= self.0[index + limbs + 1] << (64 - bits);
            }
        }

        U256(out)
    }
}

/// Aborts the call: an operator overflowed or divided by zero.
#[cold]
pub(crate) fn fail(message: &str) -> ! {
    panic!("{message}")
}

macro_rules! operator {
    ($trait:ident, $method:ident, $assign:ident, $assign_method:ident, $checked:ident, $failure:literal) => {
        impl $trait for U256 {
            type Output = U256;

            fn $method(self, other: U256) -> U256 {
                self.$checked(other).unwrap_or_else(|| fail($failure))
            }
        }

        impl $trait<u64> for U256 {
            type Output = U256;

            fn $method(self, other: u64) -> U256 {
                $trait::$method(self, U256::from(other))
            }
        }

        impl $trait<U256> for u64 {
            type Output = U256;

            fn $method(self, other: U256) -> U256 {
                $trait::$method(U256::from(self), other)
            }
        }

        impl $assign for U256 {
            fn $assign_method(&mut self, other: U256) {
                *self = $trait::$method(*self, other);
            }
        }

        impl $assign<u64> for U256 {
            fn $assign_method(&mut self, other: u64) {
                *self = $trait::$method(*self, other);
            }
        }
    };
}

operator!(Add, add, AddAssign, add_assign, checked_add, "u256 overflow");
operator!(Sub, sub, SubAssign, sub_assign, checked_sub, "u256 underflow");
operator!(Mul, mul, MulAssign, mul_assign, checked_mul, "u256 overflow");
operator!(Div, div, DivAssign, div_assign, checked_div, "division by zero");
operator!(Rem, rem, RemAssign, rem_assign, checked_rem, "division by zero");

impl From<u128> for U256 {
    fn from(value: u128) -> U256 {
        U256([value as u64, (value >> 64) as u64, 0, 0])
    }
}

impl From<u64> for U256 {
    fn from(value: u64) -> U256 {
        U256([value, 0, 0, 0])
    }
}

impl TryFrom<U256> for u64 {
    type Error = OutOfRange;

    fn try_from(value: U256) -> Result<u64, OutOfRange> {
        if value.0[1..].iter().any(|limb| *limb != 0) {
            return Err(OutOfRange);
        }

        Ok(value.0[0])
    }
}

impl TryFrom<U256> for u128 {
    type Error = OutOfRange;

    fn try_from(value: U256) -> Result<u128, OutOfRange> {
        if value.0[2..].iter().any(|limb| *limb != 0) {
            return Err(OutOfRange);
        }

        Ok(u128::from(value.0[0]) | u128::from(value.0[1]) << 64)
    }
}

impl Ord for U256 {
    fn cmp(&self, other: &U256) -> Ordering {
        self.0.iter().rev().cmp(other.0.iter().rev())
    }
}

impl PartialOrd for U256 {
    fn partial_cmp(&self, other: &U256) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq<u64> for U256 {
    fn eq(&self, other: &u64) -> bool {
        *self == U256::from(*other)
    }
}

impl PartialEq<U256> for u64 {
    fn eq(&self, other: &U256) -> bool {
        U256::from(*self) == *other
    }
}

impl PartialOrd<u64> for U256 {
    fn partial_cmp(&self, other: &u64) -> Option<Ordering> {
        Some(self.cmp(&U256::from(*other)))
    }
}

impl PartialOrd<U256> for u64 {
    fn partial_cmp(&self, other: &U256) -> Option<Ordering> {
        Some(U256::from(*self).cmp(other))
    }
}

impl fmt::Display for U256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad_integral(true, "", &self.to_decimal())
    }
}

impl fmt::Debug for U256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "U256({})", self.to_decimal())
    }
}

#[cfg(test)]
mod tests;
