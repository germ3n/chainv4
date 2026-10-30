use {
    super::*,
    crate::api::{Api, Failure},
    crate::light::Light,
    crate::model::{DecisionsPage, GovernanceWeight},
    crate::store::{Decisions, Store},
    crate::views::{Look, Settings},
    chainv4::crypto::keypair::Keypair,
    chainv4::governance::{Change, ChangeKind},
    chainv4::light::Assurance,
    chainv4::structs::hash::{Hash, HashType},
    chainv4::structs::receipt::CommittedReceipt,
    chainv4::structs::tx::{FeeOffer, SignedTx},
    chainv4::structs::u256::u256,
    serde_json::json,
};

const HASH: HashType = HashType::Blake3_256;

fn coin() -> Coin {
    (18, "DEV".to_string())
}

/// GET /governance as a council chain answers it, with one change being approved.
fn listing() -> Value {
    json!({
        "kind": "council", "epoch": 3, "version": 3, "approval_epochs": 16, "needed": "2",
        "versions": { "consensus": 0, "bootnodes": 0, "name_prices": 0, "collections": 0, "federation": 0, "directory": 0, "fees": 0, "reward": 0, "limits": 0, "upgrade": 1, "name": 2 },
        "active": { "type": "authority", "key": "ed25519:0x01", "slot_ms": 2000 },
        "decided": { "type": "authority", "key": "ed25519:0x01", "slot_ms": 2000 },
        "bootnodes": ["/dns4/boot.example/udp/4001/quic-v1"], "finality": "ffg", "switch": null,
        "proposals": [
            {
                "name": "third", "kind": "name", "version": 2, "weight": "1", "needed": "2",
                "approvals": [{ "account": "ed25519:0x02", "weight": "1", "epoch": 3, "counts_until": 18 }],
            },
            {
                "finality": { "type": "depth", "depth": 12 }, "kind": "consensus", "version": 0, "weight": "1", "needed": "2",
                "approvals": [{ "account": "ed25519:0x03", "weight": "1", "epoch": 3, "counts_until": 18 }],
            },
            {
                "name": "fourth", "kind": "name", "version": 2, "weight": "1", "needed": "2",
                "approvals": [{ "account": "ed25519:0x03", "weight": "1", "epoch": 2, "counts_until": 17 }],
            },
        ],
        "members": ["ed25519:0x02", "ed25519:0x03"],
        "in_force": {
            "upgrade": { "value": { "least": "0.2.0", "from": 150000 }, "decided": 2 },
            "name": { "value": "second", "decided": 1 },
            "reward": { "value": { "initial": "2000000000000000000", "halving_interval": null }, "decided": null },
            "something_new": { "value": 7, "decided": null },
        },
    })
}

#[test]
fn governance_reads_as_get_governance_writes_it() {
    let view: GovernanceView = serde_json::from_value(listing()).unwrap();

    assert_eq!((view.kind.as_str(), view.version, view.approval_epochs, view.needed.as_str()), ("council", 3, 16, "2"));
    assert_eq!((view.versions["name"], view.versions["upgrade"], view.versions["consensus"]), (2, 1, 0));
    assert_eq!(by_kind(&view, false), "upgrade 1 · name 2", "the header shows the kinds decided at all");
    assert!(by_kind(&view, true).starts_with("consensus 0 · bootnodes 0 · name prices 0"), "the facts show every kind");
    assert_eq!(view.members.as_deref(), Some(&["ed25519:0x02".to_string(), "ed25519:0x03".to_string()][..]));
    assert!(!view.weighs_stake(), "a council's members weigh one each");

    let proposal = &view.proposals[0];
    assert_eq!(proposal.change(), json!({ "name": "third" }), "the change is what is left once its kind, version, weight, needed and approvals are read");
    assert_eq!((proposal.kind_name(), proposal.version), ("name".to_string(), 2));

    // Found by kind, address and value, newest first by its latest approval, or by how far it got
    // and when it closes.
    let items: Vec<Item> = view.proposals.iter().map(|proposal| open_item(proposal, &coin())).collect();
    let found = |finder: Finder| ballots::select(&items, &finder);

    assert_eq!(ballots::kinds_among(&items), ["consensus", "name"]);
    assert_eq!(found(Finder { kind: Some("name".to_string()), ..Finder::default() }), [0, 2]);
    assert_eq!(found(Finder { search: "fourth".to_string(), ..Finder::default() }), [2]);
    assert_eq!(found(Finder { search: "ED25519:0x03".to_string(), ..Finder::default() }), [1, 2], "whoever approved it");
    assert_eq!(found(Finder::default()), [0, 1, 2], "two made in epoch 3, then the one in epoch 2");
    assert_eq!(found(Finder { order: ballots::Order::ClosingSoonest, ..Finder::default() }), [2, 0, 1], "the one that counts until epoch 17 first");
    assert_eq!(found(Finder { order: ballots::Order::MostSupport, ..Finder::default() }), [0, 1, 2], "all alike: the node's order");
    assert_eq!(kind_heading("name", 2), "version 2: approving the name's change 3");
    assert_eq!(kind_heading("bootnodes", 0), "version 0: approving the bootnodes' change 1");

    // A node that names no kind: the change's own field says it.
    let unkinded: Proposal = serde_json::from_value(json!({ "switch_finality": { "epoch": 1 }, "weight": "0", "needed": "1" })).unwrap();
    assert_eq!(unkinded.kind_name(), "consensus");
    assert_eq!((proposal.weight.as_str(), proposal.approvals[0].counts_until), ("1", 18));
    assert_eq!(fraction(&proposal.weight, &proposal.needed), 0.5);

    assert_eq!(view.in_force["name"], crate::model::InForce { value: json!("second"), decided: Some(1) });
    assert_eq!(view.in_force["reward"].decided, None, "genesis's stands");
    assert_eq!(
        ordered(&view).iter().map(|(kind, _)| kind.as_str()).collect::<Vec<_>>(),
        ["name", "reward", "upgrade", "something_new"],
        "the page's order, then what it does not know of"
    );

    let electorate: GovernanceView = serde_json::from_value(json!({
        "kind": "electorate", "epoch": 0, "version": 0, "approval_epochs": 1, "needed": u256::MAX.to_string(),
        "finality": "ffg", "proposals": [], "members": null, "in_force": {},
    }))
    .unwrap();

    assert!(electorate.weighs_stake());
    assert_eq!(electorate.members, None);
    assert_eq!(needed_text(&electorate, &coin()), "more than anyone can approve: no change can pass now");
    assert_eq!(weight(&electorate, &coin(), "1500000000000000000"), "1.5 DEV");
    assert_eq!(by_kind(&electorate, false), "none decided yet", "an older node lists no versions");
}

#[test]
fn a_weight_reads_as_get_governance_weight_writes_it() {
    let found: GovernanceWeight = serde_json::from_value(json!({
        "account": "ed25519:0x02", "weight": "1", "needed": "2",
        "approved": [
            { "name": "third", "kind": "name", "version": 2 },
            { "finality": { "type": "depth", "depth": 12 }, "kind": "consensus", "version": 0 },
        ],
    }))
    .unwrap();

    assert_eq!((found.weight.as_str(), found.needed.as_str()), ("1", "2"));
    assert_eq!(
        found.approved.iter().map(|approved| approved_text(approved, &coin())).collect::<Vec<_>>(),
        ["Rename the chain to third, for the name's change 3", "Retune finality: depth, final 12 blocks deep, for the consensus's change 1"],
        "one approval a kind, each for its kind's next change"
    );
    assert_eq!(found.approved[0].change(), json!({ "name": "third" }));

    let stranger: GovernanceWeight = serde_json::from_value(json!({ "account": "ed25519:0x09", "weight": "0", "needed": "2", "approved": [] })).unwrap();
    assert!(stranger.approved.is_empty());
}

fn decision(version: u64) -> Value {
    json!({
        "version": version, "tx": format!("0x{version:02x}"), "block": format!("0xb{version}"), "height": version * 10,
        "timestamp": 1_000, "confirmations": 3, "final": version < 3, "sender": "ed25519:0x02",
        "kind": "name", "change": { "name": format!("name {version}") },
    })
}

/// Takes a page as the app takes one from the node.
trait Reads {
    fn read(&mut self, page: DecisionsPage, before: Option<u64>);
}

impl Reads for Decisions {
    fn read(&mut self, page: DecisionsPage, before: Option<u64>) {
        self.take(page.version, page.from, page.decisions, before);
    }
}

fn page(version: u64, from: u64, versions: &[u64]) -> DecisionsPage {
    serde_json::from_value(json!({ "version": version, "from": from, "decisions": versions.iter().map(|version| decision(*version)).collect::<Vec<_>>() })).unwrap()
}

#[test]
fn history_reads_as_get_governance_history_writes_it_and_pages_older() {
    let first = page(45, 1, &(26..=45).rev().collect::<Vec<_>>());

    assert_eq!((first.decisions[0].version, first.decisions[0].finalized, first.decisions[0].sender.as_deref()), (45, false, Some("ed25519:0x02")));
    assert_eq!(first.decisions[0].change, Some(json!({ "name": "name 45" })));

    let mut listing = Decisions::default();
    listing.read(first, None);

    assert_eq!((listing.bottom, listing.next(), listing.gaps()), (Some(26), Some(26), Vec::new()));

    listing.read(page(45, 1, &(6..=25).rev().collect::<Vec<_>>()), Some(26));
    listing.read(page(45, 1, &[5, 4, 3, 2, 1]), Some(6));

    assert_eq!((listing.items.len(), listing.next()), (45, None), "read to the first");
    assert_eq!(listing.newest_first().next().map(|decision| decision.version), Some(45));

    // Two more since, on the newest page again: they go in front.
    listing.read(page(47, 1, &(28..=47).rev().collect::<Vec<_>>()), None);
    assert_eq!((listing.items.len(), listing.version, listing.gaps()), (47, 47, Vec::new()));

    // A reorganization undid the newest.
    listing.read(page(46, 1, &(27..=46).rev().collect::<Vec<_>>()), None);
    assert_eq!((listing.items.len(), listing.version), (46, 46));

    // So many came since that the newest page does not meet what was read: it starts over.
    listing.read(page(100, 1, &(81..=100).rev().collect::<Vec<_>>()), None);
    assert_eq!((listing.items.len(), listing.bottom, listing.next()), (20, Some(81), Some(81)));
}

#[test]
fn decisions_are_found_by_kind_address_value_and_block() {
    let mut listing = Decisions::default();
    listing.read(page(5, 1, &[5, 4, 3, 2, 1]), None);
    let read: Vec<Decision> = listing.newest_first().cloned().collect();
    let items: Vec<Item> = read.iter().map(|decision| decided_item(decision, &coin())).collect();
    let found = |finder: Finder| ballots::select(&items, &finder).iter().map(|index| read[*index].version).collect::<Vec<_>>();

    assert_eq!(found(Finder::default()), [5, 4, 3, 2, 1]);
    assert_eq!(found(Finder { order: ballots::Order::Oldest, ..Finder::default() }), [1, 2, 3, 4, 5]);
    assert_eq!(found(Finder { search: "name 3".to_string(), ..Finder::default() }), [3], "a value");
    assert_eq!(found(Finder { kind: Some("name".to_string()), ..Finder::default() }).len(), 5);
    assert_eq!(found(Finder { kind: Some("fees".to_string()), ..Finder::default() }), Vec::<u64>::new());
    assert_eq!(found(Finder { search: "ed25519:0x02 rename".to_string(), ..Finder::default() }).len(), 5, "who sent it, and its words");
    assert_eq!(items[0].kind, "name");

    // Finality's tuning is on the consensus's ballot.
    let finality: Decision = serde_json::from_value(json!({ "version": 1, "tx": "0x01", "block": "0xb1", "height": 1, "timestamp": 1, "confirmations": 1, "kind": "finality", "change": { "finality": { "type": "depth", "depth": 6 } } })).unwrap();
    assert_eq!(decided_item(&finality, &coin()).kind, "consensus");

    // Five to the node's twenty read: a page short, and older ones to read.
    assert!(ballots::wants_older(5, &Finder::default(), PER_PAGE, true));
}

#[test]
fn versions_the_node_skips_show_as_gaps() {
    let mut listing = Decisions::default();
    listing.read(page(10, 1, &[10, 9, 7, 6, 3]), None);

    assert_eq!(listing.gaps(), [(4, 5), (8, 8)], "only between the lowest read and the newest");
    assert_eq!(listing.next(), Some(3));
    assert_eq!(gap(8, 8), "Change 8 is missing: the node did not list it, so what it changed is unknown here.");
    assert_eq!(gap(4, 5), "Changes 4 to 5 are missing: the node did not list them, so what they changed is unknown here.");

    listing.read(page(10, 1, &[1]), Some(3));
    assert_eq!(listing.gaps(), [(2, 2), (4, 5), (8, 8)]);
    assert_eq!(listing.next(), None);

    // A page that lists nothing says nothing is kept from its oldest on.
    let mut empty = Decisions::default();
    empty.read(page(3, 1, &[]), None);
    assert_eq!(empty.gaps(), [(1, 3)]);

    // A node that synced from a snapshot keeps the newest alone: no gap below what it keeps.
    let mut snapshot = Decisions::default();
    snapshot.read(page(12, 11, &[12, 11]), None);
    assert_eq!((snapshot.gaps(), snapshot.next()), (Vec::new(), None));

    let mut none = Decisions::default();
    none.read(page(0, 1, &[]), None);
    assert_eq!((none.gaps(), none.next()), (Vec::new(), None), "nothing decided yet");
}

#[test]
fn every_kind_of_change_reads_in_words() {
    let coin = coin();
    let said = |change: Value| describe(&change, &coin);

    assert_eq!(said(json!({ "consensus": { "type": "pow", "hash": "blake3", "target_block_ms": 2000, "initial_difficulty": "1", "min_difficulty": "1" } })), "Change consensus to proof of work by blake3, a block every 2s");
    assert_eq!(said(json!({ "consensus": { "type": "staked", "slot_ms": 500, "schedule": "priority" } })), "Change consensus to validators by stake, priority, a block every 500 ms");
    assert_eq!(said(json!({ "consensus": { "type": "rotating", "slot_ms": 1000 } })), "Change consensus to signers in turn, a block every 1s");
    assert_eq!(said(json!({ "name": "chainv4 mainnet" })), "Rename the chain to chainv4 mainnet");
    assert_eq!(said(json!({ "bootnodes": [] })), "Remove every bootnode");
    assert_eq!(said(json!({ "bootnodes": ["/ip4/203.0.113.7/udp/4001/quic-v1"] })), "Set the bootnodes to /ip4/203.0.113.7/udp/4001/quic-v1");
    assert_eq!(said(json!({ "bootnodes": ["/ip4/1", "/ip4/2"] })), "Set the bootnodes to 2 addresses");
    assert_eq!(said(json!({ "name_prices": ["5000000000000000000", "1000000000000000000"] })), "Price names at 5 DEV, 1 DEV a year, by length");
    assert_eq!(said(json!({ "collections": { "nft": ["owner_of", "transfer"], "multi": ["balance_of"] } })), "Make collections by 2 NFT functions and 1 multi-token ones");
    assert_eq!(
        said(json!({ "federation": { "signers": ["0x02", "0x03", "0x04"], "threshold": 2, "chain_depth": 6, "evm_depth": 12, "bitcoin_depth": 3 } })),
        "Hand the bridges to 2 of 3 signers, chain depth 6, EVM 12, Bitcoin 3"
    );
    assert_eq!(
        said(json!({ "directory": { "gateway": "contract:0x01", "btc_gateway": null, "evm_gateways": [{ "chain_id": 1, "gateway": "0x02" }], "swap": "contract:0x03", "dex": null, "dex_router": null, "nft_factory": null } })),
        "Announce the gateway, the swap contract, 1 EVM gateway"
    );
    assert_eq!(said(json!({ "directory": { "evm_gateways": [] } })), "Announce no contracts");
    assert_eq!(
        said(json!({ "fees": { "block_gas_limit": 30000000, "burn_permille": 500, "schedule": { "type": "weighted" }, "pricing": { "type": "base_fee", "initial": "1000", "min": "100", "elasticity": 2, "change_denominator": 8 } } })),
        "Set the fees: 30,000,000 gas a block, 50% burned, a base price of at least 100"
    );
    assert_eq!(said(json!({ "fees": { "block_gas_limit": 1, "burn_permille": 0, "pricing": { "type": "bid", "min_price": "7" } } })), "Set the fees: 1 gas a block, 0% burned, bids of at least 7");
    assert_eq!(said(json!({ "reward": { "initial": "2000000000000000000", "halving_interval": 210000 } })), "Set the block reward to 2 DEV a block, halving every 210,000 blocks");
    assert_eq!(said(json!({ "reward": { "initial": "0", "halving_interval": null } })), "Set the block reward to 0 DEV a block, never halving");
    assert_eq!(
        said(json!({ "limits": { "tx_limits": { "fuel": 10000000, "memory": 33554432 }, "max_block_bytes": 2097152 } })),
        "Set the limits: 10,000,000 fuel and 32 MiB of memory a transaction, 2 MiB a block"
    );
    assert_eq!(said(json!({ "limits": { "tx_limits": { "fuel": 1, "memory": 1000 }, "max_block_bytes": null } })), "Set the limits: 1 fuel and 1,000 bytes of memory a transaction, blocks bound by gas alone");
    assert_eq!(said(json!({ "finality": { "type": "ffg", "checkpoint_interval": 32, "max_validators": 100 } })), "Retune finality: FFG, checkpoints every 32 blocks, at most 100 validators");
    assert_eq!(said(json!({ "finality": { "type": "depth", "depth": 12 } })), "Retune finality: depth, final 12 blocks deep");
    assert_eq!(
        said(json!({ "switch_finality": { "epoch": 9, "finality": { "type": "authority", "threshold": 2, "signers": [{}, {}, {}], "checkpoint_interval": 16 } } })),
        "Switch finality to authority, 2 of 3 signers, checkpoints every 16 blocks from epoch 9"
    );
    assert_eq!(said(json!({ "upgrade": { "least": "0.2.0", "from": 150000 } })), "Nodes older than 0.2.0 stop at block 150,000");
    assert_eq!(said(json!({ "something_new": 1 })), "Change the chain's something new");
    assert_eq!(said(json!({ "name": "a", "bootnodes": [] })), "A change the explorer cannot read", "a change has one field");
    assert_eq!(said(Value::Null), "A change the explorer cannot read");

    assert_eq!(summary("name", &json!("second"), &coin), "second", "in force, a setting reads as its value");
    assert_eq!(summary("bootnodes", &json!([]), &coin), "none");
    assert_eq!((label("name_prices"), label("reward"), label("consensus")), ("Name prices".to_string(), "Block reward".to_string(), "Consensus".to_string()));
}

/// A council member's key, and what it signs.
fn signed(tx: Tx) -> SignedTx {
    SignedTx::sign(tx, 0, FeeOffer::NONE, &Keypair::development())
}

/// What the light client proves of a decision: its receipt, with this outcome, and its transaction,
/// in block 0xb{version} at its height.
fn proven(signed: SignedTx, outcome: CommittedOutcome, block: &Decision, final_: bool) -> Checked {
    let id = signed.id(HASH);
    let block_id = Hash::from_bytes(HASH, &[block.version as u8; 32]).unwrap();

    Checked::Proven(Box::new(ProvenReceipt {
        signed,
        receipt: CommittedReceipt::new(id, 0, 0, u256::ZERO, outcome, Vec::new(), Vec::new()),
        assurance: Assurance { block: block_id, height: block.height, final_ },
    }))
}

/// The receipt's outcome for the chain's decision-th change, its kind's version-th.
fn decided(decision: u64, kind: ChangeKind, version: u64) -> CommittedOutcome {
    CommittedOutcome::Decided { decision, kind, version }
}

/// A decision as the node lists it, of this transaction, in a block of a real id.
fn listed(version: u64, signed: &SignedTx, change: Value) -> Decision {
    Decision {
        version,
        tx: signed.id(HASH).to_string(),
        block: Hash::from_bytes(HASH, &[version as u8; 32]).unwrap().to_string(),
        height: 10,
        timestamp: 0,
        confirmations: 1,
        finalized: false,
        sender: Some(signed.sender().to_string()),
        kind: Some("name".to_string()),
        change: Some(change),
    }
}

#[test]
fn a_decision_checks_when_its_receipt_decided_it_and_its_transaction_approved_it() {
    // The chain's fifth decision, the name's second: its approval named the name's version 1.
    let rename = signed(Tx::ApproveChange { version: 1, change: Change::Name("second".to_string()) });
    let decision = listed(5, &rename, json!({ "name": "second" }));
    let check = |checked: &Checked| verdict(&decision, Some(checked), None, HASH);

    assert_eq!(check(&proven(rename.clone(), decided(5, ChangeKind::Name, 2), &decision, true)), Verdict::Proven { final_: true });
    assert_eq!(check(&proven(rename.clone(), decided(5, ChangeKind::Name, 2), &decision, false)), Verdict::Proven { final_: false });
    assert_eq!(
        check(&proven(rename.clone(), CommittedOutcome::Done, &decision, true)),
        Verdict::Wrong("its receipt does not say it decided change 5".to_string()),
        "an approval that only counted"
    );
    assert_eq!(check(&proven(rename.clone(), decided(6, ChangeKind::Name, 2), &decision, true)), Verdict::Wrong("its receipt does not say it decided change 5".to_string()), "the history's number is the chain's count");

    // The receipt's kind is the one the node lists, and the one the transaction approves.
    assert_eq!(
        check(&proven(rename.clone(), decided(5, ChangeKind::Bootnodes, 2), &decision, true)),
        Verdict::Wrong("the node lists a change of name, but its receipt decided one of bootnodes".to_string())
    );
    let listed_as_bootnodes = Decision { kind: Some("bootnodes".to_string()), ..decision.clone() };
    assert_eq!(
        verdict(&listed_as_bootnodes, Some(&proven(rename.clone(), decided(5, ChangeKind::Bootnodes, 2), &listed_as_bootnodes, true)), None, HASH),
        Verdict::Wrong("its transaction approves a change of name, but its receipt decided one of bootnodes".to_string())
    );
    let unkinded = Decision { kind: None, ..decision.clone() };
    assert_eq!(
        verdict(&unkinded, Some(&proven(rename.clone(), decided(5, ChangeKind::Name, 2), &unkinded, true)), None, HASH),
        Verdict::Proven { final_: true },
        "a decision the node lists no kind for checks by the receipt and transaction alone"
    );

    // The approval names the kind's version before the receipt's, not the chain's count before.
    assert_eq!(
        check(&proven(rename.clone(), decided(5, ChangeKind::Name, 3), &decision, true)),
        Verdict::Wrong("its transaction approves the name's change after version 1, not after 2".to_string())
    );
    let by_the_count = signed(Tx::ApproveChange { version: 4, change: Change::Name("second".to_string()) });
    let counted = listed(5, &by_the_count, json!({ "name": "second" }));
    assert_eq!(
        verdict(&counted, Some(&proven(by_the_count, decided(5, ChangeKind::Name, 2), &counted, true)), None, HASH),
        Verdict::Wrong("its transaction approves the name's change after version 4, not after 1".to_string())
    );

    let other = Decision { change: Some(json!({ "name": "third" })), ..decision.clone() };
    assert_eq!(
        verdict(&other, Some(&proven(rename.clone(), decided(5, ChangeKind::Name, 2), &decision, true)), None, HASH),
        Verdict::Wrong("its transaction approves name, not the change the node lists".to_string())
    );

    let elsewhere = Decision { block: Hash::from_bytes(HASH, &[9; 32]).unwrap().to_string(), ..decision.clone() };
    assert!(matches!(verdict(&elsewhere, Some(&proven(rename.clone(), decided(5, ChangeKind::Name, 2), &decision, true)), None, HASH), Verdict::Wrong(why) if why.starts_with("the node lists it in block")));

    let transfer = signed(Tx::Burn { from: Keypair::development().public().address(), amount: chainv4::structs::amount::Amount::U128(1) });
    let burned = listed(5, &transfer, json!({ "name": "second" }));
    assert_eq!(verdict(&burned, Some(&proven(transfer, decided(5, ChangeKind::Name, 2), &burned, true)), None, HASH), Verdict::Wrong("its transaction approves no change".to_string()));

    let posing = Decision { sender: Some("ed25519:0x09".to_string()), ..decision.clone() };
    assert!(matches!(verdict(&posing, Some(&proven(rename.clone(), decided(5, ChangeKind::Name, 2), &decision, true)), None, HASH), Verdict::Wrong(why) if why.starts_with("its transaction was sent by")));

    // Before and without a proof.
    assert_eq!(verdict(&decision, None, None, HASH), Verdict::Checking);
    assert_eq!(verdict(&decision, Some(&Checked::Waiting(10)), None, HASH), Verdict::Waiting(10));
    assert_eq!(verdict(&decision, Some(&Checked::Refused("no".to_string())), None, HASH), Verdict::Wrong("no".to_string()));
    assert_eq!(verdict(&decision, None, Some("another chain"), HASH), Verdict::Unchecked("the light client stopped: another chain".to_string()));
}

#[test]
fn a_decision_of_finality_checks_on_the_consensus_s_ballot() {
    use chainv4::params::finality::{DepthConfig, FinalityConfig};

    // History names it finality; its kind, and the receipt's, is the consensus.
    let retune = signed(Tx::ApproveChange { version: 0, change: Change::Finality(FinalityConfig::Depth(DepthConfig { depth: 12 })) });
    let decision = Decision { kind: Some("finality".to_string()), ..listed(3, &retune, json!({ "finality": { "type": "depth", "depth": 12 } })) };
    let change = match &retune.tx {
        Tx::ApproveChange { change, .. } => chainv4::params::json::change_json(change),
        _ => unreachable!(),
    };
    let decision = Decision { change: Some(change), ..decision };

    assert_eq!(verdict(&decision, Some(&proven(retune.clone(), decided(3, ChangeKind::Consensus, 1), &decision, true)), None, HASH), Verdict::Proven { final_: true });
    assert_eq!(
        verdict(&decision, Some(&proven(retune, decided(3, ChangeKind::Fees, 1), &decision, true)), None, HASH),
        Verdict::Wrong("the node lists a change of finality, but its receipt decided one of fees".to_string())
    );
}

#[test]
fn a_decision_a_multisig_approved_checks_by_the_change_inside() {
    let inner = Tx::ApproveChange { version: 0, change: Change::Bootnodes(vec!["/ip4/1".to_string()]) };
    let approval = signed(Tx::ApproveMultisig {
        account: Keypair::development().public().address(),
        nonce: 0,
        fee: FeeOffer::NONE,
        tx: Box::new(inner),
    });
    let decision = Decision { kind: Some("bootnodes".to_string()), ..listed(4, &approval, json!({ "bootnodes": ["/ip4/1"] })) };

    assert_eq!(verdict(&decision, Some(&proven(approval.clone(), decided(4, ChangeKind::Bootnodes, 1), &decision, true)), None, HASH), Verdict::Proven { final_: true });
}

/// The page drawn once, on a wide screen or a phone's, and the text it drew.
fn drawn(store: &Store, narrow: bool) -> (String, bool) {
    let mut api = Api::new("http://127.0.0.1:1", Err("offline".to_string()), egui::Context::default());
    let mut light = Light::default();
    let mut settings = Settings::default();
    let mut go = None;
    let mut state = GovernanceState::default();
    let palette = crate::theme::DARK;
    let look = Look { palette, roles: crate::theme::Roles::of(&palette), narrow };
    let mut output = egui::Context::default().run_ui(egui::RawInput::default(), |ui| {
        let mut ctx = Ctx { store, light: &mut light, api: &mut api, go: &mut go, settings: &mut settings, now: 2_000, look };
        show(ui, &mut ctx, &mut state);
    });
    output.textures_delta.clear();
    let mut text = String::new();

    for clipped in &output.shapes {
        if let egui::epaint::Shape::Text(shape) = &clipped.shape {
            text.push_str(shape.galley.text());
            text.push('\n');
        }
    }

    // The offline API answers at once: what was asked is in the inbox.
    let asked = api.replies().iter().any(|reply| matches!(reply, crate::api::Reply::Answer { ask: Ask::Governance, .. }));

    (text, asked)
}

#[test]
fn a_chain_without_governance_says_it_has_none() {
    let mut store = Store::default();
    let (_, asked) = drawn(&store, false);
    assert!(asked, "nothing known yet: it asks");

    store.failures.insert(Ask::Governance, Failure { status: 404, message: "this chain has no governance".to_string() });
    let (text, asked) = drawn(&store, false);

    assert!(text.contains("This chain has no governance"), "{text}");
    assert!(!asked, "a 404 stands until the page's own poll asks again");
}

#[test]
fn a_governed_chain_shows_its_approvers_proposals_settings_and_decisions_on_any_screen() {
    let mut store = Store { governance: Some(serde_json::from_value(listing()).unwrap()), ..Store::default() };
    store.decisions.read(page(3, 1, &[3, 1]), None);

    for narrow in [false, true] {
        let (text, _) = drawn(&store, narrow);

        for wanted in [
            "Rename the chain to third",
            "Rename the chain to fourth",
            "Retune finality: depth, final 12 blocks deep",
            "version 2: approving the name's change 3",
            "version 0: approving the consensus's change 1",
            "By kind: upgrade 1 · name 2",
            "1 of 2 needed",
            "nodes older than 0.2.0 stop at block 150,000",
            "Rename the chain to name 3",
            "change 2",
            "genesis",
            "Change 2 is missing",
            "reported by the node",
        ] {
            assert!(text.contains(wanted), "{wanted} on a {} screen, in {text}", if narrow { "phone's" } else { "wide" });
        }
    }
}
