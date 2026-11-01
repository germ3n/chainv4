//! The mining port's protocol: lines of JSON over TCP, between a node and the workers mining for
//! it, which share a token.
//!
//! **The handshake** proves each side knows the token without sending it. The node sends a random
//! challenge; the worker answers with its name, payout and whether it mines alone, a random nonce
//! of its own, and an HMAC of all of it under the token; the node answers with what it took the
//! payout to be and its own HMAC of all that, and the worker stops if that payout is not its own.
//! The session key is a third HMAC of them, so it differs every connection. So nothing between them
//! changes where a worker's work pays, while the token is a secret. An open port's token is public:
//! anyone on the path can make these MACs too, so only the transport, wss:// or a private network,
//! protects a worker's payout there.
//!
//! **Every message after** is `MAC JSON`: an HMAC under the session key of its direction, its
//! number in that direction, and the JSON. A message changed, made up, replayed or dropped does not
//! check, and the connection ends.
//!
//! **The messages:**
//! - `work`, from the node: a job, the block id to seal, its difficulty and work hash, and the
//!   range of nonces this worker tries, so no two workers try the same ones.
//! - `found`, from a worker: a nonce for a job; `verdict`, from the node: whether it took it.
//! - `rate`, from a worker: its devices and their hashrates, for the node's report.
//!
//! **Under a pool**, the worker names its payout address in the handshake; the node answers with
//! `pool`, its fee and window, or `refused` for a worker without one. Each `work` carries the
//! worker's `share` difficulty, every nonce that meets it goes back as `found`, and the verdict
//! says whether it was a `share` or the `block`; every 30 seconds `standing` tells the worker its
//! work in the window and its parts of the next block.

use {
    r#macro::document,
    hmac::{Hmac, KeyInit, Mac},
    serde_json::{Value, json},
    sha2::Sha256,
};

#[cfg(not(target_arch = "wasm32"))]
use {
    std::io::{BufRead, BufReader, Write},
    std::net::TcpStream,
};

/// What both sides say first, so a stray connection, or a worker of a chain under another
/// protocol name, is told apart: chainv4-mining/2 for the default name.
pub fn version(protocol: &str) -> String {
    format!("{protocol}-mining/2")
}

/// What a worker's error starts with when the node says it pays another address than the worker's
/// own: it stops, as connecting again would only mine for that address or nothing.
pub const PAYOUT_DIFFERS: &str = "the node says it would pay";

/// Whether what a node says it pays, in its standing, is another address than this worker's own.
pub fn pays_another(message: &Value, payout: Option<&crate::structs::address::Address>) -> Option<String> {
    let said = message["payout"].as_str()?;
    let ours = payout?.to_string();

    (said != ours).then(|| format!("{PAYOUT_DIFFERS} {said}, and this worker's payout is {ours}"))
}

/// The longest line either side reads: a work or rate message is far shorter.
pub const MOST_LINE: u64 = 64 * 1024;

/// An HMAC-SHA256 of these parts, one after the other, under a key.
fn mac(key: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(key).expect("HMAC takes a key of any length");

    for part in parts {
        mac.update(part);
    }

    mac.finalize().into_bytes().into()
}

/// Whether a MAC is the one expected, in constant time.
fn checks(key: &[u8], parts: &[&[u8]], given: &[u8]) -> bool {
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(key).expect("HMAC takes a key of any length");

    for part in parts {
        mac.update(part);
    }

    mac.verify_slice(given).is_ok()
}

fn random() -> Result<[u8; 32], String> {
    let mut bytes = [0; 32];
    getrandom::fill(&mut bytes).map_err(|err| format!("no randomness: {err}"))?;

    Ok(bytes)
}

fn hex(bytes: &[u8]) -> String {
    crate::structs::hex::to_hex(bytes)
}

fn unhex<const N: usize>(value: &Value) -> Result<[u8; N], String> {
    value
        .as_str()
        .and_then(crate::structs::hex::from_hex)
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or_else(|| "a field that does not read".to_string())
}

/// Which way a message goes, as its MAC says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Way {
    ToWorker,
    ToNode,
}

impl Way {
    fn tag(self) -> &'static [u8] {
        match self {
            Way::ToWorker => b"to worker",
            Way::ToNode => b"to node",
        }
    }
}

#[document(
    summary = "The token every worker of an open port proves it knows, which anyone may: --mining-open's, and a worker's without --token.",
    category = "Mining",
    see_also = "MiningPort",
)]
pub const OPEN_TOKEN: &[u8] = b"chainv4 open mining pool";

/// How long a worker may say nothing before it is taken for gone: it sends its rates more often.
pub const QUIET: std::time::Duration = std::time::Duration::from_secs(60);
/// How often the node writes to a worker with no new work, so the worker knows it is still there.
pub const PING: std::time::Duration = std::time::Duration::from_secs(20);

/// The MACs of the messages one side sends, each over its direction, its number in that direction
/// and its text, under the session key: whatever carries the lines, a TCP connection or a
/// WebSocket's messages.
pub struct Sealer {
    key: [u8; 32],
    way: Way,
    sent: u64,
}

impl Sealer {
    /// A message as the line that carries it, `MAC JSON`, without its end.
    pub fn seal(&mut self, message: &Value) -> String {
        let text = message.to_string();
        let mac = mac(&self.key, &[self.way.tag(), &self.sent.to_le_bytes(), text.as_bytes()]);
        self.sent += 1;

        format!("{} {text}", hex(&mac))
    }
}

/// Checks the MACs of the messages one side receives, in the order they were sent.
pub struct Opener {
    key: [u8; 32],
    way: Way,
    received: u64,
}

impl Opener {
    /// The message a line carries, once its MAC checks; an error for one that does not.
    pub fn open(&mut self, line: &str) -> Result<Value, String> {
        let (mac, text) = line.split_once(' ').ok_or("a message without its MAC")?;
        let mac = crate::structs::hex::from_hex(mac).ok_or("a MAC that does not read")?;

        if !checks(&self.key, &[self.way.tag(), &self.received.to_le_bytes(), text.as_bytes()], &mac) {
            return Err("a message whose MAC does not check".to_string());
        }

        self.received += 1;

        serde_json::from_str(text).map_err(|_| "a message that does not read".to_string())
    }
}

/// A hello's fields as the MACs take them, each with its length, so no two hellos read alike.
fn hello_fields(version: &str, name: &str, payout: &str, solo: bool) -> Vec<u8> {
    let mut fields = Vec::new();

    for field in [version, name, payout] {
        fields.extend((field.len() as u64).to_le_bytes());
        fields.extend(field.as_bytes());
    }

    fields.push(u8::from(solo));
    fields
}

/// What a worker says of itself in the handshake.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Hello {
    pub name: String,
    /// Where a pool pays its shares, or a solo worker's blocks, if it gave one that reads.
    pub payout: Option<crate::structs::address::Address>,
    /// Whether it mines alone: each block it finds pays its payout in full, on a node that takes
    /// solo workers.
    pub solo: bool,
}

/// The node's greeting, the first line: the protocol it speaks and a random challenge, which it
/// keeps to check the worker's hello.
pub fn greeting(protocol: &str) -> Result<(Value, [u8; 32]), String> {
    let challenge = random()?;

    Ok((json!({ "version": version(protocol), "challenge": hex(&challenge) }), challenge))
}

/// The node's side of a worker's hello: what the worker said of itself, the node's answer, which
/// proves the node knows the token too, and the session's halves; an error for a worker of another
/// protocol or without the token.
pub fn welcome(token: &[u8], protocol: &str, challenge: &[u8; 32], hello: &Value) -> Result<(Hello, Value, Sealer, Opener), String> {
    let version = version(protocol);
    let nonce: [u8; 32] = unhex(&hello["nonce"])?;
    let proof: [u8; 32] = unhex(&hello["proof"])?;

    if hello["version"] != version.as_str() {
        return Err(format!("a worker speaking {}, not {version}", hello["version"]));
    }

    let fields = hello_fields(&version, hello["name"].as_str().unwrap_or_default(), hello["payout"].as_str().unwrap_or_default(), hello["solo"] == true);

    if !checks(token, &[b"worker", challenge, &nonce, &fields], &proof) {
        return Err("a worker without the token, or a hello changed on its way".to_string());
    }

    let said = Hello {
        name: hello["name"].as_str().unwrap_or("worker").chars().filter(|c| !c.is_control()).take(64).collect(),
        payout: hello["payout"].as_str().and_then(|payout| payout.parse().ok()),
        solo: hello["solo"] == true,
    };
    // What it took the payout to be, for the worker to compare with its own.
    let payout = said.payout.map(|payout| payout.to_string()).unwrap_or_default();
    let answer = json!({ "proof": hex(&mac(token, &[b"node", challenge, &nonce, &fields, payout.as_bytes()])), "payout": payout });
    let key = mac(token, &[b"session", challenge, &nonce, &fields]);

    Ok((said, answer, Sealer { key, way: Way::ToWorker, sent: 0 }, Opener { key, way: Way::ToNode, received: 0 }))
}

/// A worker part way through its handshake: it said hello, and waits for the node's proof.
pub struct Joining {
    challenge: [u8; 32],
    nonce: [u8; 32],
    fields: Vec<u8>,
    payout: String,
}

/// The worker's hello to the node's greeting: its name, payout and whether it mines alone, and the
/// proof it knows the token; an error for a node of another protocol.
pub fn hello(greeting: &Value, token: &[u8], protocol: &str, me: &Hello) -> Result<(Value, Joining), String> {
    let version = version(protocol);

    if greeting["version"] != version.as_str() {
        return Err(format!("a node speaking {}, not {version}", greeting["version"]));
    }

    let challenge: [u8; 32] = unhex(&greeting["challenge"])?;
    let nonce = random()?;
    let payout = me.payout.map(|payout| payout.to_string()).unwrap_or_default();
    let fields = hello_fields(&version, &me.name, &payout, me.solo);
    let proof = mac(token, &[b"worker", &challenge, &nonce, &fields]);
    let mut hello = json!({ "version": version, "name": me.name, "nonce": hex(&nonce), "proof": hex(&proof) });

    if let Some(payout) = me.payout {
        hello["payout"] = json!(payout.to_string());
    }

    if me.solo {
        hello["solo"] = json!(true);
    }

    Ok((hello, Joining { challenge, nonce, fields, payout }))
}

impl Joining {
    /// The session's halves once the node's answer proves it knows the token, not the node meant
    /// otherwise, and it would pay this worker's own payout.
    pub fn finish(self, token: &[u8], answer: &Value) -> Result<(Sealer, Opener), String> {
        let theirs: [u8; 32] = unhex(&answer["proof"])?;
        let payout = answer["payout"].as_str().unwrap_or_default();

        if !checks(token, &[b"node", &self.challenge, &self.nonce, &self.fields, payout.as_bytes()], &theirs) {
            return Err("the node does not know the token: not the node meant".to_string());
        }

        if payout != self.payout {
            return Err(format!("{PAYOUT_DIFFERS} {payout}, and this worker's payout is {}", self.payout));
        }

        let key = mac(token, &[b"session", &self.challenge, &self.nonce, &self.fields]);

        Ok((Sealer { key, way: Way::ToNode, sent: 0 }, Opener { key, way: Way::ToWorker, received: 0 }))
    }
}

/// The sending half of a connection.
#[cfg(not(target_arch = "wasm32"))]
pub struct Sender {
    stream: TcpStream,
    sealer: Sealer,
}

#[cfg(not(target_arch = "wasm32"))]
impl Sender {
    /// Sends a message, with its MAC.
    pub fn send(&mut self, message: &Value) -> Result<(), String> {
        let mut line = self.sealer.seal(message);
        line.push('\n');

        // One write a message: the stream is unbuffered, and formatting writes each piece apart.
        self.stream.write_all(line.as_bytes()).map_err(|err| format!("the connection broke: {err}"))
    }
}

/// The receiving half of a connection.
#[cfg(not(target_arch = "wasm32"))]
pub struct Receiver {
    reader: BufReader<TcpStream>,
    opener: Opener,
}

#[cfg(not(target_arch = "wasm32"))]
impl Receiver {
    /// The next message, once its MAC checks; an error for one that does not, or a closed
    /// connection.
    pub fn receive(&mut self) -> Result<Value, String> {
        let line = read_line(&mut self.reader)?;

        self.opener.open(&line)
    }
}

/// A line, without its end; an error for a closed connection or one too long.
#[cfg(not(target_arch = "wasm32"))]
fn read_line(reader: &mut BufReader<TcpStream>) -> Result<String, String> {
    let mut line = String::new();
    let read = std::io::Read::take(&mut *reader, MOST_LINE).read_line(&mut line).map_err(|err| format!("the connection broke: {err}"))?;

    if read == 0 {
        return Err("the connection closed".to_string());
    }

    if !line.ends_with('\n') {
        return Err("a line too long".to_string());
    }

    Ok(line.trim_end().to_string())
}

#[cfg(not(target_arch = "wasm32"))]
fn write_line(stream: &mut TcpStream, message: &Value) -> Result<(), String> {
    writeln!(stream, "{message}").and_then(|()| stream.flush()).map_err(|err| format!("the connection broke: {err}"))
}

#[cfg(not(target_arch = "wasm32"))]
fn read_json(reader: &mut BufReader<TcpStream>) -> Result<Value, String> {
    serde_json::from_str(&read_line(reader)?).map_err(|_| "a handshake that does not read".to_string())
}

/// The node's side of the handshake: the halves of the connection once the worker proved it knows
/// the token, and what it said of itself.
#[cfg(not(target_arch = "wasm32"))]
pub fn accept(stream: TcpStream, token: &[u8], protocol: &str) -> Result<(Sender, Receiver, Hello), String> {
    let mut writer = stream.try_clone().map_err(|err| err.to_string())?;
    let mut reader = BufReader::new(stream.try_clone().map_err(|err| err.to_string())?);
    let (greeting, challenge) = greeting(protocol)?;
    write_line(&mut writer, &greeting)?;

    let hello = read_json(&mut reader)?;
    let (said, answer, sealer, opener) = welcome(token, protocol, &challenge, &hello)?;
    write_line(&mut writer, &answer)?;

    Ok((Sender { stream: writer, sealer }, Receiver { reader, opener }, said))
}

/// A node's handshake, had it taken another payout from the hello: for the worker's tests.
#[cfg(all(test, not(target_arch = "wasm32")))]
pub(super) fn accept_paying_for_tests(stream: TcpStream, token: &[u8], protocol: &str, payout: &crate::structs::address::Address) {
    let mut writer = stream.try_clone().unwrap();
    let mut reader = BufReader::new(stream);
    let (greeting, challenge) = greeting(protocol).unwrap();
    write_line(&mut writer, &greeting).unwrap();
    let hello = read_json(&mut reader).unwrap();
    let nonce: [u8; 32] = unhex(&hello["nonce"]).unwrap();
    let fields = hello_fields(&version(protocol), hello["name"].as_str().unwrap_or_default(), hello["payout"].as_str().unwrap_or_default(), hello["solo"] == true);
    let payout = payout.to_string();
    let proof = mac(token, &[b"node", &challenge, &nonce, &fields, payout.as_bytes()]);
    write_line(&mut writer, &json!({ "proof": hex(&proof), "payout": payout })).unwrap();
}

/// The worker's side of the handshake: the halves of the connection once the node proved it knows
/// the token too.
#[cfg(not(target_arch = "wasm32"))]
pub fn connect(stream: TcpStream, token: &[u8], protocol: &str, name: &str, payout: Option<&crate::structs::address::Address>) -> Result<(Sender, Receiver), String> {
    connect_as(stream, token, protocol, &Hello { name: name.to_string(), payout: payout.copied(), solo: false })
}

/// connect, saying all a hello may: a solo worker's too.
#[cfg(not(target_arch = "wasm32"))]
pub fn connect_as(stream: TcpStream, token: &[u8], protocol: &str, me: &Hello) -> Result<(Sender, Receiver), String> {
    let mut writer = stream.try_clone().map_err(|err| err.to_string())?;
    let mut reader = BufReader::new(stream.try_clone().map_err(|err| err.to_string())?);
    let greeting = read_json(&mut reader)?;
    let (hello, joining) = hello(&greeting, token, protocol, me)?;
    write_line(&mut writer, &hello)?;

    // A node that refuses the token closes the connection.
    let answer = read_json(&mut reader).map_err(|_| "the node refused the token".to_string())?;
    let (sealer, opener) = joining.finish(token, &answer)?;

    Ok((Sender { stream: writer, sealer }, Receiver { reader, opener }))
}

#[cfg(test)]
mod tests;
