use {
    crate::crypto::hashable::Hashable,
    crate::crypto::keypair::curve_point,
    crate::crypto::verifiable::Verifiable,
    crate::crypto::verifiable::VerifyError,
    crate::structs::hex::{from_hex, to_hex},
    r#macro::document,
    std::fmt,
    std::str::FromStr,
    wincode::SchemaRead,
    wincode::SchemaWrite,
};

#[document(
    summary = "An account on the chain, named by the public key that controls it.",
    category = "Primitives",
    see_also = "Tx, Amount",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, std::hash::Hash, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
pub enum Address {
    #[document(summary = "An address backed by a 32-byte Ed25519 public key.")]
    #[wincode(tag = 0)]
    Ed25519([u8; 32]),
    #[document(
        summary = "A deployed contract. The bytes are derived from the deployer and their nonce, so no key controls it.",
        see_also = "Contracts::deploy",
    )]
    #[wincode(tag = 1)]
    Contract([u8; 32]),
    #[document(summary = "An address backed by a 32-byte EdBlake public key.")]
    #[wincode(tag = 2)]
    EdBlake([u8; 32]),
}

impl Address {
    #[document(
        parent = "Address",
        summary = "The 32 bytes behind the address, without saying which kind it is.",
    )]
    pub fn bytes(&self) -> &[u8; 32] {
        match self {
            Address::Ed25519(bytes) | Address::Contract(bytes) | Address::EdBlake(bytes) => bytes,
        }
    }

    #[document(
        parent = "Address",
        summary = "The name of the address's kind, as its text form begins: ed25519, edblake or contract.",
    )]
    pub fn kind(&self) -> &'static str {
        match self {
            Address::Ed25519(_) => "ed25519",
            Address::Contract(_) => "contract",
            Address::EdBlake(_) => "edblake",
        }
    }
}

/// The kind, a colon and the bytes in hex, such as ed25519:0x3b6a…. FromStr reads it back.
impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.kind(), to_hex(self.bytes()))
    }
}

impl FromStr for Address {
    type Err = ParseAddressError;

    fn from_str(text: &str) -> Result<Address, ParseAddressError> {
        let (kind, hex) = text.split_once(':').ok_or(ParseAddressError)?;
        let bytes: [u8; 32] = from_hex(hex)
            .and_then(|bytes| bytes.try_into().ok())
            .ok_or(ParseAddressError)?;

        match kind {
            "ed25519" => Ok(Address::Ed25519(bytes)),
            "contract" => Ok(Address::Contract(bytes)),
            "edblake" => Ok(Address::EdBlake(bytes)),
            _ => Err(ParseAddressError),
        }
    }
}

#[document(
    summary = "Text that is not an address: it must be a kind, a colon and 64 hex digits, such as ed25519:0x3b6a….",
    category = "Primitives",
    see_also = "Address::kind",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParseAddressError;

impl fmt::Display for ParseAddressError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "an address is ed25519:, edblake: or contract: and 64 hex digits")
    }
}

impl std::error::Error for ParseAddressError {}

impl Verifiable for Address {
    #[document(
        parent = "Address",
        summary = "Checks that an Ed25519 or EdBlake key is a point on the curve and not a weak small-order point, which would accept one signature for almost any message. A contract address is always valid.",
        see_also = "VerifyError::InvalidPublicKey",
    )]
    fn verify(&self) -> Result<(), VerifyError> {
        match self {
            Address::Ed25519(key) | Address::EdBlake(key) => curve_point(key),
            Address::Contract(_) => Ok(()),
        }
    }
}

impl Hashable for Address {
    const TAG_LEN: usize = 1;
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_round_trips_for_every_kind() {
        for address in [Address::Ed25519([1; 32]), Address::Contract([2; 32]), Address::EdBlake([3; 32])] {
            let text = address.to_string();

            assert!(text.starts_with(&format!("{}:0x", address.kind())), "{text}");
            assert_eq!(text.parse(), Ok(address));
        }
    }

    #[test]
    fn text_without_a_known_kind_or_32_bytes_is_refused() {
        let hex = "ab".repeat(32);

        for text in [hex.clone(), format!("rsa:{hex}"), "ed25519:0xabcd".to_string(), format!("ed25519:{hex}zz")] {
            assert_eq!(text.parse::<Address>(), Err(ParseAddressError), "{text}");
        }

        assert_eq!(format!("edblake:{hex}").parse(), Ok(Address::EdBlake([0xab; 32])));
    }
}
