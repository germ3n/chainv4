use super::*;

/// The wallet's side running over channels, with the test playing the relay and the site.
struct Rig {
    run: Run,
    connection: Connection,
    sent: mpsc::Receiver<String>,
}

fn rig() -> Rig {
    let (out, sent) = mpsc::sync_channel(64);
    let (_, came) = mpsc::sync_channel(1);
    let run = Run {
        shared: Arc::new(Mutex::new(Shared::default())),
        wake: egui::Context::default(),
        sessions: Vec::new(),
        pairings: HashMap::new(),
        waiting: HashMap::new(),
        next: 1 << 40,
        chain: 8908,
        seen: HashSet::new(),
        fetches: HashSet::new(),
    };

    Rig { run, connection: Connection { out, came }, sent }
}

impl Rig {
    /// What the wallet sent the relay since last asked.
    fn sent(&self) -> Vec<Value> {
        self.sent.try_iter().map(|line| serde_json::from_str(&line).unwrap()).collect()
    }

    /// A site's message to a topic, as the relay hands it on.
    fn deliver(&mut self, topic: &str, key: &[u8; 32], payload: Value) {
        let message = seal(key, &payload.to_string(), [4; 12]);
        let line = json!({ "id": 99, "jsonrpc": "2.0", "method": "irn_subscription", "params": { "id": "sub", "data": { "topic": topic, "message": message, "publishedAt": 1, "tag": 1100 } } });
        self.run.line(&self.connection, &line.to_string()).unwrap();
    }

    fn pending(&self) -> Vec<Value> {
        lock(&self.run.shared).pending.clone()
    }
}

/// What a publish carries, opened with the key.
fn opened(publish: &Value, key: &[u8; 32]) -> Value {
    assert_eq!(publish["method"], "irn_publish", "{publish}");
    serde_json::from_str(&open(key, publish["params"]["message"].as_str().unwrap()).unwrap()).unwrap()
}

#[test]
fn a_site_pairs_is_approved_asks_is_answered_moves_chain_and_leaves() {
    let mut rig = rig();
    let pairing = Pairing { topic: "ab".repeat(32), key: [0xcd; 32], expiry: None };
    let account = format!("0x{}", "ab".repeat(20));

    // Paired: the topic subscribed, and what waits there fetched.
    rig.run.command(&rig.connection, Command::Pair(pairing.clone())).unwrap();
    let sent = rig.sent();
    assert_eq!((sent[0]["method"].as_str(), &sent[0]["params"]["topics"]), (Some("irn_batchSubscribe"), &json!([pairing.topic])));
    assert_eq!(sent[1]["method"], "irn_batchFetchMessages");

    // The site's proposal: taken once, acknowledged, and waiting as a connect.
    let site_secret = [5; 32];
    let proposal = json!({ "id": 42, "jsonrpc": "2.0", "method": "wc_sessionPropose", "params": {
        "relays": [{ "protocol": "irn" }],
        "proposer": { "publicKey": hex(&x25519_public(&site_secret)), "metadata": { "name": "S", "url": "https://s.example" } },
        "requiredNamespaces": {}, "optionalNamespaces": { "eip155": { "chains": ["eip155:8908"] } },
    } });
    rig.deliver(&pairing.topic, &pairing.key, proposal.clone());
    rig.deliver(&pairing.topic, &pairing.key, proposal);
    assert_eq!(rig.sent().iter().filter(|line| line["result"] == true).count(), 2, "each delivery acknowledged");
    let pending = rig.pending();
    assert_eq!(pending.len(), 1, "taken once");
    assert_eq!((pending[0]["method"].as_str(), pending[0]["origin"].as_str()), (Some("eth_requestAccounts"), Some("https://s.example (by WalletConnect, as it says)")));

    // Approved with an account: the session's topic subscribed, settled there, and the proposal
    // answered on the pairing's, as the site reads them.
    let local = pending[0]["id"].as_u64().unwrap();
    rig.run.command(&rig.connection, Command::Answer(local, Ok(json!([account.to_uppercase().replace("0X", "0x")])))).unwrap();
    let sent = rig.sent();
    let session = rig.run.sessions[0].clone();
    assert_eq!(sent[0]["params"]["topics"], json!([session.topic]));
    let answer = opened(&sent[3], &pairing.key);
    assert_eq!((sent[3]["params"]["topic"].as_str(), sent[3]["params"]["tag"].as_u64(), answer["id"].as_u64()), (Some(pairing.topic.as_str()), Some(1101), Some(42)));
    let key = sym_key(&site_secret, &key32(answer["result"]["responderPublicKey"].as_str().unwrap()).unwrap());
    let settle = opened(&sent[2], &key);
    assert_eq!((sent[2]["params"]["topic"].as_str(), sent[2]["params"]["tag"].as_u64()), (Some(session.topic.as_str()), Some(1102)));
    assert_eq!(settle["params"]["namespaces"]["eip155"]["accounts"], json!([format!("eip155:8908:{account}")]), "accounts kept in lowercase");
    assert!(lock(&rig.run.shared).changed, "a session to keep");

    // A request in it waits for the approval, and its answer goes back sealed.
    rig.deliver(&session.topic, &key, json!({ "id": 7, "jsonrpc": "2.0", "method": "wc_sessionRequest", "params": { "request": { "method": "personal_sign", "params": ["0x6869", account] }, "chainId": "eip155:8908" } }));
    rig.sent();
    let asked = rig.pending().into_iter().find(|pending| pending["method"] == "personal_sign").unwrap();
    rig.run.command(&rig.connection, Command::Answer(asked["id"].as_u64().unwrap(), Ok(json!("0x5151")))).unwrap();
    let sent = rig.sent();
    assert_eq!(opened(&sent[0], &key), json!({ "id": 7, "jsonrpc": "2.0", "result": "0x5151" }));
    assert_eq!(sent[0]["params"]["tag"], 1109);

    // One it may not ask is answered at once, with no approval.
    rig.deliver(&session.topic, &key, json!({ "id": 8, "jsonrpc": "2.0", "method": "wc_sessionRequest", "params": { "request": { "method": "eth_sign", "params": [] }, "chainId": "eip155:8908" } }));
    let sent = rig.sent();
    assert_eq!(opened(&sent[1], &key)["error"]["code"], 4200);

    // The wallet moving to another chain moves the session, and tells the site.
    rig.run.command(&rig.connection, Command::Chain(1)).unwrap();
    let sent = rig.sent();
    assert_eq!(opened(&sent[0], &key)["params"]["namespaces"]["eip155"]["chains"], json!(["eip155:1"]));
    assert_eq!(opened(&sent[1], &key)["params"]["event"], json!({ "name": "chainChanged", "data": 1 }));

    // The site leaving ends it.
    rig.deliver(&session.topic, &key, json!({ "id": 9, "jsonrpc": "2.0", "method": "wc_sessionDelete", "params": { "code": 6000, "message": "bye" } }));
    let sent = rig.sent();
    assert_eq!(opened(&sent[1], &key)["result"], true);
    assert!(rig.run.sessions.is_empty());
}

#[test]
fn a_proposal_turned_down_or_on_another_chain_is_answered_on_its_pairing() {
    let mut rig = rig();
    let pairing = Pairing { topic: "ef".repeat(32), key: [0x11; 32], expiry: None };
    rig.run.command(&rig.connection, Command::Pair(pairing.clone())).unwrap();
    rig.sent();
    let propose = |id: u64, required: Value| json!({ "id": id, "jsonrpc": "2.0", "method": "wc_sessionPropose", "params": { "proposer": { "publicKey": hex(&[9; 32]), "metadata": {} }, "requiredNamespaces": required } });

    rig.deliver(&pairing.topic, &pairing.key, propose(1, json!({ "eip155": { "chains": ["eip155:1"] } })));
    let sent = rig.sent();
    let refused = opened(&sent[1], &pairing.key);
    assert_eq!((refused["id"].as_u64(), refused["error"]["code"].as_i64(), sent[1]["params"]["tag"].as_u64()), (Some(1), Some(5100), Some(1120)));
    assert!(rig.pending().is_empty());

    rig.deliver(&pairing.topic, &pairing.key, propose(2, json!({})));
    rig.sent();
    let local = rig.pending()[0]["id"].as_u64().unwrap();
    rig.run.command(&rig.connection, Command::Answer(local, crate::dapp::rejected())).unwrap();
    let rejected = opened(&rig.sent()[0], &pairing.key);
    assert_eq!((rejected["id"].as_u64(), rejected["error"]["code"].as_i64()), (Some(2), Some(5000)));
    assert!(rig.run.sessions.is_empty());

    // A message sealed with another key, or to a topic never subscribed, is dropped.
    rig.deliver(&pairing.topic, &[0x22; 32], propose(3, json!({})));
    rig.deliver(&"00".repeat(32), &pairing.key, propose(4, json!({})));
    assert!(rig.pending().is_empty());
}

#[test]
fn what_the_relay_says_reads_in_its_own_terms() {
    assert!(relay_trouble("wss://relay.walletconnect.org answered 401 rather than a WebSocket").contains("project id"));
    assert!(relay_trouble("wss://x is no mining WebSocket: the node takes no workers").contains("no relay"));
    assert_eq!(relay_trouble("the proxy 127.0.0.1:9050: refused"), "the proxy 127.0.0.1:9050: refused");
}
