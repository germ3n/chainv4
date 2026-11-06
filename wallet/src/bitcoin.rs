//! The offline signer as a Bitcoin wallet's QR hardware wallet, as a Keystone or a Passport is:
//! Sparrow, BlueWallet, Nunchuk and the like watch its Bitcoin addresses and ask it to sign. The
//! codes are Uniform Resources, as the registry (BCR-2020-006) has them:
//!
//! - **Pairing:** `ur:crypto-account`, the seed's fingerprint and its native segwit account,
//!   `wpkh()` of a crypto-hdkey: the extended public key at BIP84's m/84'/0'/0', or m/84'/1'/0' on
//!   Bitcoin's test networks. The wallet derives receiving addresses at 0/n below it, the same
//!   bc1q addresses this wallet's accounts have, and change at 1/n.
//! - **A request, and its answer:** `ur:crypto-psbt`, a partially signed transaction, BIP174's
//!   version 0: the transaction, unsigned, with the coins each input spends and the key to sign
//!   it with. The signer adds its signatures and shows the PSBT back, for the wallet to finish and
//!   send. Some wallets name it `ur:psbt`; it goes back in the name it came in.
//!
//! The signer signs the inputs of its own seed, at m/84'/0'/0' or m/84'/1'/0', that spend native
//! segwit outputs (P2WPKH), with BIP143's hash of all of the transaction (SIGHASH_ALL), and
//! nothing else. Before it does, it lays out what leaves: each output's amount and address, which
//! of them pay its own keys back, and the fee. A PSBT on both networks at once, one whose coins
//! are told two ways that disagree, or whose previous transaction is not the one spent from, is
//! refused.

pub use chainv4::crypto::bitcoin::psbt::Psbt;
pub use chainv4::crypto::bitcoin::tx::Network;

use {
    crate::hardware::{HdKey, KeyPath, Step},
    chainv4::crypto::bitcoin::hash160,
    chainv4::crypto::bitcoin::psbt::{
        Derived, HARDENED, IN_BIP32_DERIVATION, IN_WITNESS_SCRIPT, OUT_WITNESS_SCRIPT, IN_FINAL_SCRIPTSIG, IN_FINAL_SCRIPTWITNESS, IN_PARTIAL_SIG, IN_SIGHASH_TYPE, OUT_BIP32_DERIVATION, Pair, derivations, find, path_text,
        total,
    },
    chainv4::crypto::bitcoin::multisig::parse_multisig_script,
    chainv4::crypto::bitcoin::tx::{SIGHASH_ALL, address, der, p2wpkh, p2wsh},
    chainv4::crypto::keypair::{PrivateKey, PublicKey},
    chainv4::crypto::mnemonic::Seed,
    chainv4::structs::bech32,
    minicbor::data::Tag,
    minicbor::{Decoder, Encoder},
};

#[cfg(feature = "swap")]
use chainv4::swap::htlc::Htlc;

/// The UR types this module reads and writes: the pairing, and a PSBT, by the registry's name
/// and by the newer one.
pub const ACCOUNT_TYPE: &str = "crypto-account";
pub const PSBT_TYPE: &str = "crypto-psbt";
pub const NEW_PSBT_TYPE: &str = "psbt";

/// The registry's CBOR tags: a crypto-hdkey, and an output descriptor's wpkh(), native segwit.
const HDKEY: u64 = 303;
const WPKH: u64 = 404;

/// The registry's number for a network in a coin-info: 0, or 1 for testnet.
fn registry(network: Network) -> u64 {
    match network {
        Network::Main => 0,
        Network::Test => 1,
    }
}

/// The account a Bitcoin wallet pairs with, as crypto-account's CBOR: the seed's fingerprint, and
/// one output descriptor, wpkh() of the account's extended key on the network.
pub fn account(seed: &Seed, network: Network) -> Result<Vec<u8>, String> {
    let account = seed.bitcoin_account(network.coin());
    let key = HdKey {
        key: account.public,
        chain_code: Some(account.chain_code),
        coin: Some((Some(0), Some(registry(network)))),
        origin: Some(KeyPath {
            steps: vec![Step::Index(84, true), Step::Index(network.coin(), true), Step::Index(0, true)],
            fingerprint: Some(account.master_fingerprint),
            depth: Some(account.depth),
        }),
        children: None,
        parent_fingerprint: Some(account.parent_fingerprint),
        name: Some(crate::hardware::NAME.to_string()),
        note: None,
    };
    let mut e = Encoder::new(Vec::new());
    e.map(2)
        .and_then(|e| e.u8(1))
        .and_then(|e| e.u32(u32::from_be_bytes(account.master_fingerprint)))
        .and_then(|e| e.u8(2))
        .and_then(|e| e.array(1))
        .and_then(|e| e.tag(Tag::new(WPKH)))
        .and_then(|e| e.tag(Tag::new(HDKEY)))
        .map_err(text)?;
    let mut cbor = e.into_writer();
    cbor.extend(key.to_cbor()?);

    Ok(cbor)
}

/// A Bitcoin wallet's pairing code for a seed, on a network.
pub fn pairing(seed: &Seed, network: Network) -> Result<String, String> {
    Ok(crate::hardware::ur(ACCOUNT_TYPE, &account(seed, network)?))
}

/// The receiving address at index on a network, as the Bitcoin wallet paired shows it: the first,
/// 0, to check the pairing by.
pub fn receiving(seed: &Seed, network: Network, index: u32) -> String {
    bech32::encode_segwit(network.hrp(), &hash160(&public(&seed.bitcoin_key(network.coin(), 0, index).public())))
}

/// Everything to check before signing, and what each of a wallet's seeds signs: each input it
/// holds the key of. An input of none of them is another's, and a seed with no input here simply
/// signs none. Refused: an input whose coins it does not know, one of a seed's that is not native
/// segwit or not at BIP84's path, or asks for less than all of the transaction signed, outputs
/// worth more than the inputs, nothing for any seed to sign, and both networks at once.
fn plan(psbt: &Psbt, seeds: &[&Seed]) -> Result<(Review, Vec<Vec<Signing>>), String> {
    let seeds: Vec<([u8; 4], &Seed)> = seeds.iter().map(|seed| (seed.bitcoin_account(0).master_fingerprint, *seed)).collect();
    let mut signing: Vec<Vec<Signing>> = seeds.iter().map(|_| Vec::new()).collect();
    let (mut inputs, mut signed_inputs, mut networks, mut unproven, mut multisigs) = (Vec::new(), 0, Vec::new(), false, false);

    for (index, pairs) in psbt.inputs.iter().enumerate() {
        let n = index + 1;
        let (output, proven) = psbt.spent(index)?.ok_or_else(|| format!("input {n} says nothing of the coins it spends: the fee cannot be known"))?;
        let finished = pairs.iter().any(|pair| pair.key == [IN_FINAL_SCRIPTSIG] || pair.key == [IN_FINAL_SCRIPTWITNESS]);
        let derived = derivations(pairs, IN_BIP32_DERIVATION)?;
        unproven |= !proven;
        let mut spent = Spent { value: output.value, yours: None, swap: None };

        for (which, (fingerprint, seed)) in seeds.iter().enumerate() {
            let ours = derived.iter().filter(|derived| derived.fingerprint == *fingerprint).collect::<Vec<_>>();

            let (key, path) = match (finished, ours.as_slice()) {
                (true, _) | (_, []) => continue,
                (false, [derived]) => (derived.key, derived.path.clone()),
                (false, _) => return Err(format!("input {n} names more than one of this wallet's keys")),
            };
            let shown = path_text(&path);
            let (network, account, chain, number, multisig) = match (bip84(&path), bip48(&path)) {
                (Some((network, chain, number)), _) => (network, 0, chain, number, false),
                (None, Some((network, account, chain, number))) => (network, account, chain, number, true),
                _ => return Err(format!("input {n} asks for a key at {shown}, where this signer holds none: it signs at m/84'/0'/0' and m/84'/1'/0', and multisigs' at m/48'/0'/n'/2' and m/48'/1'/n'/2', any account n")),
            };
            let held = match multisig {
                true => seed.multisig_key(network.coin(), account, chain, number),
                false => seed.bitcoin_key(network.coin(), chain, number),
            };

            if public(&held.public()) != key {
                return Err(format!("input {n} asks for a key at {shown} that is not this wallet's: a PSBT for another seed"));
            }

            let swap = match multisig {
                true => None,
                false => htlc_input(pairs, &output.script, &key, n)?,
            };
            let (shared, shared_swap) = match multisig {
                true => multisig_input(pairs, &output.script, &key, n).map(|(script, swapped)| (Some(script), swapped))?,
                false => (None, None),
            };

            if swap.is_none() && shared.is_none() && output.script != p2wpkh(&hash160(&key)) {
                return Err(match kind(&output.script) {
                    Some("native segwit") => format!("input {n} spends coins of another key than the one at {shown}"),
                    Some(kind) => format!("input {n} spends a {kind} output, which this signer does not sign: it signs native segwit, bc1q, alone"),
                    None => format!("input {n} spends an output of a script this signer does not sign: it signs native segwit, bc1q, alone"),
                });
            }

            if find(pairs, &[IN_SIGHASH_TYPE]).is_some_and(|kind| kind != SIGHASH_ALL.to_le_bytes()) {
                return Err(format!("input {n} asks for a signature that leaves the rest open to change: this signer signs all of a transaction, SIGHASH_ALL, alone"));
            }

            networks.push(network);
            multisigs |= multisig;
            let (code, swapped) = swap.map_or((shared, shared_swap), |(code, swapped)| (Some(code), Some(swapped)));
            signing[which].push(Signing { index, network, account, chain, number, multisig, key, value: output.value, code });
            // Two of a wallet's seeds may both be a multisig's members: the review names the first.
            spent.swap = spent.swap.or(swapped);
            spent.yours.get_or_insert(shown);
        }

        signed_inputs += usize::from(spent.yours.is_some());
        inputs.push(spent);
    }

    let Some(network) = networks.first().copied() else {
        return Err("nothing here for this wallet to sign: no input is its keys'. Pair the Bitcoin wallet with this signer, on Pair, and ask again.".to_string());
    };
    let mut outputs = Vec::new();

    for (output, pairs) in psbt.tx.outputs.iter().zip(&psbt.outputs) {
        let mut yours = None;

        for Derived { key, fingerprint: from, path } in derivations(pairs, OUT_BIP32_DERIVATION)? {
            // Change back to any of the wallet's seeds, whichever of them signs the inputs.
            for (fingerprint, seed) in &seeds {
                // Change back to a multisig this seed is a member of: its script, holding this key.
                if let Some((at, account, chain, number)) = bip48(&path).filter(|_| from == *fingerprint) {
                    networks.push(at);
                    let script = find(pairs, &[OUT_WITNESS_SCRIPT]).unwrap_or_default();
                    let holds = parse_multisig_script(script).is_some_and(|(_, keys)| keys.contains(&key));

                    if public(&seed.multisig_key(at.coin(), account, chain, number).public()) == key && holds && output.script == p2wsh(script) {
                        yours = Some(format!("{}, the multisig's", path_text(&path)));
                    }

                    continue;
                }

                let Some((at, chain, number)) = bip84(&path).filter(|_| from == *fingerprint) else {
                    continue;
                };
                networks.push(at);

                if public(&seed.bitcoin_key(at.coin(), chain, number).public()) == key && output.script == p2wpkh(&hash160(&key)) {
                    yours = Some(path_text(&path));
                }
            }
        }

        outputs.push(Paid {
            value: output.value,
            to: address(&output.script, network),
            yours,
        });
    }

    if networks.iter().any(|at| *at != network) {
        return Err("a PSBT on Bitcoin and its testnet both: no transaction spends on both, so it is refused".to_string());
    }

    let spends = total(inputs.iter().map(|spent| spent.value)).ok_or("inputs worth more than there are bitcoin")?;
    let pays = total(outputs.iter().map(|paid| paid.value)).ok_or("outputs worth more than there are bitcoin")?;
    let fee = spends.checked_sub(pays).ok_or("it pays out more than it spends")?;
    // Signed, each input's witness is two pushes: a signature, at most 73 bytes and mostly 72,
    // and its key. Others' inputs, of any kind, cannot be weighed.
    // A multisig's inputs take its members' signatures and its script: weighed by none of this.
    let vsize = (signed_inputs == psbt.tx.inputs.len() && !multisigs).then(|| (4 * psbt.tx.base().len() as u64 + 2 + 108 * signed_inputs as u64).div_ceil(4));

    let review = Review {
        network,
        inputs,
        outputs,
        fee,
        vsize,
        unproven,
        lock_time: psbt.tx.lock_time,
    };

    Ok((review, signing))
}


/// What the offline signer does with a PSBT a Bitcoin wallet asks it to sign.
pub trait Signs {
    /// What it spends and pays, laid out to check before signing; why it cannot be signed, if it
    /// cannot. The wallet itself asks with all its seeds, by review_all.
    #[cfg(test)]
    fn review(&self, seed: &Seed) -> Result<Review, String>;

    /// The PSBT with this seed's signatures added, one for each input it holds the key of, in
    /// place of any it held for those keys. Refused as review refuses it. The wallet itself signs
    /// with all its seeds, by sign_all.
    #[cfg(test)]
    fn sign(&self, seed: &Seed) -> Result<Psbt, String>;

    /// As review, for a wallet with these seeds: an input of any of them is the wallet's, and so
    /// is change back to any of them. A seed with nothing here is passed over; only nothing here
    /// for any of them is refused.
    fn review_all(&self, seeds: &[&Seed]) -> Result<Review, String>;

    /// As sign, for a wallet with these seeds: each signs the inputs it holds the key of, under
    /// the same refusals as review_all.
    fn sign_all(&self, seeds: &[&Seed]) -> Result<Psbt, String>;
}

impl Signs for Psbt {
    #[cfg(test)]
    fn review(&self, seed: &Seed) -> Result<Review, String> {
        self.review_all(&[seed])
    }

    #[cfg(test)]
    fn sign(&self, seed: &Seed) -> Result<Psbt, String> {
        self.sign_all(&[seed])
    }

    fn review_all(&self, seeds: &[&Seed]) -> Result<Review, String> {
        plan(self, seeds).map(|(review, _)| review)
    }

    fn sign_all(&self, seeds: &[&Seed]) -> Result<Psbt, String> {
        let (_, signing) = plan(self, seeds)?;
        let mut signed = self.clone();

        for (seed, input) in seeds.iter().zip(signing).flat_map(|(seed, inputs)| inputs.into_iter().map(move |input| (*seed, input))) {
            let keypair = match input.multisig {
                true => seed.multisig_key(input.network.coin(), input.account, input.chain, input.number),
                false => seed.bitcoin_key(input.network.coin(), input.chain, input.number),
            };
            let PrivateKey::Bitcoin(secret) = keypair.private() else {
                unreachable!("a bitcoin key");
            };
            let preimage = match &input.code {
                Some(code) => self.tx.script_preimage(input.index, code, input.value),
                None => self.tx.preimage(input.index, &hash160(&input.key), input.value),
            };
            let mut signature = der(&chainv4::crypto::bitcoin::sign(secret, &preimage));
            signature.push(SIGHASH_ALL as u8);

            let key = [&[IN_PARTIAL_SIG][..], &input.key].concat();
            let pairs = &mut signed.inputs[input.index];
            pairs.retain(|pair| pair.key != key);
            pairs.push(Pair { key, value: signature });
        }

        Ok(signed)
    }
}

/// An input of this seed's to sign: where its key sits, and what it spends.
struct Signing {
    index: usize,
    network: Network,
    /// The account a multisig member's key is of, at m/48'/coin'/account'/2'; 0 for any other.
    account: u32,
    chain: u32,
    number: u32,
    /// Whether the key is a multisig member's, at m/48'/coin'/account'/2'/chain/number.
    multisig: bool,
    key: [u8; 33],
    value: u64,
    /// The script a swap's HTLC or a multisig is spent with, its BIP143 script code; None for a
    /// key's coins.
    code: Option<Vec<u8>>,
}

/// A PSBT laid out to check before signing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Review {
    pub network: Network,
    pub inputs: Vec<Spent>,
    pub outputs: Vec<Paid>,
    pub fee: u64,
    /// Its size once signed, in virtual bytes, when this signer signs every input.
    pub vsize: Option<u64>,
    /// Whether an input's amount is the wallet's word alone, with no previous transaction to
    /// prove it.
    pub unproven: bool,
    pub lock_time: u32,
}

/// What an input spends, in satoshis, and the path of this seed's key that signs it, if one does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spent {
    pub value: u64,
    pub yours: Option<String>,
    /// When it spends a swap's HTLC, which way this wallet's key takes it.
    pub swap: Option<Swapped>,
}

/// Which way a key takes a swap's HTLC: with the secret, or back after its lock time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(feature = "swap"), allow(dead_code))]
pub enum Swapped {
    Claim,
    /// Mined only once Bitcoin's median time is past the lock time, Unix seconds.
    Refund { locktime: u32 },
}

/// What an output pays, in satoshis, and to whom: its address, or what its script is when it has
/// none, and the path of this seed's key it pays, when it comes back to this wallet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paid {
    pub value: u64,
    pub to: String,
    pub yours: Option<String>,
}

impl Review {
    /// What leaves the wallet: the outputs that pay others.
    pub fn sent(&self) -> u64 {
        self.outputs.iter().filter(|paid| paid.yours.is_none()).map(|paid| paid.value).sum()
    }

    /// Whether the fee is more than a tenth of what is spent: a fee too high to sign unawares.
    pub fn costly(&self) -> bool {
        let spends: u64 = self.inputs.iter().map(|spent| spent.value).sum();

        self.fee > spends / 10
    }
}

/// A PSBT a Bitcoin wallet asks to have signed, and the UR type it came in, which the answer
/// goes back in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub psbt: Psbt,
    pub kind: &'static str,
}

impl Request {
    /// Reads one from a code's CBOR: the PSBT's bytes, and nothing after.
    pub fn from_cbor(kind: &'static str, cbor: &[u8]) -> Result<Request, String> {
        let mut d = Decoder::new(cbor);
        let bytes = d.bytes().map_err(|_| "a PSBT code that does not read".to_string())?;

        match d.position() == cbor.len() {
            true => Ok(Request { psbt: Psbt::read(bytes)?, kind }),
            false => Err("a PSBT code with more in it than it should hold".to_string()),
        }
    }

    pub fn to_cbor(&self) -> Vec<u8> {
        let mut e = Encoder::new(Vec::new());
        e.bytes(&self.psbt.to_bytes()).expect("writing to memory never fails");

        e.into_writer()
    }
}

/// What kind of output a script makes, as the refusal to sign one says.
fn kind(script: &[u8]) -> Option<&'static str> {
    match script {
        [0x76, 0xa9, 0x14, hash @ .., 0x88, 0xac] if hash.len() == 20 => Some("legacy (1…)"),
        [0xa9, 0x14, hash @ .., 0x87] if hash.len() == 20 => Some("script-hash (3…), such as wrapped segwit,"),
        [0x00, 0x14, program @ ..] if program.len() == 20 => Some("native segwit"),
        [0x00, 0x20, program @ ..] if program.len() == 32 => Some("script's native segwit (P2WSH)"),
        [0x51, 0x20, program @ ..] if program.len() == 32 => Some("Taproot (bc1p…)"),
        _ => None,
    }
}

/// A swap's HTLC an input spends, when its PSBT gives the witness script: the script, and which
/// way this key takes it. Refused: a script that is no HTLC of a swap's, one the coins are not
/// locked to, and a key that is neither of its keys.
#[cfg(feature = "swap")]
fn htlc_input(pairs: &[Pair], spent: &[u8], key: &[u8; 33], n: usize) -> Result<Option<(Vec<u8>, Swapped)>, String> {
    let Some(script) = find(pairs, &[IN_WITNESS_SCRIPT]) else {
        return Ok(None);
    };
    let htlc = Htlc::parse(script).ok_or_else(|| format!("input {n} spends a script this signer does not sign: it signs native segwit keys and swaps' HTLCs alone"))?;

    if spent != htlc.output() {
        return Err(format!("input {n}'s script is not the one its coins are locked to"));
    }

    let swapped = match *key {
        key if htlc.claim == chainv4::swap::BitcoinParty::Key(key) => Swapped::Claim,
        key if htlc.refund == chainv4::swap::BitcoinParty::Key(key) => Swapped::Refund { locktime: htlc.locktime },
        _ => return Err(format!("input {n}'s key is neither of its swap's keys")),
    };

    Ok(Some((script.to_vec(), swapped)))
}

/// A swap's HTLC whose branch is a multisig's, holding this member's key: which way it takes it.
#[cfg(feature = "swap")]
fn shared_htlc(script: &[u8], key: &[u8; 33]) -> Option<Swapped> {
    let htlc = Htlc::parse(script)?;

    match (htlc.claim.holds(key), htlc.refund.holds(key)) {
        (true, _) => Some(Swapped::Claim),
        (_, true) => Some(Swapped::Refund { locktime: htlc.locktime }),
        _ => None,
    }
}

#[cfg(not(feature = "swap"))]
fn shared_htlc(_: &[u8], _: &[u8; 33]) -> Option<Swapped> {
    None
}

#[cfg(not(feature = "swap"))]
fn htlc_input(_: &[Pair], _: &[u8], _: &[u8; 33], _: usize) -> Result<Option<(Vec<u8>, Swapped)>, String> {
    Ok(None)
}

/// A bitcoin key's 33 bytes.
fn public(key: &PublicKey) -> [u8; 33] {
    match key {
        PublicKey::Bitcoin(bytes) => *bytes,
        _ => unreachable!("a bitcoin key"),
    }
}

/// Where a multisig member's key of this seed's sits: m/48'/coin'/account'/2'/chain/index, BIP48's
/// for native segwit multisig, any account, chain 0 or 1, on one of the networks.
fn bip48(path: &[u32]) -> Option<(Network, u32, u32, u32)> {
    match *path {
        [purpose, coin, account, script, chain, index] if purpose == 48 | HARDENED && coin >= HARDENED && account >= HARDENED && script == 2 | HARDENED && chain < 2 && index < HARDENED => {
            Some((Network::from_coin(coin - HARDENED)?, account - HARDENED, chain, index))
        }
        _ => None,
    }
}

/// A multisig's coins an input spends with this key: the witness script, which must be an M-of-N
/// holding the key, or a swap's HTLC whose multisig branch holds it, and the one its coins are
/// locked to; and for an HTLC, which way it takes it.
fn multisig_input(pairs: &[Pair], spent: &[u8], key: &[u8; 33], n: usize) -> Result<(Vec<u8>, Option<Swapped>), String> {
    let script = find(pairs, &[IN_WITNESS_SCRIPT]).ok_or_else(|| format!("input {n} asks for a multisig key's signature and gives no witness script"))?;
    let swapped = match parse_multisig_script(script) {
        Some((_, keys)) if keys.contains(key) => None,
        Some(_) => return Err(format!("input {n}'s multisig does not hold this wallet's key")),
        None => Some(shared_htlc(script, key).ok_or_else(|| format!("input {n}'s witness script is no M-of-N multisig, nor a swap's HTLC whose multisig holds this wallet's key"))?),
    };

    match spent == p2wsh(script) {
        true => Ok((script.to_vec(), swapped)),
        false => Err(format!("input {n}'s script is not the one its coins are locked to")),
    }
}

/// Where a key of this seed's sits, as this signer signs: m/84'/coin'/0'/chain/index, chain 0 or
/// 1, on one of the networks.
fn bip84(path: &[u32]) -> Option<(Network, u32, u32)> {
    match *path {
        [purpose, coin, account, chain, index] if purpose == 84 | HARDENED && coin >= HARDENED && account == HARDENED && chain < 2 && index < HARDENED => {
            Some((Network::from_coin(coin - HARDENED)?, chain, index))
        }
        _ => None,
    }
}

fn text(err: impl std::fmt::Display) -> String {
    format!("a code that does not write: {err}")
}

// Used where the wallet follows Bitcoin itself, its light client finding the coins.
#[cfg(all(feature = "bitcoin-light", not(target_arch = "wasm32")))]
pub mod pay;

#[cfg(test)]
mod tests;
