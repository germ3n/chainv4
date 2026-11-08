//! What the chain keeps of validity proofs, from the proofs fork on: the proven tip, where the next
//! proof starts, and each recent block's id, which a proof names its end by. Kept in chain state,
//! so a proof of a range that holds a Tx::Prove runs it as every node did.

use {
    super::RangeEnd,
    crate::db::{Column, DbError, State},
    crate::structs::block::BlockHeader,
    crate::structs::hash::Hash,
    r#macro::document,
};

const TIP: &[u8] = b"proofs:tip";
const HEADERS: &[u8] = b"proofs:headers";
const BLOCK: &[u8] = b"proofs:block:";

#[document(
    summary = "How many recent blocks' ids the chain keeps for proofs to end at: 8,192, about a day of blocks at ten seconds each. A proof ends at one of them.",
    category = "Proofs",
    see_also = "ProvenChain",
)]
pub const RECORDED_BLOCKS: u64 = 8_192;

#[document(
    summary = "The chain's proofs in a state: the proven tip, and the ids of the blocks a proof may end at.",
    category = "Proofs",
    see_also = "Tx::Prove, RECORDED_BLOCKS",
)]
pub struct ProvenChain<'a> {
    state: &'a State,
}

impl<'a> ProvenChain<'a> {
    #[document(parent = "ProvenChain", summary = "The proofs kept in a state.")]
    pub fn new(state: &'a State) -> ProvenChain<'a> {
        ProvenChain { state }
    }

    #[document(parent = "ProvenChain", summary = "Where the next proof starts: the end of the last one taken, or the block before the fork; None before the fork.")]
    pub fn tip(&self) -> Result<Option<RangeEnd>, DbError> {
        self.state.get_value(Column::Validators, TIP)
    }

    #[document(parent = "ProvenChain", summary = "The id of the block at a height, while the chain keeps it: from the block before the fork, the latest RECORDED_BLOCKS of them.")]
    pub fn block_at(&self, height: u64) -> Result<Option<Hash>, DbError> {
        self.state.get_value(Column::Validators, &block_key(height))
    }

    #[document(parent = "ProvenChain", summary = "The latest header-chain proof taken (Tx::ProveHeaders), or None before any.")]
    #[cfg(feature = "secp256k1")]
    pub fn headers(&self) -> Result<Option<super::TakenHeaders>, DbError> {
        self.state.get_value(Column::Validators, HEADERS)
    }

    #[document(parent = "ProvenChain", summary = "Keeps a header-chain proof as the latest taken.")]
    #[cfg(feature = "secp256k1")]
    pub fn set_headers(&self, headers: &super::TakenHeaders) {
        self.state.put_value(Column::Validators, HEADERS.to_vec(), headers);
    }

    #[document(parent = "ProvenChain", summary = "Where the latest header-chain proof is kept in the Validators column, for an app to prove.")]
    pub fn headers_key() -> &'static [u8] {
        HEADERS
    }

    #[document(parent = "ProvenChain", summary = "Moves the proven tip to a proof's end.")]
    pub fn set_tip(&self, tip: &RangeEnd) {
        self.state.put_value(Column::Validators, TIP.to_vec(), tip);
    }

    #[document(
        parent = "ProvenChain",
        summary = "What every block from the fork does as it runs: records its parent's id, lets go of the one RECORDED_BLOCKS older, and, in the fork's first block, starts the proven tip at its parent, the state the first proof runs from.",
        params = { parent = "The block's parent's header." },
    )]
    pub fn follow(&self, parent: &BlockHeader) -> Result<(), DbError> {
        let id = parent.id();

        if self.tip()?.is_none() {
            self.set_tip(&RangeEnd { height: parent.height, block: id, state_root: parent.state_root });
        }

        self.state.put_value(Column::Validators, block_key(parent.height), &id);

        if let Some(gone) = parent.height.checked_sub(RECORDED_BLOCKS) {
            self.state.delete(Column::Validators, block_key(gone));
        }

        Ok(())
    }
}

impl crate::crypto::hashable::Hashable for RangeEnd {}

fn block_key(height: u64) -> Vec<u8> {
    [BLOCK, &height.to_be_bytes()].concat()
}
