//! Talking to a node's HTTP API, through the proxy set, if one is, and never around it. Every
//! request runs in the background, on the desktop and in the browser alike; answers wait in an
//! inbox the app empties each frame, and each one wakes the UI. The same question is never asked
//! twice at once.

use {
    crate::sse,
    chainv4_ui::net::{Answer, Client, Piece},
    serde_json::{Value, json},
    std::collections::HashSet,
    std::ops::ControlFlow,
    std::sync::atomic::{AtomicBool, Ordering},
    std::sync::{Arc, Mutex},
};

/// How many tokens a page of the registry asks for.
pub const TOKEN_PAGE: u64 = 100;
/// How many of an address's transactions a page holds.
pub const TX_PAGE: u64 = 50;

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
    /// A page of the transactions touching an address, from the address index.
    AccountTxs { address: String, cursor: Option<String> },
    /// A page of the token registry: tokens whose symbol starts with this, or every one for "".
    Tokens { symbol: String, cursor: Option<String> },
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
            Ask::AccountTxs { address, cursor } => {
                let mut path = format!("/accounts/{address}/txs?limit={TX_PAGE}");

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
            Ask::Light => "/light".to_string(),
            Ask::Headers { from, count } => format!("/headers?from={from}&count={count}"),
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
}

/// Which indexed events to list: a contract's, a contract's of one name, or an address's.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct EventQuery {
    pub contract: Option<String>,
    pub name: Option<String>,
    pub address: Option<String>,
}

impl EventQuery {
    pub fn contract(contract: &str, name: Option<&str>) -> EventQuery {
        EventQuery {
            contract: Some(contract.to_string()),
            name: name.filter(|name| !name.is_empty()).map(str::to_string),
            address: None,
        }
    }

    pub fn mentioning(address: &str) -> EventQuery {
        EventQuery {
            contract: None,
            name: None,
            address: Some(address.to_string()),
        }
    }

    fn params(&self) -> String {
        [("contract", &self.contract), ("name", &self.name), ("address", &self.address)]
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

/// Why a request got no answer it could use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    /// The HTTP status, or 0 when the node could not be reached.
    pub status: u16,
    pub message: String,
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
    asking: HashSet<Ask>,
    wake: egui::Context,
    /// Set to end the stream that is open.
    hang_up: Option<Arc<AtomicBool>>,
}

impl Api {
    /// An API at base, such as http://127.0.0.1:8080, reached through client, that wakes the UI
    /// when answers come.
    pub fn new(base: &str, client: Result<Client, String>, wake: egui::Context) -> Api {
        Api {
            base: base.trim_end_matches('/').to_string(),
            client,
            inbox: Arc::default(),
            asking: HashSet::new(),
            wake,
            hang_up: None,
        }
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    /// Asks, unless the same question is out already.
    pub fn ask(&mut self, ask: Ask) {
        if !self.asking.insert(ask.clone()) {
            return;
        }

        let url = format!("{}{}", self.base, ask.path());
        let body = ask.body().map(String::into_bytes);
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
            Ok(client) => client.fetch(url, body, done),
            Err(why) => done(Err(unusable(why))),
        }
    }

    /// Whether this question is out.
    pub fn asking(&self, ask: &Ask) -> bool {
        self.asking.contains(ask)
    }

    /// Everything that arrived since last time.
    pub fn replies(&mut self) -> Vec<Reply> {
        let replies = std::mem::take(&mut *lock(&self.inbox));

        for reply in &replies {
            if let Reply::Answer { ask, .. } = reply {
                self.asking.remove(ask);
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
        let newest = Ask::AccountTxs {
            address: "ed25519:0x01".to_string(),
            cursor: None,
        };
        let older = Ask::AccountTxs {
            address: "ed25519:0x01".to_string(),
            cursor: Some("0x00ff".to_string()),
        };

        assert_eq!(newest.path(), "/accounts/ed25519:0x01/txs?limit=50");
        assert_eq!(older.path(), "/accounts/ed25519:0x01/txs?limit=50&cursor=0x00ff");
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
    fn token_searches_become_paths() {
        assert_eq!(Ask::Tokens { symbol: String::new(), cursor: None }.path(), "/tokens?limit=100");
        assert_eq!(
            Ask::Tokens { symbol: "a b".to_string(), cursor: Some("0x01".to_string()) }.path(),
            "/tokens?limit=100&symbol=a%20b&cursor=0x01"
        );
    }
}
