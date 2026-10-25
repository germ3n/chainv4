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
    /// What every request goes out through, or why none can: a proxy set that does not build
    /// sends nothing rather than go direct.
    client: Result<Client, String>,
    inbox: Inbox,
    asking: HashSet<Ask>,
    wake: egui::Context,
}

impl Node {
    pub fn new(base: &str, client: Result<Client, String>, wake: egui::Context) -> Node {
        Node {
            base: base.trim().trim_end_matches('/').to_string(),
            client,
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
