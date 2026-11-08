//! What a node's HTTP API answers, as the explorer reads it. Amounts and wide integers stay the
//! decimal strings the API writes; the explorer only shows them.

use {
    serde::Deserialize,
    serde_json::Value,
    std::collections::BTreeMap,
};

/// GET /status.
#[derive(Clone, Debug, Deserialize)]
pub struct Status {
    pub genesis: String,
    pub tip: String,
    pub height: u64,
    pub timestamp: u64,
    pub difficulty: String,
    pub work: String,
    pub hash: String,
    pub coin: Coin,
    /// The chain's name, from genesis or governance, such as chainv4 testnet; none for a chain
    /// without one, and from older nodes.
    #[serde(default)]
    pub name: Option<String>,
    /// The ending every name on the chain has, such as dev for alice.dev; none on a chain without
    /// names, and from older nodes.
    #[serde(default)]
    pub names_suffix: Option<String>,
    /// The functions that make a contract a token on this chain; none keeps no registry.
    #[serde(default)]
    pub token_functions: Vec<String>,
    /// The fee tiers a new pool may open at, in hundredths of a percent; none from older nodes,
    /// which had the one fee below.
    #[serde(default)]
    pub dex_tiers: Option<Vec<u32>>,
    /// The one fee older nodes charged every trade.
    #[serde(default)]
    pub dex_fee_bps: Option<u32>,
    /// The thousandths of each trade's fee burned; none from older nodes, which burned none.
    #[serde(default)]
    pub dex_protocol_permille: Option<u32>,
    /// The EVM's spec for the next block, such as prague; none from older nodes. What contract
    /// reads run here go by, off the browser.
    #[serde(default)]
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub evm_spec: Option<String>,
    pub finality: Option<Finality>,
    /// The fee settings as genesis files write them, with base_price and tx_fuel.
    pub fees: Option<Value>,
    /// Where the chain's nodes find their first peers, from genesis or governance; none from older
    /// nodes.
    #[serde(default)]
    pub bootnodes: Vec<String>,
    /// The node's faucet, on a node that runs one: who pays, how much, how often and how much
    /// work a request carries.
    #[serde(default)]
    pub faucet: Option<Value>,
    /// The swap contract the node names: { contract }, or none.
    #[serde(default)]
    pub swap: Option<Value>,
    /// The DEX and the EVM DEX's router the node names: { contract, router }, or none.
    #[serde(default)]
    pub dex: Option<Value>,
    /// What makes a collection on this chain: { nft, multi } lists of functions, or none for a
    /// chain that keeps no collection registry.
    #[serde(default)]
    pub collections: Option<Value>,
    /// The contracts the chain announces: the bridges' gateways, the swap contract, the DEX and
    /// its router, and the NFT factory, each null while there is none; or none at all.
    #[serde(default)]
    pub directory: Option<Value>,
    /// The bridges' federation in force, with its version, or none.
    #[serde(default)]
    pub federation: Option<Value>,
    /// The chain's id as the bridges name it: what its EVM answers CHAINID with.
    #[serde(default)]
    pub chain_id: Option<u64>,
    /// The node's chainv4, such as 0.1.0; none from older nodes.
    #[serde(default)]
    pub version: Option<String>,
    /// The upgrade governance decided, if any: from a height on, nodes older than a version stop.
    #[serde(default)]
    pub upgrade: Option<UpgradeView>,
    /// The fork schedule: none from older nodes.
    #[serde(default)]
    pub forks: Option<ForksView>,
    /// Validity proofs: governance's settings, the proven tip and the latest proof taken, which
    /// the explorer checks itself; none from older nodes.
    #[serde(default)]
    pub validity: Option<Value>,
}

/// The chain's fork schedule, as /status says it: how far ahead one must be scheduled, and each
/// scheduled.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct ForksView {
    #[serde(default)]
    pub lead: u64,
    #[serde(default)]
    pub scheduled: Vec<ForkView>,
}

impl ForksView {
    /// The schedule, as the explorer's own reads run by it: on the desktop alone, as the browser
    /// runs none.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub fn schedule(&self) -> chainv4::params::forks::ForkSchedule {
        let forks = self.scheduled.iter().map(|fork| chainv4::params::forks::ScheduledFork { name: fork.name.clone(), height: fork.height, epoch: fork.epoch, least: None }).collect();

        chainv4::params::forks::ForkSchedule { forks, lead: Some(self.lead) }
    }
}

/// A scheduled fork: its name, its first block, its epoch if scheduled by one, whether it has
/// passed, whether the node's release knows it, and what it changes, as the node's release says.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct ForkView {
    pub name: String,
    pub height: u64,
    #[serde(default)]
    pub epoch: Option<u64>,
    #[serde(default)]
    pub passed: bool,
    #[serde(default)]
    pub known: bool,
    #[serde(default)]
    pub summary: Option<String>,
}

/// What the explorer calls a chain that names itself nothing, or before it knows.
pub const UNNAMED: &str = "chainv4";

/// The chain's name to show: the one it gives, if it gives one that is not blank, else
/// [`UNNAMED`].
pub fn chain_name(name: Option<&str>) -> &str {
    name.map(str::trim).filter(|name| !name.is_empty()).unwrap_or(UNNAMED)
}

impl Status {
    /// The DEXes' settings in force, as the node says them: the defaults where it says nothing.
    pub fn dex(&self) -> chainv4::params::vm::DexSettings {
        let mut status = serde_json::json!({ "dex_protocol_permille": self.dex_protocol_permille });

        match (&self.dex_tiers, self.dex_fee_bps) {
            (Some(tiers), _) => status["dex_tiers"] = serde_json::json!(tiers),
            (None, Some(fee)) => status["dex_fee_bps"] = serde_json::json!(fee),
            (None, None) => {}
        }

        chainv4::params::json::dex_from_status(&status)
    }

    /// The chain's name to show, [`UNNAMED`] for a chain without one.
    pub fn chain_name(&self) -> &str {
        chain_name(self.name.as_deref())
    }

    /// A contract the chain's directory names, by its key there, such as swap or gateway.
    pub fn announced(&self, key: &str) -> Option<String> {
        self.directory.as_ref()?.get(key)?.as_str().map(str::to_string)
    }

    /// What a transaction sent now pays per gas at least, on a chain with fees.
    pub fn base_price(&self) -> Option<&str> {
        self.fees.as_ref()?.get("base_price")?.as_str()
    }

    /// The most gas one block holds, on a chain with fees.
    pub fn block_gas_limit(&self) -> Option<u64> {
        self.fees.as_ref()?.get("block_gas_limit")?.as_u64()
    }
}

/// GET /release: the newest release the node took, as signed, for the explorer to check against
/// its own release keys, and where the node's own chainv4 stands against it. Older nodes answer
/// 404.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct NodeRelease {
    /// The node's chainv4.
    pub version: String,
    /// Whether the node fetches releases at all: a build without release keys does not.
    #[serde(default)]
    pub updates: bool,
    /// The manifest, as its keys signed it, or null while the node has none. The node only
    /// relays it: the explorer checks it itself.
    #[serde(default)]
    pub manifest: Value,
    /// The node's chainv4 against the newest: current, behind, unsupported or unknown.
    #[serde(default)]
    pub standing: String,
    /// The newest chainv4, when the node is behind it or too old.
    #[serde(default)]
    pub newest: Option<String>,
    /// Why the node's last fetch of the manifest failed, if it did.
    #[serde(default)]
    pub error: Option<String>,
    /// The version the node put in place of itself, which runs once it restarts.
    #[serde(default)]
    pub installed: Option<String>,
    /// The upgrade that stops the node first, governance's or a release's.
    #[serde(default)]
    pub upgrade: Option<UpgradeView>,
}

/// An upgrade: from block from on, nodes older than least stop.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct UpgradeView {
    pub least: String,
    pub from: u64,
    /// Who decided it: governance, or a release, for a chain without governance. /status's says
    /// none, being governance's.
    #[serde(default)]
    pub source: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Coin {
    pub symbol: String,
    pub decimals: u8,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Finality {
    #[serde(rename = "type")]
    pub kind: String,
    pub epoch: u64,
    pub checkpoint_interval: u64,
    pub justified: Checkpoint,
    pub finalized: Checkpoint,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Checkpoint {
    pub epoch: u64,
    pub block: String,
    #[serde(default)]
    pub height: Option<u64>,
}

/// A block's id and header, as listings, the stream and block pages show it.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Summary {
    pub id: String,
    pub height: u64,
    pub parent: String,
    pub timestamp: u64,
    pub producer: String,
    pub difficulty: String,
    pub tx_root: String,
    /// What its transactions did and the state it leaves, as roots; empty from older nodes.
    #[serde(default)]
    pub receipts_root: String,
    #[serde(default)]
    pub state_root: String,
    pub gas_used: u64,
    pub base_price: String,
    /// What it minted for its producer; empty from older nodes.
    #[serde(default)]
    pub reward: String,
    /// Unknown, so 0, for a pruned block.
    #[serde(deserialize_with = "count")]
    pub tx_count: usize,
    #[serde(deserialize_with = "count")]
    pub proof_count: usize,
    /// Whether it is on the main chain, where the API says.
    #[serde(default)]
    pub main: Option<bool>,
    /// Whether the node keeps its header alone, as one that synced from a snapshot does for the
    /// blocks before it.
    #[serde(default)]
    pub pruned: bool,
    /// The chain hash of the mining pool's commitment the block carries, the zero hash for one
    /// that carries none; empty from older nodes.
    #[serde(default)]
    pub pool_root: String,
    /// What a mining pool's block sent its miners, when its producer runs a pool; none from older
    /// nodes.
    #[serde(default)]
    pub pool: Option<PoolShare>,
}

/// A pool's block's commitment: the part of its reward and fees that went to the pool's escrow,
/// and the root its miners claim against. What it came to, and whose escrow it is, come with the
/// full block alone, not in listings.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct PoolShare {
    /// The escrow's part, of PAYOUT_PARTS: what is left after the pool's fee.
    pub parts: u32,
    /// The Merkle root over every miner's lifetime earnings.
    pub root: String,
    /// How many miners' earnings the root covers.
    pub count: u64,
    /// The block's reward and its producer's fees, once the node knows them.
    #[serde(default)]
    pub shared: Option<String>,
    /// What of that went to the escrow; the producer kept the rest, the pool's fee.
    #[serde(default)]
    pub escrowed: Option<String>,
    #[serde(default)]
    pub escrow: Option<String>,
}

/// Parts of PAYOUT_PARTS, as a percent.
pub fn percent(parts: u32) -> f64 {
    f64::from(parts) / f64::from(chainv4::structs::block::PAYOUT_PARTS) * 100.0
}

/// A count, or 0 where the node does not know it, as for a pruned block.
fn count<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<usize, D::Error> {
    Ok(Option::<usize>::deserialize(deserializer)?.unwrap_or(0))
}

/// GET /blocks.
#[derive(Clone, Debug, Deserialize)]
pub struct Page {
    pub blocks: Vec<Summary>,
}

/// GET /blocks/{block}.
#[derive(Clone, Debug, Deserialize)]
pub struct BlockDetail {
    #[serde(flatten)]
    pub summary: Summary,
    pub seal: Value,
    pub proofs: Vec<Value>,
    pub txs: Vec<TxView>,
}

/// A signed transaction as blocks and the mempool list it.
#[derive(Clone, Debug, Deserialize)]
pub struct TxView {
    pub id: String,
    pub sender: String,
    pub nonce: u64,
    pub signer: String,
    pub signature: String,
    pub fee: Fee,
    /// Its type and that type's fields.
    pub tx: Value,
    #[serde(default)]
    pub receipt: Option<Outcome>,
}

impl TxView {
    /// What kind of transaction it is: transfer, contract_call, deploy and so on.
    pub fn kind(&self) -> &str {
        self.tx.get("type").and_then(Value::as_str).unwrap_or("?")
    }

    /// Who it is for, when it names someone: a recipient, a contract or a voter.
    pub fn target(&self) -> Option<&str> {
        ["to", "contract", "voter"].iter().find_map(|field| self.tx.get(field).and_then(Value::as_str))
    }

    /// A batch's steps, its own or the one a multisig sends or a member approves: None for
    /// anything else.
    pub fn steps(&self) -> Option<&[Value]> {
        steps_of(&self.tx)
    }
}

/// The steps of a transaction's batch, as the node writes one.
pub fn steps_of(tx: &Value) -> Option<&[Value]> {
    match tx.get("type").and_then(Value::as_str)? {
        "batch" => tx.get("steps")?.as_array().map(Vec::as_slice),
        "multisig" | "approve_multisig" => steps_of(tx.get("tx")?),
        _ => None,
    }
}

/// What one step of a batch that went through did: as a whole transaction's outcome says it, and
/// how many of the receipt's events and payments are its.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct StepView {
    pub status: String,
    #[serde(default)]
    pub contract: Option<String>,
    #[serde(default)]
    pub values: Vec<Argument>,
    #[serde(default)]
    pub events: u32,
    #[serde(default)]
    pub payments: u32,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Fee {
    pub gas_limit: u64,
    pub max_price: String,
    pub tip: String,
}

/// What a transaction did, as a receipt says.
#[derive(Clone, Debug, Deserialize)]
pub struct Outcome {
    pub status: String,
    #[serde(default)]
    pub contract: Option<String>,
    #[serde(default)]
    pub values: Vec<Argument>,
    #[serde(default)]
    pub error: Option<String>,
    pub fuel_used: u64,
    pub gas_used: u64,
    pub fee: String,
    pub events: Vec<EventView>,
    /// The coin its contracts paid, in order, as the receipts root commits to it.
    #[serde(default)]
    pub payments: Vec<PaymentView>,
    #[serde(default)]
    pub trace: Trace,
    /// The change a governance vote decided, for status decided: the chain's count of decisions,
    /// its kind and the kind's version; or, for status rejected, the kind and the version it was
    /// rejected at.
    #[serde(default)]
    pub version: Option<u64>,
    /// The chain's count of rejections a vote made, for status rejected.
    #[serde(default)]
    pub rejection: Option<u64>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub kind_version: Option<u64>,
    /// What each step did, for a batch that went through.
    #[serde(default)]
    pub steps: Vec<StepView>,
    /// Which step stopped a batch that failed, counted from 0.
    #[serde(default)]
    pub failed_step: Option<u32>,
}

impl Outcome {
    /// What came of a batch's step, in a word, and what it did if it stands. What a later step
    /// undid or never reached says so.
    pub fn of_step(&self, index: usize) -> (&str, Option<&StepView>) {
        match self.failed_step.map(|failed| (index as u32).cmp(&failed)) {
            Some(std::cmp::Ordering::Less) => ("undone", None),
            Some(std::cmp::Ordering::Equal) => ("failed", None),
            Some(std::cmp::Ordering::Greater) => ("not run", None),
            None => match self.steps.get(index) {
                Some(step) => (step.status.as_str(), Some(step)),
                None => ("?", None),
            },
        }
    }
}

/// Coin a contract paid while a transaction ran.
#[derive(Clone, Debug, Deserialize)]
pub struct PaymentView {
    pub from: String,
    pub to: String,
    pub amount: String,
}

/// GET /txs/{id}: a receipt and where it landed.
#[derive(Clone, Debug, Deserialize)]
pub struct Receipt {
    pub block: String,
    pub height: u64,
    pub index: u32,
    pub confirmations: u64,
    #[serde(rename = "final", default)]
    pub finalized: bool,
    #[serde(flatten)]
    pub outcome: Outcome,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Argument {
    #[serde(rename = "type")]
    pub kind: String,
    pub value: Value,
}

impl Argument {
    /// The value as text, without the quotes JSON puts around strings.
    pub fn text(&self) -> String {
        match &self.value {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct EventView {
    pub contract: String,
    pub name: String,
    pub data: Vec<Argument>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Trace {
    pub hops: Vec<Hop>,
    pub dropped: u32,
}

/// One call, delegate or transfer a transaction's contracts made.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Hop {
    pub depth: u8,
    pub kind: String,
    pub from: String,
    pub to: String,
    pub function: String,
    pub value: String,
    pub fuel_used: u64,
    pub error: Option<String>,
    pub events: Vec<u32>,
}

/// GET /accounts/{address}.
#[derive(Clone, Debug, Deserialize)]
pub struct Account {
    pub nonce: u64,
    pub balance: String,
    /// Its primary name, such as alice.dev, on a chain with names.
    #[serde(default)]
    pub name: Option<String>,
    /// An Ethereum account's EIP-7702 delegation: the contract whose code it runs as its own.
    #[serde(default)]
    pub delegated_to: Option<String>,
}

/// GET /names/{name}: a name at the tip.
#[derive(Clone, Debug, Deserialize)]
pub struct NameView {
    pub name: String,
    pub display: String,
    /// active, grace or available.
    pub status: String,
    pub owner: Option<String>,
    pub target: Option<String>,
    pub resolves_to: Option<String>,
    pub expires: Option<u64>,
    pub grace_ends: Option<u64>,
    #[serde(default)]
    pub texts: std::collections::BTreeMap<String, String>,
    pub price_per_year: Option<String>,
}

/// One name of GET /accounts/{address}/names.
#[derive(Clone, Debug, Deserialize)]
pub struct OwnedName {
    pub display: String,
    pub status: String,
    pub expires: u64,
}

/// GET /accounts/{address}/names.
#[derive(Clone, Debug, Deserialize)]
pub struct OwnedNames {
    pub names: Vec<OwnedName>,
    /// Where the next page starts, when there is one.
    #[serde(default)]
    pub next: Option<String>,
}

/// GET /accounts/{address}/multisigs: the multisigs an address's key is a member of.
#[derive(Clone, Debug, Deserialize)]
pub struct MultisigsPage {
    pub multisigs: Vec<Membership>,
    #[serde(default)]
    pub next: Option<String>,
}

/// One multisig an address is a member of: its address, and how many of how many must agree.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Membership {
    pub address: String,
    pub threshold: u64,
    pub members: u64,
}

/// One name of GET /names/list: the directory, sorted by name.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct ListedName {
    pub name: String,
    pub display: String,
    /// active, grace or available.
    pub status: String,
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub expires: Option<u64>,
}

/// GET /names/list: a page of the name directory, and the name to pass as after for the next one.
/// A chain without names answers 404.
#[derive(Clone, Debug, Deserialize)]
pub struct NamesPage {
    pub names: Vec<ListedName>,
    pub next: Option<String>,
}

/// One account of GET /rich, and its balance in the smallest unit.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Holder {
    pub address: String,
    pub balance: String,
}

/// GET /rich: the richest accounts first, at a height, and how many hold anything at all. As the
/// node reports it: nothing here is proven.
#[derive(Clone, Debug, Deserialize)]
pub struct RichList {
    pub height: u64,
    /// How many accounts have a balance.
    pub holders: u64,
    pub accounts: Vec<Holder>,
}

/// GET /supply: every coin made and destroyed, what there is, and what of it is bonded or in the
/// reward pool, at a height. Minted and burned are one state entry, under key in the validators
/// column, which a light client proves; staked and the reward pool are the node's word alone.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct SupplyView {
    pub height: u64,
    pub minted: String,
    pub burned: String,
    pub supply: String,
    pub staked: String,
    pub reward_pool: String,
    /// The supply's state key, in hex.
    pub key: String,
}

/// GET /multisig/{address}: a multisig's members and threshold, its next nonce and balance, and
/// the proposals open for that nonce. Anything else answers 404.
#[derive(Clone, Debug, Deserialize)]
pub struct MultisigView {
    pub members: Vec<MultisigMemberView>,
    pub threshold: u32,
    #[serde(default)]
    pub testnet: bool,
    /// Its descriptors and first address on Bitcoin, when every member names an xpub.
    #[serde(default)]
    pub bitcoin: Value,
    /// Its contract on EVM chains, when every member names an EVM address.
    #[serde(default)]
    pub evm: Value,
    pub nonce: u64,
    pub balance: String,
    #[serde(default)]
    pub proposals: Vec<MultisigProposalView>,
}

/// A multisig's member: its key here, and its keys on Bitcoin and EVM chains, if it names them.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct MultisigMemberView {
    pub key: String,
    #[serde(default)]
    pub bitcoin: Option<String>,
    #[serde(default)]
    pub evm: Option<String>,
    /// What owns its multisig's EVM contract for it: key, contract, or ed25519:0x… and its key;
    /// none from older nodes, whose owners are all keys.
    #[serde(default)]
    pub evm_kind: Option<String>,
}

/// A transaction proposed for a multisig's next nonce, and the members' keys that approve it.
#[derive(Clone, Debug, Deserialize)]
pub struct MultisigProposalView {
    pub id: String,
    /// What it does, as transactions' JSON writes it.
    pub tx: Value,
    pub fee: Fee,
    #[serde(default)]
    pub approvals: Vec<String>,
}

/// GET /contracts/{address}.
#[derive(Clone, Debug, Deserialize)]
pub struct Contract {
    pub deployer: String,
    pub engine: String,
    pub functions: Vec<String>,
    pub reentrant: Vec<String>,
    /// The namespaces its code declares it keeps its state in; empty from nodes before layouts.
    #[serde(default)]
    pub layout: Vec<Declared>,
    pub upgradable: bool,
    pub code_size: u64,
    /// What the chain's token registry holds for it, when it is a token.
    #[serde(default)]
    pub token: Option<RegisteredToken>,
    /// What the chain's collection registry holds for it, when it is a collection.
    #[serde(default)]
    pub collection: Option<RegisteredCollection>,
}

/// One namespace a contract's storage layout declares: a map of one kind, keyed by any key or by
/// keys of a kind, a record of keys, or retired.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Declared {
    pub name: String,
    /// map, record or retired.
    pub shape: String,
    /// A map's values' kind, and the unit they are in.
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub unit: Option<String>,
    /// A map's keys' kind and unit; None for any key, and from nodes before key kinds.
    #[serde(default)]
    pub key_kind: Option<String>,
    #[serde(default)]
    pub key_unit: Option<String>,
    /// A record's keys.
    #[serde(default)]
    pub keys: Vec<Field>,
}

/// One key a record namespace declares.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Field {
    pub key: String,
    /// Its kind; None once retired.
    pub kind: Option<String>,
    /// The unit its value is in, if the layout names one.
    #[serde(default)]
    pub unit: Option<String>,
}

/// A kind as a layout writes it: with its unit after a colon, when it has one.
pub fn kind_text(kind: &str, unit: Option<&str>) -> String {
    match unit {
        Some(unit) => format!("{kind}:{unit}"),
        None => kind.to_string(),
    }
}

/// A token as the chain's registry holds it: what its name(), symbol() and decimals() answered
/// when it was deployed or last upgraded, each None when it exports none.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct RegisteredToken {
    pub address: String,
    pub name: Option<String>,
    pub symbol: Option<String>,
    pub decimals: Option<u8>,
}

/// A collection as the registry holds it: an NFT one, one owner a token, or a multi-token one.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct RegisteredCollection {
    pub address: String,
    /// nft or multi.
    pub kind: String,
    pub name: Option<String>,
    pub symbol: Option<String>,
}

impl RegisteredCollection {
    pub fn multi(&self) -> bool {
        self.kind == "multi"
    }

    /// Its name, else its symbol, else nothing.
    pub fn title(&self) -> Option<String> {
        self.name.clone().or_else(|| self.symbol.clone())
    }
}

/// GET /collections.
#[derive(Clone, Debug, Deserialize)]
pub struct CollectionsPage {
    pub collections: Vec<RegisteredCollection>,
    /// Where the next page starts, when there is one.
    #[serde(default)]
    pub next: Option<String>,
}

/// GET /tokens.
#[derive(Clone, Debug, Deserialize)]
pub struct TokensPage {
    pub tokens: Vec<RegisteredToken>,
    pub next: Option<String>,
}

/// GET /node.
#[derive(Clone, Debug, Deserialize)]
pub struct NodeView {
    pub uptime_ms: u64,
    pub listening: Vec<String>,
    pub producing: bool,
    pub validating: bool,
    pub mempool: MempoolCount,
    pub syncing: Option<Syncing>,
    pub counters: BTreeMap<String, u64>,
    pub peers: Vec<Peer>,
    /// Gossip and submissions waiting for the node, and its limits; none from older nodes.
    #[serde(default)]
    pub backlog: Option<Backlog>,
    /// How its mining goes, when it mines; none from older nodes.
    #[serde(default)]
    pub mining: Option<MiningView>,
    /// What the node keeps to itself from this asker, by field; none from older nodes.
    #[serde(default)]
    pub hidden: Vec<String>,
    /// Whether it did not take the report token sent; false from older nodes.
    #[serde(default)]
    pub token_refused: bool,
}

impl NodeView {
    /// Whether the node keeps this field, as its --hide names it, from this asker.
    pub fn hides(&self, field: &str) -> bool {
        self.hidden.iter().any(|hidden| hidden == field)
    }
}

/// A node's mining: its own devices, the workers mining for it, and their hashrates.
#[derive(Clone, Debug, Deserialize)]
pub struct MiningView {
    /// Hashes a second, every device's and worker's added up.
    pub rate: f64,
    pub devices: Vec<Device>,
    /// Where workers connect, when it takes them.
    pub listening: Option<String>,
    pub workers: Vec<MiningWorker>,
    /// The mining pool it runs on its mining port, if it does; none from older nodes.
    #[serde(default)]
    pub pool: Option<PoolSettings>,
    /// Whether its mining port takes workers mining alone, each paid its own blocks in full;
    /// false from older nodes.
    #[serde(default)]
    pub solo: bool,
}

/// A node's mining pool, as GET /node sums it up: its full books are GET /pool's.
#[derive(Clone, Debug, Deserialize)]
pub struct PoolSettings {
    /// What its producer keeps of each block, of PAYOUT_PARTS.
    pub fee: u32,
    /// How many blocks' worth of the latest shares each block splits by.
    pub window: u64,
    /// Whether anyone may join with a payout address alone, without the mining token.
    pub open: bool,
    pub miners: u64,
}

/// A GPU, or a CPU's threads, and its hashrate.
#[derive(Clone, Debug, Deserialize)]
pub struct Device {
    pub name: String,
    pub rate: f64,
}

/// A worker mining for the node over its mining port.
#[derive(Clone, Debug, Deserialize)]
pub struct MiningWorker {
    pub name: String,
    pub address: String,
    pub since: u64,
    pub rate: f64,
    pub devices: Vec<Device>,
    pub found: u64,
    pub refused: u64,
    /// Where a pool's worker has its part of each block credited; none outside a pool.
    #[serde(default)]
    pub payout: Option<String>,
    /// The shares it sent, and the difficulty the pool set it, outside a pool none and "0".
    #[serde(default)]
    pub shares: u64,
    #[serde(default)]
    pub share_difficulty: String,
    /// Whether it mines alone, its blocks paying its payout in full.
    #[serde(default)]
    pub solo: bool,
}

/// GET /pool, or GET /pool/{address} for one miner: the pool a node runs, its miners, and the
/// blocks it sealed lately with what each paid whom.
#[derive(Clone, Debug, Deserialize)]
pub struct PoolView {
    /// The pool's producer, which keeps its fee, and the escrow no key controls, which holds what
    /// its miners have yet to claim.
    pub producer: String,
    pub escrow: String,
    pub escrow_balance: String,
    /// What its producer keeps of each block, as a percent.
    pub fee_percent: f64,
    pub window: u64,
    pub open: bool,
    /// The easiest share difficulty it hands out.
    pub min_share: String,
    /// The work its miners' shares in the window add up to.
    pub work: String,
    /// What it credited every miner with, all told.
    pub earned: String,
    pub miners: Vec<PoolMiner>,
    /// A miner's workers, for one miner; none for the whole pool.
    #[serde(default)]
    pub workers: Vec<PoolWorker>,
    pub blocks: Vec<PoolBlock>,
}

/// One miner in a pool: its work in the window, what the next block gives it, and what it earned
/// and claimed.
#[derive(Clone, Debug, Deserialize)]
pub struct PoolMiner {
    pub to: String,
    pub work: String,
    pub shares: u64,
    /// Its part of the next block, as a percent.
    pub percent: f64,
    pub earned: String,
    pub claimed: String,
    pub claimable: String,
}

/// One of a miner's workers on the pool.
#[derive(Clone, Debug, Deserialize)]
pub struct PoolWorker {
    pub name: String,
    pub rate: f64,
    pub shares: u64,
    pub share_difficulty: String,
}

/// A block a pool sealed, and whether its payouts were credited.
#[derive(Clone, Debug, Deserialize)]
pub struct PoolBlock {
    pub height: u64,
    pub id: String,
    /// When it was sealed, in milliseconds since the Unix epoch.
    pub at: u64,
    /// None while it waits to be final, false once a reorganization dropped it.
    pub credited: Option<bool>,
    pub shared: Option<String>,
    pub payouts: Vec<PoolPayout>,
}

/// A miner's part of a pool's block, and what it came to once the block's total is known.
#[derive(Clone, Debug, Deserialize)]
pub struct PoolPayout {
    pub to: String,
    pub percent: f64,
    pub amount: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Backlog {
    pub waiting: usize,
    pub total: usize,
    pub per_peer: usize,
}

#[derive(Clone, Debug, Deserialize)]
pub struct MempoolCount {
    pub count: usize,
    pub capacity: usize,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Syncing {
    pub peer: String,
    pub from: u64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Peer {
    pub id: String,
    pub address: String,
    pub since: u64,
    pub heard: Heard,
    /// Its transactions and votes the backlog had no room for.
    #[serde(default)]
    pub dropped: u64,
    pub chain: Option<PeerChain>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
pub struct Heard {
    pub blocks: u64,
    pub txs: u64,
    pub votes: u64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct PeerChain {
    pub height: u64,
    pub work: String,
    pub justified: u64,
}

/// GET /mempool.
#[derive(Clone, Debug, Deserialize)]
pub struct Mempool {
    pub count: usize,
    pub capacity: usize,
    pub txs: Vec<TxView>,
}

/// GET /forks.
#[derive(Clone, Debug, Deserialize)]
pub struct Forks {
    pub blocks: Vec<Summary>,
    pub tips: Vec<String>,
}

/// One of GET /events's events: the event, and where it happened.
#[derive(Clone, Debug, Deserialize)]
pub struct FoundEvent {
    pub contract: String,
    pub name: String,
    pub data: Vec<Argument>,
    pub tx: String,
    pub block: String,
    pub height: u64,
    pub index: u32,
    pub position: u32,
}

/// What a node's index lists, by where it sits on the chain, to order and tell entries apart by.
pub trait Placed {
    fn place(&self) -> (u64, u32, u32);
}

impl Placed for FoundEvent {
    fn place(&self) -> (u64, u32, u32) {
        (self.height, self.index, self.position)
    }
}

/// A transaction the address index found touching an address, as blocks list it, with where it
/// landed.
#[derive(Clone, Debug, Deserialize)]
pub struct FoundTx {
    #[serde(flatten)]
    pub tx: TxView,
    pub block: String,
    pub height: u64,
    pub index: u32,
    /// When its block was made, in milliseconds since the Unix epoch; a node from before it said
    /// leaves it out.
    #[serde(default)]
    pub time: Option<u64>,
}

impl Placed for FoundTx {
    fn place(&self) -> (u64, u32, u32) {
        (self.height, self.index, 0)
    }
}

/// A page of what one of a node's indexes lists: GET /events, or GET /accounts/{address}/txs.
#[derive(Clone, Debug, Deserialize)]
pub struct IndexPage<T> {
    #[serde(alias = "events", alias = "txs")]
    pub items: Vec<T>,
    pub next: Option<String>,
    pub indexed_from: u64,
    /// The height the node is still moving the index's low end to, if it is.
    #[serde(default)]
    pub catching_up_to: Option<u64>,
    /// The filters the node went by: a node from before it could filter leaves it out, and sent
    /// everything.
    #[serde(default)]
    pub applied: Option<Vec<String>>,
    /// How many entries it looked through for this page.
    #[serde(default)]
    pub scanned: u64,
    /// Whether it stopped at its bound with the page not full: more may match from next on.
    #[serde(default)]
    pub limited: bool,
    /// Whether the node's event index knows EVM logs' topics, as an older node's does not.
    #[serde(default)]
    pub topics: bool,
}

pub type EventsPage = IndexPage<FoundEvent>;
pub type TxsPage = IndexPage<FoundTx>;

/// POST /call.
#[derive(Clone, Debug, Deserialize)]
pub struct Called {
    pub status: String,
    #[serde(default)]
    pub values: Vec<Argument>,
    #[serde(default)]
    pub error: Option<String>,
}

/// POST /offer.
#[derive(Clone, Debug, Deserialize)]
pub struct Offer {
    pub gas_limit: u64,
    pub max_price: String,
    pub tip: String,
    /// What a dry run found, for what runs code.
    #[serde(default)]
    pub estimate: Option<Estimate>,
}

/// A dry run, as POST /offer reports it.
#[derive(Clone, Debug, Deserialize)]
pub struct Estimate {
    pub status: String,
    #[serde(default)]
    pub error: Option<String>,
}

/// GET /governance: who votes, what a change needs, the changes being voted on, and every setting
/// in force. A chain without governance answers 404.
#[derive(Clone, Debug, Deserialize)]
pub struct GovernanceView {
    /// council or electorate.
    pub kind: String,
    pub epoch: u64,
    /// The consensus the tip's epoch runs, and the one decided last, as genesis files write them.
    #[serde(default)]
    pub active: Value,
    #[serde(default)]
    pub decided: Value,
    /// How many changes were decided so far, of every kind: the chain's gap-free count.
    pub version: u64,
    /// How many of each kind were decided, by the kind's name: what a vote on one names.
    #[serde(default)]
    pub versions: BTreeMap<String, u64>,
    /// How many epochs a vote counts for, from the one it was cast in.
    pub approval_epochs: u64,
    /// The share of every voter's weight, in thousandths, that must vote on a change before Yes
    /// decides it.
    #[serde(default)]
    pub quorum_permille: u32,
    /// What Yes must weigh to decide a change.
    pub needed: String,
    /// What every voter weighs together.
    #[serde(default)]
    pub total: String,
    /// How many changes were rejected so far, of every kind.
    #[serde(default)]
    pub rejections: u64,
    /// Blocks per epoch, to say when old votes stop counting.
    #[serde(default)]
    pub checkpoint_interval: u64,
    /// Everyone who may vote, with what each weighs: the council, or finality's voters.
    #[serde(default)]
    pub voters: Vec<Voter>,
    /// The finality the tip's epoch runs: ffg, authority or depth.
    #[serde(default)]
    pub finality: String,
    /// A switch of finality's kind on its way.
    #[serde(default)]
    pub switch: Option<SwitchView>,
    #[serde(default)]
    pub proposals: Vec<Proposal>,
    /// The council's accounts; none under an electorate.
    #[serde(default)]
    pub members: Option<Vec<String>>,
    /// Each setting governance may change, by its kind, as it stands; a kind the chain has none of
    /// is left out.
    #[serde(default)]
    pub in_force: BTreeMap<String, InForce>,
    /// The settings no vote may change, by path, in order: genesis's and those decided since. None
    /// from a node before locks.
    #[serde(default)]
    pub locks: Vec<String>,
}

impl GovernanceView {
    /// Whether votes weigh stake, in the chain's coin: an electorate under FFG. Elsewhere each
    /// weighs a member or a signer.
    pub fn weighs_stake(&self) -> bool {
        self.kind == "electorate" && self.finality == "ffg"
    }
}

/// A switch of finality's kind: from an epoch on, to a kind.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct SwitchView {
    pub epoch: u64,
    #[serde(rename = "type")]
    pub kind: String,
}

/// One who may vote, and what its vote weighs.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Voter {
    pub account: String,
    pub weight: String,
}

/// A change being voted on: the change, how its votes stand, and who voted which way.
#[derive(Clone, Debug, Deserialize)]
pub struct Proposal {
    /// The kind of change it is, whose ballot it is on: consensus, bootnodes, name_prices, …
    #[serde(default)]
    pub kind: String,
    /// Its kind's version: how many of that kind were decided before. It would be the next.
    #[serde(default)]
    pub version: u64,
    /// How its votes stand: each side's weight, what is needed, the quorum, whether it can pass.
    #[serde(default)]
    pub tally: Value,
    /// The epoch its oldest vote counts through.
    #[serde(default)]
    pub closes: Option<u64>,
    #[serde(default)]
    pub votes: Vec<CastVote>,
    /// The change itself: one field, named for its kind.
    #[serde(flatten)]
    pub change: serde_json::Map<String, Value>,
}

impl Proposal {
    /// The change, as Tx::VoteChange's JSON carries it: { kind: value }.
    pub fn change(&self) -> Value {
        Value::Object(self.change.clone())
    }

    /// How its votes stand, as numbers.
    pub fn tally(&self) -> chainv4_ui::votes::Tally {
        chainv4_ui::votes::Tally::of(&self.tally)
    }

    /// How an account voted on it, if it did.
    pub fn vote_of(&self, account: &str) -> Option<chainv4_ui::votes::Choice> {
        self.votes.iter().find(|vote| vote.account == account).and_then(|vote| chainv4_ui::votes::Choice::of(&vote.vote))
    }

    /// Its kind's name: as the node lists it, or from the change's field when it does not.
    pub fn kind_name(&self) -> String {
        match self.kind.is_empty() {
            false => self.kind.clone(),
            true => change_kind(&self.change()).unwrap_or_default(),
        }
    }
}

/// The kind a change is of, as the API names kinds, from the field its JSON names the change by:
/// finality's tuning and a switch of its kind are the consensus's. A field the library does not
/// know of is its own kind.
pub fn change_kind(change: &Value) -> Option<String> {
    let field = change.as_object()?.keys().next()?;

    Some(field.parse::<chainv4::governance::ChangeKind>().map_or_else(|_| field.clone(), |kind| kind.name().to_string()))
}

/// One vote on a change: who, which way, what it weighs, and the epoch it stops counting after.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct CastVote {
    pub account: String,
    /// yes, no or abstain.
    pub vote: String,
    pub weight: String,
    pub epoch: u64,
    pub counts_until: u64,
}

/// A setting as it stands, and the change that last set it: None while genesis's stands.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct InForce {
    pub value: Value,
    #[serde(default)]
    pub decided: Option<u64>,
}

/// GET /governance/history: a page of decisions, newest first.
#[derive(Clone, Debug, Deserialize)]
pub struct DecisionsPage {
    /// The newest decision's version: how many there were.
    pub version: u64,
    /// The oldest version the node keeps: 1 unless it synced from a snapshot, version + 1 with
    /// none.
    pub from: u64,
    pub decisions: Vec<Decision>,
}

/// A decision: the vote that decided a change, where it landed, and the change.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Decision {
    pub version: u64,
    /// The kind its receipt decided one of, and the kind's version it made.
    #[serde(default)]
    pub of: Option<String>,
    #[serde(default)]
    pub kind_version: Option<u64>,
    pub tx: String,
    pub block: String,
    pub height: u64,
    pub timestamp: u64,
    pub confirmations: u64,
    #[serde(rename = "final", default)]
    pub finalized: bool,
    #[serde(default)]
    pub sender: Option<String>,
    /// What it changed, and to what; none when the node cannot read its transaction.
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub change: Option<Value>,
}

/// GET /governance/rejections: a page of rejections, newest first.
#[derive(Clone, Debug, Deserialize)]
pub struct RejectionsPage {
    /// The newest rejection's count: how many there were.
    pub rejection: u64,
    /// The oldest the node keeps.
    pub from: u64,
    pub rejections: Vec<Rejection>,
}

/// A rejection: the vote that left a change unable to pass, where it landed, and the change.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Rejection {
    pub rejection: u64,
    pub tx: String,
    pub block: String,
    pub height: u64,
    pub timestamp: u64,
    pub confirmations: u64,
    #[serde(rename = "final", default)]
    pub finalized: bool,
    #[serde(default)]
    pub sender: Option<String>,
    /// The kind its receipt rejected one of, and the kind's version it was rejected at.
    #[serde(default)]
    pub of: Option<String>,
    #[serde(default)]
    pub kind_version: Option<u64>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub change: Option<Value>,
}

/// GET /governance/weight/{address}: what an account's vote weighs now, beside what a change
/// needs, and its votes that still count.
#[derive(Clone, Debug, Deserialize)]
pub struct GovernanceWeight {
    pub account: String,
    pub weight: String,
    pub needed: String,
    #[serde(default)]
    pub voted: Vec<VotedChange>,
}

/// A vote an account cast, on its kind's ballot, at the kind's version.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct VotedChange {
    pub kind: String,
    pub version: u64,
    /// yes, no or abstain.
    pub vote: String,
    #[serde(default)]
    pub counts_until: u64,
    /// The change: one field, named for what it changes.
    pub change: Value,
}

/// The stream's reorganized event.
#[derive(Clone, Debug, Deserialize)]
pub struct Reorganized {
    pub undone: Vec<String>,
    pub applied: Vec<Summary>,
}

/// The stream's peer_up and peer_down events.
#[derive(Clone, Debug, Deserialize)]
pub struct PeerChange {
    pub id: String,
    #[serde(default)]
    pub address: Option<String>,
}

#[cfg(test)]
mod tests {
    use {super::*, serde_json::json};

    #[test]
    fn a_status_reads_the_chains_name_and_falls_back_without_one() {
        let status = |name: Option<Value>| {
            let mut value = json!({
                "genesis": "g", "tip": "t", "height": 1, "timestamp": 0, "difficulty": "0", "work": "1",
                "hash": "blake3", "coin": { "symbol": "DEV", "decimals": 18 }, "finality": null, "fees": null,
            });

            if let Some(name) = name {
                value["name"] = name;
            }

            serde_json::from_value::<Status>(value).unwrap()
        };

        let named = status(Some(json!("chainv4 testnet")));
        assert_eq!((named.name.as_deref(), named.chain_name()), (Some("chainv4 testnet"), "chainv4 testnet"));

        let unnamed = status(Some(Value::Null));
        assert_eq!((unnamed.name.as_deref(), unnamed.chain_name()), (None, "chainv4"));

        let older = status(None);
        assert_eq!((older.name.as_deref(), older.chain_name()), (None, "chainv4"), "a node from before names");

        assert_eq!(status(Some(json!("  "))).chain_name(), "chainv4", "a blank name is none");
        assert_eq!(chain_name(None), "chainv4", "before any status");
    }

    #[test]
    fn a_receipt_reads_what_its_contracts_paid_and_one_from_before_reads_none() {
        let paid: Outcome = serde_json::from_value(json!({
            "status": "returned", "fuel_used": 1, "gas_used": 2, "fee": "3", "events": [],
            "payments": [{ "from": "contract:0x05", "to": "ed25519:0x03", "amount": "900" }],
        }))
        .unwrap();

        assert_eq!((paid.payments[0].to.as_str(), paid.payments[0].amount.as_str()), ("ed25519:0x03", "900"));

        let older: Outcome = serde_json::from_value(json!({ "status": "done", "fuel_used": 0, "gas_used": 0, "fee": "0", "events": [] })).unwrap();
        assert!(older.payments.is_empty());
    }

    #[test]
    fn layouts_read_with_key_kinds_and_units_and_without() {
        let layout: Vec<Declared> = serde_json::from_value(json!([
            {"name": "points", "shape": "map", "kind": "decimal", "unit": "points", "key_kind": "address-text", "key_unit": null},
            {"name": "notes", "shape": "map", "kind": "text"},
            {"name": "config", "shape": "record", "keys": [
                {"key": "cap", "kind": "decimal", "unit": "points", "retired": false},
                {"key": "name", "kind": null, "retired": true},
            ]},
        ]))
        .unwrap();

        assert_eq!(layout[0].key_kind.as_deref(), Some("address-text"));
        assert_eq!(kind_text(layout[0].kind.as_deref().unwrap(), layout[0].unit.as_deref()), "decimal:points");
        assert_eq!((layout[1].key_kind.as_deref(), layout[1].unit.as_deref()), (None, None), "from a node before either");
        assert_eq!(kind_text("decimal", layout[2].keys[0].unit.as_deref()), "decimal:points");
        assert_eq!(layout[2].keys[1].kind, None);
    }

    #[test]
    fn blocks_read_with_their_transactions_receipts_and_traces() {
        let block: BlockDetail = serde_json::from_str(
            r#"{
                "id": "0xaa", "height": 3, "parent": "0xbb", "timestamp": 6000, "producer": "ed25519:0x01",
                "difficulty": "0", "tx_root": "0xcc", "gas_used": 21000, "base_price": "1000000000",
                "tx_count": 1, "proof_count": 0, "main": true,
                "seal": { "type": "signature", "signature": "0x99" },
                "proofs": [],
                "txs": [{
                    "id": "0xdd", "sender": "ed25519:0x01", "nonce": 4, "signer": "ed25519:0x01", "signature": "0x98",
                    "fee": { "gas_limit": 100000, "max_price": "2000000000", "tip": "0" },
                    "tx": { "type": "contract_call", "to": "contract:0x02", "func": "xxh3_64:0x03", "amount": "0", "data": [] },
                    "receipt": {
                        "status": "returned", "values": [{ "type": "u256", "value": "7" }],
                        "fuel_used": 900, "gas_used": 22000, "fee": "22000000000000",
                        "events": [{ "contract": "contract:0x02", "name": "Transfer", "data": [{ "type": "u256", "value": "7" }] }],
                        "trace": { "hops": [{
                            "depth": 0, "kind": "call", "from": "ed25519:0x01", "to": "contract:0x02", "function": "transfer",
                            "value": "0", "fuel_used": 900, "error": null, "events": [0]
                        }], "dropped": 0 }
                    }
                }]
            }"#,
        )
        .unwrap();

        assert_eq!(block.summary.main, Some(true));
        assert_eq!(block.txs[0].kind(), "contract_call");
        assert_eq!(block.txs[0].target(), Some("contract:0x02"));

        let receipt = block.txs[0].receipt.as_ref().unwrap();

        assert_eq!(receipt.values[0].text(), "7");
        assert_eq!(receipt.trace.hops[0].events, [0]);
        assert_eq!(receipt.events[0].name, "Transfer");
    }

    #[test]
    fn index_pages_read_events_and_transactions_alike() {
        let txs: TxsPage = serde_json::from_value(json!({
            "txs": [{
                "id": "0xdd", "sender": "ed25519:0x01", "nonce": 4, "signer": "ed25519:0x01", "signature": "ed25519:0x02",
                "fee": { "gas_limit": 0, "max_price": "0", "tip": "0" },
                "tx": { "type": "transfer", "from": "ed25519:0x01", "to": "ed25519:0x03", "amount": "7" },
                "block": "0xaa", "height": 9, "index": 2,
                "receipt": { "status": "done", "fuel_used": 0, "gas_used": 0, "fee": "0", "events": [], "trace": { "hops": [], "dropped": 0 } }
            }],
            "next": null, "indexed_from": 0, "catching_up_to": null
        }))
        .unwrap();
        let events: EventsPage = serde_json::from_value(json!({ "events": [], "next": "0x01", "indexed_from": 3 })).unwrap();

        assert_eq!((txs.items[0].place(), txs.items[0].tx.target()), ((9, 2, 0), Some("ed25519:0x03")));
        assert_eq!(txs.items[0].tx.receipt.as_ref().unwrap().status, "done");
        assert_eq!((events.items.len(), events.next.as_deref(), events.catching_up_to), (0, Some("0x01"), None));
    }

    #[test]
    fn receipts_and_reports_read_as_the_api_writes_them() {
        let receipt: Receipt = serde_json::from_value(json!({
            "id": "0xdd", "block": "0xaa", "height": 3, "index": 0, "confirmations": 2, "final": false,
            "status": "failed", "error": "out of fuel", "fuel_used": 10, "gas_used": 5, "fee": "5", "events": [],
            "trace": { "hops": [], "dropped": 0 }
        }))
        .unwrap();

        assert_eq!(receipt.outcome.error.as_deref(), Some("out of fuel"));

        let node: NodeView = serde_json::from_value(json!({
            "started": 1, "now": 2, "uptime_ms": 1, "listening": ["/ip4/127.0.0.1/udp/1/quic-v1/p2p/12D3"],
            "producing": false, "validating": true, "mempool": { "count": 3, "capacity": 50000 },
            "syncing": { "peer": "12D3", "from": 40 }, "counters": { "blocks_heard": 9 },
            "peers": [{ "id": "12D3", "address": "/ip4/10.0.0.2/udp/1/quic-v1", "since": 1,
                        "heard": { "blocks": 1, "txs": 2, "votes": 3 }, "chain": null }]
        }))
        .unwrap();

        assert_eq!(node.counters["blocks_heard"], 9);
        assert_eq!(node.syncing.unwrap().from, 40);
        assert_eq!(node.peers[0].heard.votes, 3);
        assert!(node.mining.is_none(), "an older node, or one that does not mine, says nothing of it");

        let node: NodeView = serde_json::from_value(json!({
            "uptime_ms": 1, "listening": [], "producing": true, "validating": false,
            "mempool": { "count": 0, "capacity": 1 }, "syncing": null, "counters": {}, "peers": [],
            "mining": { "rate": 3e8, "devices": [{ "name": "Apple M3 Pro (Metal)", "rate": 2e8 }], "listening": "0.0.0.0:4002",
                        "workers": [{ "name": "rig", "address": "10.0.0.3:5000", "since": 1, "rate": 1e8,
                                      "devices": [{ "name": "GPU 0", "rate": 1e8 }], "found": 2, "refused": 0 }] }
        }))
        .unwrap();
        let mining = node.mining.unwrap();

        assert_eq!((mining.rate, mining.devices[0].name.as_str()), (3e8, "Apple M3 Pro (Metal)"));
        assert_eq!((mining.workers[0].found, mining.workers[0].devices[0].rate), (2, 1e8));
        assert!(!mining.solo && !mining.workers[0].solo, "an older node says nothing of mining alone");
        assert!(node.hidden.is_empty() && !node.token_refused, "nor of what it keeps to itself");

        let private: NodeView = serde_json::from_value(json!({
            "uptime_ms": 1, "listening": [], "producing": true, "validating": false,
            "mempool": { "count": 0, "capacity": 1 }, "syncing": null, "counters": {}, "peers": [], "mining": null, "backlog": null,
            "hidden": ["listening", "peers"], "token_refused": true,
        }))
        .unwrap();

        assert!(private.hides("listening") && private.hides("peers") && !private.hides("counters") && private.token_refused);

        let alone: MiningView = serde_json::from_value(json!({
            "rate": 1e6, "devices": [], "listening": "0.0.0.0:4002", "solo": true,
            "workers": [{ "name": "phone", "address": "127.0.0.1:5000", "since": 1, "rate": 1e6, "devices": [],
                          "found": 1, "refused": 0, "payout": "ed25519:0x04", "solo": true }]
        }))
        .unwrap();

        assert!(alone.solo && alone.workers[0].solo);
        assert_eq!(alone.workers[0].payout.as_deref(), Some("ed25519:0x04"));
    }

    #[test]
    fn a_pool_s_block_reads_its_commitment_and_one_from_an_older_node_none() {
        let summary = json!({
            "id": "0xaa", "height": 3, "parent": "0xbb", "timestamp": 6000, "producer": "ed25519:0x01",
            "difficulty": "5", "tx_root": "0xcc", "gas_used": 0, "base_price": "0", "tx_count": 0, "proof_count": 0,
        });
        let older: Summary = serde_json::from_value(summary.clone()).unwrap();

        assert_eq!((older.pool.as_ref(), older.pool_root.as_str()), (None, ""));

        let mut pooled = summary;
        pooled["pool_root"] = json!("0xdd");
        pooled["pool"] = json!({
            "parts": 990_000_000, "percent": 99.0, "root": "0xee", "count": 4,
            "shared": "1000", "escrowed": "990", "escrow": "ed25519:0x02",
        });
        let pooled: Summary = serde_json::from_value(pooled).unwrap();
        let pool = pooled.pool.unwrap();

        assert_eq!((pool.count, pool.escrowed.as_deref(), pool.escrow.as_deref()), (4, Some("990"), Some("ed25519:0x02")));
        assert_eq!(percent(pool.parts), 99.0);
    }

    #[test]
    fn a_pool_reads_as_get_pool_writes_it() {
        let pool: PoolView = serde_json::from_value(json!({
            "producer": "ed25519:0x01", "escrow": "ed25519:0x02", "escrow_balance": "700", "fee": 10_000_000, "fee_percent": 1.0,
            "window": 2, "open": true, "min_share": "1000", "work": "4000", "earned": "900",
            "miners": [{ "to": "ed25519:0x03", "work": "1000", "shares": 1, "parts": 250_000_000, "percent": 25.0,
                         "earned": "900", "claimed": "200", "claimable": "700" }],
            "workers": [],
            "blocks": [
                { "height": 7, "id": "0xaa", "at": 1, "credited": null, "shared": null,
                  "payouts": [{ "to": "ed25519:0x03", "parts": 250_000_000, "percent": 25.0, "amount": null }] },
                { "height": 6, "id": "0xbb", "at": 1, "credited": false, "shared": "1000",
                  "payouts": [{ "to": "ed25519:0x03", "parts": 250_000_000, "percent": 25.0, "amount": "250" }] },
            ],
        }))
        .unwrap();

        assert_eq!((pool.miners[0].claimable.as_str(), pool.miners[0].percent), ("700", 25.0));
        assert_eq!((pool.blocks[0].credited, pool.blocks[1].credited), (None, Some(false)), "waiting, then dropped");
        assert_eq!(pool.blocks[1].payouts[0].amount.as_deref(), Some("250"));

        let worker: MiningWorker = serde_json::from_value(json!({
            "name": "rig", "address": "10.0.0.3:5000", "since": 1, "rate": 1e8, "devices": [], "found": 0, "refused": 0,
        }))
        .unwrap();

        assert_eq!((worker.payout, worker.shares), (None, 0), "an older node says nothing of shares");
    }

    #[test]
    fn the_name_directory_reads_as_get_names_list_writes_it() {
        let page: NamesPage = serde_json::from_value(json!({
            "names": [
                { "name": "alice.dev", "display": "alice.dev", "status": "active", "owner": "ed25519:0x01", "target": "ed25519:0x02", "expires": 1_900_000_000_000_u64 },
                { "name": "bob.dev", "display": "Bob.dev", "status": "grace", "owner": "ed25519:0x03", "target": null, "expires": 1_000 },
            ],
            "next": "bob.dev",
        }))
        .unwrap();

        assert_eq!(page.names.len(), 2);
        assert_eq!((page.names[0].owner.as_deref(), page.names[0].target.as_deref()), (Some("ed25519:0x01"), Some("ed25519:0x02")));
        assert_eq!((page.names[1].display.as_str(), page.names[1].target.as_deref()), ("Bob.dev", None));
        assert_eq!(page.next.as_deref(), Some("bob.dev"));

        let last: NamesPage = serde_json::from_value(json!({ "names": [], "next": null })).unwrap();
        assert_eq!((last.names.len(), last.next), (0, None), "the last page names no next");
    }

    #[test]
    fn the_rich_list_and_the_supply_read_as_the_node_writes_them() {
        let rich: RichList = serde_json::from_value(json!({
            "height": 120, "holders": 3,
            "accounts": [{ "address": "ed25519:0x01", "balance": "700" }, { "address": "ed25519:0x02", "balance": "300" }],
        }))
        .unwrap();

        assert_eq!((rich.height, rich.holders, rich.accounts.len()), (120, 3, 2));
        assert_eq!(rich.accounts[0], Holder { address: "ed25519:0x01".to_string(), balance: "700".to_string() });

        let supply: SupplyView = serde_json::from_value(json!({
            "height": 120, "minted": "1500", "burned": "500", "supply": "1000", "staked": "200", "reward_pool": "50",
            "key": "0x636f696e3a737570706c79",
        }))
        .unwrap();

        assert_eq!((supply.minted.as_str(), supply.burned.as_str(), supply.supply.as_str()), ("1500", "500", "1000"));
        assert_eq!((supply.staked.as_str(), supply.reward_pool.as_str(), supply.key.as_str()), ("200", "50", "0x636f696e3a737570706c79"));
    }

    #[test]
    fn a_multisig_reads_as_get_multisig_writes_it() {
        let multisig: MultisigView = serde_json::from_value(json!({
            "address": "multisig:0x01", "threshold": 2, "testnet": false, "bitcoin": null, "evm": null, "nonce": 4, "balance": "900",
            "members": [
                { "key": "ed25519:0x02", "bitcoin": null, "evm": null },
                { "key": "secp256k1:0x03", "bitcoin": "[d34db33f/48'/0'/0'/2']xpub…", "evm": "0x04" },
            ],
            "proposals": [{
                "id": "0x05", "tx": { "type": "transfer", "from": "multisig:0x01", "to": "ed25519:0x06", "amount": "10" },
                "encoded": "0x00", "fee": { "gas_limit": 21000, "max_price": "1", "tip": "0" }, "approvals": ["ed25519:0x02"],
            }],
        }))
        .unwrap();

        assert_eq!((multisig.threshold, multisig.nonce, multisig.members.len()), (2, 4, 2));
        assert_eq!(multisig.members[1].evm.as_deref(), Some("0x04"));
        assert_eq!(multisig.proposals[0].tx["type"], "transfer");
        assert_eq!((multisig.proposals[0].fee.gas_limit, multisig.proposals[0].approvals.len()), (21000, 1));
        assert!(multisig.bitcoin.is_null());
    }

    #[test]
    fn a_node_s_release_reads_as_get_release_writes_it() {
        let release: NodeRelease = serde_json::from_value(json!({
            "version": "0.1.0", "target": "aarch64-apple-darwin", "updates": true, "manifest": null,
            "standing": "behind", "newest": "0.2.0", "checked": 1, "error": null, "installed": "0.2.0",
            "upgrade": { "least": "0.2.0", "from": 150_000, "source": "governance" },
        }))
        .unwrap();

        assert_eq!((release.manifest, release.newest.as_deref(), release.installed.as_deref()), (Value::Null, Some("0.2.0"), Some("0.2.0")));
        assert_eq!(release.upgrade.map(|upgrade| (upgrade.from, upgrade.source)), Some((150_000, Some("governance".to_string()))));

        let status: Status = serde_json::from_value(json!({
            "genesis": "g", "tip": "t", "height": 1, "timestamp": 0, "difficulty": "0", "work": "1",
            "hash": "blake3", "coin": { "symbol": "DEV", "decimals": 18 }, "finality": null, "fees": null,
        }))
        .unwrap();

        assert_eq!((status.version, status.upgrade), (None, None), "an older node names neither");
    }
}
