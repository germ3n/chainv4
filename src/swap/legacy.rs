//! Terms as they were written before legs: chainv4 against Bitcoin, the chain tag first. Codes made
//! then still read, as the same swap in two legs; they are written as legs from then on. And legs
//! as they were written before NFTs, which terms that lock none are still written as, so apps from
//! before NFTs read them.

use {
    super::{BitcoinLeg, BitcoinParty, ChainLeg, EvmLeg, Leg, Terms},
    crate::structs::address::Address,
    crate::structs::u256::u256,
    wincode::{SchemaRead, SchemaWrite},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
enum Counterparty {
    #[wincode(tag = 0)]
    Bitcoin,
    #[wincode(tag = 1)]
    BitcoinTestnet,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
enum Pays {
    #[wincode(tag = 0)]
    Bitcoin,
    #[wincode(tag = 1)]
    Chain,
}

#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
struct Legacy {
    chain: Counterparty,
    contract: Address,
    hash: [u8; 32],
    sats: u64,
    token: Option<Address>,
    #[wincode(with = "crate::structs::u256::U256Le")]
    amount: u256,
    starter: Pays,
    bitcoin_payer: Option<[u8; 33]>,
    bitcoin_receiver: Option<[u8; 33]>,
    chain_payer: Option<Address>,
    chain_receiver: Option<Address>,
    first_deadline: u64,
    second_deadline: u64,
}

/// Terms written before legs, all of these bytes.
pub(super) fn read(bytes: &[u8]) -> Result<Terms, String> {
    let old: Legacy = wincode::deserialize(bytes).map_err(|_| "terms that do not decode".to_string())?;

    if wincode::serialize(&old).ok().as_deref() != Some(bytes) {
        return Err("terms with more in them than they hold".to_string());
    }

    let chain = Leg::Chain(ChainLeg { contract: old.contract, token: old.token, item: None, amount: old.amount, payer: old.chain_payer, receiver: old.chain_receiver });
    let bitcoin = Leg::Bitcoin(BitcoinLeg { testnet: old.chain == Counterparty::BitcoinTestnet, sats: old.sats, payer: old.bitcoin_payer.map(BitcoinParty::Key), receiver: old.bitcoin_receiver.map(BitcoinParty::Key) });
    let (first, second) = match old.starter {
        Pays::Bitcoin => (bitcoin, chain),
        Pays::Chain => (chain, bitcoin),
    };

    Ok(Terms { hash: old.hash, first, second, first_deadline: old.first_deadline, second_deadline: old.second_deadline })
}

#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
struct TermsV2 {
    hash: [u8; 32],
    first: LegV2,
    second: LegV2,
    first_deadline: u64,
    second_deadline: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
enum LegV2 {
    #[wincode(tag = 0)]
    Chain(ChainLegV2),
    #[wincode(tag = 1)]
    Bitcoin(BitcoinLegV3),
    #[wincode(tag = 2)]
    Evm(EvmLegV2),
}

/// A Bitcoin leg as written before multisigs: one key each side.
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
struct BitcoinLegV3 {
    testnet: bool,
    sats: u64,
    payer: Option<[u8; 33]>,
    receiver: Option<[u8; 33]>,
}

impl BitcoinLegV3 {
    /// The leg as it was written, None when a side is a multisig, which it cannot hold.
    fn from_leg(leg: &BitcoinLeg) -> Option<BitcoinLegV3> {
        let key = |party: &Option<BitcoinParty>| match party {
            None => Some(None),
            Some(BitcoinParty::Key(key)) => Some(Some(*key)),
            Some(BitcoinParty::Multisig { .. }) => None,
        };

        Some(BitcoinLegV3 { testnet: leg.testnet, sats: leg.sats, payer: key(&leg.payer)?, receiver: key(&leg.receiver)? })
    }

    fn leg(self) -> BitcoinLeg {
        BitcoinLeg { testnet: self.testnet, sats: self.sats, payer: self.payer.map(BitcoinParty::Key), receiver: self.receiver.map(BitcoinParty::Key) }
    }
}

/// Terms as written before multisigs, legs that may lock NFTs.
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
struct TermsV3 {
    hash: [u8; 32],
    first: LegV3,
    second: LegV3,
    first_deadline: u64,
    second_deadline: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
enum LegV3 {
    #[wincode(tag = 0)]
    Chain(ChainLeg),
    #[wincode(tag = 1)]
    Bitcoin(BitcoinLegV3),
    #[wincode(tag = 2)]
    Evm(EvmLeg),
}

impl LegV3 {
    fn from_leg(leg: &Leg) -> Option<LegV3> {
        Some(match leg {
            Leg::Chain(leg) => LegV3::Chain(leg.clone()),
            Leg::Bitcoin(leg) => LegV3::Bitcoin(BitcoinLegV3::from_leg(leg)?),
            Leg::Evm(leg) => LegV3::Evm(leg.clone()),
        })
    }

    fn leg(self) -> Leg {
        match self {
            LegV3::Chain(leg) => Leg::Chain(leg),
            LegV3::Bitcoin(leg) => Leg::Bitcoin(leg.leg()),
            LegV3::Evm(leg) => Leg::Evm(leg),
        }
    }
}

/// Terms written as legs that may lock NFTs, before multisigs, after their first byte.
pub(super) fn read_items(bytes: &[u8]) -> Result<Terms, String> {
    let old: TermsV3 = wincode::deserialize(bytes).map_err(|_| "terms that do not decode".to_string())?;

    Ok(Terms { hash: old.hash, first: old.first.leg(), second: old.second.leg(), first_deadline: old.first_deadline, second_deadline: old.second_deadline })
}

/// Terms as written before multisigs, after their first byte; None for terms with one.
pub(super) fn write_items(terms: &Terms) -> Option<Vec<u8>> {
    let old = TermsV3 {
        hash: terms.hash,
        first: LegV3::from_leg(&terms.first)?,
        second: LegV3::from_leg(&terms.second)?,
        first_deadline: terms.first_deadline,
        second_deadline: terms.second_deadline,
    };

    Some(wincode::serialize(&old).expect("terms always encode"))
}

#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
struct ChainLegV2 {
    contract: Address,
    token: Option<Address>,
    #[wincode(with = "crate::structs::u256::U256Le")]
    amount: u256,
    payer: Option<Address>,
    receiver: Option<Address>,
}

#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
struct EvmLegV2 {
    chain_id: u64,
    contract: [u8; 20],
    token: Option<[u8; 20]>,
    #[wincode(with = "crate::structs::u256::U256Le")]
    amount: u256,
    payer: Option<[u8; 20]>,
    receiver: Option<[u8; 20]>,
}

impl LegV2 {
    fn from_leg(leg: &Leg) -> Option<LegV2> {
        Some(match leg {
            Leg::Chain(leg) if leg.item.is_none() => LegV2::Chain(ChainLegV2 { contract: leg.contract, token: leg.token, amount: leg.amount, payer: leg.payer, receiver: leg.receiver }),
            Leg::Evm(leg) if leg.item.is_none() => LegV2::Evm(EvmLegV2 { chain_id: leg.chain_id, contract: leg.contract, token: leg.token, amount: leg.amount, payer: leg.payer, receiver: leg.receiver }),
            Leg::Bitcoin(leg) => LegV2::Bitcoin(BitcoinLegV3::from_leg(leg)?),
            _ => return None,
        })
    }

    fn leg(self) -> Leg {
        match self {
            LegV2::Chain(leg) => Leg::Chain(ChainLeg { contract: leg.contract, token: leg.token, item: None, amount: leg.amount, payer: leg.payer, receiver: leg.receiver }),
            LegV2::Bitcoin(leg) => Leg::Bitcoin(leg.leg()),
            LegV2::Evm(leg) => Leg::Evm(EvmLeg { chain_id: leg.chain_id, contract: leg.contract, token: leg.token, item: None, amount: leg.amount, payer: leg.payer, receiver: leg.receiver }),
        }
    }
}

/// Terms written as legs before NFTs, after their first byte.
pub(super) fn read_legs(bytes: &[u8]) -> Result<Terms, String> {
    let old: TermsV2 = wincode::deserialize(bytes).map_err(|_| "terms that do not decode".to_string())?;

    Ok(Terms { hash: old.hash, first: old.first.leg(), second: old.second.leg(), first_deadline: old.first_deadline, second_deadline: old.second_deadline })
}

/// Terms as legs were written before NFTs, after their first byte; None for terms locking an NFT,
/// which those legs cannot hold.
pub(super) fn write_legs(terms: &Terms) -> Option<Vec<u8>> {
    let old = TermsV2 {
        hash: terms.hash,
        first: LegV2::from_leg(&terms.first)?,
        second: LegV2::from_leg(&terms.second)?,
        first_deadline: terms.first_deadline,
        second_deadline: terms.second_deadline,
    };

    Some(wincode::serialize(&old).expect("terms always encode"))
}

/// Old terms' bytes, for the tests to read back.
#[cfg(test)]
pub(super) fn write(terms: &Terms) -> Vec<u8> {
    let (bitcoin, chain, starter) = match (&terms.first, &terms.second) {
        (Leg::Bitcoin(bitcoin), Leg::Chain(chain)) => (bitcoin, chain, Pays::Bitcoin),
        (Leg::Chain(chain), Leg::Bitcoin(bitcoin)) => (bitcoin, chain, Pays::Chain),
        _ => panic!("old terms were chainv4 against Bitcoin"),
    };

    wincode::serialize(&Legacy {
        chain: if bitcoin.testnet { Counterparty::BitcoinTestnet } else { Counterparty::Bitcoin },
        contract: chain.contract,
        hash: terms.hash,
        sats: bitcoin.sats,
        token: chain.token,
        amount: chain.amount,
        starter,
        bitcoin_payer: bitcoin.payer.as_ref().map(|party| party.keys().1[0]),
        bitcoin_receiver: bitcoin.receiver.as_ref().map(|party| party.keys().1[0]),
        chain_payer: chain.payer,
        chain_receiver: chain.receiver,
        first_deadline: terms.first_deadline,
        second_deadline: terms.second_deadline,
    })
    .unwrap()
}
