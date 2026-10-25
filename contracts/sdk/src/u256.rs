//! A 256-bit unsigned integer, for amounts. Integer instructions only: contracts may not use
//! floating point.

use {
    alloc::{string::String, vec::Vec},
    core::cmp::Ordering,
};

/// Four 64-bit limbs, least significant first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct U256([u64; 4]);

impl U256 {
    pub const ZERO: U256 = U256([0; 4]);
    pub const MAX: U256 = U256([u64::MAX; 4]);

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

    pub fn checked_add(self, other: U256) -> Option<U256> {
        let mut out = [0; 4];
        let mut carry = false;

        for (index, limb) in out.iter_mut().enumerate() {
            let (sum, first) = self.0[index].overflowing_add(other.0[index]);
            let (sum, second) = sum.overflowing_add(u64::from(carry));
            *limb = sum;
            carry = first || second;
        }

        (!carry).then_some(U256(out))
    }

    pub fn checked_sub(self, other: U256) -> Option<U256> {
        let mut out = [0; 4];
        let mut borrow = false;

        for (index, limb) in out.iter_mut().enumerate() {
            let (difference, first) = self.0[index].overflowing_sub(other.0[index]);
            let (difference, second) = difference.overflowing_sub(u64::from(borrow));
            *limb = difference;
            borrow = first || second;
        }

        (!borrow).then_some(U256(out))
    }

    pub fn is_zero(self) -> bool {
        self == U256::ZERO
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

    /// Reads decimal digits, as Lua's u256(text) does: None for anything else, or for more than
    /// MAX.
    pub fn from_decimal(text: &str) -> Option<U256> {
        if text.is_empty() {
            return None;
        }

        let mut value = U256::ZERO;

        for byte in text.bytes() {
            if !byte.is_ascii_digit() {
                return None;
            }

            value = value.checked_mul_small(10)?.checked_add(U256::from(u64::from(byte - b'0')))?;
        }

        Some(value)
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
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arithmetic_carries_across_limbs_and_stops_at_the_edges() {
        let big = U256::from(u128::MAX);
        let one = U256::from(1u64);
        let sum = big.checked_add(one).unwrap();

        assert_eq!(sum, U256([0, 0, 1, 0]));
        assert_eq!(sum.checked_sub(one), Some(big));
        assert_eq!(U256::MAX.checked_add(one), None);
        assert_eq!(U256::ZERO.checked_sub(one), None);
        assert!(sum > big && big > one && U256::MAX > sum);
        assert_eq!(U256::from_le_bytes(sum.to_le_bytes()), sum);
    }

    #[test]
    fn decimals_read_and_write_as_lua_writes_them() {
        let max = "115792089237316195423570985008687907853269984665640564039457584007913129639935";

        assert_eq!(U256::ZERO.to_decimal(), "0");
        assert_eq!(U256::from(1_000_000_000_000_000_000u64).to_decimal(), "1000000000000000000");
        assert_eq!(U256::MAX.to_decimal(), max);
        assert_eq!(U256::from_decimal(max), Some(U256::MAX));
        assert_eq!(U256::from_decimal("340282366920938463463374607431768211456"), Some(U256([0, 0, 1, 0])));
        assert_eq!(U256::from_decimal("007"), Some(U256::from(7u64)));

        for bad in ["", "-1", "1.5", "0x10", " 1", "115792089237316195423570985008687907853269984665640564039457584007913129639936"] {
            assert_eq!(U256::from_decimal(bad), None, "{bad}");
        }
    }
}
