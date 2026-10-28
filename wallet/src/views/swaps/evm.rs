//! A swap's legs on EVM chains, such as Ethereum, Sepolia or Base: Swap.sol there, read through
//! the chain's JSON-RPC, a URL Settings gives, and sent to as every page sends there
//! (crate::views::evm): a lock, claim or refund priced, shown, signed and waited for until mined.

use {
    super::SwapsState,
    crate::views::Ctx,
    crate::views::evm::{number, request},
    chainv4::structs::hex::{from_hex, to_hex},
    chainv4::swap::evm::{get_input, item_input, read_item, read_lock},
    chainv4::swap::{EvmLeg, Lock},
    serde_json::{Value, json},
};

/// What Swap.sol or its chain answered, for the next frame to take.
pub enum Read {
    Lock { hash: String, chain_id: u64, lock: Box<Result<Lock, String>> },
    /// The chain's height, and the block its RPC calls finalized, where it names one.
    Heads { chain_id: u64, heads: Result<(u64, Option<u64>), String> },
}

/// Asks Swap.sol where a leg's lock stands, and for a leg locking an NFT, which one it holds, in
/// one batch.
pub fn ask_lock(ctx: &Ctx, state: &SwapsState, url: &str, leg: &EvmLeg, hash: &[u8; 32]) {
    let (chain_id, key) = (leg.chain_id, to_hex(hash));
    let call = |id: u64, input: Vec<u8>| request(id, "eth_call", json!([{ "to": to_hex(&leg.contract), "data": to_hex(&input) }, "latest"]));
    let output = |value: &Value| value.as_str().and_then(from_hex).ok_or_else(|| format!("eth_call answered {value}"));

    if leg.item.is_none() {
        return state.evm.ask(ctx, url, call(1, get_input(hash)), move |answer| Read::Lock { hash: key, chain_id, lock: Box::new(answer.and_then(|value| output(&value)).and_then(|output| read_lock(&output))) });
    }

    let batch = json!([call(1, get_input(hash)), call(2, item_input(hash))]);

    state.evm.ask_raw(ctx, url, batch, move |answer| Read::Lock {
        hash: key,
        chain_id,
        lock: Box::new(answer.and_then(|replies| {
            let reply = |id: u64| -> Result<Vec<u8>, String> {
                let found = replies.as_array().and_then(|replies| replies.iter().find(|reply| reply["id"] == id)).ok_or("a batch without its answer")?;

                match found.get("error").filter(|error| !error.is_null()) {
                    Some(error) => Err(error["message"].as_str().map_or_else(|| error.to_string(), str::to_string)),
                    None => output(&found["result"]),
                }
            };
            let mut lock = read_lock(&reply(1)?)?;
            lock.item = read_item(&reply(2)?)?;

            Ok(lock)
        })),
    });
}

/// Asks a chain for its height and its finalized block, in one batch.
pub fn ask_heads(ctx: &Ctx, state: &SwapsState, url: &str, chain_id: u64) {
    let batch = json!([request(1, "eth_blockNumber", json!([])), request(2, "eth_getBlockByNumber", json!(["finalized", false]))]);

    state.evm.ask_raw(ctx, url, batch, move |answer| Read::Heads { chain_id, heads: answer.and_then(|replies| heads(&replies)) });
}

/// A chain's height, and its finalized block's, from the batch's answers: none where the RPC
/// names no finalized block.
pub fn heads(replies: &Value) -> Result<(u64, Option<u64>), String> {
    let reply = |id: u64| replies.as_array().and_then(|replies| replies.iter().find(|reply| reply["id"] == id)).cloned().unwrap_or(Value::Null);
    let tip = number(&reply(1)["result"]).and_then(|tip| u64::try_from(tip).map_err(|_| "a height past 64 bits".to_string()))?;
    let finalized = reply(2)["result"]["number"].as_str().and_then(|hex| u64::from_str_radix(hex.trim_start_matches("0x"), 16).ok());

    Ok((tip, finalized))
}

/// Takes what EVM chains answered since the last frame.
pub fn take(ctx: &mut Ctx, state: &mut SwapsState) {
    let (reads, mined) = state.evm.take();

    // Asked again at once: the lock has moved.
    if let Some(hash) = mined {
        state.evm_asked.remove(&hash);
    }

    for read in reads {
        match read {
            Read::Lock { hash, chain_id, lock } => {
                // A claim there shows the secret: kept for the other leg's claim, once it is the
                // swap's, whatever the RPC says.
                let lock = *lock;

                if let Ok(Lock { secret: Some(secret), .. }) = &lock
                    && to_hex(&chainv4::crypto::bitcoin::sha256(secret)) == hash
                    && let Some(record) = ctx.book.swaps.iter_mut().find(|record| record.secret.is_none() && super::hash_of(record).as_deref() == Some(hash.as_str()))
                {
                    record.secret = Some(to_hex(secret));
                    *ctx.changed = true;
                }

                state.evm_locks.insert((hash, chain_id), lock);
            }
            Read::Heads { chain_id, heads } => {
                if let Ok(heads) = heads {
                    state.evm_heads.insert(chain_id, heads);
                }
            }
        }
    }
}
