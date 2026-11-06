//! WalletConnect v2, the wallet's side, for sites on the desktop and Android apps, which have no
//! extension: a site shows a wc: code, the wallet reads it, and the two talk through a relay,
//! everything between them sealed with keys the relay never sees.
//!
//! - **Pairing:** the code names a topic and a key. The wallet subscribes to the topic and reads
//!   the site's proposal, sealed with that key.
//! - **A session:** approved, the wallet answers with an X25519 key of its own; HKDF-SHA256 of the
//!   shared secret is the session's key, and its SHA-256 the session's topic. Everything after goes
//!   there, as ChaCha20-Poly1305 envelopes (type 0: a 0 byte, a 12-byte nonce, the sealed box, in
//!   base64).
//! - **The relay** is Reown's, or another named in Settings, reached over wss:// with a token the
//!   wallet signs with an Ed25519 key of its own (did:key), and the project id Settings gives:
//!   without one, WalletConnect is off. Through the wallet's proxy when one is set.
//!
//! Sites ask exactly what they ask through the extension, each shown and approved as there
//! (views::dapp): connecting, sending, personal_sign, eth_signTypedData_v4, switching chains and
//! adding tokens. Sessions are kept sealed with the accounts, until the site or the wallet ends
//! them, or they expire.
//!
//! The protocol is WalletConnect's as its reference client speaks it (the sign client 2.x): its
//! crypto, envelopes, relay methods, tags and lifetimes. A site's metadata, its name and URL, is
//! its own word: the approval says so.

use {
    base64::Engine as _,
    chacha20poly1305::aead::{Aead, KeyInit},
    chacha20poly1305::{ChaCha20Poly1305, Nonce},
    curve25519_dalek::montgomery::MontgomeryPoint,
    hkdf::Hkdf,
    serde::{Deserialize, Serialize},
    serde_json::{Value, json},
    sha2::{Digest, Sha256},
};

mod client;

pub use client::WalletConnect;

/// Reown's relay, the one sites' WalletConnect reaches by default.
pub const RELAY: &str = "wss://relay.walletconnect.org";

/// How long a session lasts, as the reference client makes them: a week.
pub const SESSION_EXPIRY: u64 = 7 * 24 * 3_600;

/// The methods a session offers: what the extension offers sites.
pub const METHODS: [&str; 6] = ["eth_sendTransaction", "personal_sign", "eth_signTypedData_v4", "wallet_switchEthereumChain", "wallet_addEthereumChain", "wallet_watchAsset"];
pub const EVENTS: [&str; 2] = ["chainChanged", "accountsChanged"];

/// What each message is tagged and kept for at the relay, as the sign client has them: (tag, ttl in
/// seconds).
pub mod tags {
    pub const PROPOSE_RESPONSE: (u32, u64) = (1101, 300);
    pub const PROPOSE_REJECT: (u32, u64) = (1120, 300);
    pub const SETTLE: (u32, u64) = (1102, 300);
    pub const UPDATE: (u32, u64) = (1104, 86_400);
    pub const REQUEST_RESPONSE: (u32, u64) = (1109, 900);
    pub const EVENT: (u32, u64) = (1110, 300);
    pub const DELETE: (u32, u64) = (1112, 86_400);
    pub const DELETE_RESPONSE: (u32, u64) = (1113, 86_400);
    pub const PING_RESPONSE: (u32, u64) = (1115, 86_400);
    pub const PAIRING_PING_RESPONSE: (u32, u64) = (1003, 30);
}

/// Hex as WalletConnect writes keys and topics: lowercase, no 0x.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    chainv4::structs::hex::from_hex(text.trim_start_matches("0x"))
}

fn key32(text: &str) -> Option<[u8; 32]> {
    unhex(text)?.try_into().ok()
}

/// 32 random bytes, from the system.
pub fn random32() -> Result<[u8; 32], String> {
    let mut bytes = [0; 32];
    getrandom::fill(&mut bytes).map_err(|err| format!("no randomness: {err}"))?;

    Ok(bytes)
}

/// An X25519 secret's public key.
pub fn x25519_public(secret: &[u8; 32]) -> [u8; 32] {
    MontgomeryPoint::mul_base_clamped(*secret).to_bytes()
}

/// The key two sides share: HKDF-SHA256 of their X25519 secret, with no salt and no info.
pub fn sym_key(secret: &[u8; 32], peer: &[u8; 32]) -> [u8; 32] {
    let shared = MontgomeryPoint(*peer).mul_clamped(*secret).to_bytes();
    let mut key = [0; 32];
    Hkdf::<Sha256>::new(None, &shared).expand(&[], &mut key).expect("32 bytes is a length HKDF gives");

    key
}

/// The topic a key's messages go to: its SHA-256.
pub fn topic_of(key: &[u8; 32]) -> String {
    hex(&Sha256::digest(key))
}

/// A message sealed for whoever holds the key: a type 0 envelope, in base64.
pub fn seal(key: &[u8; 32], message: &str, iv: [u8; 12]) -> String {
    let sealed = ChaCha20Poly1305::new(key.into()).encrypt(Nonce::from_slice(&iv), message.as_bytes()).expect("ChaCha20-Poly1305 seals any message");

    base64::engine::general_purpose::STANDARD.encode([&[0][..], &iv, &sealed].concat())
}

/// A sealed message opened: type 0 alone, as a session's and a pairing's are.
pub fn open(key: &[u8; 32], message: &str) -> Result<String, String> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(message.trim()).map_err(|_| "a message that is not base64".to_string())?;

    match bytes.first() {
        Some(0) if bytes.len() > 1 + 12 + 16 => {}
        Some(0) => return Err("a message too short to be sealed".to_string()),
        Some(kind) => return Err(format!("an envelope of type {kind}, which this wallet does not take")),
        None => return Err("an empty message".to_string()),
    }

    let opened = ChaCha20Poly1305::new(key.into()).decrypt(Nonce::from_slice(&bytes[1..13]), &bytes[13..]).map_err(|_| "a message not sealed with the key".to_string())?;

    String::from_utf8(opened).map_err(|_| "a message that is not text".to_string())
}

/// An Ed25519 key as a DID, as the relay names a client: did:key:z and base58 of 0xed01 and the key.
pub fn did_key(public: &[u8; 32]) -> String {
    format!("did:key:z{}", bs58::encode([&[0xed, 0x01][..], public].concat()).into_string())
}

/// The token the relay is reached with: a JWT signed with the client's Ed25519 key (EdDSA), naming
/// it as did:key, for the relay's URL, for a day.
pub fn relay_token(seed: &[u8; 32], subject: &str, relay: &str, now: u64) -> String {
    use chainv4::crypto::keypair::{Keypair, PrivateKey, PublicKey, Signature};

    let key = Keypair::from_private(PrivateKey::Ed25519(*seed));
    let PublicKey::Ed25519(public) = key.public() else {
        unreachable!("an Ed25519 key")
    };
    let url = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let header = url.encode(json!({ "alg": "EdDSA", "typ": "JWT" }).to_string());
    let payload = url.encode(json!({ "iss": did_key(&public), "sub": subject, "aud": relay, "iat": now, "exp": now + 86_400 }).to_string());
    let data = format!("{header}.{payload}");
    let Signature::Ed25519(signature) = key.sign(data.as_bytes()) else {
        unreachable!("an Ed25519 signature")
    };

    format!("{data}.{}", url.encode(signature))
}

/// The relay's URL to connect to: its token, project and who connects.
pub fn relay_url(relay: &str, token: &str, project: &str) -> String {
    let joiner = if relay.contains('?') { '&' } else { '?' };

    format!("{relay}{joiner}auth={token}&ua=wc-2/rust-chainv4/{}&projectId={project}", std::env::consts::OS)
}

/// A JSON-RPC id as the reference client makes them: the time in milliseconds, then three digits.
pub fn payload_id(now_ms: u64) -> u64 {
    let mut extra = [0; 2];
    let _ = getrandom::fill(&mut extra);

    now_ms * 1_000 + u64::from(u16::from_le_bytes(extra) % 1_000)
}

/// A pairing, as a site's code names it: wc:TOPIC@2?relay-protocol=irn&symKey=KEY.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pairing {
    pub topic: String,
    pub key: [u8; 32],
    /// When it stops, in seconds, if the code says.
    pub expiry: Option<u64>,
}

impl Pairing {
    pub fn parse(text: &str) -> Result<Pairing, String> {
        let text = text.trim();
        let rest = text.strip_prefix("wc:").ok_or("a WalletConnect code starts wc:")?;
        let (topic, query) = rest.split_once('?').ok_or("a WalletConnect code has its key after a ?")?;
        let (topic, version) = topic.split_once('@').ok_or("a WalletConnect code says its version after an @")?;

        if version != "2" {
            return Err(format!("WalletConnect {version}: this wallet speaks version 2"));
        }

        if topic.len() != 64 || unhex(topic).is_none() {
            return Err("a WalletConnect code's topic is 64 hex digits".to_string());
        }

        let mut key = None;
        let mut expiry = None;

        for pair in query.split('&') {
            match pair.split_once('=') {
                Some(("symKey", value)) => key = key32(value),
                Some(("expiryTimestamp", value)) => expiry = value.parse().ok(),
                Some(("relay-protocol", value)) if value != "irn" => return Err(format!("a relay protocol of {value}: this wallet speaks irn")),
                _ => {}
            }
        }

        Ok(Pairing { topic: topic.to_string(), key: key.ok_or("a WalletConnect code's key is 64 hex digits")?, expiry })
    }
}

/// What a site says of itself: its own word.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Metadata {
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub icons: Vec<String>,
}

impl Metadata {
    fn read(value: &Value) -> Metadata {
        let text = |name: &str, most: usize| value[name].as_str().unwrap_or_default().chars().filter(|c| !c.is_control()).take(most).collect::<String>();

        Metadata {
            name: text("name", 64),
            url: text("url", 256),
            description: text("description", 256),
            icons: value["icons"].as_array().into_iter().flatten().filter_map(Value::as_str).take(4).map(str::to_string).collect(),
        }
    }

    /// How the approval names the site: its URL, and that it said so itself.
    pub fn origin(&self) -> String {
        let site = if self.url.is_empty() { self.name.as_str() } else { self.url.as_str() };

        format!("{} (by WalletConnect, as it says)", if site.is_empty() { "a site" } else { site })
    }
}

/// This wallet, as sites see it.
pub fn our_metadata() -> Value {
    json!({ "name": "chainv4 wallet", "description": "The chainv4 light wallet", "url": "https://chainv4.invalid", "icons": [] })
}

/// A site's session proposal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proposal {
    /// Its JSON-RPC id, which the answer goes back by.
    pub id: u64,
    pub pairing: String,
    pub public: [u8; 32],
    pub metadata: Metadata,
    /// The chains it must have, and those it would take, as eip155:N.
    pub required: Vec<String>,
    pub optional: Vec<String>,
}

/// The chains a proposal's namespaces name: each eip155 namespace's chains, and a namespace that
/// is itself a chain, eip155:1.
fn chains(namespaces: &Value) -> Vec<String> {
    let mut chains = Vec::new();

    for (name, namespace) in namespaces.as_object().into_iter().flatten() {
        if name == "eip155" {
            chains.extend(namespace["chains"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string));
        } else if name.starts_with("eip155:") {
            chains.push(name.clone());
        } else {
            // Another family altogether: kept as itself, so a required one is refused.
            chains.push(format!("{name}:*"));
        }
    }

    chains
}

impl Proposal {
    pub fn read(id: u64, pairing: &str, params: &Value) -> Result<Proposal, String> {
        let public = params["proposer"]["publicKey"].as_str().and_then(key32).ok_or("a proposal with no key")?;

        Ok(Proposal {
            id,
            pairing: pairing.to_string(),
            public,
            metadata: Metadata::read(&params["proposer"]["metadata"]),
            required: chains(&params["requiredNamespaces"]),
            optional: chains(&params["optionalNamespaces"]),
        })
    }

    /// Why it cannot be had on this chain: a chain it requires that is not this one.
    pub fn refused_on(&self, chain: u64) -> Option<String> {
        let ours = format!("eip155:{chain}");

        self.required.iter().find(|required| **required != ours).map(|other| format!("it requires {other}, and this wallet follows {ours}"))
    }
}

/// A session, as the wallet keeps it, sealed with the accounts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub topic: String,
    /// Its key, in hex: what reads and seals everything in it.
    pub key: String,
    pub peer: Metadata,
    /// The accounts it sees, 0x and 40 hex digits.
    pub accounts: Vec<String>,
    pub chain: u64,
    /// When it ends, in seconds.
    pub expiry: u64,
}

impl Session {
    pub fn key(&self) -> Option<[u8; 32]> {
        key32(&self.key)
    }

    /// Its namespace, as settle and update send it.
    pub fn namespaces(&self) -> Value {
        let chain = format!("eip155:{}", self.chain);

        json!({ "eip155": {
            "chains": [chain],
            "accounts": self.accounts.iter().map(|account| format!("{chain}:{account}")).collect::<Vec<_>>(),
            "methods": METHODS,
            "events": EVENTS,
        } })
    }
}

/// An approved proposal: the session, the settle request for its topic, and the answer to the
/// proposal for the pairing's, as the reference client sends them.
pub struct Approved {
    pub session: Session,
    pub settle: Value,
    pub answer: Value,
}

pub fn approve(proposal: &Proposal, secret: &[u8; 32], chain: u64, accounts: Vec<String>, now: u64, settle_id: u64) -> Approved {
    let ours = x25519_public(secret);
    let key = sym_key(secret, &proposal.public);
    let session = Session { topic: topic_of(&key), key: hex(&key), peer: proposal.metadata.clone(), accounts, chain, expiry: now + SESSION_EXPIRY };
    let settle = json!({
        "id": settle_id,
        "jsonrpc": "2.0",
        "method": "wc_sessionSettle",
        "params": {
            "relay": { "protocol": "irn" },
            "namespaces": session.namespaces(),
            "controller": { "publicKey": hex(&ours), "metadata": our_metadata() },
            "expiry": session.expiry,
        },
    });
    let answer = json!({ "id": proposal.id, "jsonrpc": "2.0", "result": { "relay": { "protocol": "irn" }, "responderPublicKey": hex(&ours) } });

    Approved { session, settle, answer }
}

/// A JSON-RPC error answer.
pub fn error(id: u64, code: i64, message: &str) -> Value {
    json!({ "id": id, "jsonrpc": "2.0", "error": { "code": code, "message": message } })
}

pub fn result(id: u64, result: Value) -> Value {
    json!({ "id": id, "jsonrpc": "2.0", "result": result })
}

/// A site's request in a session, as the approval reads it: { id, origin, method, params }, or why
/// it is answered at once without asking.
pub fn request_for(session: &Session, local: u64, params: &Value) -> Result<Value, (i64, String)> {
    let method = params["request"]["method"].as_str().unwrap_or_default();
    let asked = &params["request"]["params"];

    if params["chainId"].as_str() != Some(format!("eip155:{}", session.chain).as_str()) && !method.starts_with("wallet_") {
        return Err((5100, format!("this session is on eip155:{}", session.chain)));
    }

    if !METHODS.contains(&method) {
        return Err((4200, format!("{method} is not supported")));
    }

    // A signature or send names an account, which must be one the session sees.
    let named = match method {
        "eth_sendTransaction" => asked[0]["from"].as_str(),
        "personal_sign" => [&asked[1], &asked[0]].into_iter().filter_map(Value::as_str).find(|value| value.len() == 42 && value.starts_with("0x")),
        "eth_signTypedData_v4" => asked[0].as_str(),
        _ => None,
    };

    if let Some(named) = named
        && !session.accounts.iter().any(|account| account.eq_ignore_ascii_case(named))
    {
        return Err((4100, "the account it names is not one this session sees".to_string()));
    }

    Ok(json!({ "id": local, "origin": session.peer.origin(), "method": method, "params": asked }))
}

#[cfg(test)]
mod tests;
