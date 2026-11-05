//! contracts/examples/eth_relay, Ethereum's light client as a contract: synthetic sync committees
//! of BLS keys made here sign real-format updates on mainnet's schedule, and the relay follows them
//! through the host's SHA-256 and BLS.

use {
    super::*,
    crate::db::namespace_key,
    ethlight::{
        Finalized, LightClientHeader, MAINNET,
        testing::{self, BlockRoots, Committee, Native, State},
        tx,
    },
};

const C: Native = Native;

/// A mainnet period well into Fulu.
const PERIOD: u64 = 1610;

fn slot(period: u64, plus: u64) -> u64 {
    period * MAINNET.slots_per_period() + plus
}

/// The chain's block time at an Ethereum slot, in milliseconds, a minute after it.
fn time_at(slot: u64) -> u64 {
    (MAINNET.genesis_time + 12 * slot + 60) * 1_000
}

fn bytes(value: &[u8]) -> TxCallArgumentType {
    TxCallArgumentType::Bytes(value.to_vec())
}

fn number(value: u64) -> TxCallArgumentType {
    TxCallArgumentType::U64(value)
}

struct Relay {
    world: World,
    relay: Address,
}

impl Relay {
    fn new() -> Relay {
        let mut world = World::new();
        let relay = world.deploy(Code::Wasm(example("eth_relay")));

        Relay { world, relay }
    }

    fn outcome(&self, sender: Address, function: &str, args: &[TxCallArgumentType], at: u64) -> Outcome {
        self.world.call_at(sender, self.relay, function, args, 0, time_at(at))
    }

    fn call(&self, function: &str, args: &[TxCallArgumentType], at: u64) -> Result<Vec<TxCallArgumentType>, String> {
        self.outcome(DEPLOYER, function, args, at).result.map_err(|err| err.to_string())
    }

    fn submit(&self, update: &ethlight::LightClientUpdate, at: u64) -> Result<Vec<TxCallArgumentType>, String> {
        self.call("submit", &[bytes(&update.encode(&MAINNET))], at)
    }

    /// A record in one of its namespaces, read natively, as the node reads btc_relay's.
    fn namespace(&self, name: &[u8], key: &[u8]) -> Option<Vec<u8>> {
        self.world.state.get(Column::Namespaces, &namespace_key(&self.relay, name, key)).unwrap()
    }

    fn finalized(&self) -> (u64, Vec<u8>, u64, Vec<u8>) {
        match &self.call("finalized", &[], 0).unwrap()[..] {
            [TxCallArgumentType::U64(slot), TxCallArgumentType::Bytes(root), TxCallArgumentType::U64(number), TxCallArgumentType::Bytes(hash)] => (*slot, root.clone(), *number, hash.clone()),
            other => panic!("{other:?}"),
        }
    }

    fn period(&self) -> (u64, bool) {
        match self.call("period", &[], 0).unwrap()[..] {
            [TxCallArgumentType::U64(period), TxCallArgumentType::Bool(known)] => (period, known),
            ref other => panic!("{other:?}"),
        }
    }
}

/// A relay started at PERIOD, with that period's committee and the next's.
fn started() -> (Relay, Committee, Committee) {
    let relay = Relay::new();
    let (current, next) = (Committee::new(1), Committee::new(2));
    let (bootstrap, trusted) = testing::bootstrap(&C, &MAINNET, slot(PERIOD, 64), &current.committee);
    relay.call("start", &[bytes(&bootstrap.encode(&MAINNET)), TxCallArgumentType::String("mainnet".into()), bytes(&trusted)], slot(PERIOD, 64)).unwrap();

    (relay, current, next)
}

/// A finalized header at a slot, its execution block holding some transactions.
fn finalized_at(at: u64, transactions: &[Vec<u8>]) -> LightClientHeader {
    testing::header(&C, &MAINNET, at, testing::filler(0x77, at, 0), testing::execution(&C, at, transactions))
}

#[test]
fn the_relay_starts_from_a_trusted_bootstrap_once_and_by_its_deployer_alone() {
    let relay = Relay::new();
    let committee = Committee::new(1);
    let (bootstrap, trusted) = testing::bootstrap(&C, &MAINNET, slot(PERIOD, 64), &committee.committee);
    let encoded = bootstrap.encode(&MAINNET);
    let start = |sender: Address, network: &str, root: &[u8]| relay.outcome(sender, "start", &[bytes(&encoded), TxCallArgumentType::String(network.into()), bytes(root)], slot(PERIOD, 64)).result.map_err(|err| err.to_string());

    assert_eq!(relay.call("network_name", &[], 0).unwrap(), [TxCallArgumentType::Nil]);
    assert!(relay.call("submit", &[bytes(&[0; 4])], 0).unwrap_err().contains("has not started"));
    assert!(start(Address::Ed25519([9; 32]), "mainnet", &trusted).unwrap_err().contains("only the deployer"));
    assert!(start(DEPLOYER, "goerli", &trusted).unwrap_err().contains("mainnet, sepolia or hoodi"));
    assert!(start(DEPLOYER, "mainnet", &[1; 32]).unwrap_err().contains("not the trusted block root"));
    assert!(start(DEPLOYER, "mainnet", &trusted[..31]).unwrap_err().contains("32 bytes"));

    start(DEPLOYER, "mainnet", &trusted).unwrap();
    assert!(start(DEPLOYER, "mainnet", &trusted).unwrap_err().contains("started already"));
    assert_eq!(relay.call("network_name", &[], 0).unwrap(), [TxCallArgumentType::String("mainnet".into())]);
    assert_eq!(relay.finalized(), (slot(PERIOD, 64), trusted.to_vec(), slot(PERIOD, 64), bootstrap.header.execution.block_hash.to_vec()));
    assert_eq!(relay.period(), (PERIOD, false));

    // Its records read natively, as their layout says.
    assert_eq!(relay.namespace(b"committees", &PERIOD.to_be_bytes()), Some(committee.committee.encode()));
    let record = relay.namespace(b"headers", &slot(PERIOD, 64).to_be_bytes()).unwrap();
    assert_eq!(Finalized::decode(&record), Some(Finalized::from_header(&C, &bootstrap.header)));
    assert_eq!(relay.namespace(b"roots", &trusted), Some(slot(PERIOD, 64).to_be_bytes().to_vec()));
}

#[test]
fn the_relay_follows_finalized_updates_across_a_period_and_keeps_their_headers() {
    let (relay, current, next) = started();
    let after = Committee::new(3);
    let header = finalized_at(slot(PERIOD, 7936), &[]);
    let learn = testing::update(&C, &MAINNET, &current, 400, slot(PERIOD, 8000), slot(PERIOD, 8001), &header, Some(&next.committee));

    // Too few signers, the wrong committee, and a signature from the future are refused.
    let few = testing::update(&C, &MAINNET, &current, 300, slot(PERIOD, 8000), slot(PERIOD, 8001), &header, Some(&next.committee));
    assert!(relay.submit(&few, slot(PERIOD, 8100)).unwrap_err().contains("two thirds"));
    let wrong = testing::update(&C, &MAINNET, &Committee::new(9), 512, slot(PERIOD, 8000), slot(PERIOD, 8001), &header, Some(&next.committee));
    assert!(relay.submit(&wrong, slot(PERIOD, 8100)).unwrap_err().contains("signature does not hold"));
    assert!(relay.submit(&learn, slot(PERIOD, 7990)).unwrap_err().contains("after the current slot"));

    // Anyone may send one that holds.
    let submitted = relay.outcome(Address::Ed25519([5; 32]), "submit", &[bytes(&learn.encode(&MAINNET))], slot(PERIOD, 8100));
    assert_eq!(submitted.result.unwrap(), [number(slot(PERIOD, 7936))]);
    assert_eq!(submitted.events[0].name, "Finalized");
    assert!(relay.submit(&learn, slot(PERIOD, 8100)).unwrap_err().contains("neither a newer"), "replayed");
    assert_eq!(relay.period(), (PERIOD, true));
    assert_eq!(relay.namespace(b"committees", &(PERIOD + 1).to_be_bytes()), Some(next.committee.encode()));
    let root = header.beacon.hash_tree_root(&C);
    assert_eq!(relay.finalized().1, root.to_vec());
    assert_eq!(relay.call("header", &[bytes(&root)], 0).unwrap(), [bytes(&Finalized::from_header(&C, &header).encode())]);
    assert_eq!(relay.call("header_at", &[number(slot(PERIOD, 7936))], 0).unwrap(), [bytes(&Finalized::from_header(&C, &header).encode())]);
    assert_eq!(relay.call("header_at", &[number(slot(PERIOD, 7937))], 0).unwrap(), [TxCallArgumentType::Nil]);

    // The next committee signs from the next period, and the relay moves on to it, dropping the
    // old committee and keeping the new next one.
    let crossing_header = finalized_at(slot(PERIOD + 1, 32), &[]);
    let crossing = testing::update(&C, &MAINNET, &next, 512, slot(PERIOD + 1, 100), slot(PERIOD + 1, 101), &crossing_header, Some(&after.committee));
    assert_eq!(relay.submit(&crossing, slot(PERIOD + 1, 200)).unwrap(), [number(slot(PERIOD + 1, 32))]);
    assert_eq!(relay.period(), (PERIOD + 1, true));
    assert_eq!(relay.namespace(b"committees", &PERIOD.to_be_bytes()), None);
    assert_eq!(relay.namespace(b"committees", &(PERIOD + 1).to_be_bytes()), Some(next.committee.encode()));
    assert_eq!(relay.namespace(b"committees", &(PERIOD + 2).to_be_bytes()), Some(after.committee.encode()));
    assert_eq!(relay.namespace(b"order", &2u64.to_be_bytes()), Some(slot(PERIOD + 1, 32).to_be_bytes().to_vec()), "the third header kept");

    // An update without a next committee, as a finality update is written.
    let plain = testing::update(&C, &MAINNET, &next, 512, slot(PERIOD + 1, 300), slot(PERIOD + 1, 301), &finalized_at(slot(PERIOD + 1, 256), &[]), None);
    assert_eq!(relay.submit(&plain, slot(PERIOD + 1, 400)).unwrap(), [number(slot(PERIOD + 1, 256))]);
    assert_eq!(relay.period(), (PERIOD + 1, true));

    // The old committee no longer signs.
    let stale = testing::update(&C, &MAINNET, &current, 512, slot(PERIOD + 1, 500), slot(PERIOD + 1, 501), &finalized_at(slot(PERIOD + 1, 480), &[]), None);
    assert!(relay.submit(&stale, slot(PERIOD + 1, 600)).is_err());
}

#[test]
fn transactions_in_held_headers_are_included_and_others_are_not() {
    let (relay, current, next) = started();
    let key = [0xa1; 20];
    let anchor = testing::eip1559(1, 7, &key, &[0xcd; 80]);
    let transactions = vec![testing::eip1559(1, 1, &[2; 20], b"swap"), anchor.clone(), testing::eip1559(1, 3, &[3; 20], &[])];

    // A checkpoint block holding the anchor, whose state also holds an older block's root.
    let older = finalized_at(slot(PERIOD, 100), &transactions);
    let at = slot(PERIOD, 4096);
    let mut state = State::new(MAINNET.fork_at_slot(at).fork, at);
    let mut roots = BlockRoots::new(at);
    roots.set(older.beacon.slot, older.beacon.hash_tree_root(&C));
    state.fields[testing::BLOCK_ROOTS as usize] = roots.root(&C);
    let checkpoint = testing::header(&C, &MAINNET, at, state.root(&C), testing::execution(&C, at, &transactions));
    let update = testing::update(&C, &MAINNET, &current, 512, at + 40, at + 41, &checkpoint, Some(&next.committee));
    relay.submit(&update, at + 100).unwrap();

    let (checkpoint_root, older_root) = (checkpoint.beacon.hash_tree_root(&C), older.beacon.hash_tree_root(&C));
    let branch = |index: u64| tx::transaction_branch(&C, &transactions, index).concat();
    let includes = |root: &[u8], transaction: &[u8], index: u64, branch: &[u8]| relay.call("includes", &[bytes(root), bytes(transaction), number(index), bytes(branch)], 0).unwrap();

    assert_eq!(includes(&checkpoint_root, &anchor, 1, &branch(1)), [TxCallArgumentType::Bool(true)]);
    assert_eq!(includes(&checkpoint_root, &anchor, 0, &branch(1)), [TxCallArgumentType::Bool(false)], "not at another place");
    assert_eq!(includes(&checkpoint_root, &transactions[0], 1, &branch(1)), [TxCallArgumentType::Bool(false)], "not another transaction");
    assert_eq!(includes(&checkpoint_root, &anchor, 1, &branch(1)[..640]), [TxCallArgumentType::Bool(false)], "not a short branch");
    assert_eq!(includes(&older_root, &anchor, 1, &branch(1)), [TxCallArgumentType::Bool(false)], "not in a block the relay does not hold");

    // The older block, proven by the checkpoint's block roots, is held then too.
    let ancestor_branch = roots.branch(&C, &state, older.beacon.slot).concat();
    let prove = |header: &LightClientHeader, branch: &[u8]| relay.call("prove_header", &[bytes(&checkpoint_root), bytes(&header.encode(MAINNET.fork_at_slot(header.beacon.slot).fork)), bytes(branch)], 0);
    assert!(prove(&finalized_at(slot(PERIOD, 101), &transactions), &ancestor_branch).unwrap_err().contains("not in the header's state"));
    assert_eq!(prove(&older, &ancestor_branch).unwrap(), [number(slot(PERIOD, 100))]);
    assert_eq!(includes(&older_root, &anchor, 1, &branch(1)), [TxCallArgumentType::Bool(true)]);
    assert_eq!(relay.call("header_at", &[number(slot(PERIOD, 100))], 0).unwrap(), [bytes(&Finalized::from_header(&C, &older).encode())]);
    assert_eq!(relay.finalized().0, at, "an ancestor does not move finality back");

    // What the anchor says: sent to the key itself on chain 1, with the 80 bytes.
    assert_eq!(relay.call("calldata", &[bytes(&anchor)], 0).unwrap(), [bytes(&key), bytes(&[0xcd; 80]), number(1)]);
    assert!(relay.call("calldata", &[bytes(&[0x02, 0xc0])], 0).is_err());
}

#[test]
fn submitting_an_update_costs_the_fuel_measured_here() {
    let (relay, current, next) = started();
    let costs = crate::params::vm::HostCosts::DEFAULT;
    let fuel = |update: &ethlight::LightClientUpdate, at: u64| {
        let outcome = relay.outcome(DEPLOYER, "submit", &[bytes(&update.encode(&MAINNET))], at);
        assert!(outcome.result.is_ok(), "{:?}", outcome.result);

        outcome.fuel_used
    };

    // With the next committee, 512 signers: its root hashed, its 24,624 bytes stored.
    let with_next = testing::update(&C, &MAINNET, &current, 512, slot(PERIOD, 200), slot(PERIOD, 201), &finalized_at(slot(PERIOD, 128), &[]), Some(&next.committee));
    let learning = fuel(&with_next, slot(PERIOD, 300));
    // Without one, 342 signers, the fewest it takes.
    let plain = testing::update(&C, &MAINNET, &current, 342, slot(PERIOD, 400), slot(PERIOD, 401), &finalized_at(slot(PERIOD, 352), &[]), None);
    let following = fuel(&plain, slot(PERIOD, 500));
    // With the next committee it already knows: hashed and compared, not stored.
    let again = testing::update(&C, &MAINNET, &current, 512, slot(PERIOD, 600), slot(PERIOD, 601), &finalized_at(slot(PERIOD, 544), &[]), Some(&next.committee));
    let known = fuel(&again, slot(PERIOD, 700));

    println!("eth_relay submit: {learning} fuel learning the next committee, {following} without one, {known} with one already known");

    // The BLS check itself is most of it, and the whole stays well within a transaction's fuel.
    assert!(following > costs.bls_verify + 342 * costs.bls_key);
    assert!(learning > costs.bls_verify + 512 * costs.bls_key);
    assert!([learning, following, known].iter().all(|fuel| *fuel < 4_000_000), "{learning} {following} {known}");
    assert!(following < learning, "no committee to hash or store");
}

/// What the chain's anchoring settings name, to stand for genesis's in the VM.
fn governing(relay: &mut Relay, network: crate::params::anchoring::EthNetwork, root: [u8; 32]) {
    let settings = crate::params::anchoring::AnchorSettings {
        ethereum: true,
        eth_relay: Some(relay.relay),
        eth_start: Some(crate::params::anchoring::EthStart { root, network }),
        ..crate::params::anchoring::AnchorSettings::OFF
    };
    relay.world.vm = Arc::new(Vm::new(&[Engine::Lua, Engine::Wasm], HashType::default_chain()).unwrap().with_anchoring(Some(settings)));
}

#[test]
fn the_relay_follows_the_network_the_chain_s_anchoring_names() {
    use crate::params::anchoring::EthNetwork;

    let (mut relay, current, next) = started();
    let trusted = testing::bootstrap(&C, &MAINNET, slot(PERIOD, 64), &current.committee).1;
    let finalized = finalized_at(slot(PERIOD, 128), &[]);
    let signed_as_built = testing::update(&C, &MAINNET, &current, 512, slot(PERIOD, 200), slot(PERIOD, 201), &finalized, Some(&next.committee));

    // Governance gives Fulu another version: updates signed under the built-in one stop holding,
    // and those under the voted one hold, without a redeploy.
    let mut voted = MAINNET.clone();
    let mut forks = voted.forks.to_vec();
    forks.last_mut().unwrap().version = [0x16, 0, 0, 0];
    voted.forks = forks.into();
    governing(&mut relay, EthNetwork::Custom(voted.encode()), trusted);
    assert!(relay.submit(&signed_as_built, slot(PERIOD, 300)).unwrap_err().contains("signature does not hold"));
    let mut signed_as_voted = signed_as_built.clone();
    testing::resign(&C, &current, 512, &mut signed_as_voted, [0x16, 0, 0, 0], &MAINNET.genesis_validators_root);
    assert_eq!(relay.submit(&signed_as_voted, slot(PERIOD, 300)).unwrap(), [number(slot(PERIOD, 128))]);

    // A network of another name is not the relay's: it goes back to the one it started with.
    let mut other = voted.clone();
    other.name = "gnosis-like".into();
    governing(&mut relay, EthNetwork::Custom(other.encode()), trusted);
    let later = testing::update(&C, &MAINNET, &current, 512, slot(PERIOD, 400), slot(PERIOD, 401), &finalized_at(slot(PERIOD, 352), &[]), None);
    assert_eq!(relay.submit(&later, slot(PERIOD, 500)).unwrap(), [number(slot(PERIOD, 352))]);

    // Lua reads the same definition.
    let lua = relay.world.deploy(Code::Lua("local M = {}\nfunction M.network() return chain.eth_network() end\nreturn M".to_string()));
    let read = |relay: &Relay| match &relay.world.call(lua, "network", &[]).result.unwrap()[..] {
        [TxCallArgumentType::Bytes(bytes)] => bytes.clone(),
        [TxCallArgumentType::String(text)] => text.clone().into_bytes(),
        other => panic!("{other:?}"),
    };
    assert_eq!(read(&relay), other.encode());
    governing(&mut relay, EthNetwork::Preset("sepolia".into()), trusted);
    assert_eq!(read(&relay), ethlight::SEPOLIA.encode(), "a preset as its built-in definition");
}

#[test]
fn a_relay_started_without_a_name_takes_the_governed_network() {
    use crate::params::anchoring::EthNetwork;

    let mut relay = Relay::new();
    let committee = Committee::new(1);
    let (bootstrap, trusted) = testing::bootstrap(&C, &MAINNET, slot(PERIOD, 64), &committee.committee);
    let start = |relay: &Relay| relay.call("start", &[bytes(&bootstrap.encode(&MAINNET)), TxCallArgumentType::String(String::new()), bytes(&trusted)], slot(PERIOD, 64));

    assert!(start(&relay).unwrap_err().contains("names no Ethereum network"));
    governing(&mut relay, EthNetwork::Preset("mainnet".into()), trusted);
    start(&relay).unwrap();
    assert_eq!(relay.call("network_name", &[], 0).unwrap(), [TxCallArgumentType::String("mainnet".into())]);
    assert_eq!(relay.namespace(b"", b"start"), Some(trusted.to_vec()));
}

/// An anchor written into Ethereum, proven end to end: a synthetic beacon chain's committee
/// finalizes a checkpoint the relay takes, the anchor's block an ancestor of it, and the chain
/// checks the proof natively against the relay's records.
#[cfg(feature = "anchoring-eth")]
#[test]
fn an_ethereum_anchor_is_proven_against_the_relay_and_refused_when_anything_is_off() {
    use crate::anchor::{Anchors, Payload, ProveError, ProveRules, chain_tag, prove};
    use crate::params::anchoring::{AnchorSettings, AnchorSource, EthNetwork, EthStart};
    use crate::structs::tx::{AnchorProof, EthAnchorProof};

    let (relay, current, next) = started();
    let trusted = testing::bootstrap(&C, &MAINNET, slot(PERIOD, 64), &current.committee).1;
    let genesis = crate::crypto::hasher::Hasher::digest(HashType::default_chain(), b"a chain");
    let checkpoint = crate::crypto::hasher::Hasher::digest(HashType::default_chain(), b"its checkpoint at epoch 2");
    let payload = |chain: [u8; 4]| Payload { chain, epoch: 2, block: checkpoint.to_bytes().try_into().unwrap(), root: [0; 32] }.encode();
    let key = [0xa1; 20];
    let anchor = testing::eip1559(1, 7, &key, &payload(chain_tag(&genesis)));
    let foreign = testing::eip1559(1, 8, &key, &payload(*b"else"));
    let short = testing::eip1559(1, 9, &key, &payload(chain_tag(&genesis))[..79]);
    let transactions = vec![testing::eip1559(1, 1, &[2; 20], b"swap"), anchor.clone(), foreign.clone(), short.clone()];

    // The anchor's block, and a checkpoint finalized after it whose state holds its root.
    let older = finalized_at(slot(PERIOD, 100), &transactions);
    let at = slot(PERIOD, 4096);
    let mut state = State::new(MAINNET.fork_at_slot(at).fork, at);
    let mut roots = BlockRoots::new(at);
    roots.set(older.beacon.slot, older.beacon.hash_tree_root(&C));
    state.fields[testing::BLOCK_ROOTS as usize] = roots.root(&C);
    let checkpoint_header = testing::header(&C, &MAINNET, at, state.root(&C), testing::execution(&C, at, &[] as &[&[u8]]));
    let update = testing::update(&C, &MAINNET, &current, 400, at + 40, at + 41, &checkpoint_header, Some(&next.committee));
    relay.submit(&update, at + 100).unwrap();

    relay.world.state.put_value(Column::Heights, 16u64.to_be_bytes().to_vec(), &checkpoint);
    let settings = AnchorSettings {
        ethereum: true,
        enforce: true,
        eth_relay: Some(relay.relay),
        eth_start: Some(EthStart { root: trusted, network: EthNetwork::Preset("mainnet".into()) }),
        ..AnchorSettings::OFF
    };
    let rules = ProveRules { settings: &settings, genesis, checkpoint_interval: Some(8), pooled: false, time: time_at(at + 100) };
    let branch = |index: u64| tx::transaction_branch(&C, &transactions, index);
    let fork = MAINNET.fork_at_slot(older.beacon.slot).fork;
    let proof = EthAnchorProof {
        header: older.encode(fork),
        root: checkpoint_header.beacon.hash_tree_root(&C),
        tx: anchor.clone(),
        index: 1,
        branch: branch(1),
        ancestry: Some(roots.branch(&C, &state, older.beacon.slot)),
        update: update.encode(&MAINNET),
    };
    let refused = |proof: EthAnchorProof| match prove(&relay.world.state, &rules, &Address::Ed25519([1; 32]), &AnchorProof::Ethereum(proof)) {
        Err(ProveError::Refused(why)) => why,
        other => panic!("{other:?}"),
    };

    // Another chain's anchor, calldata that is not the 80 bytes, a branch to another place.
    assert_eq!(refused(EthAnchorProof { tx: foreign.clone(), index: 2, branch: branch(2), ..proof.clone() }), "the anchor is another chain's");
    assert!(refused(EthAnchorProof { tx: short.clone(), index: 3, branch: branch(3), ..proof.clone() }).contains("not an anchor's 80 bytes"));
    assert!(refused(EthAnchorProof { branch: branch(0), ..proof.clone() }).contains("not at its place"));
    assert!(refused(EthAnchorProof { branch: branch(1)[..20].to_vec(), ..proof.clone() }).contains("21 nodes"));

    // A block that is no finalized header, shown without the ancestry that makes it one.
    assert!(refused(EthAnchorProof { ancestry: None, ..proof.clone() }).contains("not the finalized one"));
    assert!(refused(EthAnchorProof { ancestry: Some(roots.branch(&C, &state, older.beacon.slot + 1)), ..proof.clone() }).contains("not in the header's state"));

    // A header the relay does not hold, finalized by an update it never took.
    let unheld_header = testing::header(&C, &MAINNET, at + 32, state.root(&C), testing::execution(&C, at + 32, &[] as &[&[u8]]));
    let unheld = testing::update(&C, &MAINNET, &current, 400, at + 70, at + 71, &unheld_header, None);
    assert!(refused(EthAnchorProof { root: unheld_header.beacon.hash_tree_root(&C), update: unheld.encode(&MAINNET), ..proof.clone() }).contains("holds no such finalized header"));

    // An update that is not the one finalizing the root, or that the committee did not sign.
    assert!(refused(EthAnchorProof { update: unheld.encode(&MAINNET), ..proof.clone() }).contains("does not finalize the header"));
    let mut forged = update.clone();
    testing::resign(&C, &Committee::new(9), 512, &mut forged, MAINNET.fork_at_slot(at + 40).version, &MAINNET.genesis_validators_root);
    assert!(refused(EthAnchorProof { update: forged.encode(&MAINNET), ..proof.clone() }).contains("signature does not hold"));

    // Ethereum off, or a relay that did not start where the settings say.
    let off = AnchorSettings { ethereum: false, ..settings.clone() };
    assert!(matches!(prove(&relay.world.state, &ProveRules { settings: &off, ..rules }, &Address::Ed25519([1; 32]), &AnchorProof::Ethereum(proof.clone())), Err(ProveError::Refused(why)) if why.contains("off")));
    let elsewhere = AnchorSettings { eth_start: Some(EthStart { root: [7; 32], network: EthNetwork::Preset("mainnet".into()) }), ..settings.clone() };
    assert!(matches!(prove(&relay.world.state, &ProveRules { settings: &elsewhere, ..rules }, &Address::Ed25519([1; 32]), &AnchorProof::Ethereum(proof.clone())), Err(ProveError::Refused(why)) if why.contains("did not start")));

    // What holds is recorded, by the anchor's own block and slot.
    prove(&relay.world.state, &rules, &Address::Ed25519([1; 32]), &AnchorProof::Ethereum(proof.clone())).unwrap();
    let anchors = Anchors::new(&relay.world.state);
    assert_eq!(anchors.entry(0).unwrap().unwrap().source, AnchorSource::Ethereum { root: older.beacon.hash_tree_root(&C), slot: older.beacon.slot });
    assert_eq!(anchors.enforced_height().unwrap(), Some(16));
    assert!(refused(proof).contains("anchored already"));
}
