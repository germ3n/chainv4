//! Atomic swaps between chainv4 and Bitcoin: coins or a token here for bitcoin there, with no one
//! in between and no way for either side to take both. Each side locks what it pays under the same
//! hash: on chainv4 in the swap contract (contracts/lua/swap.lua), on Bitcoin in an HTLC
//! ([htlc]). Whoever starts knows the secret behind the hash, locks first and with the longer
//! deadline; the other locks second, with a shorter one. The starter claims the other's lock with
//! the secret, which shows it on that chain, and the other claims the starter's lock with it in
//! turn. Whoever does not claim in time gets their own lock back after its deadline.
//!
//! The terms go between the two as codes, ur:chainv4-swap: the starter's offer, the amounts, the
//! hash, the deadlines and their own keys, and the answer, the same with the other's keys added.

pub mod htlc;

use {
    crate::crypto::bitcoin::tx::Network,
    crate::crypto::bitcoin::sha256,
    crate::structs::address::Address,
    crate::structs::u256::u256,
    wincode::{SchemaRead, SchemaWrite},
};

/// The UR type the terms travel as.
pub const UR_TYPE: &str = "chainv4-swap";

/// How long before the first lock's deadline the second's must end, at least: time for the
/// starter, once the second lock stands, to claim it and so show the secret, and for the other to
/// claim the first with it before its refund opens.
pub const MIN_GAP_MS: u64 = 4 * 3_600_000;

/// The deadlines a swap gets unless its starter says otherwise: the first lock's, then the
/// second's, from when it starts.
pub const FIRST_DEADLINE_MS: u64 = 24 * 3_600_000;
pub const SECOND_DEADLINE_MS: u64 = 12 * 3_600_000;

/// The chain on the other side of a swap. Bitcoin's networks for now; another chain that locks
/// coins under a SHA-256 hash and a deadline is a new tag here, its own lock and spend beside
/// htlc's, with nothing to change on chainv4's side.
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
pub enum Counterparty {
    #[wincode(tag = 0)]
    Bitcoin,
    /// Bitcoin's test networks: testnet, signet and regtest.
    #[wincode(tag = 1)]
    BitcoinTestnet,
}

/// What the starter of a swap pays.
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
pub enum Pays {
    /// Bitcoin, for coins or a token on chainv4.
    #[wincode(tag = 0)]
    Bitcoin,
    /// Coins or a token on chainv4, for bitcoin.
    #[wincode(tag = 1)]
    Chain,
}

/// A swap's terms, as both sides hold them once agreed. An offer leaves out the answerer's keys.
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct Terms {
    /// The chain on the other side.
    pub chain: Counterparty,
    /// The SHA-256 of the starter's secret.
    pub hash: [u8; 32],
    /// The bitcoin locked, in satoshis.
    pub sats: u64,
    /// The token locked on chainv4, or None for its coin.
    pub token: Option<Address>,
    /// How much of it, in its smallest units.
    #[wincode(with = "crate::structs::u256::U256Le")]
    pub amount: u256,
    /// What the starter pays.
    pub starter: Pays,
    /// Whose bitcoin it is: the HTLC's refund key.
    pub bitcoin_payer: Option<[u8; 33]>,
    /// Who takes the bitcoin with the secret: the HTLC's claim key.
    pub bitcoin_receiver: Option<[u8; 33]>,
    /// Who locks on chainv4, and gets it back after the deadline.
    pub chain_payer: Option<Address>,
    /// Who takes what is locked on chainv4 with the secret.
    pub chain_receiver: Option<Address>,
    /// When the starter's lock, the first, opens to a refund: Unix milliseconds.
    pub first_deadline: u64,
    /// When the other's lock, the second, does.
    pub second_deadline: u64,
}

/// A secret for a swap's starter: 32 random bytes, and their SHA-256, which the terms carry.
pub fn secret() -> Result<([u8; 32], [u8; 32]), String> {
    let mut secret = [0u8; 32];
    getrandom::fill(&mut secret).map_err(|err| format!("no randomness for a secret: {err}"))?;

    Ok((secret, sha256(&secret)))
}

impl Terms {
    /// The Bitcoin network it is on.
    pub fn network(&self) -> Network {
        match self.chain {
            Counterparty::BitcoinTestnet => Network::Test,
            Counterparty::Bitcoin => Network::Main,
        }
    }

    /// Whether both sides' keys are in: an answer, rather than an offer.
    pub fn agreed(&self) -> bool {
        self.bitcoin_payer.is_some() && self.bitcoin_receiver.is_some() && self.chain_payer.is_some() && self.chain_receiver.is_some()
    }

    /// When the lock on chainv4 ends: the first deadline if the starter locks there, else the
    /// second.
    pub fn chain_deadline(&self) -> u64 {
        match self.starter {
            Pays::Chain => self.first_deadline,
            Pays::Bitcoin => self.second_deadline,
        }
    }

    /// When the lock on Bitcoin ends.
    pub fn bitcoin_deadline(&self) -> u64 {
        match self.starter {
            Pays::Bitcoin => self.first_deadline,
            Pays::Chain => self.second_deadline,
        }
    }

    /// The HTLC on Bitcoin, once both keys are in. Its lock time is the Bitcoin deadline in
    /// seconds.
    pub fn htlc(&self) -> Option<htlc::Htlc> {
        Some(htlc::Htlc {
            hash: self.hash,
            claim: self.bitcoin_receiver?,
            refund: self.bitcoin_payer?,
            locktime: u32::try_from(self.bitcoin_deadline() / 1_000).ok()?,
        })
    }

    /// Checks the terms hold together, at a time: something on each side, deadlines ahead and
    /// far enough apart, keys that are keys and not each other's.
    pub fn check(&self, now: u64) -> Result<(), String> {
        if self.sats < htlc::DUST || self.amount == u256::ZERO {
            return Err(format!("a swap pays something on each side: at least {} satoshis, and more than nothing on chainv4", htlc::DUST));
        }

        if self.second_deadline <= now {
            return Err("its deadlines have passed".to_string());
        }

        if self.second_deadline + MIN_GAP_MS > self.first_deadline {
            return Err(format!("the second lock must end at least {} hours before the first, so the second side has time to claim", MIN_GAP_MS / 3_600_000));
        }

        if self.bitcoin_deadline() / 1_000 < u64::from(htlc::LOCKTIME_THRESHOLD) || self.bitcoin_deadline() / 1_000 > u64::from(u32::MAX) {
            return Err("a Bitcoin deadline Bitcoin cannot write as a time".to_string());
        }

        for key in [self.bitcoin_payer, self.bitcoin_receiver].into_iter().flatten() {
            if !crate::crypto::secp256k1::valid_key(&key) {
                return Err("a Bitcoin key that is not a key".to_string());
            }
        }

        if self.bitcoin_payer.is_some() && self.bitcoin_payer == self.bitcoin_receiver {
            return Err("the same Bitcoin key on both sides".to_string());
        }

        if self.chain_payer.is_some() && self.chain_payer == self.chain_receiver {
            return Err("the same chainv4 address on both sides".to_string());
        }

        Ok(())
    }

    /// The terms as a code: ur:chainv4-swap, in capitals.
    pub fn to_text(&self) -> String {
        ur::encode(&wincode::serialize(self).expect("terms always encode"), &ur::Type::Custom(UR_TYPE)).to_ascii_uppercase()
    }

    /// Reads terms from their code, whole: anything past them is refused.
    pub fn from_text(text: &str) -> Result<Terms, String> {
        let text = text.trim().to_ascii_lowercase();

        if !text.starts_with(&format!("ur:{UR_TYPE}/")) {
            return Err(format!("not a swap's terms: those start ur:{UR_TYPE}/"));
        }

        match ur::decode(&text).map_err(|err| format!("a code that does not read: {err:?}"))? {
            (ur::ur::Kind::SinglePart, bytes) => Terms::from_bytes(&bytes),
            (ur::ur::Kind::MultiPart, _) => Err("one part of an animated code: read every part".to_string()),
        }
    }

    /// Reads terms from their encoding, all of these bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Terms, String> {
        let terms: Terms = wincode::deserialize(bytes).map_err(|_| "terms that do not decode".to_string())?;

        match wincode::serialize(&terms).ok().as_deref() == Some(bytes) {
            true => Ok(terms),
            false => Err("terms with more in them than they hold".to_string()),
        }
    }
}

#[cfg(test)]
mod tests;
