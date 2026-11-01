//! Partially signed transactions, BIP174's version 0: the transaction, unsigned, and what its
//! wallets say of it, its inputs and its outputs, in maps of pairs kept as they came, so a PSBT
//! goes back with all it held and the signatures added. What the offline signer reads and signs,
//! and what a native multisig's members pass round: built for its P2WSH coins, signed by each,
//! combined and finished into the transaction to broadcast.

use {
    super::multisig::{account_path, member_key, parse_multisig_script, witness_script},
    super::tx::{Network, Reader, SIGHASH_ALL, Transaction, TxIn, TxOut, address, compact, der, from_der, p2wsh},
    crate::registry::multisig::BitcoinXpub,
    std::collections::HashSet,
};

#[cfg(feature = "swap")]
use crate::swap::htlc::{Htlc, Spend};

/// The key types this reads and writes, as BIP174 numbers them.
pub const GLOBAL_UNSIGNED_TX: u8 = 0x00;
pub const GLOBAL_VERSION: u8 = 0xfb;
pub const IN_NON_WITNESS_UTXO: u8 = 0x00;
pub const IN_WITNESS_UTXO: u8 = 0x01;
pub const IN_PARTIAL_SIG: u8 = 0x02;
pub const IN_SIGHASH_TYPE: u8 = 0x03;
pub const IN_WITNESS_SCRIPT: u8 = 0x05;
pub const IN_BIP32_DERIVATION: u8 = 0x06;
pub const IN_FINAL_SCRIPTSIG: u8 = 0x07;
pub const IN_FINAL_SCRIPTWITNESS: u8 = 0x08;
pub const OUT_WITNESS_SCRIPT: u8 = 0x01;
pub const OUT_BIP32_DERIVATION: u8 = 0x02;

/// What every PSBT starts with.
pub const MAGIC: &[u8] = b"psbt\xff";


/// An index at or above this is hardened.
pub const HARDENED: u32 = 1 << 31;

/// The most there are of bitcoin, in satoshis: no amount is more.
pub const MAX_MONEY: u64 = 21_000_000 * 100_000_000;

/// One of a PSBT's pairs, kept as it came.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pair {
    pub key: Vec<u8>,
    pub value: Vec<u8>,
}

/// A partially signed transaction, BIP174's version 0: the transaction, unsigned, and what the
/// wallet says of it, its inputs and its outputs, in maps of pairs kept as they came, so a signed
/// PSBT goes back with all it held and the signatures added.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Psbt {
    pub global: Vec<Pair>,
    pub inputs: Vec<Vec<Pair>>,
    pub outputs: Vec<Vec<Pair>>,
    pub tx: Transaction,
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
    pub fn transaction(&self) -> &Transaction {
        &self.tx
    }

    /// The signatures an input holds, each a key's, 33 bytes, and the signature, DER and the
    /// sighash type after it.
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
    pub fn spent(&self, index: usize) -> Result<Option<(TxOut, bool)>, String> {
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

    /// A PSBT asking a native multisig's members to sign a swap's HTLC spend, for the branch the
    /// multisig takes: the spend unsigned, the coins, the HTLC's script, and every member's key at
    /// the receiving address index the swap's keys are at, with its origin.
    pub fn for_htlc_multisig(tx: &Transaction, value: u64, htlc: &Htlc, wallet: &MultisigWallet, index: u32) -> Result<Psbt, String> {
        let mut coins = Vec::new();
        TxOut { value, script: htlc.output() }.write(&mut coins);
        let mut pairs = wallet.describe(0, index, IN_WITNESS_SCRIPT, IN_BIP32_DERIVATION).ok_or("a member's key that does not derive")?;
        pairs[0].value = htlc.script();
        pairs.insert(0, Pair { key: vec![IN_WITNESS_UTXO], value: coins });

        Ok(Psbt {
            global: vec![Pair { key: vec![GLOBAL_UNSIGNED_TX], value: tx.base() }],
            inputs: vec![pairs],
            outputs: vec![Vec::new(); tx.outputs.len()],
            tx: Transaction { inputs: tx.inputs.iter().map(|input| TxIn { witness: Vec::new(), ..input.clone() }).collect(), ..tx.clone() },
        })
    }

    /// The spend, finished with the signatures of the branch the spend takes, worth value: one
    /// key's, or a multisig's threshold of them in its keys' order. The transaction to broadcast.
    /// Refused: fewer that hold than the branch takes.
    pub fn finish_htlc(&self, htlc: &Htlc, value: u64, spend: Spend) -> Result<Transaction, String> {
        let (needed, keys) = htlc.party(spend).keys();
        let preimage = self.tx.script_preimage(0, &htlc.script(), value);
        let held: Vec<Vec<u8>> = keys
            .iter()
            .filter_map(|key| find(&self.inputs[0], &[&[IN_PARTIAL_SIG][..], key].concat()).map(|signature| (key, signature)))
            .filter(|(key, signature)| {
                signature
                    .split_last()
                    .filter(|(kind, _)| u32::from(**kind) == SIGHASH_ALL)
                    .and_then(|(_, der)| crate::crypto::bitcoin::tx::from_der(der))
                    .is_some_and(|signature| crate::crypto::bitcoin::verify(key, &preimage, &signature))
            })
            .map(|(_, signature)| signature.to_vec())
            .take(needed as usize)
            .collect();

        let signed = keys.iter().any(|key| find(&self.inputs[0], &[&[IN_PARTIAL_SIG][..], key].concat()).is_some());

        match held.len() as u32 {
            0 if needed == 1 && !signed => return Err("the signed PSBT holds no signature by the swap's key".to_string()),
            0 if needed == 1 => return Err("the signer's signature does not hold for this spend".to_string()),
            count if count < needed => return Err(format!("the PSBT holds {count} of the {needed} signatures the swap's multisig takes")),
            _ => {}
        }

        let mut tx = self.tx.clone();
        tx.inputs[0].witness = htlc.witness(&held, spend);

        Ok(tx)
    }
}

/// The keys a script checks that a multisig's members may hold: an M-of-N's, or a swap's HTLC's,
/// both of its branches.
fn script_keys(script: &[u8]) -> Option<Vec<[u8; 33]>> {
    if let Some((_, keys)) = parse_multisig_script(script) {
        return Some(keys);
    }

    #[cfg(feature = "swap")]
    if let Some(htlc) = Htlc::parse(script) {
        return Some([htlc.claim.keys().1, htlc.refund.keys().1].concat());
    }

    None
}

/// Amounts added up, None past all the bitcoin there are.
pub fn total(values: impl Iterator<Item = u64>) -> Option<u64> {
    values.into_iter().try_fold(0u64, |sum, value| sum.checked_add(value).filter(|sum| *sum <= MAX_MONEY))
}

/// A path as wallets write it: m/84'/0'/0'/1/3.
pub fn path_text(path: &[u32]) -> String {
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
pub struct Derived {
    pub key: [u8; 33],
    pub fingerprint: [u8; 4],
    pub path: Vec<u32>,
}

/// The keys a map derives, by type. Pairs of keys of another size are no key of this seed's.
pub fn derivations(pairs: &[Pair], kind: u8) -> Result<Vec<Derived>, String> {
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
pub fn find<'a>(pairs: &'a [Pair], key: &[u8]) -> Option<&'a [u8]> {
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

/// An input's signatures that check out, its threshold and its witness script.
type Held = (Vec<Vec<u8>>, u32, Vec<u8>);

/// What a member agrees to by signing a spend: each output's address and satoshis, what its coins
/// hold, and the fee, what they hold past what it pays.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpendReview {
    pub outputs: Vec<(String, u64)>,
    pub spent: u64,
    pub fee: u64,
}

/// One of a native multisig's coins a spend takes: what it holds, and the address it is at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MultisigCoin {
    pub value: u64,
    /// 0 for a receiving address, 1 for change.
    pub chain: u32,
    pub index: u32,
}

/// A native multisig on Bitcoin: its threshold, its members' xpubs and its network.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MultisigWallet {
    pub threshold: u32,
    pub xpubs: Vec<BitcoinXpub>,
    pub network: Network,
}

impl MultisigWallet {
    /// What a map says of one of its addresses: its witness script, and every member's key there,
    /// each with its origin, the seed's fingerprint and the path, so each member's wallet finds
    /// its own.
    fn describe(&self, chain: u32, index: u32, script_key: u8, derivation_key: u8) -> Option<Vec<Pair>> {
        let script = witness_script(self.threshold, &self.xpubs, chain, index)?;
        let mut pairs = vec![Pair { key: vec![script_key], value: script }];

        for xpub in &self.xpubs {
            let key = member_key(xpub, chain, index)?;
            let path = account_path(self.network).into_iter().chain([chain, index]);
            let value = xpub.fingerprint.iter().copied().chain(path.flat_map(u32::to_le_bytes)).collect();
            pairs.push(Pair { key: [&[derivation_key][..], &key].concat(), value });
        }

        Some(pairs)
    }
}

impl Psbt {
    /// A PSBT spending a native multisig's coins: the transaction unsigned, each input with the
    /// coins it spends, its witness script and every member's key and origin, and each change
    /// output, given by its place and the change address's number, said to be the multisig's own,
    /// so each member's wallet sees it come back.
    pub fn for_multisig(tx: &Transaction, wallet: &MultisigWallet, coins: &[MultisigCoin], change: &[(usize, u32)]) -> Result<Psbt, String> {
        if coins.len() != tx.inputs.len() {
            return Err("a coin for each input".to_string());
        }

        let mut inputs = Vec::new();

        for coin in coins {
            let mut pairs = wallet.describe(coin.chain, coin.index, IN_WITNESS_SCRIPT, IN_BIP32_DERIVATION).ok_or("a member's key that does not derive")?;
            let mut held = Vec::new();
            TxOut { value: coin.value, script: p2wsh(&pairs[0].value) }.write(&mut held);
            pairs.insert(0, Pair { key: vec![IN_WITNESS_UTXO], value: held });
            inputs.push(pairs);
        }

        let mut outputs = vec![Vec::new(); tx.outputs.len()];

        for &(at, index) in change {
            let pairs = wallet.describe(1, index, OUT_WITNESS_SCRIPT, OUT_BIP32_DERIVATION).ok_or("a member's key that does not derive")?;

            match tx.outputs.get(at) {
                Some(output) if output.script == p2wsh(&pairs[0].value) => outputs[at] = pairs,
                _ => return Err(format!("output {} is not the multisig's change address {index}", at + 1)),
            }
        }

        Ok(Psbt {
            global: vec![Pair { key: vec![GLOBAL_UNSIGNED_TX], value: tx.base() }],
            inputs,
            outputs,
            tx: Transaction { inputs: tx.inputs.iter().map(|input| TxIn { witness: Vec::new(), script_sig: Vec::new(), ..input.clone() }).collect(), ..tx.clone() },
        })
    }

    /// Takes in another member's partial signatures for the same transaction, as members' wallets
    /// send their PSBTs back. Refused: another transaction.
    pub fn combine(&mut self, other: &Psbt) -> Result<(), String> {
        if other.tx != self.tx {
            return Err("a PSBT of another transaction: combine the ones signed for this one".to_string());
        }

        for (ours, theirs) in self.inputs.iter_mut().zip(&other.inputs) {
            for pair in theirs.iter().filter(|pair| pair.key.first() == Some(&IN_PARTIAL_SIG)) {
                if !ours.iter().any(|held| held.key == pair.key) {
                    ours.push(pair.clone());
                }
            }
        }

        Ok(())
    }

    /// Signs every input with the keys a secret gives for a member's origin: for each key an input
    /// names at the fingerprint, the secret of its path, when the function knows it and the key it
    /// makes is the one named. What a member's wallet does, and the CLI with the member's words.
    pub fn sign_multisig(&mut self, fingerprint: [u8; 4], secret: impl Fn(&[u32]) -> Option<[u8; 32]>) -> Result<usize, String> {
        let mut signed = 0;

        for index in 0..self.inputs.len() {
            let n = index + 1;
            let pairs = &self.inputs[index];
            let Some(script) = find(pairs, &[IN_WITNESS_SCRIPT]).map(<[u8]>::to_vec) else {
                continue;
            };
            let Some(keys) = script_keys(&script) else {
                continue;
            };
            let (output, _) = self.spent(index)?.ok_or_else(|| format!("input {n} says nothing of the coins it spends"))?;

            if output.script != p2wsh(&script) {
                return Err(format!("input {n}'s script is not the one its coins are locked to"));
            }

            for derived in derivations(pairs, IN_BIP32_DERIVATION)?.into_iter().filter(|derived| derived.fingerprint == fingerprint && keys.contains(&derived.key)) {
                let Some(secret) = secret(&derived.path) else {
                    continue;
                };

                if crate::crypto::secp256k1::public_key(&secret) != Some(derived.key) {
                    return Err(format!("input {n} names a key at {} that is not this seed's", path_text(&derived.path)));
                }

                let preimage = self.tx.script_preimage(index, &script, output.value);
                let mut signature = der(&crate::crypto::bitcoin::sign(&secret, &preimage));
                signature.push(SIGHASH_ALL as u8);
                let key = [&[IN_PARTIAL_SIG][..], &derived.key].concat();
                let pairs = &mut self.inputs[index];
                pairs.retain(|pair| pair.key != key);
                pairs.push(Pair { key, value: signature });
                signed += 1;
            }
        }

        Ok(signed)
    }

    /// What a member agrees to by signing a multisig's spend, for it to look at first: where each
    /// output goes and how much, and the fee, read from the spend itself and the coins' amounts that
    /// BIP143 signs. An error for a spend it cannot say that of: an input with no amount, inputs on
    /// more than one network or naming none, an output with no address, or one paying more than it
    /// spends.
    pub fn multisig_review(&self) -> Result<SpendReview, String> {
        let mut amounts = Vec::new();
        let mut network = None;

        for index in 0..self.inputs.len() {
            let (output, _) = self.spent(index)?.ok_or_else(|| format!("input {} says nothing of the coins it spends: its fee cannot be told", index + 1))?;
            amounts.push(output.value);

            for derived in derivations(&self.inputs[index], IN_BIP32_DERIVATION)? {
                let named = match derived.path.get(1).map(|coin| coin & !(1 << 31)) {
                    Some(0) => Network::Main,
                    Some(_) => Network::Test,
                    None => return Err(format!("input {} names a key at a path that is no multisig's", index + 1)),
                };

                if network.replace(named).is_some_and(|had| had != named) {
                    return Err("its inputs name keys of both main and test networks".to_string());
                }
            }
        }

        let network = network.ok_or("none of its inputs names a key: which network it pays on cannot be told")?;
        let mut outputs = Vec::new();

        for (index, output) in self.tx.outputs.iter().enumerate() {
            let to = address(&output.script, network);

            if to.starts_with("a script with no address") {
                return Err(format!("output {} is {to}", index + 1));
            }

            outputs.push((to, output.value));
        }

        let spent = total(amounts.into_iter()).ok_or("its inputs hold more than there could be")?;
        let paid = total(self.tx.outputs.iter().map(|output| output.value)).ok_or("its outputs pay more than there could be")?;
        let fee = spent.checked_sub(paid).ok_or_else(|| format!("its outputs pay {paid} satoshis, more than the {spent} it spends"))?;

        Ok(SpendReview { outputs, spent, fee })
    }

    /// How many signatures each input holds that check out, and how many it needs: what a wallet
    /// shows while the members sign. Inputs of no multisig are left out.
    pub fn multisig_progress(&self) -> Vec<(usize, u32)> {
        (0..self.inputs.len()).filter_map(|index| self.multisig_signatures(index).ok().map(|(held, needed, _)| (held.len(), needed))).collect()
    }

    /// An input's signatures that check out, in its script's key order, its threshold, and its
    /// script.
    fn multisig_signatures(&self, index: usize) -> Result<Held, String> {
        let n = index + 1;
        let pairs = &self.inputs[index];
        let script = find(pairs, &[IN_WITNESS_SCRIPT]).ok_or_else(|| format!("input {n} holds no witness script"))?.to_vec();
        let (threshold, keys) = parse_multisig_script(&script).ok_or_else(|| format!("input {n}'s script is no multisig"))?;
        let (output, _) = self.spent(index)?.ok_or_else(|| format!("input {n} says nothing of the coins it spends"))?;
        let preimage = self.tx.script_preimage(index, &script, output.value);
        let held = keys
            .iter()
            .filter_map(|key| find(pairs, &[&[IN_PARTIAL_SIG][..], key].concat()).map(|signature| (key, signature)))
            .filter(|(key, signature)| {
                signature
                    .split_last()
                    .filter(|(kind, _)| u32::from(**kind) == SIGHASH_ALL)
                    .and_then(|(_, der)| from_der(der))
                    .is_some_and(|signature| crate::crypto::bitcoin::verify(key, &preimage, &signature))
            })
            .map(|(_, signature)| signature.to_vec())
            .collect();

        Ok((held, threshold, script))
    }

    /// The transaction to broadcast, each input's witness its multisig's: an empty push, as
    /// CHECKMULTISIG takes one too many, the first threshold signatures that check out in the
    /// script's key order, and the script. Refused: an input short of its threshold.
    pub fn finish_multisig(&self) -> Result<Transaction, String> {
        let mut tx = self.tx.clone();

        for (index, input) in tx.inputs.iter_mut().enumerate() {
            let (held, threshold, script) = self.multisig_signatures(index)?;

            if held.len() < threshold as usize {
                return Err(format!("input {} holds {} of the {threshold} signatures it needs", index + 1, held.len()));
            }

            input.witness = std::iter::once(Vec::new()).chain(held.into_iter().take(threshold as usize)).chain([script]).collect();
        }

        Ok(tx)
    }
}

/// One of a native multisig's coins, where it is: what Esplora or a light client found at one of
/// its addresses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Utxo {
    /// The transaction it is in, its id as hashed, not as explorers write it.
    pub txid: [u8; 32],
    pub vout: u32,
    pub coin: MultisigCoin,
}

/// The smallest output worth making, in satoshis: change below it goes to the fee instead.
const DUST: u64 = 330;

impl MultisigWallet {
    /// A spend's virtual size, signed: its inputs each with threshold signatures and the script,
    /// and its outputs. What its fee is worked out from.
    fn vsize(&self, inputs: usize, outputs: &[usize]) -> u64 {
        let script = 3 + 34 * self.xpubs.len() as u64;
        // The witness: its count, the empty push, the signatures, the script, each with its length.
        let witness = 1 + 1 + u64::from(self.threshold) * 73 + 1 + script;
        let base = 10 + 41 * inputs as u64 + outputs.iter().map(|script| 9 + *script as u64).sum::<u64>();

        (4 * base + 2 + witness * inputs as u64).div_ceil(4)
    }

    /// A PSBT paying an amount to a script from these coins, largest first until they cover it
    /// and the fee at fee_rate satoshis a virtual byte, its change, unless it would be dust, to the
    /// multisig's change address change_index.
    pub fn spend(&self, utxos: &[Utxo], to: Vec<u8>, amount: u64, fee_rate: u64, change_index: u32) -> Result<Psbt, String> {
        let mut sorted = utxos.to_vec();
        sorted.sort_by_key(|utxo| std::cmp::Reverse(utxo.coin.value));
        let change = p2wsh(&witness_script(self.threshold, &self.xpubs, 1, change_index).ok_or("a member's key that does not derive")?);
        let (mut taken, mut held) = (Vec::new(), 0u64);

        for utxo in sorted {
            taken.push(utxo);
            held = held.checked_add(utxo.coin.value).ok_or("more than there are bitcoin")?;
            let fee = fee_rate * self.vsize(taken.len(), &[to.len(), change.len()]);

            if held >= amount + fee {
                break;
            }
        }

        let with_change = fee_rate * self.vsize(taken.len(), &[to.len(), change.len()]);
        let without = fee_rate * self.vsize(taken.len(), &[to.len()]);

        if held < amount + without {
            return Err(format!("the multisig holds {held} satoshis here, short of {} and the fee", amount));
        }

        let mut outputs = vec![TxOut { value: amount, script: to }];
        let mut changed = Vec::new();

        if held >= amount + with_change + DUST {
            outputs.push(TxOut { value: held - amount - with_change, script: change });
            changed.push((1, change_index));
        }

        let tx = Transaction {
            version: 2,
            inputs: taken.iter().map(|utxo| TxIn { txid: utxo.txid, vout: utxo.vout, script_sig: Vec::new(), sequence: 0xffff_fffd, witness: Vec::new() }).collect(),
            outputs,
            lock_time: 0,
        };
        let coins: Vec<MultisigCoin> = taken.iter().map(|utxo| utxo.coin).collect();

        Psbt::for_multisig(&tx, self, &coins, &changed)
    }
}

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

impl Psbt {
    /// Its bytes in standard base64: how Bitcoin Core, Sparrow and most wallets pass a PSBT as text.
    pub fn to_base64(&self) -> String {
        let bytes = self.to_bytes();
        let mut out = String::new();

        for chunk in bytes.chunks(3) {
            let n = chunk.iter().enumerate().fold(0u32, |n, (at, byte)| n | u32::from(*byte) << (16 - 8 * at));

            for at in 0..4 {
                out.push(match at <= chunk.len() {
                    true => char::from(BASE64[(n >> (18 - 6 * at) & 63) as usize]),
                    false => '=',
                });
            }
        }

        out
    }

    /// Reads one from its base64, whitespace ignored.
    pub fn from_base64(text: &str) -> Result<Psbt, String> {
        let digits: Vec<u8> = text.bytes().filter(|byte| !byte.is_ascii_whitespace()).collect();
        let mut bytes = Vec::new();

        for chunk in digits.chunks(4) {
            let mut n = 0u32;
            let mut held = 0;

            for (at, digit) in chunk.iter().enumerate() {
                if *digit == b'=' {
                    break;
                }

                let value = BASE64.iter().position(|known| known == digit).ok_or("not base64")? as u32;
                n |= value << (18 - 6 * at);
                held += 1;
            }

            bytes.extend(&n.to_be_bytes()[1..held.max(1)]);
        }

        Psbt::read(&bytes)
    }
}

#[cfg(test)]
#[cfg(feature = "mnemonic")]
mod tests;
