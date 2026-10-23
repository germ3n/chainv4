//! The chain this node follows: genesis, every block it accepted, and which branch is the main
//! chain, the one with the most total work. A block that extends the tip is run and committed at
//! once. One on a side branch is stored without running; if its branch comes to weigh more than
//! the main chain, the main chain is rolled back to where they meet, using the undo record each
//! block left, and the branch is run on top, all in one commit. A crash never leaves state from
//! a block the main chain does not have.

use {
    crate::consensus::{Consensus, ConsensusConfig, ConsensusError, PowConfig, PowHash},
    crate::crypto::hashable::Hashable,
    crate::crypto::keypair::PublicKey,
    crate::db::{Column, Database, DbError, Read, State},
    crate::registry::{Accounts, RegistryError},
    crate::structs::address::Address,
    crate::structs::block::{Block, BlockHeader, Seal},
    crate::structs::code::Engine,
    crate::structs::hash::{Hash, HashType},
    crate::structs::tx::SignedTx,
    crate::structs::u256::{U256Le, u256},
    crate::vm::{Limits, Vm},
    r#macro::document,
    std::collections::HashSet,
    std::fmt,
    std::sync::Arc,
    wincode::{SchemaRead, SchemaWrite},
};

mod execute;
mod receipt;

pub use execute::{Effect, Failure, Receipt, Rules, TxError};
pub use receipt::{TxReceipt, TxStatus};

/// What one deploy or contract call may use. Part of consensus: every node must enforce the same.
pub const TX_LIMITS: Limits = Limits {
    fuel: 10_000_000,
    memory: 32 * 1024 * 1024,
};

/// How far back the main chain may be rolled back. A branch that splits off deeper is never
/// followed, however much work it holds, and undo records older than this are dropped.
pub const MAX_REORG_DEPTH: u64 = 100;

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
    summary = "What each block mints for its producer, credited after its transactions run. A fixed reward has no halving interval; with one, the reward halves every that many blocks until it reaches zero.",
    category = "Chain",
    see_also = "Genesis",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct Reward {
    #[document(summary = "The reward for the first blocks.")]
    #[wincode(with = "U256Le")]
    pub initial: u256,
    #[document(summary = "Blocks between halvings, or None for a reward that never changes.")]
    pub halving_interval: Option<u64>,
}

impl Hashable for Reward {}

impl Reward {
    #[document(parent = "Reward", summary = "No reward at all.")]
    pub const NONE: Reward = Reward {
        initial: u256::ZERO,
        halving_interval: None,
    };

    #[document(parent = "Reward", summary = "The reward for the block at a height.")]
    pub fn at(&self, height: u64) -> u256 {
        let halvings = match self.halving_interval {
            Some(interval) if interval > 0 => height / interval,
            _ => 0,
        };

        if halvings >= 256 {
            return u256::ZERO;
        }

        self.initial >> halvings as u32
    }
}

#[document(
    summary = "Everything a chain starts from. Nodes with different genesis are on different chains: the genesis block commits to all of this.",
    category = "Chain",
    see_also = "Chain::open, ConsensusConfig",
)]
#[derive(Clone, Debug, PartialEq, SchemaWrite, SchemaRead)]
pub struct Genesis {
    #[document(summary = "The genesis block's timestamp, in milliseconds since the Unix epoch.")]
    pub timestamp: u64,
    #[document(summary = "Which consensus the chain runs, and its parameters.")]
    pub consensus: ConsensusConfig,
    #[document(summary = "What each block mints for its producer.")]
    pub reward: Reward,
    #[document(summary = "The one address allowed to send Mint transactions, if any.")]
    pub minter: Option<Address>,
    #[document(summary = "Who holds what from the start.")]
    pub allocations: Vec<Allocation>,
    #[document(summary = "The engines contracts may be deployed for. A node built without one of them refuses to open the chain rather than fall out of step with it.")]
    pub engines: Vec<Engine>,
}

impl Hashable for Genesis {}

/// A million coins at 18 decimals.
fn dev_funds() -> u256 {
    u256::from(1_000_000u64) * u256::from(10u64).pow(18)
}

impl Genesis {
    #[document(
        parent = "Genesis",
        summary = "A development chain where one key produces a block every two seconds. That key holds a million coins at 18 decimals and may mint; there is no block reward. Allows every engine this node was built with, so nodes built with different engines are on different dev chains.",
        params = { authority = "The producing key, usually DevAuthority::dev_keypair." },
    )]
    pub fn dev_authority(authority: PublicKey) -> Genesis {
        Genesis {
            timestamp: 0,
            consensus: ConsensusConfig::Authority {
                key: authority,
                slot_ms: 2_000,
            },
            reward: Reward::NONE,
            minter: Some(authority.address()),
            allocations: vec![Allocation {
                address: authority.address(),
                amount: dev_funds(),
            }],
            engines: Engine::compiled(),
        }
    }

    #[document(
        parent = "Genesis",
        summary = "A development chain mined with proof of work on BLAKE3, aiming for a block every two seconds. Miners earn 50 coins a block, halving every 210,000 blocks. The development key holds a million coins and may mint, so the CLI works out of the box. Allows every engine this node was built with.",
        params = { dev = "The development key, usually DevAuthority::dev_keypair." },
    )]
    pub fn dev_pow(dev: PublicKey) -> Genesis {
        Genesis {
            timestamp: 0,
            consensus: ConsensusConfig::ProofOfWork(PowConfig {
                hash: PowHash::Blake3,
                target_block_ms: 2_000,
                initial_difficulty: 100_000,
                min_difficulty: 1_000,
            }),
            reward: Reward {
                initial: u256::from(50u64) * u256::from(10u64).pow(18),
                halving_interval: Some(210_000),
            },
            minter: Some(dev.address()),
            allocations: vec![Allocation {
                address: dev.address(),
                amount: dev_funds(),
            }],
            engines: Engine::compiled(),
        }
    }

    #[document(
        parent = "Genesis",
        summary = "The genesis block. Its tx_root is the hash of this configuration, and its producer is the authority's address, or the zero address under proof of work.",
    )]
    pub fn block(&self) -> Block {
        let producer = match &self.consensus {
            ConsensusConfig::Authority { key, .. } => key.address(),
            ConsensusConfig::ProofOfWork(_) => Address::Ed25519([0; 32]),
        };

        Block {
            header: BlockHeader {
                parent: Hash::Blake3_256([0; 32]),
                height: 0,
                timestamp: self.timestamp,
                producer,
                difficulty: 0,
                tx_root: self.hash(HashType::Blake3_256),
            },
            txs: Vec::new(),
            seal: Seal::None,
        }
    }

    #[document(
        parent = "Genesis",
        summary = "The rules transactions run under on this chain.",
    )]
    pub fn rules(&self) -> Rules {
        Rules {
            minter: self.minter,
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
    #[document(summary = "Crediting its reward would overflow the producer's balance.")]
    Reward,
    #[document(summary = "It, or a block it builds on, already failed to run.")]
    Bad,
}

impl fmt::Display for BlockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BlockError::Height => write!(f, "wrong height"),
            BlockError::TxRoot => write!(f, "tx_root does not match"),
            BlockError::Consensus(err) => write!(f, "{err}"),
            BlockError::Tx { index, error } => write!(f, "transaction {index}: {error}"),
            BlockError::Reward => write!(f, "the reward overflows the producer's balance"),
            BlockError::Bad => write!(f, "builds on a block that failed to run"),
        }
    }
}

#[document(
    summary = "What importing a block did.",
    category = "Chain",
    see_also = "Chain::import",
)]
#[derive(Debug)]
pub enum Imported {
    #[document(summary = "It extended the main chain. A receipt per transaction.")]
    Extended(Vec<Receipt>),
    #[document(summary = "It is stored on a side branch, which holds no more work than the main chain, or split off further back than MAX_REORG_DEPTH.")]
    Side,
    #[document(summary = "Its branch outweighed the main chain, which now follows it. undone are the blocks taken off the main chain, newest first; applied the ones put on, oldest first.")]
    Reorganized { undone: Vec<Block>, applied: Vec<Block> },
}

#[document(
    summary = "The chain this node follows: where blocks are stored, which branch is the main chain, and the rules blocks are checked against.",
    category = "Chain",
    see_also = "Genesis, Node, Consensus",
)]
pub struct Chain {
    db: Arc<dyn Database>,
    rules: Rules,
    vm: Arc<Vm>,
    consensus: Box<dyn Consensus>,
    reward: Reward,
    genesis: Hash,
    tip: BlockHeader,
    tip_work: u256,
    /// Blocks that failed to run, and so their descendants too. Forgotten on restart, when the
    /// branch would fail again the same way.
    bad: HashSet<Hash>,
}

impl Chain {
    #[document(
        parent = "Chain",
        summary = "Opens the chain in a database, writing genesis on first use.",
        returns = "The chain; WrongGenesis when the database holds a different chain, MissingEngine when this node lacks an engine genesis allows.",
    )]
    pub fn open(db: Arc<dyn Database>, genesis: &Genesis) -> Result<Chain, ChainError> {
        let vm = Arc::new(Vm::new(&genesis.engines).map_err(ChainError::MissingEngine)?);
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

                state.put_value(Column::Blocks, id_key(&id), &block);
                state.put_value(Column::Work, id_key(&id), &WorkRecord { total: u256::ZERO });
                state.put_value(Column::Heights, 0u64.to_be_bytes().to_vec(), &id);
                state.put_value(Column::Meta, TIP_KEY.to_vec(), &id);
                db.commit(state.take_changes())?;
                log::info!(
                    "[chain] wrote genesis {id} with {} allocations",
                    genesis.allocations.len()
                );
            }
        }

        let tip_id: Hash = read_value(&*db, Column::Meta, TIP_KEY)?.expect("genesis sets the tip");
        let tip: Block = read_value(&*db, Column::Blocks, &id_key(&tip_id))?.expect("the tip is stored");
        let tip_work = work_of(&*db, &tip_id)?;

        Ok(Chain {
            db,
            rules: genesis.rules(),
            vm,
            consensus: genesis.consensus.rules(),
            reward: genesis.reward,
            genesis: id,
            tip: tip.header,
            tip_work,
            bad: HashSet::new(),
        })
    }

    #[document(parent = "Chain", summary = "The newest main-chain block's header.")]
    pub fn tip(&self) -> &BlockHeader {
        &self.tip
    }

    #[document(parent = "Chain", summary = "The total work of the main chain.")]
    pub fn tip_work(&self) -> u256 {
        self.tip_work
    }

    #[document(parent = "Chain", summary = "The genesis block's id, which names the chain.")]
    pub fn genesis(&self) -> Hash {
        self.genesis
    }

    #[document(parent = "Chain", summary = "The rules blocks are checked against.")]
    pub fn consensus(&self) -> &dyn Consensus {
        &*self.consensus
    }

    #[document(parent = "Chain", summary = "A stored block by id, on the main chain or a side branch.")]
    pub fn block(&self, id: &Hash) -> Result<Option<Block>, DbError> {
        stored_block(&*self.db, id)
    }

    #[document(parent = "Chain", summary = "The main-chain block at a height.")]
    pub fn block_at(&self, height: u64) -> Result<Option<Block>, DbError> {
        stored_block_at(&*self.db, height)
    }

    #[document(parent = "Chain", summary = "Where a transaction landed on the main chain and what it did, if it did.")]
    pub fn receipt(&self, tx: &Hash) -> Result<Option<TxReceipt>, DbError> {
        stored_receipt(&*self.db, tx)
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
        summary = "The next block on the tip, unsealed: the producer still has to seal it.",
        params = {
            txs = "The transactions, in order.",
            producer = "Who produces it; the reward goes here.",
            timestamp = "When it is made, which also sets its difficulty.",
        },
    )]
    pub fn build(&self, txs: Vec<SignedTx>, producer: Address, timestamp: u64) -> Block {
        Block {
            header: BlockHeader {
                parent: self.tip.id(),
                height: self.tip.height + 1,
                timestamp,
                producer,
                difficulty: self.consensus.difficulty(&self.tip, timestamp),
                tx_root: Block::tx_root(&txs),
            },
            txs,
            seal: Seal::None,
        }
    }

    #[document(
        parent = "Chain",
        summary = "Checks a block against its parent and stores it. On the tip, it runs and becomes the new tip; on a side branch it waits, unless its branch now outweighs the main chain, which then reorganizes to follow it.",
        params = {
            block = "The block, from this node or a peer.",
            now = "This node's clock, in milliseconds since the Unix epoch.",
        },
        returns = "What the import did.",
    )]
    pub fn import(&mut self, block: &Block, now: u64) -> Result<Imported, ImportError> {
        let id = block.id();
        let parent_id = block.header.parent;

        if self.bad.contains(&id) {
            return Err(ImportError::Invalid(BlockError::Bad));
        }

        if self.block(&id)?.is_some() {
            return Err(ImportError::Known);
        }

        let Some(parent) = self.block(&parent_id)? else {
            return Err(ImportError::UnknownParent(parent_id));
        };

        if self.bad.contains(&parent_id) {
            self.bad.insert(id);

            return Err(ImportError::Invalid(BlockError::Bad));
        }

        if block.header.height != parent.header.height + 1 {
            return Err(ImportError::Invalid(BlockError::Height));
        }

        if block.header.tx_root != Block::tx_root(&block.txs) {
            return Err(ImportError::Invalid(BlockError::TxRoot));
        }

        self.consensus
            .check(&block.header, &block.seal, &parent.header, now)
            .map_err(|err| ImportError::Invalid(BlockError::Consensus(err)))?;
        let work = work_of(&*self.db, &parent_id)? + self.consensus.work(&block.header);
        let state = Arc::new(State::new(self.db.clone()));
        state.put_value(Column::Blocks, id_key(&id), block);
        state.put_value(Column::Work, id_key(&id), &WorkRecord { total: work });

        if parent_id == self.tip.id() {
            let receipts = match self.apply(&state, block) {
                Ok(receipts) => receipts,
                Err(err) => {
                    if matches!(err, ImportError::Invalid(_)) {
                        self.bad.insert(id);
                    }

                    return Err(err);
                }
            };
            self.db.commit(state.take_changes())?;
            self.tip = block.header.clone();
            self.tip_work = work;

            return Ok(Imported::Extended(receipts));
        }

        self.db.commit(state.take_changes())?;

        if work <= self.tip_work {
            return Ok(Imported::Side);
        }

        self.reorganize(block, work)
    }

    /// Makes block, whose branch outweighs the main chain, the tip: rolls the main chain back to
    /// where the branch splits off, then runs the branch, in one commit.
    fn reorganize(&mut self, new_tip: &Block, work: u256) -> Result<Imported, ImportError> {
        let mut branch = vec![new_tip.clone()];
        let mut fork = new_tip.header.parent;

        while !self.on_main_chain(&fork)? {
            if branch.len() as u64 > MAX_REORG_DEPTH {
                break;
            }

            let block = self.block(&fork)?.ok_or_else(|| missing(&fork))?;
            fork = block.header.parent;
            branch.push(block);
        }

        let fork_height = self.block(&fork)?.ok_or_else(|| missing(&fork))?.header.height;

        if !self.on_main_chain(&fork)? || self.tip.height - fork_height > MAX_REORG_DEPTH {
            log::warn!(
                "[chain] not following a heavier branch to {} that splits off more than {MAX_REORG_DEPTH} blocks back",
                new_tip.id()
            );

            return Ok(Imported::Side);
        }

        let state = Arc::new(State::new(self.db.clone()));
        let mut undone = Vec::new();

        for height in (fork_height + 1..=self.tip.height).rev() {
            let id: Hash = read_value(&*state, Column::Heights, &height.to_be_bytes())?
                .ok_or_else(|| DbError::Corrupt(format!("no main-chain block at height {height}")))?;
            revert(&state, &id)?;
            undone.push(self.block(&id)?.ok_or_else(|| missing(&id))?);
        }

        branch.reverse();

        for (position, block) in branch.iter().enumerate() {
            if let Err(err) = self.apply(&state, block) {
                if matches!(err, ImportError::Invalid(_)) {
                    self.bad.extend(branch[position..].iter().map(Block::id));
                }

                return Err(err);
            }
        }

        self.db.commit(state.take_changes())?;
        self.tip = new_tip.header.clone();
        self.tip_work = work;

        Ok(Imported::Reorganized {
            undone,
            applied: branch,
        })
    }

    /// Runs a block on top of below and makes it the tip there, keeping an undo record of every
    /// value it replaced.
    fn apply(&self, below: &Arc<State>, block: &Block) -> Result<Vec<Receipt>, ImportError> {
        let id = block.id();
        let height = block.header.height;
        let layer = Arc::new(State::new(below.clone()));
        let mut receipts = Vec::new();

        for (index, tx) in block.txs.iter().enumerate() {
            match execute::execute(&layer, tx, &self.rules, &self.vm) {
                Ok(receipt) => receipts.push(receipt),
                Err(execute::ExecuteError::Db(err)) => return Err(ImportError::Db(err)),
                Err(execute::ExecuteError::Invalid(error)) => {
                    return Err(ImportError::Invalid(BlockError::Tx { index, error }));
                }
            }
        }

        let reward = self.reward.at(height);

        if reward > u256::ZERO {
            Accounts::new(&layer)
                .credit(&block.header.producer, reward)
                .map_err(|err| match err {
                    RegistryError::Db(err) => ImportError::Db(err),
                    _ => ImportError::Invalid(BlockError::Reward),
                })?;
        }

        for (index, (tx, receipt)) in block.txs.iter().zip(&receipts).enumerate() {
            let record = TxReceipt::new(id, index as u32, receipt);
            layer.put_value(Column::Receipts, id_key(&tx.id()), &record);
        }

        layer.put_value(Column::Heights, height.to_be_bytes().to_vec(), &id);
        layer.put_value(Column::Meta, TIP_KEY.to_vec(), &id);
        let changes = layer.take_changes();
        let mut entries = Vec::with_capacity(changes.len() + 1);

        for (column, key) in changes.keys() {
            entries.push(UndoEntry {
                column: *column,
                key: key.clone(),
                value: below.get(*column, key)?,
            });
        }

        // Undoing the block also forgets how to undo it.
        entries.push(UndoEntry {
            column: Column::Undo,
            key: id_key(&id),
            value: None,
        });
        below.merge(changes);
        below.put_value(Column::Undo, id_key(&id), &Undo { entries });

        // Nothing rolls back further than MAX_REORG_DEPTH, so older undo records go.
        if let Some(old) = height.checked_sub(MAX_REORG_DEPTH + 1)
            && let Some(old_id) = read_value::<Hash>(&**below, Column::Heights, &old.to_be_bytes())?
        {
            below.delete(Column::Undo, id_key(&old_id));
        }

        Ok(receipts)
    }

    fn on_main_chain(&self, id: &Hash) -> Result<bool, DbError> {
        let Some(block) = self.block(id)? else {
            return Ok(false);
        };
        let at_height: Option<Hash> = read_value(&*self.db, Column::Heights, &block.header.height.to_be_bytes())?;

        Ok(at_height == Some(*id))
    }
}

/// Takes a main-chain block back off, restoring every value it replaced.
fn revert(state: &Arc<State>, id: &Hash) -> Result<(), DbError> {
    let undo: Undo = read_value(&**state, Column::Undo, &id_key(id))?
        .ok_or_else(|| DbError::Corrupt(format!("no undo record for {id}")))?;

    for entry in undo.entries {
        match entry.value {
            Some(value) => state.put(entry.column, entry.key, value),
            None => state.delete(entry.column, entry.key),
        }
    }

    Ok(())
}

fn missing(id: &Hash) -> DbError {
    DbError::Corrupt(format!("block {id} is not stored"))
}

/// One value a block replaced: what to write back to undo it, None meaning delete.
#[derive(Clone, Debug, SchemaWrite, SchemaRead)]
struct UndoEntry {
    column: Column,
    key: Vec<u8>,
    value: Option<Vec<u8>>,
}

#[derive(Clone, Debug, SchemaWrite, SchemaRead)]
struct Undo {
    entries: Vec<UndoEntry>,
}

impl Hashable for Undo {}

#[derive(Clone, Copy, Debug, SchemaWrite, SchemaRead)]
struct WorkRecord {
    #[wincode(with = "U256Le")]
    total: u256,
}

impl Hashable for WorkRecord {}

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
    #[document(summary = "The chain allows contracts for an engine this node was built without.")]
    MissingEngine(Engine),
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
            ChainError::MissingEngine(engine) => write!(
                f,
                "the chain runs {} contracts; build the node with the {} feature",
                engine.name(),
                match engine {
                    Engine::Lua => "lvm",
                    Engine::Wasm => "wasm",
                }
            ),
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
    summary = "The main-chain block at a height, read straight from a database.",
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

#[document(
    summary = "A transaction's receipt on the main chain, read straight from a database.",
    category = "Chain",
    returns = "The receipt, or None when the transaction is not on the main chain.",
)]
pub fn stored_receipt(db: &dyn Read, tx: &Hash) -> Result<Option<TxReceipt>, DbError> {
    read_value(db, Column::Receipts, &id_key(tx))
}

#[document(
    summary = "The total work of the chain ending at a block, read straight from a database.",
    category = "Chain",
)]
pub fn stored_work(db: &dyn Read, id: &Hash) -> Result<Option<u256>, DbError> {
    Ok(read_value::<WorkRecord>(db, Column::Work, &id_key(id))?.map(|record| record.total))
}

fn work_of(db: &dyn Read, id: &Hash) -> Result<u256, DbError> {
    stored_work(db, id)?.ok_or_else(|| DbError::Corrupt(format!("no work recorded for {id}")))
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
