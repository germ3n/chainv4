//! The relay, on a thread of its own: connecting and coming back when the connection goes,
//! subscribing to the pairings' and sessions' topics, and turning what sites send into requests the
//! approval shows, and the answers back into sealed messages.

use {
    super::*,
    std::collections::{HashMap, HashSet},
    std::sync::{Arc, Mutex, PoisonError, mpsc},
    std::time::{Duration, SystemTime, UNIX_EPOCH},
};

/// The most one relay message may be: far more than a site's request needs.
const MOST_MESSAGE: usize = 1 << 20;
/// How long to wait before connecting again, at first and at most.
const BACK_OFF: (Duration, Duration) = (Duration::from_secs(2), Duration::from_secs(60));
/// How often the thread looks for the app's commands while nothing comes from the relay.
const POLL: Duration = Duration::from_millis(100);

/// How the relay connection stands.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub connected: bool,
    pub trouble: Option<String>,
}

enum Command {
    Pair(Pairing),
    Answer(u64, crate::dapp::Answer),
    Disconnect(String),
    Chain(u64),
}

/// What waits for the approval, by the id the approval knows it by.
enum Waiting {
    Proposal(Proposal),
    Request { topic: String, id: u64 },
}

#[derive(Default)]
struct Shared {
    status: Status,
    /// What waits, as the approval reads it.
    pending: Vec<Value>,
    sessions: Vec<Session>,
    /// Whether the sessions changed since the app last kept them.
    changed: bool,
}

/// WalletConnect running, until dropped.
pub struct WalletConnect {
    /// The project, relay and proxy it was started with, so a change starts it again.
    pub started_with: (String, String, Option<String>),
    shared: Arc<Mutex<Shared>>,
    commands: mpsc::Sender<Command>,
}

fn lock(shared: &Mutex<Shared>) -> std::sync::MutexGuard<'_, Shared> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs())
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_millis() as u64)
}

impl WalletConnect {
    /// Starts it, with the sessions kept, on a chain.
    pub fn start(project: &str, relay: &str, proxy: Option<String>, sessions: Vec<Session>, chain: u64, wake: egui::Context) -> WalletConnect {
        let sessions: Vec<Session> = sessions.into_iter().filter(|session| session.expiry > now()).collect();
        let shared = Arc::new(Mutex::new(Shared { sessions: sessions.clone(), ..Shared::default() }));
        let (commands, received) = mpsc::channel();
        let (inner, project_id, relay_url, through) = (shared.clone(), project.to_string(), relay.to_string(), proxy.clone());

        let _ = std::thread::Builder::new().name("walletconnect".to_string()).spawn(move || {
            let mut run = Run { shared: inner, wake, sessions, pairings: HashMap::new(), waiting: HashMap::new(), next: 1 << 40, chain, seen: HashSet::new(), fetches: HashSet::new() };
            run.run(&project_id, &relay_url, through.as_deref(), &received);
        });

        WalletConnect { started_with: (project.to_string(), relay.to_string(), proxy), shared, commands }
    }

    /// Pairs with a site by its code: its proposal then waits for the approval.
    pub fn pair(&self, code: &str) -> Result<(), String> {
        let pairing = Pairing::parse(code)?;

        if pairing.expiry.is_some_and(|expiry| expiry < now()) {
            return Err("That code has expired: ask the site for a new one.".to_string());
        }

        self.commands.send(Command::Pair(pairing)).map_err(|_| "WalletConnect stopped".to_string())
    }

    /// What waits for the approval, as the extension hands requests on.
    pub fn pending(&self) -> Vec<Value> {
        lock(&self.shared).pending.clone()
    }

    /// The approval's answer to one of them.
    pub fn answer(&self, id: u64, answer: crate::dapp::Answer) {
        lock(&self.shared).pending.retain(|pending| pending["id"].as_u64() != Some(id));
        let _ = self.commands.send(Command::Answer(id, answer));
    }

    pub fn disconnect(&self, topic: &str) {
        let _ = self.commands.send(Command::Disconnect(topic.to_string()));
    }

    /// The chain the wallet follows now: sessions on another are moved to it, and told.
    pub fn follow(&self, chain: u64) {
        let _ = self.commands.send(Command::Chain(chain));
    }

    pub fn status(&self) -> Status {
        lock(&self.shared).status.clone()
    }

    pub fn sessions(&self) -> Vec<Session> {
        lock(&self.shared).sessions.clone()
    }

    /// The sessions, once they changed since last asked: for the app to keep, sealed.
    pub fn changed(&self) -> Option<Vec<Session>> {
        let mut shared = lock(&self.shared);

        std::mem::take(&mut shared.changed).then(|| shared.sessions.clone())
    }
}

struct Run {
    shared: Arc<Mutex<Shared>>,
    wake: egui::Context,
    sessions: Vec<Session>,
    pairings: HashMap<String, [u8; 32]>,
    waiting: HashMap<u64, Waiting>,
    next: u64,
    chain: u64,
    /// Messages taken, by topic and id, so one the relay sends again is not taken twice.
    seen: HashSet<(String, u64)>,
    /// The ids of the fetches asked, whose answers carry messages.
    fetches: HashSet<u64>,
}

/// A connection to the relay: its lines out, and in.
struct Connection {
    out: mpsc::SyncSender<String>,
    came: mpsc::Receiver<Result<String, String>>,
}

impl Connection {
    fn send(&self, message: &Value) -> Result<(), String> {
        self.out.send(message.to_string()).map_err(|_| "the connection closed".to_string())
    }

    fn publish(&self, topic: &str, key: &[u8; 32], payload: &Value, (tag, ttl): (u32, u64)) -> Result<(), String> {
        let mut iv = [0; 12];
        getrandom::fill(&mut iv).map_err(|err| format!("no randomness: {err}"))?;
        let message = seal(key, &payload.to_string(), iv);

        self.send(&json!({ "id": payload_id(now_ms()), "jsonrpc": "2.0", "method": "irn_publish", "params": { "topic": topic, "message": message, "ttl": ttl, "tag": tag, "prompt": false } }))
    }
}

/// Why the relay would not connect, in its own terms: what the WebSocket client says is worded
/// for a node's mining port.
fn relay_trouble(why: &str) -> String {
    match () {
        _ if why.contains("answered 401") || why.contains("answered 403") => "the relay turned the project id down: check it in Settings".to_string(),
        _ if why.contains("no mining WebSocket") => "there is no relay at that address: check the relay in Settings".to_string(),
        _ if why.contains("too many from this address") => "the relay is turning connections away for now".to_string(),
        _ => why.to_string(),
    }
}

impl Run {
    fn say(&self, connected: bool, trouble: Option<String>) {
        lock(&self.shared).status = Status { connected, trouble };
        self.wake.request_repaint();
    }

    fn keep_sessions(&self) {
        let mut shared = lock(&self.shared);
        shared.sessions = self.sessions.clone();
        shared.changed = true;
        self.wake.request_repaint();
    }

    fn run(&mut self, project: &str, relay: &str, proxy: Option<&str>, commands: &mpsc::Receiver<Command>) {
        let mut wait = BACK_OFF.0;

        loop {
            match self.connect(project, relay, proxy) {
                Ok(connection) => {
                    wait = BACK_OFF.0;
                    self.say(true, None);

                    match self.session(&connection, commands) {
                        Ok(()) => return,
                        Err(why) => self.say(false, Some(why)),
                    }
                }
                Err(why) => self.say(false, Some(why)),
            }

            // Commands keep coming while it waits: the app gone ends it.
            let until = std::time::Instant::now() + wait;

            while std::time::Instant::now() < until {
                match commands.recv_timeout(POLL) {
                    Ok(Command::Pair(pairing)) => {
                        self.pairings.insert(pairing.topic, pairing.key);
                    }
                    Ok(Command::Chain(chain)) => self.chain = chain,
                    Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
            }

            wait = (wait * 2).min(BACK_OFF.1);
        }
    }

    fn connect(&self, project: &str, relay: &str, proxy: Option<&str>) -> Result<Connection, String> {
        let seed = random32()?;
        let url = relay_url(relay, &relay_token(&seed, &hex(&random32()?), relay.split('?').next().unwrap_or(relay), now()), project);
        let target = chainv4::mining::wss::Target::parse(&url)?;
        let stream = chainv4::mining::worker::connect_to(&target.address(), proxy)?;
        // The URL carries the token: never in a message.
        let lines = chainv4::mining::wss::open_sized(stream, &target, None, MOST_MESSAGE).map_err(|why| relay_trouble(&why.replace(&url, relay)))?;

        Ok(Connection { out: lines.out, came: lines.came })
    }

    fn topics(&self) -> Vec<String> {
        self.sessions.iter().map(|session| session.topic.clone()).chain(self.pairings.keys().cloned()).collect()
    }

    fn subscribe(&mut self, connection: &Connection, topics: Vec<String>) -> Result<(), String> {
        if topics.is_empty() {
            return Ok(());
        }

        connection.send(&json!({ "id": payload_id(now_ms()), "jsonrpc": "2.0", "method": "irn_batchSubscribe", "params": { "topics": topics } }))?;
        let fetch = payload_id(now_ms()) + 1;
        self.fetches.insert(fetch);

        connection.send(&json!({ "id": fetch, "jsonrpc": "2.0", "method": "irn_batchFetchMessages", "params": { "topics": topics } }))
    }

    /// One connection, until it goes or the app does: Ok for the app gone.
    fn session(&mut self, connection: &Connection, commands: &mpsc::Receiver<Command>) -> Result<(), String> {
        self.subscribe(connection, self.topics())?;

        loop {
            loop {
                match commands.try_recv() {
                    Ok(command) => self.command(connection, command)?,
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => return Ok(()),
                }
            }

            match connection.came.recv_timeout(POLL) {
                Ok(Ok(line)) => self.line(connection, &line)?,
                Ok(Err(why)) => return Err(format!("the relay: {why}")),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return Err("the relay's connection closed".to_string()),
            }
        }
    }

    fn command(&mut self, connection: &Connection, command: Command) -> Result<(), String> {
        match command {
            Command::Pair(pairing) => {
                let topic = pairing.topic.clone();
                self.pairings.insert(pairing.topic, pairing.key);
                self.subscribe(connection, vec![topic])
            }
            Command::Answer(local, answer) => {
                lock(&self.shared).pending.retain(|pending| pending["id"].as_u64() != Some(local));
                self.answer(connection, local, answer)
            }
            Command::Disconnect(topic) => {
                if let Some(key) = self.sessions.iter().find(|session| session.topic == topic).and_then(Session::key) {
                    let delete = json!({ "id": payload_id(now_ms()), "jsonrpc": "2.0", "method": "wc_sessionDelete", "params": { "code": 6000, "message": "User disconnected." } });
                    connection.publish(&topic, &key, &delete, tags::DELETE)?;
                }

                self.sessions.retain(|session| session.topic != topic);
                self.keep_sessions();

                Ok(())
            }
            Command::Chain(chain) => {
                if chain == self.chain {
                    return Ok(());
                }

                self.chain = chain;

                for session in &mut self.sessions {
                    session.chain = chain;
                }

                for session in self.sessions.clone() {
                    let Some(key) = session.key() else { continue };
                    let update = json!({ "id": payload_id(now_ms()), "jsonrpc": "2.0", "method": "wc_sessionUpdate", "params": { "namespaces": session.namespaces() } });
                    connection.publish(&session.topic, &key, &update, tags::UPDATE)?;
                    let event = json!({ "id": payload_id(now_ms()), "jsonrpc": "2.0", "method": "wc_sessionEvent", "params": { "event": { "name": "chainChanged", "data": chain }, "chainId": format!("eip155:{chain}") } });
                    connection.publish(&session.topic, &key, &event, tags::EVENT)?;
                }

                self.keep_sessions();

                Ok(())
            }
        }
    }

    fn answer(&mut self, connection: &Connection, local: u64, answer: crate::dapp::Answer) -> Result<(), String> {
        match self.waiting.remove(&local) {
            Some(Waiting::Proposal(proposal)) => self.settle(connection, &proposal, answer),
            Some(Waiting::Request { topic, id }) => {
                let Some(key) = self.sessions.iter().find(|session| session.topic == topic).and_then(Session::key) else {
                    return Ok(());
                };
                let payload = match answer {
                    Ok(value) => result(id, value),
                    Err((code, message)) => error(id, code, &message),
                };

                connection.publish(&topic, &key, &payload, tags::REQUEST_RESPONSE)
            }
            None => Ok(()),
        }
    }

    /// A connect answered: accounts approve the proposal, anything else turns it down.
    fn settle(&mut self, connection: &Connection, proposal: &Proposal, answer: crate::dapp::Answer) -> Result<(), String> {
        let Some(pairing_key) = self.pairings.get(&proposal.pairing).copied() else {
            return Ok(());
        };
        let accounts: Vec<String> = match &answer {
            Ok(Value::Array(accounts)) => accounts.iter().filter_map(Value::as_str).map(str::to_lowercase).collect(),
            _ => Vec::new(),
        };

        if accounts.is_empty() {
            let (code, message) = answer.err().unwrap_or((5000, "User rejected.".to_string()));

            return connection.publish(&proposal.pairing, &pairing_key, &error(proposal.id, if code == 4001 { 5000 } else { code }, &message), tags::PROPOSE_REJECT);
        }

        let approved = approve(proposal, &random32()?, self.chain, accounts, now(), payload_id(now_ms()));
        let key = approved.session.key().expect("its own key");
        self.subscribe(connection, vec![approved.session.topic.clone()])?;
        connection.publish(&approved.session.topic, &key, &approved.settle, tags::SETTLE)?;
        connection.publish(&proposal.pairing, &pairing_key, &approved.answer, tags::PROPOSE_RESPONSE)?;
        self.sessions.push(approved.session);
        self.keep_sessions();

        Ok(())
    }

    /// A line from the relay: a message to a topic, the messages a fetch found, or an answer to
    /// what the wallet asked of it.
    fn line(&mut self, connection: &Connection, line: &str) -> Result<(), String> {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            return Ok(());
        };

        if value["method"] == "irn_subscription" {
            // Taken, so the relay does not send it again.
            connection.send(&result(value["id"].as_u64().unwrap_or(0), json!(true)))?;
            let data = &value["params"]["data"];

            return self.message(connection, data["topic"].as_str().unwrap_or_default(), data["message"].as_str().unwrap_or_default());
        }

        if let Some(id) = value["id"].as_u64()
            && self.fetches.remove(&id)
        {
            for found in value["result"]["messages"].as_array().into_iter().flatten() {
                self.message(connection, found["topic"].as_str().unwrap_or_default(), found["message"].as_str().unwrap_or_default())?;
            }
        }

        if let Some(why) = value["error"]["message"].as_str() {
            self.say(true, Some(format!("the relay said: {why}")));
        }

        Ok(())
    }

    fn queue(&mut self, waiting: Waiting, request: Value) {
        let local = request["id"].as_u64().unwrap_or_default();
        self.waiting.insert(local, waiting);
        lock(&self.shared).pending.push(request);
        self.wake.request_repaint();
    }

    fn local(&mut self) -> u64 {
        self.next += 1;

        self.next
    }

    /// A sealed message to one of the topics.
    fn message(&mut self, connection: &Connection, topic: &str, sealed: &str) -> Result<(), String> {
        let session = self.sessions.iter().find(|session| session.topic == topic).cloned();
        let Some(key) = session.as_ref().and_then(Session::key).or_else(|| self.pairings.get(topic).copied()) else {
            return Ok(());
        };
        let Ok(payload) = open(&key, sealed).and_then(|text| serde_json::from_str::<Value>(&text).map_err(|err| err.to_string())) else {
            return Ok(());
        };
        let (Some(id), Some(method)) = (payload["id"].as_u64(), payload["method"].as_str()) else {
            // An answer to the wallet, such as the site taking the settle: nothing to do.
            return Ok(());
        };

        if !self.seen.insert((topic.to_string(), id)) {
            return Ok(());
        }

        let params = &payload["params"];

        match (method, &session) {
            ("wc_sessionPropose", None) => match Proposal::read(id, topic, params) {
                Ok(proposal) => match proposal.refused_on(self.chain) {
                    Some(why) => connection.publish(topic, &key, &error(id, 5100, &format!("Unsupported chains: {why}")), tags::PROPOSE_REJECT),
                    None => {
                        let local = self.local();
                        let request = json!({ "id": local, "origin": proposal.metadata.origin(), "method": "eth_requestAccounts", "params": [] });
                        self.queue(Waiting::Proposal(proposal), request);

                        Ok(())
                    }
                },
                Err(why) => connection.publish(topic, &key, &error(id, 5000, &why), tags::PROPOSE_REJECT),
            },
            ("wc_sessionRequest", Some(session)) => {
                let local = self.local();

                match request_for(session, local, params) {
                    Ok(request) => {
                        self.queue(Waiting::Request { topic: topic.to_string(), id }, request);

                        Ok(())
                    }
                    Err((code, why)) => connection.publish(topic, &key, &error(id, code, &why), tags::REQUEST_RESPONSE),
                }
            }
            ("wc_sessionPing", Some(_)) => connection.publish(topic, &key, &result(id, json!(true)), tags::PING_RESPONSE),
            ("wc_pairingPing", None) => connection.publish(topic, &key, &result(id, json!(true)), tags::PAIRING_PING_RESPONSE),
            ("wc_sessionDelete", Some(_)) => {
                connection.publish(topic, &key, &result(id, json!(true)), tags::DELETE_RESPONSE)?;
                self.sessions.retain(|session| session.topic != topic);
                self.keep_sessions();

                Ok(())
            }
            ("wc_pairingDelete", None) => {
                self.pairings.remove(topic);

                Ok(())
            }
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests;
