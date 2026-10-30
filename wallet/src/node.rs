//! Talking to a node's HTTP API, in the background on the desktop and in the browser alike, and
//! through the proxy set, if one is, and never around it. Answers wait in an inbox the app empties
//! each frame, and each wakes the UI. The node is never trusted: everything it sends that the
//! wallet shows as proven, the light client checked.

use {
    chainv4_ui::asks::Asks,
    chainv4_ui::net::{Answer, Client},
    serde_json::Value,
    std::collections::HashSet,
    std::sync::{Arc, Mutex},
    web_time::Instant,
};

pub use chainv4_ui::net::Failure;

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
    Finality { from: u64, count: u64 },
    /// How far the node has finalized, by GET /status: what says whether a client fast-resuming
    /// from this node is far enough behind to.
    Finalized,
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
    /// The transactions touching an address that pass a filter, as the filter's query says, from
    /// the newest or from where a cursor says: where the history looks once filters are on. An
    /// older node ignores the filter, and lists everything.
    Filtered { address: String, query: String, cursor: Option<String> },
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
    #[cfg_attr(not(feature = "mining"), allow(dead_code))]
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
            Ask::Status | Ask::Finalized => "/status".to_string(),
            Ask::Light => "/light".to_string(),
            Ask::Headers { from, count } | Ask::Ancestors { from, count } => format!("/headers?from={from}&count={count}"),
            Ask::Finality { from, count } => format!("/light/finality?from={from}&count={count}"),
            Ask::Checkpoint(Some(block)) => format!("/light/checkpoint/{block}"),
            Ask::Checkpoint(None) => "/light/checkpoint".to_string(),
            Ask::Proof { column, key, block } => format!("/proofs/{column}/{key}?block={block}"),
            Ask::Receipt(id) => format!("/proofs/receipts/{id}"),
            Ask::Tokens => "/tokens?limit=100".to_string(),
            Ask::Collections => "/collections?limit=500".to_string(),
            Ask::History(address) => format!("/accounts/{address}/txs?limit=50"),
            Ask::Filtered { address, query, cursor } => {
                let cursor = cursor.as_ref().map_or_else(String::new, |cursor| format!("&cursor={cursor}"));

                format!("/accounts/{address}/txs?limit=50&{query}{cursor}")
            }
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

    /// Whether a reload of the page that asked this asks it again: what a page reads, not what the
    /// light client follows and proves by, which it asks again itself, nor anything sent.
    pub fn reloads(&self) -> bool {
        self.body().is_none()
            && !matches!(
                self,
                Ask::Status | Ask::Light | Ask::Headers { .. } | Ask::Ancestors { .. } | Ask::Finality { .. } | Ask::Finalized | Ask::Checkpoint(_) | Ask::Proof { .. } | Ask::Receipt(_) | Ask::Release
            )
    }
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
    /// What is out, what failed, and when to ask it again.
    asks: Asks<Ask>,
    /// What the seconds these go by are counted from.
    began: Instant,
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
            asks: Asks::default(),
            began: Instant::now(),
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

    /// The seconds since this node was connected to.
    fn now(&self) -> f64 {
        self.began.elapsed().as_secs_f64()
    }

    /// Asks, unless the same question is out already, or failed a moment ago and has not been held
    /// back long enough. Something sent, rather than read, goes whenever it is pressed for.
    pub fn ask(&mut self, ask: Ask) {
        let now = self.now();
        let sending = match ask.body() {
            Some(_) => self.asks.force(&ask),
            None => self.asks.begin(&ask, now),
        };

        if sending {
            self.send(ask);
        }
    }

    /// Asks again now, whatever failed before, and lets go of why it did: what Retry and Reload do.
    pub fn retry(&mut self, ask: Ask) {
        self.asks.forget(&ask);

        if self.asks.force(&ask) {
            self.send(ask);
        }
    }

    /// Why the node does not answer, while it does not: what became of the status asked of it every
    /// few seconds, rather than of something one page asked.
    pub fn down(&self) -> Option<&Failure> {
        self.asks.failure(&Ask::Status).filter(|failure| failure.is_trouble())
    }

    /// What went wrong on the way to the node, for what is still wanted, for a page to say: the
    /// failure that has gone on longest, and everything that failed so, to ask again.
    pub fn trouble(&self) -> Option<(Failure, Vec<Ask>)> {
        // What was sent says for itself on its own page, and is never sent again unasked.
        let troubles: Vec<_> = self.asks.troubles(self.now()).into_iter().filter(|(ask, _)| ask.body().is_none()).collect();
        let failure = troubles.first()?.1.clone();

        Some((failure, troubles.into_iter().map(|(ask, _)| ask.clone()).collect()))
    }

    /// Whether this is out.
    pub fn asking(&self, ask: &Ask) -> bool {
        self.asks.asking(ask)
    }

    /// What went out since last time.
    pub fn sent(&mut self) -> Vec<Ask> {
        self.asks.take_sent()
    }

    fn send(&mut self, ask: Ask) {
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
        let now = self.now();

        for (ask, result) in &answers {
            self.asks.ended(ask, result.as_ref().err(), now);
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
        assert!(!node.asking(&Ask::Status), "answered, so asked again later");
    }

    /// A node with no way out, whose every request fails at once and says so.
    fn unreachable() -> Node {
        Node::new("http://127.0.0.1:1", Err("not a port: 0".to_string()), egui::Context::default())
    }

    #[test]
    fn what_failed_is_held_back_for_a_moment_and_retry_clears_it_and_asks_at_once() {
        let mut node = unreachable();

        node.ask(Ask::Tokens);
        assert_eq!(node.answers().len(), 1);

        let (failure, failed) = node.trouble().expect("the way to the node is in trouble");
        assert!(failure.sentence().starts_with("Could not reach the node"));
        assert_eq!(failed, [Ask::Tokens]);

        // The page goes on asking, as pages do each frame: nothing goes out for now.
        node.ask(Ask::Tokens);
        assert_eq!(node.answers().len(), 0, "held back");
        assert!(!node.asking(&Ask::Tokens));
        assert_eq!(node.sent(), [Ask::Tokens], "the first ask alone went out");

        // Retry goes past the hold, and clears what failed until it comes back.
        node.retry(Ask::Tokens);
        assert_eq!(node.sent(), [Ask::Tokens]);
        assert_eq!(node.answers().len(), 1);
        assert!(node.trouble().is_some(), "and failed again");
    }

    #[test]
    fn something_sent_goes_whenever_it_is_pressed_for_failed_or_not() {
        let mut node = unreachable();
        let submit = Ask::Submit("{}".to_string());

        node.ask(submit.clone());
        assert_eq!(node.answers().len(), 1);
        node.ask(submit);
        assert_eq!(node.answers().len(), 1, "no hold on what someone sends");
        assert!(node.trouble().is_none(), "nor is it what a page offers to ask again");
    }

    #[test]
    fn a_reload_asks_again_what_a_page_reads_and_leaves_the_light_client_its_own() {
        assert!(Ask::Governance.reloads() && Ask::Pool("a".to_string()).reloads());
        assert!(Ask::Filtered { address: "a".to_string(), query: "kind=transfer".to_string(), cursor: None }.reloads());
        assert!(!Ask::Status.reloads(), "always asked again by itself");
        assert!(!Ask::Headers { from: 0, count: 1 }.reloads());
        assert!(!Ask::Receipt("0x01".to_string()).reloads());
        assert!(!Ask::Submit("{}".to_string()).reloads(), "nothing sent is sent again");
    }

    #[test]
    fn a_filter_goes_on_the_listing_s_path() {
        let ask = |cursor: Option<&str>| Ask::Filtered { address: "ed25519:0x01".to_string(), query: "kind=transfer&direction=in".to_string(), cursor: cursor.map(str::to_string) };

        assert_eq!(ask(None).path(), "/accounts/ed25519:0x01/txs?limit=50&kind=transfer&direction=in");
        assert_eq!(ask(Some("0xff")).path(), "/accounts/ed25519:0x01/txs?limit=50&kind=transfer&direction=in&cursor=0xff");
        assert_eq!(Ask::Finalized.path(), "/status");
    }
}
