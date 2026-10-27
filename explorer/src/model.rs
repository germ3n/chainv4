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
    /// The functions that make a contract a token on this chain; none keeps no registry.
    #[serde(default)]
    pub token_functions: Vec<String>,
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
}

impl Status {
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
    #[serde(default)]
    pub trace: Trace,
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
    }
}
