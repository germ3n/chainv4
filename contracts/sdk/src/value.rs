//! Arguments and return values, encoded exactly as the node encodes its TxCallArgumentType.

use alloc::{string::String, vec::Vec};

/// An address: its kind and its bytes, 32 of them, or 20 for secp256k1 and EVM contracts with the
/// rest zero.
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
    /// Ethereum's: 20 bytes of keccak256 of a secp256k1 key.
    Secp256k1 = 3,
    /// An EVM contract's: the 20 bytes Ethereum's CREATE or CREATE2 gives it.
    Evm = 4,
}

impl AddressKind {
    fn from_tag(tag: u8) -> Option<AddressKind> {
        match tag {
            0 => Some(AddressKind::Ed25519),
            1 => Some(AddressKind::Contract),
            2 => Some(AddressKind::EdBlake),
            3 => Some(AddressKind::Secp256k1),
            4 => Some(AddressKind::Evm),
            _ => None,
        }
    }

    /// How many bytes an address of this kind has: 32, or 20 for secp256k1 and the EVM.
    pub fn width(self) -> usize {
        match self {
            AddressKind::Secp256k1 | AddressKind::Evm => 20,
            _ => 32,
        }
    }
}

impl Address {
    /// An address of a kind from its bytes, as many as the kind has.
    pub fn new(kind: AddressKind, bytes: &[u8]) -> Option<Address> {
        let mut out = [0; 32];
        out.get_mut(..kind.width())?.copy_from_slice(bytes.get(..kind.width()).filter(|_| bytes.len() == kind.width())?);

        Some(Address { kind, bytes: out })
    }

    /// Its bytes: 32, or 20 for secp256k1.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.kind.width()]
    }

    /// The 33 bytes the host reads and writes: the kind, its bytes, and zeros after a shorter
    /// kind's.
    pub fn encode(&self) -> [u8; 33] {
        let mut out = [0; 33];
        out[0] = self.kind as u8;
        out[1..1 + self.kind.width()].copy_from_slice(self.as_bytes());

        out
    }

    pub fn decode(bytes: &[u8; 33]) -> Option<Address> {
        let kind = AddressKind::from_tag(bytes[0])?;

        // One address, one form: zeros after a shorter kind's bytes.
        if bytes[1 + kind.width()..].iter().any(|byte| *byte != 0) {
            return None;
        }

        Address::new(kind, &bytes[1..1 + kind.width()])
    }

    /// The address in its text form, as the node writes it: its kind, a colon and 0x hex, such
    /// as `ed25519:0x3b6a…`. What the `address-text` kind holds.
    pub fn text(&self) -> String {
        let kind = match self.kind {
            AddressKind::Ed25519 => "ed25519",
            AddressKind::Contract => "contract",
            AddressKind::EdBlake => "edblake",
            AddressKind::Secp256k1 => "secp256k1",
            AddressKind::Evm => "evm",
        };
        let mut out = String::with_capacity(kind.len() + 67);
        out.push_str(kind);
        out.push(':');
        out.push_str(&self.hex());

        out
    }

    /// Reads the text form, as text writes it and Lua's address.parse reads it. None for anything
    /// else.
    pub fn parse(text: &str) -> Option<Address> {
        let (kind, hex) = text.split_once(':')?;
        let kind = match kind {
            "ed25519" => AddressKind::Ed25519,
            "contract" => AddressKind::Contract,
            "edblake" => AddressKind::EdBlake,
            "secp256k1" => AddressKind::Secp256k1,
            "evm" => AddressKind::Evm,
            _ => return None,
        };

        Address::new(kind, &from_hex(hex.strip_prefix("0x")?)?)
    }

    /// The address as `0x` and lowercase hex, as Lua's Address:hex writes it: what the
    /// `address-hex` kind holds. It keeps no kind.
    pub fn hex(&self) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut out = String::with_capacity(66);
        out.push_str("0x");

        for byte in self.as_bytes() {
            out.push(HEX[usize::from(byte >> 4)] as char);
            out.push(HEX[usize::from(byte & 15)] as char);
        }

        out
    }

    /// Reads `0x` and hex, as Lua's address(hex) does: 64 digits are an Ed25519 address, 40 a
    /// secp256k1 one, the hex form keeping no kind.
    pub fn from_hex(text: &str) -> Option<Address> {
        let bytes = from_hex(text.strip_prefix("0x")?)?;
        let kind = match bytes.len() {
            32 => AddressKind::Ed25519,
            20 => AddressKind::Secp256k1,
            _ => return None,
        };

        Address::new(kind, &bytes)
    }
}

/// Hex digits, either case, into bytes.
fn from_hex(hex: &str) -> Option<Vec<u8>> {
    let digit = |byte: u8| (byte as char).to_digit(16).map(|digit| digit as u8);
    let bytes = hex.as_bytes();

    if !bytes.len().is_multiple_of(2) {
        return None;
    }

    bytes.chunks(2).map(|pair| Some(digit(pair[0])? << 4 | digit(pair[1])?)).collect()
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
    /// No value: Lua's nil.
    Nil,
    /// Little-endian, two's complement.
    I256([u8; 32]),
}

impl From<crate::U256> for Value {
    fn from(value: crate::U256) -> Value {
        Value::U256(value.to_le_bytes())
    }
}

impl From<crate::I256> for Value {
    fn from(value: crate::I256) -> Value {
        Value::I256(value.to_le_bytes())
    }
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
            Value::I256(bytes) => i64::try_from(crate::I256::from_le_bytes(bytes)).ok(),
            _ => None,
        }
    }

    /// The value as an I256, from any integer variant, an Amount or a whole Number that fits.
    pub fn as_i256(&self) -> Option<crate::I256> {
        match *self {
            Value::I256(bytes) => Some(crate::I256::from_le_bytes(bytes)),
            Value::I64(value) => Some(crate::I256::from(value)),
            Value::I128(value) => Some(crate::I256::from(value)),
            Value::Number(bits) => whole_number(bits).map(crate::I256::from),
            _ => crate::I256::try_from(self.as_u256()?).ok(),
        }
    }

    /// The value as an amount, a little-endian u256, from an Amount, any non-negative integer
    /// or a whole non-negative Number.
    pub fn as_amount(&self) -> Option<[u8; 32]> {
        let small = match *self {
            Value::Amount(Amount::U256(bytes)) | Value::U256(bytes) => return Some(bytes),
            Value::Amount(Amount::U128(value)) | Value::U128(value) => value,
            Value::U64(value) => u128::from(value),
            Value::I64(value) => u128::try_from(value).ok()?,
            Value::I128(value) => u128::try_from(value).ok()?,
            Value::Number(bits) => u128::try_from(whole_number(bits)?).ok()?,
            Value::I256(bytes) => return crate::U256::try_from(crate::I256::from_le_bytes(bytes)).ok().map(crate::U256::to_le_bytes),
            _ => return None,
        };

        Some(amount(small))
    }

    /// The value as a u128, from anything as_amount takes that fits.
    pub fn as_u128(&self) -> Option<u128> {
        let bytes = self.as_amount()?;
        let mut low = [0; 16];
        low.copy_from_slice(&bytes[..16]);

        bytes[16..].iter().all(|byte| *byte == 0).then(|| u128::from_le_bytes(low))
    }

    /// The value as a U256, from anything as_amount takes.
    pub fn as_u256(&self) -> Option<crate::U256> {
        self.as_amount().map(crate::U256::from_le_bytes)
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

/// An amount as the host reads it: a little-endian u256.
pub fn amount(value: u128) -> [u8; 32] {
    let mut out = [0; 32];
    out[..16].copy_from_slice(&value.to_le_bytes());

    out
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
                out.extend_from_slice(&[0, address.kind as u8]);
                out.extend_from_slice(address.as_bytes());
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
            Value::Nil => out.push(11),
            Value::I256(number) => {
                out.push(12);
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
            0 => {
                let kind = AddressKind::from_tag(reader.byte()?)?;

                Value::Address(Address::new(kind, reader.take(kind.width())?)?)
            }
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
            11 => Value::Nil,
            12 => Value::I256(reader.array()?),
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
    fn addresses_read_back_from_text_and_hex_as_lua_writes_them() {
        let mut bytes = [0; 32];
        bytes[0] = 0xab;
        bytes[31] = 0x01;
        let edblake = Address::new(AddressKind::EdBlake, &bytes).unwrap();
        let ethereum = Address::new(AddressKind::Secp256k1, &[0xff; 20]).unwrap();

        assert_eq!(edblake.text(), alloc::format!("edblake:0xab{}01", "00".repeat(30)));
        assert_eq!(Address::parse(&edblake.text()), Some(edblake));
        assert_eq!(Address::parse(&ethereum.text().to_uppercase().replace("SECP256K1:0X", "secp256k1:0x")), Some(ethereum), "either case");
        assert_eq!(ethereum.hex(), alloc::format!("0x{}", "ff".repeat(20)));
        assert_eq!(Address::from_hex(&ethereum.hex()), Some(ethereum));
        assert_eq!(Address::from_hex(&edblake.hex()).map(|address| address.kind), Some(AddressKind::Ed25519), "hex keeps no kind");

        for bad in ["edblake:ab", "ed25519:0x12", "nokind:0x00", "ed25519:0xzz", "0x"] {
            assert_eq!(Address::parse(bad), None, "{bad}");
        }

        assert_eq!(Address::from_hex("0x1234"), None);
    }

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
            Value::Address(Address::new(AddressKind::Secp256k1, &[4; 20]).unwrap()),
            Value::Amount(Amount::U128(7)),
            Value::Bytes(vec![1, 2]),
            Value::Bool(true),
            Value::String("hi".into()),
            Value::Number(2.5f64.to_bits()),
            Value::I64(-1),
            Value::U256([9; 32]),
            Value::Nil,
        ];

        assert_eq!(decode(&encode(&values)), Some(values));
        assert_eq!(decode(&[1, 0, 0, 0, 0, 0, 0, 0, 99]), None);
    }

    #[test]
    fn a_secp256k1_address_is_20_bytes_in_values_and_padded_in_memory() {
        let address = Address::new(AddressKind::Secp256k1, &[4; 20]).unwrap();
        let encoded = encode(&[Value::Address(address)]);

        // As the node encodes it: the value's tag, the kind's, and its 20 bytes.
        assert_eq!(&encoded[8..], &[&[0, 3][..], &[4; 20]].concat()[..]);
        assert_eq!(address.text(), alloc::format!("secp256k1:0x{}", "04".repeat(20)));

        let slot = address.encode();

        assert_eq!((&slot[1..21], &slot[21..]), (&[4; 20][..], &[0; 12][..]));
        assert_eq!(Address::decode(&slot), Some(address));

        let mut padded = slot;
        padded[25] = 1;

        assert_eq!(Address::decode(&padded), None, "zeros after the 20 bytes, so one address has one form");
        assert_eq!(Address::new(AddressKind::Secp256k1, &[4; 32]), None);
    }
}
