use {
    crate::crypto::hashable::Hashable,
    crate::crypto::keypair::curve_point,
    crate::crypto::verifiable::Verifiable,
    crate::crypto::verifiable::VerifyError,
    crate::structs::bech32,
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
    #[document(
        summary = "An address of a secp256k1 key, as Ethereum makes it: the last 20 bytes of the keccak256 of the key's point. The same key has the same 0x address here as on Ethereum.",
        see_also = "PublicKey::address",
    )]
    #[wincode(tag = 3)]
    Secp256k1([u8; 20]),
    #[document(
        summary = "An EVM contract, at the 20-byte address Ethereum gives it: CREATE's from the deployer's address and nonce, or CREATE2's.",
        see_also = "Engine::Evm",
    )]
    #[wincode(tag = 4)]
    Evm([u8; 20]),
    #[document(
        summary = "An address of a Bitcoin key, as Bitcoin's native segwit (P2WPKH) addresses are made: the HASH160 of its compressed key, written bc1q…. The same key has the same bc1q address here as in Bitcoin's wallets.",
        see_also = "KeyType::Bitcoin",
    )]
    #[wincode(tag = 5)]
    Bitcoin([u8; 20]),
}

impl Address {
    #[document(
        parent = "Address",
        summary = "The bytes behind the address, without saying which kind it is: 32, or 20 for secp256k1, bitcoin and EVM contracts.",
    )]
    pub fn bytes(&self) -> &[u8] {
        match self {
            Address::Ed25519(bytes) | Address::Contract(bytes) | Address::EdBlake(bytes) => bytes,
            Address::Secp256k1(bytes) | Address::Evm(bytes) | Address::Bitcoin(bytes) => bytes,
        }
    }

    #[document(
        parent = "Address",
        summary = "The 33 bytes a WebAssembly contract's memory holds an address in: its kind's tag, its bytes, and zeros after a kind shorter than 32, as the SDK's Address::encode writes them.",
        see_also = "Address::from_abi",
    )]
    pub fn to_abi(&self) -> [u8; 33] {
        let mut out = [0; 33];
        let tag = self.hashable_bytes_with_tag()[0];
        out[0] = tag;
        out[1..1 + self.bytes().len()].copy_from_slice(self.bytes());

        out
    }

    #[document(
        parent = "Address",
        summary = "Reads the 33 bytes to_abi writes. None for an unknown kind, or a shorter kind not followed by zeros, so one address has one form.",
        see_also = "Address::to_abi",
    )]
    pub fn from_abi(bytes: &[u8]) -> Option<Address> {
        let bytes: &[u8; 33] = bytes.try_into().ok()?;
        let wide = || <[u8; 32]>::try_from(&bytes[1..]).ok();

        match bytes[0] {
            0 => wide().map(Address::Ed25519),
            1 => wide().map(Address::Contract),
            2 => wide().map(Address::EdBlake),
            3 if bytes[21..].iter().all(|byte| *byte == 0) => bytes[1..21].try_into().ok().map(Address::Secp256k1),
            4 if bytes[21..].iter().all(|byte| *byte == 0) => bytes[1..21].try_into().ok().map(Address::Evm),
            5 if bytes[21..].iter().all(|byte| *byte == 0) => bytes[1..21].try_into().ok().map(Address::Bitcoin),
            _ => None,
        }
    }

    #[document(
        parent = "Address",
        summary = "The name of the address's kind, as its text form begins: ed25519, edblake, secp256k1, contract or evm; or bitcoin, whose text is bc1q… alone.",
    )]
    pub fn kind(&self) -> &'static str {
        match self {
            Address::Ed25519(_) => "ed25519",
            Address::Contract(_) => "contract",
            Address::EdBlake(_) => "edblake",
            Address::Secp256k1(_) => "secp256k1",
            Address::Evm(_) => "evm",
            Address::Bitcoin(_) => "bitcoin",
        }
    }
}

/// The kind, a colon and the bytes in hex, such as ed25519:0x3b6a…; a bitcoin address as Bitcoin
/// writes it, bc1q… in bech32, which says its own kind. FromStr reads it back.
impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Address::Bitcoin(bytes) => f.write_str(&bech32::encode_segwit(BITCOIN_HRP, bytes)),
            _ => write!(f, "{}:{}", self.kind(), to_hex(self.bytes())),
        }
    }
}

/// The human-readable part of a bitcoin address: mainnet's, bc, on every chain.
const BITCOIN_HRP: &str = "bc";

impl FromStr for Address {
    type Err = ParseAddressError;

    fn from_str(text: &str) -> Result<Address, ParseAddressError> {
        let Some((kind, hex)) = text.split_once(':') else {
            return match bech32::decode_segwit(text) {
                Some((hrp, program)) if hrp == BITCOIN_HRP => program.try_into().map(Address::Bitcoin).map_err(|_| ParseAddressError),
                _ => Err(ParseAddressError),
            };
        };
        let bytes = from_hex(hex).ok_or(ParseAddressError)?;
        let wide = || <[u8; 32]>::try_from(bytes.as_slice()).map_err(|_| ParseAddressError);

        match kind {
            "ed25519" => Ok(Address::Ed25519(wide()?)),
            "contract" => Ok(Address::Contract(wide()?)),
            "edblake" => Ok(Address::EdBlake(wide()?)),
            "secp256k1" => bytes.try_into().map(Address::Secp256k1).map_err(|_| ParseAddressError),
            "evm" => bytes.try_into().map(Address::Evm).map_err(|_| ParseAddressError),
            _ => Err(ParseAddressError),
        }
    }
}

#[document(
    summary = "Text that is not an address: it must be a kind, a colon and 64 hex digits, such as ed25519:0x3b6a…, or 40 for secp256k1 and evm; or a bitcoin address, bc1q… and 38 more letters.",
    category = "Primitives",
    see_also = "Address::kind",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParseAddressError;

impl fmt::Display for ParseAddressError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "an address is ed25519:, edblake: or contract: and 64 hex digits, secp256k1: or evm: and 40, or a bc1q… bitcoin address")
    }
}

impl std::error::Error for ParseAddressError {}

impl Verifiable for Address {
    #[document(
        parent = "Address",
        summary = "Checks that an Ed25519 or EdBlake key is a point on the curve and not a weak small-order point, which would accept one signature for almost any message. A contract address, and a secp256k1 or bitcoin one, a hash, is always valid.",
        see_also = "VerifyError::InvalidPublicKey",
    )]
    fn verify(&self) -> Result<(), VerifyError> {
        match self {
            Address::Ed25519(key) | Address::EdBlake(key) => curve_point(key),
            Address::Contract(_) | Address::Secp256k1(_) | Address::Evm(_) | Address::Bitcoin(_) => Ok(()),
        }
    }
}

impl Hashable for Address {
    const TAG_LEN: usize = 1;
}
#[document(
    summary = "Where a deploy puts its contract: the chain hash of a domain tag, the deployer's address and their nonce. The same deployer and nonce always give the same address on a chain, and no key controls it.",
    category = "Primitives",
    params = {
        deployer = "Who deploys.",
        nonce = "The deploy's nonce.",
        hash = "The chain's hash: any secure, 256-bit one.",
    },
    see_also = "Address::Contract",
)]
pub fn contract_address(deployer: &Address, nonce: u64, hash: crate::structs::hash::HashType) -> Address {
    let mut hasher = crate::crypto::hasher::Hasher::new(hash);
    hasher.update(b"chainv4 contract address");
    hasher.update(&deployer.hashable_bytes_with_tag());
    hasher.update(&nonce.to_le_bytes());
    let bytes: [u8; 32] = hasher
        .finalize()
        .to_bytes()
        .try_into()
        .expect("genesis only allows 256-bit chain hashes");

    Address::Contract(bytes)
}

#[document(
    summary = "Where contracts.clone puts a contract: the chain hash of a domain tag, the address of the contract that clones and a salt it picks. The same creator and salt always give the same address on a chain, so a contract can find what it cloned again without keeping it, and no key controls it.",
    category = "Primitives",
    params = {
        creator = "The contract that clones.",
        salt = "Its pick: 32 bytes, such as the hash of what the clone is for.",
        hash = "The chain's hash: any secure, 256-bit one.",
    },
    see_also = "Address::Contract, contract_address, Host::clone_contract",
)]
pub fn clone_address(creator: &Address, salt: &[u8; 32], hash: crate::structs::hash::HashType) -> Address {
    let mut hasher = crate::crypto::hasher::Hasher::new(hash);
    hasher.update(b"chainv4 clone address");
    hasher.update(&creator.hashable_bytes_with_tag());
    hasher.update(salt);
    let bytes: [u8; 32] = hasher
        .finalize()
        .to_bytes()
        .try_into()
        .expect("genesis only allows 256-bit chain hashes");

    Address::Contract(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_round_trips_for_every_kind() {
        for address in [Address::Ed25519([1; 32]), Address::Contract([2; 32]), Address::EdBlake([3; 32]), Address::Secp256k1([4; 20]), Address::Evm([5; 20])] {
            let text = address.to_string();

            assert!(text.starts_with(&format!("{}:0x", address.kind())), "{text}");
            assert_eq!(text.parse(), Ok(address));
        }
    }

    #[test]
    fn contracts_memory_holds_every_kind_in_33_bytes() {
        for address in [Address::Ed25519([1; 32]), Address::Contract([2; 32]), Address::EdBlake([3; 32]), Address::Secp256k1([4; 20]), Address::Evm([5; 20]), Address::Bitcoin([6; 20])] {
            let abi = address.to_abi();

            assert_eq!(abi[0], wincode::serialize(&address).unwrap()[0], "the same tag as the encoding");
            assert_eq!(Address::from_abi(&abi), Some(address));
        }

        let mut padded = Address::Secp256k1([4; 20]).to_abi();
        padded[30] = 1;

        assert_eq!(Address::from_abi(&padded), None, "a shorter kind is followed by zeros only");
        assert_eq!(Address::from_abi(&[9; 33]), None);
        assert_eq!(Address::from_abi(&[0; 32]), None);
    }

    #[test]
    fn text_without_a_known_kind_or_32_bytes_is_refused() {
        let hex = "ab".repeat(32);

        for text in [hex.clone(), format!("rsa:{hex}"), "ed25519:0xabcd".to_string(), format!("ed25519:{hex}zz")] {
            assert_eq!(text.parse::<Address>(), Err(ParseAddressError), "{text}");
        }

        assert_eq!(format!("edblake:{hex}").parse(), Ok(Address::EdBlake([0xab; 32])));

        // secp256k1 addresses are 20 bytes, and only they are.
        let short = "cd".repeat(20);

        assert_eq!(format!("secp256k1:0x{short}").parse(), Ok(Address::Secp256k1([0xcd; 20])));
        assert_eq!(format!("secp256k1:{hex}").parse::<Address>(), Err(ParseAddressError));
        assert_eq!(format!("ed25519:0x{short}").parse::<Address>(), Err(ParseAddressError));
    }

    #[test]
    fn bitcoin_addresses_are_written_as_bitcoin_writes_them() {
        // BIP173's P2WPKH example.
        let text = "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4";
        let address: Address = text.parse().unwrap();

        assert_eq!(address, Address::Bitcoin(from_hex("0x751e76e8199196d454941c45d1b3a323f1433bd6").unwrap().try_into().unwrap()));
        assert_eq!(address.to_string(), text);
        assert_eq!(address.kind(), "bitcoin");
        assert_eq!(text.to_ascii_uppercase().parse(), Ok(address), "as a QR code writes it");

        for other in [
            "tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx",                       // testnet's
            "bc1qrp33g0q5c5txsp9arysrx4k6zdkfs4nce4xj0gdcccefvpysxf3qccfmv3", // a script's 32 bytes
            "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t5",                       // a typo
            "bitcoin:0x751e76e8199196d454941c45d1b3a323f1433bd6",                // one text only
        ] {
            assert_eq!(other.parse::<Address>(), Err(ParseAddressError), "{other}");
        }
    }
}
