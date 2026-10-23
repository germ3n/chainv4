//! The chain this node follows: genesis, the stored blocks, and the tip. A block is imported
//! atomically: its state changes, the block itself and the new tip go to the database in one
//! commit, so a crash never leaves state from a block the chain does not have.

use {
    crate::consensus::{Consensus, ConsensusError},
    crate::crypto::hashable::Hashable,
    crate::crypto::keypair::PublicKey,
    crate::db::{Column, Database, DbError, Read, State},
    crate::registry::Accounts,
    crate::structs::address::Address,
    crate::structs::block::{Block, BlockHeader},
    crate::structs::hash::{Hash, HashType},
    crate::structs::tx::SignedTx,
    crate::structs::u256::{U256Le, u256},
    crate::vm::lvm::Limits,
    r#macro::document,
    std::fmt,
    std::sync::Arc,
    wincode::{SchemaRead, SchemaWrite},
};

mod execute;

pub use execute::{Effect, Failure, Receipt, Rules, TxError};

/// What one deploy or contract call may use. Part of consensus: every node must enforce the same.
pub const TX_LIMITS: Limits = Limits {
    fuel: 10_000_000,
    memory: 32 * 1024 * 1024,
};

const TIP_KEY: &[u8] = b"tip";

#[document(
    summary = "Funds an address holds from the start.",
    category = "Chain",
    see_also = "Genesis",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct Allocation {
    #[document(summary = "Who holds them.")]
    pub address: Address,
    #[document(summary = "How much.")]
    #[wincode(with = "U256Le")]
    pub amount: u256,
}

impl Hashable for Allocation {}

#[document(
    summary = "Everything a chain starts from. Nodes with different genesis are on different chains: the genesis block commits to all of this.",
    category = "Chain",
    see_also = "Chain::open",
)]
#[derive(Clone, Debug, PartialEq, SchemaWrite, SchemaRead)]
pub struct Genesis {
    #[document(summary = "The genesis block's timestamp, in milliseconds since the Unix epoch.")]
    pub timestamp: u64,
    #[document(summary = "The key that produces blocks under the dev authority, and the only one that may mint.")]
    pub authority: PublicKey,
    #[document(summary = "Milliseconds between blocks.")]
    pub slot_ms: u64,
    #[document(summary = "Who holds what from the start.")]
    pub allocations: Vec<Allocation>,
}

impl Hashable for Genesis {}

impl Genesis {
    #[document(
        parent = "Genesis",
        summary = "A development chain: one block every two seconds, and a million coins at 18 decimals for the authority.",
        params = { authority = "The producing key, usually DevAuthority::dev_keypair." },
    )]
    pub fn dev(authority: PublicKey) -> Genesis {
        Genesis {
            timestamp: 0,
            authority,
            slot_ms: 2_000,
            allocations: vec![Allocation {
                address: authority.address(),
                amount: u256::from(1_000_000u64) * u256::from(10u64).pow(18),
            }],
        }
    }

    #[document(
        parent = "Genesis",
        summary = "The genesis block. Its tx_root is the hash of this configuration.",
    )]
    pub fn block(&self) -> Block {
        Block {
            header: BlockHeader {
                parent: Hash::Blake3_256([0; 32]),
                height: 0,
                timestamp: self.timestamp,
                producer: self.authority,
                tx_root: self.hash(HashType::Blake3_256),
            },
            txs: Vec::new(),
            seal: None,
        }
    }

    #[document(
        parent = "Genesis",
        summary = "The rules transactions run under on this chain.",
    )]
    pub fn rules(&self) -> Rules {
        Rules {
            minter: Some(self.authority.address()),
            limits: TX_LIMITS,
        }
    }
}

#[document(
    summary = "Why a block could not be imported.",
    category = "Chain",
)]
#[derive(Debug)]
pub enum ImportError {
    #[document(summary = "The database failed. The node's fault: stop rather than carry on.")]
    Db(DbError),
    #[document(summary = "This node already has the block.")]
    Known,
    #[document(summary = "This node does not have the block's parent yet. Sync must fetch the blocks in between.")]
    UnknownParent(Hash),
    #[document(summary = "The block extends a block that is not the tip. Choosing between forks comes later.")]
    NotOnTip(Hash),
    #[document(summary = "The block breaks the rules. Never relay it.")]
    Invalid(BlockError),
}

impl From<DbError> for ImportError {
    fn from(err: DbError) -> ImportError {
        ImportError::Db(err)
    }
}

impl fmt::Display for ImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ImportError::Db(err) => write!(f, "{err}"),
            ImportError::Known => write!(f, "already known"),
            ImportError::UnknownParent(parent) => write!(f, "unknown parent {parent}"),
            ImportError::NotOnTip(parent) => write!(f, "extends {parent}, not the tip"),
            ImportError::Invalid(err) => write!(f, "invalid: {err}"),
        }
    }
}

#[document(
    summary = "What makes a block invalid.",
    category = "Chain",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockError {
    #[document(summary = "The height is not one more than the parent's.")]
    Height,
    #[document(summary = "The tx_root does not match the transactions.")]
    TxRoot,
    #[document(summary = "Consensus rejected it.")]
    Consensus(ConsensusError),
    #[document(summary = "A transaction in it could not run at all.")]
    Tx { index: usize, error: TxError },
}

impl fmt::Display for BlockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BlockError::Height => write!(f, "wrong height"),
            BlockError::TxRoot => write!(f, "tx_root does not match"),
            BlockError::Consensus(err) => write!(f, "{err}"),
            BlockError::Tx { index, error } => write!(f, "transaction {index}: {error}"),
        }
    }
}

#[document(
    summary = "The chain this node follows: where blocks are stored, and which one is the tip.",
    category = "Chain",
    see_also = "Genesis, Node",
)]
pub struct Chain {
    db: Arc<dyn Database>,
    rules: Rules,
    genesis: Hash,
    tip: BlockHeader,
}

impl Chain {
    #[document(
        parent = "Chain",
        summary = "Opens the chain in a database, writing genesis on first use.",
        returns = "The chain, or WrongGenesis when the database holds a different chain.",
    )]
    pub fn open(db: Arc<dyn Database>, genesis: &Genesis) -> Result<Chain, ChainError> {
        let block = genesis.block();
        let id = block.id();
        let stored: Option<Hash> = read_value(&*db, Column::Heights, &0u64.to_be_bytes())?;

        match stored {
            Some(stored) if stored != id => return Err(ChainError::WrongGenesis { stored, expected: id }),
            Some(_) => {}
            None => {
                let state = State::new(db.clone());
                let accounts = Accounts::new(&state);

                for allocation in &genesis.allocations {
                    accounts
                        .credit(&allocation.address, allocation.amount)
                        .map_err(|err| ChainError::BadGenesis(err.to_string()))?;
                }

                store(&state, &block);
                db.commit(state.take_changes())?;
                log::info!(
                    "[chain] wrote genesis {id} with {} allocations",
                    genesis.allocations.len()
                );
            }
        }

        let tip_id: Hash = read_value(&*db, Column::Meta, TIP_KEY)?.expect("genesis sets the tip");
        let tip: Block = read_value(&*db, Column::Blocks, &id_key(&tip_id))?.expect("the tip is stored");

        Ok(Chain {
            db,
            rules: genesis.rules(),
            genesis: id,
            tip: tip.header,
        })
    }

    #[document(parent = "Chain", summary = "The newest block's header.")]
    pub fn tip(&self) -> &BlockHeader {
        &self.tip
    }

    #[document(parent = "Chain", summary = "The genesis block's id, which names the chain.")]
    pub fn genesis(&self) -> Hash {
        self.genesis
    }

    #[document(parent = "Chain", summary = "A stored block by id.")]
    pub fn block(&self, id: &Hash) -> Result<Option<Block>, DbError> {
        stored_block(&*self.db, id)
    }

    #[document(parent = "Chain", summary = "The block at a height on this chain.")]
    pub fn block_at(&self, height: u64) -> Result<Option<Block>, DbError> {
        stored_block_at(&*self.db, height)
    }

    #[document(
        parent = "Chain",
        summary = "A read-only view of the state at the tip, for checks outside a block such as admitting transactions to the mempool.",
    )]
    pub fn state(&self) -> State {
        State::new(self.db.clone())
    }

    #[document(
        parent = "Chain",
        summary = "Builds and seals the next block from transactions. Does not import it.",
        returns = "The block, or None when this node does not produce.",
    )]
    pub fn build(&self, txs: Vec<SignedTx>, consensus: &dyn Consensus, now: u64) -> Option<Block> {
        let slot = consensus.next_slot(&self.tip)?;
        let header = BlockHeader {
            parent: self.tip.id(),
            height: self.tip.height + 1,
            timestamp: now.max(slot),
            producer: consensus.producer()?,
            tx_root: Block::tx_root(&txs),
        };
        let seal = consensus.seal(&header)?;

        Some(Block {
            header,
            txs,
            seal: Some(seal),
        })
    }

    #[document(
        parent = "Chain",
        summary = "Checks a block, runs its transactions and makes it the tip, all in one commit.",
        params = {
            block = "The block, from this node or a peer.",
            consensus = "Decides whether the right key produced it at the right time.",
            now = "This node's clock, in milliseconds since the Unix epoch.",
        },
        returns = "A receipt per transaction.",
    )]
    pub fn import(&mut self, block: &Block, consensus: &dyn Consensus, now: u64) -> Result<Vec<Receipt>, ImportError> {
        let id = block.id();
        let parent = block.header.parent;

        if self.block(&id)?.is_some() {
            return Err(ImportError::Known);
        }

        if parent != self.tip.id() {
            if self.block(&parent)?.is_some() {
                return Err(ImportError::NotOnTip(parent));
            }

            return Err(ImportError::UnknownParent(parent));
        }

        if block.header.height != self.tip.height + 1 {
            return Err(ImportError::Invalid(BlockError::Height));
        }

        if block.header.tx_root != Block::tx_root(&block.txs) {
            return Err(ImportError::Invalid(BlockError::TxRoot));
        }

        consensus
            .check(&block.header, block.seal.as_ref(), &self.tip, now)
            .map_err(|err| ImportError::Invalid(BlockError::Consensus(err)))?;
        let state = Arc::new(State::new(self.db.clone()));
        let mut receipts = Vec::new();

        for (index, tx) in block.txs.iter().enumerate() {
            match execute::execute(&state, tx, &self.rules) {
                Ok(receipt) => receipts.push(receipt),
                Err(execute::ExecuteError::Db(err)) => return Err(ImportError::Db(err)),
                Err(execute::ExecuteError::Invalid(error)) => {
                    return Err(ImportError::Invalid(BlockError::Tx { index, error }));
                }
            }
        }

        store(&state, block);
        self.db.commit(state.take_changes())?;
        self.tip = block.header.clone();

        Ok(receipts)
    }
}

#[document(
    summary = "Why the chain could not be opened.",
    category = "Chain",
)]
#[derive(Debug)]
pub enum ChainError {
    #[document(summary = "The database failed.")]
    Db(DbError),
    #[document(summary = "The database holds a different chain.")]
    WrongGenesis { stored: Hash, expected: Hash },
    #[document(summary = "The genesis configuration cannot be applied, for example because allocations overflow.")]
    BadGenesis(String),
}

impl From<DbError> for ChainError {
    fn from(err: DbError) -> ChainError {
        ChainError::Db(err)
    }
}

impl fmt::Display for ChainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ChainError::Db(err) => write!(f, "{err}"),
            ChainError::WrongGenesis { stored, expected } => {
                write!(f, "the database holds genesis {stored}, not {expected}")
            }
            ChainError::BadGenesis(text) => write!(f, "bad genesis: {text}"),
        }
    }
}

impl std::error::Error for ChainError {}

#[document(
    summary = "A block by id, read straight from a database. For readers outside the node, such as the HTTP API.",
    category = "Chain",
)]
pub fn stored_block(db: &dyn Read, id: &Hash) -> Result<Option<Block>, DbError> {
    read_value(db, Column::Blocks, &id_key(id))
}

#[document(
    summary = "The block at a height, read straight from a database.",
    category = "Chain",
)]
pub fn stored_block_at(db: &dyn Read, height: u64) -> Result<Option<Block>, DbError> {
    let Some(id) = read_value::<Hash>(db, Column::Heights, &height.to_be_bytes())? else {
        return Ok(None);
    };

    stored_block(db, &id)
}

#[document(
    summary = "The tip block, read straight from a database.",
    category = "Chain",
    returns = "The tip, or None before genesis is written.",
)]
pub fn stored_tip(db: &dyn Read) -> Result<Option<Block>, DbError> {
    let Some(id) = read_value::<Hash>(db, Column::Meta, TIP_KEY)? else {
        return Ok(None);
    };

    stored_block(db, &id)
}

/// Writes a block, its height and the new tip into a state layer.
fn store(state: &State, block: &Block) {
    let id = block.id();
    state.put_value(Column::Blocks, id_key(&id), block);
    state.put_value(Column::Heights, block.header.height.to_be_bytes().to_vec(), &id);
    state.put_value(Column::Meta, TIP_KEY.to_vec(), &id);
}

fn id_key(id: &Hash) -> Vec<u8> {
    id.hashable_bytes_with_tag()
}

fn read_value<T>(db: &dyn Read, column: Column, key: &[u8]) -> Result<Option<T>, DbError>
where
    T: for<'de> wincode::SchemaRead<'de, wincode::config::DefaultConfig, Dst = T>,
{
    let Some(bytes) = db.read(column, key)? else {
        return Ok(None);
    };

    wincode::deserialize(&bytes)
        .map(Some)
        .map_err(|err| DbError::Corrupt(err.to_string()))
}

#[cfg(test)]
mod tests;
