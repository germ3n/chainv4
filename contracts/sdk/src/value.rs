//! Arguments and return values, encoded exactly as the node encodes its TxCallArgumentType.

use alloc::{string::String, vec::Vec};

/// An address: its kind and 32 bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Address {
    pub kind: AddressKind,
    pub bytes: [u8; 32],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum AddressKind {
    Ed25519 = 0,
    Contract = 1,
    EdBlake = 2,
}

impl Address {
    /// The 33 bytes the host reads and writes: kind, then bytes.
    pub fn encode(&self) -> [u8; 33] {
        let mut out = [0; 33];
        out[0] = self.kind as u8;
        out[1..].copy_from_slice(&self.bytes);

        out
    }

    pub fn decode(bytes: &[u8; 33]) -> Option<Address> {
        let kind = match bytes[0] {
            0 => AddressKind::Ed25519,
            1 => AddressKind::Contract,
            2 => AddressKind::EdBlake,
            _ => return None,
        };
        let mut out = [0; 32];
        out.copy_from_slice(&bytes[1..]);

        Some(Address { kind, bytes: out })
    }
}

/// An amount of the chain's currency.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Amount {
    U128(u128),
    /// Little-endian.
    U256([u8; 32]),
}

/// One argument or return value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Address(Address),
    Amount(Amount),
    Bytes(Vec<u8>),
    Bool(bool),
    String(String),
    /// A float, kept as its bits: contracts cannot do floating-point arithmetic, since machines
    /// may disagree on it. Lua passes its numbers this way; as_i64 reads whole ones.
    Number(u64),
    I64(i64),
    U64(u64),
    I128(i128),
    U128(u128),
    /// Little-endian.
    U256([u8; 32]),
}

impl Value {
    /// The value as a whole number, from any integer variant or a whole Number that fits.
    pub fn as_i64(&self) -> Option<i64> {
        match *self {
            Value::I64(value) => Some(value),
            Value::U64(value) => i64::try_from(value).ok(),
            Value::I128(value) => i64::try_from(value).ok(),
            Value::U128(value) | Value::Amount(Amount::U128(value)) => i64::try_from(value).ok(),
            Value::U256(bytes) | Value::Amount(Amount::U256(bytes)) => {
                let (low, high) = bytes.split_at(8);
                let low = i64::from_le_bytes(low.try_into().ok()?);

                (high.iter().all(|byte| *byte == 0) && low >= 0).then_some(low)
            }
            Value::Number(bits) => whole_number(bits),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(text) => Some(text),
            _ => None,
        }
    }

    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Value::Bytes(bytes) => Some(bytes),
            Value::String(text) => Some(text.as_bytes()),
            _ => None,
        }
    }

    pub fn as_address(&self) -> Option<Address> {
        match self {
            Value::Address(address) => Some(*address),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(flag) => Some(*flag),
            _ => None,
        }
    }
}

/// The integer an f64's bits hold, worked out with integer instructions only.
fn whole_number(bits: u64) -> Option<i64> {
    let negative = bits >> 63 == 1;
    let exponent = ((bits >> 52) & 0x7ff) as i64;
    let fraction = bits & ((1 << 52) - 1);

    if exponent == 0 {
        // Zero, or a subnormal, which is never whole.
        return (fraction == 0).then_some(0);
    }

    let shift = exponent - 1075;
    let significand = fraction | (1 << 52);
    let magnitude = if shift >= 0 {
        // Past 2^62, or infinity and NaN.
        if shift > 10 {
            return None;
        }

        significand << shift
    } else {
        if shift < -52 || significand & ((1 << -shift) - 1) != 0 {
            return None;
        }

        significand >> -shift
    };

    Some(if negative { -(magnitude as i64) } else { magnitude as i64 })
}

/// Encodes a list of values: its length as a u64, then each value's tag and contents.
pub fn encode(values: &[Value]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(values.len() as u64).to_le_bytes());

    for value in values {
        match value {
            Value::Address(address) => {
                out.push(0);
                out.extend_from_slice(&address.encode());
            }
            Value::Amount(Amount::U128(amount)) => {
                out.extend_from_slice(&[1, 0]);
                out.extend_from_slice(&amount.to_le_bytes());
            }
            Value::Amount(Amount::U256(amount)) => {
                out.extend_from_slice(&[1, 1]);
                out.extend_from_slice(amount);
            }
            Value::Bytes(bytes) => {
                out.push(2);
                out.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
                out.extend_from_slice(bytes);
            }
            Value::Bool(flag) => out.extend_from_slice(&[3, u8::from(*flag)]),
            Value::String(text) => {
                out.push(4);
                out.extend_from_slice(&(text.len() as u64).to_le_bytes());
                out.extend_from_slice(text.as_bytes());
            }
            Value::Number(bits) => {
                out.push(5);
                out.extend_from_slice(&bits.to_le_bytes());
            }
            Value::I64(number) => {
                out.push(6);
                out.extend_from_slice(&number.to_le_bytes());
            }
            Value::U64(number) => {
                out.push(7);
                out.extend_from_slice(&number.to_le_bytes());
            }
            Value::I128(number) => {
                out.push(8);
                out.extend_from_slice(&number.to_le_bytes());
            }
            Value::U128(number) => {
                out.push(9);
                out.extend_from_slice(&number.to_le_bytes());
            }
            Value::U256(number) => {
                out.push(10);
                out.extend_from_slice(number);
            }
        }
    }

    out
}

/// Decodes a list encoded by encode, or by the node.
pub fn decode(bytes: &[u8]) -> Option<Vec<Value>> {
    let mut reader = Reader(bytes);
    let count = reader.u64()?;
    let mut values = Vec::new();

    for _ in 0..count {
        let value = match reader.byte()? {
            0 => Value::Address(Address::decode(&reader.array()?)?),
            1 => match reader.byte()? {
                0 => Value::Amount(Amount::U128(u128::from_le_bytes(reader.array()?))),
                1 => Value::Amount(Amount::U256(reader.array()?)),
                _ => return None,
            },
            2 => Value::Bytes(reader.sized()?.to_vec()),
            3 => match reader.byte()? {
                0 => Value::Bool(false),
                1 => Value::Bool(true),
                _ => return None,
            },
            4 => Value::String(String::from_utf8(reader.sized()?.to_vec()).ok()?),
            5 => Value::Number(reader.u64()?),
            6 => Value::I64(i64::from_le_bytes(reader.array()?)),
            7 => Value::U64(reader.u64()?),
            8 => Value::I128(i128::from_le_bytes(reader.array()?)),
            9 => Value::U128(u128::from_le_bytes(reader.array()?)),
            10 => Value::U256(reader.array()?),
            _ => return None,
        };
        values.push(value);
    }

    reader.0.is_empty().then_some(values)
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, len: usize) -> Option<&'a [u8]> {
        if self.0.len() < len {
            return None;
        }

        let (head, rest) = self.0.split_at(len);
        self.0 = rest;

        Some(head)
    }

    fn byte(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }

    fn array<const N: usize>(&mut self) -> Option<[u8; N]> {
        self.take(N)?.try_into().ok()
    }

    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.array()?))
    }

    fn sized(&mut self) -> Option<&'a [u8]> {
        let len = usize::try_from(self.u64()?).ok()?;

        self.take(len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn whole_numbers_come_out_of_float_bits() {
        let cases: [(u64, Option<i64>); 8] = [
            (0.0f64.to_bits(), Some(0)),
            (5.0f64.to_bits(), Some(5)),
            ((-42.0f64).to_bits(), Some(-42)),
            (1.5f64.to_bits(), None),
            (0.25f64.to_bits(), None),
            (9007199254740992.0f64.to_bits(), Some(1 << 53)),
            (f64::INFINITY.to_bits(), None),
            (f64::NAN.to_bits(), None),
        ];

        for (bits, expected) in cases {
            assert_eq!(whole_number(bits), expected, "{bits:x}");
        }
    }

    #[test]
    fn values_round_trip() {
        let values = vec![
            Value::Address(Address { kind: AddressKind::EdBlake, bytes: [3; 32] }),
            Value::Amount(Amount::U128(7)),
            Value::Bytes(vec![1, 2]),
            Value::Bool(true),
            Value::String("hi".into()),
            Value::Number(2.5f64.to_bits()),
            Value::I64(-1),
            Value::U256([9; 32]),
        ];

        assert_eq!(decode(&encode(&values)), Some(values));
        assert_eq!(decode(&[1, 0, 0, 0, 0, 0, 0, 0, 99]), None);
    }
}
