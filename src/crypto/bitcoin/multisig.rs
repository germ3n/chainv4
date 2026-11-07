//! A native multisig's side on Bitcoin: the P2WSH M-of-N its members' xpubs make, as Bitcoin's
//! own multisig wallets make it, so Sparrow, Nunchuk, Electrum or a hardware wallet watch and
//! co-sign the same coins:
//!
//! ```text
//! wsh(sortedmulti(M,[fingerprint/48'/coin'/account'/2']xpub…/<0;1>/*,…))
//! ```
//!
//! Each member names its account xpub at BIP48's path for native segwit multisig, m/48'/0'/a'/2'
//! on Bitcoin or m/48'/1'/a'/2' on its test networks, a the account of its seed. Receiving address
//! n takes every member's key at 0/n, change address n at 1/n, sorted (BIP67), so the same members
//! make the same address whatever order they are listed in.

use {
    super::sha256d,
    super::tx::{Network, address, p2wsh},
    crate::registry::multisig::BitcoinXpub,
    hmac::{Hmac, KeyInit, Mac},
    r#macro::document,
    sha2::Sha512,
};

/// An index at or above this is hardened.
const HARDENED: u32 = 1 << 31;

/// BIP48's purpose, and its script type for native segwit, P2WSH.
const BIP48_PURPOSE: u32 = 48;
const BIP48_P2WSH: u32 = 2;

#[document(
    summary = "The path a member's multisig account xpub sits at: m/48'/coin'/account'/2', BIP48's for native segwit multisig, coin 0 on Bitcoin and 1 on its test networks.",
    category = "Bitcoin",
    params = { network = "Which network.", account = "Which account of the seed, below 2^31." },
)]
pub fn account_path(network: Network, account: u32) -> [u32; 4] {
    [BIP48_PURPOSE | HARDENED, network.coin() | HARDENED, account | HARDENED, BIP48_P2WSH | HARDENED]
}

/// An xpub's version bytes on a network, as base58 writes them: xpub… or tpub….
fn version(network: Network) -> [u8; 4] {
    match network {
        Network::Main => [0x04, 0x88, 0xb2, 0x1e],
        Network::Test => [0x04, 0x35, 0x87, 0xcf],
    }
}

/// SLIP-0132's versions for a multisig account, Zpub… and Vpub…, which some wallets write.
fn multisig_version(network: Network) -> [u8; 4] {
    match network {
        Network::Main => [0x02, 0xaa, 0x7e, 0xd3],
        Network::Test => [0x02, 0x57, 0x54, 0x83],
    }
}

#[document(
    summary = "A member's xpub as wallets write it, with its origin: [fingerprint/48'/coin'/account'/2']xpub… or tpub…, what a descriptor names it by.",
    category = "Bitcoin",
    params = { xpub = "The member's account xpub.", network = "Which network it is for." },
)]
pub fn xpub_text(xpub: &BitcoinXpub, network: Network) -> String {
    format!("[{}]{}", origin(xpub, network), base58_xpub(xpub, network))
}

/// Its origin, as a descriptor writes it: the seed's fingerprint and the path, h for hardened.
fn origin(xpub: &BitcoinXpub, network: Network) -> String {
    let steps: Vec<String> = account_path(network, xpub.account).iter().map(|step| format!("{}h", step - HARDENED)).collect();

    format!("{}/{}", crate::structs::hex::to_hex(&xpub.fingerprint).trim_start_matches("0x"), steps.join("/"))
}

/// The bare xpub, in base58: version, depth 4, the parent's fingerprint, the child 2', the chain
/// code and the key.
fn base58_xpub(xpub: &BitcoinXpub, network: Network) -> String {
    let mut bytes = version(network).to_vec();
    bytes.push(4);
    bytes.extend(xpub.parent);
    bytes.extend((BIP48_P2WSH | HARDENED).to_be_bytes());
    bytes.extend(xpub.chain_code);
    bytes.extend(xpub.public);

    super::tx::base58check(bytes[0], &bytes[1..])
}

#[document(
    summary = "Reads a member's xpub with its origin, [fingerprint/48'/coin'/account'/2']xpub…, as a descriptor names it, with ' or h for hardened, and xpub/tpub or SLIP-0132's Zpub/Vpub. The origin must be a BIP48 native segwit multisig account, any account, on the network the xpub's version says.",
    category = "Bitcoin",
    returns = "The xpub, and the network it is for; why it is not one, otherwise.",
)]
pub fn parse_xpub(text: &str) -> Result<(BitcoinXpub, Network), String> {
    let text = text.trim();
    let (origin, key) = text
        .strip_prefix('[')
        .and_then(|rest| rest.split_once(']'))
        .ok_or("an xpub with its origin, [fingerprint/48'/0'/0'/2']xpub…")?;
    let (fingerprint, path) = origin.split_once('/').ok_or("an origin is the seed's fingerprint and the path, [73c5da0a/48'/0'/0'/2']")?;
    let fingerprint: [u8; 4] = crate::structs::hex::from_hex(fingerprint).and_then(|bytes| bytes.try_into().ok()).ok_or("an origin's fingerprint is 8 hex digits")?;
    let bytes = base58_decode(key).ok_or("an xpub in base58, with its checksum")?;

    if bytes.len() != 78 {
        return Err("an xpub is 78 bytes, in base58".to_string());
    }

    let versioned: [u8; 4] = bytes[..4].try_into().expect("4 bytes");
    let network = [Network::Main, Network::Test]
        .into_iter()
        .find(|network| versioned == version(*network) || versioned == multisig_version(*network))
        .ok_or("an xpub, tpub, Zpub or Vpub")?;
    let steps: Option<Vec<u32>> = path
        .split('/')
        .map(|step| match step.strip_suffix(['\'', 'h', 'H']) {
            Some(number) => number.parse::<u32>().ok().filter(|number| *number < HARDENED).map(|number| number | HARDENED),
            None => step.parse::<u32>().ok().filter(|number| *number < HARDENED),
        })
        .collect();

    // Any account of the seed, BIP48's third step, hardened.
    let account = match steps.as_deref() {
        Some([_, _, account, _]) if *account >= HARDENED && steps.as_deref() == Some(&account_path(network, account - HARDENED)[..]) => account - HARDENED,
        _ => return Err(format!("a multisig account sits at m/48'/{}'/n'/2', BIP48's native segwit, on {}", network.coin(), network.name())),
    };

    if bytes[4] != 4 || bytes[9..13] != (BIP48_P2WSH | HARDENED).to_be_bytes() {
        return Err("an xpub whose depth or child is not the account's, 4 down at 2'".to_string());
    }

    let xpub = BitcoinXpub {
        fingerprint,
        parent: bytes[5..9].try_into().expect("4 bytes"),
        chain_code: bytes[13..45].try_into().expect("32 bytes"),
        public: bytes[45..78].try_into().expect("33 bytes"),
        account,
    };

    if k256::PublicKey::from_sec1_bytes(&xpub.public).is_err() {
        return Err("an xpub whose key is not on the curve".to_string());
    }

    Ok((xpub, network))
}

/// Base58 with its four-byte checksum, read back; None for text that is not.
fn base58_decode(text: &str) -> Option<Vec<u8>> {
    const ALPHABET: &[u8; 58] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
    let mut bytes: Vec<u8> = Vec::new();

    for character in text.bytes() {
        let mut carry = ALPHABET.iter().position(|digit| *digit == character)? as u32;

        for byte in bytes.iter_mut().rev() {
            carry += u32::from(*byte) * 58;
            *byte = carry as u8;
            carry >>= 8;
        }

        while carry > 0 {
            bytes.insert(0, carry as u8);
            carry >>= 8;
        }
    }

    let zeros = text.bytes().take_while(|character| *character == b'1').count();
    let mut decoded = vec![0; zeros];
    decoded.extend(bytes);
    let (payload, check) = decoded.split_at(decoded.len().checked_sub(4)?);

    (sha256d(payload)[..4] == *check).then(|| payload.to_vec())
}

#[document(
    summary = "A public child of an xpub, BIP32's CKDpub: the key and chain code at a non-hardened index below it.",
    category = "Bitcoin",
    params = { public = "The parent's compressed key.", chain_code = "Its chain code.", index = "The child's number, below 2^31." },
    returns = "None for an index past the non-hardened ones, a key not on the curve, or one of the indexes BIP32 skips, not one in 2^127.",
)]
pub fn child(public: &[u8; 33], chain_code: &[u8; 32], index: u32) -> Option<([u8; 33], [u8; 32])> {
    use k256::ecdsa::VerifyingKey;
    use k256::elliptic_curve::ff::PrimeField;

    if index >= HARDENED {
        return None;
    }

    let mut mac = <Hmac<Sha512> as KeyInit>::new_from_slice(chain_code).expect("HMAC takes a key of any length");
    mac.update(public);
    mac.update(&index.to_be_bytes());
    let mixed: [u8; 64] = mac.finalize().into_bytes().into();
    let tweak: [u8; 32] = mixed[..32].try_into().expect("32 bytes");
    let tweak = Option::<k256::Scalar>::from(k256::Scalar::from_repr(tweak.into()))?;
    let parent = k256::ProjectivePoint::from(*VerifyingKey::from_sec1_bytes(public).ok()?.as_affine());
    // The point at infinity is no key: from_affine refuses it.
    let child = VerifyingKey::from_affine((k256::ProjectivePoint::GENERATOR * tweak + parent).to_affine()).ok()?;
    let key: [u8; 33] = child.to_sec1_point(true).as_bytes().try_into().ok()?;

    Some((key, mixed[32..].try_into().expect("32 bytes")))
}

#[document(
    summary = "A member's key at one of the multisig's addresses: its xpub's child at chain, 0 for receiving or 1 for change, then index.",
    category = "Bitcoin",
)]
pub fn member_key(xpub: &BitcoinXpub, chain: u32, index: u32) -> Option<[u8; 33]> {
    let (key, code) = child(&xpub.public, &xpub.chain_code, chain)?;

    child(&key, &code, index).map(|(key, _)| key)
}

#[document(
    summary = "The witness script of one of the multisig's addresses: OP_M, each member's key there sorted (BIP67), OP_N, OP_CHECKMULTISIG.",
    category = "Bitcoin",
    params = {
        threshold = "M: how many signatures a spend needs.",
        xpubs = "The members' account xpubs.",
        chain = "0 for a receiving address, 1 for change.",
        index = "The address's number on that chain.",
    },
)]
pub fn witness_script(threshold: u32, xpubs: &[BitcoinXpub], chain: u32, index: u32) -> Option<Vec<u8>> {
    let mut keys = xpubs.iter().map(|xpub| member_key(xpub, chain, index)).collect::<Option<Vec<_>>>()?;
    keys.sort();

    Some(multisig_script(threshold, &keys))
}

#[document(
    summary = "OP_M, each key, OP_N, OP_CHECKMULTISIG: a bare M-of-N script, numbers past 16 pushed in one byte, as Bitcoin Core writes them.",
    category = "Bitcoin",
)]
pub fn multisig_script(threshold: u32, keys: &[[u8; 33]]) -> Vec<u8> {
    let small = |number: usize| match number {
        0..=16 => vec![0x50 + number as u8],
        _ => vec![1, number as u8],
    };
    let mut script = small(threshold as usize);

    for key in keys {
        script.push(33);
        script.extend_from_slice(key);
    }

    script.extend(small(keys.len()));
    script.push(0xae);

    script
}

#[document(
    summary = "A bare M-of-N script read back: its threshold and keys, in the order it holds them; None for any other script.",
    category = "Bitcoin",
)]
pub fn parse_multisig_script(script: &[u8]) -> Option<(u32, Vec<[u8; 33]>)> {
    let (number, mut rest) = small_number(script)?;
    let mut keys = Vec::new();

    while let [33, key @ ..] = rest {
        keys.push(key.get(..33)?.try_into().ok()?);
        rest = &key[33..];
    }

    let (count, rest) = small_number(rest)?;

    (rest == [0xae] && count as usize == keys.len() && (1..=count).contains(&number)).then_some((number, keys))
}

/// A number as multisig_script writes it, and what follows.
fn small_number(script: &[u8]) -> Option<(u32, &[u8])> {
    match script {
        [op @ 0x51..=0x60, rest @ ..] => Some((u32::from(op - 0x50), rest)),
        [1, number @ 17..=20, rest @ ..] => Some((u32::from(*number), rest)),
        _ => None,
    }
}

#[document(
    summary = "One of the multisig's addresses on its network: the P2WSH of its witness script, bc1q… or tb1q….",
    category = "Bitcoin",
)]
pub fn bitcoin_address(threshold: u32, xpubs: &[BitcoinXpub], network: Network, chain: u32, index: u32) -> Option<String> {
    witness_script(threshold, xpubs, chain, index).map(|script| address(&p2wsh(&script), network))
}

#[document(
    summary = "The multisig's descriptor, with its checksum, as Bitcoin's wallets import it: wsh(sortedmulti(M,[origin]xpub/0/*,…)) for receiving, or /1/* for change.",
    category = "Bitcoin",
    params = { chain = "0 for receiving addresses, 1 for change." },
)]
pub fn descriptor(threshold: u32, xpubs: &[BitcoinXpub], network: Network, chain: u32) -> String {
    let keys: Vec<String> = xpubs.iter().map(|xpub| format!("{}/{chain}/*", xpub_text(xpub, network))).collect();
    let bare = format!("wsh(sortedmulti({threshold},{}))", keys.join(","));

    format!("{bare}#{}", checksum(&bare).expect("a descriptor written here uses only descriptor characters"))
}

#[document(
    summary = "A descriptor's checksum, BIP380's: eight characters a wallet checks it by.",
    category = "Bitcoin",
    returns = "None for a character no descriptor holds.",
)]
pub fn checksum(descriptor: &str) -> Option<String> {
    const INPUT: &str = "0123456789()[],'/*abcdefgh@:$%{}IJKLMNOPQRSTUVWXYZ&+-.;<=>?!^_|~ijklmnopqrstuvwxyzABCDEFGH`#\"\\ ";
    const CHECKSUM: &[u8] = b"qpzry9x8gf2tvdw0s3jn54khce6mua7l";

    fn polymod(c: u64, value: u64) -> u64 {
        let top = c >> 35;
        let mut c = ((c & 0x7_ffff_ffff) << 5) ^ value;

        for (bit, generator) in [0xf5_dee5_1989, 0xa9_fdca_3312, 0x1b_ab10_e32d, 0x37_06b1_677a, 0x64_4d62_6ffd].into_iter().enumerate() {
            if top >> bit & 1 == 1 {
                c ^= generator;
            }
        }

        c
    }

    let (mut c, mut class, mut count) = (1u64, 0u64, 0);

    for character in descriptor.chars() {
        let position = INPUT.find(character)? as u64;
        c = polymod(c, position & 31);
        class = class * 3 + (position >> 5);
        count += 1;

        if count == 3 {
            c = polymod(c, class);
            class = 0;
            count = 0;
        }
    }

    if count > 0 {
        c = polymod(c, class);
    }

    for _ in 0..8 {
        c = polymod(c, 0);
    }

    c ^= 1;

    Some((0..8).map(|at| char::from(CHECKSUM[(c >> (5 * (7 - at)) & 31) as usize])).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_descriptor_s_checksum_is_bip380_s() {
        // BIP380's example, and Bitcoin Core's descriptor tests'.
        assert_eq!(checksum("addr(mkmZxiEcEd8ZqjQWVZuC6so5dFMKEFpN2j)").as_deref(), Some("02wpgw69"));
        assert_eq!(checksum("raw(deadbeef)").as_deref(), Some("89f8spxm"));
    }

    #[test]
    fn public_children_are_bip32_s() {
        // BIP32's first test vector: m/0' xpub, and its non-hardened child m/0'/1.
        let (parent, _) = parse_unchecked("xpub68Gmy5EdvgibQVfPdqkBBCHxA5htiqg55crXYuXoQRKfDBFA1WEjWgP6LHhwBZeNK1VTsfTFUHCdrfp1bgwQ9xv5ski8PX9rL2dZXvgGDnw");
        let (child_key, child_code) = child(&parent.public, &parent.chain_code, 1).unwrap();
        let (expected, _) = parse_unchecked("xpub6ASuArnXKPbfEwhqN6e3mwBcDTgzisQN1wXN9BJcM47sSikHjJf3UFHKkNAWbWMiGj7Wf5uMash7SyYq527Hqck2AxYysAA7xmALppuCkwQ");

        assert_eq!((child_key, child_code), (expected.public, expected.chain_code));
        assert_eq!(child(&parent.public, &parent.chain_code, HARDENED), None, "hardened needs the secret");
    }

    /// An xpub's key and chain code, whatever its depth and path.
    fn parse_unchecked(text: &str) -> (BitcoinXpub, ()) {
        let bytes = base58_decode(text).unwrap();

        (
            BitcoinXpub {
                fingerprint: [0; 4],
                parent: bytes[5..9].try_into().unwrap(),
                chain_code: bytes[13..45].try_into().unwrap(),
                public: bytes[45..78].try_into().unwrap(),
                account: 0,
            },
            (),
        )
    }

    #[test]
    fn an_xpub_reads_back_from_what_it_writes_and_wrong_ones_are_refused() {
        let (parent, _) = parse_unchecked("xpub68Gmy5EdvgibQVfPdqkBBCHxA5htiqg55crXYuXoQRKfDBFA1WEjWgP6LHhwBZeNK1VTsfTFUHCdrfp1bgwQ9xv5ski8PX9rL2dZXvgGDnw");
        let xpub = BitcoinXpub { fingerprint: [0x73, 0xc5, 0xda, 0x0a], ..parent };

        for network in [Network::Main, Network::Test] {
            let text = xpub_text(&xpub, network);

            assert_eq!(parse_xpub(&text), Ok((xpub, network)), "{text}");
            assert_eq!(parse_xpub(&text.replace("h/", "'/").replace("h]", "']")), Ok((xpub, network)), "with ' for hardened");
        }

        let main = xpub_text(&xpub, Network::Main);

        assert!(parse_xpub(&main.replace("/48h/0h", "/48h/1h")).unwrap_err().contains("m/48'/0'/n'/2'"), "a testnet path on a mainnet xpub");
        // Any account of the seed, kept with the key.
        let fifth = parse_xpub(&main.replace("/48h/0h/0h/", "/48h/0h/5h/")).unwrap().0;
        assert_eq!(fifth, BitcoinXpub { account: 5, ..xpub });
        assert!(xpub_text(&fifth, Network::Main).contains("/48h/0h/5h/2h]"));
        assert!(parse_xpub(&main.replace("/48h/0h/0h/", "/48h/0h/5/")).is_err(), "an account is hardened");
        assert!(parse_xpub(&main.replace("/2h]", "/1h]")).is_err(), "nested segwit's account");
        assert!(parse_xpub(&main[..main.len() - 1]).is_err(), "its checksum caught");
        assert!(parse_xpub(main.split(']').nth(1).unwrap()).is_err(), "no origin");
    }

    #[test]
    fn members_listed_in_any_order_make_the_same_addresses() {
        let xpubs: Vec<BitcoinXpub> = (1..=3u8)
            .map(|seed| {
                let secret = [seed; 32];
                let public = crate::crypto::secp256k1::public_key(&secret).unwrap();

                BitcoinXpub { fingerprint: [seed; 4], parent: [0; 4], public, chain_code: [seed; 32], account: 0 }
            })
            .collect();
        let reversed: Vec<BitcoinXpub> = xpubs.iter().rev().copied().collect();
        let first = bitcoin_address(2, &xpubs, Network::Main, 0, 0).unwrap();

        assert!(first.starts_with("bc1q") && first.len() == 62, "{first}");
        assert_eq!(bitcoin_address(2, &reversed, Network::Main, 0, 0).unwrap(), first);
        assert_ne!(bitcoin_address(2, &xpubs, Network::Main, 0, 1).unwrap(), first, "a fresh one each index");
        assert_ne!(bitcoin_address(2, &xpubs, Network::Main, 1, 0).unwrap(), first, "change apart");
        assert!(bitcoin_address(2, &xpubs, Network::Test, 0, 0).unwrap().starts_with("tb1q"));

        let descriptor = descriptor(2, &xpubs, Network::Main, 0);
        let (bare, sum) = descriptor.split_once('#').unwrap();

        assert!(bare.starts_with("wsh(sortedmulti(2,[01010101/48h/0h/0h/2h]xpub"));
        assert!(bare.ends_with("/0/*))"));
        assert_eq!(checksum(bare).as_deref(), Some(sum));
    }
}

