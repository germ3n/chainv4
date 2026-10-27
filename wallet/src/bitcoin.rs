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

pub use chainv4::crypto::bitcoin::tx::Network;

use {
    crate::hardware::{HdKey, KeyPath, Step},
    chainv4::crypto::bitcoin::hash160,
    chainv4::crypto::bitcoin::tx::{Reader, SIGHASH_ALL, Transaction, TxOut, address, compact, der, p2wpkh},
    chainv4::crypto::keypair::{PrivateKey, PublicKey},
    chainv4::crypto::mnemonic::Seed,
    chainv4::structs::bech32,
    minicbor::data::Tag,
    minicbor::{Decoder, Encoder},
    std::collections::HashSet,
};

#[cfg(feature = "swap")]
use {
    chainv4::crypto::bitcoin::tx::TxIn,
    chainv4::swap::htlc::{Htlc, Spend},
};

/// The UR types this module reads and writes: the pairing, and a PSBT, by the registry's name
/// and by the newer one.
pub const ACCOUNT_TYPE: &str = "crypto-account";
pub const PSBT_TYPE: &str = "crypto-psbt";
pub const NEW_PSBT_TYPE: &str = "psbt";

/// The registry's CBOR tags: a crypto-hdkey, and an output descriptor's wpkh(), native segwit.
const HDKEY: u64 = 303;
const WPKH: u64 = 404;

/// The key types this reads and writes, as BIP174 numbers them.
const GLOBAL_UNSIGNED_TX: u8 = 0x00;
const GLOBAL_VERSION: u8 = 0xfb;
const IN_NON_WITNESS_UTXO: u8 = 0x00;
const IN_WITNESS_UTXO: u8 = 0x01;
const IN_PARTIAL_SIG: u8 = 0x02;
const IN_SIGHASH_TYPE: u8 = 0x03;
#[cfg(feature = "swap")]
const IN_WITNESS_SCRIPT: u8 = 0x05;
const IN_BIP32_DERIVATION: u8 = 0x06;
const IN_FINAL_SCRIPTSIG: u8 = 0x07;
const IN_FINAL_SCRIPTWITNESS: u8 = 0x08;
const OUT_BIP32_DERIVATION: u8 = 0x02;

/// What every PSBT starts with.
const MAGIC: &[u8] = b"psbt\xff";


/// An index at or above this is hardened.
const HARDENED: u32 = 1 << 31;

/// The most there are of bitcoin, in satoshis: no amount is more.
const MAX_MONEY: u64 = 21_000_000 * 100_000_000;

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

/// One of a PSBT's pairs, kept as it came.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Pair {
    key: Vec<u8>,
    value: Vec<u8>,
}

/// A partially signed transaction, BIP174's version 0: the transaction, unsigned, and what the
/// wallet says of it, its inputs and its outputs, in maps of pairs kept as they came, so a signed
/// PSBT goes back with all it held and the signatures added.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Psbt {
    global: Vec<Pair>,
    inputs: Vec<Vec<Pair>>,
    outputs: Vec<Vec<Pair>>,
    tx: Transaction,
}

impl Psbt {
    /// Reads one from all of these bytes. Refused: another version, a transaction signed already
    /// or with nothing in or out, a map short of the transaction's inputs or outputs, a key twice
    /// in one map, and anything after.
    pub fn read(bytes: &[u8]) -> Result<Psbt, String> {
        let mut r = Reader::new(bytes);

        if r.take(MAGIC.len()).ok() != Some(MAGIC) {
            return Err("not a PSBT: one starts psbt and 0xff".to_string());
        }

        let global = map(&mut r)?;

        if global.iter().any(|pair| pair.key.first() == Some(&GLOBAL_UNSIGNED_TX) && pair.key.len() > 1) {
            return Err("a PSBT whose transaction's key holds more than its type".to_string());
        }

        if let Some(version) = find(&global, &[GLOBAL_VERSION])
            && version != [0; 4]
        {
            return Err(match <[u8; 4]>::try_from(version) {
                Ok(version) => format!("a PSBT of version {}, which this signer does not read: it reads version 0", u32::from_le_bytes(version)),
                Err(_) => "a PSBT whose version does not read".to_string(),
            });
        }

        let (tx, witnessed) = Transaction::read(find(&global, &[GLOBAL_UNSIGNED_TX]).ok_or("a PSBT with no transaction in it")?)?;

        if witnessed || tx.inputs.iter().any(|input| !input.script_sig.is_empty()) {
            return Err("a PSBT whose transaction is signed already: a PSBT holds it bare".to_string());
        }

        if tx.inputs.is_empty() || tx.outputs.is_empty() {
            return Err("a transaction that spends nothing, or pays no one".to_string());
        }

        let inputs = tx.inputs.iter().map(|_| map(&mut r)).collect::<Result<Vec<_>, _>>()?;
        let outputs = tx.outputs.iter().map(|_| map(&mut r)).collect::<Result<Vec<_>, _>>()?;

        match r.done() {
            true => Ok(Psbt { global, inputs, outputs, tx }),
            false => Err("a PSBT with more after it than it holds".to_string()),
        }
    }

    /// Its bytes: every pair it was read with, and any signatures added.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = MAGIC.to_vec();

        for map in std::iter::once(&self.global).chain(&self.inputs).chain(&self.outputs) {
            for pair in map {
                compact(&mut out, pair.key.len());
                out.extend(&pair.key);
                compact(&mut out, pair.value.len());
                out.extend(&pair.value);
            }

            out.push(0);
        }

        out
    }

    /// The transaction, unsigned.
    #[cfg(test)]
    pub fn transaction(&self) -> &Transaction {
        &self.tx
    }

    /// The signatures an input holds, each a key's, 33 bytes, and the signature, DER and the
    /// sighash type after it.
    #[cfg(test)]
    pub fn signatures(&self, index: usize) -> Vec<([u8; 33], Vec<u8>)> {
        self.inputs[index]
            .iter()
            .filter(|pair| pair.key.first() == Some(&IN_PARTIAL_SIG))
            .filter_map(|pair| Some((pair.key[1..].try_into().ok()?, pair.value.clone())))
            .collect()
    }

    /// The output an input spends, as the PSBT tells it, and whether the transaction it came in
    /// proves it, as that transaction's id, which the input names, covers its amount. Told both
    /// ways, the two must agree.
    fn spent(&self, index: usize) -> Result<Option<(TxOut, bool)>, String> {
        let (pairs, input, n) = (&self.inputs[index], &self.tx.inputs[index], index + 1);
        let told = match find(pairs, &[IN_WITNESS_UTXO]) {
            Some(value) => {
                let mut r = Reader::new(value);
                let output = TxOut::read(&mut r)?;

                if !r.done() {
                    return Err(format!("input {n}'s coins, with more after them than they hold"));
                }

                Some(output)
            }
            None => None,
        };

        let Some(previous) = find(pairs, &[IN_NON_WITNESS_UTXO]) else {
            return Ok(told.map(|output| (output, false)));
        };
        let (previous, _) = Transaction::read(previous)?;

        if previous.txid() != input.txid {
            return Err(format!("input {n}'s previous transaction is not the one it spends from"));
        }

        let output = usize::try_from(input.vout).ok().and_then(|vout| previous.outputs.get(vout)).cloned();
        let output = output.ok_or_else(|| format!("input {n} spends an output its previous transaction does not have"))?;

        match told.is_some_and(|told| told != output) {
            true => Err(format!("input {n}'s coins, told two ways that disagree")),
            false => Ok(Some((output, true))),
        }
    }

    /// Everything to check before signing, and what this seed signs: each input it holds the key
    /// of. Refused: an input whose coins it does not know, one of this seed's that is not native
    /// segwit or not at BIP84's path, or asks for less than all of the transaction signed, outputs
    /// worth more than the inputs, nothing for this seed to sign, and both networks at once.
    fn plan(&self, seed: &Seed) -> Result<(Review, Vec<Signing>), String> {
        let fingerprint = seed.bitcoin_account(0).master_fingerprint;
        let (mut inputs, mut signing, mut networks, mut unproven) = (Vec::new(), Vec::new(), Vec::new(), false);

        for (index, pairs) in self.inputs.iter().enumerate() {
            let n = index + 1;
            let (output, proven) = self.spent(index)?.ok_or_else(|| format!("input {n} says nothing of the coins it spends: the fee cannot be known"))?;
            let finished = pairs.iter().any(|pair| pair.key == [IN_FINAL_SCRIPTSIG] || pair.key == [IN_FINAL_SCRIPTWITNESS]);
            let ours = derivations(pairs, IN_BIP32_DERIVATION)?.into_iter().filter(|derived| derived.fingerprint == fingerprint).collect::<Vec<_>>();
            unproven |= !proven;

            let (key, path) = match (finished, ours.as_slice()) {
                (true, _) | (_, []) => {
                    inputs.push(Spent { value: output.value, yours: None, swap: None });

                    continue;
                }
                (false, [derived]) => (derived.key, derived.path.clone()),
                (false, _) => return Err(format!("input {n} names more than one of this wallet's keys")),
            };
            let shown = path_text(&path);
            let (network, chain, number) = bip84(&path).ok_or_else(|| format!("input {n} asks for a key at {shown}, where this signer holds none: it signs at m/84'/0'/0' and m/84'/1'/0'"))?;

            if public(&seed.bitcoin_key(network.coin(), chain, number).public()) != key {
                return Err(format!("input {n} asks for a key at {shown} that is not this wallet's: a PSBT for another seed"));
            }

            let swap = htlc_input(pairs, &output.script, &key, n)?;

            if swap.is_none() && output.script != p2wpkh(&hash160(&key)) {
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
            let (code, swapped) = swap.map_or((None, None), |(code, swapped)| (Some(code), Some(swapped)));
            signing.push(Signing { index, network, chain, number, key, value: output.value, code });
            inputs.push(Spent { value: output.value, yours: Some(shown), swap: swapped });
        }

        let Some(network) = networks.first().copied() else {
            return Err("nothing here for this wallet to sign: no input is its keys'. Pair the Bitcoin wallet with this signer, on Pair, and ask again.".to_string());
        };
        let mut outputs = Vec::new();

        for (output, pairs) in self.tx.outputs.iter().zip(&self.outputs) {
            let mut yours = None;

            for Derived { key, fingerprint: from, path } in derivations(pairs, OUT_BIP32_DERIVATION)? {
                let Some((at, chain, number)) = bip84(&path).filter(|_| from == fingerprint) else {
                    continue;
                };
                networks.push(at);

                if public(&seed.bitcoin_key(at.coin(), chain, number).public()) == key && output.script == p2wpkh(&hash160(&key)) {
                    yours = Some(path_text(&path));
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
        let vsize = (signing.len() == self.tx.inputs.len()).then(|| (4 * self.tx.base().len() as u64 + 2 + 108 * signing.len() as u64).div_ceil(4));

        let review = Review {
            network,
            inputs,
            outputs,
            fee,
            vsize,
            unproven,
            lock_time: self.tx.lock_time,
        };

        Ok((review, signing))
    }

    /// What it spends and pays, laid out to check before signing; why it cannot be signed, if it
    /// cannot.
    pub fn review(&self, seed: &Seed) -> Result<Review, String> {
        self.plan(seed).map(|(review, _)| review)
    }

    /// The PSBT with this seed's signatures added, one for each input it holds the key of, in
    /// place of any it held for those keys. Refused as review refuses it.
    pub fn sign(&self, seed: &Seed) -> Result<Psbt, String> {
        let (_, signing) = self.plan(seed)?;
        let mut signed = self.clone();

        for input in signing {
            let keypair = seed.bitcoin_key(input.network.coin(), input.chain, input.number);
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
    chain: u32,
    number: u32,
    key: [u8; 33],
    value: u64,
    /// The script a swap's HTLC is spent with, its BIP143 script code; None for a key's coins.
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
        key if key == htlc.claim => Swapped::Claim,
        key if key == htlc.refund => Swapped::Refund { locktime: htlc.locktime },
        _ => return Err(format!("input {n}'s key is neither of its swap's keys")),
    };

    Ok(Some((script.to_vec(), swapped)))
}

#[cfg(not(feature = "swap"))]
fn htlc_input(_: &[Pair], _: &[u8], _: &[u8; 33], _: usize) -> Result<Option<(Vec<u8>, Swapped)>, String> {
    Ok(None)
}

#[cfg(feature = "swap")]
impl Psbt {
    /// A PSBT asking the offline signer to sign a swap's HTLC spend: the spend unsigned, the
    /// coins it spends, the HTLC's script, and where the signer's words derive the key, at
    /// fingerprint and path. Its outputs say nothing, so the review shows where the coins go.
    pub fn for_htlc(tx: &Transaction, value: u64, htlc: &Htlc, key: [u8; 33], fingerprint: [u8; 4], path: &[u32]) -> Psbt {
        let mut coins = Vec::new();
        TxOut { value, script: htlc.output() }.write(&mut coins);
        let derivation = fingerprint.iter().copied().chain(path.iter().flat_map(|step| step.to_le_bytes())).collect();

        Psbt {
            global: vec![Pair { key: vec![GLOBAL_UNSIGNED_TX], value: tx.base() }],
            inputs: vec![vec![
                Pair { key: vec![IN_WITNESS_UTXO], value: coins },
                Pair { key: vec![IN_WITNESS_SCRIPT], value: htlc.script() },
                Pair { key: [&[IN_BIP32_DERIVATION][..], &key].concat(), value: derivation },
            ]],
            outputs: vec![Vec::new(); tx.outputs.len()],
            tx: Transaction { inputs: tx.inputs.iter().map(|input| TxIn { witness: Vec::new(), ..input.clone() }).collect(), ..tx.clone() },
        }
    }

    /// The spend, finished with the signer's signature, worth value: the transaction to
    /// broadcast. Refused: no signature by the key the spend needs, or one that does not hold.
    pub fn finish_htlc(&self, htlc: &Htlc, value: u64, spend: Spend) -> Result<Transaction, String> {
        let key = match spend {
            Spend::Claim { .. } => htlc.claim,
            Spend::Refund => htlc.refund,
        };
        let signature = find(&self.inputs[0], &[&[IN_PARTIAL_SIG][..], &key].concat()).ok_or("the signed PSBT holds no signature by the swap's key")?;
        let preimage = self.tx.script_preimage(0, &htlc.script(), value);
        let holds = signature
            .split_last()
            .filter(|(kind, _)| u32::from(**kind) == SIGHASH_ALL)
            .and_then(|(_, der)| chainv4::crypto::bitcoin::tx::from_der(der))
            .is_some_and(|signature| chainv4::crypto::bitcoin::verify(&key, &preimage, &signature));

        if !holds {
            return Err("the signer's signature does not hold for this spend".to_string());
        }

        let mut tx = self.tx.clone();
        tx.inputs[0].witness = htlc.witness(signature, spend);

        Ok(tx)
    }
}

/// Amounts added up, None past all the bitcoin there are.
fn total(values: impl Iterator<Item = u64>) -> Option<u64> {
    values.into_iter().try_fold(0u64, |sum, value| sum.checked_add(value).filter(|sum| *sum <= MAX_MONEY))
}

/// A bitcoin key's 33 bytes.
fn public(key: &PublicKey) -> [u8; 33] {
    match key {
        PublicKey::Bitcoin(bytes) => *bytes,
        _ => unreachable!("a bitcoin key"),
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

/// A path as wallets write it: m/84'/0'/0'/1/3.
fn path_text(path: &[u32]) -> String {
    std::iter::once("m".to_string())
        .chain(path.iter().map(|step| match *step >= HARDENED {
            true => format!("{}'", step - HARDENED),
            false => step.to_string(),
        }))
        .collect::<Vec<_>>()
        .join("/")
}

/// A key a map derives: its 33 bytes, the fingerprint of the seed it derives from, and the path
/// below that.
struct Derived {
    key: [u8; 33],
    fingerprint: [u8; 4],
    path: Vec<u32>,
}

/// The keys a map derives, by type. Pairs of keys of another size are no key of this seed's.
fn derivations(pairs: &[Pair], kind: u8) -> Result<Vec<Derived>, String> {
    pairs
        .iter()
        .filter(|pair| pair.key.first() == Some(&kind) && pair.key.len() == 34)
        .map(|pair| match pair.value.len() >= 4 && pair.value.len() % 4 == 0 {
            true => Ok(Derived {
                key: pair.key[1..].try_into().expect("33 bytes"),
                fingerprint: pair.value[..4].try_into().expect("4 bytes"),
                path: pair.value[4..].chunks(4).map(|step| u32::from_le_bytes(step.try_into().expect("4 bytes"))).collect(),
            }),
            false => Err("a key's derivation that does not read".to_string()),
        })
        .collect()
}

/// The value of the pair with this key.
fn find<'a>(pairs: &'a [Pair], key: &[u8]) -> Option<&'a [u8]> {
    pairs.iter().find(|pair| pair.key == key).map(|pair| pair.value.as_slice())
}

/// One of a PSBT's maps: its pairs, up to the empty key that ends it. A key twice is refused.
fn map(r: &mut Reader) -> Result<Vec<Pair>, String> {
    let (mut pairs, mut keys) = (Vec::new(), HashSet::new());

    loop {
        let key = r.sized()?;

        if key.is_empty() {
            return Ok(pairs);
        }

        if !keys.insert(key) {
            return Err("a PSBT with a key twice in one map".to_string());
        }

        pairs.push(Pair { key: key.to_vec(), value: r.sized()?.to_vec() });
    }
}

fn text(err: impl std::fmt::Display) -> String {
    format!("a code that does not write: {err}")
}

#[cfg(test)]
mod tests;
