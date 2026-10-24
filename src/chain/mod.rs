//! The chain this node follows: genesis, every block it accepted, and which branch is the main
//! chain: the one with the most total work among those holding the latest justified checkpoint,
//! never leaving the finalized one (see finality/). A block that extends the tip is run and
//! committed at once. One on a side branch is stored without running; if its branch comes to weigh more than
//! the main chain, the main chain is rolled back to where they meet, using the undo record each
//! block left, and the branch is run on top, all in one commit. A crash never leaves state from
//! a block the main chain does not have.

use {
    crate::consensus::{Consensus, ConsensusConfig, ConsensusError, PowConfig},
    crate::crypto::hashable::Hashable,
    crate::crypto::keypair::PublicKey,
    crate::db::{Column, Database, DbError, Read, State},
    crate::registry::tokens::STANDARD_TOKEN_FUNCTIONS,
    crate::registry::{Accounts, RegistryError},
    crate::structs::address::Address,
    crate::structs::block::{Block, BlockHeader, Seal},
    crate::structs::code::Engine,
    crate::structs::hash::{Hash, HashType},
    crate::structs::tx::SignedTx,
    crate::structs::u256::{U256Le, u256},
    crate::structs::vote::Justification,
    crate::vm::{Limits, Vm},
    r#macro::document,
    std::collections::HashSet,
    std::fmt,
    std::sync::Arc,
    wincode::{SchemaRead, SchemaWrite},
};

mod events;
mod execute;
pub mod fees;
mod finality;
mod parallel;
mod receipt;

pub use events::{EventFilter, EventIndex, EventsFrom, IndexEvents, IndexedEvent, stored_event_index, stored_events};
pub use execute::{BlockContext, Effect, Failure, Receipt, Rules, SignerRules, StakingRules, TxError};
pub use fees::{FeeConfig, Fees};
pub use parallel::Workers;
use parallel::{Runner, Unfit};
#[cfg(feature = "finality-authority")]
pub use finality::AuthorityConfig;
#[cfg(feature = "finality-depth")]
pub use finality::DepthConfig;
pub use finality::{
    Evidence, Finality, FinalityConfig, FinalityError, FinalityUpdate, View, stored_evidence, stored_finality,
    stored_justification,
};
#[cfg(feature = "finality-ffg")]
pub use finality::{FfgConfig, GenesisValidator};
pub use receipt::{TxReceipt, TxStatus};

/// The development chains' transaction limits: what one deploy or contract call may use.
pub const DEV_TX_LIMITS: Limits = Limits {
    fuel: 10_000_000,
    memory: 32 * 1024 * 1024,
};

/// The development chains' reorg limit.
pub const DEV_MAX_REORG_DEPTH: u64 = 100;

#[document(
    summary = "The layout this node keeps its database in. It goes up whenever a stored record changes shape, so a node never reads a database it would misread.",
    category = "Chain",
    see_also = "ChainError::OldFormat",
    note = "1: receipts carry traces, and the chain keeps its leaf blocks in the database. 2: authority finality's approvals of a signer change record the epoch they were made in. 3: the token registry.",
)]
pub const STORAGE_FORMAT: u32 = 3;

const TIP_KEY: &[u8] = b"tip";
const TIPS_KEY: &[u8] = b"tips";
const FORMAT_KEY: &[u8] = b"format";

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
    #[document(summary = "The chain's hash, for block and transaction ids, the roots blocks commit to and contract addresses: any secure algorithm this build has. Proof of work names its own.")]
    pub hash: HashType,
    #[document(summary = "What the chain's own coin is called, and where a display puts its point.")]
    pub coin: Coin,
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
    #[document(summary = "What one deploy, upgrade or contract call may use, called contracts included.")]
    pub tx_limits: Limits,
    #[document(summary = "The functions that make a contract a token: one that exports them all is in the token registry. Usually STANDARD_TOKEN_FUNCTIONS, ERC-20's required six; empty keeps no registry.")]
    pub token_functions: Vec<String>,
    #[document(summary = "How far back the main chain may be rolled back. A branch that splits off deeper is never followed, however much work it holds, and undo records older than this are dropped.")]
    pub max_reorg_depth: u64,
    #[document(summary = "Finality: its kind and every parameter of it, or None for a chain where only work decides.")]
    pub finality: Option<FinalityConfig>,
    #[document(summary = "What transactions pay for gas, and the block gas limit, or None for a chain where transactions are free and blocks unbounded.")]
    pub fees: Option<FeeConfig>,
}

impl Hashable for Genesis {}

#[document(
    summary = "How the chain's own coin is named and shown, so every client shows balances alike. Only displays read it: balances, fees and rewards are always counted in whole units.",
    category = "Chain",
    see_also = "Genesis",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct Coin {
    #[document(summary = "Its ticker, such as DEV: 1 to 12 ASCII letters and digits.")]
    pub symbol: String,
    #[document(summary = "How many decimals a display divides by: 18 means 10^18 units make one coin. At most 77, the digits a balance can have.")]
    pub decimals: u8,
}

impl Hashable for Coin {}

impl Coin {
    #[document(parent = "Coin", summary = "The development chains' coin: DEV, at 18 decimals.")]
    pub fn dev() -> Coin {
        Coin {
            symbol: "DEV".to_string(),
            decimals: 18,
        }
    }

    fn problem(&self) -> Option<&'static str> {
        if self.symbol.is_empty() || self.symbol.len() > 12 || !self.symbol.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
            return Some("the coin's symbol must be 1 to 12 ASCII letters and digits");
        }

        if self.decimals > 77 {
            return Some("the coin's decimals must be at most 77");
        }

        None
    }
}

#[document(
    summary = "STANDARD_TOKEN_FUNCTIONS as genesis keeps them: ERC-20's required six.",
    category = "Chain",
    see_also = "Genesis, STANDARD_TOKEN_FUNCTIONS",
)]
pub fn standard_token_functions() -> Vec<String> {
    STANDARD_TOKEN_FUNCTIONS.map(str::to_string).to_vec()
}

/// A million coins at 18 decimals.
fn dev_funds() -> u256 {
    u256::from(1_000_000u64) * u256::from(10u64).pow(18)
}

impl Genesis {
    #[document(
        parent = "Genesis",
        summary = "A development chain where one key produces a block every two seconds. That key holds a million coins at 18 decimals, may mint, and, in a build with FFG finality, is the one validator; there is no block reward, and fees follow FeeConfig::dev. Allows every engine this node was built with, so nodes built with different engines are on different dev chains.",
        params = { authority = "The producing key, usually DevAuthority::dev_keypair." },
    )]
    pub fn dev_authority(authority: PublicKey) -> Genesis {
        Genesis {
            timestamp: 0,
            hash: HashType::default_chain(),
            coin: Coin::dev(),
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
            tx_limits: DEV_TX_LIMITS,
            token_functions: standard_token_functions(),
            max_reorg_depth: DEV_MAX_REORG_DEPTH,
            finality: FinalityConfig::dev(authority),
            fees: Some(FeeConfig::dev()),
        }
    }

    #[document(
        parent = "Genesis",
        summary = "A development chain mined with proof of work, aiming for a block every two seconds. It hashes with HashType::default_chain, BLAKE3 in a default build, for both its ids and its work. Miners earn 50 coins a block, halving every 210,000 blocks. The development key holds a million coins, may mint, and, in a build with FFG finality, is the one validator, so the CLI works out of the box. Fees follow FeeConfig::dev. Allows every engine this node was built with.",
        params = { dev = "The development key, usually DevAuthority::dev_keypair." },
    )]
    pub fn dev_pow(dev: PublicKey) -> Genesis {
        Genesis {
            timestamp: 0,
            hash: HashType::default_chain(),
            coin: Coin::dev(),
            consensus: ConsensusConfig::ProofOfWork(PowConfig {
                hash: HashType::default_chain(),
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
            tx_limits: DEV_TX_LIMITS,
            token_functions: standard_token_functions(),
            max_reorg_depth: DEV_MAX_REORG_DEPTH,
            finality: FinalityConfig::dev(dev),
            fees: Some(FeeConfig::dev()),
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
                parent: Hash::zero(self.hash),
                height: 0,
                timestamp: self.timestamp,
                producer,
                difficulty: 0,
                tx_root: Hashable::hash(self, self.hash),
                proof_root: Block::proof_root(self.hash, &[]),
                gas_used: 0,
                base_price: self.fees.as_ref().map_or(u256::ZERO, |fees| fees.pricing.build().genesis_price()),
            },
            txs: Vec::new(),
            proofs: Vec::new(),
            seal: Seal::None,
        }
    }

    /// Settings no chain could run with.
    fn problem(&self) -> Option<&'static str> {
        if !self.hash.secure() {
            return Some("the chain's hash must be a secure 256-bit one, not xxh3");
        }

        if let ConsensusConfig::ProofOfWork(pow) = &self.consensus
            && !pow.hash.secure()
        {
            return Some("the proof-of-work hash must be a secure 256-bit one, not xxh3");
        }

        let mut token_functions: Vec<&String> = self.token_functions.iter().collect();
        token_functions.sort();

        if token_functions.iter().any(|name| name.is_empty()) || token_functions.windows(2).any(|pair| pair[0] == pair[1]) {
            return Some("token_functions must name each function once");
        }

        self.coin
            .problem()
            .or_else(|| self.finality.as_ref().and_then(FinalityConfig::problem))
            .or_else(|| self.fees.as_ref().and_then(FeeConfig::problem))
    }

    #[document(
        parent = "Genesis",
        summary = "The rules transactions run under on this chain.",
    )]
    pub fn rules(&self) -> Rules {
        Rules {
            hash: self.hash,
            minter: self.minter,
            limits: self.tx_limits,
            token_functions: self.token_functions.clone().into(),
            staking: self.finality.as_ref().and_then(|config| config.staking(self.block().id())),
            signers: self.finality.as_ref().and_then(FinalityConfig::signer_rules),
            fees: self.fees.as_ref().map(|config| Arc::new(config.build())),
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
    #[document(summary = "The proof_root does not match the finality proofs.")]
    ProofRoot,
    #[document(summary = "A finality proof it carries does not check out.")]
    Proof(String),
    #[document(summary = "It is on a branch that leaves the finalized checkpoint.")]
    ConflictsWithFinality,
    #[document(summary = "Its base price is not what its parent's makes it.")]
    BasePrice { expected: u256, got: u256 },
    #[document(summary = "Its gas_used is not what its transactions used.")]
    GasUsed { expected: u64, got: u64 },
    #[document(summary = "Crediting the producer its share of the fees would overflow its balance.")]
    Fee(String),
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
            BlockError::ProofRoot => write!(f, "proof_root does not match"),
            BlockError::Proof(why) => write!(f, "bad finality proof: {why}"),
            BlockError::ConflictsWithFinality => write!(f, "conflicts with the finalized checkpoint"),
            BlockError::BasePrice { expected, got } => write!(f, "base price {got}, but its parent makes it {expected}"),
            BlockError::GasUsed { expected, got } => write!(f, "gas_used {got}, but its transactions used {expected}"),
            BlockError::Fee(why) => write!(f, "paying the producer its fees: {why}"),
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
    #[document(summary = "It is stored on a side branch: one that holds no more work than the main chain, misses the latest justified checkpoint, or split off further back than max_reorg_depth.")]
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
    hash: HashType,
    vm: Arc<Vm>,
    consensus: Box<dyn Consensus>,
    reward: Reward,
    genesis: Hash,
    tip: BlockHeader,
    tip_work: u256,
    max_reorg_depth: u64,
    /// Blocks that failed to run, and so their descendants too. Forgotten on restart, when the
    /// branch would fail again the same way.
    bad: HashSet<Hash>,
    /// Stored blocks no stored block builds on yet, which a justification may move the chain to.
    tips: HashSet<Hash>,
    finality_config: Option<FinalityConfig>,
    finality: Option<Box<dyn Finality>>,
    /// The threads blocks run on, shared with finality to check votes.
    workers: Arc<Workers>,
    /// The event index, when this node keeps one.
    events: Option<EventIndex>,
}

impl Chain {
    #[document(
        parent = "Chain",
        summary = "Opens the chain in a database, writing genesis on first use.",
        returns = "The chain; WrongGenesis when the database holds a different chain, MissingEngine when this node lacks an engine genesis allows.",
    )]
    pub fn open(db: Arc<dyn Database>, genesis: &Genesis) -> Result<Chain, ChainError> {
        if let Some(problem) = genesis.problem() {
            return Err(ChainError::BadGenesis(problem.to_string()));
        }

        let vm = Arc::new(Vm::new(&genesis.engines, genesis.hash).map_err(ChainError::MissingEngine)?);
        let block = genesis.block();
        let id = block.id();
        let stored: Option<Hash> = read_value(&*db, Column::Heights, &0u64.to_be_bytes())?;

        match stored {
            Some(stored) if stored != id => return Err(ChainError::WrongGenesis { stored, expected: id }),
            Some(_) => {
                let found = read_value::<FormatRecord>(&*db, Column::Meta, FORMAT_KEY)?.map(|record| record.format);

                if found != Some(STORAGE_FORMAT) {
                    return Err(ChainError::OldFormat {
                        found,
                        expected: STORAGE_FORMAT,
                    });
                }
            }
            None => {
                let state = State::new(db.clone());
                let accounts = Accounts::new(&state);

                for allocation in &genesis.allocations {
                    accounts
                        .credit(&allocation.address, allocation.amount)
                        .map_err(|err| ChainError::BadGenesis(err.to_string()))?;
                }

                if let Some(config) = &genesis.finality {
                    config.write_genesis(&state)?;
                }

                state.put_value(Column::Blocks, id_key(&id), &block);
                state.put_value(Column::Work, id_key(&id), &WorkRecord { total: u256::ZERO });
                state.put_value(Column::Heights, 0u64.to_be_bytes().to_vec(), &id);
                state.put_value(Column::Meta, TIP_KEY.to_vec(), &id);
                state.put_value(Column::Meta, TIPS_KEY.to_vec(), &Leaves { blocks: vec![id] });
                state.put_value(Column::Meta, FORMAT_KEY.to_vec(), &FormatRecord { format: STORAGE_FORMAT });
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
        let tips = read_value::<Leaves>(&*db, Column::Meta, TIPS_KEY)?.map_or_else(|| vec![tip_id], |leaves| leaves.blocks);
        let workers = Arc::new(Workers::new());
        let finality = match &genesis.finality {
            Some(config) => Some(config.build(&View::new(&db, &tip.header, id, genesis.max_reorg_depth, &workers))?),
            None => None,
        };
        let events = events::stored_event_index(&*db)?;
        let mut chain = Chain {
            db,
            rules: genesis.rules(),
            hash: genesis.hash,
            vm,
            consensus: genesis.consensus.rules(),
            reward: genesis.reward,
            genesis: id,
            tip: tip.header,
            tip_work,
            max_reorg_depth: genesis.max_reorg_depth,
            bad: HashSet::new(),
            tips: tips.into_iter().collect(),
            finality_config: genesis.finality.clone(),
            finality,
            workers,
            events,
        };
        // In case the node stopped between committing a block and catching finality up with it.
        chain.tip_moved()?;

        Ok(chain)
    }

    #[document(parent = "Chain", summary = "How far back the main chain may be rolled back, from genesis.")]
    pub fn max_reorg_depth(&self) -> u64 {
        self.max_reorg_depth
    }

    #[document(
        parent = "Chain",
        summary = "Keeps the event index, and sets which blocks it keeps. A new index starts empty at the next block, and catch_up_events fills it in; whatever an index dropped half-way left behind is cleared first. Once on, it stays on until drop_event_index.",
        params = { keep = "Which blocks to keep; None keeps what the database keeps, or every block for a new index." },
        returns = "The index.",
        see_also = "Chain::catch_up_events, stored_events",
    )]
    pub fn index_events(&mut self, keep: Option<EventsFrom>) -> Result<EventIndex, DbError> {
        let index = match (self.events, keep) {
            (Some(index), None) => return Ok(index),
            (Some(index), Some(keep)) if index.keep == keep => return Ok(index),
            (Some(index), Some(keep)) => EventIndex { keep, ..index },
            (None, keep) => {
                let keep = keep.unwrap_or(EventsFrom::Height(0));

                EventIndex {
                    from: keep.lowest(self.tip.height).max(self.tip.height + 1),
                    keep,
                }
            }
        };
        let state = State::new(self.db.clone());
        state.put_value(Column::Meta, events::FROM_KEY.to_vec(), &events::IndexedFrom { height: index.from });
        state.put_value(Column::Meta, events::KEEP_KEY.to_vec(), &index.keep);

        match self.events {
            Some(_) => self.db.commit(state.take_changes())?,
            None => self.db.clear(Column::Events, state.take_changes())?,
        }

        self.events = Some(index);

        Ok(index)
    }

    #[document(
        parent = "Chain",
        summary = "Stops keeping the event index, and deletes it and its records in Meta in one atomic write, quick however big it is.",
        returns = "Whether there was an index to drop.",
        see_also = "Chain::index_events",
    )]
    pub fn drop_event_index(&mut self) -> Result<bool, DbError> {
        let state = State::new(self.db.clone());
        state.delete(Column::Meta, events::FROM_KEY.to_vec());
        state.delete(Column::Meta, events::KEEP_KEY.to_vec());
        self.db.clear(Column::Events, state.take_changes())?;

        Ok(self.events.take().is_some())
    }

    #[document(
        parent = "Chain",
        summary = "Moves the event index's low end a step toward what it keeps: indexing older blocks' events from their receipts, newest first, or dropping the oldest blocks' events. A step reads at most a few hundred blocks and commits once, so the index never has gaps, and a catch-up cut short picks up where it stopped.",
        returns = "Whether there is more catching up to do.",
        see_also = "Chain::index_events",
    )]
    pub fn catch_up_events(&mut self) -> Result<bool, DbError> {
        let Some(index) = self.events else {
            return Ok(false);
        };

        if index.caught_up(self.tip.height) {
            return Ok(false);
        }

        let index = EventIndex {
            from: events::catch_up(&self.db, self.hash, index, self.tip.height)?,
            ..index
        };
        self.events = Some(index);

        Ok(!index.caught_up(self.tip.height))
    }

    #[document(parent = "Chain", summary = "The threads the chain runs blocks and checks signatures on, for others to share.")]
    pub fn workers(&self) -> Arc<Workers> {
        self.workers.clone()
    }

    #[document(parent = "Chain", summary = "The event index, when this node keeps one.")]
    pub fn event_index(&self) -> Option<EventIndex> {
        self.events
    }

    #[document(parent = "Chain", summary = "The newest main-chain block's header.")]
    pub fn tip(&self) -> &BlockHeader {
        &self.tip
    }

    #[document(parent = "Chain", summary = "The total work of the main chain.")]
    pub fn tip_work(&self) -> u256 {
        self.tip_work
    }

    #[document(parent = "Chain", summary = "The chain's hash, from genesis: what block and transaction ids, roots and contract addresses are hashed with.")]
    pub fn hash(&self) -> HashType {
        self.hash
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
            proofs = "Finality proofs to carry, which pay their voters; unpaid_proofs gives them.",
            producer = "Who produces it; the reward goes here.",
            timestamp = "When it is made, which also sets its difficulty.",
        },
    )]
    pub fn build(&self, txs: Vec<SignedTx>, proofs: Vec<Justification>, producer: Address, timestamp: u64) -> Result<Block, DbError> {
        let context = BlockContext {
            height: self.tip.height + 1,
            producer,
            base_price: self.next_base_price(),
        };
        // The transactions run here, on a layer that is thrown away, to keep only those that can
        // run and fit, and to learn the gas they use.
        let layer = Arc::new(State::new(self.db.clone()));
        let ran = match self.runner().run(&layer, &txs, &context, Unfit::Skip) {
            Ok(ran) => ran,
            Err(ImportError::Db(err)) => return Err(err),
            Err(err) => unreachable!("building skips what does not fit, so only the database fails it: {err}"),
        };
        let gas_used = ran.gas_used;
        let kept: Vec<SignedTx> = ran.kept.into_iter().map(|index| txs[index].clone()).collect();

        Ok(Block {
            header: BlockHeader {
                parent: self.tip.id(),
                height: context.height,
                timestamp,
                producer,
                difficulty: self.consensus.difficulty(&self.tip, timestamp),
                tx_root: Block::tx_root(self.hash, &kept),
                proof_root: Block::proof_root(self.hash, &proofs),
                gas_used,
                base_price: context.base_price,
            },
            txs: kept,
            proofs,
            seal: Seal::None,
        })
    }

    #[document(parent = "Chain", summary = "The chain's fees, or None when transactions are free.")]
    pub fn fees(&self) -> Option<&Fees> {
        self.rules.fees.as_deref()
    }

    #[document(parent = "Chain", summary = "The base price of gas in the next block, the one on the tip: what a transaction sent now must offer at least.")]
    pub fn next_base_price(&self) -> u256 {
        self.base_price_after(&self.tip)
    }

    #[document(
        parent = "Chain",
        summary = "What a producer gains per gas from a transaction in the next block, which is what producers order the mempool by.",
    )]
    pub fn priority(&self, tx: &SignedTx) -> u256 {
        self.fees()
            .map_or(u256::ZERO, |fees| fees.priority(self.next_base_price(), &tx.fee))
    }

    /// The base price of the block after parent.
    fn base_price_after(&self, parent: &BlockHeader) -> u256 {
        match self.fees() {
            Some(fees) => fees.pricing().next_price(parent.base_price, parent.gas_used, fees.block_gas_limit()),
            None => u256::ZERO,
        }
    }

    /// What a block's transactions run with.
    fn runner(&self) -> Runner<'_> {
        Runner {
            rules: &self.rules,
            vm: &self.vm,
            workers: &self.workers,
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

        if block.header.tx_root != Block::tx_root(self.hash, &block.txs) {
            return Err(ImportError::Invalid(BlockError::TxRoot));
        }

        if block.header.proof_root != Block::proof_root(self.hash, &block.proofs) {
            return Err(ImportError::Invalid(BlockError::ProofRoot));
        }

        let base_price = self.base_price_after(&parent.header);

        if block.header.base_price != base_price {
            return Err(ImportError::Invalid(BlockError::BasePrice {
                expected: base_price,
                got: block.header.base_price,
            }));
        }

        if self.finalized_height().is_some_and(|finalized| block.header.height <= finalized) {
            // At or below the finalized checkpoint, only the main chain's blocks exist.
            return Err(ImportError::Invalid(BlockError::ConflictsWithFinality));
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
            self.stored_leaf(&state, id, &parent_id);
            self.db.commit(state.take_changes())?;
            self.tip = block.header.clone();
            self.tip_work = work;
            self.tip_moved()?;

            return Ok(Imported::Extended(receipts));
        }

        self.stored_leaf(&state, id, &parent_id);
        self.db.commit(state.take_changes())?;

        if work <= self.tip_work {
            return Ok(Imported::Side);
        }

        self.reorganize(block, work, false)
    }

    /// Notes that a block is being stored: no stored block builds on it yet, and its parent is
    /// built on now. The leaves go in the commit too, for readers such as the API.
    fn stored_leaf(&mut self, state: &State, id: Hash, parent: &Hash) {
        self.tips.remove(parent);
        self.tips.insert(id);
        let blocks = self.tips.iter().copied().collect();
        state.put_value(Column::Meta, TIPS_KEY.to_vec(), &Leaves { blocks });
    }

    /// Makes new_tip the tip: rolls the main chain back to where its branch splits off, then runs
    /// the branch, in one commit. A branch that outweighs the main chain is only followed when it
    /// holds the latest justified checkpoint; a justified one (justified set) only needs to hold
    /// the finalized one. Neither may split off deeper than max_reorg_depth.
    fn reorganize(&mut self, new_tip: &Block, work: u256, justified: bool) -> Result<Imported, ImportError> {
        let mut branch = vec![new_tip.clone()];
        let mut fork = new_tip.header.parent;

        while !self.on_main_chain(&fork)? {
            if branch.len() as u64 > self.max_reorg_depth {
                break;
            }

            let block = self.block(&fork)?.ok_or_else(|| missing(&fork))?;
            fork = block.header.parent;
            branch.push(block);
        }

        let fork_height = self.block(&fork)?.ok_or_else(|| missing(&fork))?.header.height;

        if !self.on_main_chain(&fork)? || self.tip.height - fork_height > self.max_reorg_depth {
            log::warn!(
                "[chain] not following a branch to {} that splits off more than {} blocks back",
                new_tip.id(),
                self.max_reorg_depth
            );

            return Ok(Imported::Side);
        }

        if let Some(finality) = &self.finality {
            let anchor = if justified { finality.finalized() } else { finality.justified() };

            if fork_height < finality.height_of(&anchor) {
                log::info!(
                    "[chain] not following a branch to {} that leaves the {} checkpoint of epoch {}",
                    new_tip.id(),
                    if justified { "finalized" } else { "justified" },
                    anchor.epoch
                );

                return Ok(Imported::Side);
            }
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
        self.tip_moved()?;

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
        let context = BlockContext {
            height,
            producer: block.header.producer,
            base_price: block.header.base_price,
        };
        let ran = self.runner().run(&layer, &block.txs, &context, Unfit::Refuse)?;
        let receipts = ran.receipts;

        if ran.gas_used != block.header.gas_used {
            return Err(ImportError::Invalid(BlockError::GasUsed {
                expected: ran.gas_used,
                got: block.header.gas_used,
            }));
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

        match &self.finality {
            Some(finality) => finality.apply(&layer, block)?,
            None if !block.proofs.is_empty() => {
                return Err(ImportError::Invalid(BlockError::Proof("this chain has no finality".to_string())));
            }
            None => {}
        }

        for (index, (tx, receipt)) in block.txs.iter().zip(&receipts).enumerate() {
            let record = TxReceipt::new(id, index as u32, receipt);
            let tx_id = tx.id(self.hash);
            layer.put_value(Column::Receipts, id_key(&tx_id), &record);

            // Blocks below the low end wait for catch_up_events, or were dropped.
            if self.events.is_some_and(|events| height >= events.from) {
                events::index(&layer, id, height, tx_id, index as u32, &receipt.events);
            }
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

        // Nothing rolls back further than max_reorg_depth, so older undo records go.
        if let Some(old) = height.checked_sub(self.max_reorg_depth + 1)
            && let Some(old_id) = read_value::<Hash>(&**below, Column::Heights, &old.to_be_bytes())?
        {
            below.delete(Column::Undo, id_key(&old_id));
        }

        Ok(receipts)
    }

    pub(super) fn on_main_chain(&self, id: &Hash) -> Result<bool, DbError> {
        self.view().on_main_chain(id)
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

/// The stored blocks no stored block builds on yet.
#[derive(Clone, Debug, Default, SchemaWrite, SchemaRead)]
struct Leaves {
    blocks: Vec<Hash>,
}

impl Hashable for Leaves {}

/// Which STORAGE_FORMAT a database is kept in.
#[derive(Clone, Copy, Debug, SchemaWrite, SchemaRead)]
struct FormatRecord {
    format: u32,
}

impl Hashable for FormatRecord {}

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
    #[document(summary = "The database is kept in another layout than STORAGE_FORMAT: found is the one it is in, None for one from before formats were recorded.")]
    OldFormat { found: Option<u32>, expected: u32 },
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
            ChainError::OldFormat { found, expected } => {
                let found = found.map_or("an older one".to_string(), |found| format!("format {found}"));

                write!(f, "the database is kept in {found}, not format {expected}; delete it or use another --db")
            }
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
    summary = "The stored blocks no stored block builds on yet, the tip among them, read straight from a database. Every side branch ends in one.",
    category = "Chain",
)]
pub fn stored_tips(db: &dyn Read) -> Result<Vec<Hash>, DbError> {
    Ok(read_value::<Leaves>(db, Column::Meta, TIPS_KEY)?.unwrap_or_default().blocks)
}

#[document(
    summary = "Whether a block is on the main chain, read straight from a database.",
    category = "Chain",
)]
pub fn stored_on_main_chain(db: &dyn Read, block: &Block) -> Result<bool, DbError> {
    let at: Option<Hash> = read_value(db, Column::Heights, &block.header.height.to_be_bytes())?;

    Ok(at == Some(block.id()))
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

pub(super) fn work_of(db: &dyn Read, id: &Hash) -> Result<u256, DbError> {
    stored_work(db, id)?.ok_or_else(|| DbError::Corrupt(format!("no work recorded for {id}")))
}

pub(super) fn id_key(id: &Hash) -> Vec<u8> {
    id.hashable_bytes_with_tag()
}

pub(super) fn read_value<T>(db: &dyn Read, column: Column, key: &[u8]) -> Result<Option<T>, DbError>
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
