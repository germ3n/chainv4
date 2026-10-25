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
    crate::db::{Column, Database, DbError, MemoryDb, Path, Read, SnapshotWalk, State, StatePart, StateTree, WalkError},
    crate::light::{LightFinality, LightParams, LightUpdate},
    crate::registry::tokens::STANDARD_TOKEN_FUNCTIONS,
    crate::registry::{Accounts, RegistryError},
    crate::structs::address::Address,
    crate::structs::block::{Block, BlockHeader, Seal, SealedHeader},
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

mod addresses;
mod events;
mod execute;
pub mod fees;
mod finality;
mod index;
pub mod migrate;
mod parallel;
mod receipt;

pub use addresses::{IndexedTx, stored_address_txs, touched};
pub use events::{EventFilter, IndexedEvent, stored_events};
pub use index::{IndexKind, IndexWindow, Indexing, KeepFrom, stored_index};
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
pub use crate::structs::receipt::{CommittedOutcome, CommittedReceipt, receipts_root};
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
    note = "1: receipts carry traces, and the chain keeps its leaf blocks in the database. 2: authority finality's approvals of a signer change record the epoch they were made in. 3: the token registry. 4: contracts record their storage layout. 5: layouts declare record keys and checked kinds. 6: headers commit to a state root and a receipts root, and the state tree is kept. 7: every header is kept on its own, state tree leaves name their entries, and checkpoints keep what light clients need. 8: tx_root is a Merkle root, and the address index has a column of its own. From 8 on, each new format comes with a migration in migrate::MIGRATIONS, from the one before and back.",
)]
pub const STORAGE_FORMAT: u32 = 8;

const TIP_KEY: &[u8] = b"tip";
const TIPS_KEY: &[u8] = b"tips";
const FORMAT_KEY: &[u8] = b"format";
/// Where a node that synced from a snapshot keeps which block it took state at.
const SNAPSHOT_KEY: &[u8] = b"snapshot";

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
        summary = "The genesis block. Its tx_root is the hash of this configuration and its proof_root the hash of its light_params, its state_root the root of the state it starts with, and its producer the authority's address, or the zero address under proof of work.",
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
                proof_root: Hashable::hash(&self.light_params(), self.hash),
                receipts_root: receipts_root(self.hash, &[]),
                state_root: self.state_root(),
                gas_used: 0,
                base_price: self.fees.as_ref().map_or(u256::ZERO, |fees| fees.pricing.build().genesis_price()),
            },
            txs: Vec::new(),
            proofs: Vec::new(),
            seal: Seal::None,
        }
    }

    #[document(
        parent = "Genesis",
        summary = "What a light client needs to follow the chain: its hash, consensus, and how its finality works. The genesis block's proof_root commits to them.",
        see_also = "LightParams, LightClient",
    )]
    pub fn light_params(&self) -> LightParams {
        let finality = self.finality.as_ref().map(|config| match *config {
            #[cfg(feature = "finality-ffg")]
            FinalityConfig::Ffg(ref ffg) => LightFinality::Ffg {
                checkpoint_interval: ffg.checkpoint_interval,
                fixed_ahead: ffg.activation_delay,
            },
            #[cfg(feature = "finality-authority")]
            FinalityConfig::Authority(ref authority) => LightFinality::Authority {
                checkpoint_interval: authority.checkpoint_interval,
                fixed_ahead: authority.activation_delay,
            },
            #[cfg(feature = "finality-depth")]
            FinalityConfig::Depth(ref depth) => LightFinality::Depth { depth: depth.depth },
        });

        LightParams {
            hash: self.hash,
            consensus: self.consensus.clone(),
            finality,
        }
    }

    /// Writes the state the chain starts from: the allocations, and what its finality starts with.
    fn write_state(&self, state: &State) -> Result<(), ChainError> {
        let accounts = Accounts::new(state);

        for allocation in &self.allocations {
            accounts
                .credit(&allocation.address, allocation.amount)
                .map_err(|err| ChainError::BadGenesis(err.to_string()))?;
        }

        if let Some(config) = &self.finality {
            config.write_genesis(state)?;
        }

        Ok(())
    }

    /// The root of the state the chain starts from. A genesis whose state cannot be written has
    /// none, and Chain::open refuses it for that.
    fn state_root(&self) -> Hash {
        let state = State::new(Arc::new(MemoryDb::default()));

        match self.write_state(&state) {
            Ok(()) => StateTree::new(&state, self.hash).follow().expect("a memory database never fails"),
            Err(_) => Hash::zero(self.hash),
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
    #[document(summary = "Its receipts_root is not the root of what its transactions did.")]
    ReceiptsRoot,
    #[document(summary = "Its state_root is not the state tree's root once it has run.")]
    StateRoot { expected: Hash, got: Hash },
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
            BlockError::ReceiptsRoot => write!(f, "receipts_root does not match what its transactions did"),
            BlockError::StateRoot { expected, got } => write!(f, "state_root {got}, but running it reaches {expected}"),
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
    /// How finality works, as light clients see it, for the light updates blocks keep.
    light: Option<LightFinality>,
    /// The threads blocks run on, shared with finality to check votes.
    workers: Arc<Workers>,
    /// The event index and the address index, when this node keeps them.
    events: Option<IndexWindow>,
    addresses: Option<IndexWindow>,
    /// The height this node took state at, when it synced from a snapshot: it has headers only
    /// below it, and never rolls back past it.
    snapshot: Option<u64>,
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

                // A step stopped part way leaves a database the format does not describe.
                if let Some(step) = migrate::under_way(&*db)? {
                    return Err(ChainError::Migrating { from: step.from, to: step.to });
                }

                if found != Some(STORAGE_FORMAT) {
                    return Err(ChainError::OldFormat {
                        found,
                        expected: STORAGE_FORMAT,
                    });
                }
            }
            None => {
                let state = State::new(db.clone());
                genesis.write_state(&state)?;
                let root = StateTree::new(&state, genesis.hash).follow()?;
                debug_assert_eq!(root, block.header.state_root, "genesis's state is the one its block commits to");

                if let Some(finality) = genesis.light_params().finality
                    && let Some(update) = LightUpdate::of(&state, genesis.hash, &finality, id, 0)?
                {
                    state.put_value(Column::Light, id_key(&id), &update);
                }

                state.put_value(Column::Blocks, id_key(&id), &block);
                state.put_value(Column::Headers, id_key(&id), &block.sealed_header());
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
        let events = index::stored_index(&*db, IndexKind::Events)?;
        let addresses = index::stored_index(&*db, IndexKind::Addresses)?;
        let snapshot = read_value::<SnapshotRecord>(&*db, Column::Meta, SNAPSHOT_KEY)?.map(|record| record.height);
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
            light: genesis.light_params().finality,
            workers,
            events,
            addresses,
            snapshot,
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
        summary = "Keeps one of the indexes, and sets which blocks it keeps. A new index starts empty at the next block, and catch_up_index fills it in; whatever an index dropped half-way left behind is cleared first. Once on, it stays on until drop_index.",
        params = {
            kind = "Which index.",
            keep = "Which blocks to keep; None keeps what the database keeps, or every block for a new index.",
        },
        returns = "The index's window.",
        see_also = "Chain::catch_up_index, stored_events, stored_address_txs",
    )]
    pub fn keep_index(&mut self, kind: IndexKind, keep: Option<KeepFrom>) -> Result<IndexWindow, DbError> {
        let tip = self.tip.height;
        let window = match (self.index_window(kind), keep) {
            (Some(window), None) => return Ok(window),
            (Some(window), Some(keep)) if window.keep == keep => return Ok(window),
            (Some(window), Some(keep)) => IndexWindow { keep, ..window },
            (None, keep) => {
                let keep = keep.unwrap_or(KeepFrom::Height(0));

                IndexWindow {
                    from: keep.lowest(tip).max(tip + 1),
                    keep,
                }
            }
        };
        let state = State::new(self.db.clone());
        state.put_value(Column::Meta, kind.low_end_key().to_vec(), &index::IndexedFrom { height: window.from });
        state.put_value(Column::Meta, kind.keep_key().to_vec(), &window.keep);

        match self.index_window(kind) {
            Some(_) => self.db.commit(state.take_changes())?,
            None => self.db.clear(kind.column(), state.take_changes())?,
        }

        *self.window(kind) = Some(window);

        Ok(window)
    }

    #[document(
        parent = "Chain",
        summary = "Stops keeping one of the indexes, and deletes it and its records in Meta in one atomic write, quick however big it is.",
        params = { kind = "Which index." },
        returns = "Whether there was an index to drop.",
        see_also = "Chain::keep_index",
    )]
    pub fn drop_index(&mut self, kind: IndexKind) -> Result<bool, DbError> {
        let state = State::new(self.db.clone());
        state.delete(Column::Meta, kind.low_end_key().to_vec());
        state.delete(Column::Meta, kind.keep_key().to_vec());
        self.db.clear(kind.column(), state.take_changes())?;

        Ok(self.window(kind).take().is_some())
    }

    #[document(
        parent = "Chain",
        summary = "Moves an index's low end a step toward what it keeps: indexing older blocks from their receipts, newest first, or dropping the oldest blocks' entries. A step reads at most a few hundred blocks and commits once, so the index never has gaps, and a catch-up cut short picks up where it stopped. A node that synced from a snapshot has no receipts up to its pivot, so its indexes reach back no further than the block after it.",
        params = { kind = "Which index." },
        returns = "Whether there is more catching up to do.",
        see_also = "Chain::keep_index",
    )]
    pub fn catch_up_index(&mut self, kind: IndexKind) -> Result<bool, DbError> {
        let Some(window) = self.index_window(kind) else {
            return Ok(false);
        };

        let floor = self.snapshot.map_or(0, |pivot| pivot + 1);
        let lowest = window.keep.lowest(self.tip.height).max(floor);

        if window.from == lowest {
            return Ok(false);
        }

        let window = IndexWindow {
            from: index::catch_up(&self.db, self.hash, kind, window, self.tip.height, floor)?,
            ..window
        };
        *self.window(kind) = Some(window);

        Ok(window.from != lowest)
    }

    #[document(parent = "Chain", summary = "What one of the indexes holds and keeps, when this node keeps it.")]
    pub fn index_window(&self, kind: IndexKind) -> Option<IndexWindow> {
        match kind {
            IndexKind::Events => self.events,
            IndexKind::Addresses => self.addresses,
        }
    }

    fn window(&mut self, kind: IndexKind) -> &mut Option<IndexWindow> {
        match kind {
            IndexKind::Events => &mut self.events,
            IndexKind::Addresses => &mut self.addresses,
        }
    }

    #[document(parent = "Chain", summary = "The threads the chain runs blocks and checks signatures on, for others to share.")]
    pub fn workers(&self) -> Arc<Workers> {
        self.workers.clone()
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
        let layer = Arc::new(State::new(self.db.clone()));

        self.build_in(&layer, &self.tip, txs, proofs, producer, timestamp)
    }

    #[document(
        parent = "Chain",
        summary = "The next block on any stored block, unsealed, as build makes one on the tip. It is built in the state at the end of parent, worked out on a layer that is thrown away: the main chain rolled back to where parent's branch splits off, and the branch run up to it.",
        params = {
            parent = "The block to build on: the tip, or one on a side branch no deeper than max_reorg_depth.",
            txs = "The transactions, in order.",
            proofs = "Finality proofs to carry.",
            producer = "Who produces it.",
            timestamp = "When it is made.",
        },
        returns = "The block; UnknownParent when parent is not stored, Invalid when its branch does not run.",
    )]
    pub fn build_on(
        &self,
        parent: &Hash,
        txs: Vec<SignedTx>,
        proofs: Vec<Justification>,
        producer: Address,
        timestamp: u64,
    ) -> Result<Block, ImportError> {
        let header = self.block(parent)?.ok_or(ImportError::UnknownParent(*parent))?.header;
        let layer = self.state_at(parent)?;

        Ok(self.build_in(&layer, &header, txs, proofs, producer, timestamp)?)
    }

    /// Builds the next block on parent in layer, which holds the state at the end of parent. The
    /// transactions run there, to keep only those that can run and fit and learn the gas they
    /// use, and so do the reward and the proofs, to learn the roots; the layer is thrown away.
    fn build_in(
        &self,
        layer: &Arc<State>,
        parent: &BlockHeader,
        txs: Vec<SignedTx>,
        proofs: Vec<Justification>,
        producer: Address,
        timestamp: u64,
    ) -> Result<Block, DbError> {
        let context = BlockContext {
            height: parent.height + 1,
            producer,
            base_price: self.base_price_after(parent),
        };
        let ran = match self.runner().run(layer, &txs, &context, Unfit::Skip) {
            Ok(ran) => ran,
            Err(ImportError::Db(err)) => return Err(err),
            Err(err) => unreachable!("building skips what does not fit, so only the database fails it: {err}"),
        };
        let gas_used = ran.gas_used;
        let kept: Vec<SignedTx> = ran.kept.into_iter().map(|index| txs[index].clone()).collect();
        let committed: Vec<CommittedReceipt> = ran.receipts.iter().map(CommittedReceipt::of).collect();
        let mut block = Block {
            header: BlockHeader {
                parent: parent.id(),
                height: context.height,
                timestamp,
                producer,
                difficulty: self.consensus.difficulty(parent, timestamp),
                tx_root: Block::tx_root(self.hash, &kept),
                proof_root: Block::proof_root(self.hash, &proofs),
                receipts_root: receipts_root(self.hash, &committed),
                state_root: Hash::zero(self.hash),
                gas_used,
                base_price: context.base_price,
            },
            txs: kept,
            proofs,
            seal: Seal::None,
        };

        // The reward and the proofs change state too, so the root is the one importing reaches.
        // A block whose reward or proofs do not hold is refused for that on import, whatever its
        // root says.
        match self.settle(layer, &block) {
            Ok(()) | Err(ImportError::Invalid(_)) => {}
            Err(ImportError::Db(err)) => return Err(err),
            Err(err) => unreachable!("settling a block only fails it or the database: {err}"),
        }

        block.header.state_root = StateTree::new(layer, self.hash).follow()?;

        Ok(block)
    }

    /// The state at the end of a stored block, in a layer that is thrown away: the main chain
    /// rolled back to where the block's branch splits off, and the branch run up to it.
    fn state_at(&self, id: &Hash) -> Result<Arc<State>, ImportError> {
        let state = Arc::new(State::new(self.db.clone()));
        let mut branch = Vec::new();
        let mut fork = *id;

        while !self.on_main_chain(&fork)? {
            let block = self.block(&fork)?.ok_or_else(|| missing(&fork))?;
            fork = block.header.parent;
            branch.push(block);
        }

        let fork_height = self.block(&fork)?.ok_or_else(|| missing(&fork))?.header.height;

        if self.snapshot.is_some_and(|pivot| fork_height < pivot) {
            return Err(ImportError::Db(DbError::Corrupt("history before the snapshot cannot be rolled back".to_string())));
        }

        for height in (fork_height + 1..=self.tip.height).rev() {
            let undone: Hash = read_value(&*state, Column::Heights, &height.to_be_bytes())?
                .ok_or_else(|| DbError::Corrupt(format!("no main-chain block at height {height}")))?;
            revert(&state, &undone)?;
        }

        for block in branch.iter().rev() {
            self.apply(&state, block)?;
        }

        Ok(state)
    }

    #[document(parent = "Chain", summary = "The database the chain is kept in.")]
    pub fn db(&self) -> &Arc<dyn Database> {
        &self.db
    }

    #[document(
        parent = "Chain",
        summary = "The height this node took its state at, when it synced from a snapshot: it holds only headers below it, and never rolls back past it.",
    )]
    pub fn snapshot(&self) -> Option<u64> {
        self.snapshot
    }

    #[document(parent = "Chain", summary = "Main-chain headers with their seals, from a height on: what a peer syncing from a snapshot checks first.")]
    pub fn headers_from(&self, from: u64, count: u32) -> Result<Vec<SealedHeader>, DbError> {
        let mut headers = Vec::new();

        for height in from..from.saturating_add(u64::from(count)) {
            match stored_header_at(&*self.db, height)? {
                Some(sealed) => headers.push(sealed),
                None => break,
            }
        }

        Ok(headers)
    }

    #[document(
        parent = "Chain",
        summary = "One position of the state tree as it was at a recent main-chain block, for a peer syncing from a snapshot.",
        params = {
            block = "The peer's pivot.",
            depth = "The position's depth.",
            prefix = "Its path bits.",
            limit = "The most entries to send.",
        },
        returns = "The part; None for a block not on the main chain or older than the undo records, or a position the tree has no node at.",
    )]
    pub fn state_part(&self, block: &Hash, depth: u16, prefix: &Path, limit: usize) -> Result<Option<StatePart>, DbError> {
        let Some(state) = stored_state_at(self.db.clone(), block)? else {
            return Ok(None);
        };

        StateTree::new(&state, self.hash).part(depth, prefix, limit)
    }

    #[document(
        parent = "Chain",
        summary = "The next position a snapshot walk wants from peers, given the state this node holds now.",
    )]
    pub fn snapshot_next(&self, walk: &mut SnapshotWalk) -> Result<Option<(u16, Path)>, DbError> {
        walk.next(&StateTree::new(&self.state(), self.hash))
    }

    #[document(
        parent = "Chain",
        summary = "Takes a peer's part of a snapshot into the database, once the walk checked it, and brings the state tree in line, in one commit. Only for a chain that holds genesis alone while it syncs from a snapshot: the state is half the pivot's until the walk is done.",
        returns = "Invalid or Unasked from the walk, for a part not to keep.",
    )]
    pub fn take_snapshot_part(&self, walk: &mut SnapshotWalk, depth: u16, prefix: Path, part: StatePart) -> Result<(), WalkError> {
        let layer = State::new(self.db.clone());
        walk.take(&layer, self.hash, depth, prefix, part)?;
        StateTree::new(&layer, self.hash).follow()?;
        self.db.commit(layer.take_changes())?;

        Ok(())
    }

    #[document(
        parent = "Chain",
        summary = "Makes a block whose state the database already holds, taken from peers, the tip of a chain that held only genesis: stores the headers below it and the block itself, and from then on never rolls back past it. Finality starts over from genesis, and learns again from peers.",
        params = {
            headers = "Every header from height 1 to the block's, checked by consensus.",
            pivot = "The block, whose state root the state in the database must have.",
            work = "Its total work.",
        },
        returns = "BadSnapshot when the chain holds more than genesis, the headers do not end at the block, or the state is not the block's.",
    )]
    pub fn install_snapshot(&mut self, headers: &[SealedHeader], pivot: &Block, work: u256) -> Result<(), ChainError> {
        let bad = |why: &str| Err(ChainError::BadSnapshot(why.to_string()));
        let id = pivot.id();

        if self.tip.height != 0 {
            return bad("this chain holds more than genesis");
        }

        if headers.last().map(SealedHeader::id) != Some(id) || headers.len() as u64 != pivot.header.height {
            return bad("the headers do not lead to the block");
        }

        let state = State::new(self.db.clone());

        if StateTree::new(&state, self.hash).root()? != pivot.header.state_root {
            return bad("the state held is not the block's");
        }

        for sealed in headers {
            state.put_value(Column::Headers, id_key(&sealed.id()), sealed);
            state.put_value(Column::Heights, sealed.header.height.to_be_bytes().to_vec(), &sealed.id());
        }

        state.put_value(Column::Blocks, id_key(&id), pivot);
        state.put_value(Column::Work, id_key(&id), &WorkRecord { total: work });
        state.put_value(Column::Meta, TIP_KEY.to_vec(), &id);
        state.put_value(Column::Meta, TIPS_KEY.to_vec(), &Leaves { blocks: vec![id] });
        state.put_value(Column::Meta, SNAPSHOT_KEY.to_vec(), &SnapshotRecord {
            height: pivot.header.height,
            block: id,
        });
        self.db.commit(state.take_changes())?;
        log::info!("[chain] installed a snapshot at height {} {id}", pivot.header.height);

        self.tip = pivot.header.clone();
        self.tip_work = work;
        self.tips = HashSet::from([id]);
        self.snapshot = Some(pivot.header.height);
        self.finality = match &self.finality_config {
            Some(config) => Some(config.build(&self.view())?),
            None => None,
        };
        self.tip_moved()?;

        Ok(())
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
        state.put_value(Column::Headers, id_key(&id), &block.sealed_header());
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

        if self.snapshot.is_some_and(|pivot| fork_height < pivot) {
            log::warn!(
                "[chain] not following a branch to {} that splits off below the snapshot this node took state at",
                new_tip.id()
            );

            return Ok(Imported::Side);
        }

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

    /// What a block does to state past its transactions: the producer's reward, and what its
    /// finality proofs pay and record.
    fn settle(&self, layer: &Arc<State>, block: &Block) -> Result<(), ImportError> {
        let reward = self.reward.at(block.header.height);

        if reward > u256::ZERO {
            Accounts::new(layer)
                .credit(&block.header.producer, reward)
                .map_err(|err| match err {
                    RegistryError::Db(err) => ImportError::Db(err),
                    _ => ImportError::Invalid(BlockError::Reward),
                })?;
        }

        match &self.finality {
            Some(finality) => finality.apply(layer, block),
            None if !block.proofs.is_empty() => Err(ImportError::Invalid(BlockError::Proof("this chain has no finality".to_string()))),
            None => Ok(()),
        }
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

        self.settle(&layer, block)?;

        let committed: Vec<CommittedReceipt> = receipts.iter().map(CommittedReceipt::of).collect();

        if receipts_root(self.hash, &committed) != block.header.receipts_root {
            return Err(ImportError::Invalid(BlockError::ReceiptsRoot));
        }

        let state_root = StateTree::new(&layer, self.hash).follow()?;

        if state_root != block.header.state_root {
            return Err(ImportError::Invalid(BlockError::StateRoot {
                expected: state_root,
                got: block.header.state_root,
            }));
        }

        if let Some(finality) = &self.light
            && let Some(update) = LightUpdate::of(&layer, self.hash, finality, id, height)?
        {
            layer.put_value(Column::Light, id_key(&id), &update);
        }

        for (index, (tx, receipt)) in block.txs.iter().zip(&receipts).enumerate() {
            let record = TxReceipt::new(id, index as u32, receipt);
            let tx_id = tx.id(self.hash);
            layer.put_value(Column::Receipts, id_key(&tx_id), &record);

            // Blocks below an index's low end wait for catch_up_index, or were dropped.
            for kind in IndexKind::ALL {
                if self.index_window(kind).is_some_and(|window| height >= window.from) {
                    let landed = index::Landed {
                        block: id,
                        height,
                        at: index as u32,
                        tx: tx_id,
                    };
                    kind.index(&layer, landed, tx, &record);
                }
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

/// The block a node that synced from a snapshot took its state at.
#[derive(Clone, Copy, Debug, SchemaWrite, SchemaRead)]
struct SnapshotRecord {
    height: u64,
    block: Hash,
}

impl Hashable for SnapshotRecord {}

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
    #[document(summary = "The database is kept in another layout than STORAGE_FORMAT, which no migration this node knows reaches: found is the one it is in, None for one from before formats were recorded. A newer one goes back with the newer chainv4's db migrate.")]
    OldFormat { found: Option<u32>, expected: u32 },
    #[document(summary = "A migration from one format to another stopped part way, and the database is in neither until it finishes.")]
    Migrating { from: u32, to: u32 },
    #[document(summary = "A snapshot could not be installed, and why.")]
    BadSnapshot(String),
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
            ChainError::BadSnapshot(why) => write!(f, "cannot install the snapshot: {why}"),
            ChainError::OldFormat { found: Some(found), expected } if found > expected => write!(
                f,
                "the database is kept in format {found}, newer than this node's {expected}: take it back with `chainv4 db migrate --to {expected}`, run by the chainv4 that wrote it, or delete it or use another --db"
            ),
            ChainError::OldFormat { found, expected } => {
                let found = found.map_or("an older one".to_string(), |found| format!("format {found}"));

                write!(f, "the database is kept in {found}, not format {expected}, and no migration reaches it; delete it or use another --db")
            }
            ChainError::Migrating { from, to } => write!(
                f,
                "a migration of the database from format {from} to {to} stopped part way: finish it with `chainv4 db migrate`, or start the node to go on to its own format"
            ),
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
    summary = "A block's header and seal by id, read straight from a database: there for every block the node stored, and for those before a snapshot it synced from, whose bodies it never had.",
    category = "Chain",
)]
pub fn stored_header(db: &dyn Read, id: &Hash) -> Result<Option<SealedHeader>, DbError> {
    read_value(db, Column::Headers, &id_key(id))
}

#[document(
    summary = "The state at the end of a recent main-chain block, read straight from a database into a layer that is thrown away: every block above it rolled back by its undo record. For proofs against an older block's state root.",
    category = "Chain",
    returns = "The layer; None for a block not on the main chain, or older than the undo records, which reach back max_reorg_depth blocks.",
)]
pub fn stored_state_at(db: Arc<dyn Read>, id: &Hash) -> Result<Option<Arc<State>>, DbError> {
    let Some(sealed) = stored_header(&*db, id)? else {
        return Ok(None);
    };
    let height = sealed.header.height;

    if read_value::<Hash>(&*db, Column::Heights, &height.to_be_bytes())? != Some(*id) {
        return Ok(None);
    }

    let Some(tip) = read_value::<Hash>(&*db, Column::Meta, TIP_KEY)? else {
        return Ok(None);
    };
    let Some(top) = stored_header(&*db, &tip)?.map(|sealed| sealed.header.height) else {
        return Ok(None);
    };
    let state = Arc::new(State::new(db));

    for above in (height + 1..=top).rev() {
        let Some(undone) = read_value::<Hash>(&*state, Column::Heights, &above.to_be_bytes())? else {
            return Ok(None);
        };

        if read_value::<Undo>(&*state, Column::Undo, &id_key(&undone))?.is_none() {
            return Ok(None);
        }

        revert(&state, &undone)?;
    }

    Ok(Some(state))
}

#[document(
    summary = "What a block keeps for light clients, read straight from a database: for genesis and checkpoints, proofs of the voters they fixed. None for other blocks, and for those before a snapshot this node synced from.",
    category = "Chain",
    see_also = "LightUpdate",
)]
pub fn stored_light_update(db: &dyn Read, block: &Hash) -> Result<Option<LightUpdate>, DbError> {
    read_value(db, Column::Light, &id_key(block))
}

#[document(
    summary = "The main-chain block's header and seal at a height, read straight from a database.",
    category = "Chain",
)]
pub fn stored_header_at(db: &dyn Read, height: u64) -> Result<Option<SealedHeader>, DbError> {
    let Some(id) = read_value::<Hash>(db, Column::Heights, &height.to_be_bytes())? else {
        return Ok(None);
    };

    stored_header(db, &id)
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
