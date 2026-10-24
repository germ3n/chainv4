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
    pub finality: Option<Finality>,
    /// The fee settings as genesis files write them, with base_price and tx_fuel.
    pub fees: Option<Value>,
}

impl Status {
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
    pub gas_used: u64,
    pub base_price: String,
    pub tx_count: usize,
    pub proof_count: usize,
    /// Whether it is on the main chain, where the API says.
    #[serde(default)]
    pub main: Option<bool>,
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
}

/// GET /contracts/{address}.
#[derive(Clone, Debug, Deserialize)]
pub struct Contract {
    pub deployer: String,
    pub engine: String,
    pub functions: Vec<String>,
    pub reentrant: Vec<String>,
    pub upgradable: bool,
    pub code_size: u64,
}

impl Contract {
    /// Whether it exports what the token standard asks.
    pub fn is_token(&self) -> bool {
        ["symbol", "decimals", "balance_of", "transfer"]
            .iter()
            .all(|wanted| self.functions.iter().any(|function| function == wanted))
    }
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
    }
}
