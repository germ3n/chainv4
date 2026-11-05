//! Anchors in Ethereum. An Ethereum anchor is a transaction whose calldata is the anchor's 80 bytes,
//! in a block Ethereum finalized. Its proof is checked against the Ethereum relay contract's
//! records, read natively as Relay reads btc_relay's, by ethlight's rules, the code the relay runs:
//! the relay holds the finalized header the proof rests on, the finality update that came with it
//! finalizes that header and is signed by two thirds of the sync committee the relay holds for its
//! period, the anchor's block is that header or an ancestor of it by its state's block_roots, and
//! the transaction is in the block's execution payload.
//!
//! Anchored sync checks the same proof without the relay: it follows sync committees from
//! anchoring's eth_start through updates a beacon API serves (EthCommittees), checking each as the
//! relay would, so the committee it checks a kept finality update's signature with is Ethereum's
//! own, whatever the beacon API or a peer claims.

use {
    super::{Payload, ProveError, ProveRules, Shown},
    crate::{
        db::{Column, DbError, State, namespace_key},
        params::anchoring::{AnchorSource, EthStart},
        structs::{address::Address, tx::{EthAnchorProof, MAX_ANCHOR_HEADER, MAX_ANCHOR_TX, MAX_ANCHOR_UPDATE}},
    },
    ethlight::{Finalized, LightClientBootstrap, LightClientHeader, LightClientUpdate, Network, Store, SyncCommittee, prove_ancestor, tx, verify_finality},
    r#macro::document,
    sha2::{Digest, Sha256},
    std::collections::BTreeMap,
};

#[cfg(test)]
mod tests;

#[document(
    summary = "SHA-256 and BLS natively, for ethlight's rules: what the node checks Ethereum anchors with, as the eth_relay contract checks them through its host calls.",
    category = "Anchoring",
    see_also = "EthRelay, EthCommittees",
)]
pub struct Native;

impl ethlight::Crypto for Native {
    fn sha256(&self, data: &[u8]) -> [u8; 32] {
        Sha256::digest(data).into()
    }

    fn fast_aggregate_verify(&self, pubkeys: &[ethlight::PublicKey], message: &[u8; 32], signature: &ethlight::Signature) -> bool {
        crate::crypto::bls::fast_aggregate_verify(pubkeys.as_flattened(), message, signature)
    }
}

#[document(
    summary = "An Ethereum relay contract's records in a state, read as the chain reads its own, laid out as contracts/examples/eth_relay keeps them: no call into it, so nothing it runs can change what a proof shows.",
    category = "Anchoring",
    see_also = "AnchorSettings::eth_relay, Relay",
)]
pub struct EthRelay<'a> {
    state: &'a State,
    address: Address,
}

impl<'a> EthRelay<'a> {
    #[document(parent = "EthRelay", summary = "The relay at an address.")]
    pub fn new(state: &'a State, address: Address) -> EthRelay<'a> {
        EthRelay { state, address }
    }

    fn own(&self, key: &[u8]) -> Result<Option<Vec<u8>>, DbError> {
        self.state.get(Column::Namespaces, &namespace_key(&self.address, &[], key))
    }

    fn spaced(&self, space: &[u8], key: &[u8]) -> Result<Option<Vec<u8>>, DbError> {
        self.state.get(Column::Namespaces, &namespace_key(&self.address, space, key))
    }

    #[document(parent = "EthRelay", summary = "The name of the network it started on.")]
    pub fn network_name(&self) -> Result<Option<String>, DbError> {
        Ok(self.own(b"network")?.and_then(|bytes| String::from_utf8(bytes).ok()))
    }

    #[document(parent = "EthRelay", summary = "The trusted block root it started at.")]
    pub fn start(&self) -> Result<Option<[u8; 32]>, DbError> {
        Ok(self.own(b"start")?.and_then(|bytes| bytes.try_into().ok()))
    }

    #[document(parent = "EthRelay", summary = "Its newest finalized header.")]
    pub fn finalized(&self) -> Result<Option<Finalized>, DbError> {
        Ok(self.own(b"finalized")?.and_then(|bytes| Finalized::decode(&bytes)))
    }

    #[document(parent = "EthRelay", summary = "A finalized header it holds, by its beacon block root: one kept in headers, or the newest.")]
    pub fn header(&self, root: &[u8; 32]) -> Result<Option<Finalized>, DbError> {
        let kept = match self.spaced(b"roots", root)? {
            Some(slot) => self.spaced(b"headers", &slot)?.and_then(|bytes| Finalized::decode(&bytes)),
            None => None,
        };

        match kept {
            Some(kept) => Ok(Some(kept)),
            None => Ok(self.finalized()?.filter(|finalized| finalized.root == *root)),
        }
    }

    #[document(parent = "EthRelay", summary = "The sync committee it holds for a period: the current one's, and the next's once known.")]
    pub fn committee(&self, period: u64) -> Result<Option<SyncCommittee>, DbError> {
        Ok(self.spaced(b"committees", &period.to_be_bytes())?.and_then(|bytes| SyncCommittee::decode(&bytes).ok()))
    }
}

impl EthAnchorProof {
    #[document(parent = "EthAnchorProof", summary = "Why it could never prove anything, before any header is read: parts past their limits, or a transaction branch other than 21 nodes.")]
    pub fn problem(&self) -> Option<&'static str> {
        if self.header.len() > MAX_ANCHOR_HEADER || self.tx.len() > MAX_ANCHOR_TX || self.update.len() > MAX_ANCHOR_UPDATE {
            return Some("an Ethereum anchor's header, transaction or update is past its limit");
        }

        (self.branch.len() != tx::TRANSACTION_BRANCH_LEN).then_some("a transaction's branch is 21 nodes")
    }
}

/// What a proof shows of Ethereum once its update is taken: the anchor's block, by its root and
/// slot, and the payload its transaction carries.
struct Found {
    root: [u8; 32],
    slot: u64,
    payload: Payload,
}

/// Checks what a proof shows beyond the update's signature, by ethlight's rules: the header is the
/// update's finalized one, or an ancestor of it by ancestry; the transaction is in its payload at
/// its place; its calldata is an anchor's 80 bytes.
fn follow_proof(network: &Network, proof: &EthAnchorProof, update: &LightClientUpdate) -> Result<Found, String> {
    let header = LightClientHeader::decode_for(&proof.header, network).map_err(|err| err.message().to_string())?;
    let held = Finalized::from_header(&Native, &update.finalized_header);
    let anchored = match &proof.ancestry {
        None => {
            header.check(&Native, network).map_err(|err| err.message().to_string())?;
            let found = Finalized::from_header(&Native, &header);

            if found.root != held.root {
                return Err("the anchor's header is not the finalized one, and no ancestry shows it is an ancestor".to_string());
            }

            found
        }
        Some(branch) => prove_ancestor(&Native, network, &held, &header, branch).map_err(|err| err.message().to_string())?,
    };

    if !tx::verify_transaction(&Native, &anchored.transactions_root, &proof.tx, proof.index, &proof.branch) {
        return Err("the transaction is not at its place in the block's execution payload".to_string());
    }

    let call = tx::decode_transaction(&proof.tx).map_err(|err| err.message().to_string())?;
    let payload = Payload::decode(call.data).ok_or("the transaction's calldata is not an anchor's 80 bytes")?;

    Ok(Found { root: anchored.root, slot: anchored.beacon.slot, payload })
}

/// Checks an Ethereum anchor's proof: Ethereum anchoring on and set up, the relay the settings name
/// started at eth_start, holding the finalized header the proof rests on, the update finalizing it
/// signed by the committee the relay holds for its period, and this chain's anchor of a checkpoint
/// on this branch with the list's root now in a transaction of that block, or of an ancestor.
pub(super) fn check(state: &State, rules: &ProveRules<'_>, proof: &EthAnchorProof) -> Result<Shown, ProveError> {
    let settings = rules.settings;
    let fail = |why: &str| Err(ProveError::Refused(why.to_string()));

    if !settings.ethereum {
        return fail("anchoring to Ethereum is off on this chain");
    }

    let (Some(relay), Some(start)) = (settings.eth_relay, &settings.eth_start) else {
        return fail("anchoring has no Ethereum relay or start");
    };
    let interval = super::interval(rules)?;

    if let Some(problem) = proof.problem() {
        return fail(problem);
    }

    let network = start.network.definition().map_err(|why| ProveError::Refused(why.to_string()))?;
    let relay = EthRelay::new(state, relay);

    if relay.start()? != Some(start.root) || relay.network_name()?.as_deref() != Some(network.name.as_ref()) {
        return fail("the Ethereum relay did not start where anchoring's eth_start says");
    }

    let update = LightClientUpdate::decode(&proof.update, &network).map_err(|err| ProveError::Refused(err.message().to_string()))?;

    if update.finalized_header.beacon.hash_tree_root(&Native) != proof.root {
        return fail("the finality update does not finalize the header the proof rests on");
    }

    let Some(held) = relay.header(&proof.root)? else {
        return fail("the Ethereum relay holds no such finalized header");
    };

    if held != Finalized::from_header(&Native, &update.finalized_header) {
        return fail("the Ethereum relay holds another header at that root");
    }

    let period = network.period_at_slot(update.signature_slot);
    let Some(committee) = relay.committee(period)? else {
        return fail("the Ethereum relay no longer holds the sync committee that signed the finality update: prove it with a newer one");
    };

    verify_finality(&Native, &network, &committee, &update, network.slot_at(rules.time / 1_000)).map_err(|err| ProveError::Refused(err.message().to_string()))?;
    let found = follow_proof(&network, proof, &update).map_err(ProveError::Refused)?;
    let (block, height) = super::check_payload(state, rules, interval, &found.payload)?;

    Ok(Shown { source: AnchorSource::Ethereum { root: found.root, slot: found.slot }, epoch: found.payload.epoch, block, height })
}

#[document(
    summary = "The sync committees of an Ethereum beacon network by period, learned by following light client updates from anchoring's eth_start, each checked as the relay checks them: what anchored sync checks a kept finality update's signature with, so nothing a beacon API serves is taken on its word.",
    category = "Anchoring",
    see_also = "EthStart, Sources, check_anchors",
)]
#[derive(Clone, Debug)]
pub struct EthCommittees {
    #[document(summary = "Where they were followed from.")]
    pub start: EthStart,
    #[document(summary = "The network followed.")]
    pub network: Network,
    #[document(summary = "Each period's committee, as far as the updates reached.")]
    pub by_period: BTreeMap<u64, SyncCommittee>,
    #[document(summary = "The network's slot when they were followed, which no update checked by them may be signed after.")]
    pub current_slot: u64,
}

impl EthCommittees {
    #[document(
        parent = "EthCommittees",
        summary = "Follows a beacon network from a start: the bootstrap at its trusted root, then each update in turn, every one taken only if the light client's rules take it. An update that does not apply is passed over, so a beacon API serving one wrong stops nothing it cannot prove.",
        params = { start = "Anchoring's eth_start.", bootstrap = "The bootstrap at its root, as SSZ.", updates = "Updates, as SSZ, oldest period first.", now = "The time now, in seconds since the Unix epoch." },
        returns = "The committees, or why the start does not check.",
    )]
    pub fn follow(start: &EthStart, bootstrap: &[u8], updates: &[Vec<u8>], now: u64) -> Result<EthCommittees, String> {
        let network = start.network.definition().map_err(str::to_string)?;
        let bootstrap = LightClientBootstrap::decode(bootstrap, &network).map_err(|err| format!("the bootstrap: {err}"))?;
        let mut store = Store::bootstrap(&Native, &network, &start.root, &bootstrap).map_err(|err| format!("the bootstrap: {err}"))?;
        let current_slot = network.slot_at(now);
        let mut by_period = BTreeMap::from([(store.period(), store.current.clone())]);

        for bytes in updates {
            let Ok(update) = LightClientUpdate::decode(bytes, &network) else {
                continue;
            };

            if store.process_update(&Native, &update, current_slot).is_ok() {
                by_period.insert(store.period(), store.current.clone());

                if let Some(next) = &store.next {
                    by_period.insert(store.period() + 1, next.clone());
                }
            }
        }

        Ok(EthCommittees { start: start.clone(), network, by_period, current_slot })
    }

    #[document(
        parent = "EthCommittees",
        summary = "Follows a beacon network from a start through a beacon API: the bootstrap at the start's root, then every period's best update to now, MAX_UPDATES_ASKED a request, each checked as follow checks it.",
        params = { beacon = "The beacon API.", start = "Anchoring's eth_start.", now = "The time now, in seconds since the Unix epoch." },
    )]
    pub fn fetch(beacon: &dyn EthBeacon, start: &EthStart, now: u64) -> Result<EthCommittees, String> {
        let network = start.network.definition().map_err(str::to_string)?;
        let bootstrap = beacon.bootstrap(&start.root)?;
        let from = LightClientBootstrap::decode(&bootstrap, &network).map(|bootstrap| network.period_at_slot(bootstrap.header.beacon.slot)).map_err(|err| format!("the bootstrap: {err}"))?;
        let to = network.period_at_slot(network.slot_at(now));
        let mut updates = Vec::new();
        let mut period = from;

        while period <= to {
            let count = (to - period + 1).min(MAX_UPDATES_ASKED);
            let got = beacon.updates(period, count)?;

            if got.is_empty() {
                break;
            }

            updates.extend(got);
            period += count;
        }

        EthCommittees::follow(start, &bootstrap, &updates, now)
    }

    #[document(parent = "EthCommittees", summary = "The last period whose committee it knows: where following goes on from.")]
    pub fn last_period(&self) -> u64 {
        self.by_period.keys().next_back().copied().unwrap_or(0)
    }
}

#[document(
    summary = "The most updates asked of a beacon API at once: its MAX_REQUEST_LIGHT_CLIENT_UPDATES, 128.",
    category = "Anchoring",
    see_also = "EthCommittees::fetch",
)]
pub const MAX_UPDATES_ASKED: u64 = 128;

#[document(
    summary = "What anchored sync learns Ethereum's sync committees from: a beacon API's bootstrap at a root and its updates by period, each as SSZ. Nothing it serves is taken on its word.",
    category = "Anchoring",
    see_also = "EthCommittees::fetch",
)]
pub trait EthBeacon: Send + Sync {
    #[document(parent = "EthBeacon", summary = "The bootstrap at a block root.")]
    fn bootstrap(&self, root: &[u8; 32]) -> Result<Vec<u8>, String>;
    #[document(parent = "EthBeacon", summary = "The best update of each of so many periods from one on, oldest first.")]
    fn updates(&self, start_period: u64, count: u64) -> Result<Vec<Vec<u8>>, String>;
}

/// Checks an Ethereum anchor's proof the way anchored sync does, against committees followed from
/// eth_start rather than the relay: the update signed by its period's committee and finalizing the
/// header the proof rests on, the anchor's block that header or an ancestor of it, the one the
/// entry names, and the transaction in it. Its payload and its slot.
pub(super) fn check_shown(proof: &EthAnchorProof, source: &AnchorSource, committees: &EthCommittees, index: usize) -> Result<(Payload, u64), String> {
    let network = &committees.network;

    if let Some(problem) = proof.problem() {
        return Err(format!("anchor {index}'s proof: {problem}"));
    }

    let update = LightClientUpdate::decode(&proof.update, network).map_err(|err| format!("anchor {index}'s finality update: {err}"))?;

    if update.finalized_header.beacon.hash_tree_root(&Native) != proof.root {
        return Err(format!("anchor {index}'s finality update does not finalize the header its proof rests on"));
    }

    let period = network.period_at_slot(update.signature_slot);
    let Some(committee) = committees.by_period.get(&period) else {
        return Err(format!("anchor {index}'s finality update is signed in period {period}, whose committee was not learned from the beacon API"));
    };

    verify_finality(&Native, network, committee, &update, committees.current_slot).map_err(|err| format!("anchor {index}'s finality update: {err}"))?;
    let found = follow_proof(network, proof, &update).map_err(|why| format!("anchor {index}: {why}"))?;

    if *source != (AnchorSource::Ethereum { root: found.root, slot: found.slot }) {
        return Err(format!("anchor {index}'s proof is of another Ethereum block than its entry"));
    }

    Ok((found.payload, found.slot))
}
