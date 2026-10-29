//! Mining for a node from elsewhere: a node takes workers on a mining port of its own, hands each
//! the block to seal and a range of nonces of its own, and seals the block with the first nonce
//! one finds, which pays the node's reward address. A worker runs no node: `chainv4 worker`, on
//! its GPUs or CPU threads, or the wallet's Mining page. They share a token, which neither ever
//! sends. A worker mining alone, on a node that takes solo workers, is handed a block of its own,
//! whose reward goes to its payout in full.
//!
//! Without the node feature, the worker feature builds the worker's side alone: the protocol, its
//! devices and the claims it keeps, for an app that mines but keeps no chain. The protocol's
//! handshake and MACs carry over any transport, so a browser speaks it over a WebSocket.

pub mod claims;
#[cfg(feature = "node")]
pub mod pool;
pub mod protocol;
#[cfg(feature = "pool")]
pub mod shares;
pub mod status;
#[cfg(not(target_arch = "wasm32"))]
pub mod worker;

use {r#macro::document, std::path::Path};

/// The shortest token taken: 32 characters, 128 bits of hex.
const SHORTEST_TOKEN: usize = 32;

#[document(
    summary = "Reads a mining token from a file: its text, without the whitespace around it, which must be at least 32 characters, as openssl rand -hex 32 makes.",
    category = "Mining",
    see_also = "Pool, Worker",
)]
pub fn read_token(path: &Path) -> Result<Vec<u8>, String> {
    let text = std::fs::read_to_string(path).map_err(|err| format!("the mining token {} does not read: {err}", path.display()))?;
    let token = text.trim();

    if token.len() < SHORTEST_TOKEN {
        return Err(format!("the mining token {} is {} characters; it needs {SHORTEST_TOKEN} at least, as openssl rand -hex 32 makes", path.display(), token.len()));
    }

    Ok(token.as_bytes().to_vec())
}
