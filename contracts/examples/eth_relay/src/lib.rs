//! A light client of Ethereum's beacon chain as a contract: its deployer starts it from a
//! bootstrap at a block root it trusts, and then anyone submits light client updates, which it
//! takes once two thirds of the sync committee signed them, keeping the headers they finalize.
//! Contracts, and the node reading its records, then ask it whether an Ethereum transaction is in
//! a block Ethereum finalized: how an anchor written into Ethereum can be proven without anyone
//! vouching for it (docs/anchoring.md).
//!
//! The rules are ethlight's (the repository's `ethlight/`), the same the node runs: SHA-256 and the
//! BLS check go to the host, `hash` and `fast_aggregate_verify`.
//!
//! Networks: mainnet, sepolia and hoodi, by name, through each one's latest fork the build knows.
//! A fork after it signs under a version the schedule lacks, so updates stop until the contract is
//! upgraded with the new schedule.
//!
//! # Storage
//!
//! Numbers in values are little-endian, numbers in keys big-endian so they sort.
//!
//! The contract's own space:
//!
//! | Key | Value |
//! | --- | --- |
//! | `owner` | The deployer's address, 33 bytes: the only one who may start it. |
//! | `network` | The network's name, UTF-8: `mainnet`, `sepolia` or `hoodi`. |
//! | `store` | 57 bytes: the finalized header's slot (u64) and root (32), the current committee's period (u64), whether the next committee is known (1 byte), and how many headers were ever kept (u64). |
//! | `finalized` | The finalized header's record, 288 bytes, as `headers` holds it. |
//!
//! Namespaces:
//!
//! | Namespace | Key | Value |
//! | --- | --- | --- |
//! | `committees` | A period, u64 | Its sync committee, 24,624 bytes: 512 keys, then their aggregate, 48 bytes each, compressed. Only the current period's and, when known, the next's. |
//! | `headers` | A slot, u64 | The finalized header there, 288 bytes (`ethlight::Finalized`): its beacon root (32), its beacon header as SSZ (112: slot, proposer, parent, state and body roots), then its execution block's number (u64), hash (32), timestamp (u64), state root, receipts root and transactions root (32 each). |
//! | `roots` | A beacon block root, 32 bytes | Its slot, u64, big-endian: the key into `headers`. |
//! | `order` | How many were kept before it, u64 | Its slot, u64, big-endian. |
//!
//! At most MOST_KEPT = 8,192 headers stay in `headers`: keeping one more drops the one kept
//! longest ago, by `order`, from `headers`, `roots` and `order`. The finalized header is always in
//! `finalized` too, so the store survives that. Finalized updates come at most once an epoch, so
//! that is at least 36 days of them; ancestors proven with prove_header count towards it too, and
//! each costs its sender a transaction.

#![no_std]

extern crate alloc;

use {
    alloc::{string::String, vec::Vec},
    chainv4_sdk::{Address, Value, block_time, emit, export, sender, storage},
    ethlight::{Crypto, Error, Finalized, LightClientBootstrap, LightClientHeader, LightClientUpdate, Network, PublicKey, Root, Signature, Store, SyncCommittee, prove_ancestor, tx},
};

/// The most headers kept.
const MOST_KEPT: u64 = 8_192;

/// SHA-256 and BLS, on the host.
struct Host;

impl Crypto for Host {
    fn sha256(&self, data: &[u8]) -> [u8; 32] {
        chainv4_sdk::sha256(data)
    }

    fn fast_aggregate_verify(&self, pubkeys: &[PublicKey], message: &[u8; 32], signature: &Signature) -> bool {
        chainv4_sdk::fast_aggregate_verify(pubkeys.as_flattened(), message, signature)
    }
}

fn committees() -> storage::Namespace<'static> {
    storage::namespace(b"committees")
}

fn headers() -> storage::Namespace<'static> {
    storage::namespace(b"headers")
}

fn roots() -> storage::Namespace<'static> {
    storage::namespace(b"roots")
}

fn order() -> storage::Namespace<'static> {
    storage::namespace(b"order")
}

/// What `store` holds.
struct Kept {
    finalized_slot: u64,
    finalized_root: Root,
    period: u64,
    next_known: bool,
    count: u64,
}

impl Kept {
    const LEN: usize = 57;

    fn encode(&self) -> [u8; Self::LEN] {
        let mut out = [0u8; Self::LEN];
        out[..8].copy_from_slice(&self.finalized_slot.to_le_bytes());
        out[8..40].copy_from_slice(&self.finalized_root);
        out[40..48].copy_from_slice(&self.period.to_le_bytes());
        out[48] = u8::from(self.next_known);
        out[49..57].copy_from_slice(&self.count.to_le_bytes());

        out
    }

    fn decode(bytes: &[u8]) -> Option<Kept> {
        let number = |at: usize| Some(u64::from_le_bytes(bytes.get(at..at + 8)?.try_into().ok()?));

        (bytes.len() == Self::LEN).then_some(())?;

        Some(Kept { finalized_slot: number(0)?, finalized_root: bytes[8..40].try_into().ok()?, period: number(40)?, next_known: bytes[48] == 1, count: number(49)? })
    }
}

fn kept() -> Option<Kept> {
    storage::own().get(b"store").and_then(|bytes| Kept::decode(&bytes))
}

fn network() -> Result<&'static Network, &'static str> {
    let name = storage::own().get(b"network").ok_or("the relay has not started")?;

    core::str::from_utf8(&name).ok().and_then(Network::by_name).ok_or("the relay's network is unknown")
}

fn committee(period: u64) -> Result<SyncCommittee, &'static str> {
    let bytes = committees().get(&period.to_be_bytes()).ok_or("the relay lost a committee")?;

    SyncCommittee::decode(&bytes).map_err(Error::message)
}

fn finalized_record() -> Option<Finalized> {
    storage::own().get(b"finalized").and_then(|bytes| Finalized::decode(&bytes))
}

/// A header it holds, by its beacon root: kept in `headers`, or the finalized one.
fn by_root(root: &[u8]) -> Option<Finalized> {
    let slot = roots().get(root).and_then(|slot| <[u8; 8]>::try_from(slot).ok());
    let kept = slot.and_then(|slot| headers().get(&slot)).and_then(|bytes| Finalized::decode(&bytes));

    kept.or_else(|| finalized_record().filter(|finalized| finalized.root == root))
}

/// Keeps a header, dropping the one kept longest ago past MOST_KEPT. Whether it was new.
fn keep(header: &Finalized, kept: &mut Kept) -> bool {
    let slot = header.beacon.slot.to_be_bytes();

    if headers().get(&slot).is_some() {
        return false;
    }

    headers().set(&slot, &header.encode());
    roots().set(&header.root, &slot);
    order().set(&kept.count.to_be_bytes(), &slot);

    if let Some(oldest) = kept.count.checked_sub(MOST_KEPT) {
        let place = oldest.to_be_bytes();

        if let Some(old_slot) = order().get(&place) {
            if let Some(old) = headers().get(&old_slot).and_then(|bytes| Finalized::decode(&bytes)) {
                roots().delete(&old.root);
            }

            headers().delete(&old_slot);
            order().delete(&place);
        }
    }

    kept.count += 1;

    true
}

fn announce(header: &Finalized) {
    emit("Finalized", &[Value::U64(header.beacon.slot), Value::Bytes(header.root.to_vec()), Value::U64(header.block_number), Value::Bytes(header.block_hash.to_vec())]);
}

fn owner() -> Option<Address> {
    Address::decode(storage::own().get(b"owner")?.as_slice().try_into().ok()?)
}

/// Run once by the chain: whoever deploys may start it.
#[export]
fn init() {
    storage::own().set(b"owner", &sender().encode());
}

/// Starts the relay from a light client bootstrap, as SSZ, as a beacon node serves it at
/// /eth/v1/beacon/light_client/bootstrap/{root}, on a network, mainnet, sepolia or hoodi, at the
/// block root its deployer trusts: the header must be that block, and its state must hold the
/// committee. The deployer's, once.
#[export]
fn start(bootstrap: Vec<u8>, network_name: String, trusted_root: Vec<u8>) -> Result<bool, &'static str> {
    if owner() != Some(sender()) {
        return Err("only the deployer starts the relay");
    }

    if kept().is_some() {
        return Err("the relay has started already");
    }

    let network = Network::by_name(&network_name).ok_or(Error::UnknownNetwork.message())?;
    let trusted: Root = trusted_root.try_into().map_err(|_| "a block root is 32 bytes")?;
    let bootstrap = LightClientBootstrap::decode(&bootstrap, network).map_err(Error::message)?;
    let store = Store::bootstrap(&Host, network, &trusted, &bootstrap).map_err(Error::message)?;
    let mut kept = Kept { finalized_slot: store.finalized.beacon.slot, finalized_root: store.finalized.root, period: store.period(), next_known: false, count: 0 };

    storage::own().set(b"network", network.name.as_bytes());
    committees().set(&kept.period.to_be_bytes(), &store.current.encode());
    storage::own().set(b"finalized", &store.finalized.encode());
    keep(&store.finalized, &mut kept);
    storage::own().set(b"store", &kept.encode());
    announce(&store.finalized);

    Ok(true)
}

/// Takes a light client update, as SSZ, as a beacon node serves one of
/// /eth/v1/beacon/light_client/updates: one that two thirds of the committee signed, finalizing a
/// header newer than the relay's, or naming the next committee while it is unknown. A finality
/// update is taken too, written as an update whose next committee and its branch are zeros.
/// Anyone may send it. The finalized slot it then holds.
#[export]
fn submit(update: Vec<u8>) -> Result<u64, &'static str> {
    let network = network()?;
    let mut kept = kept().ok_or("the relay has not started")?;
    let next = match kept.next_known {
        true => Some(committee(kept.period + 1)?),
        false => None,
    };
    let finalized = finalized_record().ok_or("the relay lost its finalized header")?;
    let mut store = Store { network: network.clone(), finalized, current: committee(kept.period)?, next };
    let update = LightClientUpdate::decode(&update, network).map_err(Error::message)?;
    let applied = store.process_update(&Host, &update, network.slot_at(block_time() / 1_000)).map_err(Error::message)?;

    if applied.rotated {
        committees().delete(&kept.period.to_be_bytes());
        kept.period += 1;
    }

    if applied.rotated || applied.learned_next {
        if let Some(next) = &store.next {
            committees().set(&(kept.period + 1).to_be_bytes(), &next.encode());
        }

        kept.next_known = store.next.is_some();
    }

    if applied.finalized {
        storage::own().set(b"finalized", &store.finalized.encode());
        (kept.finalized_slot, kept.finalized_root) = (store.finalized.beacon.slot, store.finalized.root);
        keep(&store.finalized, &mut kept);
        announce(&store.finalized);
    }

    storage::own().set(b"store", &kept.encode());

    Ok(kept.finalized_slot)
}

/// Keeps an ancestor of a header it holds: a light client header, as SSZ, within the 8,192 slots
/// before it, proven by the held header's state's block_roots: the branch, 32 bytes a node, from
/// the ancestor's root to that state's root. How an Ethereum block that was not itself a
/// checkpoint is proven final. Anyone may send it. The ancestor's slot.
#[export]
fn prove_header(held_root: Vec<u8>, header: Vec<u8>, branch: Vec<u8>) -> Result<u64, &'static str> {
    let network = network()?;
    let mut kept = kept().ok_or("the relay has not started")?;
    let held = by_root(&held_root).ok_or("a block root the relay does not hold")?;
    let header = LightClientHeader::decode_for(&header, network).map_err(Error::message)?;

    if !branch.len().is_multiple_of(32) {
        return Err("a branch is 32 bytes a node");
    }

    let branch: Vec<Root> = branch.as_chunks::<32>().0.to_vec();
    let ancestor = prove_ancestor(&Host, network, &held, &header, &branch).map_err(Error::message)?;

    if keep(&ancestor, &mut kept) {
        storage::own().set(b"store", &kept.encode());
    }

    Ok(ancestor.beacon.slot)
}

/// What finalized gives: the slot, the beacon block root, the execution block's number and hash.
type Head = (u64, Vec<u8>, u64, Vec<u8>);

/// What calldata gives: the recipient, the calldata, the chain id.
type Calldata = (Option<Vec<u8>>, Vec<u8>, Option<u64>);

/// The finalized header: its slot, its beacon block root, and its execution block's number and
/// hash.
#[export]
fn finalized() -> Result<Head, &'static str> {
    let header = finalized_record().ok_or("the relay has not started")?;

    Ok((header.beacon.slot, header.root.to_vec(), header.block_number, header.block_hash.to_vec()))
}

/// The header kept at a slot, as its 288-byte record (see the storage layout); none for a slot it
/// does not hold.
#[export]
fn header_at(slot: u64) -> Option<Vec<u8>> {
    headers().get(&slot.to_be_bytes()).or_else(|| finalized_record().filter(|finalized| finalized.beacon.slot == slot).map(|finalized| finalized.encode().to_vec()))
}

/// The header kept with a beacon block root, as its record.
#[export]
fn header(root: Vec<u8>) -> Option<Vec<u8>> {
    by_root(&root).map(|header| header.encode().to_vec())
}

/// Whether a transaction, its bytes as the execution payload holds them, is the index-th in a
/// finalized block the relay holds, by its beacon block root and the SSZ branch from the
/// transaction to the payload's transactions_root: 21 nodes of 32 bytes, the last the count.
#[export]
fn includes(beacon_root: Vec<u8>, transaction: Vec<u8>, index: u64, branch: Vec<u8>) -> bool {
    let Some(header) = by_root(&beacon_root) else {
        return false;
    };

    if branch.len() != 32 * tx::TRANSACTION_BRANCH_LEN {
        return false;
    }

    let branch: Vec<Root> = branch.as_chunks::<32>().0.to_vec();

    tx::verify_transaction(&Host, &header.transactions_root, &transaction, index, &branch)
}

/// What an Ethereum transaction says, read from its bytes: who it went to, none for a creation;
/// its calldata; and the chain it is signed for, none for a legacy one from before EIP-155.
#[export]
fn calldata(transaction: Vec<u8>) -> Result<Calldata, &'static str> {
    let call = tx::decode_transaction(&transaction).map_err(Error::message)?;

    Ok((call.to.map(|to| to.to_vec()), call.data.to_vec(), call.chain_id))
}

/// The network it follows, as it was started; none before.
#[export]
fn network_name() -> Option<String> {
    storage::own().get(b"network").and_then(|bytes| String::from_utf8(bytes).ok())
}

/// The current sync committee's period, and whether the next committee is known.
#[export]
fn period() -> Result<(u64, bool), &'static str> {
    let kept = kept().ok_or("the relay has not started")?;

    Ok((kept.period, kept.next_known))
}
