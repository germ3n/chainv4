//! A range of blocks as a node sends them to a client that means to rest on what it checks itself,
//! and the checking. Each item holds what recomputes every root in its header, its receipts, and a
//! proof against its state_root of each state entry the block changed, so a client scanning a range
//! picks out its own accounts' entries without telling the node which it wanted.
//!
//! Everything here is outside the node, so the apps' light build and wasm reach it: the node's side
//! is chain::stored_block_range. What the state root cannot do is say which entries a block
//! changed: it commits to the state, not to the difference. So the list an item's entries come from
//! is the node's word, and an entry left out goes unnoticed. Each entry given is proven; that all
//! were given is not.

use {
    crate::db::{Column, STATE_COLUMNS, StateProof},
    crate::structs::block::{Block, BodyRoot, MAX_LIST_MEMORY, PoolCommitment, SealedHeader},
    crate::structs::hash::{Hash, HashType},
    crate::structs::receipt::{CommittedReceipt, receipts_root},
    crate::structs::tx::SignedTx,
    crate::structs::vote::Justification,
    r#macro::document,
    std::collections::BTreeSet,
    std::fmt,
    wincode::{SchemaRead, SchemaWrite},
};

#[document(
    summary = "The most bytes decode_block_range reads, 128 MiB: twice what a sync response may hold. A node stops a response long before this, but it must always give a block's body and receipts whole, whatever they come to; past this a client refuses them rather than hold them.",
    category = "Light client",
    see_also = "decode_block_range, BlockRange",
)]
pub const MAX_RANGE_DECODE: usize = 2 * MAX_LIST_MEMORY;

#[document(
    summary = "One answer of GET /blocks/range: consecutive main-chain blocks, each with what checks it, and where to go on from. Its encoding is the route's body, read with decode_block_range.",
    category = "Light client",
    see_also = "RangeItem, check_range_item, decode_block_range",
)]
#[derive(Clone, Debug, PartialEq, SchemaWrite, SchemaRead)]
pub struct BlockRange {
    #[document(summary = "The height of the node's tip as it answered.")]
    pub tip: u64,
    #[document(summary = "The blocks, by height, starting where asked. None when asked from past the tip.")]
    pub items: Vec<RangeItem>,
    #[document(summary = "Where to go on from, to ask next; None once it has given everything up to the tip.")]
    pub next: Option<RangeCursor>,
}

#[document(
    summary = "Where a range goes on from: from= and entry= of the next GET /blocks/range. A response stopped inside a block's entries goes on from the first it did not give.",
    category = "Light client",
    see_also = "BlockRange",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct RangeCursor {
    #[document(summary = "The height.")]
    pub height: u64,
    #[document(summary = "How many of that block's entries came already; 0 for a block not begun.")]
    pub entry: u32,
}

#[document(
    summary = "One block of a range, as a node holds it.",
    category = "Light client",
    see_also = "BlockRange, check_range_item, CheckedItem",
)]
#[derive(Clone, Debug, PartialEq, SchemaWrite, SchemaRead)]
pub struct RangeItem {
    #[document(summary = "Its header and seal. A client compares them with the header it checked, or feeds them to a light client to check.")]
    pub sealed: SealedHeader,
    #[document(summary = "What it holds of the block's body.")]
    pub body: RangeBody,
    #[document(summary = "The state entries the block changed, proven; or why there are none.")]
    pub changes: RangeChanges,
}

#[document(
    summary = "What a range item holds of its block's body.",
    category = "Light client",
    see_also = "RangeItem",
)]
#[derive(Clone, Debug, PartialEq, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
pub enum RangeBody {
    #[document(summary = "All of it, which recomputes tx_root, proof_root and pool_root, and with its receipts, receipts_root. receipts is None where the node holds them not: for the block a snapshot node took state at, whose transactions it never ran. A block without transactions has an empty list, under genesis too.")]
    #[wincode(tag = 0)]
    Whole {
        // A full block's run past wincode's default 4 MiB in memory: see Block::txs.
        #[wincode(with = "wincode::containers::Vec<SignedTx, wincode::len::BincodeLen<{ crate::structs::block::MAX_LIST_MEMORY }>>")]
        txs: Vec<SignedTx>,
        proofs: Vec<Justification>,
        pool: Option<PoolCommitment>,
        #[wincode(with = "Option<wincode::containers::Vec<CommittedReceipt, wincode::len::BincodeLen<{ crate::structs::block::MAX_LIST_MEMORY }>>>")]
        receipts: Option<Vec<CommittedReceipt>>,
    },
    #[document(summary = "Nothing: a node that synced from a snapshot keeps only the headers of blocks below where it took state.")]
    #[wincode(tag = 1)]
    HeaderOnly,
    #[document(summary = "Nothing, because the answer before gave it: this item goes on through a block's entries from where that one stopped.")]
    #[wincode(tag = 2)]
    Continued,
}

#[document(
    summary = "The state entries a range item proves, or why it proves none. The block is given either way.",
    category = "Light client",
    see_also = "RangeItem, NoProofs",
)]
#[derive(Clone, Debug, PartialEq, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
pub enum RangeChanges {
    #[document(summary = "Entries, starting at the from-th of the listed many the node's list holds for the block. Both counts, like the list, are the node's word. An empty block's list may still hold its producer's account and more.")]
    #[wincode(tag = 0)]
    Proven {
        from: u32,
        listed: u32,
        // As for txs: a node's own limits may say how many, so wincode's default should not.
        #[wincode(with = "wincode::containers::Vec<ProvenEntry, wincode::len::BincodeLen<{ crate::structs::block::MAX_LIST_MEMORY }>>")]
        entries: Vec<ProvenEntry>,
    },
    #[document(summary = "None, and why.")]
    #[wincode(tag = 1)]
    None(NoProofs),
}

#[document(
    summary = "Why a range item proves no entries. It does not mean the block changed nothing.",
    category = "Light client",
    see_also = "RangeChanges",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
pub enum NoProofs {
    #[document(summary = "The node holds no list of what the block changed: genesis, which ran nothing; the block a snapshot node took state at; the lowest block its history still reaches, whose list went with the block below; or a list lost.")]
    #[wincode(tag = 0)]
    NoList,
    #[document(summary = "The node no longer keeps the state at this block, nor any below kept_from: past its --state-history window, or below where it synced from a snapshot or where its history began.")]
    #[wincode(tag = 1)]
    StatePruned { kept_from: u64 },
}

#[document(
    summary = "One state entry a block changed, as it stood once the block had run, with its proof against that block's state_root.",
    category = "Light client",
    see_also = "RangeChanges, StateProof::verify",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct ProvenEntry {
    #[document(summary = "One of STATE_COLUMNS.")]
    pub column: Column,
    #[document(summary = "Its key in that column.")]
    pub key: Vec<u8>,
    #[document(summary = "What it holds; None for an entry the block deleted, or one it wrote and took out again, which the proof shows absent. A contract's record holds its code, so this may reach the largest deploy and past wincode's default 4 MiB.")]
    #[wincode(with = "Option<wincode::containers::Vec<u8, wincode::len::BincodeLen<{ crate::structs::block::MAX_LIST_MEMORY }>>>")]
    pub value: Option<Vec<u8>>,
    #[document(summary = "The proof of that value, or of its absence.")]
    pub proof: StateProof,
}

#[document(
    summary = "What a range item came to once every root and proof in it checked: only these parts should be read. What it does not hold, it says: a body or receipts missing, or no entries and why. Entries are all proven, but not proven to be all there are.",
    category = "Light client",
    see_also = "check_range_item, RangeItem",
)]
#[derive(Clone, Debug, PartialEq)]
pub struct CheckedItem {
    #[document(summary = "The block's id.")]
    pub block: Hash,
    #[document(summary = "Its header and seal.")]
    pub sealed: SealedHeader,
    #[document(summary = "What it held of the body, every root of it checked.")]
    pub body: RangeBody,
    #[document(summary = "The entries it proved, each checked against state_root, or why none.")]
    pub changes: RangeChanges,
}

impl CheckedItem {
    #[document(parent = "CheckedItem", summary = "The block's height.")]
    pub fn height(&self) -> u64 {
        self.sealed.header.height
    }

    #[document(parent = "CheckedItem", summary = "The entries it proved: none when it says why not.")]
    pub fn entries(&self) -> &[ProvenEntry] {
        match &self.changes {
            RangeChanges::Proven { entries, .. } => entries,
            RangeChanges::None(_) => &[],
        }
    }
}

#[document(
    summary = "What was wrong with a range a node sent. It names the place and never quotes the bytes, so the message is safe to show and short.",
    category = "Light client",
    see_also = "check_range_item, decode_block_range, BlockRange::check_shape",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RangeError {
    #[document(summary = "More bytes than MAX_RANGE_DECODE.")]
    TooBig,
    #[document(summary = "The bytes do not decode.")]
    Undecodable,
    #[document(summary = "Its header and seal are not those of the header checked.")]
    WrongHeader,
    #[document(summary = "A root its body does not fit.")]
    Root(BodyRoot),
    #[document(summary = "Under genesis, a body that holds anything: its roots commit to its configuration, so nothing else there can be checked.")]
    GenesisBody,
    #[document(summary = "Its receipts do not fit receipts_root.")]
    ReceiptsRoot,
    #[document(summary = "A receipt, the index-th, is not for the transaction at its place, or their counts differ.")]
    ReceiptOutOfPlace { index: u32 },
    #[document(summary = "An entry, the index-th given, whose proof does not check against state_root.")]
    BadEntry { index: u32, column: Column },
    #[document(summary = "An entry, the index-th given, in a column the state root does not cover.")]
    NotStateColumn { index: u32 },
    #[document(summary = "An entry, the index-th given, that came before in the same item.")]
    RepeatedEntry { index: u32 },
    #[document(summary = "Counts that do not add up: more entries than it says the list holds.")]
    BadCounts,
    #[document(summary = "Blocks or a cursor that are not where the request and each other put them.")]
    OutOfPlace,
}

impl fmt::Display for RangeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RangeError::TooBig => write!(f, "a range too big to read"),
            RangeError::Undecodable => write!(f, "a range that does not decode"),
            RangeError::WrongHeader => write!(f, "a block whose header is not the one checked"),
            RangeError::Root(BodyRoot::Tx) => write!(f, "transactions that do not fit the header's tx_root"),
            RangeError::Root(BodyRoot::Proof) => write!(f, "finality proofs that do not fit the header's proof_root"),
            RangeError::Root(BodyRoot::Pool) => write!(f, "a pool commitment that does not fit the header's pool_root"),
            RangeError::GenesisBody => write!(f, "a genesis block that holds something"),
            RangeError::ReceiptsRoot => write!(f, "receipts that do not fit the header's receipts_root"),
            RangeError::ReceiptOutOfPlace { index } => write!(f, "receipt {index} is not for the transaction at its place"),
            RangeError::BadEntry { index, column } => write!(f, "entry {index}, in {}, whose proof does not check against the header's state_root", column.name()),
            RangeError::NotStateColumn { index } => write!(f, "entry {index} is in a column the state root does not cover"),
            RangeError::RepeatedEntry { index } => write!(f, "entry {index} came before"),
            RangeError::BadCounts => write!(f, "more entries than the block lists"),
            RangeError::OutOfPlace => write!(f, "blocks or a cursor out of place"),
        }
    }
}

impl std::error::Error for RangeError {}

#[document(
    summary = "Reads what GET /blocks/range answered: safe on any bytes, as lists allocate within wincode's limits, nothing nests without bound, and too many bytes are refused unread. Nothing it returns is checked yet.",
    params = { bytes = "The response's body." },
    returns = "The range, or TooBig or Undecodable.",
    category = "Light client",
    see_also = "BlockRange, BlockRange::check_shape, check_range_item",
)]
pub fn decode_block_range(bytes: &[u8]) -> Result<BlockRange, RangeError> {
    if bytes.len() > MAX_RANGE_DECODE {
        return Err(RangeError::TooBig);
    }

    wincode::deserialize(bytes).map_err(|_| RangeError::Undecodable)
}

impl BlockRange {
    #[document(
        parent = "BlockRange",
        summary = "Whether the answer is in the shape asked for: consecutive heights from where asked, a body only left out where going on through a block, every block but the last with all its entries, and a cursor that moves on from where the last stopped, or none only at the tip. So a node cannot skip a block or a stretch of entries in plain sight, or send a client round in a loop. What is in each item is check_range_item's to check.",
        params = {
            from = "The from= asked.",
            entry = "The entry= asked.",
        },
    )]
    pub fn check_shape(&self, from: u64, entry: u32) -> Result<(), RangeError> {
        let Some(last) = self.items.last() else {
            return match from > self.tip && self.next.is_none() {
                true => Ok(()),
                false => Err(RangeError::OutOfPlace),
            };
        };

        for (at, item) in self.items.iter().enumerate() {
            let at = at as u64;

            if from.checked_add(at) != Some(item.sealed.header.height) {
                return Err(RangeError::OutOfPlace);
            }

            let continued = at == 0 && entry > 0;

            if continued != matches!(item.body, RangeBody::Continued) {
                return Err(RangeError::OutOfPlace);
            }

            if let RangeChanges::Proven { from, .. } = item.changes
                && from != if continued { entry } else { 0 }
            {
                return Err(RangeError::OutOfPlace);
            }

            if at + 1 < self.items.len() as u64 && item.unfinished().is_some() {
                return Err(RangeError::OutOfPlace);
            }
        }

        let height = last.sealed.header.height;
        let expected = match last.unfinished() {
            Some(entry) => Some(RangeCursor { height, entry }),
            None => height.checked_add(1).filter(|next| *next <= self.tip).map(|height| RangeCursor { height, entry: 0 }),
        };

        match (height <= self.tip, expected) {
            (true, Some(expected)) if self.next == Some(expected) => Ok(()),
            // The last block's done and it is the tip, so nothing follows.
            (true, None) if self.next.is_none() => Ok(()),
            _ => Err(RangeError::OutOfPlace),
        }
    }
}

impl RangeItem {
    /// Where its entries stop short of the list it says there is, if they do.
    fn unfinished(&self) -> Option<u32> {
        match &self.changes {
            RangeChanges::Proven { from, listed, entries } => {
                let reached = u64::from(*from) + entries.len() as u64;

                (reached < u64::from(*listed)).then_some(reached as u32)
            }
            RangeChanges::None(_) => None,
        }
    }
}

#[document(
    summary = "Checks one range item against a header already checked: its header is that one, its body recomputes tx_root, proof_root and pool_root, its receipts recompute receipts_root each for the transaction at its place, and each entry's proof checks against state_root. It takes the item and gives back only what checked, so nothing unchecked is left to read. It cannot tell a list that left an entry out, nor whether a node that says it holds no proofs tells the truth.",
    params = {
        hash = "The chain's hash.",
        header = "The block's header and seal, as a light client checked them.",
        item = "What the node sent.",
    },
    returns = "What checked, or the first thing that did not.",
    category = "Light client",
    see_also = "LightClient::check_range_item, CheckedItem, RangeError",
)]
pub fn check_range_item(hash: HashType, header: &SealedHeader, item: RangeItem) -> Result<CheckedItem, RangeError> {
    if item.sealed != *header {
        return Err(RangeError::WrongHeader);
    }

    if let RangeBody::Whole { txs, proofs, pool, receipts } = &item.body {
        check_body(hash, header, txs, proofs, pool.as_ref(), receipts.as_deref())?;
    }

    if let RangeChanges::Proven { from, listed, entries } = &item.changes {
        if u64::from(*from) + entries.len() as u64 > u64::from(*listed) {
            return Err(RangeError::BadCounts);
        }

        let mut seen = BTreeSet::new();

        for (index, entry) in entries.iter().enumerate() {
            let index = index as u32;

            if !STATE_COLUMNS.contains(&entry.column) {
                return Err(RangeError::NotStateColumn { index });
            }

            if !seen.insert((entry.column, entry.key.as_slice())) {
                return Err(RangeError::RepeatedEntry { index });
            }

            if !entry.proof.verify(&header.header.state_root, entry.column, &entry.key, entry.value.as_deref()) {
                return Err(RangeError::BadEntry { index, column: entry.column });
            }
        }
    }

    Ok(CheckedItem {
        block: item.sealed.id(),
        sealed: item.sealed,
        body: item.body,
        changes: item.changes,
    })
}

/// A whole body against the header's roots. Genesis's tx_root and proof_root commit to its
/// configuration, which the header was already checked by, so it can only hold nothing.
fn check_body(hash: HashType, sealed: &SealedHeader, txs: &[SignedTx], proofs: &[Justification], pool: Option<&PoolCommitment>, receipts: Option<&[CommittedReceipt]>) -> Result<(), RangeError> {
    let header = &sealed.header;

    if header.height == 0 {
        if !txs.is_empty() || !proofs.is_empty() || pool.is_some() {
            return Err(RangeError::GenesisBody);
        }
    } else if let Some(root) = Block::body_mismatch(hash, header, txs, proofs, pool) {
        return Err(RangeError::Root(root));
    }

    let Some(receipts) = receipts else {
        return Ok(());
    };

    if receipts_root(hash, receipts) != header.receipts_root {
        return Err(RangeError::ReceiptsRoot);
    }

    // The root says these are the block's receipts in its order, and the block's transactions are
    // proven apart: each must be for the transaction beside it.
    for (index, pair) in txs.iter().zip(receipts.iter()).enumerate() {
        if pair.0.id(hash) != pair.1.tx {
            return Err(RangeError::ReceiptOutOfPlace { index: index as u32 });
        }
    }

    if txs.len() != receipts.len() {
        return Err(RangeError::ReceiptOutOfPlace { index: txs.len().min(receipts.len()) as u32 });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::structs::block::{BlockHeader, Seal};
    use crate::structs::address::Address;
    use crate::structs::u256::u256;

    fn hash() -> HashType {
        HashType::default_chain()
    }

    fn sealed(height: u64, state_root: Hash) -> SealedHeader {
        let zero = Hash::zero(hash());

        SealedHeader {
            header: BlockHeader {
                parent: zero,
                height,
                timestamp: 0,
                producer: Address::Ed25519([0; 32]),
                difficulty: 0,
                tx_root: Block::tx_root(hash(), &[]),
                proof_root: Block::proof_root(hash(), &[]),
                pool_root: zero,
                receipts_root: receipts_root(hash(), &[]),
                state_root,
                gas_used: 0,
                base_price: u256::ZERO,
            },
            seal: Seal::None,
        }
    }

    fn empty_body() -> RangeBody {
        RangeBody::Whole { txs: Vec::new(), proofs: Vec::new(), pool: None, receipts: Some(Vec::new()) }
    }

    fn item(height: u64, body: RangeBody, changes: RangeChanges) -> RangeItem {
        RangeItem { sealed: sealed(height, Hash::zero(hash())), body, changes }
    }

    fn proven(from: u32, listed: u32, entries: Vec<ProvenEntry>) -> RangeChanges {
        RangeChanges::Proven { from, listed, entries }
    }

    /// An entry absent from the empty tree, whose root is zero: all these tests need of a proof.
    fn absent(key: &[u8]) -> ProvenEntry {
        ProvenEntry { column: Column::Accounts, key: key.to_vec(), value: None, proof: StateProof { siblings: Vec::new(), leaf: None } }
    }

    #[test]
    fn a_range_decodes_as_it_encoded_and_the_empty_block_checks() {
        let range = BlockRange { tip: 1, items: vec![item(1, empty_body(), proven(0, 1, vec![absent(b"a")]))], next: None };
        let decoded = decode_block_range(&wincode::serialize(&range).unwrap()).unwrap();

        assert_eq!(decoded, range);
        decoded.check_shape(1, 0).unwrap();

        let header = sealed(1, Hash::zero(hash()));
        let checked = check_range_item(hash(), &header, decoded.items[0].clone()).unwrap();

        assert_eq!((checked.height(), checked.block, checked.entries().len()), (1, header.id(), 1));
    }

    #[test]
    fn counts_repeats_and_columns_outside_the_state_are_refused() {
        let header = sealed(1, Hash::zero(hash()));
        let check = |changes| check_range_item(hash(), &header, item(1, empty_body(), changes));

        assert_eq!(check(proven(0, 0, vec![absent(b"a")])), Err(RangeError::BadCounts));
        assert_eq!(check(proven(u32::MAX, u32::MAX, vec![absent(b"a")])), Err(RangeError::BadCounts), "no overflow lets it by");
        assert_eq!(check(proven(0, 2, vec![absent(b"a"), absent(b"a")])), Err(RangeError::RepeatedEntry { index: 1 }));
        let blocks = ProvenEntry { column: Column::Blocks, ..absent(b"a") };
        assert_eq!(check(proven(0, 1, vec![blocks])), Err(RangeError::NotStateColumn { index: 0 }));
    }

    #[test]
    fn genesis_holds_nothing_and_its_roots_are_not_recomputed() {
        let mut header = sealed(0, Hash::zero(hash()));
        // As its configuration's hash would be: no body recomputes it.
        header.header.tx_root = crate::crypto::hasher::Hasher::digest(hash(), b"genesis");
        let check = |body| check_range_item(hash(), &header, RangeItem { sealed: header.clone(), body, changes: RangeChanges::None(NoProofs::NoList) });

        assert!(check(empty_body()).is_ok());
        let pool = Some(PoolCommitment { parts: 1, root: Hash::zero(hash()), count: 0 });
        assert_eq!(check(RangeBody::Whole { txs: Vec::new(), proofs: Vec::new(), pool, receipts: None }), Err(RangeError::GenesisBody));
    }

    #[test]
    fn a_shape_out_of_place_is_refused() {
        let whole = |height| item(height, empty_body(), RangeChanges::None(NoProofs::NoList));
        let shape = |items, next, tip, from, entry| BlockRange { tip, items, next }.check_shape(from, entry);
        let at = |height, entry| Some(RangeCursor { height, entry });

        assert_eq!(shape(vec![whole(3), whole(4)], at(5, 0), 9, 3, 0), Ok(()));
        assert_eq!(shape(vec![whole(3), whole(4)], None, 4, 3, 0), Ok(()), "at the tip nothing follows");
        assert_eq!(shape(Vec::new(), None, 4, 5, 0), Ok(()), "past the tip");

        assert_eq!(shape(vec![whole(3), whole(5)], at(6, 0), 9, 3, 0), Err(RangeError::OutOfPlace), "a block skipped");
        assert_eq!(shape(vec![whole(4)], at(5, 0), 9, 3, 0), Err(RangeError::OutOfPlace), "not where asked");
        assert_eq!(shape(vec![whole(3)], None, 9, 3, 0), Err(RangeError::OutOfPlace), "stops short of the tip");
        assert_eq!(shape(vec![whole(3)], at(3, 0), 9, 3, 0), Err(RangeError::OutOfPlace), "a cursor that goes nowhere");
        assert_eq!(shape(Vec::new(), at(3, 0), 9, 3, 0), Err(RangeError::OutOfPlace), "nothing, and more to come");
        assert_eq!(shape(Vec::new(), None, 9, 3, 0), Err(RangeError::OutOfPlace), "nothing, below the tip");
        assert_eq!(shape(vec![whole(u64::MAX)], None, u64::MAX, u64::MAX, 0), Ok(()), "no overflow at the very top");

        // Going on through a block's entries.
        let part = |height, body, from, total, count| item(height, body, proven(from, total, (0..count).map(|n: u8| absent(&[n])).collect()));

        assert_eq!(shape(vec![part(3, empty_body(), 0, 4, 2)], at(3, 2), 9, 3, 0), Ok(()));
        assert_eq!(shape(vec![part(3, RangeBody::Continued, 2, 4, 2)], at(4, 0), 9, 3, 2), Ok(()));
        assert_eq!(shape(vec![part(3, empty_body(), 0, 4, 2)], at(4, 0), 9, 3, 0), Err(RangeError::OutOfPlace), "entries skipped");
        assert_eq!(shape(vec![part(3, empty_body(), 0, 4, 2), whole(4)], at(5, 0), 9, 3, 0), Err(RangeError::OutOfPlace), "a block unfinished before another");
        assert_eq!(shape(vec![part(3, RangeBody::Continued, 3, 4, 1)], at(4, 0), 9, 3, 2), Err(RangeError::OutOfPlace), "not from where asked");
        assert_eq!(shape(vec![part(3, empty_body(), 2, 4, 2)], at(4, 0), 9, 3, 2), Err(RangeError::OutOfPlace), "a body where going on");
        assert_eq!(shape(vec![part(3, RangeBody::Continued, 0, 4, 2)], at(3, 2), 9, 3, 0), Err(RangeError::OutOfPlace), "no body where it should be");
    }

    #[test]
    fn hostile_bytes_are_refused_and_allocate_nothing_much() {
        assert_eq!(decode_block_range(&[]), Err(RangeError::Undecodable));
        assert_eq!(decode_block_range(&[0xff; 64]), Err(RangeError::Undecodable));
        assert_eq!(decode_block_range(&vec![0; MAX_RANGE_DECODE + 1]), Err(RangeError::TooBig));

        // Lists that claim more than there is, at each place a length is read.
        let mut huge_items = 1u64.to_le_bytes().to_vec();
        huge_items.extend_from_slice(&u64::MAX.to_le_bytes());
        assert_eq!(decode_block_range(&huge_items), Err(RangeError::Undecodable));

        let range = BlockRange { tip: 1, items: vec![item(1, empty_body(), proven(0, 1, vec![absent(b"a")]))], next: None };
        let bytes = wincode::serialize(&range).unwrap();

        for end in 0..bytes.len() {
            assert_eq!(decode_block_range(&bytes[..end]), Err(RangeError::Undecodable), "cut at {end}");
        }

        for at in 0..bytes.len() {
            for value in [0x00, 0x01, 0x7f, 0xff] {
                let mut changed = bytes.clone();
                changed[at] = value;
                // Anything at all may decode; it must only never panic, and check or be refused.
                if let Ok(decoded) = decode_block_range(&changed) {
                    let _ = decoded.check_shape(1, 0);

                    for item in decoded.items {
                        let _ = check_range_item(hash(), &sealed(1, Hash::zero(hash())), item);
                    }
                }
            }
        }
    }

    #[test]
    fn what_goes_wrong_is_said_without_the_bytes() {
        let error = RangeError::BadEntry { index: 3, column: Column::Storage };

        assert_eq!(error.to_string(), "entry 3, in storage, whose proof does not check against the header's state_root");
    }
}
