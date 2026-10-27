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

use {
    crate::hardware::{HdKey, KeyPath, Step},
    chainv4::crypto::bitcoin::{hash160, sha256d},
    chainv4::crypto::keypair::{PrivateKey, PublicKey},
    chainv4::crypto::mnemonic::{BITCOIN_TESTNET_COIN_TYPE, Seed},
    chainv4::structs::bech32,
    chainv4::structs::hex::to_hex,
    minicbor::data::Tag,
    minicbor::{Decoder, Encoder},
    std::collections::HashSet,
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
const IN_BIP32_DERIVATION: u8 = 0x06;
const IN_FINAL_SCRIPTSIG: u8 = 0x07;
const IN_FINAL_SCRIPTWITNESS: u8 = 0x08;
const OUT_BIP32_DERIVATION: u8 = 0x02;

/// What every PSBT starts with.
const MAGIC: &[u8] = b"psbt\xff";

/// Signing all of the transaction, its inputs and outputs: the one way this signer signs.
const SIGHASH_ALL: u32 = 1;

/// An index at or above this is hardened.
const HARDENED: u32 = 1 << 31;

/// The most there are of bitcoin, in satoshis: no amount is more.
const MAX_MONEY: u64 = 21_000_000 * 100_000_000;

/// Which Bitcoin: its own network, or its test networks, testnet, signet and regtest, whose
/// coins are worth nothing. Their keys are others, so the one never spends the other's coins.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Network {
    #[default]
    Main,
    Test,
}

impl Network {
    pub const ALL: [Network; 2] = [Network::Main, Network::Test];

    /// Its coin type in the keys' path, as SLIP-44 has it.
    pub fn coin(self) -> u32 {
        match self {
            Network::Main => 0,
            Network::Test => BITCOIN_TESTNET_COIN_TYPE,
        }
    }

    fn from_coin(coin: u32) -> Option<Network> {
        Network::ALL.into_iter().find(|network| network.coin() == coin)
    }

    /// What the pairing and the review call it, large.
    pub fn name(self) -> &'static str {
        match self {
            Network::Main => "Bitcoin",
            Network::Test => "Bitcoin testnet",
        }
    }

    /// What the switch between them calls it.
    pub fn short(self) -> &'static str {
        match self {
            Network::Main => "Mainnet",
            Network::Test => "Testnet",
        }
    }

    pub fn symbol(self) -> &'static str {
        match self {
            Network::Main => "BTC",
            Network::Test => "tBTC",
        }
    }

    /// The account's path: m/84'/0'/0', or m/84'/1'/0'.
    pub fn path(self) -> String {
        format!("m/84'/{}'/0'", self.coin())
    }

    /// Its segwit addresses' human-readable part: bc1…, or tb1… on testnet and signet.
    fn hrp(self) -> &'static str {
        match self {
            Network::Main => "bc",
            Network::Test => "tb",
        }
    }

    /// Its legacy addresses' version bytes: paying a key's hash, 1…, or m… and n…, and a script's,
    /// 3…, or 2….
    fn versions(self) -> (u8, u8) {
        match self {
            Network::Main => (0x00, 0x05),
            Network::Test => (0x6f, 0xc4),
        }
    }

    /// The registry's number for it in a coin-info: 0, or 1 for testnet.
    fn registry(self) -> u64 {
        match self {
            Network::Main => 0,
            Network::Test => 1,
        }
    }
}

/// The account a Bitcoin wallet pairs with, as crypto-account's CBOR: the seed's fingerprint, and
/// one output descriptor, wpkh() of the account's extended key on the network.
pub fn account(seed: &Seed, network: Network) -> Result<Vec<u8>, String> {
    let account = seed.bitcoin_account(network.coin());
    let key = HdKey {
        key: account.public,
        chain_code: Some(account.chain_code),
        coin: Some((Some(0), Some(network.registry()))),
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

/// A Bitcoin transaction, as the network encodes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transaction {
    pub version: u32,
    pub inputs: Vec<TxIn>,
    pub outputs: Vec<TxOut>,
    pub lock_time: u32,
}

/// An input: the output it spends, by its transaction's id, as hashed, and its index there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TxIn {
    pub txid: [u8; 32],
    pub vout: u32,
    pub script_sig: Vec<u8>,
    pub sequence: u32,
}

/// An output: an amount, in satoshis, and the script that spends it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TxOut {
    pub value: u64,
    pub script: Vec<u8>,
}

impl Transaction {
    /// Reads one from all of these bytes, with its witnesses, as BIP144 writes them, or without;
    /// says which.
    pub fn read(bytes: &[u8]) -> Result<(Transaction, bool), String> {
        let mut r = Reader::new(bytes);
        let version = r.u32()?;
        // The marker and flag, 0 and 1, where a transaction with witnesses counts its inputs.
        let witnessed = matches!(bytes.get(4..6), Some([0, 1]));

        if witnessed {
            r.take(2)?;
        }

        let mut inputs = Vec::new();

        for _ in 0..r.compact()? {
            inputs.push(TxIn {
                txid: r.take(32)?.try_into().expect("32 bytes"),
                vout: r.u32()?,
                script_sig: r.sized()?.to_vec(),
                sequence: r.u32()?,
            });
        }

        let mut outputs = Vec::new();

        for _ in 0..r.compact()? {
            outputs.push(TxOut::read(&mut r)?);
        }

        if witnessed {
            for _ in 0..inputs.len() {
                for _ in 0..r.compact()? {
                    r.sized()?;
                }
            }
        }

        let lock_time = r.u32()?;

        match r.done() {
            true => Ok((Transaction { version, inputs, outputs, lock_time }, witnessed)),
            false => Err("a transaction with more after it than it holds".to_string()),
        }
    }

    /// Its encoding without witnesses: what its id hashes, and a PSBT holds unsigned.
    pub fn base(&self) -> Vec<u8> {
        let mut out = self.version.to_le_bytes().to_vec();
        compact(&mut out, self.inputs.len());

        for input in &self.inputs {
            out.extend(input.txid);
            out.extend(input.vout.to_le_bytes());
            compact(&mut out, input.script_sig.len());
            out.extend(&input.script_sig);
            out.extend(input.sequence.to_le_bytes());
        }

        compact(&mut out, self.outputs.len());

        for output in &self.outputs {
            output.write(&mut out);
        }

        out.extend(self.lock_time.to_le_bytes());

        out
    }

    /// Its id, as an input names it: the double SHA-256 of it without witnesses, as hashed, the
    /// reverse of how explorers write it.
    pub fn txid(&self) -> [u8; 32] {
        sha256d(&self.base())
    }

    /// What BIP143 signs for an input that spends a native segwit output of a key's hash: all of
    /// the transaction, as SIGHASH_ALL has it, the input's place in it, and the amount it spends.
    pub fn preimage(&self, index: usize, key_hash: &[u8; 20], value: u64) -> Vec<u8> {
        let (mut prevouts, mut sequences, mut outputs) = (Vec::new(), Vec::new(), Vec::new());

        for input in &self.inputs {
            prevouts.extend(input.txid);
            prevouts.extend(input.vout.to_le_bytes());
            sequences.extend(input.sequence.to_le_bytes());
        }

        for output in &self.outputs {
            output.write(&mut outputs);
        }

        let input = &self.inputs[index];
        let mut out = self.version.to_le_bytes().to_vec();
        out.extend(sha256d(&prevouts));
        out.extend(sha256d(&sequences));
        out.extend(input.txid);
        out.extend(input.vout.to_le_bytes());
        // The script code: the key hash's pay-to-key-hash script, as BIP143 has it for P2WPKH.
        out.extend([0x19, 0x76, 0xa9, 0x14]);
        out.extend(key_hash);
        out.extend([0x88, 0xac]);
        out.extend(value.to_le_bytes());
        out.extend(input.sequence.to_le_bytes());
        out.extend(sha256d(&outputs));
        out.extend(self.lock_time.to_le_bytes());
        out.extend(SIGHASH_ALL.to_le_bytes());

        out
    }
}

impl TxOut {
    fn read(r: &mut Reader) -> Result<TxOut, String> {
        Ok(TxOut {
            value: r.u64()?,
            script: r.sized()?.to_vec(),
        })
    }

    fn write(&self, out: &mut Vec<u8>) {
        out.extend(self.value.to_le_bytes());
        compact(out, self.script.len());
        out.extend(&self.script);
    }
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
                    inputs.push(Spent { value: output.value, yours: None });

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

            if output.script != p2wpkh(&hash160(&key)) {
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
            signing.push(Signing { index, network, chain, number, key, value: output.value });
            inputs.push(Spent { value: output.value, yours: Some(shown) });
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
            let preimage = self.tx.preimage(input.index, &hash160(&input.key), input.value);
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

/// A script as its address, on a network: a key's or a script's hash in base58, 1… or 3…, a
/// witness program in bech32, bc1q…, or bech32m, bc1p…. Data, and scripts with no address, are
/// told as what they are.
pub fn address(script: &[u8], network: Network) -> String {
    let (key_hash, script_hash) = network.versions();

    match script {
        [0x76, 0xa9, 0x14, hash @ .., 0x88, 0xac] if hash.len() == 20 => base58check(key_hash, hash),
        [0xa9, 0x14, hash @ .., 0x87] if hash.len() == 20 => base58check(script_hash, hash),
        [0x00, size @ (0x14 | 0x20), program @ ..] if program.len() == usize::from(*size) => bech32::encode_segwit(network.hrp(), program),
        [version @ 0x51..=0x60, size @ 2..=40, program @ ..] if program.len() == usize::from(*size) => bech32::encode_witness(network.hrp(), version - 0x50, program),
        [0x6a, ..] => format!("data, burned: {}", to_hex(&script[1..])),
        _ => format!("a script with no address: {}", to_hex(script)),
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

/// Amounts added up, None past all the bitcoin there are.
fn total(values: impl Iterator<Item = u64>) -> Option<u64> {
    values.into_iter().try_fold(0u64, |sum, value| sum.checked_add(value).filter(|sum| *sum <= MAX_MONEY))
}

/// The native segwit script paying a key's hash: 0, and the 20 bytes.
fn p2wpkh(hash: &[u8; 20]) -> Vec<u8> {
    [&[0x00, 0x14][..], hash].concat()
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

/// A signature, r then s, as DER writes it, as Bitcoin's scripts take it: each a whole number in
/// its fewest bytes, a 0 before one whose top bit is set.
pub fn der(signature: &[u8; 64]) -> Vec<u8> {
    let integer = |bytes: &[u8]| {
        let trimmed = &bytes[bytes.iter().position(|byte| *byte != 0).unwrap_or(bytes.len() - 1)..];

        match trimmed[0] & 0x80 {
            0 => trimmed.to_vec(),
            _ => [&[0][..], trimmed].concat(),
        }
    };
    let (r, s) = (integer(&signature[..32]), integer(&signature[32..]));
    let mut out = vec![0x30, (4 + r.len() + s.len()) as u8, 0x02, r.len() as u8];
    out.extend(r);
    out.extend([0x02, s.len() as u8]);
    out.extend(s);

    out
}

/// Bytes behind a version byte, with a checksum, in base58: Bitcoin's legacy addresses.
fn base58check(version: u8, payload: &[u8]) -> String {
    const ALPHABET: &[u8; 58] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
    let mut bytes = vec![version];
    bytes.extend(payload);
    let check = sha256d(&bytes);
    bytes.extend(&check[..4]);

    // The digits, least first: each byte multiplies them by 256 and adds itself.
    let mut digits: Vec<u8> = Vec::new();

    for byte in &bytes {
        let mut carry = u32::from(*byte);

        for digit in &mut digits {
            carry += u32::from(*digit) << 8;
            *digit = (carry % 58) as u8;
            carry /= 58;
        }

        while carry > 0 {
            digits.push((carry % 58) as u8);
            carry /= 58;
        }
    }

    // Leading zero bytes are 1s, as they are no digits.
    let zeros = bytes.iter().take_while(|byte| **byte == 0).count();

    "1".repeat(zeros) + &digits.iter().rev().map(|digit| char::from(ALPHABET[usize::from(*digit)])).collect::<String>()
}

/// Bitcoin's compact size, in its fewest bytes.
fn compact(out: &mut Vec<u8>, size: usize) {
    match size {
        0..0xfd => out.push(size as u8),
        0xfd..=0xffff => {
            out.push(0xfd);
            out.extend((size as u16).to_le_bytes());
        }
        0x1_0000..=0xffff_ffff => {
            out.push(0xfe);
            out.extend((size as u32).to_le_bytes());
        }
        _ => {
            out.push(0xff);
            out.extend((size as u64).to_le_bytes());
        }
    }
}

/// Reading bytes in order, refusing to read past their end.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Reader<'a> {
        Reader { bytes, at: 0 }
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8], String> {
        let end = self.at.checked_add(count).filter(|end| *end <= self.bytes.len()).ok_or("cut short: it ends before what it holds")?;
        let taken = &self.bytes[self.at..end];
        self.at = end;

        Ok(taken)
    }

    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().expect("4 bytes")))
    }

    fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().expect("8 bytes")))
    }

    /// A compact size, as it must be written: in its fewest bytes, as Bitcoin reads it.
    fn compact(&mut self) -> Result<u64, String> {
        let (size, least) = match self.take(1)?[0] {
            0xfd => (u64::from(u16::from_le_bytes(self.take(2)?.try_into().expect("2 bytes"))), 0xfd),
            0xfe => (u64::from(u32::from_le_bytes(self.take(4)?.try_into().expect("4 bytes"))), 0x1_0000),
            0xff => (u64::from_le_bytes(self.take(8)?.try_into().expect("8 bytes")), 0x1_0000_0000),
            small => return Ok(u64::from(small)),
        };

        match size >= least {
            true => Ok(size),
            false => Err("a size not written in its fewest bytes".to_string()),
        }
    }

    /// A compact size, then that many bytes.
    fn sized(&mut self) -> Result<&'a [u8], String> {
        let size = self.compact()?;

        self.take(usize::try_from(size).map_err(|_| "a size past what this device holds")?)
    }

    fn done(&self) -> bool {
        self.at == self.bytes.len()
    }
}

fn text(err: impl std::fmt::Display) -> String {
    format!("a code that does not write: {err}")
}

#[cfg(test)]
mod tests;
