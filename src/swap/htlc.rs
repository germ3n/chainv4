//! Bitcoin's side of a swap: a hash time-locked contract, as a P2WSH script. Its coins go to the
//! claim key with the secret whose SHA-256 is the hash, or after the lock time back to the
//! refund key:
//!
//! ```text
//! OP_IF
//!     OP_SIZE 32 OP_EQUALVERIFY OP_SHA256 <hash> OP_EQUALVERIFY <claim key>
//! OP_ELSE
//!     <lock time> OP_CHECKLOCKTIMEVERIFY OP_DROP <refund key>
//! OP_ENDIF
//! OP_CHECKSIG
//! ```
//!
//! A side that is a native multisig checks M of its members' keys instead, each branch ending in
//! its own check, a key's OP_CHECKSIG or the multisig's OP_CHECKMULTISIG:
//!
//! ```text
//! OP_IF
//!     OP_SIZE 32 OP_EQUALVERIFY OP_SHA256 <hash> OP_EQUALVERIFY <claim check>
//! OP_ELSE
//!     <lock time> OP_CHECKLOCKTIMEVERIFY OP_DROP <refund check>
//! OP_ENDIF
//! ```
//!
//! The secret must be 32 bytes, as on chainv4's side, so a secret one chain takes the other
//! takes too. The lock time is a Unix time in seconds, which Bitcoin checks against the median
//! time of the last eleven blocks: a refund confirms an hour or so after it, not at once.

use {
    super::BitcoinParty,
    crate::crypto::bitcoin::tx::{Network, SIGHASH_ALL, Transaction, TxIn, TxOut, der, p2wsh},
    crate::crypto::bitcoin::{self, sha256},
    crate::structs::bech32,
    r#macro::document,
};

const OP_IF: u8 = 0x63;
const OP_ELSE: u8 = 0x67;
const OP_ENDIF: u8 = 0x68;
const OP_DROP: u8 = 0x75;
const OP_SIZE: u8 = 0x82;
const OP_EQUALVERIFY: u8 = 0x88;
const OP_SHA256: u8 = 0xa8;
const OP_CHECKSIG: u8 = 0xac;
const OP_CHECKLOCKTIMEVERIFY: u8 = 0xb1;

#[document(
    summary = "The smallest lock time that is a Unix time, in seconds, rather than a block height.",
    category = "Swaps",
)]
pub const LOCKTIME_THRESHOLD: u32 = 500_000_000;

#[document(
    summary = "A Bitcoin output's dust limit for native segwit: one worth less is not relayed.",
    category = "Swaps",
)]
pub const DUST: u64 = 294;

#[document(summary = "A swap's hash time-locked contract on Bitcoin.", category = "Swaps")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Htlc {
    #[document(summary = "The SHA-256 of the secret.")]
    pub hash: [u8; 32],
    #[document(summary = "Who takes the coins with the secret: a key, or a multisig's M of N.")]
    pub claim: BitcoinParty,
    #[document(summary = "Who takes them back after the lock time.")]
    pub refund: BitcoinParty,
    #[document(summary = "When the refund opens: a Unix time, in seconds.")]
    pub locktime: u32,
}

#[document(
    summary = "The coins an HTLC holds on Bitcoin: the output paying it, by its transaction's id as hashed and its index there, and what it holds, in satoshis.",
    category = "Swaps",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Coins {
    pub txid: [u8; 32],
    pub vout: u32,
    pub value: u64,
}

#[document(summary = "How an HTLC's coins are spent.", category = "Swaps")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Spend {
    #[document(summary = "By the claim key, with the secret.")]
    Claim { secret: [u8; 32] },
    #[document(summary = "By the refund key, once the lock time passed.")]
    Refund,
}

impl Htlc {
    #[document(parent = "Htlc", summary = "Its witness script.")]
    pub fn script(&self) -> Vec<u8> {
        let mut out = vec![OP_IF, OP_SIZE, 0x01, 0x20, OP_EQUALVERIFY, OP_SHA256, 0x20];
        out.extend(self.hash);
        out.push(OP_EQUALVERIFY);

        // Two keys share one OP_CHECKSIG after the branches, as swaps' HTLCs always have.
        if let (BitcoinParty::Key(claim), BitcoinParty::Key(refund)) = (&self.claim, &self.refund) {
            out.push(0x21);
            out.extend(claim);
            out.push(OP_ELSE);
            push_number(&mut out, i64::from(self.locktime));
            out.extend([OP_CHECKLOCKTIMEVERIFY, OP_DROP, 0x21]);
            out.extend(refund);
            out.extend([OP_ENDIF, OP_CHECKSIG]);

            return out;
        }

        out.extend(self.claim.check());
        out.push(OP_ELSE);
        push_number(&mut out, i64::from(self.locktime));
        out.extend([OP_CHECKLOCKTIMEVERIFY, OP_DROP]);
        out.extend(self.refund.check());
        out.push(OP_ENDIF);

        out
    }

    #[document(
        parent = "Htlc",
        summary = "Reads one from its witness script: exactly one of these templates, two keys' or one with a multisig's branch, or None.",
    )]
    pub fn parse(script: &[u8]) -> Option<Htlc> {
        let rest = script.strip_prefix(&[OP_IF, OP_SIZE, 0x01, 0x20, OP_EQUALVERIFY, OP_SHA256, 0x20][..])?;
        let (hash, rest) = rest.split_at_checked(32)?;
        let rest = rest.strip_prefix(&[OP_EQUALVERIFY][..])?;
        let (claim, rest) = party(rest)?;
        let rest = rest.strip_prefix(&[OP_ELSE][..])?;
        let (&size, rest) = rest.split_first()?;

        if !(1..=5).contains(&size) {
            return None;
        }

        let (number, rest) = rest.split_at_checked(usize::from(size))?;
        let rest = rest.strip_prefix(&[OP_CHECKLOCKTIMEVERIFY, OP_DROP][..])?;
        let (refund, _) = party(rest)?;
        let htlc = Htlc {
            hash: hash.try_into().ok()?,
            claim,
            refund,
            locktime: u32::try_from(read_number(number)?).ok()?,
        };

        // Only the one way of writing it: what this module makes, byte for byte.
        (htlc.script() == script).then_some(htlc)
    }

    #[document(parent = "Htlc", summary = "The output that pays it: P2WSH of its script.")]
    pub fn output(&self) -> Vec<u8> {
        p2wsh(&self.script())
    }

    #[document(
        parent = "Htlc",
        summary = "Its address on a network: bc1q…, or tb1q…, longer than a key's, as a script's is.",
    )]
    pub fn address(&self, network: Network) -> String {
        bech32::encode_segwit(network.hrp(), &sha256(&self.script()))
    }

    #[document(
        parent = "Htlc",
        summary = "A transaction spending its coins to a script, less a fee of fee_rate satoshis a virtual byte, signed with the claim's or the refund's key. A refund carries the lock time, as CHECKLOCKTIMEVERIFY wants, and so is not mined before it. Refused: a key that is not the one the spend needs, a secret that is not the hash's, and a fee that leaves less than dust.",
    )]
    pub fn spend(&self, coins: Coins, to: Vec<u8>, fee_rate: u64, spend: Spend, secret: &[u8; 32]) -> Result<Transaction, String> {
        let key = crate::crypto::secp256k1::public_key(secret).ok_or("not a secp256k1 key")?;

        match spend {
            Spend::Claim { .. } if self.claim != BitcoinParty::Key(key) => return Err("a key that is not the swap's claim key".to_string()),
            Spend::Refund if self.refund != BitcoinParty::Key(key) => return Err("a key that is not the swap's refund key".to_string()),
            _ => {}
        }

        let mut tx = self.unsigned(coins, to, fee_rate, spend)?;
        let signature = self.signature(&tx, coins.value, secret);
        tx.inputs[0].witness = self.witness(&[signature], spend);

        Ok(tx)
    }

    #[document(parent = "Htlc", summary = "Who a spend's branch takes: the claim's party, or the refund's.")]
    pub fn party(&self, spend: Spend) -> &BitcoinParty {
        match spend {
            Spend::Claim { .. } => &self.claim,
            Spend::Refund => &self.refund,
        }
    }

    #[document(
        parent = "Htlc",
        summary = "The same spend, unsigned, its fee paid for the witness it will carry: what a PSBT asks an offline signer to sign.",
    )]
    pub fn unsigned(&self, coins: Coins, to: Vec<u8>, fee_rate: u64, spend: Spend) -> Result<Transaction, String> {
        let Coins { txid, vout, value } = coins;

        if let Spend::Claim { secret } = spend
            && sha256(&secret) != self.hash
        {
            return Err("a secret whose SHA-256 is not the swap's hash".to_string());
        }

        let (lock_time, sequence) = match spend {
            Spend::Claim { .. } => (0, 0xffff_fffd),
            // A sequence short of final, or the lock time is not checked.
            Spend::Refund => (self.locktime, 0xffff_fffe),
        };
        let mut tx = Transaction {
            version: 2,
            inputs: vec![TxIn { txid, vout, script_sig: Vec::new(), sequence, witness: Vec::new() }],
            outputs: vec![TxOut { value, script: to }],
            lock_time,
        };

        // Weighed with signatures as long as DER makes them, as many as the branch takes, then priced.
        let (needed, _) = self.party(spend).keys();
        tx.inputs[0].witness = self.witness(&vec![vec![0; 73]; needed as usize], spend);
        let fee = fee_rate.checked_mul(tx.vsize()).ok_or("a fee past what there is")?;
        let left = value.checked_sub(fee).filter(|left| *left >= DUST).ok_or_else(|| format!("{value} satoshis less a fee of {fee} leaves less than dust"))?;
        tx.outputs[0].value = left;
        tx.inputs[0].witness.clear();

        Ok(tx)
    }

    #[document(
        parent = "Htlc",
        summary = "The signature a key makes over a spend of its coins, worth value: BIP143's hash with the script as its code, DER, and SIGHASH_ALL after it.",
    )]
    pub fn signature(&self, tx: &Transaction, value: u64, secret: &[u8; 32]) -> Vec<u8> {
        let preimage = tx.script_preimage(0, &self.script(), value);
        let mut signature = der(&bitcoin::sign(secret, &preimage));
        signature.push(SIGHASH_ALL as u8);

        signature
    }

    #[document(
        parent = "Htlc",
        summary = "The witness that spends it: the signatures, a key's one, or a multisig's after the empty push CHECKMULTISIG takes one too many, in its keys' order; the secret and 1 for the claim's branch, or nothing for the refund's; and the script.",
    )]
    pub fn witness(&self, signatures: &[Vec<u8>], spend: Spend) -> Vec<Vec<u8>> {
        let mut witness = match self.party(spend) {
            BitcoinParty::Key(_) => Vec::new(),
            BitcoinParty::Multisig { .. } => vec![Vec::new()],
        };
        witness.extend(signatures.iter().cloned());

        match spend {
            Spend::Claim { secret } => witness.extend([secret.to_vec(), vec![1]]),
            Spend::Refund => witness.push(Vec::new()),
        }

        witness.push(self.script());

        witness
    }
}

#[document(
    summary = "The secret a claim revealed: the 32 bytes in a spend's witness that hash to the swap's hash, from the transaction that claimed it, for the other side to claim with in turn.",
    category = "Swaps",
)]
pub fn revealed(tx: &Transaction, hash: &[u8; 32]) -> Option<[u8; 32]> {
    tx.inputs.iter().flat_map(|input| &input.witness).filter_map(|item| <[u8; 32]>::try_from(item.as_slice()).ok()).find(|item| sha256(item) == *hash)
}

/// A branch's check read back, and what follows: a key and OP_CHECKSIG, a key before the shared
/// OP_CHECKSIG of two keys' template, or a multisig's.
fn party(script: &[u8]) -> Option<(BitcoinParty, &[u8])> {
    if let [0x21, rest @ ..] = script {
        let (key, rest) = rest.split_at_checked(33)?;
        let rest = rest.strip_prefix(&[OP_CHECKSIG][..]).unwrap_or(rest);

        return Some((BitcoinParty::Key(key.try_into().ok()?), rest));
    }

    // A multisig's check ends at its OP_CHECKMULTISIG: the first 0xae its keys' layout reaches.
    let end = (0..script.len()).find(|at| script[*at] == 0xae && crate::crypto::bitcoin::multisig::parse_multisig_script(&script[..=*at]).is_some())?;
    let (threshold, keys) = crate::crypto::bitcoin::multisig::parse_multisig_script(&script[..=end])?;

    Some((BitcoinParty::Multisig { threshold, keys }, &script[end + 1..]))
}

/// A number as a script pushes it: in its fewest little-endian bytes, the top bit the sign.
fn push_number(out: &mut Vec<u8>, number: i64) {
    let (mut magnitude, negative) = (number.unsigned_abs(), number < 0);
    let mut bytes = Vec::new();

    while magnitude > 0 {
        bytes.push((magnitude & 0xff) as u8);
        magnitude >>= 8;
    }

    match bytes.last().copied() {
        Some(last) if last & 0x80 != 0 => bytes.push(if negative { 0x80 } else { 0 }),
        Some(_) if negative => *bytes.last_mut().expect("one at least") |= 0x80,
        _ => {}
    }

    out.push(bytes.len() as u8);
    out.extend(bytes);
}

/// A number a script pushed, as push_number writes it.
fn read_number(bytes: &[u8]) -> Option<i64> {
    let (&last, _) = bytes.split_last()?;
    let mut magnitude = 0i64;

    for (index, byte) in bytes.iter().enumerate() {
        let byte = if index == bytes.len() - 1 { byte & 0x7f } else { *byte };
        magnitude |= i64::from(byte) << (8 * index);
    }

    Some(if last & 0x80 != 0 { -magnitude } else { magnitude })
}

#[cfg(test)]
mod tests;
