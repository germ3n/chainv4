//! Talking to a node's HTTP API, through the proxy set, if one is, and never around it. Every
//! request runs in the background, on the desktop and in the browser alike; answers wait in an
//! inbox the app empties each frame, and each one wakes the UI. The same question is never asked
//! twice at once, and what failed is held back for a moment before it is asked again, unless Retry
//! or Reload asks for it now.

use {
    crate::sse,
    chainv4_ui::asks::Asks,
    chainv4_ui::net::{Answer, Client, Piece},
    serde_json::{Value, json},
    std::ops::ControlFlow,
    std::sync::atomic::{AtomicBool, Ordering},
    std::sync::{Arc, Mutex},
    web_time::Instant,
};

pub use chainv4_ui::net::Failure;

/// How many tokens a page of the registry asks for.
pub const TOKEN_PAGE: u64 = 100;
/// How many of an address's transactions a page holds.
pub const TX_PAGE: u64 = 50;
/// How many of governance's decisions a page holds.
pub const DECISION_PAGE: u64 = 20;
/// How many names a page of the directory holds.
pub const NAME_PAGE: u64 = 50;
/// How many of the richest accounts the rich list asks for: the node's most.
pub const RICH_LIST: u64 = 100;

/// Something to ask a node, which also names what came back.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Ask {
    Status,
    Node,
    Mempool,
    Forks,
    /// Main-chain summaries below a height, newest first.
    Blocks { before: Option<u64>, limit: u64 },
    /// A block by height or id.
    Block(String),
    Tx(String),
    /// A 0x id that may name a block or a transaction: the block is tried first.
    Lookup(String),
    Account(String),
    Contract(String),
    Validators,
    Signers,
    Evidence,
    /// A page of indexed events, from the newest or from where a cursor says.
    Events { query: EventQuery, cursor: Option<String> },
    /// A page of the transactions touching an address, from the address index, that pass a filter,
    /// as its query says, or every one for "". An older node ignores the filter.
    AccountTxs { address: String, query: String, cursor: Option<String> },
    /// A page of the token registry: tokens whose symbol starts with this, or every one for "".
    Tokens { symbol: String, cursor: Option<String> },
    /// The collection registry, every NFT and multi-token collection, up to the node's most.
    Collections,
    /// A read-only call, with its arguments already written as JSON.
    Call { contract: String, function: String, args: String },
    /// A fee offer for a signed probe, with the sender's own choices, as POST /offer's body.
    Offer(String),
    /// A signed transaction to submit, as POST /txs's body.
    Submit(String),
    /// What the light client starts from: the chain's light parameters and genesis header.
    Light,
    /// Main-chain headers from a height on, encoded.
    Headers { from: u64, count: u32 },
    /// Finality from an epoch on, encoded, many epochs to an answer.
    Finality(u64),
    /// What the light client starts from at a checkpoint: the one named, or the node's finalized
    /// one, for a node met for the first time.
    Checkpoint(Option<String>),
    /// A proof of a chain-state entry, at a block, or at the tip.
    Proof { column: String, key: String, block: Option<String> },
    /// A name, such as alice.dev, at the tip.
    Name(String),
    /// The names an address owns.
    AccountNames(String),
    /// A JSON-RPC request to POST /rpc, as its body: what EVM reads go through.
    Rpc(String),
    /// A request to the node's faucet, with its proof of work: the body of POST /faucet.
    Faucet(String),
    /// The mining pool the node runs: its miners and its latest blocks.
    Pool,
    /// The same, for one miner by its payout address, with its workers.
    PoolMiner(String),
    /// The newest release the node took, as signed, and where the node stands against it.
    Release,
    /// Governance: who approves, the changes being approved, and every setting in force.
    Governance,
    /// A page of governance's decisions, newest first, below a version, or from the newest.
    Decisions { before: Option<u64> },
    /// What an account's approval weighs now.
    GovernanceWeight(String),
    /// A proof of a transaction's receipt, and of the transaction, against its block.
    ReceiptProof(String),
    /// Main-chain headers below the lowest the light client holds, encoded, to check older
    /// receipts against.
    Ancestors { from: u64, count: u32 },
    /// A multisig: its members, threshold, nonce and open proposals.
    Multisig(String),
    /// A page of the name directory, sorted by name: from the first, or after a name.
    NamesList { after: Option<String> },
    /// The richest accounts, as the node reports them.
    Rich,
    /// The coin's supply: minted and burned, which the light client proves, and what is staked.
    Supply,
}

impl Ask {
    fn path(&self) -> String {
        match self {
            Ask::Status => "/status".to_string(),
            Ask::Node => "/node".to_string(),
            Ask::Mempool => "/mempool?limit=200".to_string(),
            Ask::Forks => "/forks".to_string(),
            Ask::Blocks { before: Some(before), limit } => format!("/blocks?before={before}&limit={limit}"),
            Ask::Blocks { before: None, limit } => format!("/blocks?limit={limit}"),
            Ask::Block(block) | Ask::Lookup(block) => format!("/blocks/{block}"),
            Ask::Tx(id) => format!("/txs/{id}"),
            Ask::Account(address) => format!("/accounts/{address}"),
            Ask::Name(name) => format!("/names/{}", encode(name)),
            Ask::AccountNames(address) => format!("/accounts/{address}/names?limit=100"),
            Ask::Rpc(_) => "/rpc".to_string(),
            Ask::Faucet(_) => "/faucet".to_string(),
            Ask::Pool => "/pool".to_string(),
            Ask::PoolMiner(address) => format!("/pool/{address}"),
            Ask::Release => "/release".to_string(),
            Ask::Governance => "/governance".to_string(),
            Ask::Decisions { before: Some(before) } => format!("/governance/history?before={before}&limit={DECISION_PAGE}"),
            Ask::Decisions { before: None } => format!("/governance/history?limit={DECISION_PAGE}"),
            Ask::GovernanceWeight(address) => format!("/governance/weight/{address}"),
            Ask::ReceiptProof(tx) => format!("/proofs/receipts/{tx}"),
            Ask::Multisig(address) => format!("/multisig/{address}"),
            Ask::NamesList { after: Some(after) } => format!("/names/list?after={}&limit={NAME_PAGE}", encode(after)),
            Ask::NamesList { after: None } => format!("/names/list?limit={NAME_PAGE}"),
            Ask::Rich => format!("/rich?limit={RICH_LIST}"),
            Ask::Supply => "/supply".to_string(),
            Ask::Contract(address) => format!("/contracts/{address}"),
            Ask::Validators => "/validators".to_string(),
            Ask::Signers => "/signers".to_string(),
            Ask::Evidence => "/evidence".to_string(),
            Ask::Events { query, cursor } => {
                let mut path = format!("/events?{}", query.params());

                if let Some(cursor) = cursor {
                    path.push_str(&format!("&cursor={cursor}"));
                }

                path
            }
            Ask::AccountTxs { address, query, cursor } => {
                let mut path = format!("/accounts/{address}/txs?limit={TX_PAGE}");

                if !query.is_empty() {
                    path.push('&');
                    path.push_str(query);
                }

                if let Some(cursor) = cursor {
                    path.push_str(&format!("&cursor={cursor}"));
                }

                path
            }
            Ask::Tokens { symbol, cursor } => {
                let mut path = format!("/tokens?limit={TOKEN_PAGE}");

                if !symbol.is_empty() {
                    path.push_str(&format!("&symbol={}", encode(symbol)));
                }

                if let Some(cursor) = cursor {
                    path.push_str(&format!("&cursor={cursor}"));
                }

                path
            }
            Ask::Collections => "/collections?limit=500".to_string(),
            Ask::Light => "/light".to_string(),
            Ask::Headers { from, count } | Ask::Ancestors { from, count } => format!("/headers?from={from}&count={count}"),
            Ask::Finality(from) => format!("/light/finality?from={from}"),
            Ask::Checkpoint(Some(block)) => format!("/light/checkpoint/{block}"),
            Ask::Checkpoint(None) => "/light/checkpoint".to_string(),
            Ask::Proof { column, key, block: Some(block) } => format!("/proofs/{column}/{key}?block={block}"),
            Ask::Proof { column, key, block: None } => format!("/proofs/{column}/{key}"),
            Ask::Call { .. } => "/call".to_string(),
            Ask::Offer(_) => "/offer".to_string(),
            Ask::Submit(_) => "/txs".to_string(),
        }
    }

    fn body(&self) -> Option<String> {
        match self {
            Ask::Call { contract, function, args } => Some(format!(
                "{{\"contract\":{},\"function\":{},\"args\":{args}}}",
                json!(contract),
                json!(function)
            )),
            Ask::Offer(body) | Ask::Submit(body) | Ask::Rpc(body) | Ask::Faucet(body) => Some(body.clone()),
            _ => None,
        }
    }

    /// Whether this reads, rather than sends: only what reads is asked again unasked.
    pub fn reads(&self) -> bool {
        self.body().is_none()
    }

    /// Whether a reload of the page that asked this asks it again: what a page reads, not what the
    /// light client follows and proves by, which goes on by itself, nor anything sent, nor what
    /// goes to another page as it is answered.
    pub fn reloads(&self) -> bool {
        self.body().is_none()
            && !matches!(
                self,
                Ask::Status | Ask::Node | Ask::Release | Ask::Lookup(_) | Ask::Light | Ask::Headers { .. } | Ask::Finality(_) | Ask::Checkpoint(_) | Ask::Proof { .. } | Ask::ReceiptProof(_) | Ask::Ancestors { .. }
            )
    }
}

/// Which indexed events to list: a contract's, a contract's of one name, or an address's.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct EventQuery {
    pub contract: Option<String>,
    pub name: Option<String>,
    pub address: Option<String>,
    /// How many to a page: the node's default unless set.
    pub limit: Option<u32>,
}

impl EventQuery {
    pub fn contract(contract: &str, name: Option<&str>) -> EventQuery {
        EventQuery {
            contract: Some(contract.to_string()),
            name: name.filter(|name| !name.is_empty()).map(str::to_string),
            address: None,
            limit: None,
        }
    }

    /// The same, so many to a page.
    pub fn limited(self, limit: u32) -> EventQuery {
        EventQuery { limit: Some(limit), ..self }
    }

    pub fn mentioning(address: &str) -> EventQuery {
        EventQuery {
            contract: None,
            name: None,
            address: Some(address.to_string()),
            limit: None,
        }
    }

    fn params(&self) -> String {
        let limit = self.limit.map(|limit| limit.to_string());

        [("contract", &self.contract), ("name", &self.name), ("address", &self.address), ("limit", &limit)]
            .iter()
            .filter_map(|(key, value)| value.as_ref().map(|value| format!("{key}={}", encode(value))))
            .collect::<Vec<_>>()
            .join("&")
    }
}

/// Text made safe for a URL's query.
fn encode(text: &str) -> String {
    text.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b':' => (byte as char).to_string(),
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// What arrived from the node.
#[derive(Debug)]
pub enum Reply {
    Answer { ask: Ask, result: Result<Value, Failure> },
    Stream(News),
}

/// What the stream said.
#[derive(Debug)]
pub enum News {
    /// It is open: events follow.
    Open,
    Event { name: String, data: Value },
    /// It ended. Unsupported when the node does not stream at all.
    Closed { why: String, unsupported: bool },
}

pub struct Api {
    base: String,
    /// What every request goes out through, or why none can: a proxy set that does not build
    /// sends nothing rather than go direct.
    client: Result<Client, String>,
    inbox: Arc<Mutex<Vec<Reply>>>,
    /// What is out, what failed, and when to ask it again.
    asks: Asks<Ask>,
    /// What Retry or Reload asked again since the app last took it: why each failed goes.
    retried: Vec<Ask>,
    /// What the seconds these go by are counted from.
    began: Instant,
    wake: egui::Context,
    /// Set to end the stream that is open.
    hang_up: Option<Arc<AtomicBool>>,
    /// The node's report token, shown to GET /node alone, for it to share everything.
    report_token: Option<String>,
}

/// Whether a report token may go to this node: only where nobody on the way reads it, over TLS, an
/// onion or this machine. Its bearer sees where a node's peers and miners are.
pub fn carries_token(base: &str) -> bool {
    let Some((scheme, rest)) = base.split_once("://") else {
        return false;
    };
    let host = rest.split('/').next().unwrap_or_default();
    let host = host.rsplit_once('@').map_or(host, |(_, host)| host);
    let host = match host.strip_prefix('[') {
        Some(bracketed) => bracketed.split(']').next().unwrap_or_default(),
        None => host.split(':').next().unwrap_or_default(),
    };

    scheme.eq_ignore_ascii_case("https") || host.ends_with(".onion") || matches!(host, "localhost" | "127.0.0.1" | "::1")
}

impl Api {
    /// An API at base, such as http://127.0.0.1:8080, reached through client, that wakes the UI
    /// when answers come.
    pub fn new(base: &str, client: Result<Client, String>, wake: egui::Context) -> Api {
        Api {
            base: base.trim_end_matches('/').to_string(),
            client,
            inbox: Arc::default(),
            asks: Asks::default(),
            retried: Vec::new(),
            began: Instant::now(),
            wake,
            hang_up: None,
            report_token: None,
        }
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    /// The node's report token, sent with each GET /node from here on, where it may go at all.
    pub fn bear_report_token(&mut self, token: Option<&str>) {
        self.report_token = token.map(str::trim).filter(|token| !token.is_empty() && carries_token(&self.base)).map(str::to_string);
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

    /// Asks again now, whatever failed before: what Retry and Reload do. Why it failed goes.
    pub fn retry(&mut self, ask: Ask) {
        self.asks.forget(&ask);
        self.retried.push(ask.clone());

        if self.asks.force(&ask) {
            self.send(ask);
        }
    }

    /// What Retry or Reload asked again since last time.
    pub fn retried(&mut self) -> Vec<Ask> {
        std::mem::take(&mut self.retried)
    }

    /// What went out since last time.
    pub fn sent(&mut self) -> Vec<Ask> {
        self.asks.take_sent()
    }

    fn send(&mut self, ask: Ask) {
        let url = format!("{}{}", self.base, ask.path());
        let body = ask.body().map(String::into_bytes);
        let token = self.report_token.clone().filter(|_| ask == Ask::Node);
        let (inbox, wake) = (self.inbox.clone(), self.wake.clone());
        let done = move |answer: Result<Answer, String>| {
            let result = match answer {
                Ok(answer) => read(&answer),
                Err(message) => Err(Failure { status: 0, message }),
            };

            lock(&inbox).push(Reply::Answer { ask, result });
            wake.request_repaint();
        };

        match &self.client {
            Ok(client) => client.fetch_bearing(url, body, token, done),
            Err(why) => done(Err(unusable(why))),
        }
    }

    /// Whether this question is out.
    pub fn asking(&self, ask: &Ask) -> bool {
        self.asks.asking(ask)
    }

    /// Everything that arrived since last time.
    pub fn replies(&mut self) -> Vec<Reply> {
        let replies = std::mem::take(&mut *lock(&self.inbox));
        let now = self.now();

        for reply in &replies {
            if let Reply::Answer { ask, result } = reply {
                self.asks.ended(ask, result.as_ref().err(), now);
            }
        }

        replies
    }

    /// Opens the node's event stream, hanging up any open one.
    pub fn listen(&mut self) {
        self.hang_up();
        let hang_up = Arc::new(AtomicBool::new(false));
        self.hang_up = Some(hang_up.clone());
        let url = format!("{}/stream", self.base);
        let (inbox, wake) = (self.inbox.clone(), self.wake.clone());
        let parser = Mutex::new(sse::Parser::default());
        let on = move |piece: Result<Piece, String>| {
            let tell = |news: News| {
                lock(&inbox).push(Reply::Stream(news));
                wake.request_repaint();
            };

            if hang_up.load(Ordering::Relaxed) {
                return ControlFlow::Break(());
            }

            match piece {
                Ok(Piece::Head { ok: true, .. }) => tell(News::Open),
                Ok(Piece::Head { status, status_text, .. }) => {
                    tell(News::Closed {
                        why: format!("{status} {status_text}"),
                        unsupported: status == 404,
                    });

                    return ControlFlow::Break(());
                }
                Ok(Piece::Chunk(bytes)) if bytes.is_empty() => {
                    tell(News::Closed {
                        why: "the node closed the stream".to_string(),
                        unsupported: false,
                    });

                    return ControlFlow::Break(());
                }
                Ok(Piece::Chunk(bytes)) => {
                    for event in lock(&parser).feed(&bytes) {
                        let data = serde_json::from_str(&event.data).unwrap_or(Value::String(event.data));
                        tell(News::Event { name: event.name, data });
                    }
                }
                Err(why) => {
                    tell(News::Closed { why, unsupported: false });

                    return ControlFlow::Break(());
                }
            }

            ControlFlow::Continue(())
        };

        match &self.client {
            Ok(client) => client.stream(url, on),
            Err(why) => {
                let _ = on(Err(unusable(why)));
            }
        }
    }

    /// Ends the open stream, if any.
    pub fn hang_up(&mut self) {
        if let Some(hang_up) = self.hang_up.take() {
            hang_up.store(true, Ordering::Relaxed);
        }
    }
}

impl Drop for Api {
    fn drop(&mut self) {
        self.hang_up();
    }
}

/// Why nothing was sent: the proxy set does not build, and nothing goes around it.
fn unusable(why: &str) -> String {
    format!("the proxy set does not work: {why}")
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

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_report_token_goes_only_where_nobody_on_the_way_reads_it() {
        for base in ["https://node.example", "https://150-40-117-173.sslip.io/", "http://abcdefgh.onion", "http://abcdefgh.onion:8080/", "http://127.0.0.1:8090", "http://localhost:8080", "http://[::1]:8080", "HTTPS://node.example"] {
            assert!(carries_token(base), "{base}");
        }

        for base in ["http://node.example", "http://10.0.2.2:8080", "http://203.0.113.7:8080", "http://localhost.evil.example", "http://user@node.example", "node.example"] {
            assert!(!carries_token(base), "{base}");
        }
    }

    use super::*;

    #[test]
    fn event_queries_become_paths() {
        let named = Ask::Events {
            query: EventQuery::contract("contract:0xab", Some("Big Win")),
            cursor: Some("0x00ff".to_string()),
        };
        let everything = Ask::Events {
            query: EventQuery::contract("contract:0xab", Some("")),
            cursor: None,
        };

        assert_eq!(named.path(), "/events?contract=contract:0xab&name=Big%20Win&cursor=0x00ff");
        assert_eq!(everything.path(), "/events?contract=contract:0xab");
        assert_eq!(
            Ask::Events { query: EventQuery::mentioning("ed25519:0x01"), cursor: None }.path(),
            "/events?address=ed25519:0x01"
        );
    }

    #[test]
    fn an_address_s_transactions_become_paths() {
        let ask = |query: &str, cursor: Option<&str>| Ask::AccountTxs {
            address: "ed25519:0x01".to_string(),
            query: query.to_string(),
            cursor: cursor.map(str::to_string),
        };

        assert_eq!(ask("", None).path(), "/accounts/ed25519:0x01/txs?limit=50");
        assert_eq!(ask("", Some("0x00ff")).path(), "/accounts/ed25519:0x01/txs?limit=50&cursor=0x00ff");
        assert_eq!(ask("kind=transfer&direction=in", None).path(), "/accounts/ed25519:0x01/txs?limit=50&kind=transfer&direction=in");
        assert_eq!(ask("kind=transfer", Some("0x00ff")).path(), "/accounts/ed25519:0x01/txs?limit=50&kind=transfer&cursor=0x00ff");
    }

    #[test]
    fn with_a_proxy_that_does_not_work_nothing_is_sent_and_everything_says_why() {
        let mut api = Api::new("http://127.0.0.1:1", Err("not a port: 0".to_string()), egui::Context::default());
        api.ask(Ask::Status);
        api.listen();
        let replies = api.replies();

        assert!(matches!(&replies[0], Reply::Answer { ask: Ask::Status, result: Err(Failure { status: 0, message }) } if message == "the proxy set does not work: not a port: 0"));
        assert!(matches!(&replies[1], Reply::Stream(News::Closed { why, unsupported: false }) if why.starts_with("the proxy set does not work")));
        assert!(!api.asking(&Ask::Status), "answered, so asked again later");
    }

    #[test]
    fn what_failed_is_held_back_for_a_moment_and_retry_clears_it_and_asks_at_once() {
        let mut api = Api::new("http://127.0.0.1:1", Err("not a port: 0".to_string()), egui::Context::default());

        api.ask(Ask::Mempool);
        assert_eq!(api.replies().len(), 1);

        // The page goes on asking, as pages do each frame: nothing goes out for now.
        api.ask(Ask::Mempool);
        assert_eq!(api.replies().len(), 0, "held back");
        assert!(!api.asking(&Ask::Mempool), "never stuck as out");

        // Something sent goes whenever it is pressed for.
        api.ask(Ask::Submit("{}".to_string()));
        assert_eq!(api.replies().len(), 1);
        api.ask(Ask::Submit("{}".to_string()));
        assert_eq!(api.replies().len(), 1, "no hold on what someone sends");
        api.sent();

        // Retry goes past the hold, asks at once, and says so for the failure to go.
        api.retry(Ask::Mempool);
        assert_eq!(api.sent(), [Ask::Mempool]);
        assert_eq!(api.retried(), [Ask::Mempool]);
        assert_eq!(api.retried(), []);
        assert_eq!(api.replies().len(), 1);
    }

    #[test]
    fn a_reload_asks_again_what_a_page_reads_and_nothing_sent() {
        assert!(Ask::Tx("0x01".to_string()).reloads() && Ask::Governance.reloads());
        assert!(Ask::AccountTxs { address: "a".to_string(), query: String::new(), cursor: None }.reloads());
        assert!(!Ask::Status.reloads(), "always asked again by itself");
        assert!(!Ask::Lookup("0x01".to_string()).reloads(), "it goes to another page");
        assert!(!Ask::Headers { from: 0, count: 1 }.reloads(), "the light client's");
        assert!(!Ask::Submit("{}".to_string()).reloads(), "nothing sent is sent again");
    }

    #[test]
    fn a_pool_and_one_of_its_miners_become_paths() {
        assert_eq!(Ask::Pool.path(), "/pool");
        assert_eq!(Ask::PoolMiner("ed25519:0x01".to_string()).path(), "/pool/ed25519:0x01");
        assert_eq!(Ask::Pool.body(), None);
    }

    #[test]
    fn a_release_becomes_a_path() {
        assert_eq!((Ask::Release.path(), Ask::Release.body()), ("/release".to_string(), None));
    }

    #[test]
    fn governance_s_questions_become_paths() {
        assert_eq!(Ask::Governance.path(), "/governance");
        assert_eq!(Ask::Decisions { before: None }.path(), "/governance/history?limit=20");
        assert_eq!(Ask::Decisions { before: Some(31) }.path(), "/governance/history?before=31&limit=20");
        assert_eq!(Ask::GovernanceWeight("ed25519:0x01".to_string()).path(), "/governance/weight/ed25519:0x01");
        assert_eq!(Ask::ReceiptProof("0xab".to_string()).path(), "/proofs/receipts/0xab");
        assert_eq!(Ask::Ancestors { from: 10, count: 5 }.path(), "/headers?from=10&count=5");
    }

    #[test]
    fn the_directory_the_rich_list_the_supply_and_a_multisig_become_paths() {
        assert_eq!(Ask::NamesList { after: None }.path(), "/names/list?limit=50");
        assert_eq!(Ask::NamesList { after: Some("bob.dev".to_string()) }.path(), "/names/list?after=bob.dev&limit=50");
        assert_eq!(Ask::NamesList { after: Some("a b".to_string()) }.path(), "/names/list?after=a%20b&limit=50");
        assert_eq!(Ask::Rich.path(), "/rich?limit=100");
        assert_eq!((Ask::Supply.path(), Ask::Supply.body()), ("/supply".to_string(), None));
        assert_eq!(Ask::Multisig("multisig:0x01".to_string()).path(), "/multisig/multisig:0x01");
        assert_eq!(
            Ask::Proof { column: "validators".to_string(), key: "0x01".to_string(), block: None }.path(),
            "/proofs/validators/0x01"
        );
    }

    #[test]
    fn token_searches_become_paths() {
        assert_eq!(Ask::Tokens { symbol: String::new(), cursor: None }.path(), "/tokens?limit=100");
        assert_eq!(Ask::Collections.path(), "/collections?limit=500");
        assert_eq!(
            Ask::Tokens { symbol: "a b".to_string(), cursor: Some("0x01".to_string()) }.path(),
            "/tokens?limit=100&symbol=a%20b&cursor=0x01"
        );
    }
}
