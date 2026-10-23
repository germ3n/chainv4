use {
    crate::crypto::hashable::Hashable,
    crate::crypto::keypair::{PublicKey, Signature},
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
    summary = "What a block commits to. Its hash is the block's id, and the producer's seal signs that hash.",
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
    #[document(summary = "The key that produced and sealed the block.")]
    pub producer: PublicKey,
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
    summary = "A header, the transactions it commits to, and the producer's seal over the header. Genesis has no seal: it is defined by the configuration every node starts from.",
    category = "Chain",
    see_also = "BlockHeader, SignedTx",
)]
#[derive(Clone, Debug, PartialEq, SchemaWrite, SchemaRead)]
pub struct Block {
    #[document(summary = "The header.")]
    pub header: BlockHeader,
    #[document(summary = "The transactions, run in this order.")]
    pub txs: Vec<SignedTx>,
    #[document(summary = "The producer's signature over the header's id. None only for genesis.")]
    pub seal: Option<Signature>,
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
