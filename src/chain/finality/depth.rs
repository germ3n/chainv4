//! Depth finality: no votes. A main-chain block is final once depth blocks are on top of it, and
//! the chain then never reorganizes past it, however much work another branch holds. Every block
//! is a checkpoint, so the finalized checkpoint's epoch is its height.
//!
//! Finality here is each node's own view: nodes kept apart for more than depth blocks, by a
//! partition or a long private fork, each finalize their own branch and never come back together.

use {
    super::{FINALIZED_KEY, Finality, JUSTIFIED_KEY, View},
    crate::chain::{BlockError, ImportError, read_value},
    crate::crypto::hashable::Hashable,
    crate::db::{Column, DbError, Read, State},
    crate::structs::block::Block,
    crate::structs::hash::Hash,
    crate::structs::vote::Checkpoint,
    r#macro::document,
    std::sync::Arc,
    wincode::{SchemaRead, SchemaWrite},
};

#[document(
    summary = "The one parameter of depth finality, set in genesis.",
    category = "Finality",
    see_also = "FinalityConfig",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct DepthConfig {
    #[document(summary = "How many blocks must be on top of a block for it to be final. 0 makes every block final as soon as it is the tip, so the chain never reorganizes.")]
    pub depth: u64,
}

impl Hashable for DepthConfig {}

impl DepthConfig {
    #[document(parent = "DepthConfig", summary = "Development settings: final under 6 blocks.")]
    pub fn dev() -> DepthConfig {
        DepthConfig { depth: 6 }
    }
}

/// The block depth blocks below the tip, and everything under it, is final.
pub(super) struct Depth {
    depth: u64,
    finalized: Checkpoint,
}

impl Depth {
    pub(super) fn load(config: &DepthConfig, db: &dyn Read, genesis: Hash) -> Result<Depth, DbError> {
        let start = Checkpoint {
            epoch: 0,
            block: genesis,
        };

        Ok(Depth {
            depth: config.depth,
            finalized: read_value(db, Column::Finality, FINALIZED_KEY)?.unwrap_or(start),
        })
    }
}

impl Finality for Depth {
    fn name(&self) -> &'static str {
        "depth"
    }

    fn checkpoint_interval(&self) -> u64 {
        1
    }

    fn finalized(&self) -> Checkpoint {
        self.finalized
    }

    fn justified(&self) -> Checkpoint {
        self.finalized
    }

    fn apply(&self, _state: &Arc<State>, block: &Block) -> Result<(), ImportError> {
        match block.proofs.is_empty() {
            true => Ok(()),
            false => Err(ImportError::Invalid(BlockError::Proof("this chain's finality takes no proofs".to_string()))),
        }
    }

    /// Moves the finalized checkpoint up to depth blocks below the tip, and stores it for readers
    /// such as the API.
    fn tip_moved(&mut self, chain: &View) -> Result<(), DbError> {
        let height = chain.tip().height.saturating_sub(self.depth);

        if height <= self.finalized.epoch {
            return Ok(());
        }

        let header = chain
            .header_at(height)?
            .ok_or_else(|| DbError::Corrupt(format!("no main-chain block at height {height}")))?;
        self.finalized = Checkpoint {
            epoch: height,
            block: header.id(),
        };
        let state = State::new(chain.db().clone());
        state.put_value(Column::Finality, JUSTIFIED_KEY.to_vec(), &self.finalized);
        state.put_value(Column::Finality, FINALIZED_KEY.to_vec(), &self.finalized);

        chain.db().commit(state.take_changes())
    }
}
