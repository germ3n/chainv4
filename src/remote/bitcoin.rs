//! Bitcoin, read and written through Esplora or a plain bitcoind, behind one trait, so every
//! Bitcoin read and write the bridge, the anchorer and the swap CLI make works from either. bitcoind
//! needs no wallet: coins come from scantxoutset, Merkle proofs from gettxoutproof. Looking up a
//! transaction already in a block by its id alone (getrawtransaction, gettxoutproof without its
//! block) takes -txindex, as bitcoind keeps no index of them otherwise; Esplora always can.

use {
    super::{Endpoints, EndpointFailure, Http, judged},
    r#macro::document,
    serde_json::{Value, json},
    std::{collections::BTreeMap, sync::Arc, time::Duration},
};

#[document(
    summary = "Where a Bitcoin transaction stands: in a block, which, at what height; or not yet, or unknown.",
    category = "Client",
    see_also = "BitcoinApi",
)]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BtcTxStatus {
    #[document(summary = "Whether it is in a block.")]
    pub confirmed: bool,
    #[document(summary = "That block's hash, as explorers write it.")]
    pub block_hash: Option<String>,
    #[document(summary = "Its height.")]
    pub block_height: Option<u64>,
}

#[document(
    summary = "A transaction's place in its block: the block's height and hash, its index, and its Merkle path, each sibling in Bitcoin's own byte order, from its row up.",
    category = "Client",
    see_also = "BitcoinApi::merkle_proof",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BtcMerkleProof {
    #[document(summary = "The block's height.")]
    pub block_height: u64,
    #[document(summary = "Its index in the block.")]
    pub pos: u64,
    #[document(summary = "The siblings, internal byte order.")]
    pub path: Vec<[u8; 32]>,
}

#[document(
    summary = "A coin an address holds: its outpoint and value in satoshis.",
    category = "Client",
    see_also = "BitcoinApi::coins",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BtcCoin {
    #[document(summary = "Its transaction, as explorers write it.")]
    pub txid: String,
    #[document(summary = "Its output.")]
    pub vout: u32,
    #[document(summary = "Its value.")]
    pub value: u64,
}

#[document(
    summary = "Every Bitcoin read and write the tools make, from Esplora or bitcoind alike. Hashes and ids are written as explorers write them, reversed.",
    category = "Client",
    see_also = "Esplora, Bitcoind, Fallback",
)]
pub trait BitcoinApi: Send + Sync {
    #[document(parent = "BitcoinApi", summary = "The main chain's height.")]
    fn tip_height(&self) -> Result<u64, String>;
    #[document(parent = "BitcoinApi", summary = "The main chain's block at a height.")]
    fn block_hash(&self, height: u64) -> Result<String, String>;
    #[document(parent = "BitcoinApi", summary = "A block's header, 80 bytes.")]
    fn header(&self, hash: &str) -> Result<Vec<u8>, String>;
    #[document(parent = "BitcoinApi", summary = "A transaction's bytes, or None for one it does not know.")]
    fn raw_tx(&self, txid: &str) -> Result<Option<Vec<u8>>, String>;
    #[document(parent = "BitcoinApi", summary = "Where a transaction stands.")]
    fn tx_status(&self, txid: &str) -> Result<BtcTxStatus, String>;
    #[document(parent = "BitcoinApi", summary = "A transaction's place and path in its block, the block given when known.")]
    fn merkle_proof(&self, txid: &str, block: Option<&str>) -> Result<BtcMerkleProof, String>;
    #[document(parent = "BitcoinApi", summary = "A block's coinbase's id.")]
    fn coinbase_txid(&self, block: &str) -> Result<String, String>;
    #[document(parent = "BitcoinApi", summary = "The coins an address holds, confirmed or not.")]
    fn coins(&self, address: &str) -> Result<Vec<BtcCoin>, String>;
    #[document(parent = "BitcoinApi", summary = "Broadcasts a transaction, as hex: its id.")]
    fn broadcast(&self, hex: &str) -> Result<String, String>;
    #[document(parent = "BitcoinApi", summary = "Fee rates by how many blocks they are for, in satoshis a virtual byte: none on a chain with no estimates, as regtest.")]
    fn fee_estimates(&self) -> Result<BTreeMap<u32, f64>, String>;
}

/// A hash written as explorers write it, in Bitcoin's own byte order.
fn internal(display: &str) -> Result<[u8; 32], String> {
    let mut bytes: [u8; 32] = crate::structs::hex::from_hex(&format!("0x{}", display.trim().trim_start_matches("0x"))).and_then(|bytes| bytes.try_into().ok()).ok_or_else(|| format!("{display} is not a 32-byte hash"))?;
    bytes.reverse();

    Ok(bytes)
}

/// A hash in Bitcoin's own byte order, written as explorers write it.
fn shown(internal: &[u8; 32]) -> String {
    let mut bytes = *internal;
    bytes.reverse();

    crate::structs::hex::to_hex(&bytes).trim_start_matches("0x").to_string()
}

fn hex_bytes(text: &str, what: &str) -> Result<Vec<u8>, String> {
    crate::structs::hex::from_hex(&format!("0x{}", text.trim().trim_start_matches("0x"))).ok_or_else(|| format!("{what} is not hex"))
}

#[document(
    summary = "Esplora's /fee-estimates, a rate by how many blocks it is for, with what is no rate left out.",
    category = "Client",
)]
pub fn esplora_fee_map(answer: &str) -> Result<BTreeMap<u32, f64>, String> {
    let estimates: BTreeMap<String, f64> = serde_json::from_str(answer).map_err(|_| "Esplora gave fee estimates that do not read".to_string())?;

    Ok(estimates.into_iter().filter_map(|(blocks, rate)| Some((blocks.parse().ok()?, rate))).filter(|(_, rate): &(u32, f64)| rate.is_finite() && *rate > 0.0).collect())
}

#[document(
    summary = "An Esplora API, such as https://mempool.space/api, over its URLs: asked at an even pace, and again, waiting twice as long each time, while every URL fails or answers a web page, as a throttled one does.",
    category = "Client",
    see_also = "BitcoinApi",
)]
pub struct Esplora {
    http: Arc<dyn Http>,
    endpoints: Endpoints,
    pace: Duration,
    tries: u32,
    wait: fn(Duration),
}

/// How long between two questions to Esplora, so a sync does not burst.
const ESPLORA_PACE: Duration = Duration::from_millis(150);
/// How many times Esplora is asked before a question fails: waiting 2, 4, 8, 16 and 32 seconds
/// between, about a minute in all.
const ESPLORA_TRIES: u32 = 6;

impl Esplora {
    #[document(parent = "Esplora", summary = "Esplora at its URLs.")]
    pub fn new(http: Arc<dyn Http>, endpoints: Endpoints) -> Esplora {
        Esplora { http, endpoints, pace: ESPLORA_PACE, tries: ESPLORA_TRIES, wait: std::thread::sleep }
    }

    #[document(parent = "Esplora", summary = "One that neither paces nor waits, asking once: for tests.")]
    pub fn unpaced(http: Arc<dyn Http>, endpoints: Endpoints) -> Esplora {
        Esplora { http, endpoints, pace: Duration::ZERO, tries: 1, wait: |_| {} }
    }

    /// One question, of every URL in turn: its answer, None for a 404, which Esplora gives for what
    /// it does not know.
    fn once(&self, path: &str) -> Result<Option<String>, String> {
        self.endpoints.ask(&format!("Esplora {path}"), |endpoint| {
            let reply = self.http.send(&endpoint.request(false, path, &[], Vec::new())?).map_err(EndpointFailure::Endpoint)?;
            let reply = judged(reply)?;
            let text = reply.text();

            match reply.status {
                404 => Ok(None),
                _ if text.trim_start().starts_with('<') => Err(EndpointFailure::Endpoint("it answered a web page, as it does when it throttles".to_string())),
                200..=299 => Ok(Some(text)),
                status => Err(EndpointFailure::Answer(format!("it answered {status}: {}", text.trim()))),
            }
        })
    }

    fn get_maybe(&self, path: &str, tries: u32) -> Result<Option<String>, String> {
        let mut last = String::new();

        for attempt in 0..tries.max(1) {
            if attempt > 0 {
                (self.wait)(Duration::from_secs(1 << attempt));
            }

            if !self.pace.is_zero() {
                std::thread::sleep(self.pace);
            }

            match self.once(path) {
                Ok(answer) => return Ok(answer),
                Err(why) => last = why,
            }
        }

        Err(format!("{last}; asked {} times", tries.max(1)))
    }

    fn get(&self, path: &str) -> Result<String, String> {
        self.get_maybe(path, self.tries)?.ok_or_else(|| format!("Esplora {path}: not found"))
    }

    fn json(&self, path: &str) -> Result<Value, String> {
        serde_json::from_str(&self.get(path)?).map_err(|_| format!("Esplora {path}: not JSON"))
    }
}

impl BitcoinApi for Esplora {
    fn tip_height(&self) -> Result<u64, String> {
        self.get("/blocks/tip/height")?.trim().parse().map_err(|_| "Esplora gave a height that is not a number".to_string())
    }

    fn block_hash(&self, height: u64) -> Result<String, String> {
        Ok(self.get(&format!("/block-height/{height}"))?.trim().to_string())
    }

    fn header(&self, hash: &str) -> Result<Vec<u8>, String> {
        hex_bytes(&self.get(&format!("/block/{}/header", hash.trim()))?, "Esplora's header")
    }

    fn raw_tx(&self, txid: &str) -> Result<Option<Vec<u8>>, String> {
        self.get_maybe(&format!("/tx/{txid}/hex"), self.tries)?.map(|hex| hex_bytes(&hex, "Esplora's transaction")).transpose()
    }

    fn tx_status(&self, txid: &str) -> Result<BtcTxStatus, String> {
        let Some(answer) = self.get_maybe(&format!("/tx/{txid}/status"), self.tries)? else {
            return Ok(BtcTxStatus::default());
        };
        let status: Value = serde_json::from_str(&answer).unwrap_or(Value::Null);

        Ok(BtcTxStatus { confirmed: status["confirmed"].as_bool() == Some(true), block_hash: status["block_hash"].as_str().map(str::to_string), block_height: status["block_height"].as_u64() })
    }

    fn merkle_proof(&self, txid: &str, _block: Option<&str>) -> Result<BtcMerkleProof, String> {
        let proof = self.json(&format!("/tx/{txid}/merkle-proof"))?;
        let path = proof["merkle"].as_array().into_iter().flatten().map(|sibling| internal(sibling.as_str().unwrap_or_default())).collect::<Result<Vec<_>, _>>()?;

        Ok(BtcMerkleProof { block_height: proof["block_height"].as_u64().ok_or("the transaction is not in a block yet")?, pos: proof["pos"].as_u64().unwrap_or(0), path })
    }

    fn coinbase_txid(&self, block: &str) -> Result<String, String> {
        Ok(self.get(&format!("/block/{}/txid/0", block.trim()))?.trim().to_string())
    }

    fn coins(&self, address: &str) -> Result<Vec<BtcCoin>, String> {
        let coins = self.json(&format!("/address/{address}/utxo"))?;

        Ok(coins
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|coin| Some(BtcCoin { txid: coin["txid"].as_str()?.to_string(), vout: u32::try_from(coin["vout"].as_u64()?).ok()?, value: coin["value"].as_u64()? }))
            .collect())
    }

    fn broadcast(&self, hex: &str) -> Result<String, String> {
        self.endpoints.ask("Esplora /tx", |endpoint| {
            let reply = judged(self.http.send(&endpoint.request(true, "/tx", &[], hex.trim_start_matches("0x").as_bytes().to_vec())?).map_err(EndpointFailure::Endpoint)?)?;

            match reply.status {
                200..=299 => Ok(reply.text().trim().to_string()),
                _ => Err(EndpointFailure::Answer(format!("it would not take it: {}", reply.text().trim()))),
            }
        })
    }

    fn fee_estimates(&self) -> Result<BTreeMap<u32, f64>, String> {
        // Asked twice at most: fees going unread must not hold up a pass for long.
        match self.get_maybe("/fee-estimates", 2)? {
            Some(answer) => esplora_fee_map(&answer),
            None => Ok(BTreeMap::new()),
        }
    }
}

#[document(
    summary = "A plain bitcoind's JSON-RPC, over its URLs, each with its login (Auth: its cookie file, or rpcuser and rpcpassword from a file or the environment). No wallet: coins come from scantxoutset. Transactions already in a block are found by id with -txindex.",
    category = "Client",
    see_also = "BitcoinApi",
)]
pub struct Bitcoind {
    http: Arc<dyn Http>,
    endpoints: Endpoints,
}

/// bitcoind's code for a transaction or block it does not know.
const NOT_FOUND: i64 = -5;

impl Bitcoind {
    #[document(parent = "Bitcoind", summary = "bitcoind at its URLs.")]
    pub fn new(http: Arc<dyn Http>, endpoints: Endpoints) -> Bitcoind {
        Bitcoind { http, endpoints }
    }

    /// A call's result, None where bitcoind does not know what was asked for.
    fn maybe(&self, method: &str, params: Value) -> Result<Option<Value>, String> {
        let body = json!({ "jsonrpc": "1.0", "id": "chainv4", "method": method, "params": params }).to_string().into_bytes();

        self.endpoints.ask(&format!("bitcoind {method}"), |endpoint| {
            let reply = self.http.send(&endpoint.request(true, "", &[("content-type", "application/json")], body.clone())?).map_err(EndpointFailure::Endpoint)?;

            // bitcoind answers an RPC error with a 500 or 404 whose body says what: that is its
            // answer, not it failing.
            let answer: Option<Value> = serde_json::from_slice(&reply.body).ok().filter(|answer: &Value| answer.get("result").is_some() || answer.get("error").is_some());
            let Some(answer) = answer else {
                return Err(judged(reply).err().unwrap_or_else(|| EndpointFailure::Endpoint("it did not answer JSON-RPC".to_string())));
            };

            match &answer["error"] {
                Value::Null => Ok(Some(answer["result"].clone())),
                error if error["code"].as_i64() == Some(NOT_FOUND) => Ok(None),
                error => Err(EndpointFailure::Answer(error["message"].as_str().map_or_else(|| error.to_string(), str::to_string))),
            }
        })
    }

    fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        self.maybe(method, params)?.ok_or_else(|| format!("bitcoind {method}: not found; a transaction already in a block is found by its id alone with -txindex"))
    }

    fn text(&self, method: &str, params: Value) -> Result<String, String> {
        let answer = self.call(method, params)?;

        answer.as_str().map(str::to_string).ok_or_else(|| format!("bitcoind {method} answered {answer}"))
    }

    fn height_of(&self, block: &str) -> Result<u64, String> {
        self.call("getblockheader", json!([block, true]))?["height"].as_u64().ok_or_else(|| "bitcoind gave a header with no height".to_string())
    }
}

impl BitcoinApi for Bitcoind {
    fn tip_height(&self) -> Result<u64, String> {
        self.call("getblockcount", json!([]))?.as_u64().ok_or_else(|| "bitcoind gave a height that is not a number".to_string())
    }

    fn block_hash(&self, height: u64) -> Result<String, String> {
        self.text("getblockhash", json!([height]))
    }

    fn header(&self, hash: &str) -> Result<Vec<u8>, String> {
        hex_bytes(&self.text("getblockheader", json!([hash.trim(), false]))?, "bitcoind's header")
    }

    fn raw_tx(&self, txid: &str) -> Result<Option<Vec<u8>>, String> {
        match self.maybe("getrawtransaction", json!([txid, false]))? {
            Some(hex) => hex_bytes(hex.as_str().unwrap_or_default(), "bitcoind's transaction").map(Some),
            None => Ok(None),
        }
    }

    fn tx_status(&self, txid: &str) -> Result<BtcTxStatus, String> {
        let Some(tx) = self.maybe("getrawtransaction", json!([txid, true]))? else {
            return Ok(BtcTxStatus::default());
        };

        match tx["blockhash"].as_str() {
            Some(block) if tx["confirmations"].as_u64().unwrap_or(0) > 0 => Ok(BtcTxStatus { confirmed: true, block_hash: Some(block.to_string()), block_height: Some(self.height_of(block)?) }),
            _ => Ok(BtcTxStatus::default()),
        }
    }

    fn merkle_proof(&self, txid: &str, block: Option<&str>) -> Result<BtcMerkleProof, String> {
        let params = match block {
            Some(block) => json!([[txid], block]),
            None => json!([[txid]]),
        };
        let proof = hex_bytes(&self.text("gettxoutproof", params)?, "bitcoind's proof")?;
        let (block, pos, path) = merkle_block_path(&proof, &internal(txid)?)?;

        Ok(BtcMerkleProof { block_height: self.height_of(&shown(&block))?, pos, path })
    }

    fn coinbase_txid(&self, block: &str) -> Result<String, String> {
        let answer = self.call("getblock", json!([block.trim(), 1]))?;

        answer["tx"][0].as_str().map(str::to_string).ok_or_else(|| "bitcoind gave a block with no transactions".to_string())
    }

    fn coins(&self, address: &str) -> Result<Vec<BtcCoin>, String> {
        scanned_coins(&self.call("scantxoutset", json!(["start", [format!("addr({address})")]]))?)
    }

    fn broadcast(&self, hex: &str) -> Result<String, String> {
        self.text("sendrawtransaction", json!([hex.trim_start_matches("0x")]))
    }

    fn fee_estimates(&self) -> Result<BTreeMap<u32, f64>, String> {
        let mut rates = BTreeMap::new();

        for blocks in [1u32, 2, 3, 6, 12, 144] {
            // In bitcoin a kilo-virtual-byte: satoshis a virtual byte is 100,000 times it. Where it
            // has too little to go by, as regtest, it names no rate.
            if let Some(rate) = self.call("estimatesmartfee", json!([blocks]))?["feerate"].as_f64().map(|rate| rate * 100_000.0).filter(|rate| rate.is_finite() && *rate > 0.0) {
                rates.insert(blocks, rate);
            }
        }

        Ok(rates)
    }
}

#[document(
    summary = "scantxoutset's coins, their values in bitcoin turned to satoshis.",
    category = "Client",
)]
pub fn scanned_coins(answer: &Value) -> Result<Vec<BtcCoin>, String> {
    if answer["success"].as_bool() == Some(false) {
        return Err("bitcoind's scantxoutset did not finish".to_string());
    }

    answer["unspents"]
        .as_array()
        .ok_or("bitcoind's scantxoutset gave no unspents")?
        .iter()
        .map(|coin| {
            let amount = coin["amount"].as_f64().ok_or("a coin with no amount")?;

            Ok(BtcCoin {
                txid: coin["txid"].as_str().ok_or("a coin with no txid")?.to_string(),
                vout: coin["vout"].as_u64().and_then(|vout| u32::try_from(vout).ok()).ok_or("a coin with no vout")?,
                value: (amount * 100_000_000.0).round() as u64,
            })
        })
        .collect()
}

/// A compact size: Bitcoin's variable-length count.
fn compact_size(bytes: &[u8], at: &mut usize) -> Option<u64> {
    let first = *bytes.get(*at)?;
    *at += 1;
    let size = match first {
        0xfd => 2,
        0xfe => 4,
        0xff => 8,
        small => return Some(u64::from(small)),
    };
    let piece = bytes.get(*at..*at + size)?;
    *at += size;

    Some(piece.iter().rev().fold(0u64, |total, byte| total << 8 | u64::from(*byte)))
}

#[document(
    summary = "Where gettxoutproof places a transaction: its block's hash, its index, and its Merkle path.",
    category = "Client",
    see_also = "merkle_block_path",
)]
pub type PlacedTx = ([u8; 32], u64, Vec<[u8; 32]>);

#[document(
    summary = "gettxoutproof's merkleblock read for one transaction: the block's hash, the transaction's index, and its Merkle path, each sibling in Bitcoin's own byte order, the last node standing in for a missing right sibling as Bitcoin's tree has it. Checked to come to the header's Merkle root.",
    params = { proof = "The merkleblock: the header, the transaction count, the hashes and the flag bits.", txid = "The transaction, internal byte order." },
    returns = "The block, index and path, or why the proof does not read or holds no such transaction.",
    category = "Client",
)]
pub fn merkle_block_path(proof: &[u8], txid: &[u8; 32]) -> Result<PlacedTx, String> {
    use crate::crypto::bitcoin::sha256d;

    let bad = || "bitcoind's proof does not read".to_string();
    let header = proof.get(..80).ok_or_else(bad)?;
    let total = u64::from(u32::from_le_bytes(proof.get(80..84).ok_or_else(bad)?.try_into().expect("4 bytes")));
    let mut at = 84;
    let count = compact_size(proof, &mut at).ok_or_else(bad)? as usize;
    let hashes: Vec<[u8; 32]> = (0..count).map(|index| proof.get(at + 32 * index..at + 32 * index + 32).and_then(|hash| hash.try_into().ok())).collect::<Option<_>>().ok_or_else(bad)?;
    at += 32 * count;
    let flag_bytes = compact_size(proof, &mut at).ok_or_else(bad)? as usize;
    let flags = proof.get(at..at + flag_bytes).ok_or_else(bad)?;

    if total == 0 {
        return Err(bad());
    }

    let width = |height: u32| (total + (1 << height) - 1) >> height;
    let mut height = 0;

    while width(height) > 1 {
        height += 1;
    }

    struct Walk<'a> {
        hashes: &'a [[u8; 32]],
        flags: &'a [u8],
        used_hashes: usize,
        used_bits: usize,
        nodes: BTreeMap<(u32, u64), [u8; 32]>,
        matched: Vec<u64>,
    }

    fn walk(state: &mut Walk<'_>, height: u32, pos: u64, width: &dyn Fn(u32) -> u64) -> Option<[u8; 32]> {
        let flag = state.flags.get(state.used_bits / 8)? >> (state.used_bits % 8) & 1 == 1;
        state.used_bits += 1;

        let hash = if height == 0 || !flag {
            let hash = *state.hashes.get(state.used_hashes)?;
            state.used_hashes += 1;

            if height == 0 && flag {
                state.matched.push(pos);
            }

            hash
        } else {
            let left = walk(state, height - 1, pos * 2, width)?;
            let right = match pos * 2 + 1 < width(height - 1) {
                true => walk(state, height - 1, pos * 2 + 1, width)?,
                false => left,
            };

            sha256d(&[left, right].concat())
        };
        state.nodes.insert((height, pos), hash);

        Some(hash)
    }

    let mut state = Walk { hashes: &hashes, flags, used_hashes: 0, used_bits: 0, nodes: BTreeMap::new(), matched: Vec::new() };
    let root = walk(&mut state, height, 0, &width).ok_or_else(bad)?;

    if root[..] != header[36..68] || state.used_hashes != hashes.len() {
        return Err("bitcoind's proof does not come to its block's Merkle root".to_string());
    }

    let pos = state.matched.iter().copied().find(|pos| state.nodes.get(&(0, *pos)) == Some(txid)).ok_or("bitcoind's proof is not of that transaction")?;
    let path = (0..height)
        .map(|level| {
            let node = pos >> level;
            let sibling = match (node ^ 1) < width(level) {
                true => node ^ 1,
                false => node,
            };

            state.nodes.get(&(level, sibling)).copied().ok_or_else(bad)
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok((sha256d(header), pos, path))
}

#[document(
    summary = "Two Bitcoin sources standing in for each other: Esplora's URLs, then bitcoind's, the second asked when every URL of the first fails.",
    category = "Client",
    see_also = "BitcoinApi",
)]
pub struct Fallback {
    sources: Vec<Box<dyn BitcoinApi>>,
}

impl Fallback {
    #[document(parent = "Fallback", summary = "Sources in the order they are asked; None for none.")]
    pub fn new(sources: Vec<Box<dyn BitcoinApi>>) -> Option<Fallback> {
        (!sources.is_empty()).then_some(Fallback { sources })
    }

    fn each<T>(&self, mut ask: impl FnMut(&dyn BitcoinApi) -> Result<T, String>) -> Result<T, String> {
        let mut failed = Vec::new();

        for source in &self.sources {
            match ask(source.as_ref()) {
                Ok(answer) => return Ok(answer),
                Err(why) => failed.push(why),
            }
        }

        Err(failed.join("; "))
    }
}

impl BitcoinApi for Fallback {
    fn tip_height(&self) -> Result<u64, String> {
        self.each(|source| source.tip_height())
    }

    fn block_hash(&self, height: u64) -> Result<String, String> {
        self.each(|source| source.block_hash(height))
    }

    fn header(&self, hash: &str) -> Result<Vec<u8>, String> {
        self.each(|source| source.header(hash))
    }

    fn raw_tx(&self, txid: &str) -> Result<Option<Vec<u8>>, String> {
        self.each(|source| source.raw_tx(txid))
    }

    fn tx_status(&self, txid: &str) -> Result<BtcTxStatus, String> {
        self.each(|source| source.tx_status(txid))
    }

    fn merkle_proof(&self, txid: &str, block: Option<&str>) -> Result<BtcMerkleProof, String> {
        self.each(|source| source.merkle_proof(txid, block))
    }

    fn coinbase_txid(&self, block: &str) -> Result<String, String> {
        self.each(|source| source.coinbase_txid(block))
    }

    fn coins(&self, address: &str) -> Result<Vec<BtcCoin>, String> {
        self.each(|source| source.coins(address))
    }

    fn broadcast(&self, hex: &str) -> Result<String, String> {
        self.each(|source| source.broadcast(hex))
    }

    fn fee_estimates(&self) -> Result<BTreeMap<u32, f64>, String> {
        self.each(|source| source.fee_estimates())
    }
}

#[document(
    summary = "The Bitcoin source the flags name: --esplora's URLs, then --bitcoind's with --bitcoind-auth's logins by order, through the HTTP client given. None when neither is given.",
    category = "Client",
    see_also = "Fallback",
)]
pub fn from_flags(http: Arc<dyn Http>, esplora: &[String], bitcoind: &[String], bitcoind_auth: &[String]) -> Result<Option<Fallback>, String> {
    let mut sources: Vec<Box<dyn BitcoinApi>> = Vec::new();

    if !esplora.is_empty() {
        sources.push(Box::new(Esplora::new(http.clone(), Endpoints::from_flags(esplora, &[], "esplora", false)?)));
    }

    if !bitcoind.is_empty() {
        sources.push(Box::new(Bitcoind::new(http, Endpoints::from_flags(bitcoind, bitcoind_auth, "bitcoind", true)?)));
    } else if !bitcoind_auth.is_empty() {
        return Err("--bitcoind-auth is given without --bitcoind".to_string());
    }

    Ok(Fallback::new(sources))
}

/// What a reply's body is, for tests' fakes.
#[cfg(test)]
pub(super) fn reply(status: u16, body: &str) -> super::Reply {
    super::Reply { status, body: body.as_bytes().to_vec() }
}
