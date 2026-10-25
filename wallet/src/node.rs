//! Talking to a node's HTTP API, in the background on the desktop and in the browser alike.
//! Answers wait in an inbox the app empties each frame, and each wakes the UI. The node is never
//! trusted: everything it sends that the wallet shows as proven, the light client checked.

use {
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
    LightUpdate(String),
    Justifications(u64),
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
    /// A fee offer for a signed probe.
    Offer(String),
    /// A signed transaction.
    Submit(String),
}

impl Ask {
    fn path(&self) -> String {
        match self {
            Ask::Status => "/status".to_string(),
            Ask::Light => "/light".to_string(),
            Ask::Headers { from, count } | Ask::Ancestors { from, count } => format!("/headers?from={from}&count={count}"),
            Ask::LightUpdate(block) => format!("/light/updates/{block}"),
            Ask::Justifications(from) => format!("/justifications?from={from}"),
            Ask::Proof { column, key, block } => format!("/proofs/{column}/{key}?block={block}"),
            Ask::Receipt(id) => format!("/proofs/receipts/{id}"),
            Ask::Tokens => "/tokens?limit=100".to_string(),
            Ask::History(address) => format!("/accounts/{address}/txs?limit=50"),
            Ask::Events(address) => format!("/events?address={address}&limit=50"),
            Ask::Offer(_) => "/offer".to_string(),
            Ask::Submit(_) => "/txs".to_string(),
        }
    }

    fn body(&self) -> Option<&str> {
        match self {
            Ask::Offer(body) | Ask::Submit(body) => Some(body),
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

pub struct Node {
    base: String,
    inbox: Inbox,
    asking: HashSet<Ask>,
    wake: egui::Context,
}

impl Node {
    pub fn new(base: &str, wake: egui::Context) -> Node {
        Node {
            base: base.trim().trim_end_matches('/').to_string(),
            inbox: Arc::default(),
            asking: HashSet::new(),
            wake,
        }
    }

    /// Asks, unless the same question is out already.
    pub fn ask(&mut self, ask: Ask) {
        if !self.asking.insert(ask.clone()) {
            return;
        }

        let url = format!("{}{}", self.base, ask.path());
        let request = match ask.body() {
            Some(body) => ehttp::Request::post(url, body.as_bytes().to_vec()),
            None => ehttp::Request::get(url),
        };
        let (inbox, wake) = (self.inbox.clone(), self.wake.clone());

        ehttp::fetch(request, move |response| {
            let result = match response {
                Ok(response) => read(&response),
                Err(message) => Err(Failure { status: 0, message }),
            };

            inbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push((ask, result));
            wake.request_repaint();
        });
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
fn read(response: &ehttp::Response) -> Result<Value, Failure> {
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
