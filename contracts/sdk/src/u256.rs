//! A 256-bit unsigned integer, for amounts. Integer instructions only: contracts may not use
//! floating point.

use core::cmp::Ordering;

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
}
