//! Terms as they were written before legs: chainv4 against Bitcoin, the chain tag first. Codes made
//! then still read, as the same swap in two legs; they are written as legs from then on.

use {
    super::{BitcoinLeg, ChainLeg, Leg, Terms},
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

    let chain = Leg::Chain(ChainLeg { contract: old.contract, token: old.token, amount: old.amount, payer: old.chain_payer, receiver: old.chain_receiver });
    let bitcoin = Leg::Bitcoin(BitcoinLeg { testnet: old.chain == Counterparty::BitcoinTestnet, sats: old.sats, payer: old.bitcoin_payer, receiver: old.bitcoin_receiver });
    let (first, second) = match old.starter {
        Pays::Bitcoin => (bitcoin, chain),
        Pays::Chain => (chain, bitcoin),
    };

    Ok(Terms { hash: old.hash, first, second, first_deadline: old.first_deadline, second_deadline: old.second_deadline })
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
        bitcoin_payer: bitcoin.payer,
        bitcoin_receiver: bitcoin.receiver,
        chain_payer: chain.payer,
        chain_receiver: chain.receiver,
        first_deadline: terms.first_deadline,
        second_deadline: terms.second_deadline,
    })
    .unwrap()
}
