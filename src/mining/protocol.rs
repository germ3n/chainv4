//! The mining port's protocol: lines of JSON over TCP, between a node and the workers mining for
//! it, which share a token.
//!
//! **The handshake** proves each side knows the token without sending it. The node sends a random
//! challenge; the worker answers with its name, a random nonce of its own, and an HMAC of both
//! under the token; the node answers with its own HMAC of both. The session key is a third HMAC of
//! both, so it differs every connection.
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
    hmac::{Hmac, KeyInit, Mac},
    serde_json::{Value, json},
    sha2::Sha256,
    std::io::{BufRead, BufReader, Write},
    std::net::TcpStream,
};

/// What both sides say first, so a stray connection, or a worker of a chain under another
/// protocol name, is told apart: chainv4-mining/1 for the default name.
pub fn version(protocol: &str) -> String {
    format!("{protocol}-mining/1")
}

/// The longest line either side reads: a work or rate message is far shorter.
const MOST_LINE: u64 = 64 * 1024;

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

/// The sending half of a connection.
pub struct Sender {
    stream: TcpStream,
    key: [u8; 32],
    way: Way,
    sent: u64,
}

impl Sender {
    /// Sends a message, with its MAC.
    pub fn send(&mut self, message: &Value) -> Result<(), String> {
        let text = message.to_string();
        let mac = mac(&self.key, &[self.way.tag(), &self.sent.to_le_bytes(), text.as_bytes()]);
        self.sent += 1;

        writeln!(self.stream, "{} {text}", hex(&mac)).and_then(|()| self.stream.flush()).map_err(|err| format!("the connection broke: {err}"))
    }
}

/// The receiving half of a connection.
pub struct Receiver {
    reader: BufReader<TcpStream>,
    key: [u8; 32],
    way: Way,
    received: u64,
}

impl Receiver {
    /// The next message, once its MAC checks; an error for one that does not, or a closed
    /// connection.
    pub fn receive(&mut self) -> Result<Value, String> {
        let line = read_line(&mut self.reader)?;
        let (mac, text) = line.split_once(' ').ok_or("a message without its MAC")?;
        let mac = crate::structs::hex::from_hex(mac).ok_or("a MAC that does not read")?;

        if !checks(&self.key, &[self.way.tag(), &self.received.to_le_bytes(), text.as_bytes()], &mac) {
            return Err("a message whose MAC does not check".to_string());
        }

        self.received += 1;

        serde_json::from_str(text).map_err(|_| "a message that does not read".to_string())
    }
}

/// A line, without its end; an error for a closed connection or one too long.
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

fn write_line(stream: &mut TcpStream, message: &Value) -> Result<(), String> {
    writeln!(stream, "{message}").and_then(|()| stream.flush()).map_err(|err| format!("the connection broke: {err}"))
}

fn read_json(reader: &mut BufReader<TcpStream>) -> Result<Value, String> {
    serde_json::from_str(&read_line(reader)?).map_err(|_| "a handshake that does not read".to_string())
}

/// What a worker said of itself in the handshake.
pub struct Hello {
    pub name: String,
    /// Where a pool pays its shares, if it gave one that reads.
    pub payout: Option<crate::structs::address::Address>,
}

/// The node's side of the handshake: the halves of the connection once the worker proved it knows
/// the token, and its name.
pub fn accept(stream: TcpStream, token: &[u8], protocol: &str) -> Result<(Sender, Receiver, Hello), String> {
    let version = version(protocol);
    let mut writer = stream.try_clone().map_err(|err| err.to_string())?;
    let mut reader = BufReader::new(stream.try_clone().map_err(|err| err.to_string())?);
    let challenge = random()?;
    write_line(&mut writer, &json!({ "version": version, "challenge": hex(&challenge) }))?;

    let hello = read_json(&mut reader)?;
    let nonce: [u8; 32] = unhex(&hello["nonce"])?;
    let proof: [u8; 32] = unhex(&hello["proof"])?;

    if hello["version"] != version.as_str() {
        return Err(format!("a worker speaking {}, not {version}", hello["version"]));
    }

    if !checks(token, &[b"worker", &challenge, &nonce], &proof) {
        return Err("a worker without the token".to_string());
    }

    write_line(&mut writer, &json!({ "proof": hex(&mac(token, &[b"node", &challenge, &nonce])) }))?;
    let key = mac(token, &[b"session", &challenge, &nonce]);
    let name: String = hello["name"].as_str().unwrap_or("worker").chars().filter(|c| !c.is_control()).take(64).collect();
    let payout = hello["payout"].as_str().and_then(|payout| payout.parse().ok());

    Ok((
        Sender { stream: writer, key, way: Way::ToWorker, sent: 0 },
        Receiver { reader, key, way: Way::ToNode, received: 0 },
        Hello { name, payout },
    ))
}

/// The worker's side of the handshake: the halves of the connection once the node proved it knows
/// the token too.
pub fn connect(stream: TcpStream, token: &[u8], protocol: &str, name: &str, payout: Option<&crate::structs::address::Address>) -> Result<(Sender, Receiver), String> {
    let version = version(protocol);
    let mut writer = stream.try_clone().map_err(|err| err.to_string())?;
    let mut reader = BufReader::new(stream.try_clone().map_err(|err| err.to_string())?);
    let greeting = read_json(&mut reader)?;

    if greeting["version"] != version.as_str() {
        return Err(format!("a node speaking {}, not {version}", greeting["version"]));
    }

    let challenge: [u8; 32] = unhex(&greeting["challenge"])?;
    let nonce = random()?;
    let proof = mac(token, &[b"worker", &challenge, &nonce]);
    let mut hello = json!({ "version": version, "name": name, "nonce": hex(&nonce), "proof": hex(&proof) });

    if let Some(payout) = payout {
        hello["payout"] = json!(payout.to_string());
    }

    write_line(&mut writer, &hello)?;

    // A node that refuses the token closes the connection.
    let answer = read_json(&mut reader).map_err(|_| "the node refused the token".to_string())?;
    let theirs: [u8; 32] = unhex(&answer["proof"])?;

    if !checks(token, &[b"node", &challenge, &nonce], &theirs) {
        return Err("the node does not know the token: not the node meant".to_string());
    }

    let key = mac(token, &[b"session", &challenge, &nonce]);

    Ok((
        Sender { stream: writer, key, way: Way::ToNode, sent: 0 },
        Receiver { reader, key, way: Way::ToWorker, received: 0 },
    ))
}

#[cfg(test)]
mod tests;
