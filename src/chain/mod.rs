//! The chain this node follows: genesis, every block it accepted, and which branch is the main
//! chain: the one with the most total work among those holding the latest justified checkpoint,
//! never leaving the finalized one (see finality/). A block that extends the tip is run and
//! committed at once. One on a side branch is stored without running; if its branch comes to weigh more than
//! the main chain, the main chain is rolled back to where they meet, using the undo record each
//! block left, and the branch is run on top, all in one commit. A crash never leaves state from
//! a block the main chain does not have.

use {
    crate::consensus::{Consensus, ConsensusConfig, ConsensusError, PowConfig},
    crate::governance::GovernanceConfig,
    crate::registry::changes::Changes,
    crate::crypto::hashable::Hashable,
    crate::crypto::keypair::{Keypair, PublicKey},
    crate::db::{Column, Database, DbError, MemoryDb, Path, Read, SnapshotWalk, State, StatePart, StateTree, WalkError, history_key, history_list_key, kept_in_history, replaced},
    crate::light::{CheckpointStart, FinalityStep, LightFinality, LightParams, LightUpdate},
    crate::registry::tokens::STANDARD_TOKEN_FUNCTIONS,
    crate::registry::validators::Member,
    crate::registry::{Accounts, RegistryError},
    crate::structs::address::Address,
    crate::structs::block::{Block, BlockHeader, PAYOUT_PARTS, PoolCommitment, Seal, SealedHeader, parts_of},
    crate::structs::code::Engine,
    crate::structs::hash::{Hash, HashType},
    crate::structs::tx::SignedTx,
    crate::structs::u256::{U256Le, u256},
    crate::structs::vote::Checkpoint,
    crate::structs::vote::Justification,
    crate::vm::{EnginePools, Limits, Vm},
    r#macro::document,
    std::collections::{BTreeSet, HashSet, VecDeque},
    std::fmt,
    std::sync::{Arc, Mutex},
    std::time::Instant,
    wincode::{SchemaRead, SchemaWrite},
};

mod addresses;
mod events;
mod execute;
pub mod fees;
mod finality;
pub(crate) use finality::switch;
mod index;
mod members;
#[cfg(feature = "json")]
pub mod json;
pub mod migrate;
mod parallel;
mod range;
mod receipt;

pub use addresses::{AddressScan, IndexedTx, address_txs_from, scan_address_txs, stored_address_txs, touched};
pub use events::{EventFilter, IndexedEvent, stored_events};
pub use index::{IndexKind, IndexWindow, Indexing, KeepFrom, stored_index};
pub use members::{stored_member_index, stored_multisigs_of};
pub use range::{RangeLimits, RangeProblem, stored_block_range};
pub use execute::{BlockContext, Effect, Failure, GovernanceRules, Receipt, Rules, SignerRules, StakingRules, TxError};
pub use fees::{FeeConfig, Fees};
pub use crate::params::{BlockLimits, MAX_BLOCK_BYTES, MIN_BLOCK_BYTES, Reward, reward_problem};
pub use crate::params::fees::VoteRewardSource;
pub use parallel::Workers;
use parallel::{Runner, Unfit};

#[document(
    summary = "The contracts a genesis may name in preinstalls, each written into its first state: deployment_proxy, the deterministic deployment proxy, which CREATE2s its call's data after a 32-byte salt, at the address, deployer and code other EVM chains have it with.",
    category = "Chain",
    see_also = "Genesis::preinstalls",
)]
pub const PREINSTALLS: &[&str] = &["deployment_proxy"];

/// The deterministic deployment proxy, which CREATE2s its call's data after a 32-byte salt: where
/// other EVM chains have it, who deployed it there, and its code.
const PROXY: [u8; 20] = [0x4e, 0x59, 0xb4, 0x48, 0x47, 0xb3, 0x79, 0x57, 0x85, 0x88, 0x92, 0x0c, 0xa7, 0x8f, 0xbf, 0x26, 0xc0, 0xb4, 0x95, 0x6c];
const PROXY_DEPLOYER: [u8; 20] = [0x3f, 0xab, 0x18, 0x46, 0x22, 0xdc, 0x19, 0xb6, 0x10, 0x93, 0x49, 0xb9, 0x48, 0x11, 0x49, 0x3b, 0xf2, 0xa4, 0x53, 0x62];
const PROXY_CODE: &str = "0x7fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffe03601600081602082378035828234f58015156039578182fd5b8082525050506014600cf3";
#[cfg(feature = "finality-authority")]
pub use finality::AuthorityConfig;
#[cfg(feature = "finality-depth")]
pub use finality::DepthConfig;
pub use finality::{
    Evidence, Finality, FinalityConfig, FinalityError, FinalityUpdate, View, VoteBounds, stored_evidence, stored_finality,
    stored_justification,
};
#[cfg(feature = "finality-ffg")]
pub use finality::{FfgConfig, GenesisValidator, Leak};
pub use crate::structs::receipt::{CommittedOutcome, CommittedReceipt, receipts_root};
pub use receipt::{LegacyTxReceipt, TxReceipt, TxStatus};

/// The development chains' transaction limits: what one deploy or contract call may use.
pub const DEV_TX_LIMITS: Limits = Limits {
    fuel: 10_000_000,
    memory: 32 * 1024 * 1024,
};

/// The development chains' reorg limit.
pub const DEV_MAX_REORG_DEPTH: u64 = 100;

#[document(
    summary = "The development proof-of-work chain's difficulty window, 11 blocks: Bitcoin's median-time window, and the blocks difficulty retargets over.",
    category = "Chain",
    see_also = "Genesis::pow_window",
)]
pub const DEV_POW_WINDOW: u64 = 11;

#[document(
    summary = "The development chains' max_block_bytes, 4 MiB: about 14,000 transfers, more than their gas limit lets in, so gas binds first for transfers and bytes for big deploys.",
    category = "Chain",
    see_also = "Genesis::max_block_bytes",
)]
pub const DEV_MAX_BLOCK_BYTES: u64 = 4 * 1024 * 1024;

#[document(
    summary = "The layout this node keeps its database in. It goes up whenever a stored record changes shape, so a node never reads a database it would misread.",
    category = "Chain",
    see_also = "ChainError::OldFormat",
    note = "1: receipts carry traces, and the chain keeps its leaf blocks in the database. 2: authority finality's approvals of a signer change record the epoch they were made in. 3: the token registry. 4: contracts record their storage layout. 5: layouts declare record keys and checked kinds. 6: headers commit to a state root and a receipts root, and the state tree is kept. 7: every header is kept on its own, state tree leaves name their entries, and checkpoints keep what light clients need. 8: tx_root is a Merkle root, and the address index has a column of its own. From 8 on, each new format comes with a migration in migrate::MIGRATIONS, from the one before and back, but for 9. 9: blocks may carry a mining pool's commitment, which their headers commit to, and the pools column keeps the claims on them; that changes every block's id, so a database in 8 holds a chain this build cannot follow, and none migrates: start a new chain.",
)]
pub const STORAGE_FORMAT: u32 = 9;

const TIP_KEY: &[u8] = b"tip";
const TIPS_KEY: &[u8] = b"tips";
const FORMAT_KEY: &[u8] = b"format";
/// Where a node that synced from a snapshot keeps which block it took state at.
const SNAPSHOT_KEY: &[u8] = b"snapshot";
/// Where a pruning node keeps the lowest height whose block body it still holds.
const BODIES_FROM_KEY: &[u8] = b"bodies_from";
/// How many blocks one step of pruning goes through, at most, in one commit.
const PRUNE_STEP: u64 = 256;
/// Where the chain keeps the blocks that failed to run, so a restart refuses them again.
const BAD_KEY: &[u8] = b"bad";
/// Where the chain keeps the lowest height whose state its history still holds.
const HISTORY_FROM_KEY: &[u8] = b"history-from";

#[document(
    summary = "How many blocks back a node keeps state it can read and prove at, unless told otherwise: about 5.5 hours of 2-second blocks. What --state-history sets; --archive keeps every height.",
    category = "Storage",
    see_also = "Chain::keep_state_history",
)]
pub const DEFAULT_STATE_HISTORY: u64 = 10_000;

#[document(
    summary = "How many blocks that failed to run a node remembers across restarts, newest kept, unless --tune chain.bad-blocks says otherwise: 1,024. A branch past that is run again before it is refused, as every one was before the memory outlived a restart.",
    category = "Chain",
    see_also = "Chain::import, BlockError::Bad, ChainOptions::bad_blocks",
)]
pub const MAX_BAD_BLOCKS: usize = 1_024;

#[document(
    summary = "What a node's operator sizes of its chain: the engines' pools and caches, and how many bad blocks, waiting votes and pieces of evidence it remembers. None of it is consensus: it changes how fast and with how much memory a node gets to the same state, never which. Chain::open_with takes it; Chain::open takes DEFAULT.",
    category = "Chain",
    see_also = "Chain::open_with, EnginePools, VoteBounds",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChainOptions {
    #[document(summary = "Lua's pool of states and WebAssembly's module cache: --lua-states, --lua-builders, --wasm-cache-mb and --wasm-cache-modules.")]
    pub pools: EnginePools,
    #[document(summary = "How many blocks that failed to run it remembers: MAX_BAD_BLOCKS by default.")]
    pub bad_blocks: usize,
    #[document(summary = "How many votes waiting and pieces of evidence finality holds.")]
    pub votes: VoteBounds,
}

impl ChainOptions {
    #[document(parent = "ChainOptions", summary = "What every node had before these were sized on the node.")]
    pub const DEFAULT: ChainOptions = ChainOptions { pools: EnginePools::DEFAULT, bad_blocks: MAX_BAD_BLOCKS, votes: VoteBounds::DEFAULT };
}

impl Default for ChainOptions {
    fn default() -> ChainOptions {
        ChainOptions::DEFAULT
    }
}

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

impl Reward {
    #[document(parent = "Reward", summary = "No reward at all.")]
    pub const NONE: Reward = Reward {
        initial: u256::ZERO,
        halving_interval: None,
    };

    #[document(parent = "Reward", summary = "The reward for the block at a height.")]
    pub fn at(&self, height: u64) -> u256 {
        self.since(height, 0)
    }

    #[document(
        parent = "Reward",
        summary = "The reward for the block at a height, for a reward that took effect at another: governance's, whose halvings count from the block after the one that decided it.",
        params = { height = "The block's height.", from = "The height it took effect at: initial there." },
    )]
    pub fn since(&self, height: u64, from: u64) -> u256 {
        let halvings = match self.halving_interval {
            Some(interval) if interval > 0 => height.saturating_sub(from) / interval,
            _ => 0,
        };

        if halvings >= 256 {
            return u256::ZERO;
        }

        self.initial >> halvings as u32
    }
}

#[document(
    summary = "The block reward as it stood at each height: genesis's, then the one governance decided last, from the height it took effect at. What the API shows each block minted.",
    category = "Chain",
    see_also = "Reward, DecidedReward",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RewardSchedule {
    #[document(summary = "Genesis's.")]
    pub genesis: Reward,
    #[document(summary = "Governance's last, with where it took effect.")]
    pub decided: Option<crate::registry::changes::DecidedReward>,
}

impl RewardSchedule {
    #[document(parent = "RewardSchedule", summary = "The reward for the block at a height.")]
    pub fn at(&self, height: u64) -> u256 {
        match &self.decided {
            Some(decided) if height >= decided.from => decided.reward.since(height, decided.from),
            _ => self.genesis.at(height),
        }
    }
}

#[document(
    summary = "Everything a chain starts from. Nodes with different genesis are on different chains: the genesis block commits to all of this.",
    category = "Chain",
    see_also = "Chain::open, ConsensusConfig",
)]
// Hashed for its id and read from JSON, never decoded: so it encodes, but does not decode, and
// fields added since chains launched can encode as nothing while unset.
#[derive(Clone, Debug, PartialEq, SchemaWrite)]
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
    #[document(summary = "The one address allowed to send Mint transactions, if any: the development key's, or None. Mint is a faucet for development chains; a chain of value names nobody, and gets new coins from block rewards alone.")]
    pub minter: Option<Address>,
    #[document(summary = "Who holds what from the start.")]
    pub allocations: Vec<Allocation>,
    #[document(summary = "The engines contracts may be deployed for. A node built without one of them refuses to open the chain rather than fall out of step with it.")]
    pub engines: Vec<Engine>,
    #[document(summary = "What one deploy, upgrade or contract call may use, called contracts included.")]
    pub tx_limits: Limits,
    #[document(summary = "The functions that make a contract a token: one that exports them all is in the token registry. Usually STANDARD_TOKEN_FUNCTIONS, ERC-20's required six; empty keeps no registry.")]
    pub token_functions: Vec<String>,
    #[document(summary = "How far back the main chain may be rolled back by work. A branch that splits off deeper is never followed for its work alone, however much it holds; one holding a justified checkpoint is, down to the finalized one. Undo records older than this, and on a chain with finality below the finalized checkpoint, are dropped.")]
    pub max_reorg_depth: u64,
    #[document(summary = "Finality: its kind and every parameter of it, or None for a chain where only work decides.")]
    pub finality: Option<FinalityConfig>,
    #[document(summary = "What transactions pay for gas, and the block gas limit, or None for a chain where transactions are free and blocks unbounded.")]
    pub fees: Option<FeeConfig>,
    #[document(summary = "The most bytes a block's transactions may take, encoded, from MIN_BLOCK_BYTES to MAX_BLOCK_BYTES: a consensus rule, so a block past it is refused on import. None bounds blocks by gas alone, or on a chain without fees, by nothing. Chains made before it keep their genesis id, since unset it adds nothing to genesis's encoding.")]
    #[wincode(with = "Unset")]
    pub max_block_bytes: Option<u64>,
    #[document(summary = "Under proof of work, the window, in blocks, that difficulty retargets over and timestamps are bounded by: a block's difficulty is the window's work over the time it took, scaled to the target, and its timestamp must be after the median of the window's. At least 2; None keeps the rule that looks at the parent alone. Genesis files write it as consensus.window. Chains made before it keep their genesis id, since unset it adds nothing to genesis's encoding, which is why it sits here, last, and not in PowConfig.")]
    #[wincode(with = "Unset")]
    pub pow_window: Option<u64>,
    #[document(summary = "Where vote rewards come from: minted, as when None, or from the reward pool, REWARD_POOL, which the burned share of every fee then feeds instead of vanishing. Added after chains launched: None adds nothing to genesis's encoding, so chains from before keep their ids.")]
    #[wincode(with = "UnsetSource")]
    pub vote_reward_source: Option<VoteRewardSource>,
    #[document(summary = "How the chain changes what genesis set, or None for a chain where nothing does: which kind of governance approves a change, and its settings. Needs finality that votes. Added after chains launched: None adds nothing to genesis's encoding, so chains from before keep their ids.")]
    #[wincode(with = "UnsetGovernance")]
    pub governance: Option<GovernanceConfig>,
    #[document(summary = "Where nodes of the chain find their first peers: libp2p addresses, such as /ip4/203.0.113.7/udp/4001/quic-v1, at most 32, which every node dials while it has fewer peers than it wants. Fixed with the chain's id: under governance, Change::Bootnodes replaces them on a live chain, and nodes add their own with --bootnode. Added after chains launched: empty adds nothing to genesis's encoding, so chains from before keep their ids.")]
    #[wincode(with = "crate::structs::unset::UnsetList")]
    pub bootnodes: Vec<String>,
    #[document(summary = "The name registry: names such as alice.dev, their suffix, their price a year by length, their grace period and the most years ahead they are held; or None for a chain without names, where name transactions fail. Under governance, Change::NamePrices replaces the prices. Added after chains launched: None adds nothing to genesis's encoding, so chains from before keep their ids.")]
    #[wincode(with = "crate::structs::unset::UnsetOption<crate::registry::names::NamesConfig>")]
    pub names: Option<crate::registry::names::NamesConfig>,
    #[document(summary = "What makes a contract a collection: the NFT and multi-token standards' functions, usually CollectionFunctions::standard; or None for a chain that keeps no collection registry until governance decides a Change::Collections. Added after chains launched: None adds nothing to genesis's encoding, so chains from before keep their ids.")]
    #[wincode(with = "crate::structs::unset::UnsetOption<crate::registry::collections::CollectionFunctions>")]
    pub collections: Option<crate::registry::collections::CollectionFunctions>,
    #[document(summary = "The bridges' federation from the start, version 0, or None for a chain that has none until governance decides a Change::Federation. Added after chains launched: None adds nothing to genesis's encoding, so chains from before keep their ids.")]
    #[wincode(with = "crate::structs::unset::UnsetOption<crate::registry::federation::Federation>")]
    pub federation: Option<crate::registry::federation::Federation>,
    #[document(summary = "The contracts the chain announces from the start, its directory: the bridges' gateways, the swap contract, the DEX and the NFT factory; or None for a chain that announces none until governance decides a Change::Directory. Added after chains launched: None adds nothing to genesis's encoding, so chains from before keep their ids.")]
    #[wincode(with = "crate::structs::unset::UnsetOption<crate::registry::directory::Directory>")]
    pub directory: Option<crate::registry::directory::Directory>,    #[document(summary = "The chain's name, such as chainv4 testnet, as the apps show it wherever they name the network: 1 to 40 characters, no control characters, no space at either end. None leaves the apps to call it chainv4. Under governance, Change::Name renames it. Added after chains launched: None adds nothing to genesis's encoding, so chains from before keep their ids.")]
    #[wincode(with = "crate::structs::unset::UnsetOption<String>")]
    pub name: Option<String>,
    #[document(summary = "Who sets the directory on a chain without governance: an address that may send Tx::SetDirectory, and hand the role on or renounce it with Tx::HandDirectory. None for none: a chain with governance changes its directory by Change::Directory alone, and one with neither keeps genesis's for good. Added after chains launched: None adds nothing to genesis's encoding, so chains from before keep their ids.")]
    #[wincode(with = "crate::structs::unset::UnsetOption<Address>")]
    pub directory_admin: Option<Address>,
    #[document(summary = "The limits a contract keeps within, or None for ContractLimits::DEFAULT's. Under governance, Change::Limits replaces them. Added after chains launched: None adds nothing to genesis's encoding.")]
    #[wincode(with = "crate::structs::unset::UnsetOption<crate::params::vm::ContractLimits>")]
    pub contract_limits: Option<crate::params::vm::ContractLimits>,
    #[document(summary = "The most bytes one transaction may take, encoded, Ethereum-signed ones' raw bytes, or None: native ones bounded by the block alone, Ethereum-signed ones by ETHEREUM_TX_BYTES. Under governance, Change::Limits replaces it. Added after chains launched: None adds nothing to genesis's encoding.")]
    #[wincode(with = "crate::structs::unset::UnsetOption<u64>")]
    pub max_tx_bytes: Option<u64>,
    #[document(summary = "The most members a native multisig may have, at most MAX_MEMBERS, or None for MULTISIG_MEMBERS. Under governance, Change::Limits replaces it. Added after chains launched: None adds nothing to genesis's encoding.")]
    #[wincode(with = "crate::structs::unset::UnsetOption<u32>")]
    pub multisig_members: Option<u32>,
    #[document(summary = "The EVM's spec from the first block, or None for Prague. Under governance, Change::EvmSpec moves to another from a height. Added after chains launched: None adds nothing to genesis's encoding.")]
    #[wincode(with = "crate::structs::unset::UnsetOption<crate::params::vm::EvmSpec>")]
    pub evm_spec: Option<crate::params::vm::EvmSpec>,
    #[document(summary = "The DEX's settings, its fee, or None for DexSettings::DEFAULT's 0.3%. Under governance, Change::Dex replaces them. Added after chains launched: None adds nothing to genesis's encoding.")]
    #[wincode(with = "crate::structs::unset::UnsetOption<crate::params::vm::DexSettings>")]
    pub dex: Option<crate::params::vm::DexSettings>,
    #[document(summary = "Anchoring's settings, or None for everything off. Under governance, Change::Anchoring replaces them. Added after chains launched: None adds nothing to genesis's encoding.")]
    #[wincode(with = "crate::structs::unset::UnsetOption<crate::params::anchoring::AnchorSettings>")]
    pub anchoring: Option<crate::params::anchoring::AnchorSettings>,
    #[document(summary = "Settings no vote may change, by path: a change's name, such as reward, alone or with the dotted names of fields as change_json writes its value, such as anchoring.enforce or limits.tx_limits.fuel, each once. Under governance, Change::Locks adds more and nothing takes one away; a chain without governance names none. A chain that locks upgrade or consensus needs a new genesis for any fix to them. Added after chains launched: empty adds nothing to genesis's encoding.")]
    #[wincode(with = "crate::structs::unset::UnsetMarkedList")]
    pub locked: Vec<String>,
    #[document(summary = "Contracts the chain starts with, by name, from PREINSTALLS: deployment_proxy, the deterministic deployment proxy at 0x4e59…956c, where other EVM chains have it, so what is deployed through it, MultiSig.sol and Swap.sol, lands at the same address here. Each needs the EVM among engines; chainv4 genesis names it for a chain that runs the EVM. Added after chains launched: empty adds nothing to genesis's encoding, and a file without it starts with none.")]
    #[wincode(with = "crate::structs::unset::UnsetMarkedList")]
    pub preinstalls: Vec<String>,
    #[document(summary = "The forks it schedules from the start, each this release knows, at a height or an epoch's first block, and how far ahead governance must schedule others. Under governance, Change::Fork adds, moves and cancels them. Added after chains launched: none adds nothing to genesis's encoding.")]
    #[wincode(with = "UnsetForks")]
    pub forks: crate::params::forks::ForkSchedule,
}

impl Hashable for Genesis {}

/// Encodes Genesis::forks as nothing while it schedules none and sets no lead, and as a 2 and its
/// encoding otherwise: a marker no earlier unset field writes, so no genesis from before the field
/// hashes as one with it. Genesis is never decoded.
pub struct UnsetForks;

// SAFETY: size_of is exactly what write writes: nothing for an empty schedule, a byte and the
// encoding for any other.
unsafe impl<C: wincode::config::Config> SchemaWrite<C> for UnsetForks {
    type Src = crate::params::forks::ForkSchedule;

    fn size_of(src: &crate::params::forks::ForkSchedule) -> wincode::WriteResult<usize> {
        match *src == crate::params::forks::ForkSchedule::default() {
            true => Ok(0),
            false => Ok(1 + <crate::params::forks::ForkSchedule as SchemaWrite<C>>::size_of(src)?),
        }
    }

    fn write(mut writer: impl wincode::io::Writer, src: &crate::params::forks::ForkSchedule) -> wincode::WriteResult<()> {
        if *src == crate::params::forks::ForkSchedule::default() {
            return Ok(());
        }
        <[u8; 1] as SchemaWrite<C>>::write(&mut writer, &[2])?;

        <crate::params::forks::ForkSchedule as SchemaWrite<C>>::write(writer, src)
    }
}

/// Encodes Genesis::governance as nothing while None, and as a 1 and its encoding once set, so a
/// genesis from before the field hashes as it did. Genesis is never decoded.
pub struct UnsetGovernance;

// SAFETY: size_of is exactly what write writes: nothing for None, a byte and the encoding for Some.
unsafe impl<C: wincode::config::Config> SchemaWrite<C> for UnsetGovernance {
    type Src = Option<GovernanceConfig>;

    fn size_of(src: &Option<GovernanceConfig>) -> wincode::WriteResult<usize> {
        match src {
            Some(config) => Ok(1 + <GovernanceConfig as SchemaWrite<C>>::size_of(config)?),
            None => Ok(0),
        }
    }

    fn write(mut writer: impl wincode::io::Writer, src: &Option<GovernanceConfig>) -> wincode::WriteResult<()> {
        let Some(config) = src else {
            return Ok(());
        };
        <[u8; 1] as SchemaWrite<C>>::write(&mut writer, &[1])?;

        <GovernanceConfig as SchemaWrite<C>>::write(writer, config)
    }
}

#[document(
    summary = "Why a consensus cannot run with a finality: a rotating authority needs authority finality, whose signers produce, and production by stake needs FFG, whose validators do. What genesis checks, and what a change approved under governance must pass too.",
    category = "Chain",
    see_also = "Genesis, Change::Consensus",
)]
#[cfg_attr(not(any(feature = "finality-ffg", feature = "finality-authority")), allow(unused_variables))]
pub fn consensus_problem(consensus: &ConsensusConfig, finality: Option<&FinalityConfig>) -> Option<&'static str> {
    if let ConsensusConfig::ProofOfWork(pow) = consensus
        && !pow.hash.secure()
    {
        return Some("the proof-of-work hash must be a secure 256-bit one, not xxh3");
    }

    if let ConsensusConfig::ProofOfWork(pow) = consensus {
        if pow.window.is_some_and(|window| window < 2) {
            return Some("consensus.window is at least 2 blocks");
        }

        if let Some(problem) = pow.tuning.as_ref().and_then(crate::consensus::PowTuning::problem) {
            return Some(problem);
        }
    }

    if matches!(consensus, ConsensusConfig::Rotating { .. }) {
        #[cfg(feature = "finality-authority")]
        let rotating_ok = matches!(finality, Some(FinalityConfig::Authority(_)));
        #[cfg(not(feature = "finality-authority"))]
        let rotating_ok = false;

        if !rotating_ok {
            return Some("consensus rotating needs finality of type authority, whose signers produce the blocks");
        }
    }

    if matches!(consensus, ConsensusConfig::Staked { .. }) {
        #[cfg(feature = "finality-ffg")]
        let staked_ok = matches!(finality, Some(FinalityConfig::Ffg(_)));
        #[cfg(not(feature = "finality-ffg"))]
        let staked_ok = false;

        if !staked_ok {
            return Some("consensus staked needs finality of type ffg, whose validators produce the blocks");
        }
    }

    None
}

/// Encodes Genesis::vote_reward_source as nothing while None, and as a 1 and its tag once set, so
/// a genesis from before the field hashes as it did. Only as a last field; see Unset.
pub struct UnsetSource;

// SAFETY: size_of is exactly what write writes: nothing for None, two bytes for Some.
unsafe impl<C: wincode::config::ConfigCore> SchemaWrite<C> for UnsetSource {
    type Src = Option<VoteRewardSource>;

    fn size_of(src: &Option<VoteRewardSource>) -> wincode::WriteResult<usize> {
        Ok(if src.is_some() { 2 } else { 0 })
    }

    fn write(writer: impl wincode::io::Writer, src: &Option<VoteRewardSource>) -> wincode::WriteResult<()> {
        let Some(source) = src else {
            return Ok(());
        };
        let tag = match source {
            VoteRewardSource::Mint => 0,
            VoteRewardSource::Fees => 1,
        };

        <[u8; 2] as SchemaWrite<C>>::write(writer, &[1, tag])
    }
}

#[document(
    summary = "The account the burned share of fees goes to, and vote rewards are paid from, on a chain whose vote_reward_source is fees. A contract address no contract lives at, so nothing can spend it but the chain itself.",
    category = "Chain",
    see_also = "VoteRewardSource",
)]
pub const REWARD_POOL: Address = Address::Contract([0; 32]);

pub use crate::structs::unset::Unset;

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

#[document(
    summary = "What running a transaction now would do, as dry_run found.",
    category = "Chain",
    see_also = "dry_run",
)]
#[derive(Clone, Debug, PartialEq)]
pub struct DryRun {
    #[document(summary = "The gas it used, intrinsic cost included; 0 on a chain without fees.")]
    pub gas_used: u64,
    #[document(summary = "The fuel its contracts burned.")]
    pub fuel_used: u64,
    #[document(summary = "Why it would fail, or could not run at all; None when it would go through.")]
    pub failure: Option<String>,
    #[document(summary = "What a contract call would return, or the address a deploy would make: how a read-only call learns what a contract says, without a transaction.")]
    pub effect: Option<Effect>,
}

#[document(
    summary = "Runs a transaction as the next block would, on a throwaway layer over the committed state, to see what gas it uses and whether it would fail; nothing it does is kept. It runs at its sender's next nonce whatever its own says, with the gas limit given at the base price, so a probe signed with any offer will do. Its signature is not checked, nor, for a multisig's own, whether its members back it.",
    category = "Chain",
    params = {
        db = "The node's database.",
        rules = "The chain's rules, Genesis::rules.",
        vm = "The chain's engines.",
        probe = "The transaction.",
        gas_limit = "What it may use: the most a real one would get, such as the fuel cap's worth.",
        base_price = "The next block's base price.",
    },
    returns = "What it did, or the database failure that kept it from running.",
    see_also = "DryRun, Fees::estimated",
)]
pub fn dry_run(db: Arc<dyn crate::db::Committed>, rules: &Rules, vm: &Arc<Vm>, probe: &SignedTx, gas_limit: u64, base_price: u256) -> Result<DryRun, DbError> {
    let layer = Arc::new(State::new(db.clone()));
    let mut tx = probe.clone();
    tx.nonce = Accounts::new(&layer).get(&tx.sender())?.nonce;
    tx.fee = crate::structs::tx::FeeOffer { gas_limit, max_price: base_price, tip: u256::ZERO };
    let tip = stored_tip(&*db)?;
    let height = tip.as_ref().map_or(0, |tip| tip.header.height) + 1;
    let timestamp = tip.as_ref().map_or(0, |tip| tip.header.timestamp) + 1;
    let genesis = stored_header_at(&*db, 0)?.map_or_else(|| Hash::zero(rules.hash), |genesis| genesis.id());
    let context = BlockContext { height, timestamp, producer: Address::Ed25519([0; 32]), base_price, genesis };

    // As its signature, so a multisig's members' backing: a dry run sizes their transaction before
    // they sign it.
    match execute::execute_authorized(&layer, &tx, rules, vm, &context) {
        Ok(executed) => Ok(DryRun {
            gas_used: executed.receipt.gas_used,
            fuel_used: executed.receipt.fuel_used,
            failure: executed.receipt.result.as_ref().err().map(|failure| failure.to_string()),
            effect: executed.receipt.result.ok(),
        }),
        Err(execute::ExecuteError::Invalid(err)) => Ok(DryRun { gas_used: 0, fuel_used: 0, failure: Some(err.to_string()), effect: None }),
        Err(execute::ExecuteError::Db(err)) => Err(err),
    }
}

#[document(
    summary = "How many bytes a transaction takes in a block: its encoding, what max_block_bytes counts.",
    category = "Chain",
    see_also = "Genesis::max_block_bytes",
)]
pub fn tx_bytes(tx: &SignedTx) -> u64 {
    wincode::serialized_size(tx).expect("a signed transaction always encodes")
}

/// What a block's transactions take, encoded.
fn txs_bytes(txs: &[SignedTx]) -> u64 {
    txs.iter().map(tx_bytes).sum()
}

/// The dev chains' names, in a build with them.
/// Why a genesis's preinstalls cannot be: a name PREINSTALLS lacks, one named twice, or any without
/// the EVM, which every one is code for.
fn preinstalls_problem(preinstalls: &[String], engines: &[Engine]) -> Option<&'static str> {
    if preinstalls.iter().any(|name| !PREINSTALLS.contains(&name.as_str())) {
        return Some("preinstalls names only deployment_proxy");
    }

    if preinstalls.iter().enumerate().any(|(at, name)| preinstalls[..at].contains(name)) {
        return Some("preinstalls names each once");
    }

    (!preinstalls.is_empty() && !engines.contains(&Engine::Evm)).then_some("preinstalls are EVM contracts: the chain's engines must include the EVM")
}

/// The development chains' preinstalls: the deployment proxy, in a build with the EVM.
fn dev_preinstalls() -> Vec<String> {
    match Engine::compiled().contains(&Engine::Evm) {
        true => vec!["deployment_proxy".to_string()],
        false => Vec::new(),
    }
}

fn dev_names() -> Option<crate::registry::names::NamesConfig> {
    cfg!(feature = "names").then(crate::registry::names::NamesConfig::dev)
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
            max_block_bytes: Some(DEV_MAX_BLOCK_BYTES),
            pow_window: None,
            vote_reward_source: None,
            governance: None,
            bootnodes: Vec::new(),
            names: dev_names(),
            collections: Some(crate::registry::collections::CollectionFunctions::standard()),
            federation: None,
            directory: None,
            name: None,
            directory_admin: None,
            contract_limits: None,
            dex: None,
            anchoring: None,
            evm_spec: None,
            max_tx_bytes: None,
            multisig_members: None,
            locked: Vec::new(),
            preinstalls: dev_preinstalls(),
            forks: Default::default(),
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
                tuning: None,
                window: None,
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
            max_block_bytes: Some(DEV_MAX_BLOCK_BYTES),
            pow_window: Some(DEV_POW_WINDOW),
            vote_reward_source: None,
            governance: None,
            bootnodes: Vec::new(),
            names: dev_names(),
            collections: Some(crate::registry::collections::CollectionFunctions::standard()),
            federation: None,
            directory: None,
            name: None,
            directory_admin: None,
            contract_limits: None,
            dex: None,
            anchoring: None,
            evm_spec: None,
            max_tx_bytes: None,
            multisig_members: None,
            locked: Vec::new(),
            preinstalls: dev_preinstalls(),
            forks: Default::default(),
        }
    }

    #[document(
        parent = "Genesis",
        summary = "A development chain whose authority finality signers produce the blocks in turn, a block every two seconds: the development key is its one signer, so it produces every block, and votes. Otherwise as dev_authority.",
        params = { signer = "The one signer, usually DevAuthority::dev_keypair." },
    )]
    #[cfg(feature = "finality-authority")]
    pub fn dev_rotating(signer: PublicKey) -> Genesis {
        Genesis {
            consensus: ConsensusConfig::Rotating { slot_ms: 2_000 },
            finality: Some(FinalityConfig::Authority(AuthorityConfig::dev(signer))),
            ..Genesis::dev_authority(signer)
        }
    }

    #[document(
        parent = "Genesis",
        summary = "A development chain whose FFG validators produce the blocks by stake, a block every two seconds, picked by a weighted draw: the development key is its one validator, bonded from genesis, so it produces every block, and votes. Otherwise as dev_authority.",
        params = { validator = "The one validator, usually DevAuthority::dev_keypair, which both owns the bond and votes." },
    )]
    #[cfg(feature = "finality-ffg")]
    pub fn dev_staked(validator: PublicKey) -> Genesis {
        Genesis {
            consensus: ConsensusConfig::Staked {
                slot_ms: 2_000,
                schedule: crate::consensus::Schedule::Draw,
            },
            ..Genesis::dev_authority(validator)
        }
    }

    #[document(
        parent = "Genesis",
        summary = "The consensus rules this chain's blocks are checked against: its consensus with its proof-of-work window, if any, drawing with its hash and starting each priority rotation at its finality's epochs.",
    )]
    pub fn consensus_rules(&self) -> Box<dyn Consensus> {
        self.consensus.rules(self.hash, self.pow_window, self.finality.as_ref().map(FinalityConfig::checkpoint_interval))
    }

    #[document(
        parent = "Genesis",
        summary = "The genesis block. Its tx_root is the hash of this configuration and its proof_root the hash of its light_params, its state_root the root of the state it starts with, and its producer the authority's address, or the zero address under proof of work.",
    )]
    pub fn block(&self) -> Block {
        let producer = match &self.consensus {
            ConsensusConfig::Authority { key, .. } => key.address(),
            ConsensusConfig::ProofOfWork(_) | ConsensusConfig::Rotating { .. } | ConsensusConfig::Staked { .. } => Address::Ed25519([0; 32]),
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
                pool_root: Hash::zero(self.hash),
                receipts_root: receipts_root(self.hash, &[]),
                state_root: self.state_root(),
                gas_used: 0,
                base_price: self.fees.as_ref().map_or(u256::ZERO, |fees| fees.pricing.build().genesis_price()),
            },
            txs: Vec::new(),
            proofs: Vec::new(),
            pool: None,
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
            pow_window: self.pow_window,
            governance: self.governance.as_ref().map(GovernanceConfig::kind),
        }
    }

    /// Writes the state the chain starts from: the allocations, and what its finality starts with.
    fn write_state(&self, state: &State) -> Result<(), ChainError> {
        let accounts = Accounts::new(state);

        for allocation in &self.allocations {
            accounts
                .credit(&allocation.address, allocation.amount)
                .map_err(|err| ChainError::BadGenesis(err.to_string()))?;
            crate::registry::supply::Supply::mint(state, allocation.amount)?;
        }

        if let Some(config) = &self.finality {
            config.write_genesis(state)?;
        }

        if self.governance.is_some() {
            crate::registry::changes::Changes::new(state).write_genesis(&self.consensus, self.fixed_ahead())?;
        }

        // In the state, so a light client proves the federation and the directory in force from
        // either source.
        if let Some(federation) = &self.federation {
            crate::registry::changes::Changes::new(state).write_genesis_federation(federation, self.timestamp);
        }

        if let Some(directory) = &self.directory {
            crate::registry::changes::Changes::new(state).write_directory(directory);
        }

        if let Some(name) = &self.name {
            crate::registry::changes::Changes::new(state).write_name(name);
        }

        // Written only when it schedules any, so a genesis without forks starts from the state it did.
        if self.forks != crate::params::forks::ForkSchedule::default() {
            crate::registry::changes::Changes::new(state).write_forks(&self.forks);
        }

        // Written only when there are any, so a genesis without locks starts from the state it did.
        if self.governance.is_some() && !self.locked.is_empty() {
            crate::registry::changes::Changes::new(state).add_locks(&self.locked)?;
        }

        if self.directory_admin.is_some() {
            crate::registry::directory::write_directory_admin(state, self.directory_admin);
        }

        // The deterministic deployment proxy where other EVM chains have it, when genesis names it,
        // so what is deployed through it lands here where it lands there.
        if self.preinstalls.iter().any(|name| name == "deployment_proxy") {
            let record = crate::registry::contracts::ContractRecord {
                deployer: Address::Evm(PROXY_DEPLOYER),
                engine: Engine::Evm,
                code: crate::structs::hex::from_hex(PROXY_CODE).expect("the proxy's code is hex"),
                functions: Vec::new(),
                reentrant: Vec::new(),
                layout: Default::default(),
            };
            state.put_value(Column::Contracts, crate::db::address_key(&Address::Evm(PROXY)), &record);
        }

        Ok(())
    }

    #[document(
        parent = "Genesis",
        summary = "How many epochs ahead finality fixes each epoch's voters, and governance the consensus: activation_delay under the kinds that vote, 0 otherwise.",
    )]
    pub fn fixed_ahead(&self) -> u64 {
        self.light_params().finality.and_then(|finality| finality.fixed_ahead()).unwrap_or(0)
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

        if let Some(problem) = consensus_problem(&self.consensus, self.finality.as_ref()) {
            return Some(problem);
        }

        if let Some(governance) = &self.governance {
            if !self.finality.as_ref().is_some_and(FinalityConfig::votes) {
                return Some("governance needs finality that votes, whose light updates carry its decisions to light clients");
            }

            if let Some(problem) = governance.problem() {
                return Some(problem);
            }
        }

        let mut token_functions: Vec<&String> = self.token_functions.iter().collect();
        token_functions.sort();

        if token_functions.iter().any(|name| name.is_empty()) || token_functions.windows(2).any(|pair| pair[0] == pair[1]) {
            return Some("token_functions must name each function once");
        }

        if let Some(problem) = self.collections.as_ref().and_then(crate::registry::collections::CollectionFunctions::problem) {
            return Some(problem);
        }

        if let Some(problem) = self.federation.as_ref().and_then(crate::registry::federation::Federation::problem) {
            return Some(problem);
        }

        if let Some(problem) = self.directory.as_ref().and_then(crate::registry::directory::Directory::problem) {
            return Some(problem);
        }

        if self.max_block_bytes.is_some_and(|bytes| bytes < MIN_BLOCK_BYTES) {
            return Some("max_block_bytes must be at least 1 MiB, 1048576, so a block holds the largest deploy");
        }

        if self.max_block_bytes.is_some_and(|bytes| bytes > MAX_BLOCK_BYTES) {
            return Some("max_block_bytes must be at most 6 MiB, 6291456, so a full block fits one gossip message");
        }

        if self.minter.is_some_and(|minter| minter != Keypair::development().public().address()) {
            return Some("minter must be the development key, or null: Mint is for development chains alone");
        }

        if self.vote_reward_source == Some(VoteRewardSource::Fees) && self.fees.is_none() {
            return Some("vote_reward_source fees needs fees to fill the pool");
        }

        if self.pow_window.is_some_and(|window| window < 2) {
            return Some("consensus.window must be at least 2 blocks, one interval to measure");
        }

        if let Some(problem) = crate::governance::bootnodes_problem(&self.bootnodes) {
            return Some(problem);
        }

        if let Some(problem) = self.name.as_deref().and_then(crate::governance::chain_name_problem) {
            return Some(problem);
        }

        if let Some(problem) = preinstalls_problem(&self.preinstalls, &self.engines) {
            return Some(problem);
        }

        if self.forks.genesis_problem().is_some() {
            return Some("forks names a fork this release does not know, a name that cannot be one, or one twice");
        }

        if self.forks.forks.iter().any(|fork| fork.epoch.is_some() && self.finality.is_none()) {
            return Some("a fork at an epoch needs a chain with finality");
        }

        if !self.locked.is_empty() && self.governance.is_none() {
            return Some("locked holds settings against governance: a chain without governance changes nothing, so it locks nothing");
        }

        if self.directory_admin.is_some() && self.governance.is_some() {
            return Some("a chain with governance changes its directory by governance: leave directory_admin out");
        }

        if matches!(self.directory_admin, Some(Address::Contract(_))) {
            return Some("directory_admin is an account that signs, not a contract");
        }

        if self.names.is_some() && !cfg!(feature = "names") {
            return Some("names needs a build with the names feature");
        }

        if let Some(problem) = self.names.as_ref().and_then(crate::registry::names::NamesConfig::problem) {
            return Some(problem);
        }

        if let Some(problem) = self.block_limits().problem() {
            return Some(problem);
        }

        if let Some(problem) = self.dex.as_ref().and_then(crate::params::vm::DexSettings::problem) {
            return Some(problem);
        }

        if let Some(problem) = self.anchoring.as_ref().and_then(crate::params::anchoring::AnchorSettings::problem) {
            return Some(problem);
        }

        if let Some(problem) = self.fees.as_ref().and_then(|fees| fees.costs.as_ref()).and_then(crate::params::vm::HostCosts::problem) {
            return Some(problem);
        }

        if self.pow_window.is_some() && !matches!(self.consensus, ConsensusConfig::ProofOfWork(_)) {
            return Some("consensus.window is for proof of work alone");
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
            standards: crate::registry::Standards {
                tokens: self.token_functions.clone().into(),
                collections: self.collections.clone(),
            },
            staking: self.finality.as_ref().and_then(|config| config.staking(self.block().id())),
            signers: self.finality.as_ref().and_then(|config| config.signer_rules(self.block().id())),
            governance: self
                .governance
                .as_ref()
                .zip(self.finality.as_ref())
                .map(|(governance, finality)| GovernanceRules::new(governance, finality.checkpoint_interval(), finality, Arc::new(self.governance_settings()))),
            fees: self.fees.as_ref().map(|config| Arc::new(config.build())),
            pooled_fees: self.vote_reward_source == Some(VoteRewardSource::Fees),
            names: self.names.clone().map(Arc::new),
            vm: self.vm_rules(),
            anchoring: self.anchoring.clone().unwrap_or_default(),
            target_block_ms: Some(self.consensus.block_ms()),
        }
    }

    #[document(parent = "Genesis", summary = "What contracts run under from the first block: genesis's limits, costs, Lua costs, EVM spec and DEX fee, or the defaults it leaves unset.")]
    pub fn vm_rules(&self) -> crate::params::vm::VmRules {
        crate::params::vm::VmRules {
            costs: self.fees.as_ref().and_then(|fees| fees.costs).unwrap_or_default(),
            lua_costs: self.fees.as_ref().and_then(|fees| fees.lua_costs.as_deref().copied()).unwrap_or_default(),
            limits: self.contract_limits.unwrap_or_default(),
            engines: crate::params::vm::EngineSet::of(&self.engines),
            evm_spec: self.evm_spec.unwrap_or_default(),
            dex: self.dex.unwrap_or_default(),
            max_tx_bytes: self.max_tx_bytes,
            multisig_members: self.multisig_members.unwrap_or(crate::params::MULTISIG_MEMBERS),
            forks: self.forks.at(0),
        }
    }

    #[document(parent = "Genesis", summary = "What it sets of each setting governance changes, as Changes::in_force falls back to while nothing was decided.")]
    pub fn governance_settings(&self) -> crate::registry::changes::GenesisSettings {
        crate::registry::changes::GenesisSettings {
            consensus: self.consensus.clone(),
            bootnodes: self.bootnodes.clone(),
            names: self.names.as_ref().map(crate::registry::names::NamesConfig::settings),
            token_functions: self.token_functions.clone(),
            collections: self.collections.clone(),
            evm_spec: self.evm_spec.unwrap_or_default(),
            dex: self.dex.unwrap_or_default(),
            anchoring: self.anchoring.clone().unwrap_or_default(),
            federation: self.federation.clone(),
            fees: self.fees.clone(),
            reward: self.reward,
            limits: self.block_limits(),
            engines: self.engines.clone(),
        }
    }

    #[document(parent = "Genesis", summary = "Its limits as a change of limits carries them, whole.")]
    pub fn block_limits(&self) -> BlockLimits {
        BlockLimits {
            tx_limits: self.tx_limits,
            max_block_bytes: self.max_block_bytes,
            contract: self.contract_limits,
            max_tx_bytes: self.max_tx_bytes,
            multisig_members: self.multisig_members,
        }
    }

    #[document(parent = "Genesis", summary = "Where vote rewards come from: mint unless genesis says fees.")]
    pub fn vote_rewards(&self) -> VoteRewardSource {
        self.vote_reward_source.unwrap_or_default()
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
    #[document(summary = "The chain scheduled a fork at or below the block's height that this release does not know, by its name: this node cannot run it as the others do, and stops until updated. Nothing is said of the block.")]
    UnknownFork(String),
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
            ImportError::UnknownFork(name) => write!(f, "the chain runs fork {name} from here, which this release does not know: update chainv4"),
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
    #[document(summary = "It is on a branch that leaves a checkpoint anchored in Bitcoin while anchoring was enforced.")]
    ConflictsWithAnchor,
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
    #[document(summary = "Its transactions take more bytes, encoded, than genesis's max_block_bytes.")]
    TooBig { bytes: u64, limit: u64 },
    #[document(summary = "Its pool_root does not match its pool commitment.")]
    PoolRoot,
    #[document(summary = "Its pool commitment sends more than all of the reward to the escrow, or its producer no longer holds the fees it sends.")]
    Pool(String),
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
            BlockError::ConflictsWithAnchor => write!(f, "conflicts with a checkpoint anchored in Bitcoin"),
            BlockError::BasePrice { expected, got } => write!(f, "base price {got}, but its parent makes it {expected}"),
            BlockError::GasUsed { expected, got } => write!(f, "gas_used {got}, but its transactions used {expected}"),
            BlockError::Fee(why) => write!(f, "paying the producer its fees: {why}"),
            BlockError::ReceiptsRoot => write!(f, "receipts_root does not match what its transactions did"),
            BlockError::StateRoot { expected, got } => write!(f, "state_root {got}, but running it reaches {expected}"),
            BlockError::TooBig { bytes, limit } => write!(f, "its transactions take {bytes} bytes, past the {limit} a block may hold"),
            BlockError::PoolRoot => write!(f, "pool_root does not match"),
            BlockError::Pool(why) => write!(f, "its pool commitment: {why}"),
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
    /// Genesis's consensus rules: every block's, on a chain without governance.
    consensus: Arc<dyn Consensus>,
    /// Whether governance may change the consensus, so each epoch's rules come from state.
    governed: bool,
    /// The rules built for each consensus governance fixed, by its configuration.
    rules_cache: Mutex<Vec<(ConsensusConfig, Arc<dyn Consensus>)>>,
    /// What rules are built with: the proof-of-work window and the finality's epochs.
    pow_window: Option<u64>,
    /// How many epochs ahead each epoch's consensus is fixed, under governance.
    fixed_ahead: u64,
    /// Genesis's bootnodes, which stand until governance decides others.
    bootnodes: Vec<String>,
    reward: Reward,
    genesis: Hash,
    tip: BlockHeader,
    tip_work: u256,
    max_reorg_depth: u64,
    /// The most bytes a block's transactions may take, encoded, if genesis bounds them.
    max_block_bytes: Option<u64>,
    /// Blocks that failed to run, and so their descendants too, the last MAX_BAD_BLOCKS of them
    /// kept in Meta so a restart refuses them without running them again. The queue says which
    /// is oldest.
    bad: HashSet<Hash>,
    bad_order: VecDeque<Hash>,
    /// How many bad blocks it remembers, MAX_BAD_BLOCKS unless its operator said otherwise.
    most_bad: usize,
    /// How many votes waiting and pieces of evidence finality holds.
    vote_bounds: VoteBounds,
    /// Stored blocks no stored block builds on yet, which a justification may move the chain to.
    /// Ordered, so they are kept, and GET /forks lists them, the same way every time.
    tips: BTreeSet<Hash>,
    finality_config: Option<FinalityConfig>,
    finality: Option<Box<dyn Finality>>,
    /// How finality works, as light clients see it, for the light updates blocks keep.
    light: Option<LightFinality>,
    /// Where vote rewards come from, for finality rebuilt after a snapshot.
    vote_rewards: VoteRewardSource,
    /// The chain as light clients see it, for checkpoint starts.
    light_params: LightParams,
    /// The threads blocks run on, shared with finality to check votes.
    workers: Arc<Workers>,
    /// The event index and the address index, when this node keeps them.
    events: Option<IndexWindow>,
    addresses: Option<IndexWindow>,
    /// Whether this node keeps the member index, which follows the multisig registry whole.
    members: bool,
    /// The height this node took state at, when it synced from a snapshot: it has headers only
    /// below it, and never rolls back past it.
    snapshot: Option<u64>,
    /// How many blocks back state is kept to read at, or None to keep every height.
    state_history: Option<u64>,
    /// The lowest height whose body this node still holds, once it pruned: 0 for one that never did.
    bodies_from: u64,
    /// How many blocks below the settled height a pruning node keeps the bodies of, or None for one
    /// that keeps every body: what --prune-blocks sets.
    prune_blocks: Option<u64>,
    /// What the block on the tip runs with, as the tip's state decides: what the node charges
    /// and admits by, and builds the next block with.
    next: BlockParams,
    /// The fees last built from a decided config, so a block does not build them again.
    built_fees: BuiltFees,
}

#[document(
    summary = "The fees last built from a decided config, kept so each block, or each API request, does not build them again.",
    category = "Fees",
    see_also = "built_fees",
)]
#[derive(Default)]
pub struct BuiltFees(Mutex<Option<(FeeConfig, Arc<Fees>)>>);

#[document(summary = "The fees of a decided config: those built last when it is the same, else built now and kept.", category = "Fees", see_also = "BuiltFees")]
pub fn built_fees(cache: &BuiltFees, config: FeeConfig) -> Arc<Fees> {
    let mut built = cache.0.lock().expect("no panic holds it");

    match built.as_ref() {
        Some((kept, fees)) if *kept == config => fees.clone(),
        _ => {
            let fees = Arc::new(config.build());
            *built = Some((config, fees.clone()));

            fees
        }
    }
}

/// What a block runs with: genesis's rules, reward and size, with what governance decided in the
/// state its parent left, finality's settings as they stand at its epoch.
#[derive(Clone)]
pub(crate) struct BlockParams {
    pub(crate) rules: Rules,
    /// The reward, and the height it took effect at.
    pub(crate) reward: (Reward, u64),
    pub(crate) max_block_bytes: Option<u64>,
}

/// The lowest height whose state history holds.
#[derive(Clone, Copy, Debug, SchemaWrite, SchemaRead)]
struct HistoryFrom {
    height: u64,
}

/// The lowest height a pruning node still holds the body of: those below it it pruned.
#[derive(Clone, Copy, Debug, SchemaWrite, SchemaRead)]
struct BodiesFrom {
    height: u64,
}

impl Hashable for BodiesFrom {}

impl Hashable for HistoryFrom {}

/// What a block that split its reward split.
#[derive(Clone, Copy, Debug, SchemaWrite, SchemaRead)]
struct Split {
    #[wincode(with = "U256Le")]
    total: u256,
}

impl Hashable for Split {}

/// The keys a block changed in the state columns and the tree: what pruning its history deletes.
#[derive(Clone, Debug, Default, SchemaWrite, SchemaRead)]
struct HistoryList {
    #[wincode(with = "wincode::containers::Vec<HistoryKey, wincode::len::BincodeLen<{ crate::structs::block::MAX_LIST_MEMORY }>>")]
    keys: Vec<HistoryKey>,
}

impl Hashable for HistoryList {}

#[derive(Clone, Debug, SchemaWrite, SchemaRead)]
struct HistoryKey {
    column: Column,
    key: Vec<u8>,
}

impl Chain {
    #[document(
        parent = "Chain",
        summary = "Opens the chain in a database, writing genesis on first use.",
        returns = "The chain; WrongGenesis when the database holds a different chain, MissingEngine when this node lacks an engine genesis allows.",
    )]
    pub fn open(db: Arc<dyn Database>, genesis: &Genesis) -> Result<Chain, ChainError> {
        Chain::open_with(db, genesis, ChainOptions::DEFAULT)
    }

    #[document(
        parent = "Chain",
        summary = "Opens the chain as open does, with what this node's operator sized: the engines' pools, and how many bad blocks, waiting votes and pieces of evidence it remembers. None of it is consensus.",
        returns = "As open.",
        see_also = "ChainOptions",
    )]
    pub fn open_with(db: Arc<dyn Database>, genesis: &Genesis, options: ChainOptions) -> Result<Chain, ChainError> {
        if let Some(problem) = genesis.problem() {
            return Err(ChainError::BadGenesis(problem.to_string()));
        }

        if let Some(problem) = crate::governance::locks::genesis_problem(&genesis.locked) {
            return Err(ChainError::BadGenesis(problem));
        }

        let block = genesis.block();
        let id = block.id();
        // Every engine genesis runs, which this node must have; and every one it was built with, as
        // governance may turn others on (Change::Engines), the block's rules saying which run.
        if let Some(missing) = genesis.engines.iter().find(|engine| !Engine::compiled().contains(engine)) {
            return Err(ChainError::MissingEngine(*missing));
        }

        let vm = Arc::new(
            Vm::with_pools(&Engine::compiled(), genesis.hash, options.pools)
                .map_err(ChainError::MissingEngine)?
                .with_names(genesis.names.clone())
                .with_federation(genesis.federation.clone())
                .with_anchoring(genesis.anchoring.clone())
                .with_chain_id(crate::crypto::ethereum::chain_id(&id)),
        );
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
                    && let Some(update) = LightUpdate::of(&state, genesis.hash, &finality, genesis.governance.is_some(), id, 0)?
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
                state.put_value(Column::Meta, HISTORY_FROM_KEY.to_vec(), &HistoryFrom { height: 0 });
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
            Some(config) => Some(config.build(&View::new(&db, &tip.header, id, genesis.max_reorg_depth, &workers, options.votes))?),
            None => None,
        };
        let events = index::events_up_to_date(&db, index::stored_index(&*db, IndexKind::Events)?, tip.header.height)?;
        let addresses = index::stored_index(&*db, IndexKind::Addresses)?;
        let members = members::stored_member_index(&*db)?;
        let snapshot = read_value::<SnapshotRecord>(&*db, Column::Meta, SNAPSHOT_KEY)?.map(|record| record.height);
        let bodies_from = read_value::<BodiesFrom>(&*db, Column::Meta, BODIES_FROM_KEY)?.map_or(0, |from| from.height);
        let bad_order: VecDeque<Hash> = read_value::<BadBlocks>(&*db, Column::Meta, BAD_KEY)?.unwrap_or_default().blocks.into();
        // A database from before history begins it at its tip.
        if read_value::<HistoryFrom>(&*db, Column::Meta, HISTORY_FROM_KEY)?.is_none() {
            let state = State::new(db.clone());
            state.put_value(Column::Meta, HISTORY_FROM_KEY.to_vec(), &HistoryFrom { height: tip.header.height });
            db.commit(state.take_changes())?;
        }

        let mut chain = Chain {
            db,
            rules: genesis.rules(),
            hash: genesis.hash,
            vm,
            consensus: Arc::from(genesis.consensus_rules()),
            governed: genesis.governance.is_some(),
            rules_cache: Mutex::new(Vec::new()),
            pow_window: genesis.pow_window,
            fixed_ahead: genesis.fixed_ahead(),
            bootnodes: genesis.bootnodes.clone(),
            reward: genesis.reward,
            genesis: id,
            tip: tip.header,
            tip_work,
            max_reorg_depth: genesis.max_reorg_depth,
            max_block_bytes: genesis.max_block_bytes,
            bad: bad_order.iter().copied().collect(),
            bad_order,
            most_bad: options.bad_blocks.max(1),
            vote_bounds: options.votes,
            tips: tips.into_iter().collect(),
            finality_config: genesis.finality.clone(),
            finality,
            light: genesis.light_params().finality,
            vote_rewards: genesis.vote_rewards(),
            light_params: genesis.light_params(),
            workers,
            events,
            addresses,
            members,
            snapshot,
            state_history: Some(DEFAULT_STATE_HISTORY),
            bodies_from,
            prune_blocks: None,
            next: BlockParams { rules: genesis.rules(), reward: (genesis.reward, 0), max_block_bytes: genesis.max_block_bytes },
            built_fees: BuiltFees::default(),
        };
        chain.next = chain.params_in(&chain.state(), chain.tip.height + 1)?;
        // In case the node stopped between committing a block and catching finality up with it.
        chain.tip_moved()?;

        Ok(chain)
    }

    #[document(
        parent = "Chain",
        summary = "How many blocks back to keep state to read and prove at, or None to keep every height from now on: what --state-history and --archive set. History older than the window goes as the next block comes; a wider window, or an archive, keeps from what is held now.",
        params = { window = "Blocks back from the tip, at least 1; None for every height." },
    )]
    pub fn keep_state_history(&mut self, window: Option<u64>) {
        self.state_history = window.map(|window| window.max(1));
    }

    #[document(
        parent = "Chain",
        summary = "How many blocks below the settled height to keep the bodies of, or None to keep every one: what --prune-blocks sets. Below that, a block's transactions, its receipts and the transaction index go a step at a time (Chain::prune_step), and the event and address indexes follow; headers, seals, justifications and light updates stay. The settled height is the finalized checkpoint's, and never above max_reorg_depth below the tip.",
        params = { keep = "Blocks to keep below the settled height, at least 1; None to stop pruning, which keeps what is left and brings back nothing pruned." },
        see_also = "Chain::prune_step, Chain::bodies_from",
    )]
    pub fn keep_blocks(&mut self, keep: Option<u64>) {
        self.prune_blocks = keep.map(|keep| keep.max(1));
    }

    #[document(
        parent = "Chain",
        summary = "The lowest height whose block body this node holds: 0 for one that holds every one, the block after its pivot for one that synced from a snapshot, or where pruning reached. Below it, it has headers alone.",
        see_also = "stored_bodies_from",
    )]
    pub fn bodies_from(&self) -> u64 {
        self.snapshot.map_or(0, |pivot| pivot + 1).max(self.bodies_from)
    }

    #[document(
        parent = "Chain",
        summary = "Prunes a step toward what keep_blocks keeps: up to 256 main-chain blocks' bodies, their receipts and the transaction index's entries for them, in one commit, from the lowest held on. Genesis's body is never pruned. Nothing at or above the settled height less what is kept goes, so no block a reorg or the finalized checkpoint could need goes either.",
        returns = "Whether there is more to prune.",
        see_also = "Chain::keep_blocks",
    )]
    pub fn prune_step(&mut self) -> Result<bool, DbError> {
        let Some(keep) = self.prune_blocks else {
            return Ok(false);
        };

        let settled = match &self.finality {
            Some(_) => self.finalized_height().unwrap_or(0),
            None => self.tip.height,
        }
        .min(self.tip.height.saturating_sub(self.max_reorg_depth));
        let below = settled.saturating_sub(keep);
        let from = self.bodies_from().max(1);

        if from >= below {
            return Ok(false);
        }

        let to = below.min(from.saturating_add(PRUNE_STEP));
        let state = State::new(self.db.clone());

        for height in from..to {
            let Some(id) = read_value::<Hash>(&*self.db, Column::Heights, &height.to_be_bytes())? else {
                continue;
            };

            if let Some(block) = stored_block(&*self.db, &id)? {
                for tx in &block.txs {
                    state.delete(Column::Receipts, id_key(&tx.id(self.hash)));

                    #[cfg(feature = "ethereum")]
                    if let crate::structs::tx::Tx::Ethereum { raw } = &tx.tx {
                        state.delete(Column::Ethereum, crate::crypto::ethereum::tx_hash(raw).to_vec());
                    }
                }

                state.delete(Column::Blocks, id_key(&id));
            }
        }

        state.put_value(Column::Meta, BODIES_FROM_KEY.to_vec(), &BodiesFrom { height: to });
        self.db.commit(state.take_changes())?;
        self.bodies_from = to;

        Ok(to < below)
    }

    #[document(parent = "Chain", summary = "The lowest height whose state this node can read and prove at: genesis's for one that ran every block and keeps every height, else where history began or the window starts.")]
    pub fn history_from(&self) -> Result<u64, DbError> {
        Ok(read_value::<HistoryFrom>(&*self.db, Column::Meta, HISTORY_FROM_KEY)?.map_or(0, |from| from.height))
    }

    #[document(parent = "Chain", summary = "How far back the main chain may be rolled back, from genesis.")]
    pub fn max_reorg_depth(&self) -> u64 {
        self.max_reorg_depth
    }

    #[document(
        parent = "Chain",
        summary = "Whether a mining pool's block can no longer be undone, for the pool to credit its miners: once it is at or under the finalized checkpoint, or deeper than max_reorg_depth under the tip, it is Final, with what it shared out, on the main chain, and Dropped off it; Pending until then.",
        params = {
            id = "The block.",
            height = "Its height.",
        },
    )]
    pub fn pool_settled(&self, id: &Hash, height: u64) -> Result<crate::consensus::PoolSettled, DbError> {
        use crate::consensus::PoolSettled;

        let finalized = self.finalized().zip(self.finality_config()).map(|(checkpoint, config)| checkpoint.epoch * config.checkpoint_interval());
        let settled = self.tip.height.saturating_sub(height) > self.max_reorg_depth || finalized.is_some_and(|finalized| height <= finalized);

        if !settled {
            return Ok(PoolSettled::Pending);
        }

        match self.on_main_chain(id)? {
            true => Ok(stored_split(&*self.db, id)?.map_or(PoolSettled::Dropped, PoolSettled::Final)),
            false => Ok(PoolSettled::Dropped),
        }
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

        if kind == IndexKind::Events {
            index::mark_events_schema(&state);
        }

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

        if kind == IndexKind::Events {
            index::forget_events_schema(&state);
        }

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

        let floor = self.bodies_from();
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

    #[document(
        parent = "Chain",
        summary = "Keeps the member index: each multisig under the address of every member's key, for GET /accounts/{address}/multisigs. One the database does not keep yet is built whole from the multisig registry at once, in one atomic write, however long the chain has run; from then on each block moves it as it changes a multisig's members. Once on, it stays on until drop_member_index.",
        returns = "How many multisigs it was built from, or None when the database kept it already.",
        see_also = "Chain::drop_member_index, stored_multisigs_of",
    )]
    pub fn keep_member_index(&mut self) -> Result<Option<usize>, DbError> {
        if self.members {
            return Ok(None);
        }

        let built = members::rebuild(&self.db)?;
        self.members = true;

        Ok(Some(built))
    }

    #[document(
        parent = "Chain",
        summary = "Stops keeping the member index, deleting it in one atomic write.",
        returns = "Whether there was one to drop.",
        see_also = "Chain::keep_member_index",
    )]
    pub fn drop_member_index(&mut self) -> Result<bool, DbError> {
        members::forget(&self.db)?;

        Ok(std::mem::take(&mut self.members))
    }

    #[document(parent = "Chain", summary = "Whether this node keeps the member index.")]
    pub fn member_index(&self) -> bool {
        self.members
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

    #[document(parent = "Chain", summary = "What a light client needs to follow this chain, from genesis: what a node syncing from a snapshot checks headers with.")]
    pub fn light_params(&self) -> LightParams {
        self.light_params.clone()
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
        self.build_within(txs, proofs, producer, timestamp, None)
    }

    #[document(
        parent = "Chain",
        summary = "The next block on the tip, as build makes it, within a time budget: once the deadline passes, the transactions that went in by then are the block, and the rest are left out unrun. Without a deadline, every transaction given runs.",
        params = {
            txs = "The transactions, in order.",
            proofs = "Finality proofs to carry.",
            producer = "Who produces it.",
            timestamp = "When it is made.",
            deadline = "When to stop running transactions, or None to run them all.",
        },
        returns = "The block, and whether the deadline cut it short.",
    )]
    pub fn build_within(
        &self,
        txs: Vec<SignedTx>,
        proofs: Vec<Justification>,
        producer: Address,
        timestamp: u64,
        deadline: Option<Instant>,
    ) -> Result<Block, DbError> {
        self.build_declaring(txs, proofs, producer, timestamp, deadline, None, None)
    }

    #[document(
        parent = "Chain",
        summary = "The next block on the tip, as build_within makes it, declaring the difficulty the producer decides when consensus leaves it to the producer, as a rotating authority's offset in the schedule, and carrying a mining pool's commitment.",
        params = {
            difficulty = "The difficulty to declare; None declares what consensus says.",
            pool = "The pool's commitment: what goes to its escrow, and the root of its claims; None for the producer to keep the reward.",
        },
    )]
    #[allow(clippy::too_many_arguments)]
    pub fn build_declaring(
        &self,
        txs: Vec<SignedTx>,
        proofs: Vec<Justification>,
        producer: Address,
        timestamp: u64,
        deadline: Option<Instant>,
        difficulty: Option<u128>,
        pool: Option<PoolCommitment>,
    ) -> Result<Block, DbError> {
        let layer = Arc::new(State::new(self.db.clone()));

        self.build_in(&layer, &self.tip, Draft { txs, proofs, producer, timestamp, deadline, difficulty, pool })
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

        Ok(self.build_in(&layer, &header, Draft { txs, proofs, producer, timestamp, deadline: None, difficulty: None, pool: None })?)
    }

    /// Builds the next block on parent in layer, which holds the state at the end of parent. The
    /// transactions run there, to keep only those that can run and fit and learn the gas they
    /// use, and so do the reward and the proofs, to learn the roots; the layer is thrown away. A
    /// deadline stops the transactions once it passes, and the block holds those that ran by then.
    fn build_in(&self, layer: &Arc<State>, parent: &BlockHeader, draft: Draft) -> Result<Block, DbError> {
        let Draft { txs, proofs, producer, timestamp, deadline, difficulty, pool } = draft;
        let params = self.params_in(layer, parent.height + 1)?;
        let context = BlockContext {
            height: parent.height + 1,
            timestamp,
            producer,
            base_price: base_price_after(params.rules.fees.as_deref(), parent),
            genesis: self.genesis(),
        };
        let txs = Chain::within_bytes(txs, params.max_block_bytes);
        let ran = match self.runner(&params.rules).run(layer, &txs, &context, Unfit::Skip, deadline) {
            Ok(ran) => ran,
            Err(ImportError::Db(err)) => return Err(err),
            Err(err) => unreachable!("building skips what does not fit, so only the database fails it: {err}"),
        };

        if ran.cut {
            log::info!(
                "[chain] the time budget cut block {} short: {} of {} transactions went in",
                context.height,
                ran.kept.len(),
                txs.len()
            );
        }

        let gas_used = ran.gas_used;
        let kept: Vec<SignedTx> = ran.kept.into_iter().map(|index| txs[index].clone()).collect();
        let committed: Vec<CommittedReceipt> = ran.receipts.iter().map(CommittedReceipt::of).collect();
        let mut block = Block {
            header: BlockHeader {
                parent: parent.id(),
                height: context.height,
                timestamp,
                producer,
                difficulty: match difficulty {
                    Some(declared) => declared,
                    None => self.difficulty_after(parent, timestamp)?,
                },
                tx_root: Block::tx_root(self.hash, &kept),
                proof_root: Block::proof_root(self.hash, &proofs),
                pool_root: Block::pool_root(self.hash, pool.as_ref()),
                receipts_root: receipts_root(self.hash, &committed),
                state_root: Hash::zero(self.hash),
                gas_used,
                base_price: context.base_price,
            },
            txs: kept,
            proofs,
            pool,
            seal: Seal::None,
        };

        // The reward and the proofs change state too, so the root is the one importing reaches.
        // A block whose reward or proofs do not hold is refused for that on import, whatever its
        // root says.
        match self.settle(layer, &block, params.reward, ran.to_producer, ran.burned, params.rules.pooled_fees) {
            Ok(()) => {}
            Err(ImportError::Invalid(err)) => log::warn!("[chain] block {} will be refused: {err}", context.height),
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
            revert(&state, &undone, false)?;
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

    #[document(parent = "Chain", summary = "Anchoring's settings in force at the tip: the last decided, or genesis's.")]
    pub fn anchoring(&self) -> Result<crate::params::anchoring::AnchorSettings, DbError> {
        let state = self.state();

        Ok(crate::registry::changes::Changes::new(&state).anchoring()?.unwrap_or_else(|| self.rules.anchoring.clone()))
    }

    #[document(parent = "Chain", summary = "The highest checkpoint an enforced anchor holds on the main chain, by height: no block at or under it replaces the chain's.")]
    pub fn anchored_height(&self) -> Result<Option<u64>, DbError> {
        #[cfg(feature = "anchoring")]
        return crate::anchor::Anchors::new(&self.state()).enforced_height();

        #[cfg(not(feature = "anchoring"))]
        Ok(None)
    }

    #[cfg(feature = "anchoring")]
    #[document(
        parent = "Chain",
        summary = "What a peer checking anchors asks for: every anchor the main chain recorded, the newest's proof and those asked for, where anchoring's Bitcoin headers start and the relay's tip, and where its Ethereum relay started. None while the chain holds no anchors, or anchoring has no relay or start this build reads.",
    )]
    pub fn anchors_answer(&self, asked: &[u64]) -> Result<Option<crate::params::anchoring::AnchorsAnswer>, DbError> {
        let settings = self.anchoring()?;
        let state = self.state();
        #[cfg(feature = "anchoring-btc")]
        let (start, btc_tip) = match (settings.relay, settings.btc_start.clone()) {
            (Some(relay), Some(start)) => {
                let tip = crate::anchor::Relay::new(&state, relay).tip()?.map_or(start.height, |(_, height)| height);

                (Some(start), tip)
            }
            _ => (None, 0),
        };
        #[cfg(not(feature = "anchoring-btc"))]
        let (start, btc_tip) = (None::<crate::params::anchoring::BtcStart>, 0);
        #[cfg(feature = "anchoring-eth")]
        let eth_start = settings.eth_start.clone().filter(|_| settings.eth_relay.is_some());
        #[cfg(not(feature = "anchoring-eth"))]
        let eth_start = {
            let _ = &settings;

            None
        };

        if start.is_none() && eth_start.is_none() {
            return Ok(None);
        }

        let anchors = crate::anchor::Anchors::new(&state);
        let Some(newest) = anchors.count()?.checked_sub(1) else {
            return Ok(None);
        };
        let mut wanted: Vec<u64> = asked.iter().copied().filter(|index| *index < newest).take(crate::net::MAX_SYNC_ANCHOR_PROOFS).collect();
        wanted.push(newest);
        wanted.sort_unstable();
        wanted.dedup();
        let mut proofs = Vec::with_capacity(wanted.len());

        for index in wanted {
            proofs.push((index, anchors.proof(index)?.ok_or_else(|| DbError::Corrupt(format!("anchor {index}'s proof is missing")))?));
        }

        Ok(Some(crate::params::anchoring::AnchorsAnswer { start, btc_tip, eth_start, entries: anchors.entries(0, newest + 1)?, proofs }))
    }

    #[cfg(feature = "anchoring-btc")]
    #[document(parent = "Chain", summary = "Bitcoin headers from anchoring's relay's main chain, from a height on, 80 bytes each: stopping at its tip, or at one it keeps too little of to write whole.")]
    pub fn btc_headers(&self, from: u64, count: u32) -> Result<Vec<Vec<u8>>, DbError> {
        let Some(relay) = self.anchoring()?.relay else {
            return Ok(Vec::new());
        };
        let state = self.state();
        let relay = crate::anchor::Relay::new(&state, relay);
        let mut headers = Vec::new();

        for height in from..from.saturating_add(u64::from(count)) {
            let Some(hash) = relay.main_at(height)? else {
                break;
            };
            let Some(header) = relay.raw_header(&hash)? else {
                break;
            };

            headers.push(header.to_vec());
        }

        Ok(headers)
    }

    #[document(
        parent = "Chain",
        summary = "What a peer starting at a checkpoint it trusts asks for: the checkpoint start, and the checkpoint's total work; None when the block is no checkpoint on the main chain, or the headers it needs are missing.",
    )]
    pub fn checkpoint_answer(&self, block: &Hash) -> Result<Option<(CheckpointStart, u256)>, DbError> {
        let params = self.light_params.clone();

        match stored_checkpoint_start(&*self.db, &params, block)? {
            Ok((_, start)) => Ok(Some((start, work_of(&*self.db, block)?))),
            Err(_) => Ok(None),
        }
    }

    #[document(
        parent = "Chain",
        summary = "Finality for light clients, an epoch at a time from an epoch on: each main-chain checkpoint's justification and the light update its block keeps, as stored_light_finality reads them, up to the tip. Empty on a chain whose finality does not vote. What a peer syncing from a snapshot learns the producers of a rotating or staked chain from.",
    )]
    pub fn light_finality_from(&self, from: u64, count: u64) -> Result<Vec<FinalityStep>, DbError> {
        let Some(interval) = self.light_params.finality.and_then(|finality| finality.checkpoint_interval()) else {
            return Ok(Vec::new());
        };

        stored_light_finality(&*self.db, interval, from, count, self.tip.height)
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
        summary = "Makes a block whose state the database already holds, taken from peers, the tip of a chain that held only genesis: stores the headers below it and the block itself, and from then on never rolls back past it. Finality starts over from genesis, and learns again from peers; or, for a node that started at a checkpoint it trusts, from that checkpoint, which it holds as justified and finalized.",
        params = {
            headers = "Consecutive headers ending at the block's, checked by consensus: from height 1, or from a checkpoint start's first header when a checkpoint is given.",
            pivot = "The block, whose state root the state in the database must have.",
            work = "Its total work.",
            checkpoint = "The checkpoint the node trusts, among the headers, or None for one that checked every header from genesis.",
            finality = "What the light client that checked the headers checked of finality: the justifications and light updates of checkpoints among them, stored so this node serves light clients from genesis, as one that ran every block does. Those of blocks not among the headers are left out.",
        },
        returns = "BadSnapshot when the chain holds more than genesis, the headers do not end at the block or start past genesis without a checkpoint, the checkpoint is not among them, or the state is not the block's.",
    )]
    pub fn install_snapshot(&mut self, headers: &[SealedHeader], pivot: &Block, work: u256, checkpoint: Option<Checkpoint>, finality: &crate::light::Kept) -> Result<(), ChainError> {
        let bad = |why: &str| Err(ChainError::BadSnapshot(why.to_string()));
        let id = pivot.id();

        if self.tip.height != 0 {
            return bad("this chain holds more than genesis");
        }

        let consecutive = headers.windows(2).all(|pair| pair[1].header.parent == pair[0].id() && pair[1].header.height == pair[0].header.height + 1);
        let first = headers.first().map_or(0, |sealed| sealed.header.height);

        if headers.last().map(SealedHeader::id) != Some(id) || !consecutive || first == 0 {
            return bad("the headers do not lead to the block");
        }

        match checkpoint {
            Some(checkpoint) if !headers.iter().any(|sealed| sealed.id() == checkpoint.block) => {
                return bad("the checkpoint is not among the headers");
            }
            None if first != 1 => return bad("the headers do not start at genesis, and no checkpoint is trusted"),
            _ => {}
        }

        let state = State::new(self.db.clone());

        if StateTree::new(&state, self.hash).root()? != pivot.header.state_root {
            return bad("the state held is not the block's");
        }

        for sealed in headers {
            state.put_value(Column::Headers, id_key(&sealed.id()), sealed);
            state.put_value(Column::Heights, sealed.header.height.to_be_bytes().to_vec(), &sealed.id());
        }

        if let Some(checkpoint) = checkpoint {
            finality::anchor(&state, &checkpoint);
        }

        // The finality before the pivot, as checked: what light clients ask this node for.
        let installed: HashSet<Hash> = headers.iter().map(SealedHeader::id).collect();

        for update in finality.updates.iter().filter(|update| installed.contains(&update.block)) {
            state.put_value(Column::Light, id_key(&update.block), update);
        }

        for justification in finality.justifications.iter().filter(|justification| installed.contains(&justification.target.block)) {
            finality::keep_justification(&state, justification);
        }

        state.put_value(Column::Blocks, id_key(&id), pivot);
        state.put_value(Column::Work, id_key(&id), &WorkRecord { total: work });
        state.put_value(Column::Meta, TIP_KEY.to_vec(), &id);
        state.put_value(Column::Meta, TIPS_KEY.to_vec(), &Leaves { blocks: vec![id] });
        state.put_value(Column::Meta, SNAPSHOT_KEY.to_vec(), &SnapshotRecord {
            height: pivot.header.height,
            block: id,
        });
        // History begins at the pivot: nothing below it was run here.
        state.put_value(Column::Meta, HISTORY_FROM_KEY.to_vec(), &HistoryFrom { height: pivot.header.height });
        self.db.commit(state.take_changes())?;
        log::info!("[chain] installed a snapshot at height {} {id}", pivot.header.height);

        // The member index follows the registry, which the snapshot's state replaced whole.
        if self.members {
            members::rebuild(&self.db)?;
        }

        self.tip = pivot.header.clone();
        self.tip_work = work;
        self.tips = BTreeSet::from([id]);
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
        self.next.rules.fees.as_deref()
    }

    /// The chain's fees, shared, for what checks transactions off the node loop.
    pub(crate) fn shared_fees(&self) -> Option<Arc<Fees>> {
        self.next.rules.fees.clone()
    }

    /// What a block at a height runs with, in the state its parent left: genesis's, with what
    /// governance decided there. Fees, limits and the reward stand from the block after the one
    /// that decided them; finality's settings by the epoch.
    pub(crate) fn params_in(&self, state: &State, height: u64) -> Result<BlockParams, DbError> {
        let mut params = BlockParams { rules: self.rules.clone(), reward: (self.reward, 0), max_block_bytes: self.max_block_bytes };
        let changes = Changes::new(state);
        // The forks in force, on a chain with governance or without: genesis's are in state too.
        params.rules.vm.forks = changes.forks()?.at(height);

        if !self.governed {
            return Ok(params);
        }

        if let Some(config) = changes.fees()? {
            params.rules.fees = Some(built_fees(&self.built_fees, config));
        }

        if let Some(limits) = changes.limits()? {
            params.max_block_bytes = limits.max_block_bytes;
        }

        if let Some(decided) = changes.reward()? {
            params.reward = (decided.reward, decided.from);
        }

        govern_rules(&mut params.rules, state, height)?;

        if let Some(config) = &self.finality_config
            && config.votes()
        {
            (params.rules.staking, params.rules.signers) = switch::rules_at(state, config, config.epoch_of(height), self.genesis)?;
        }

        Ok(params)
    }

    /// Reads what the next block runs with again, once the tip moved.
    pub(super) fn refresh_params(&mut self) -> Result<(), DbError> {
        self.next = self.params_in(&self.state(), self.tip.height + 1)?;

        Ok(())
    }

    #[document(
        parent = "Chain",
        summary = "The chain's bootnodes at the tip: the ones governance decided last, or genesis's while it decided none.",
    )]
    pub fn bootnodes(&self) -> Result<Vec<String>, DbError> {
        if !self.governed {
            return Ok(self.bootnodes.clone());
        }

        Ok(Changes::new(&self.state()).bootnodes()?.unwrap_or_else(|| self.bootnodes.clone()))
    }

    #[document(parent = "Chain", summary = "The most bytes a block's transactions may take, encoded, if genesis bounds them.")]
    pub fn max_block_bytes(&self) -> Option<u64> {
        self.next.max_block_bytes
    }

    /// The transactions a block can hold within max_block_bytes, in order. One that would pass
    /// it is left out and the rest still tried: a sender's later ones then fail at their nonce and
    /// are left out by running too.
    fn within_bytes(txs: Vec<SignedTx>, max_block_bytes: Option<u64>) -> Vec<SignedTx> {
        let Some(limit) = max_block_bytes else {
            return txs;
        };
        let mut used = 0u64;

        txs.into_iter()
            .filter(|tx| {
                let bytes = tx_bytes(tx);
                let fits = used + bytes <= limit;
                used += if fits { bytes } else { 0 };

                fits
            })
            .collect()
    }

    #[document(parent = "Chain", summary = "The base price of gas in the next block, the one on the tip: what a transaction sent now must offer at least.")]
    pub fn next_base_price(&self) -> u256 {
        base_price_after(self.fees(), &self.tip)
    }

    #[document(
        parent = "Chain",
        summary = "What a producer gains per gas from a transaction in the next block, which is what producers order the mempool by.",
    )]
    pub fn priority(&self, tx: &SignedTx) -> u256 {
        self.fees()
            .map_or(u256::ZERO, |fees| fees.priority(self.next_base_price(), &tx.fee))
    }

    #[document(
        parent = "Chain",
        summary = "What a transaction is worth to a producer in the next block: its priority, or None when the block cannot take it, as its offer is under the next base price. What producers select from the mempool by.",
        see_also = "Chain::priority, Mempool::select",
    )]
    pub fn worth(&self, tx: &SignedTx) -> Option<u256> {
        let Some(fees) = self.fees() else {
            return Some(u256::ZERO);
        };
        let base = self.next_base_price();

        fees.pricing().price(base, &tx.fee).map(|_| fees.priority(base, &tx.fee))
    }

    /// What a block's transactions run with.
    /// The headers consensus looks at when checking the block after parent: the parent first,
    /// then its parent, and so on, as many as consensus wants and the chain has.
    fn ancestors(&self, parent: &BlockHeader) -> Result<Vec<BlockHeader>, DbError> {
        let mut ancestors = vec![parent.clone()];
        let lookback = self.consensus_at(parent.height + 1)?.lookback();

        while ancestors.len() < lookback {
            let last = ancestors.last().expect("starts with the parent");

            if last.height == 0 {
                break;
            }

            match stored_header(&*self.db, &last.parent)? {
                Some(sealed) => ancestors.push(sealed.header),
                None => break,
            }
        }

        Ok(ancestors)
    }

    #[document(
        parent = "Chain",
        summary = "The difficulty the block after a stored block must declare, given its timestamp: what consensus says from that block and the ancestors it looks at.",
    )]
    pub fn difficulty_after(&self, parent: &BlockHeader, timestamp: u64) -> Result<u128, DbError> {
        Ok(self.consensus_at(parent.height + 1)?.difficulty(&self.ancestors(parent)?, timestamp))
    }

    #[document(
        parent = "Chain",
        summary = "The consensus rules the block at a height is checked by: genesis's, or, under governance, the consensus fixed for the height's epoch in the state at the tip, which the checkpoint fixed_ahead epochs before it fixed, so every branch that split off since agrees. Past the epochs fixed so far, the consensus decided last.",
    )]
    pub fn consensus_at(&self, height: u64) -> Result<Arc<dyn Consensus>, DbError> {
        if !self.governed {
            return Ok(self.consensus.clone());
        }

        let config = self.consensus_config_at(height)?;
        let mut cache = self.rules_cache.lock().expect("no panic holds it");

        if let Some((_, rules)) = cache.iter().find(|(known, _)| *known == config) {
            return Ok(rules.clone());
        }

        let rules: Arc<dyn Consensus> = Arc::from(config.rules(self.hash, self.pow_window, self.finality_config.as_ref().map(FinalityConfig::checkpoint_interval)));
        cache.push((config, rules.clone()));

        Ok(rules)
    }

    #[document(
        parent = "Chain",
        summary = "The consensus the block at a height runs under, as consensus_at takes it: what a producing node picks its producer by.",
    )]
    pub fn consensus_config_at(&self, height: u64) -> Result<ConsensusConfig, DbError> {
        if !self.governed {
            return Ok(self.light_params.consensus.clone());
        }

        let state = self.state();
        let changes = Changes::new(&state);
        let epoch = self.finality_config.as_ref().map_or(height, |config| config.epoch_of(height));

        match changes.consensus_for(epoch)? {
            Some(config) => Ok(config),
            None => Ok(changes.decided()?.consensus),
        }
    }

    #[document(
        parent = "Chain",
        summary = "Who may produce the block at a height, under a consensus that takes its producers from finality: the voters fixed for the height's epoch, in the state at the tip, the authority signers under a rotating authority and the bonded validators, weighted by bond, under production by stake. Empty under any other consensus, or when no set is fixed for the epoch yet.",
    )]
    pub fn producers_for(&self, height: u64) -> Result<Vec<Member>, DbError> {
        if !self.consensus_at(height)?.needs_producers() {
            return Ok(Vec::new());
        }

        self.producers_in(&self.state(), height)
    }

    #[document(
        parent = "Chain",
        summary = "The account a signer's key speaks through, under authority finality: the decided set's, which a switch into authority names, or genesis's.",
    )]
    pub fn signer_account(&self, key: &PublicKey) -> Result<Option<Address>, DbError> {
        let decided = crate::registry::signers::Signers::new(&self.state()).account_of(key)?;

        Ok(decided.or_else(|| self.finality_config.as_ref().and_then(|config| config.signer_account(key))))
    }

    /// The producers for a height, in a state: the epoch's voters.
    fn producers_in(&self, state: &State, height: u64) -> Result<Vec<Member>, DbError> {
        let Some(config) = &self.finality_config else {
            return Ok(Vec::new());
        };

        Ok(config.voters_in(state, config.epoch_of(height))?.map(|set| set.members).unwrap_or_default())
    }

    fn runner<'a>(&'a self, rules: &'a Rules) -> Runner<'a> {
        Runner {
            rules,
            vm: &self.vm,
            workers: &self.workers,
        }
    }

    #[document(
        parent = "Chain",
        summary = "Whether a block whose parent this chain lacks is worth holding until the parent comes: what can be checked without it holds. It is no block the chain has or knows to be bad, nor a bad block's child; it sits above finality and at most depth heights past the tip; and its transaction, proof and pool roots are its own.",
        params = { depth = "How many heights past the tip it may be." },
    )]
    pub fn fits_ahead(&self, block: &Block, depth: u64) -> bool {
        let id = block.id();
        let height = block.header.height;

        !self.bad.contains(&id)
            && !self.bad.contains(&block.header.parent)
            && matches!(self.block(&id), Ok(None))
            && self.finalized_height().is_none_or(|finalized| height > finalized)
            && height <= self.tip().height.saturating_add(depth)
            && block.header.tx_root == Block::tx_root(self.hash, &block.txs)
            && block.header.proof_root == Block::proof_root(self.hash, &block.proofs)
            && block.header.pool_root == Block::pool_root(self.hash, block.pool.as_ref())
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

        // At or below the finalized checkpoint, only the main chain's blocks exist: refused before
        // anything is fetched for it, so a node never asks for a branch under its finality.
        if self.finalized_height().is_some_and(|finalized| block.header.height <= finalized) {
            return Err(ImportError::Invalid(BlockError::ConflictsWithFinality));
        }

        // Nor at or below a checkpoint anchored while anchoring was enforced, final or not.
        if self.anchored_height()?.is_some_and(|anchored| block.header.height <= anchored) {
            return Err(ImportError::Invalid(BlockError::ConflictsWithAnchor));
        }

        let Some(parent) = self.block(&parent_id)? else {
            return Err(ImportError::UnknownParent(parent_id));
        };

        if self.bad.contains(&parent_id) {
            self.mark_bad([id])?;

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

        if block.header.pool_root != Block::pool_root(self.hash, block.pool.as_ref()) {
            return Err(ImportError::Invalid(BlockError::PoolRoot));
        }

        if block.pool.is_some_and(|pool| pool.parts > PAYOUT_PARTS) {
            return Err(ImportError::Invalid(BlockError::Pool(format!("it sends more than the {PAYOUT_PARTS} parts there are"))));
        }

        // On the tip, its size and base price check against what the next block runs with. On a
        // branch, they wait for running it, on its parent's state, which decides them there.
        if parent_id == self.tip.id() {
            check_size_and_price(block, &parent.header, &self.next)?;
        }

        let consensus = self.consensus_at(block.header.height)?;
        let producers = match consensus.needs_producers() {
            true => self.producers_for(block.header.height)?,
            false => Vec::new(),
        };
        consensus
            .check(&block.header, &block.seal, &self.ancestors(&parent.header)?, &producers, now)
            .map_err(|err| ImportError::Invalid(BlockError::Consensus(err)))?;
        let work = work_of(&*self.db, &parent_id)? + consensus.work(&block.header);
        let state = Arc::new(State::new(self.db.clone()));
        state.put_value(Column::Blocks, id_key(&id), block);
        state.put_value(Column::Headers, id_key(&id), &block.sealed_header());
        state.put_value(Column::Work, id_key(&id), &WorkRecord { total: work });

        if parent_id == self.tip.id() {
            let receipts = match self.apply(&state, block) {
                Ok(receipts) => receipts,
                Err(err) => {
                    if matches!(err, ImportError::Invalid(_)) {
                        self.mark_bad([id])?;
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

    /// Remembers blocks that failed to run, in memory and in Meta, so neither this run nor the
    /// next tries their branch again. Beyond MAX_BAD_BLOCKS the oldest are let go.
    fn mark_bad(&mut self, ids: impl IntoIterator<Item = Hash>) -> Result<(), DbError> {
        for id in ids {
            if self.bad.insert(id) {
                self.bad_order.push_back(id);
            }
        }

        while self.bad_order.len() > self.most_bad {
            if let Some(oldest) = self.bad_order.pop_front() {
                self.bad.remove(&oldest);
            }
        }

        let state = State::new(self.db.clone());
        state.put_value(Column::Meta, BAD_KEY.to_vec(), &BadBlocks { blocks: self.bad_order.iter().copied().collect() });

        self.db.commit(state.take_changes())
    }

    #[document(
        parent = "Chain",
        summary = "The blocks this node remembers as failing to run, oldest first: it refuses them and anything built on them without running them again. The last MAX_BAD_BLOCKS, or as many as ChainOptions::bad_blocks says, kept across restarts.",
    )]
    pub fn bad_blocks(&self) -> Vec<Hash> {
        self.bad_order.iter().copied().collect()
    }

    /// Deletes the side branches the chain can never follow: those that split off more than
    /// max_reorg_depth blocks below the tip, below the finalized checkpoint, or below the snapshot
    /// this node took state at. Run after the tip moves. A branch's blocks go from its leaf down
    /// to the fork; the blocks on the main chain stay.
    fn prune_side_branches(&mut self) -> Result<usize, DbError> {
        // With finality, a justified branch may be followed down to the finalized checkpoint, so
        // side branches are kept that far; without, work alone moves the chain, max_reorg_depth
        // back at most.
        let by_depth = match self.finality {
            Some(_) => 0,
            None => self.tip.height.saturating_sub(self.max_reorg_depth),
        };
        let floor = [
            by_depth,
            self.finalized_height().unwrap_or(0),
            self.snapshot.unwrap_or(0),
        ]
        .into_iter()
        .max()
        .expect("three candidates");
        let state = State::new(self.db.clone());
        let mut pruned = 0;
        let mut dropped_leaves = Vec::new();

        for leaf in self.tips.iter().copied() {
            if leaf == self.tip.id() || self.on_main_chain(&leaf)? {
                continue;
            }

            // The branch from its leaf back to where it meets the main chain, if it does above
            // the floor; else everything stored of it, which can never be followed.
            let mut branch = Vec::new();
            let mut current = leaf;
            let followable = loop {
                let Some(header) = stored_header(&*self.db, &current)? else {
                    break false;
                };

                if self.on_main_chain(&current)? {
                    break header.header.height >= floor;
                }

                if header.header.height <= floor {
                    branch.push(current);

                    break false;
                }

                branch.push(current);
                current = header.header.parent;
            };

            if followable {
                continue;
            }

            for id in &branch {
                state.delete(Column::Blocks, id_key(id));
                state.delete(Column::Headers, id_key(id));
                state.delete(Column::Work, id_key(id));
            }

            pruned += branch.len();
            dropped_leaves.push(leaf);
        }

        if dropped_leaves.is_empty() {
            return Ok(0);
        }

        for leaf in dropped_leaves {
            self.tips.remove(&leaf);
        }

        let blocks = self.tips.iter().copied().collect();
        state.put_value(Column::Meta, TIPS_KEY.to_vec(), &Leaves { blocks });
        self.db.commit(state.take_changes())?;
        log::info!("[chain] pruned {pruned} side blocks on branches the chain can no longer follow");

        Ok(pruned)
    }

    /// Makes new_tip the tip: rolls the main chain back to where its branch splits off, then runs
    /// the branch, in one commit. A branch that outweighs the main chain is only followed when it
    /// holds the latest justified checkpoint, and splits off no deeper than max_reorg_depth. A
    /// justified one (justified set) only needs to hold the finalized one, however deep it splits
    /// off: the voters chose it, and a node that mined on its own while cut off from them follows
    /// it back, as far as it went.
    fn reorganize(&mut self, new_tip: &Block, work: u256, justified: bool) -> Result<Imported, ImportError> {
        let mut branch = vec![new_tip.clone()];
        let mut fork = new_tip.header.parent;
        // How deep a reorg by work may go is measured from this node's tip, not along the branch,
        // which may be far longer: a node that mined a few blocks of its own follows a peer's chain
        // that grew from below them, however long it is. One to a justified branch goes down to the
        // finalized checkpoint.
        let finalized = self.finalized_height().unwrap_or(0);
        let floor = match justified {
            true => finalized,
            false => self.tip.height.saturating_sub(self.max_reorg_depth),
        };

        while !self.on_main_chain(&fork)? {
            let block = self.block(&fork)?.ok_or_else(|| missing(&fork))?;

            // Its parent, the deepest the split could be, is below the floor.
            if block.header.height <= floor {
                break;
            }

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

        if !self.on_main_chain(&fork)? || (!justified && self.tip.height - fork_height > self.max_reorg_depth) {
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

        // Never off a checkpoint anchored while anchoring was enforced, however the branch weighs.
        if let Some(anchored) = self.anchored_height()?
            && fork_height < anchored
        {
            log::info!("[chain] not following a branch to {} that leaves the checkpoint at height {anchored} anchored in Bitcoin", new_tip.id());

            return Ok(Imported::Side);
        }

        let state = Arc::new(State::new(self.db.clone()));
        let mut undone = Vec::new();

        for height in (fork_height + 1..=self.tip.height).rev() {
            let id: Hash = read_value(&*state, Column::Heights, &height.to_be_bytes())?
                .ok_or_else(|| DbError::Corrupt(format!("no main-chain block at height {height}")))?;
            revert(&state, &id, self.members)?;
            undone.push(self.block(&id)?.ok_or_else(|| missing(&id))?);
        }

        branch.reverse();

        for (position, block) in branch.iter().enumerate() {
            if let Err(err) = self.apply(&state, block) {
                if matches!(err, ImportError::Invalid(_)) {
                    self.mark_bad(branch[position..].iter().map(Block::id))?;
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

    /// What a block does to state past its transactions: the producer's reward, a mining pool's
    /// share of it and of the fees sent to the pool's escrow with the root of its claims, and what
    /// its finality proofs pay and record.
    fn settle(&self, layer: &Arc<State>, block: &Block, (reward, from): (Reward, u64), fees: u256, burned: u256, pooled: bool) -> Result<(), ImportError> {
        let reward = reward.since(block.header.height, from);
        let accounts = Accounts::new(layer);

        if reward > u256::ZERO {
            accounts.credit(&block.header.producer, reward).map_err(|err| match err {
                RegistryError::Db(err) => ImportError::Db(err),
                _ => ImportError::Invalid(BlockError::Reward),
            })?;
        }

        // The supply: the reward made, and what the block's fees burned for good.
        crate::registry::supply::Supply::mint(layer, reward)?;
        crate::registry::supply::Supply::burn(layer, burned)?;

        // The producer holds the reward and its fees by now, and sends the pool's share of both
        // to its escrow. It cannot have spent the fees in the block: a pool's node keeps its own
        // transactions out of the blocks it builds.
        if let Some(pool) = &block.pool {
            let total = reward.saturating_add(fees);
            let producer = &block.header.producer;
            let escrow = crate::registry::pools::pool_escrow(producer, self.hash);
            accounts.transfer(producer, &escrow, parts_of(total, pool.parts)).map_err(|err| match err {
                RegistryError::Db(err) => ImportError::Db(err),
                other => ImportError::Invalid(BlockError::Pool(other.to_string())),
            })?;
            crate::registry::pools::Pools::new(layer, self.hash).commit(block.header.height, crate::registry::pools::PoolRoot { pool: *producer, root: pool.root, count: pool.count });
            // What was shared out, which the block alone does not say, as its fees are not in it.
            layer.put_value(Column::Meta, split_key(&block.id()), &Split { total });
        }

        match &self.finality {
            Some(finality) => finality.apply(layer, block, if pooled { VoteRewardSource::Fees } else { VoteRewardSource::Mint })?,
            None if !block.proofs.is_empty() => return Err(ImportError::Invalid(BlockError::Proof("this chain has no finality".to_string()))),
            None => {}
        }

        // Each checkpoint fixes the consensus decided last for the epoch fixed_ahead ahead, as
        // finality fixes its voters.
        if self.governed
            && let Some(config) = &self.finality_config
            && block.header.height.is_multiple_of(config.checkpoint_interval().max(1))
        {
            Changes::new(layer).fix(config.epoch_of(block.header.height) + self.fixed_ahead)?;
        }

        Ok(())
    }

    /// Runs a block on top of below and makes it the tip there, keeping an undo record of every
    /// value it replaced.
    fn apply(&self, below: &Arc<State>, block: &Block) -> Result<Vec<Receipt>, ImportError> {
        let id = block.id();
        let height = block.header.height;
        let layer = Arc::new(State::new(below.clone()));
        let context = BlockContext {
            height,
            timestamp: block.header.timestamp,
            producer: block.header.producer,
            base_price: block.header.base_price,
            genesis: self.genesis(),
        };
        let params = self.params_in(below, height)?;

        // A fork this release does not know: it cannot run the block as nodes that know it do.
        if let Some(fork) = Changes::new(below).forks()?.unknown().filter(|fork| fork.height <= height) {
            return Err(ImportError::UnknownFork(fork.name.clone()));
        }

        let parent = stored_header(&*self.db, &block.header.parent)?.ok_or_else(|| missing(&block.header.parent))?.header;
        check_size_and_price(block, &parent, &params)?;
        let ran = self.runner(&params.rules).run(&layer, &block.txs, &context, Unfit::Refuse, None)?;
        let receipts = ran.receipts;

        if ran.gas_used != block.header.gas_used {
            return Err(ImportError::Invalid(BlockError::GasUsed {
                expected: ran.gas_used,
                got: block.header.gas_used,
            }));
        }

        self.settle(&layer, block, params.reward, ran.to_producer, ran.burned, params.rules.pooled_fees)?;

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
            && let Some(update) = LightUpdate::of(&layer, self.hash, finality, self.governed, id, height)?
        {
            layer.put_value(Column::Light, id_key(&id), &update);
        }

        for (index, (tx, receipt)) in block.txs.iter().zip(&receipts).enumerate() {
            let record = TxReceipt::new(id, index as u32, receipt);
            let tx_id = tx.id(self.hash);
            layer.put_value(Column::Receipts, id_key(&tx_id), &record);

            // Ethereum's tools find a transaction by the keccak256 of its signed bytes.
            #[cfg(feature = "ethereum")]
            if let crate::structs::tx::Tx::Ethereum { raw } = &tx.tx {
                layer.put_value(Column::Ethereum, crate::crypto::ethereum::tx_hash(raw).to_vec(), &tx_id);
            }

            // Which vote decided or rejected each change, for GET /governance/history.
            match record.status {
                receipt::TxStatus::Decided { decision, .. } => layer.put_value(Column::Decisions, decision.to_be_bytes().to_vec(), &tx_id),
                receipt::TxStatus::Rejected { rejection, .. } => layer.put_value(Column::Decisions, rejection_key(rejection), &tx_id),
                _ => {}
            }

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

        // Whoever the block made or changed a multisig's members, as the registry now says.
        if self.members {
            members::follow(&layer, &**below)?;
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

        // History: what each key of state and of the tree held before this block, by key and
        // height, and the list of them, for pruning together. Undoing the block takes them back.
        let mut listed = HistoryList::default();

        for entry in entries.iter().filter(|entry| kept_in_history(entry.column)) {
            below.put(Column::History, history_key(entry.column, &entry.key, height), replaced(&entry.value));
            listed.keys.push(HistoryKey { column: entry.column, key: entry.key.clone() });
        }

        for HistoryKey { column, key } in &listed.keys {
            entries.push(UndoEntry { column: Column::History, key: history_key(*column, key, height), value: None });
        }

        below.put_value(Column::History, history_list_key(height), &listed);
        entries.push(UndoEntry { column: Column::History, key: history_list_key(height), value: None });

        // Undoing the block also forgets how to undo it.
        entries.push(UndoEntry {
            column: Column::Undo,
            key: id_key(&id),
            value: None,
        });
        below.merge(changes);
        below.put_value(Column::Undo, id_key(&id), &Undo { entries });
        self.prune_history(below, height)?;

        // Nothing rolls back further than max_reorg_depth by work, nor, on a chain with finality,
        // below the finalized checkpoint, down to which a justified branch may reach: older undo
        // records go. While finality stalls they are kept, and go together once it moves on.
        if let Some(old) = height.checked_sub(self.max_reorg_depth + 1) {
            let mut gone = match self.finality.as_ref().map(|_| self.finalized_height().unwrap_or(0)) {
                Some(finalized) => old.min(finalized),
                None => old,
            };

            while gone > 0
                && let Some(old_id) = read_value::<Hash>(&**below, Column::Heights, &gone.to_be_bytes())?
                && read_value::<Undo>(&**below, Column::Undo, &id_key(&old_id))?.is_some()
            {
                below.delete(Column::Undo, id_key(&old_id));
                gone -= 1;
            }
        }

        Ok(receipts)
    }

    /// Lets go of history past the window, as the tip reaches a height: the entries of the block
    /// after the lowest height kept, which only reads below it need, one height at a time.
    fn prune_history(&self, below: &Arc<State>, height: u64) -> Result<(), DbError> {
        let Some(window) = self.state_history else {
            return Ok(());
        };
        let mut from = read_value::<HistoryFrom>(&**below, Column::Meta, HISTORY_FROM_KEY)?.map_or(0, |from| from.height);

        if from + window >= height {
            return Ok(());
        }

        while from + window < height {
            let gone = from + 1;

            if let Some(list) = read_value::<HistoryList>(&**below, Column::History, &history_list_key(gone))? {
                for HistoryKey { column, key } in list.keys {
                    below.delete(Column::History, history_key(column, &key, gone));
                }
            }

            below.delete(Column::History, history_list_key(gone));
            from = gone;
        }

        below.put_value(Column::Meta, HISTORY_FROM_KEY.to_vec(), &HistoryFrom { height: from });

        Ok(())
    }

    pub(super) fn on_main_chain(&self, id: &Hash) -> Result<bool, DbError> {
        self.view().on_main_chain(id)
    }
}

/// Takes a main-chain block back off, restoring every value it replaced.
fn revert(state: &Arc<State>, id: &Hash, members: bool) -> Result<(), DbError> {
    let undo: Undo = read_value(&**state, Column::Undo, &id_key(id))?
        .ok_or_else(|| DbError::Corrupt(format!("no undo record for {id}")))?;
    let layer = State::new(state.clone());

    for entry in undo.entries {
        match entry.value {
            Some(value) => layer.put(entry.column, entry.key, value),
            None => layer.delete(entry.column, entry.key),
        }
    }

    // The member index goes back with the registry it follows, built late or not: a block run
    // before the node kept it left nothing of it in its undo record.
    if members {
        members::follow(&layer, &**state)?;
    }

    state.merge(layer.take_changes());

    Ok(())
}

/// The base price of the block after parent, under these fees.
fn base_price_after(fees: Option<&Fees>, parent: &BlockHeader) -> u256 {
    match fees {
        Some(fees) => fees.pricing().next_price(parent.base_price, parent.gas_used, fees.block_gas_limit()),
        None => u256::ZERO,
    }
}

/// A block's size and base price, against what it runs with.
fn check_size_and_price(block: &Block, parent: &BlockHeader, params: &BlockParams) -> Result<(), ImportError> {
    if let Some(limit) = params.max_block_bytes {
        let bytes = txs_bytes(&block.txs);

        if bytes > limit {
            return Err(ImportError::Invalid(BlockError::TooBig { bytes, limit }));
        }
    }

    let base_price = base_price_after(params.rules.fees.as_deref(), parent);

    if block.header.base_price != base_price {
        return Err(ImportError::Invalid(BlockError::BasePrice { expected: base_price, got: block.header.base_price }));
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
    // A full block writes more than wincode's default 4 MiB of these: see Block::txs.
    #[wincode(with = "wincode::containers::Vec<UndoEntry, wincode::len::BincodeLen<{ crate::structs::block::MAX_LIST_MEMORY }>>")]
    entries: Vec<UndoEntry>,
}

impl Hashable for Undo {}

#[derive(Clone, Copy, Debug, SchemaWrite, SchemaRead)]
struct WorkRecord {
    #[wincode(with = "U256Le")]
    total: u256,
}

impl Hashable for WorkRecord {}

/// What a block is built from: its transactions and proofs, who produces it and when, and how long
/// running the transactions may take.
struct Draft {
    txs: Vec<SignedTx>,
    proofs: Vec<Justification>,
    producer: Address,
    timestamp: u64,
    deadline: Option<Instant>,
    /// The difficulty the producer declares, when consensus leaves it to the producer.
    difficulty: Option<u128>,
    /// A mining pool's commitment.
    pool: Option<PoolCommitment>,
}

/// The stored blocks no stored block builds on yet.
#[derive(Clone, Debug, Default, SchemaWrite, SchemaRead)]
struct Leaves {
    blocks: Vec<Hash>,
}

impl Hashable for Leaves {}

/// The blocks that failed to run, oldest first, as Meta keeps them.
#[derive(Clone, Debug, Default, SchemaWrite, SchemaRead)]
struct BadBlocks {
    blocks: Vec<Hash>,
}

impl Hashable for BadBlocks {}

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
                    Engine::Evm => "evm",
                }
            ),
        }
    }
}

impl std::error::Error for ChainError {}

#[document(
    summary = "The lowest height whose block body a database holds, as Chain::bodies_from says: for readers outside the node, such as the HTTP API, to tell a pruned block from one never stored.",
    category = "Chain",
)]
pub fn stored_bodies_from(db: &dyn Read) -> Result<u64, DbError> {
    let snapshot = read_value::<SnapshotRecord>(db, Column::Meta, SNAPSHOT_KEY)?.map_or(0, |record| record.height + 1);
    let pruned = read_value::<BodiesFrom>(db, Column::Meta, BODIES_FROM_KEY)?.map_or(0, |from| from.height);

    Ok(snapshot.max(pruned))
}

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

/// Where a block that split its reward keeps what it split.
fn split_key(id: &Hash) -> Vec<u8> {
    [b"split/".as_slice(), &id_key(id)].concat()
}

#[document(
    summary = "What a mining pool's block shared out: its reward and the fees its producer earned, together, of which its commitment's parts went to the pool's escrow. Read straight from a database; None for a block that carried no pool commitment, or one this node has not run.",
    category = "Chain",
    see_also = "PoolCommitment, parts_of",
)]
pub fn stored_split(db: &dyn Read, id: &Hash) -> Result<Option<u256>, DbError> {
    Ok(read_value::<Split>(db, Column::Meta, &split_key(id))?.map(|split| split.total))
}

#[document(
    summary = "The state at the end of a main-chain block, read straight from a database: the tip's as it is, an older one's through history, as a layer to read and prove from and throw away. For values, calls and proofs at a past block.",
    category = "Chain",
    returns = "The layer; None for a block not on the main chain, or below the lowest height history holds: the node's --state-history window, or where its history began.",
    see_also = "Historic, Chain::history_from",
)]
pub fn stored_state_at(db: Arc<dyn crate::db::Committed>, id: &Hash) -> Result<Option<Arc<State>>, DbError> {
    let Some(sealed) = stored_header(&*db, id)? else {
        return Ok(None);
    };
    let height = sealed.header.height;

    if read_value::<Hash>(&*db, Column::Heights, &height.to_be_bytes())? != Some(*id) {
        return Ok(None);
    }

    let Some(top) = read_value::<Hash>(&*db, Column::Meta, TIP_KEY)?.map(|tip| stored_header(&*db, &tip)).transpose()?.flatten().map(|sealed| sealed.header.height) else {
        return Ok(None);
    };

    if height == top {
        return Ok(Some(Arc::new(State::new(db))));
    }

    if read_value::<HistoryFrom>(&*db, Column::Meta, HISTORY_FROM_KEY)?.is_none_or(|from| height < from.height) {
        return Ok(None);
    }

    Ok(Some(Arc::new(State::new(Arc::new(crate::db::Historic::new(db, height))))))
}

#[document(
    summary = "What a block keeps for light clients, read straight from a database: for genesis and checkpoints, proofs of the voters they fixed. None for other blocks, and for those before a snapshot this node synced from.",
    category = "Chain",
    see_also = "LightUpdate",
)]
pub fn stored_light_update(db: &dyn Read, block: &Hash) -> Result<Option<LightUpdate>, DbError> {
    let Some(bytes) = db.read(Column::Light, &id_key(block))? else {
        return Ok(None);
    };

    // Updates stored before switched epochs, or before eras, lack those fields.
    if let Ok(update) = wincode::deserialize::<LightUpdate>(&bytes) {
        return Ok(Some(update));
    }

    if let Ok(update) = wincode::deserialize::<crate::light::ErasLightUpdate>(&bytes) {
        return Ok(Some(update.into()));
    }

    Ok(Some(
        wincode::deserialize::<crate::light::LegacyLightUpdate>(&bytes)
            .map_err(|err| DbError::Corrupt(format!("light update {block}: {err}")))?
            .into(),
    ))
}

#[document(
    summary = "Finality for light clients, read straight from a database, an epoch at a time from one on: each main-chain checkpoint's justification, none for genesis, and the light update its block keeps. It stops after count epochs, past top, and before the first checkpoint after genesis without a justification.",
    category = "Chain",
    params = {
        interval = "Blocks between checkpoints.",
        from = "The first epoch.",
        count = "The most epochs.",
        top = "The tip's height: nothing above it is sent.",
    },
    see_also = "FinalityStep, LightClient::deliver_finality",
)]
pub fn stored_light_finality(db: &dyn Read, interval: u64, from: u64, count: u64, top: u64) -> Result<Vec<FinalityStep>, DbError> {
    let mut steps = Vec::new();

    for epoch in from..from.saturating_add(count) {
        let Some(height) = epoch.checked_mul(interval).filter(|height| *height <= top) else {
            break;
        };
        let Some(sealed) = stored_header_at(db, height)? else {
            break;
        };
        let block = sealed.id();
        // An epoch may go unjustified for good, as when too few voted, and later ones be justified
        // from a source below it: a step without one says it is skipped, so a client gets past it.
        let justification = match epoch {
            0 => None,
            _ => stored_justification(db, &block)?,
        };

        steps.push(FinalityStep {
            justification,
            update: stored_light_update(db, &block)?,
        });
    }

    // Unjustified epochs with nothing justified after them are where the chain's votes have got to,
    // not a gap: they end the answer, whatever the answer's size cut it off at.
    if steps.last().is_some_and(|step: &FinalityStep| step.justification.is_none()) && !justified_within(db, interval, from.saturating_add(count), top)? {
        while steps.last().is_some_and(|step| step.justification.is_none()) && steps.len() > usize::from(from == 0) {
            steps.pop();
        }
    }

    Ok(steps)
}

/// How many epochs past an answer a node looks for a justification, to tell a gap from the end of
/// what is justified: a chain with no finality for longer shows light clients its end there.
const MOST_GAP_EPOCHS: u64 = 4096;

/// Whether any epoch from this one, within MOST_GAP_EPOCHS, has a justification on the main chain.
fn justified_within(db: &dyn Read, interval: u64, from: u64, top: u64) -> Result<bool, DbError> {
    for epoch in from..from.saturating_add(MOST_GAP_EPOCHS) {
        let Some(height) = epoch.checked_mul(interval).filter(|height| *height <= top) else {
            return Ok(false);
        };
        let Some(sealed) = stored_header_at(db, height)? else {
            return Ok(false);
        };

        if stored_justification(db, &sealed.id())?.is_some() {
            return Ok(true);
        }
    }

    Ok(false)
}

#[document(
    summary = "Why a block cannot be a checkpoint start.",
    category = "Chain",
    see_also = "stored_checkpoint_start",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartProblem {
    #[document(summary = "The block is not stored, or not on the main chain.")]
    NotOnChain,
    #[document(summary = "Under voting finality, the block opens no epoch.")]
    NoEpoch,
    #[document(summary = "Starting there takes more headers than one answer holds.")]
    TooMany,
    #[document(summary = "A header below it is missing: this node synced from a snapshot or a checkpoint past it.")]
    Missing(u64),
}

impl fmt::Display for StartProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StartProblem::NotOnChain => write!(f, "not on the main chain"),
            StartProblem::NoEpoch => write!(f, "opens no epoch: it is no checkpoint"),
            StartProblem::TooMany => write!(f, "starting there takes more than {} headers", crate::light::MAX_LIGHT_HEADERS),
            StartProblem::Missing(height) => write!(f, "no main-chain header at {height}: this node synced from a snapshot past it"),
        }
    }
}

#[document(
    summary = "What a client, light or full, starts from at a checkpoint: its epoch, and the headers and light updates back to where consensus and finality need them, read straight from a database. What GET /light/checkpoint and SyncRequest::Checkpoint answer.",
    category = "Chain",
    params = {
        db = "The database.",
        params = "The chain's light parameters, for its finality's timing and consensus's lookback.",
        block = "The checkpoint's id.",
    },
    returns = "The epoch and the start, or why the block cannot be one.",
    see_also = "CheckpointStart, checkpoint_start, checkpoint_first_header",
)]
pub fn stored_checkpoint_start(db: &dyn Read, params: &LightParams, block: &Hash) -> Result<Result<(u64, CheckpointStart), StartProblem>, DbError> {
    use crate::light::{checkpoint_first_header, checkpoint_start};

    let Some(sealed) = stored_header(db, block)? else {
        return Ok(Err(StartProblem::NotOnChain));
    };
    let height = sealed.header.height;

    if stored_header_at(db, height)?.map(|header| header.id()) != Some(*block) {
        return Ok(Err(StartProblem::NotOnChain));
    }

    let Some((epoch, from)) = checkpoint_start(params.finality, height) else {
        return Ok(Err(StartProblem::NoEpoch));
    };
    // Below the epochs' start, as many headers as consensus looks at when checking a header.
    let first = checkpoint_first_header(from, height, params.lookback());

    if height - first >= u64::from(crate::light::MAX_LIGHT_HEADERS) {
        return Ok(Err(StartProblem::TooMany));
    }

    let mut headers = Vec::new();

    for at in first..=height {
        match stored_header_at(db, at)? {
            Some(sealed) => headers.push(sealed),
            None => return Ok(Err(StartProblem::Missing(at))),
        }
    }

    let interval = params.finality.and_then(|finality| finality.checkpoint_interval().filter(|_| finality.fixed_ahead().is_some()));
    let updates = match interval {
        Some(interval) => headers[(from - first) as usize..]
            .iter()
            .step_by(interval as usize)
            .map(|header| stored_light_update(db, &header.id()))
            .collect::<Result<Vec<_>, _>>()?,
        None => Vec::new(),
    };

    Ok(Ok((epoch, CheckpointStart { headers, updates })))
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
    let Some(bytes) = db.read(Column::Receipts, &id_key(tx))? else {
        return Ok(None);
    };

    // Receipts stored before payments were kept lack the field.
    match wincode::deserialize::<TxReceipt>(&bytes) {
        Ok(receipt) => Ok(Some(receipt)),
        Err(_) => Ok(Some(
            wincode::deserialize::<receipt::LegacyTxReceipt>(&bytes)
                .map_err(|err| DbError::Corrupt(format!("receipt {tx}: {err}")))?
                .into(),
        )),
    }
}

#[document(
    summary = "The chainv4 id of the Ethereum-signed transaction whose signed bytes hash to this keccak256, once a block holding it ran here: on the main chain or a branch, which its receipt says.",
    category = "Chain",
    see_also = "Tx::Ethereum, stored_receipt",
)]
#[cfg(feature = "ethereum")]
pub fn stored_ethereum_tx(db: &dyn Read, hash: &[u8; 32]) -> Result<Option<Hash>, DbError> {
    read_value(db, Column::Ethereum, hash)
}

#[document(
    summary = "The ids of the approvals that decided governance's changes on the main chain, newest first: each with the version it made, from the one below before (or newest, the version decided last) down, at most limit. It stops at the first it lacks, as a node that synced from a snapshot lacks those decided before it.",
    category = "Governance",
    see_also = "TxStatus::Decided, stored_receipt",
)]
pub fn stored_decisions(db: &dyn Read, newest: u64, before: Option<u64>, limit: usize) -> Result<Vec<(u64, Hash)>, DbError> {
    let top = before.map_or(newest, |before| before.saturating_sub(1).min(newest));
    let mut found = Vec::new();

    // Versions count up one a decision, so each is read by its number.
    for version in (1..=top).rev().take(limit) {
        match read_value(db, Column::Decisions, &version.to_be_bytes())? {
            Some(id) => found.push((version, id)),
            None => break,
        }
    }

    Ok(found)
}

/// Lays what governance decided in a state over the rules a block at a height runs by, genesis's
/// coming in: the limits, where vote rewards come from, the host costs, names, the DEX's fee, the
/// EVM's spec at the height and governance's own settings. What the chain runs each block by, and
/// what the API answers dry runs and settings by, alike. Fees themselves, which are built and
/// kept, are the caller's.
pub(crate) fn govern_rules(rules: &mut Rules, state: &State, height: u64) -> Result<(), DbError> {
    let changes = Changes::new(state);

    if let Some(config) = changes.fees()? {
        // Where vote rewards come from moves with the fees, when they name it.
        if let Some(source) = config.vote_rewards {
            rules.pooled_fees = source == VoteRewardSource::Fees;
        }

        // A change of fees names its costs, or goes back to the defaults.
        rules.vm.costs = config.costs.unwrap_or_default();
        rules.vm.lua_costs = config.lua_costs.as_deref().copied().unwrap_or_default();
    }

    if let Some(engines) = changes.engines()? {
        rules.vm.engines = crate::params::vm::EngineSet::of(&engines);
    }

    if let Some(limits) = changes.limits()? {
        rules.limits = limits.tx_limits;
        // What a change of limits leaves unset stays genesis's.
        rules.vm.limits = limits.contract.unwrap_or(rules.vm.limits);
        rules.vm.max_tx_bytes = limits.max_tx_bytes.or(rules.vm.max_tx_bytes);
        rules.vm.multisig_members = limits.multisig_members.unwrap_or(rules.vm.multisig_members);
    }

    if let Some(settings) = changes.names()?
        && let Some(names) = &rules.names
    {
        rules.names = Some(Arc::new(names.with(&settings)));
    }

    if let Some(dex) = changes.dex()? {
        rules.vm.dex = dex;
    }

    if let Some(anchoring) = changes.anchoring()? {
        rules.anchoring = anchoring;
    }

    rules.vm.evm_spec = changes.evm_spec_at(height, rules.vm.evm_spec)?;
    // The forks in force at the height, as the schedule in state has them: genesis's, written at
    // the chain's start, or governance's since.
    rules.vm.forks = changes.forks()?.at(height);

    // Governance's own settings, from the block after the one that decided them.
    #[cfg(feature = "governance")]
    if let Some(governance) = &rules.governance {
        rules.governance = Some(governance.in_state(state)?);
    }

    Ok(())
}

/// Where the vote that made the chain's rejection-th is kept, in Column::Decisions beside the
/// decisions, whose keys are the 8 bytes of their count alone.
fn rejection_key(rejection: u64) -> Vec<u8> {
    [b"rejected:".as_slice(), &rejection.to_be_bytes()].concat()
}

#[document(
    summary = "The ids of the votes that rejected governance's changes on the main chain, newest first: each with the chain's count of rejections it made, from the one below before (or newest) down, at most limit. It stops at the first it lacks, as a node that synced from a snapshot lacks those made before it.",
    category = "Governance",
    see_also = "TxStatus::Rejected, stored_decisions",
)]
pub fn stored_rejections(db: &dyn Read, newest: u64, before: Option<u64>, limit: usize) -> Result<Vec<(u64, Hash)>, DbError> {
    let top = before.map_or(newest, |before| before.saturating_sub(1).min(newest));
    let mut found = Vec::new();

    for rejection in (1..=top).rev().take(limit) {
        match read_value(db, Column::Decisions, &rejection_key(rejection))? {
            Some(id) => found.push((rejection, id)),
            None => break,
        }
    }

    Ok(found)
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
