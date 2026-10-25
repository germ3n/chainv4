use {
    crate::crypto::hashable::Hashable,
    crate::crypto::keypair::Signature,
    crate::structs::address::Address,
    crate::structs::hash::Hash,
    crate::structs::tx::SignedTx,
    crate::structs::u256::{U256Le, u256},
    crate::structs::vote::Justification,
    r#macro::document,
    wincode::SchemaRead,
    wincode::SchemaWrite,
};

use {
    crate::crypto::hasher::Hasher,
    crate::structs::hash::HashType,
};

#[document(
    summary = "What a block commits to. Its hash is the block's id. The seal is not part of it: under an authority the seal signs the id, and under proof of work it is the nonce that makes the id's work hash small enough. Every hash in a header uses the chain's algorithm, from genesis.",
    category = "Chain",
    see_also = "Block",
)]
#[derive(Clone, Debug, PartialEq, SchemaWrite, SchemaRead)]
pub struct BlockHeader {
    #[document(summary = "The id of the block this one extends. Its algorithm is the chain's, which is how a header knows what to hash itself with; genesis's parent is the chain algorithm's zero hash.")]
    pub parent: Hash,
    #[document(summary = "How many blocks come before this one. Genesis is 0.")]
    pub height: u64,
    #[document(summary = "When the block was produced, in milliseconds since the Unix epoch.")]
    pub timestamp: u64,
    #[document(summary = "Who produced the block. Its reward goes here.")]
    pub producer: Address,
    #[document(summary = "How hard the block was to produce: under proof of work, the expected number of hashes to find its seal; 0 under an authority. Consensus decides what it must be.")]
    pub difficulty: u128,
    #[document(summary = "The chain hash of the block's transactions, in order. For genesis, the hash of the genesis configuration instead, so chains with different genesis never share a block.")]
    pub tx_root: Hash,
    #[document(summary = "The chain hash of the finality proofs the block carries, in order.")]
    pub proof_root: Hash,
    #[document(summary = "The Merkle root of what its transactions did, in order: receipts_root over their CommittedReceipts. The zero hash for a block without transactions.")]
    pub receipts_root: Hash,
    #[document(summary = "The root of the state tree once the block has run: every chain-state entry, as StateTree keeps them. What snapshots and light clients check state against.")]
    pub state_root: Hash,
    #[document(summary = "The gas its transactions used, by the chain's gas schedule; 0 on a chain without fees. What moves a base fee, and at most the block gas limit.")]
    pub gas_used: u64,
    #[document(summary = "The base price of gas in this block, which every transaction pays at least: under a base fee, what the parent's gas used made it; when senders bid, their minimum; 0 on a chain without fees.")]
    #[wincode(with = "U256Le")]
    pub base_price: u256,
}

impl Hashable for BlockHeader {}

impl BlockHeader {
    #[document(
        parent = "BlockHeader",
        summary = "The block's id: the header hashed with the chain's algorithm, which its parent's id carries.",
    )]
    pub fn id(&self) -> Hash {
        self.hash(self.parent.algorithm())
    }
}

#[document(
    summary = "A header, the transactions it commits to, and the seal that proves the producer had the right to make it. Genesis has none: it is defined by the configuration every node starts from.",
    category = "Chain",
    see_also = "BlockHeader, SignedTx",
)]
#[derive(Clone, Debug, PartialEq, SchemaWrite, SchemaRead)]
pub struct Block {
    #[document(summary = "The header.")]
    pub header: BlockHeader,
    #[document(summary = "The transactions, run in this order.")]
    pub txs: Vec<SignedTx>,
    #[document(summary = "Finality proofs for checkpoints on this branch, which pay every voter in them. Each checkpoint is paid for once.")]
    pub proofs: Vec<Justification>,
    #[document(summary = "The proof that the producer had the right to make the block.")]
    pub seal: Seal,
}

#[document(
    summary = "What proves a block was made by someone allowed to make it. Which kind a block needs depends on the chain's consensus.",
    category = "Chain",
    see_also = "Block, Consensus",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
pub enum Seal {
    #[document(summary = "No seal: only genesis, and blocks still being sealed.")]
    #[wincode(tag = 0)]
    None,
    #[document(summary = "The authority's signature over the block's id.")]
    #[wincode(tag = 1)]
    Signature(Signature),
    #[document(summary = "A nonce that makes the hash of the block's id and itself fall under the target its difficulty sets.")]
    #[wincode(tag = 2)]
    Work { nonce: u64 },
}

impl Hashable for Seal {
    const TAG_LEN: usize = 1;
}

impl Hashable for Block {}

#[document(
    summary = "A header and its seal: all a light client, or a node syncing headers first, needs to check a block by consensus, without its transactions.",
    category = "Chain",
    see_also = "BlockHeader, Seal, Consensus::check",
)]
#[derive(Clone, Debug, PartialEq, SchemaWrite, SchemaRead)]
pub struct SealedHeader {
    #[document(summary = "The header.")]
    pub header: BlockHeader,
    #[document(summary = "Its seal.")]
    pub seal: Seal,
}

impl Hashable for SealedHeader {}

impl SealedHeader {
    #[document(parent = "SealedHeader", summary = "The block's id.")]
    pub fn id(&self) -> Hash {
        self.header.id()
    }
}

impl Block {
    #[document(parent = "Block", summary = "Its header and seal, without the transactions and proofs.")]
    pub fn sealed_header(&self) -> SealedHeader {
        SealedHeader {
            header: self.header.clone(),
            seal: self.seal,
        }
    }

    #[document(
        parent = "Block",
        summary = "The block's id, the same as its header's.",
    )]
    pub fn id(&self) -> Hash {
        self.header.id()
    }

    #[document(
        parent = "Block",
        summary = "What a header's tx_root must be for these transactions.",
        params = {
            hash = "The chain's hash.",
            txs = "The transactions, in order.",
        },
    )]
    pub fn tx_root(hash: HashType, txs: &[SignedTx]) -> Hash {
        let mut hasher = Hasher::new(hash);
        hasher.update(&(txs.len() as u64).to_le_bytes());

        for tx in txs {
            wincode::serialize_into(&mut hasher, tx).expect("a hasher never fails a write");
        }

        hasher.finalize()
    }

    #[document(
        parent = "Block",
        summary = "What a header's proof_root must be for these proofs.",
        params = {
            hash = "The chain's hash.",
            proofs = "The proofs, in order.",
        },
    )]
    pub fn proof_root(hash: HashType, proofs: &[Justification]) -> Hash {
        let mut hasher = Hasher::new(hash);
        hasher.update(&(proofs.len() as u64).to_le_bytes());

        for proof in proofs {
            wincode::serialize_into(&mut hasher, proof).expect("a hasher never fails a write");
        }

        hasher.finalize()
    }
}
