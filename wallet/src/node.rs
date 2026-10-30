//! Talking to a node's HTTP API, in the background on the desktop and in the browser alike, and
//! through the proxy set, if one is, and never around it. Answers wait in an inbox the app empties
//! each frame, and each wakes the UI. The node is never trusted: everything it sends that the
//! wallet shows as proven, the light client checked.

use {
    chainv4_ui::net::{Answer, Client},
    serde_json::Value,
    std::collections::HashSet,
    std::sync::{Arc, Mutex},
};

/// Something to ask the node, which also names what came back.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Ask {
    /// The coin's symbol and decimals, for showing amounts: nothing proven rests on it.
    Status,
    /// What the light client starts from.
    Light,
    Headers { from: u64, count: u32 },
    /// Headers below the lowest the light client holds, for it to prove by their hash chain.
    Ancestors { from: u64, count: u32 },
    /// Finality from an epoch on, many epochs to an answer.
    Finality(u64),
    /// What the light client starts from at a checkpoint: the one named, or the node's finalized
    /// one, for a node met for the first time.
    Checkpoint(Option<String>),
    /// A chain-state entry with its proof, at a block.
    Proof { column: &'static str, key: String, block: String },
    /// A transaction and its receipt, each with its Merkle path.
    Receipt(String),
    /// The token registry's first page, to know which tokens to prove.
    Tokens,
    /// The transactions touching an address, newest first, from the node's address index: where
    /// the history looks.
    History(String),
    /// Events that mention an address, newest first: where it looks on a node with no address
    /// index but an event index.
    Events(String),
    /// The chain's names: their suffix, prices and grace period, as the node says. Nothing proven
    /// rests on them but whether a name is in its grace period.
    Names,
    /// The names the node says an address owns: where the Names page looks, each one proven.
    OwnedNames(String),
    /// A contract's record, as the node says it: its engine, and a native contract's functions.
    Contract(String),
    /// A native contract's function run read-only, with the body of POST /call.
    Call(String),
    /// An EVM contract read: a call's probe dry-run at POST /offer, for what it returns.
    Read(String),
    /// A fee offer for a signed probe.
    Offer(String),
    /// A signed transaction.
    Submit(String),
    /// A request to the node's faucet, with its proof of work: the body of POST /faucet.
    Faucet(String),
    /// A contract's events of one name, newest first, from the node's event index: what the Trade
    /// page draws a pool's price from.
    #[cfg_attr(not(feature = "dex"), allow(dead_code))]
    Named { contract: String, name: String },
    /// A JSON-RPC request to POST /rpc, as its body: what a site's transaction is priced and sent
    /// by, as Ethereum's tools do.
    Rpc(String),
    /// A multisig's members, nonce and open proposals, as the node says: what the Multisig page
    /// shows and counts approvals by. Nothing it signs rests on it: a bundle says what it does.
    Multisig(String),
    /// What a mining pool owes an address, with the Merkle path a claim takes, as the pool's node
    /// says: what the Mining page claims with. Nothing rests on it but the fee: the chain checks
    /// the path against the root the pool's block committed to.
    PoolClaim(String),
    /// A mining pool's standing for an address: its work in the window, its part of the next
    /// block, its workers and the blocks it shared in.
    Pool(String),
    /// The node's mining port, as GET /mining says it: where it is reached, the protocol it speaks,
    /// and whether it takes a pool's workers, workers mining alone, and anyone. What the Mining
    /// page mines by; the node checks every nonce it is sent, so nothing rests on it.
    Mining,
    /// What an address has claimed from a pool, and the root the pool's block at a height
    /// committed to, as the wallet's own node says: what a claim kept on this device is checked
    /// against before it is sent, the pool's node no matter.
    Claims { pool: String, to: String, height: u64 },
    /// The collection registry's first page, for the NFTs page to look through.
    #[cfg_attr(not(feature = "nft"), allow(dead_code))]
    Collections,
    /// Governance as the node says it: its kind, the version an approval names, what a change
    /// needs, the open proposals with their approvals, and every setting as it stands. What the
    /// Governance page shows; nothing signed rests on it but the version, which the chain checks.
    Governance,
    /// A page of governance's decisions, newest first: the newest, or those below a version.
    GovernanceHistory { before: Option<u64> },
    /// What an address's approval weighs, and the change it counts for, as the node says.
    GovernanceWeight(String),
    /// The newest release the node took, as signed, beside where the node's own chainv4 stands:
    /// what the wallet checks its own version against, with the keys built in, as the node only
    /// relays it.
    Release,
}

impl Ask {
    fn path(&self) -> String {
        match self {
            Ask::Status => "/status".to_string(),
            Ask::Light => "/light".to_string(),
            Ask::Headers { from, count } | Ask::Ancestors { from, count } => format!("/headers?from={from}&count={count}"),
            Ask::Finality(from) => format!("/light/finality?from={from}"),
            Ask::Checkpoint(Some(block)) => format!("/light/checkpoint/{block}"),
            Ask::Checkpoint(None) => "/light/checkpoint".to_string(),
            Ask::Proof { column, key, block } => format!("/proofs/{column}/{key}?block={block}"),
            Ask::Receipt(id) => format!("/proofs/receipts/{id}"),
            Ask::Tokens => "/tokens?limit=100".to_string(),
            Ask::Collections => "/collections?limit=500".to_string(),
            Ask::History(address) => format!("/accounts/{address}/txs?limit=50"),
            Ask::Events(address) => format!("/events?address={address}&limit=50"),
            Ask::Named { contract, name } => format!("/events?contract={contract}&name={name}&limit=200"),
            Ask::Names => "/names".to_string(),
            Ask::OwnedNames(address) => format!("/accounts/{address}/names?limit=100"),
            Ask::Contract(address) => format!("/contracts/{address}"),
            Ask::Multisig(address) => format!("/multisig/{address}"),
            Ask::PoolClaim(address) => format!("/pool/claim/{address}"),
            Ask::Pool(address) => format!("/pool/{address}"),
            Ask::Mining => "/mining".to_string(),
            Ask::Claims { pool, to, height } => format!("/claims/{pool}/{to}?height={height}"),
            Ask::Release => "/release".to_string(),
            Ask::Governance => "/governance".to_string(),
            Ask::GovernanceHistory { before: Some(before) } => format!("/governance/history?before={before}&limit=20"),
            Ask::GovernanceHistory { before: None } => "/governance/history?limit=20".to_string(),
            Ask::GovernanceWeight(address) => format!("/governance/weight/{address}"),
            Ask::Call(_) => "/call".to_string(),
            Ask::Read(_) => "/offer".to_string(),
            Ask::Offer(_) => "/offer".to_string(),
            Ask::Submit(_) => "/txs".to_string(),
            Ask::Rpc(_) => "/rpc".to_string(),
            Ask::Faucet(_) => "/faucet".to_string(),
        }
    }

    fn body(&self) -> Option<&str> {
        match self {
            Ask::Offer(body) | Ask::Submit(body) | Ask::Call(body) | Ask::Read(body) | Ask::Rpc(body) | Ask::Faucet(body) => Some(body),
            _ => None,
        }
    }
}

/// Why a request got no answer it could use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    /// The HTTP status, or 0 when the node could not be reached.
    pub status: u16,
    pub message: String,
}

/// Where answers wait for the app, each with what it answers.
type Inbox = Arc<Mutex<Vec<(Ask, Result<Value, Failure>)>>>;

/// What was fetched from elsewhere than the node, by URL: its bytes, or why there are none.
type Fetched = Arc<Mutex<Vec<(String, Result<Vec<u8>, String>)>>>;

pub struct Node {
    base: String,
    /// What every request goes out through, or why none can: a proxy set that does not build
    /// sends nothing rather than go direct.
    client: Result<Client, String>,
    inbox: Inbox,
    asking: HashSet<Ask>,
    wake: egui::Context,
    fetched: Fetched,
    fetching: HashSet<String>,
}

impl Node {
    pub fn new(base: &str, client: Result<Client, String>, wake: egui::Context) -> Node {
        Node {
            base: base.trim().trim_end_matches('/').to_string(),
            client,
            inbox: Arc::default(),
            asking: HashSet::new(),
            wake,
            fetched: Arc::default(),
            fetching: HashSet::new(),
        }
    }

    /// Fetches a URL that is not the node's, as an NFT's metadata or picture, through the same
    /// proxy, unless it is out already. At most `most` bytes are kept: more fails.
    #[cfg_attr(not(feature = "nft"), allow(dead_code))]
    pub fn fetch(&mut self, url: &str, most: usize) {
        if !self.fetching.insert(url.to_string()) {
            return;
        }

        let (fetched, wake, key) = (self.fetched.clone(), self.wake.clone(), url.to_string());
        let done = move |answer: Result<Answer, String>| {
            let result = match answer {
                Ok(answer) if !answer.ok => Err(format!("{} {}", answer.status, answer.status_text)),
                Ok(answer) if answer.bytes.len() > most => Err(format!("more than {} MB", most / (1024 * 1024))),
                Ok(answer) => Ok(answer.bytes),
                Err(message) => Err(message),
            };

            fetched.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push((key, result));
            wake.request_repaint();
        };

        match &self.client {
            Ok(client) => client.fetch(url.to_string(), None, done),
            Err(why) => done(Err(format!("the proxy set does not work: {why}"))),
        }
    }

    /// Everything fetched from elsewhere since last time.
    #[cfg_attr(not(feature = "nft"), allow(dead_code))]
    pub fn fetched(&mut self) -> Vec<(String, Result<Vec<u8>, String>)> {
        let fetched = std::mem::take(&mut *self.fetched.lock().unwrap_or_else(std::sync::PoisonError::into_inner));

        for (url, _) in &fetched {
            self.fetching.remove(url);
        }

        fetched
    }

    /// Asks, unless the same question is out already.
    pub fn ask(&mut self, ask: Ask) {
        if !self.asking.insert(ask.clone()) {
            return;
        }

        let url = format!("{}{}", self.base, ask.path());
        let body = ask.body().map(|body| body.as_bytes().to_vec());
        let (inbox, wake) = (self.inbox.clone(), self.wake.clone());
        let done = move |answer: Result<Answer, String>| {
            let result = match answer {
                Ok(answer) => read(&answer),
                Err(message) => Err(Failure { status: 0, message }),
            };

            inbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push((ask, result));
            wake.request_repaint();
        };

        match &self.client {
            Ok(client) => client.fetch(url, body, done),
            Err(why) => done(Err(format!("the proxy set does not work: {why}"))),
        }
    }

    /// Everything that arrived since last time.
    pub fn answers(&mut self) -> Vec<(Ask, Result<Value, Failure>)> {
        let answers = std::mem::take(&mut *self.inbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner));

        for (ask, _) in &answers {
            self.asking.remove(ask);
        }

        answers
    }
}

/// An answer's JSON, or the API's {"error": …} as a failure.
fn read(response: &Answer) -> Result<Value, Failure> {
    let value: Option<Value> = serde_json::from_slice(&response.bytes).ok();

    match (response.ok, value) {
        (true, Some(value)) => Ok(value),
        (_, value) => Err(Failure {
            status: response.status,
            message: value
                .as_ref()
                .and_then(|value| value.get("error"))
                .and_then(Value::as_str)
                .map_or_else(|| format!("{} {}", response.status, response.status_text), str::to_string),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_a_proxy_that_does_not_work_nothing_is_sent_and_the_answer_says_why() {
        let mut node = Node::new("http://127.0.0.1:1", Err("not a port: 0".to_string()), egui::Context::default());
        node.ask(Ask::Status);
        let answers = node.answers();

        assert_eq!(answers, vec![(Ask::Status, Err(Failure {
            status: 0,
            message: "the proxy set does not work: not a port: 0".to_string(),
        }))]);
        assert!(!node.asking.contains(&Ask::Status), "answered, so asked again later");
    }
}
