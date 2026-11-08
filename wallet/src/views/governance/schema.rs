//! Every field of every kind of change the propose form asks for: where it sits in the change's
//! JSON, what it is called, what control it takes, and the bounds the node holds it to. Plain data,
//! so the form draws it, reads it and compares it the same way for every kind, and a test checks
//! that it leaves no field of a change out.

use serde_json::{Value, json};

/// A choice's options: their values in the JSON, and what the form calls them.
pub type Options = &'static [(&'static str, &'static str)];

/// What a field of a table holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cell {
    Whole,
    Key,
    Account,
    Evm,
    /// An EVM address, or none when left empty.
    MaybeEvm,
    Amount,
}

/// One column of a table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Column {
    pub name: &'static str,
    pub label: &'static str,
    pub cell: Cell,
}

/// What one line of a list holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Line {
    /// A libp2p address.
    Bootnode,
    /// A contract's function, by name.
    Function,
    /// A kind of transaction, as a fee scorer names it.
    TxKind,
    /// A compressed secp256k1 key, 0x and 66 hex digits.
    Compressed,
    /// A price in the coin, by its place: names of 3 characters first.
    Price,
    /// An account here, such as a council's member.
    Account,
    /// A setting's path, as a lock names it: anchoring.enforce.
    Path,
    /// A DEX's fee tier, in hundredths of a percent: 1 to 1,000, written as a number.
    Tier,
}

/// The control a field takes, and how what is typed in it is written in the change's JSON.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    /// Text, trimmed.
    Text,
    /// A whole number, written as a number.
    Whole,
    /// A whole number that fits 32 bits.
    Small,
    /// A whole number of any size, written as a string.
    Digits,
    /// An amount typed in the coin, written as a string in its smallest unit.
    Amount,
    /// Thousandths, typed as they are and shown as a percentage.
    Permille,
    /// Milliseconds.
    Millis,
    /// A number of bytes, typed as bytes or as so many KiB or MiB.
    Bytes,
    /// A key on this chain.
    Key,
    /// A contract here.
    Contract,
    /// A contract here, or none.
    MaybeContract,
    /// An EVM address, or none.
    MaybeEvm,
    /// A version, such as 0.2.0.
    Version,
    /// One of a few.
    Choice(Options),
    /// Some of a few, at least one: written as a list of their names, in the order listed.
    Choices(Options),
    /// One of a few, or null, named off: what genesis set, say.
    MaybeChoice { off: &'static str, options: Options },
    /// A whole number, or null: on or off, named, with the number beside on.
    MaybeWhole { off: &'static str, on: &'static str },
    /// A number of bytes, or null, likewise.
    MaybeBytes { off: &'static str, on: &'static str },
    /// An object or null: on or off, the fields under it shown while on.
    Switch { off: &'static str, on: &'static str },
    /// True or false, named.
    Flag { off: &'static str, on: &'static str },
    /// A list, with lines to add and take away.
    List(Line),
    /// A list of objects, each with these fields.
    Table(&'static [Column]),
}

/// One field of a kind of change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Spec {
    /// Where it sits in the change's value, its keys joined by dots; empty for the value itself.
    pub path: &'static str,
    pub label: &'static str,
    /// What it is for, in a line; empty when its label says it.
    pub help: &'static str,
    pub control: Control,
    /// The heading it goes under, shared with the fields before it.
    pub group: &'static str,
    /// Shown only while each of these fields, by path, says this; "on" for a switch.
    pub when: &'static [(&'static str, &'static str)],
    /// Shown, never changed: what the chain keeps as it is.
    pub fixed: bool,
    /// The least and most the node takes, for a number.
    pub least: Option<u64>,
    pub most: Option<u64>,
}

const fn field(path: &'static str, label: &'static str, help: &'static str, control: Control) -> Spec {
    Spec { path, label, help, control, group: "", when: &[], fixed: false, least: None, most: None }
}

impl Spec {
    const fn under(self, group: &'static str) -> Spec {
        Spec { group, ..self }
    }

    const fn when(self, when: &'static [(&'static str, &'static str)]) -> Spec {
        Spec { when, ..self }
    }

    const fn fixed(self) -> Spec {
        Spec { fixed: true, ..self }
    }

    const fn least(self, least: u64) -> Spec {
        Spec { least: Some(least), ..self }
    }

    const fn most(self, most: u64) -> Spec {
        Spec { most: Some(most), ..self }
    }
}

const FORK_KINDS: Options = &[("schedule", "Schedule or move a fork"), ("cancel", "Cancel a fork"), ("lead", "Set the lead")];
const CONSENSUS_KINDS: Options = &[("pow", "Proof of work"), ("authority", "One key"), ("rotating", "Signers in turn"), ("staked", "Validators by stake")];
const SECURE_HASHES: Options = &[("blake3", "BLAKE3"), ("sha256", "SHA-256"), ("blake2s", "BLAKE2s"), ("blake2b", "BLAKE2b"), ("keccak256", "Keccak-256")];
const SCHEDULES: Options = &[("draw", "A weighted draw"), ("priority", "Priority rotation")];
const GAS_SCHEDULES: Options = &[("weighted", "Weights"), ("contract", "A contract scores")];
const PRICINGS: Options = &[("base_fee", "A base price"), ("bid", "Bids")];
const FINALITIES: Options = &[("ffg", "FFG"), ("authority", "Authority"), ("depth", "Depth")];
const VOTE_REWARD_SOURCES: Options = &[("mint", "Minted"), ("fees", "Reward pool")];
const SWITCHES: Options = &[("ffg", "FFG, by stake"), ("authority", "Authority, by signers")];
const GOVERNANCES: Options = &[("council", "A council"), ("electorate", "Finality's voters")];
const PROOF_KINDS: Options = &[("sp1-groth16", "SP1 in Groth16"), ("sp1-plonk", "SP1 in PLONK"), ("sp1-stark", "SP1's STARK, chain only")];
const GATEWAY_MODES: Options = &[("proof-or-federation", "A proof, or the federation's signatures"), ("both", "A proof and the federation's signatures"), ("federation-only", "The federation's signatures alone")];
const EVM_SPECS: Options = &[("shanghai", "Shanghai"), ("cancun", "Cancun"), ("prague", "Prague"), ("osaka", "Osaka")];

const VALIDATOR: &[Column] = &[Column { name: "account", label: "Account", cell: Cell::Account }, Column { name: "voter", label: "Voting key", cell: Cell::Key }, Column { name: "bond", label: "Bond", cell: Cell::Amount }, Column { name: "attester", label: "Attester, for Ethereum", cell: Cell::MaybeEvm }];
const SIGNER: &[Column] = &[Column { name: "key", label: "Voting key", cell: Cell::Key }, Column { name: "account", label: "Account", cell: Cell::Account }];
const EVM_GATEWAY: &[Column] = &[Column { name: "chain_id", label: "Chain id", cell: Cell::Whole }, Column { name: "gateway", label: "Gateway.sol", cell: Cell::Evm }];

const WHEN_POW: &[(&str, &str)] = &[("type", "pow")];
const WHEN_AUTHORITY: &[(&str, &str)] = &[("type", "authority")];
const WHEN_ROTATING: &[(&str, &str)] = &[("type", "rotating")];
const WHEN_STAKED: &[(&str, &str)] = &[("type", "staked")];
const WHEN_CONTRACT: &[(&str, &str)] = &[("schedule.type", "contract")];
const WHEN_BASE_FEE: &[(&str, &str)] = &[("pricing.type", "base_fee")];
const WHEN_BID: &[(&str, &str)] = &[("pricing.type", "bid")];
const WHEN_FFG: &[(&str, &str)] = &[("type", "ffg")];
const WHEN_DEPTH: &[(&str, &str)] = &[("type", "depth")];
const WHEN_LEAK: &[(&str, &str)] = &[("type", "ffg"), ("leak", "on")];
const WHEN_BTC_START: &[(&str, &str)] = &[("btc_start", "on")];
const BTC_NETWORKS: Options = &[("main", "Bitcoin"), ("test", "Testnet4 or signet"), ("regtest", "Regtest")];
const WHEN_ETH_START: &[(&str, &str)] = &[("eth_start", "on")];
const INTO_FFG: &[(&str, &str)] = &[("finality.type", "ffg")];
const INTO_AUTHORITY: &[(&str, &str)] = &[("finality.type", "authority")];
const INTO_LEAK: &[(&str, &str)] = &[("finality.type", "ffg"), ("finality.leak", "on")];
const WHEN_COUNCIL: &[(&str, &str)] = &[("type", "council")];
const WHEN_COSTS: &[(&str, &str)] = &[("costs", "on")];
const WHEN_BTC: &[(&str, &str)] = &[("btc", "on")];
const WHEN_CONTRACT_LIMITS: &[(&str, &str)] = &[("contract", "on")];

/// The least fuel and memory a transaction may have, as BlockLimits::problem says, and the least
/// and most a block may hold.
const LEAST_FUEL: u64 = 100_000;
const MIB: u64 = 1 << 20;

/// Every field of a kind of change, in the order the form lists them, each variant's fields among
/// them with what shows them.
pub fn schema(kind: &str) -> Vec<Spec> {
    match kind {
        "name" => vec![field("", "The chain's name", "What the apps call the network, from the block that decides it on.", Control::Text)],
        "bootnodes" => vec![field("", "Bootnodes", "Addresses new nodes dial first, at most 32. None leaves the chain without any.", Control::List(Line::Bootnode))],
        "upgrade" => vec![
            field("least", "Least version that runs on", "Nodes older than this stop and say to update.", Control::Version),
            field("from", "From block", "Must be ahead of the block that decides it.", Control::Whole),
        ],
        "proofs" => vec![
            field("kinds", "Proof systems", "The kinds of proof the chain takes, any of them. Groth16 and PLONK go to Ethereum too; the STARK needs no trusted setup but only the chain checks it. To switch, vote the new one in beside the old, then the old one out.", Control::Choices(PROOF_KINDS)),
            field("program", "Program", "Its verifying key hash, 0x and 64 hex digits, as chainv4 prove program prints it. Each release that runs blocks otherwise is another.", Control::Text),
            field("reward_per_block", "Reward a block proven", "Minted to whoever proves, for each block a proof covers. None pays nothing.", Control::Amount),
            field("gateway", "Gateway.sol releases by", "How withdrawals to EVM chains arrive: each proof carries this to Ethereum.", Control::Choice(GATEWAY_MODES)),
            field("finality_program", "Finality program", "What proves a link of finality from the voters' own votes, for ChainV4Finality.sol beside the attesters: chainv4 prove program --of finality. 0x and 64 zeros for none.", Control::Text),
            field("headers_program", "Header-chain program", "What proves the finalized checkpoints from genesis, for the apps to sync by: chainv4 prove program --of headers. 0x and 64 zeros for none.", Control::Text),
            field("reward_per_epoch", "Reward an epoch of header chain", "Minted to whoever proves the header chain, for each epoch a proof adds. None pays nothing.", Control::Amount),
        ],
        "fork" => vec![
            field("type", "What it does", "A fork changes what blocks compute from a height on. One within the lead of this vote, or passed, stays as it is.", Control::Choice(FORK_KINDS)),
            field("name", "Fork", "Its name, as the release that knows it calls it: scheduling it again moves it.", Control::Text).when(&[("type", "schedule")]),
            field("name", "Fork", "The one to cancel, not yet within the lead.", Control::Text).when(&[("type", "cancel")]),
            field("height", "From a block", "At least the chain's lead ahead: a day of blocks unless it set another. A block or an epoch, one of them.", Control::MaybeWhole { off: "Not by block", on: "From block" }).when(&[("type", "schedule")]),
            field("epoch", "From an epoch's first block", "", Control::MaybeWhole { off: "Not by epoch", on: "From epoch" }).when(&[("type", "schedule")]),
            field("least", "Least release that knows it", "Older nodes stop there, as for an upgrade.", Control::Version).when(&[("type", "schedule")]),
            field("blocks", "Lead, in blocks", "How far ahead a fork must be scheduled, moved or cancelled.", Control::Whole).when(&[("type", "lead")]).least(1),
        ],
        "names" => vec![
            field("prices", "What a year of a name costs", "By its length; the last price holds for every longer name.", Control::List(Line::Price)),
            field("grace_ms", "Grace after a name expires, in ms", "Its owner alone may renew it until then; a name in grace is judged by the grace in force.", Control::Millis),
            field("max_years", "Most years ahead a name is held", "", Control::Small).least(1),
        ],
        "token_functions" => vec![field("", "Functions a token exports", "A contract exporting all of them is one. None keeps no token registry.", Control::List(Line::Function))],
        "governance" => vec![
            field("type", "Kind", "Governance keeps its kind.", Control::Choice(GOVERNANCES)).fixed(),
            field("members", "Members", "Each once; they vote from the block after the decision, and every open vote is cleared.", Control::List(Line::Account)).when(WHEN_COUNCIL),
            field("threshold", "Members whose Yes decides", "At least one, at most all.", Control::Small).when(WHEN_COUNCIL).least(1),
            field("approval_epochs", "Epochs a vote counts", "", Control::Whole).least(1),
            field("quorum_permille", "Quorum, in thousandths", "How much of the voters' weight must vote at all, Abstain included, before Yes decides. 0 asks for none.", Control::Permille).most(1000),
        ],
        "evm_spec" => vec![
            field("spec", "Hard fork", "No older than the one in force, so no deployed contract loses an opcode.", Control::Choice(EVM_SPECS)),
            field("from", "From block", "Must be ahead of the block that decides it; blocks below replay as they ran.", Control::Whole),
        ],
        "dex" => vec![
            field("tiers", "Fee tiers, in hundredths of a percent", "30 is 0.3%. A new pool picks one, at most 4, smallest first. A pool keeps its tier when the list changes.", Control::List(Line::Tier)),
            field("protocol_permille", "Share of each trade's fee burned, in thousandths", "0 leaves it all to the pool's providers; 100 burns a tenth.", Control::Permille).most(1000),
        ],
        "anchoring" => vec![
            field("bitcoin", "Bitcoin anchors", "Whether the chain takes proofs of its checkpoints written into Bitcoin.", Control::Flag { off: "Off", on: "On" }),
            field("ethereum", "Ethereum anchors", "Whether the chain takes proofs of its checkpoints written into Ethereum, checked against the Ethereum relay.", Control::Flag { off: "Off", on: "On" }),
            field("enforce", "Enforced", "Anchors recorded from now on bind nodes: no block replaces them, and new nodes sync by them. Those recorded while on stay so.", Control::Flag { off: "Off", on: "On" }),
            field("relay", "Bitcoin relay", "The relay contract proofs are checked against.", Control::MaybeContract),
            field("btc_start", "Where the relay started", "Nodes checking anchors on their own check Bitcoin's headers from here.", Control::Switch { off: "Not set", on: "This header" }),
            field("btc_start.header", "Header", "80 bytes, as 0x hex.", Control::Text).when(WHEN_BTC_START),
            field("btc_start.height", "Its height", "On Bitcoin itself, divisible by 2016.", Control::Whole).when(WHEN_BTC_START),
            field("btc_start.network", "Network", "", Control::Choice(BTC_NETWORKS)).when(WHEN_BTC_START),
            field("eth_relay", "Ethereum relay", "The eth_relay contract Ethereum proofs are checked against.", Control::MaybeContract),
            field("eth_start", "Where the Ethereum relay started", "Nodes checking Ethereum anchors on their own follow sync committees from here.", Control::Switch { off: "Not set", on: "This root" }),
            field("eth_start.root", "Beacon block root", "The trusted root the relay started at: 0x and 64 hex digits.", Control::Text).when(WHEN_ETH_START),
            field("eth_start.network", "Beacon network", "mainnet, sepolia or hoodi, or a network's whole definition as 0x SSZ: a fork table of its own, or a chain no build has.", Control::Text).when(WHEN_ETH_START),
            field("depth", "Bitcoin blocks deep", "Before an anchor in one counts: 1 is the tip.", Control::Whole).least(1).most(144),
            field("every", "Epochs between rewards", "Anchors in between are recorded, unpaid.", Control::Whole).least(1),
            field("reward", "Reward", "For the first proof of a rewarded anchor.", Control::Amount),
        ],
        "consensus" => vec![
            field("type", "Who makes blocks", "It must fit the chain's finality: signers in turn need authority, validators by stake need FFG.", Control::Choice(CONSENSUS_KINDS)),
            field("hash", "Work hash", "", Control::Choice(SECURE_HASHES)).when(WHEN_POW),
            field("target_block_ms", "Time between blocks, in ms", "Difficulty rises when blocks come faster, and falls when they come slower.", Control::Millis).when(WHEN_POW),
            field("initial_difficulty", "First block's difficulty", "The hashes expected to find its seal.", Control::Digits).when(WHEN_POW),
            field("min_difficulty", "Least difficulty", "Difficulty never falls below this.", Control::Digits).when(WHEN_POW),
            field("window", "Retarget window", "Genesis's, or over so many blocks, at least 2.", Control::MaybeWhole { off: "Genesis's", on: "Blocks" }).when(WHEN_POW).least(2),
            field("adjustment", "Step of difficulty", "Each step moves it by the parent's divided by this, 64 by default.", Control::MaybeWhole { off: "Default", on: "Divide by" }).when(WHEN_POW).least(2),
            field("max_steps_down", "Most steps down in a block", "32 by default.", Control::MaybeWhole { off: "Default", on: "At most" }).when(WHEN_POW).least(1),
            field("max_swing", "Most an interval counts", "Under a window, from a quarter to four times the target by default.", Control::MaybeWhole { off: "Default", on: "Times the target" }).when(WHEN_POW).least(1),
            field("key", "The key that makes blocks", "", Control::Key).when(WHEN_AUTHORITY),
            field("slot_ms", "Time between blocks, in ms", "", Control::Millis).when(WHEN_AUTHORITY),
            field("slot_ms", "Time between blocks, in ms", "Also from a missed slot to the next in line's.", Control::Millis).when(WHEN_ROTATING),
            field("slot_ms", "Time between blocks, in ms", "Also from a missed slot to the next in line's.", Control::Millis).when(WHEN_STAKED),
            field("schedule", "How a slot's maker is picked", "", Control::Choice(SCHEDULES)).when(WHEN_STAKED),
        ],
        "fees" => vec![
            field("block_gas_limit", "Gas a block may use", "", Control::Whole).least(1),
            field("burn_permille", "Share of each fee burned, in thousandths", "Of a fee's base part: 500 burns half, and the rest goes to the block's maker.", Control::Permille).most(1000),
            field("vote_rewards", "Where vote rewards come from", "Minted new, or paid from the reward pool fees fill, from the block after the one that decides it. The pool keeps what it holds either way.", Control::MaybeChoice { off: "As genesis", options: VOTE_REWARD_SOURCES }),
            field("schedule.type", "How gas is counted", "", Control::Choice(GAS_SCHEDULES)).under("Gas schedule"),
            field("schedule.per_tx", "Gas per transaction", "", Control::Whole).under("Gas schedule"),
            field("schedule.per_byte", "Gas per byte", "", Control::Whole).under("Gas schedule"),
            field("schedule.per_code_byte", "Gas per byte of code", "", Control::Whole).under("Gas schedule"),
            field("schedule.fuel_per_gas", "Fuel per gas", "", Control::Whole).under("Gas schedule").least(1),
            field("schedule.contract", "The scoring contract", "", Control::Contract).under("Gas schedule").when(WHEN_CONTRACT),
            field("schedule.fuel", "Fuel it may use", "", Control::Whole).under("Gas schedule").when(WHEN_CONTRACT).least(1),
            field("schedule.kinds", "Kinds of transaction it scores", "None scores every kind.", Control::List(Line::TxKind)).under("Gas schedule").when(WHEN_CONTRACT),
            field("pricing.type", "How gas is priced", "", Control::Choice(PRICINGS)).under("Pricing"),
            field("pricing.initial", "Price to start from", "Only a chain turning fees on starts from it; one with fees carries on from its price.", Control::Amount).under("Pricing").when(WHEN_BASE_FEE),
            field("pricing.min", "Least price", "", Control::Amount).under("Pricing").when(WHEN_BASE_FEE),
            field("pricing.elasticity", "Elasticity", "The gas limit divided by this is the target a block aims for: 2 aims for half-full blocks.", Control::Whole).under("Pricing").when(WHEN_BASE_FEE).least(1),
            field("pricing.change_denominator", "Change denominator", "The price moves by at most one over this a block: 8 is an eighth.", Control::Whole).under("Pricing").when(WHEN_BASE_FEE).least(1),
            field("pricing.min_price", "Least bid", "", Control::Amount).under("Pricing").when(WHEN_BID),
            field("costs", "What host operations cost", "In fuel, and the EVM precompiles in gas.", Control::Switch { off: "The defaults", on: "These" }).under("Host costs"),
            field("costs.storage_read", "Storage read", "", Control::Whole).under("Host costs").when(WHEN_COSTS),
            field("costs.storage_write", "Storage write", "", Control::Whole).under("Host costs").when(WHEN_COSTS),
            field("costs.storage_bytes_per_fuel", "Storage bytes per fuel", "", Control::Whole).under("Host costs").when(WHEN_COSTS).least(1),
            field("costs.storage_scan", "Storage scan", "", Control::Whole).under("Host costs").when(WHEN_COSTS),
            field("costs.storage_scan_entry", "Storage scan entry", "", Control::Whole).under("Host costs").when(WHEN_COSTS),
            field("costs.hash", "Hash", "", Control::Whole).under("Host costs").when(WHEN_COSTS),
            field("costs.hash_bytes_per_fuel", "Hash bytes per fuel", "", Control::Whole).under("Host costs").when(WHEN_COSTS).least(1),
            field("costs.call", "Call", "", Control::Whole).under("Host costs").when(WHEN_COSTS),
            field("costs.balance", "Balance", "", Control::Whole).under("Host costs").when(WHEN_COSTS),
            field("costs.transfer", "Transfer", "", Control::Whole).under("Host costs").when(WHEN_COSTS),
            field("costs.event", "Event", "", Control::Whole).under("Host costs").when(WHEN_COSTS),
            field("costs.event_bytes_per_fuel", "Event bytes per fuel", "", Control::Whole).under("Host costs").when(WHEN_COSTS).least(1),
            field("costs.copy_bytes_per_fuel", "Copy bytes per fuel", "", Control::Whole).under("Host costs").when(WHEN_COSTS).least(1),
            field("costs.load_byte", "Load byte", "", Control::Whole).under("Host costs").when(WHEN_COSTS),
            field("costs.token_refresh", "Token refresh", "", Control::Whole).under("Host costs").when(WHEN_COSTS),
            field("costs.check_layout", "Check layout", "", Control::Whole).under("Host costs").when(WHEN_COSTS),
            field("costs.clone", "Clone", "", Control::Whole).under("Host costs").when(WHEN_COSTS),
            field("costs.name", "Name", "", Control::Whole).under("Host costs").when(WHEN_COSTS),
            field("costs.abi", "Abi", "", Control::Whole).under("Host costs").when(WHEN_COSTS),
            field("costs.verify", "Verify", "", Control::Whole).under("Host costs").when(WHEN_COSTS),
            field("costs.recover", "Recover", "", Control::Whole).under("Host costs").when(WHEN_COSTS),
            field("costs.key_address", "Key address", "", Control::Whole).under("Host costs").when(WHEN_COSTS),
            field("costs.check_proof", "Check proof", "", Control::Whole).under("Host costs").when(WHEN_COSTS),
            field("costs.proof_sibling", "Proof sibling", "", Control::Whole).under("Host costs").when(WHEN_COSTS),
            field("costs.federation", "Federation", "", Control::Whole).under("Host costs").when(WHEN_COSTS),
            field("costs.chain_id", "Chain id", "", Control::Whole).under("Host costs").when(WHEN_COSTS),
            field("costs.evm_native_call", "Evm native call", "", Control::Whole).under("Host costs").when(WHEN_COSTS),
            field("costs.evm_native_word", "Evm native word", "", Control::Whole).under("Host costs").when(WHEN_COSTS),
            field("costs.evm_name_lookup", "Evm name lookup", "", Control::Whole).under("Host costs").when(WHEN_COSTS),
            field("costs.bls_verify", "BLS verify", "An Ethereum aggregate signature's check, before its keys.", Control::Whole).under("Host costs").when(WHEN_COSTS),
            field("costs.bls_key", "BLS key", "Each key in it.", Control::Whole).under("Host costs").when(WHEN_COSTS),
        ],
        "reward" => vec![
            field("initial", "What a block mints", "From the block after the one that decides it.", Control::Amount),
            field("halving_interval", "Halving", "Counted from there, not from genesis.", Control::MaybeWhole { off: "Never", on: "Every so many blocks" }).least(1),
        ],
        "limits" => vec![
            field("tx_limits.fuel", "Fuel a transaction may use", "", Control::Whole).least(LEAST_FUEL),
            field("tx_limits.memory", "Memory a transaction may use", "", Control::Bytes).least(MIB),
            field("max_block_bytes", "Size of a block", "Bound by gas alone, or at most 1 MiB to 6 MiB.", Control::MaybeBytes { off: "Gas alone", on: "At most" }).least(MIB).most(6 * MIB),
            field("max_tx_bytes", "Size of a transaction", "Genesis's, or at most 64 KiB to 6 MiB; Ethereum-signed ones by their raw bytes.", Control::MaybeBytes { off: "Genesis's", on: "At most" }).least(64 * 1024).most(6 * MIB),
            field("multisig_members", "Members of a multisig", "Genesis's, or at most 1 to 20; a multisig made under more keeps its members.", Control::MaybeWhole { off: "Genesis's", on: "At most" }).least(1).most(20),
            field("contract", "Contract limits", "Lowering what deployed contracts may rely on is refused.", Control::Switch { off: "Genesis's", on: "These" }).under("Contracts"),
            field("contract.call_depth", "Deepest calls nest", "", Control::Small).under("Contracts").when(WHEN_CONTRACT_LIMITS).least(1).most(64),
            field("contract.key_len", "Longest storage key", "", Control::Bytes).under("Contracts").when(WHEN_CONTRACT_LIMITS).least(1),
            field("contract.value_len", "Largest storage value", "", Control::Bytes).under("Contracts").when(WHEN_CONTRACT_LIMITS).least(1),
            field("contract.scan", "Most entries a scan gives", "", Control::Small).under("Contracts").when(WHEN_CONTRACT_LIMITS).least(1),
            field("contract.event_data", "Largest event data", "", Control::Bytes).under("Contracts").when(WHEN_CONTRACT_LIMITS).least(1),
            field("contract.events", "Most events a call emits", "", Control::Small).under("Contracts").when(WHEN_CONTRACT_LIMITS).least(1),
            field("contract.module_len", "Largest WebAssembly module", "Bounds only new deploys.", Control::Bytes).under("Contracts").when(WHEN_CONTRACT_LIMITS).least(1).most(512 * 1024),
            field("contract.source_len", "Longest Lua source", "Bounds only new deploys.", Control::Bytes).under("Contracts").when(WHEN_CONTRACT_LIMITS).least(1).most(512 * 1024),
        ],
        "finality" => vec![
            field("type", "Kind", "A switch changes it: pick Finality's kind.", Control::Choice(FINALITIES)).fixed(),
            field("checkpoint_interval", "Blocks per epoch", "Every light client's epoch arithmetic rests on it.", Control::Whole).fixed().when(WHEN_FFG),
            field("checkpoint_interval", "Blocks per epoch", "Every light client's epoch arithmetic rests on it.", Control::Whole).fixed().when(WHEN_AUTHORITY),
            field("activation_delay", "Epochs a change waits", "", Control::Whole).fixed().when(WHEN_FFG),
            field("activation_delay", "Epochs a change waits", "", Control::Whole).fixed().when(WHEN_AUTHORITY),
            field("vote_depth", "Blocks on a checkpoint before voting", "", Control::Whole).when(WHEN_FFG),
            field("min_bond", "Smallest bond", "", Control::Amount).when(WHEN_FFG),
            field("withdrawal_delay", "Epochs a bond stays locked", "After its validator stops voting.", Control::Whole).when(WHEN_FFG),
            field("reporter_divisor", "Reporter's divisor", "Who reports cheating gets the bond divided by this; the rest burns.", Control::Whole).when(WHEN_FFG).least(1),
            field("max_validators", "Most validators", "", Control::Small).when(WHEN_FFG),
            field("vote_reward", "What each voter earns", "For a justification a block carries.", Control::Amount).when(WHEN_FFG),
            field("max_proofs_per_block", "Most justifications a block carries", "", Control::Small).when(WHEN_FFG),
            field("validators", "Validators from genesis", "History keeps them.", Control::Table(VALIDATOR)).fixed().when(WHEN_FFG),
            field("slash_permille", "What a slash takes", "Of a cheater's bond, in thousandths; the reporter gets its share of that.", Control::MaybeWhole { off: "The whole bond", on: "Thousandths" }).when(WHEN_FFG).least(1).most(1000),
            field("justify_permille", "Weight that justifies a link", "Of the bonded weight, in thousandths: from 667, two thirds, up to 1000, all of it. Higher is safer but stalls sooner when validators go quiet. From the proofs fork on; Ethereum's attestations follow it.", Control::MaybeWhole { off: "Two thirds", on: "Thousandths" }).when(WHEN_FFG).least(667).most(1000),
            field("leak", "Inactivity leak", "Takes bonds from validators who stopped voting once nothing was proven for a while.", Control::Switch { off: "None", on: "A leak" }).when(WHEN_FFG),
            field("leak.after_epochs", "After this many epochs without a proof", "", Control::Whole).when(WHEN_LEAK).least(1),
            field("leak.permille", "Thousandths of a bond each checkpoint", "", Control::Permille).when(WHEN_LEAK).most(1000),
            field("vote_depth", "Blocks on a checkpoint before voting", "", Control::Whole).when(WHEN_AUTHORITY),
            field("max_signers", "Most signers", "", Control::Small).when(WHEN_AUTHORITY),
            field("signers", "Signers from genesis", "History keeps them; they change by their own approval.", Control::Table(SIGNER)).fixed().when(WHEN_AUTHORITY),
            field("threshold", "Signers that justify a checkpoint", "They change by their own approval.", Control::Small).fixed().when(WHEN_AUTHORITY),
            field("change_threshold", "Signers that change the set", "", Control::Small).fixed().when(WHEN_AUTHORITY),
            field("approval_epochs", "Epochs a signer's approval counts", "", Control::Whole).when(WHEN_AUTHORITY).least(1),
            field("vote_reward", "What each signer earns", "For a justification a block carries.", Control::Amount).when(WHEN_AUTHORITY),
            field("max_proofs_per_block", "Most justifications a block carries", "", Control::Small).when(WHEN_AUTHORITY),
            field("depth", "Blocks on a block before it is final", "", Control::Whole).when(WHEN_DEPTH),
        ],
        "switch_finality" => vec![
            field("epoch", "First epoch the new kind runs", "It runs from here, or from the earliest the chain allows, whichever is later.", Control::Whole),
            field("finality.type", "Switch to", "Goes between FFG and authority; a chain keeps its epochs.", Control::Choice(SWITCHES)).fixed(),
            field("finality.checkpoint_interval", "Blocks per epoch", "A switch keeps genesis's.", Control::Whole).fixed(),
            field("finality.activation_delay", "Epochs a change waits", "A switch keeps genesis's.", Control::Whole).fixed(),
            field("finality.vote_depth", "Blocks on a checkpoint before voting", "", Control::Whole),
            field("finality.min_bond", "Smallest bond", "Validators bond from the block that decides the switch: name none here.", Control::Amount).when(INTO_FFG),
            field("finality.withdrawal_delay", "Epochs a bond stays locked", "After its validator stops voting.", Control::Whole).when(INTO_FFG),
            field("finality.reporter_divisor", "Reporter's divisor", "Who reports cheating gets the bond divided by this; the rest burns.", Control::Whole).when(INTO_FFG).least(1),
            field("finality.max_validators", "Most validators", "", Control::Small).when(INTO_FFG),
            field("finality.justify_permille", "Weight that justifies a link", "Of the bonded weight, in thousandths, 667 to 1000: two thirds unless set.", Control::MaybeWhole { off: "Two thirds", on: "Thousandths" }).when(INTO_FFG).least(667).most(1000),
            field("finality.leak", "Inactivity leak", "Takes bonds from validators who stopped voting once nothing was proven for a while.", Control::Switch { off: "None", on: "A leak" }).when(INTO_FFG),
            field("finality.leak.after_epochs", "After this many epochs without a proof", "", Control::Whole).when(INTO_LEAK).least(1),
            field("finality.leak.permille", "Thousandths of a bond each checkpoint", "", Control::Permille).when(INTO_LEAK).most(1000),
            field("finality.max_signers", "Most signers", "", Control::Small).when(INTO_AUTHORITY),
            field("finality.signers", "Signers", "They vote from the switch epoch.", Control::Table(SIGNER)).when(INTO_AUTHORITY),
            field("finality.threshold", "Signers that justify a checkpoint", "More than half of them.", Control::Small).when(INTO_AUTHORITY),
            field("finality.change_threshold", "Signers that change the set", "More than half of them.", Control::Small).when(INTO_AUTHORITY),
            field("finality.approval_epochs", "Epochs a signer's approval counts", "", Control::Whole).when(INTO_AUTHORITY).least(1),
            field("finality.vote_reward", "What each voter earns", "For a justification a block carries.", Control::Amount),
            field("finality.max_proofs_per_block", "Most justifications a block carries", "", Control::Small),
        ],
        "collections" => vec![
            field("nft", "Functions an NFT collection exports", "A contract exporting all of them is one.", Control::List(Line::Function)),
            field("multi", "Functions a multi-token collection exports", "", Control::List(Line::Function)),
        ],
        "federation" => vec![
            field("signers", "Signers", "Who vouches for the bridges: 1 to 20 secp256k1 keys, each once.", Control::List(Line::Compressed)),
            field("threshold", "Signers that must agree", "", Control::Small).least(1),
            field("chain_depth", "Blocks to wait here", "", Control::Whole).least(1),
            field("evm_depth", "Blocks to wait on EVM chains", "Where the chain's RPC names no finalized block.", Control::Whole).least(1),
            field("bitcoin_depth", "Confirmations to wait on Bitcoin", "", Control::Small).least(1),
            field("btc", "Bitcoin policy", "What the Bitcoin gateway and its signers go by; while unset, what the gateway was set up with.", Control::Switch { off: "The gateway's own", on: "These" }).under("Bitcoin"),
            field("btc.fee_cap", "Most fee a release pays, in sats", "", Control::Whole).under("Bitcoin").when(WHEN_BTC).least(710),
            field("btc.fee_floor", "Least a release pays a vbyte, in sats", "", Control::Whole).under("Bitcoin").when(WHEN_BTC).least(1),
            field("btc.grace_ms", "How long a retired federation's addresses stay open, in ms", "", Control::Millis).under("Bitcoin").when(WHEN_BTC),
            field("btc.cancel_after_ms", "How long a release waits before its signers vote it down, in ms", "", Control::Millis).under("Bitcoin").when(WHEN_BTC).least(1),
            field("btc.fee_part", "A withdrawal's fee is at most its sats divided by", "10 is a tenth.", Control::Whole).under("Bitcoin").when(WHEN_BTC).least(1),
            field("btc.bump_after_ms", "How long a signed release goes unmined before its signers bump it, in ms", "And its bump again before they bump that.", Control::Millis).under("Bitcoin").when(WHEN_BTC).least(1),
            field("btc.bump_below_pct", "Bumped when it pays under this percent of what Bitcoin asks", "80 bumps one paying under four fifths of the rate.", Control::Whole).under("Bitcoin").when(WHEN_BTC).least(1).most(100),
            field("btc.repay_pct", "A release repays what children cost, up to this percent of its fee", "0 never repays.", Control::Whole).under("Bitcoin").when(WHEN_BTC).most(100),
            field("btc.repay_most", "The most one withdrawal repays of it, in sats", "Within what it agreed to lose.", Control::Whole).under("Bitcoin").when(WHEN_BTC),
        ],
        "attestations" => vec![field("", "Validators attest for Ethereum", "Off, no validator attests links any more and blocks carry no attestations, so ChainV4Finality.sol on Ethereum follows the chain no further. Not while the proofs' gateway mode is both.", Control::Flag { off: "Off", on: "On" })],
        "locks" => vec![field("", "Paths to lock", "Each a setting no vote may change from then on, such as reward or anchoring.enforce. Nothing unlocks one, and a lock on locks freezes the set.", Control::List(Line::Path))],
        "directory" => vec![
            field("gateway", "The bridges' gateway", "", Control::MaybeContract),
            field("btc_gateway", "The Bitcoin gateway", "", Control::MaybeContract),
            field("evm_gateways", "Gateway.sol on EVM chains", "At most 32, each chain once.", Control::Table(EVM_GATEWAY)),
            field("swap", "The swap contract", "", Control::MaybeContract),
            field("dex", "The DEX", "", Control::MaybeContract),
            field("dex_router", "The EVM DEX's router", "", Control::MaybeEvm),
            field("dex_split", "The split router, router.lua", "", Control::MaybeContract),
            field("nft_factory", "The NFT factory", "", Control::MaybeContract),
        ],
        _ => Vec::new(),
    }
}

/// What a field starts from where nothing stands for it: a chain turning a setting on, or a variant
/// it does not run now.
pub fn defaults(kind: &str) -> Value {
    match kind {
        "bootnodes" | "token_functions" | "locks" => json!([]),
        "names" => json!({ "prices": [], "grace_ms": 7_776_000_000u64, "max_years": 100 }),
        "evm_spec" => json!({ "spec": "prague", "from": null }),
        "dex" => json!({ "tiers": [5, 30, 100], "protocol_permille": 0 }),
        "anchoring" => json!({ "bitcoin": false, "ethereum": false, "enforce": false, "relay": null, "btc_start": null, "eth_relay": null, "eth_start": null, "depth": 6, "every": 90, "reward": "0" }),
        "name" => json!(""),
        "upgrade" => json!({ "least": "", "from": null }),
        "fork" => json!({ "type": "schedule", "name": "", "height": null, "epoch": null, "least": "", "blocks": null }),
        "proofs" => json!({ "kinds": ["sp1-groth16"], "program": "", "reward_per_block": "0", "gateway": "proof-or-federation", "finality_program": format!("0x{}", "00".repeat(32)), "headers_program": format!("0x{}", "00".repeat(32)), "reward_per_epoch": "0" }),
        "attestations" => json!(true),
        "consensus" => json!({ "type": "pow", "hash": "blake3", "target_block_ms": 2000, "initial_difficulty": "100000", "min_difficulty": "1000", "key": "", "slot_ms": 2000, "schedule": "draw" }),
        "fees" => json!({
            "block_gas_limit": 50_000_000,
            "burn_permille": 500,
            "schedule": { "type": "weighted", "per_tx": 1000, "per_byte": 10, "per_code_byte": 20, "fuel_per_gas": 1, "contract": "", "fuel": 1_000_000, "kinds": [] },
            "pricing": { "type": "base_fee", "initial": "1000000000", "min": "10000000", "elasticity": 2, "change_denominator": 8, "min_price": "10000000" },
        }),
        "reward" => json!({ "initial": "0", "halving_interval": null }),
        "limits" => json!({ "tx_limits": { "fuel": 10_000_000, "memory": 64 * MIB }, "max_block_bytes": null }),
        "finality" => json!({ "depth": 12 }),
        "switch_finality" => json!({
            "epoch": null,
            "finality": {
                "type": "authority",
                "checkpoint_interval": null,
                "activation_delay": null,
                "vote_depth": 2,
                "min_bond": "100000000000000000000",
                "withdrawal_delay": 64,
                "reporter_divisor": 16,
                "max_validators": 1000,
                "leak": null,
                "max_signers": 100,
                "signers": [],
                "threshold": 1,
                "change_threshold": 1,
                "approval_epochs": 64,
                "vote_reward": "0",
                "max_proofs_per_block": 4,
            },
        }),
        "collections" => json!({ "nft": [], "multi": [] }),
        "federation" => json!({ "signers": [], "threshold": 1, "chain_depth": 20, "evm_depth": 12, "bitcoin_depth": 6 }),
        "directory" => json!({ "gateway": null, "btc_gateway": null, "evm_gateways": [], "swap": null, "dex": null, "dex_router": null, "dex_split": null, "nft_factory": null }),
        _ => Value::Null,
    }
}

/// The objects in a kind whose type picks their fields: where another type replaces the object
/// whole, as the fields of the one standing mean nothing to it.
pub fn variants(kind: &str) -> &'static [&'static str] {
    match kind {
        "consensus" | "fork" => &[""],
        "fees" => &["schedule", "pricing"],
        "switch_finality" => &["finality"],
        _ => &[],
    }
}
