//! An Ethereum beacon API's light client routes, as SSZ (Accept: application/octet-stream): the
//! bootstrap at a root, updates by period, the newest finality update, and a slot's block root.
//! Nothing it serves is taken on its word: the relay contract and the node check every signature
//! and branch by ethlight's rules. ChainSafe's Lodestar serves these routes publicly
//! (https://lodestar-sepolia.chainsafe.io for Sepolia); not every beacon API does.

use {
    super::{Endpoints, EndpointFailure, Http, judged},
    r#macro::document,
    serde_json::Value,
    std::sync::Arc,
};

#[document(
    summary = "The most updates one request asks for: the beacon API's MAX_REQUEST_LIGHT_CLIENT_UPDATES.",
    category = "Client",
    see_also = "Beacon::updates",
)]
pub const MAX_UPDATES: u64 = 128;

#[document(
    summary = "A beacon API over its URLs, each with its login.",
    category = "Client",
    see_also = "Endpoints",
)]
pub struct Beacon {
    http: Arc<dyn Http>,
    endpoints: Endpoints,
}

impl Beacon {
    #[document(parent = "Beacon", summary = "A beacon API at its URLs.")]
    pub fn new(http: Arc<dyn Http>, endpoints: Endpoints) -> Beacon {
        Beacon { http, endpoints }
    }

    #[document(parent = "Beacon", summary = "Whether it has any URL.")]
    pub fn is_empty(&self) -> bool {
        self.endpoints.is_empty()
    }

    /// A route's body, or None for a 404.
    fn get(&self, path: &str, accept: &str) -> Result<Option<Vec<u8>>, String> {
        let what = format!("the beacon API's {}", path.split('?').next().unwrap_or(path));

        self.endpoints.ask(&what, |endpoint| {
            let reply = judged(self.http.send(&endpoint.request(false, path, &[("accept", accept)], Vec::new())?).map_err(EndpointFailure::Endpoint)?)?;

            match reply.status {
                200..=299 => Ok(Some(reply.body)),
                404 => Ok(None),
                status => Err(EndpointFailure::Answer(format!("it answered {status}: {}", reply.text().trim()))),
            }
        })
    }

    fn ssz(&self, path: &str) -> Result<Vec<u8>, String> {
        self.get(path, "application/octet-stream")?.ok_or_else(|| format!("the beacon API has nothing at {}", path.split('?').next().unwrap_or(path)))
    }

    #[document(parent = "Beacon", summary = "The light client bootstrap at a block root, as SSZ: served for finalized checkpoints' roots.")]
    pub fn bootstrap(&self, root: &[u8; 32]) -> Result<Vec<u8>, String> {
        self.ssz(&format!("/eth/v1/beacon/light_client/bootstrap/{}", crate::structs::hex::to_hex(root)))
    }

    #[document(parent = "Beacon", summary = "The best update of each period from one on, at most MAX_UPDATES, each as SSZ, oldest first.")]
    pub fn updates(&self, start_period: u64, count: u64) -> Result<Vec<Vec<u8>>, String> {
        let body = self.ssz(&format!("/eth/v1/beacon/light_client/updates?start_period={start_period}&count={}", count.clamp(1, MAX_UPDATES)))?;

        response_chunks(&body)
    }

    #[document(parent = "Beacon", summary = "The newest finality update, as SSZ.")]
    pub fn finality_update(&self) -> Result<Vec<u8>, String> {
        self.ssz("/eth/v1/beacon/light_client/finality_update")
    }

    #[document(parent = "Beacon", summary = "The root of the block at a slot, None for a slot without one.")]
    pub fn block_root(&self, slot: u64) -> Result<Option<[u8; 32]>, String> {
        let Some(body) = self.get(&format!("/eth/v1/beacon/blocks/{slot}/root"), "application/json")? else {
            return Ok(None);
        };
        let answer: Value = serde_json::from_slice(&body).map_err(|_| "the beacon API gave a root that is not JSON".to_string())?;

        answer["data"]["root"].as_str().and_then(crate::structs::hex::from_hex).and_then(|root| root.try_into().ok()).map(Some).ok_or_else(|| "the beacon API gave no root".to_string())
    }
}

/// Chunks as the length counts them: with its 4 context bytes, or without.
fn chunks_counted(body: &[u8], with_context: bool) -> Option<Vec<Vec<u8>>> {
    let mut chunks = Vec::new();
    let mut at = 0;

    while at < body.len() {
        let length = usize::try_from(u64::from_le_bytes(body.get(at..at + 8)?.try_into().ok()?)).ok()?;
        let payload = if with_context { length.checked_sub(4)? } else { length };
        let start = at + 8 + 4;
        chunks.push(body.get(start..start.checked_add(payload)?)?.to_vec());
        at = start + payload;
    }

    Some(chunks)
}

#[document(
    summary = "The beacon API's SSZ answer of several objects: each a response chunk, an 8-byte little-endian length, the 4-byte fork digest, then the object. The length counts the digest and the object, as the API's spec has it; one counting the object alone is read too.",
    returns = "Each object's SSZ, or why the body is no such sequence.",
    category = "Client",
)]
pub fn response_chunks(body: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    chunks_counted(body, true).or_else(|| chunks_counted(body, false)).ok_or_else(|| "the beacon API's updates do not read as response chunks".to_string())
}

#[cfg(feature = "anchoring-eth")]
impl crate::anchor::ethereum::EthBeacon for Beacon {
    fn bootstrap(&self, root: &[u8; 32]) -> Result<Vec<u8>, String> {
        Beacon::bootstrap(self, root)
    }

    fn updates(&self, start_period: u64, count: u64) -> Result<Vec<Vec<u8>>, String> {
        Beacon::updates(self, start_period, count)
    }
}
