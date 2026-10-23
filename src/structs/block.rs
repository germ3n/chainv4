use {
    crate::crypto::hashable::Hashable,
    crate::crypto::keypair::Signature,
    crate::structs::address::Address,
    crate::structs::hash::Hash,
    crate::structs::tx::SignedTx,
    r#macro::document,
    wincode::SchemaRead,
    wincode::SchemaWrite,
};

#[cfg(feature = "blake3")]
use {
    crate::crypto::hasher::Hasher,
    crate::structs::hash::HashType,
};

#[document(
    summary = "What a block commits to. Its hash is the block's id. The seal is not part of it: under an authority the seal signs the id, and under proof of work it is the nonce that makes the id's work hash small enough.",
    category = "Chain",
    see_also = "Block",
)]
#[derive(Clone, Debug, PartialEq, SchemaWrite, SchemaRead)]
pub struct BlockHeader {
    #[document(summary = "The id of the block this one extends.")]
    pub parent: Hash,
    #[document(summary = "How many blocks come before this one. Genesis is 0.")]
    pub height: u64,
    #[document(summary = "When the block was produced, in milliseconds since the Unix epoch.")]
    pub timestamp: u64,
    #[document(summary = "Who produced the block. Its reward goes here.")]
    pub producer: Address,
    #[document(summary = "How hard the block was to produce: under proof of work, the expected number of hashes to find its seal; 0 under an authority. Consensus decides what it must be.")]
    pub difficulty: u128,
    #[document(summary = "BLAKE3 of the block's transactions, in order. For genesis, BLAKE3 of the genesis configuration instead, so chains with different genesis never share a block.")]
    pub tx_root: Hash,
}

impl Hashable for BlockHeader {}

#[cfg(feature = "blake3")]
impl BlockHeader {
    #[document(
        parent = "BlockHeader",
        summary = "The block's id: BLAKE3 of the header.",
    )]
    pub fn id(&self) -> Hash {
        self.hash(HashType::Blake3_256)
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

#[cfg(feature = "blake3")]
impl Block {
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
    )]
    pub fn tx_root(txs: &[SignedTx]) -> Hash {
        let mut hasher = Hasher::new(HashType::Blake3_256);
        hasher.update(&(txs.len() as u64).to_le_bytes());

        for tx in txs {
            wincode::serialize_into(&mut hasher, tx).expect("a hasher never fails a write");
        }

        hasher.finalize()
    }
}
