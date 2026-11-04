//! Transactions in an execution payload: proven under its transactions_root by an SSZ branch, and
//! read from their EIP-2718 encoding for where they went and what calldata they carried.
//!
//! The payload's transactions are an SSZ `List[Transaction, 2**20]`, each a
//! `ByteList[2**30]` of the transaction's own bytes: a typed transaction's type byte and RLP, or a
//! legacy one's RLP. So a transaction's place is the generalized index `2**21 + index`, under a
//! branch of 21 nodes: the 20 siblings in the list's tree, then the list's length.

use {
    crate::{
        Crypto, Error, Root,
        ssz::{self, merkleize, mix_in_length, pack},
    },
    alloc::vec::Vec,
};

/// The most transactions a payload holds.
pub const MAX_TRANSACTIONS_PER_PAYLOAD: u64 = 1 << 20;
/// The most bytes a transaction may be.
pub const MAX_BYTES_PER_TRANSACTION: u64 = 1 << 30;
/// The nodes in a transaction's branch: 20 in the list's tree, and its length.
pub const TRANSACTION_BRANCH_LEN: usize = 21;

/// A transaction's root: its bytes, a ByteList[2**30].
pub fn transaction_root<C: Crypto + ?Sized>(crypto: &C, transaction: &[u8]) -> Root {
    mix_in_length(crypto, &merkleize(crypto, &pack(transaction), MAX_BYTES_PER_TRANSACTION / 32), transaction.len() as u64)
}

/// A payload's transactions_root.
pub fn transactions_root<C: Crypto + ?Sized, T: AsRef<[u8]>>(crypto: &C, transactions: &[T]) -> Root {
    let roots: Vec<Root> = transactions.iter().map(|transaction| transaction_root(crypto, transaction.as_ref())).collect();

    mix_in_length(crypto, &merkleize(crypto, &roots, MAX_TRANSACTIONS_PER_PAYLOAD), transactions.len() as u64)
}

/// The branch of the transaction at an index, for verify_transaction: what a relayer makes from a
/// block's transactions.
pub fn transaction_branch<C: Crypto + ?Sized, T: AsRef<[u8]>>(crypto: &C, transactions: &[T], index: u64) -> Vec<Root> {
    let roots: Vec<Root> = transactions.iter().map(|transaction| transaction_root(crypto, transaction.as_ref())).collect();
    let mut branch = ssz::merkle_branch(crypto, &roots, ssz::floorlog2(MAX_TRANSACTIONS_PER_PAYLOAD), index);
    branch.push(ssz::chunk_u64(transactions.len() as u64));

    branch
}

/// Whether a transaction is the one at an index under a payload's transactions_root, by its
/// branch: 21 nodes, the last the list's length, which the index must be under.
pub fn verify_transaction<C: Crypto + ?Sized>(crypto: &C, transactions_root: &Root, transaction: &[u8], index: u64, branch: &[Root]) -> bool {
    let Some(length) = branch.last() else {
        return false;
    };
    let count = u64::from_le_bytes(length[..8].try_into().expect("8 bytes"));

    branch.len() == TRANSACTION_BRANCH_LEN
        && length[8..].iter().all(|byte| *byte == 0)
        && index < count
        && count <= MAX_TRANSACTIONS_PER_PAYLOAD
        && transaction.len() as u64 <= MAX_BYTES_PER_TRANSACTION
        && ssz::is_valid_merkle_branch(crypto, &transaction_root(crypto, transaction), branch, TRANSACTION_BRANCH_LEN as u32, index, transactions_root)
}

/// What a transaction says of where it went and what it carried.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Call<'a> {
    /// Its EIP-2718 type: 0 for legacy, 1 (EIP-2930), 2 (EIP-1559), 3 (EIP-4844) or 4 (EIP-7702).
    pub tx_type: u8,
    /// The chain it is signed for: a typed one's chain id, a legacy one's from its v by EIP-155, or
    /// None for a legacy one from before it.
    pub chain_id: Option<u64>,
    /// Who it went to, or None for a contract creation.
    pub to: Option<[u8; 20]>,
    /// Its calldata.
    pub data: &'a [u8],
}

/// A transaction as an execution payload holds it, read: a legacy one's RLP list of nine, or a
/// type byte, 1, 2, 3 or 4, before its RLP list. Strict: non-canonical RLP, bytes left over, a
/// field count other than the type's, or a recipient that is not 20 bytes, are refused.
pub fn decode_transaction(bytes: &[u8]) -> Result<Call<'_>, Error> {
    let (tx_type, list) = match bytes.first() {
        Some(&first) if first >= 0xc0 => (0, bytes),
        Some(&first @ 1..=4) => (first, &bytes[1..]),
        Some(_) => return Err(Error::BadTransaction("not a transaction type this reads: legacy, 1, 2, 3 or 4")),
        None => return Err(Error::BadTransaction("an empty transaction")),
    };
    let (item, rest) = rlp::item(list)?;

    if !rest.is_empty() {
        return Err(Error::BadTransaction("bytes after the transaction"));
    }

    let rlp::Item::List(payload) = item else {
        return Err(Error::BadTransaction("a transaction is an RLP list"));
    };
    let fields = rlp::list(payload)?;
    // How many fields each type has, and where its recipient is; the calldata follows two on.
    let (count, to_at) = match tx_type {
        0 => (9, 3),
        1 => (11, 4),
        2 => (12, 5),
        3 => (14, 5),
        _ => (13, 5),
    };

    if fields.len() != count {
        return Err(Error::BadTransaction("a transaction with the wrong number of fields"));
    }

    let to = match fields[to_at].string()? {
        [] if tx_type <= 2 => None,
        to => Some(<[u8; 20]>::try_from(to).map_err(|_| Error::BadTransaction("a recipient is 20 bytes"))?),
    };
    let data = fields[to_at + 2].string()?;
    let chain_id = match tx_type {
        0 => {
            let v = rlp::uint(fields[6].string()?)?;
            (v >= 35).then(|| (v - 35) / 2)
        }
        _ => Some(rlp::uint(fields[0].string()?)?),
    };

    Ok(Call { tx_type, chain_id, to, data })
}

/// A minimal RLP reader, strict about canonical encodings.
pub mod rlp {
    use {crate::Error, alloc::vec::Vec};

    /// One RLP item: a string's bytes, or a list's payload.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Item<'a> {
        String(&'a [u8]),
        List(&'a [u8]),
    }

    impl<'a> Item<'a> {
        /// A string's bytes, refusing a list.
        pub fn string(self) -> Result<&'a [u8], Error> {
            match self {
                Item::String(bytes) => Ok(bytes),
                Item::List(_) => Err(Error::BadTransaction("a list where RLP should have a string")),
            }
        }
    }

    fn bad(why: &'static str) -> Error {
        Error::BadTransaction(why)
    }

    /// A long item's length, big-endian, in so many bytes: no leading zero, and at least 56.
    fn long_length(bytes: &[u8], size: usize) -> Result<usize, Error> {
        let length = bytes.get(1..1 + size).ok_or(bad("RLP cut short"))?;

        if length[0] == 0 || size > 8 {
            return Err(bad("RLP length not canonical"));
        }

        let length = length.iter().fold(0u64, |total, byte| total << 8 | u64::from(*byte));

        match usize::try_from(length) {
            Ok(length) if length >= 56 => Ok(length),
            _ => Err(bad("RLP length not canonical")),
        }
    }

    /// The first item of some bytes, and what follows it.
    pub fn item(bytes: &[u8]) -> Result<(Item<'_>, &[u8]), Error> {
        let first = *bytes.first().ok_or(bad("RLP cut short"))?;
        let (header, length, list) = match first {
            0x00..=0x7f => return Ok((Item::String(&bytes[..1]), &bytes[1..])),
            0x80..=0xb7 => (1, usize::from(first - 0x80), false),
            0xb8..=0xbf => {
                let size = usize::from(first - 0xb7);
                (1 + size, long_length(bytes, size)?, false)
            }
            0xc0..=0xf7 => (1, usize::from(first - 0xc0), true),
            0xf8..=0xff => {
                let size = usize::from(first - 0xf7);
                (1 + size, long_length(bytes, size)?, true)
            }
        };
        let payload = header.checked_add(length).and_then(|end| bytes.get(header..end)).ok_or(bad("RLP cut short"))?;

        if !list && length == 1 && payload[0] < 0x80 {
            return Err(bad("a single byte under 0x80 is its own RLP"));
        }

        let rest = &bytes[header + length..];

        Ok((if list { Item::List(payload) } else { Item::String(payload) }, rest))
    }

    /// A list's payload as its items.
    pub fn list(mut payload: &[u8]) -> Result<Vec<Item<'_>>, Error> {
        let mut items = Vec::new();

        while !payload.is_empty() {
            let (item, rest) = item(payload)?;
            items.push(item);
            payload = rest;
        }

        Ok(items)
    }

    /// A whole number of at most 8 bytes, big-endian, with no leading zero.
    pub fn uint(bytes: &[u8]) -> Result<u64, Error> {
        if bytes.len() > 8 || bytes.first() == Some(&0) {
            return Err(bad("an RLP number not canonical, or past 64 bits"));
        }

        Ok(bytes.iter().fold(0u64, |total, byte| total << 8 | u64::from(*byte)))
    }
}
