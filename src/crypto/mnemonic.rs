//! Seed phrases. A phrase is BIP39 words: 12 carry 128 bits of randomness and a checksum, 24 carry
//! 256. PBKDF2 stretches the phrase, with an optional passphrase, into a 64-byte seed, and one seed
//! gives every key an account needs:
//!
//! - **Ed25519 and EdBlake** by SLIP-0010, whose Edwards derivation is hardened only: account n at
//!   m/44'/7744'/n'/0'/0' and m/44'/7744'/n'/1'/0'. A branch each, so one seed never makes the
//!   same secret for two schemes. 7744 is chainv4's coin type, unregistered in SLIP-44.
//! - **secp256k1** by BIP32, at Ethereum's m/44'/60'/0'/0/n: the same words give the same accounts
//!   and 0x addresses here as in Ethereum's wallets.
//! - **bitcoin** by BIP32, at BIP84's m/84'/0'/0'/0/n: the same words give the same bc1q addresses
//!   here as in Bitcoin's wallets, account n being the wallet's nth receiving address.
//!
//! Each derivation step hashes with HMAC-SHA512, as both standards do.

use {
    crate::crypto::keypair::{KeyType, Keypair, PrivateKey},
    hmac::{Hmac, KeyInit, Mac},
    r#macro::document,
    sha2::Sha512,
    std::fmt,
    zeroize::{Zeroize, Zeroizing},
};

#[document(
    summary = "chainv4's coin type in the derivation paths of its Edwards keys. It is not registered in SLIP-44.",
    category = "Keys",
    see_also = "Seed::key",
)]
pub const COIN_TYPE: u32 = 7744;

#[document(
    summary = "Ethereum's coin type, 60: secp256k1 keys derive on its path, so they match Ethereum's wallets.",
    category = "Keys",
    see_also = "Seed::key",
)]
pub const ETHEREUM_COIN_TYPE: u32 = 60;

#[document(
    summary = "BIP84's purpose, 84: bitcoin keys derive at m/84'/0'/0'/0/n, as Bitcoin's wallets derive native segwit (bc1q) addresses.",
    category = "Keys",
    see_also = "Seed::key, Seed::bitcoin_account, Seed::bitcoin_key",
)]
pub const BIP84_PURPOSE: u32 = 84;

#[document(
    summary = "The coin type of Bitcoin's test networks, 1, as SLIP-44 has it: testnet's, signet's and regtest's keys derive at m/84'/1'/0', apart from Bitcoin's own, so coins on a test network never share a key with real ones.",
    category = "Keys",
    see_also = "Seed::bitcoin_account, Seed::bitcoin_key",
)]
pub const BITCOIN_TESTNET_COIN_TYPE: u32 = 1;

/// An index at or above this is hardened: its child cannot be derived from the parent's public key.
const HARDENED: u32 = 1 << 31;

#[document(
    summary = "A seed phrase: BIP39's English words, 12 or 24 of them as made here, any length BIP39 allows as read, with their checksum. It shows its words; the seed comes from seed, with a passphrase or none.",
    category = "Keys",
    see_also = "Seed, Phrase::seed",
)]
#[derive(Clone, PartialEq, Eq)]
pub struct Phrase(bip39::Mnemonic);

impl Phrase {
    #[document(
        parent = "Phrase",
        summary = "A new phrase from the system's randomness.",
        params = { words = "12 or 24." },
        returns = "The phrase, or why there is none: another number of words, or no randomness.",
    )]
    pub fn generate(words: usize) -> Result<Phrase, MnemonicError> {
        let size = match words {
            12 => 16,
            24 => 32,
            _ => return Err(MnemonicError::WordCount(words)),
        };
        let mut entropy = [0; 32];
        getrandom::fill(&mut entropy[..size]).map_err(|err| MnemonicError::NoRandomness(err.to_string()))?;
        let phrase = bip39::Mnemonic::from_entropy(&entropy[..size]).expect("16 or 32 bytes make a phrase");
        entropy.zeroize();

        Ok(Phrase(phrase))
    }

    #[document(
        parent = "Phrase",
        summary = "Reads a phrase as someone wrote it down: any spacing and case, its words checked against the list and its checksum.",
        returns = "The phrase, or what is wrong with it.",
    )]
    pub fn parse(text: &str) -> Result<Phrase, MnemonicError> {
        // Every copy of the words made here is wiped as it goes, as the phrase itself is.
        let words: Vec<Zeroizing<String>> = text.split_whitespace().map(|word| Zeroizing::new(word.to_lowercase())).collect();
        let mut joined = Zeroizing::new(String::with_capacity(text.len()));

        for (index, word) in words.iter().enumerate() {
            if index > 0 {
                joined.push(' ');
            }

            joined.push_str(word);
        }

        match bip39::Mnemonic::parse_in_normalized(bip39::Language::English, &joined) {
            Ok(phrase) => Ok(Phrase(phrase)),
            Err(bip39::Error::BadWordCount(count)) => Err(MnemonicError::WordCount(count)),
            Err(bip39::Error::UnknownWord(at)) => Err(MnemonicError::UnknownWord(words[at].to_string())),
            Err(_) => Err(MnemonicError::Checksum),
        }
    }

    #[document(parent = "Phrase", summary = "Its words, in order.")]
    pub fn words(&self) -> Vec<&'static str> {
        self.0.words().collect()
    }

    #[document(
        parent = "Phrase",
        summary = "The seed: the phrase stretched with PBKDF2 and a passphrase, as BIP39 does. Another passphrase makes another seed, and so other keys.",
        params = { passphrase = "BIP39's optional passphrase; empty for none." },
    )]
    pub fn seed(&self, passphrase: &str) -> Seed {
        Seed(self.0.to_seed(passphrase))
    }
}

/// Its words, separated by spaces.
impl fmt::Display for Phrase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.words().join(" "))
    }
}

/// Never its words, so a phrase cannot end up in a log.
impl fmt::Debug for Phrase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Phrase({} words)", self.0.word_count())
    }
}

#[document(
    summary = "The 64-byte seed a phrase stretches to, which every account's keys derive from. Wiped from memory when dropped.",
    category = "Keys",
    see_also = "Phrase::seed, Seed::key",
)]
pub struct Seed([u8; 64]);

impl Seed {
    #[document(
        parent = "Seed",
        summary = "An account's key in one scheme: Ed25519 and EdBlake by SLIP-0010 at m/44'/7744'/n'/0'/0' and m/44'/7744'/n'/1'/0', secp256k1 by BIP32 at Ethereum's m/44'/60'/0'/0/n, bitcoin by BIP32 at BIP84's m/84'/0'/0'/0/n.",
        params = {
            scheme = "Which scheme.",
            account = "The account's number, from 0. Below 2^31.",
        },
        see_also = "COIN_TYPE, ETHEREUM_COIN_TYPE, BIP84_PURPOSE",
    )]
    pub fn key(&self, scheme: KeyType, account: u32) -> Keypair {
        let private = match scheme {
            #[cfg(feature = "ed25519")]
            KeyType::Ed25519 => PrivateKey::Ed25519(slip10(&self.0, &[44, COIN_TYPE, account, 0, 0])),
            #[cfg(feature = "edblake")]
            KeyType::EdBlake => PrivateKey::EdBlake(slip10(&self.0, &[44, COIN_TYPE, account, 1, 0])),
            // Not one seed in 2^127 lands on an index BIP32 skips.
            #[cfg(feature = "secp256k1")]
            KeyType::Secp256k1 => {
                let path = [(44, true), (ETHEREUM_COIN_TYPE, true), (0, true), (0, false), (account, false)];

                PrivateKey::Secp256k1(bip32(&self.0, &path).expect("a key at every index but one in 2^127"))
            }
            #[cfg(feature = "bitcoin")]
            KeyType::Bitcoin => PrivateKey::Bitcoin(self.bip84(0, 0, account)),
        };

        Keypair::from_private(private)
    }

    #[document(
        parent = "Seed",
        summary = "The extended public key at m/44'/60'/0', Ethereum's account level: every secp256k1 account key derives below it, account n at 0/n. What MetaMask takes to pair with a QR hardware wallet, and lists the accounts of.",
        see_also = "ExtendedPublic, Seed::key",
    )]
    #[cfg(feature = "secp256k1")]
    pub fn ethereum_account(&self) -> ExtendedPublic {
        self.account_level([(44, true), (ETHEREUM_COIN_TYPE, true), (0, true)])
    }

    #[document(
        parent = "Seed",
        summary = "The extended public key at m/84'/coin'/0', BIP84's account level: at coin type 0, Bitcoin's, every bitcoin account key derives below it, account n at 0/n. What a Bitcoin wallet takes to watch the addresses an offline signer holds the keys of, its change at 1/n.",
        params = {
            coin = "The coin type: 0 for Bitcoin, BITCOIN_TESTNET_COIN_TYPE for its test networks.",
        },
        see_also = "ExtendedPublic, Seed::key, Seed::bitcoin_key, BIP84_PURPOSE",
    )]
    #[cfg(feature = "bitcoin")]
    pub fn bitcoin_account(&self, coin: u32) -> ExtendedPublic {
        self.account_level([(BIP84_PURPOSE, true), (coin, true), (0, true)])
    }

    #[document(
        parent = "Seed",
        summary = "The bitcoin key at m/84'/coin'/0'/chain/index, below bitcoin_account: what a Bitcoin wallet watching that account asks an offline signer to sign with. At coin 0 and chain 0 it is account index's key, as key gives it.",
        params = {
            coin = "The coin type: 0 for Bitcoin, BITCOIN_TESTNET_COIN_TYPE for its test networks.",
            chain = "0 for receiving addresses, 1 for change.",
            index = "The address's number on that chain, from 0. Below 2^31.",
        },
        see_also = "Seed::bitcoin_account, Seed::key",
    )]
    #[cfg(feature = "bitcoin")]
    pub fn bitcoin_key(&self, coin: u32, chain: u32, index: u32) -> Keypair {
        Keypair::from_private(PrivateKey::Bitcoin(self.bip84(coin, chain, index)))
    }

    /// BIP84's key at m/84'/coin'/0'/chain/index. Not one seed in 2^127 lands on an index BIP32
    /// skips.
    #[cfg(feature = "bitcoin")]
    fn bip84(&self, coin: u32, chain: u32, index: u32) -> [u8; 32] {
        let path = [(BIP84_PURPOSE, true), (coin, true), (0, true), (chain, false), (index, false)];

        bip32(&self.0, &path).expect("a key at every index but one in 2^127")
    }

    /// The extended public key three hardened steps down, where BIP44 and BIP84 put an account.
    #[cfg(feature = "secp256k1")]
    fn account_level(&self, path: [(u32, bool); 3]) -> ExtendedPublic {
        let public = |node: &Node| crate::crypto::secp256k1::public_key(&node.secret).expect("a BIP32 key is a valid secret");
        let master = bip32_node(&self.0, &[]).expect("a master key for every seed but one in 2^127");
        let parent = bip32_node(&self.0, &path[..2]).expect("a key at every index but one in 2^127");
        let account = bip32_node(&self.0, &path).expect("a key at every index but one in 2^127");

        ExtendedPublic {
            public: public(&account),
            chain_code: account.chain,
            depth: 3,
            parent_fingerprint: fingerprint(&public(&parent)),
            master_fingerprint: fingerprint(&public(&master)),
        }
    }
}

impl Drop for Seed {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

#[document(
    summary = "Why a phrase does not do.",
    category = "Keys",
    see_also = "Phrase::parse",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MnemonicError {
    #[document(summary = "Not a number of words BIP39 allows: 12, 15, 18, 21 or 24, and 12 or 24 to make one.")]
    WordCount(usize),
    #[document(summary = "A word not in the list.")]
    UnknownWord(String),
    #[document(summary = "The words do not check: one is wrong, or out of place.")]
    Checksum,
    #[document(summary = "The system gave no randomness for a new phrase.")]
    NoRandomness(String),
}

impl fmt::Display for MnemonicError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MnemonicError::WordCount(count) => write!(f, "{count} words: a phrase is 12, 15, 18, 21 or 24"),
            MnemonicError::UnknownWord(word) => write!(f, "\"{word}\" is not a word of the list"),
            MnemonicError::Checksum => write!(f, "the words do not check: one is wrong, or out of place"),
            MnemonicError::NoRandomness(why) => write!(f, "no randomness for a phrase: {why}"),
        }
    }
}

impl std::error::Error for MnemonicError {}

fn hmac(key: &[u8], parts: &[&[u8]]) -> [u8; 64] {
    let mut mac = <Hmac<Sha512> as KeyInit>::new_from_slice(key).expect("HMAC takes a key of any length");

    for part in parts {
        mac.update(part);
    }

    mac.finalize().into_bytes().into()
}

/// SLIP-0010's Edwards derivation: every step hardened, from the parent's key and chain code alone.
fn slip10(seed: &[u8], path: &[u32]) -> [u8; 32] {
    let mut node = hmac(b"ed25519 seed", &[seed]);

    for index in path {
        let (key, chain) = node.split_at(32);
        node = hmac(chain, &[&[0], key, &(index | HARDENED).to_be_bytes()]);
    }

    let key = node[..32].try_into().expect("32 bytes");
    node.zeroize();

    key
}

/// BIP32 on secp256k1: a hardened step from the parent's key, a normal one from its public key.
/// None at the rare index BIP32 skips, where the child would be zero or past the order.
#[cfg(feature = "secp256k1")]
fn bip32(seed: &[u8], path: &[(u32, bool)]) -> Option<[u8; 32]> {
    bip32_node(seed, path).map(|node| node.secret)
}

/// A BIP32 node: its key, and the chain code its children derive with.
#[cfg(feature = "secp256k1")]
struct Node {
    secret: [u8; 32],
    chain: [u8; 32],
}

#[cfg(feature = "secp256k1")]
impl Drop for Node {
    fn drop(&mut self) {
        self.secret.zeroize();
        self.chain.zeroize();
    }
}

#[cfg(feature = "secp256k1")]
fn bip32_node(seed: &[u8], path: &[(u32, bool)]) -> Option<Node> {
    use k256::elliptic_curve::ff::PrimeField;
    use k256::{FieldBytes, Scalar};

    let scalar = |bytes: &[u8]| -> Option<Scalar> {
        let bytes: [u8; 32] = bytes.try_into().ok()?;

        Option::from(Scalar::from_repr(FieldBytes::from(bytes))).filter(|scalar: &Scalar| !bool::from(scalar.is_zero()))
    };
    let mut node = hmac(b"Bitcoin seed", &[seed]);
    let mut key = scalar(&node[..32])?;

    for &(index, hardened) in path {
        let secret: [u8; 32] = key.to_bytes().into();
        let chain = &node[32..];
        let next = match hardened {
            true => hmac(chain, &[&[0], &secret, &(index | HARDENED).to_be_bytes()]),
            false => {
                let public = crate::crypto::secp256k1::public_key(&secret)?;

                hmac(chain, &[&public, &index.to_be_bytes()])
            }
        };
        let tweak = scalar(&next[..32])?;
        key = Some(tweak + key).filter(|child: &Scalar| !bool::from(child.is_zero()))?;
        node = next;
    }

    let found = Node {
        secret: key.to_bytes().into(),
        chain: node[32..].try_into().expect("64 bytes"),
    };
    node.zeroize();

    Some(found)
}

/// A key's BIP32 fingerprint: the first 4 bytes of the HASH160 of its public key, RIPEMD-160 of
/// SHA-256.
#[cfg(feature = "secp256k1")]
fn fingerprint(public: &[u8; 33]) -> [u8; 4] {
    use ripemd::Digest;

    let hashed = <ripemd::Ripemd160 as Digest>::digest(<sha2::Sha256 as Digest>::digest(public));

    hashed[..4].try_into().expect("20 bytes")
}

#[document(
    summary = "An extended public key, as BIP32 has one: a public key and the chain code its children derive with, and where it sits. What a wallet hands a watch-only one, such as MetaMask pairing with a QR hardware wallet, to derive the accounts below it without their keys.",
    category = "Keys",
    see_also = "Seed::ethereum_account",
)]
#[cfg(feature = "secp256k1")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExtendedPublic {
    #[document(summary = "The public key, compressed: 33 bytes.")]
    pub public: [u8; 33],
    #[document(summary = "The chain code.")]
    pub chain_code: [u8; 32],
    #[document(summary = "How many steps down from the master key.")]
    pub depth: u8,
    #[document(summary = "The fingerprint of the key one step up.")]
    pub parent_fingerprint: [u8; 4],
    #[document(summary = "The fingerprint of the master key: what names the seed, and so the wallet, to one that pairs with it.")]
    pub master_fingerprint: [u8; 4],
}

#[cfg(test)]
mod tests {
    use {super::*, crate::structs::hex::from_hex};

    fn hex<const N: usize>(text: &str) -> [u8; N] {
        from_hex(text).unwrap().try_into().unwrap()
    }

    #[test]
    fn phrases_and_seeds_follow_bip39() {
        // BIP39's first test vector, with the passphrase its vectors use.
        let phrase = Phrase::parse("abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about").unwrap();

        assert_eq!(
            phrase.seed("TREZOR").0,
            hex::<64>("0xc55257c360c07c72029aebc1b53c05ed0362ada38ead3e3e9efa3708e53495531f09a6987599d18264c1e1c92f2cf141630c7a3c4ab7c81b2f001698e7463b04")
        );
        assert_eq!(Phrase::parse("  ABANDON abandon\tabandon abandon abandon abandon abandon abandon abandon abandon abandon about ").as_ref(), Ok(&phrase));
        assert_eq!(Phrase::parse("abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon"), Err(MnemonicError::Checksum));
        assert_eq!(Phrase::parse("abandon abandon abandon"), Err(MnemonicError::WordCount(3)));
        assert_eq!(
            Phrase::parse("abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon chainv4"),
            Err(MnemonicError::UnknownWord("chainv4".to_string()))
        );

        let made = Phrase::generate(12).unwrap();

        assert_eq!((made.words().len(), Phrase::generate(24).unwrap().words().len()), (12, 24));
        assert_eq!(Phrase::parse(&made.to_string()), Ok(made.clone()), "a phrase reads back from its words");
        assert_ne!(made, Phrase::generate(12).unwrap());
        assert_eq!(Phrase::generate(13).err(), Some(MnemonicError::WordCount(13)));
        assert!(!format!("{made:?}").contains(made.words()[0]), "Debug never shows the words");
    }

    #[test]
    fn edwards_keys_follow_slip10() {
        // SLIP-0010's first test vector for ed25519.
        let seed = hex::<16>("0x000102030405060708090a0b0c0d0e0f");

        assert_eq!(slip10(&seed, &[]), hex("0x2b4be7f19ee27bbf30c667b642d5f4aa69fd169872f8fc3059c08ebae2eb19e7"));
        assert_eq!(slip10(&seed, &[0]), hex("0x68e0fe46dfb67e368c75379acec591dad19df3cde26e63b93a8e704f1dade7a3"));
        assert_eq!(slip10(&seed, &[0, 1, 2, 2, 1_000_000_000]), hex("0x8f94d394a8e8fd6b1bc2f3f49f5c47e385281d5c17e65324b0f62483e37e8793"));
    }

    #[cfg(feature = "secp256k1")]
    #[test]
    fn secp256k1_keys_follow_bip32_and_match_ethereums() {
        // BIP32's first test vector.
        let seed = hex::<16>("0x000102030405060708090a0b0c0d0e0f");

        assert_eq!(bip32(&seed, &[]), Some(hex("0xe8f32e723decf4051aefac8e2c93c9c5b214313817cdb01a1494b917c8436b35")));
        assert_eq!(bip32(&seed, &[(0, true)]), Some(hex("0xedb2e14f9ee77d26dd93b4ecede8d16ed408ce149b6cd80b0715a2d911a0afea")));
        assert_eq!(
            bip32(&seed, &[(0, true), (1, false), (2, true), (2, false), (1_000_000_000, false)]),
            Some(hex("0x471b76e389e528d6de6d816857e012c5455051cad6660850e58372a6c3e6e7c8"))
        );

        // Hardhat's well-known development phrase, and the addresses every Ethereum wallet derives.
        let seed = Phrase::parse("test test test test test test test test test test test junk").unwrap().seed("");
        let address = |account| seed.key(KeyType::Secp256k1, account).public().address().to_string();

        assert_eq!(address(0), "secp256k1:0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266");
        assert_eq!(address(1), "secp256k1:0x70997970c51812dc3a010c7d01b50e0d17dc79c8");
    }

    #[cfg(feature = "secp256k1")]
    #[test]
    fn the_account_level_extended_key_derives_the_same_addresses_without_the_keys() {
        use k256::ecdsa::VerifyingKey;
        use k256::{ProjectivePoint, Scalar};

        // BIP32's first test vector: m's identifier begins 3442193e, m/0H's 5c1bd648.
        let seed = hex::<16>("0x000102030405060708090a0b0c0d0e0f");
        let public = |path: &[(u32, bool)]| crate::crypto::secp256k1::public_key(&bip32_node(&seed, path).unwrap().secret).unwrap();

        assert_eq!(fingerprint(&public(&[])), hex("0x3442193e"));
        assert_eq!(fingerprint(&public(&[(0, true)])), hex("0x5c1bd648"));

        // Down from the account's extended key by public steps alone, as MetaMask does: 0/n.
        let seed = Phrase::parse("test test test test test test test test test test test junk").unwrap().seed("");
        let account = seed.ethereum_account();
        let step = |(public, chain): ([u8; 33], [u8; 32]), index: u32| -> ([u8; 33], [u8; 32]) {
            let out = hmac(&chain, &[&public, &index.to_be_bytes()]);
            let tweak: Scalar = Option::from(<Scalar as k256::elliptic_curve::ff::PrimeField>::from_repr(k256::FieldBytes::from(<[u8; 32]>::try_from(&out[..32]).unwrap()))).unwrap();
            let parent = ProjectivePoint::from(*VerifyingKey::from_sec1_bytes(&public).unwrap().as_affine());
            let child = VerifyingKey::from_affine((ProjectivePoint::GENERATOR * tweak + parent).to_affine()).unwrap();

            (child.to_sec1_point(true).as_bytes().try_into().unwrap(), out[32..].try_into().unwrap())
        };
        let external = step((account.public, account.chain_code), 0);

        for (n, expected) in [(0, "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266"), (1, "0x70997970c51812dc3a010c7d01b50e0d17dc79c8")] {
            let (public, _) = step(external, n);

            assert_eq!(crate::structs::hex::to_hex(&crate::crypto::secp256k1::address(&public).unwrap()), expected);
        }

        assert_eq!(account.depth, 3);
        assert_ne!(account.master_fingerprint, account.parent_fingerprint);
        assert_eq!(account.master_fingerprint, fingerprint(&crate::crypto::secp256k1::public_key(&bip32_node(&seed.0, &[]).unwrap().secret).unwrap()));
    }

    #[cfg(feature = "bitcoin")]
    #[test]
    fn bitcoin_keys_follow_bip84_and_match_bitcoins_wallets() {
        // BIP84's test vector: the receiving addresses of its phrase's first account.
        let seed = Phrase::parse("abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about").unwrap().seed("");
        let key = seed.key(KeyType::Bitcoin, 0);

        assert_eq!(key.public().to_string(), "bitcoin:0x0330d54fd0dd420a6e5f8d3624f5f3482cae350f79d5f0753bf5beef9c2d91af3c");
        assert_eq!(key.public().address().to_string(), "bc1qcr8te4kr609gcawutmrza0j4xv80jy8z306fyu");
        assert_eq!(seed.key(KeyType::Bitcoin, 1).public().address().to_string(), "bc1qnjg0jd8228aq7egyzacy8cys3knf9xvrerkf9g");

        let account = seed.bitcoin_account(0);

        // The phrase's master fingerprint, as every wallet that reads it shows it.
        assert_eq!(account.master_fingerprint, hex("0x73c5da0a"));
        assert_eq!(account.public, crate::crypto::secp256k1::public_key(&bip32(&seed.0, &[(84, true), (0, true), (0, true)]).unwrap()).unwrap());
        assert_eq!(account.depth, 3);

        // Below it, BIP84's first change address, and the receiving ones as key gives them.
        assert_eq!(seed.bitcoin_key(0, 1, 0).public().address().to_string(), "bc1q8c6fshw2dlwun7ekn9qwf37cu2rn755upcp6el");
        assert_eq!(seed.bitcoin_key(0, 0, 1).public(), seed.key(KeyType::Bitcoin, 1).public());

        // The test networks' keys are others, at coin type 1, under the same fingerprint.
        let testnet = seed.bitcoin_account(BITCOIN_TESTNET_COIN_TYPE);

        assert_eq!(testnet.public, crate::crypto::secp256k1::public_key(&bip32(&seed.0, &[(84, true), (1, true), (0, true)]).unwrap()).unwrap());
        assert_eq!(testnet.master_fingerprint, account.master_fingerprint);
        assert_ne!(seed.bitcoin_key(1, 0, 0).public(), seed.bitcoin_key(0, 0, 0).public());
    }

    #[test]
    fn an_account_has_a_key_in_each_scheme_and_a_passphrase_makes_another_wallet() {
        let phrase = Phrase::generate(12).unwrap();
        let (seed, other) = (phrase.seed(""), phrase.seed("another"));
        let mut keys = Vec::new();

        for scheme in KeyType::ALL {
            for account in 0..2 {
                let key = seed.key(*scheme, account);

                assert_eq!(key.public(), phrase.seed("").key(*scheme, account).public(), "the same words, the same key");
                assert_ne!(key.public().address(), other.key(*scheme, account).public().address());
                keys.push(key.to_key_file()[key.to_key_file().find(':').unwrap()..].to_string());
            }
        }

        keys.sort();
        keys.dedup();

        assert_eq!(keys.len(), 2 * KeyType::ALL.len(), "no secret twice");
    }
}
