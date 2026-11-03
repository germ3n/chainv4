use {
    super::*,
    crate::api::{Api, Failure},
    crate::light::Light,
    crate::model::{DecisionsPage, GovernanceWeight, RejectionsPage},
    crate::store::{Decisions, Rejections, Store},
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

/// A change's tally, as GET /governance writes it, of a council of two needing two.
fn tally(yes: u64, no: u64, abstain: u64) -> Value {
    json!({
        "yes": yes.to_string(), "no": no.to_string(), "abstain": abstain.to_string(), "voted": (yes + no + abstain).to_string(),
        "total": "2", "needed": "2", "quorum": "1", "more_needed": 2u64.saturating_sub(yes).to_string(),
        "quorum_met": yes + no + abstain >= 1, "can_pass": 2 - no >= 2,
    })
}

/// GET /governance as a council chain answers it, with three changes being voted on.
fn listing() -> Value {
    json!({
        "kind": "council", "epoch": 3, "version": 3, "approval_epochs": 16, "needed": "2", "total": "2",
        "quorum_permille": 500, "rejections": 1, "checkpoint_interval": 10,
        "voters": [{ "account": "ed25519:0x02", "weight": "1" }, { "account": "ed25519:0x03", "weight": "1" }],
        "versions": { "consensus": 0, "bootnodes": 0, "names": 0, "collections": 0, "federation": 0, "directory": 0, "fees": 0, "reward": 0, "limits": 0, "upgrade": 1, "name": 2 },
        "active": { "type": "authority", "key": "ed25519:0x01", "slot_ms": 2000 },
        "decided": { "type": "authority", "key": "ed25519:0x01", "slot_ms": 2000 },
        "bootnodes": ["/dns4/boot.example/udp/4001/quic-v1"], "finality": "ffg", "switch": null,
        "proposals": [
            {
                "name": "third", "kind": "name", "version": 2, "tally": tally(1, 0, 1), "closes": 18,
                "votes": [{ "account": "ed25519:0x02", "vote": "yes", "weight": "1", "epoch": 3, "counts_until": 18 }, { "account": "ed25519:0x03", "vote": "abstain", "weight": "1", "epoch": 3, "counts_until": 18 }],
            },
            {
                "finality": { "type": "depth", "depth": 12 }, "kind": "consensus", "version": 0, "tally": tally(0, 0, 0), "closes": 18,
                "votes": [{ "account": "ed25519:0x03", "vote": "no", "weight": "0", "epoch": 3, "counts_until": 18 }],
            },
            {
                "name": "fourth", "kind": "name", "version": 2, "tally": tally(0, 1, 0), "closes": 17,
                "votes": [{ "account": "ed25519:0x03", "vote": "no", "weight": "1", "epoch": 2, "counts_until": 17 }],
            },
        ],
        "members": ["ed25519:0x02", "ed25519:0x03"],
        "in_force": {
            "upgrade": { "value": { "least": "0.2.0", "from": 150000 }, "decided": 2 },
            "name": { "value": "second", "decided": 1 },
            "finality": { "value": { "type": "depth", "depth": 6 }, "decided": null },
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
    assert!(by_kind(&view, true).starts_with("consensus 0 · bootnodes 0 · names 0"), "the facts show every kind");
    assert_eq!(view.members.as_deref(), Some(&["ed25519:0x02".to_string(), "ed25519:0x03".to_string()][..]));
    assert!(!view.weighs_stake(), "a council's members weigh one each");

    let proposal = &view.proposals[0];
    assert_eq!(proposal.change(), json!({ "name": "third" }), "the change is what is left once its kind, version, tally and votes are read");
    assert_eq!((view.quorum_permille, view.rejections, view.checkpoint_interval, view.voters.len()), (500, 1, 10, 2));
    assert_eq!((proposal.vote_of("ed25519:0x02"), proposal.vote_of("ed25519:0x03"), proposal.vote_of("ed25519:0x09")), (Some(Choice::Yes), Some(Choice::Abstain), None));
    assert_eq!((proposal.tally().yes, proposal.tally().abstain, proposal.tally().support()), (1.0, 1.0, 0.5));
    assert!(!view.proposals[2].tally().can_pass, "one No of two leaves one: it can no longer pass");
    assert_eq!((proposal.kind_name(), proposal.version), ("name".to_string(), 2));

    // Found by kind, address and value, newest first by its latest approval, or by how far it got
    // and when it closes.
    let items: Vec<Item> = view.proposals.iter().map(|proposal| open_item(proposal, &coin())).collect();
    let found = |finder: Finder| ballots::select(&items, &finder);

    assert_eq!(ballots::kinds_among(&items), ["consensus", "name"]);
    assert_eq!(found(Finder { kind: Some("name".to_string()), ..Finder::default() }), [0, 2]);
    assert_eq!(found(Finder { search: "fourth".to_string(), ..Finder::default() }), [2]);
    assert_eq!(found(Finder { search: "ED25519:0x03".to_string(), ..Finder::default() }), [0, 1, 2], "whoever voted on it");
    assert_eq!(found(Finder { search: "0x03 no".to_string(), ..Finder::default() }), [1, 2], "and which way");
    assert_eq!(found(Finder::default()), [0, 1, 2], "two voted in epoch 3, then the one in epoch 2");
    assert_eq!(found(Finder { order: ballots::Order::ClosingSoonest, ..Finder::default() }), [2, 0, 1], "the one whose votes count until epoch 17 first");
    assert_eq!(found(Finder { order: ballots::Order::MostSupport, ..Finder::default() }), [0, 1, 2], "half of what it needs, then none, none: ties keep the node's order");
    assert_eq!(kind_heading("name", 2), "version 2: voting on the name's change 3");
    assert_eq!(kind_heading("bootnodes", 0), "version 0: voting on the bootnodes' change 1");

    // A node that names no kind: the change's own field says it.
    let unkinded: Proposal = serde_json::from_value(json!({ "switch_finality": { "epoch": 1 }, "tally": tally(0, 0, 0) })).unwrap();
    assert_eq!(unkinded.kind_name(), "consensus");
    assert_eq!((proposal.votes[0].vote.as_str(), proposal.votes[0].counts_until), ("yes", 18));

    assert_eq!(view.in_force["name"], crate::model::InForce { value: json!("second"), decided: Some(1) });
    assert_eq!(view.in_force["reward"].decided, None, "genesis's stands");
    assert_eq!(
        ordered(&view).iter().map(|(kind, _)| kind.as_str()).collect::<Vec<_>>(),
        ["name", "reward", "finality", "upgrade", "something_new"],
        "the page's order, then what it does not know of"
    );

    let electorate: GovernanceView = serde_json::from_value(json!({
        "kind": "electorate", "epoch": 0, "version": 0, "approval_epochs": 1, "needed": u256::MAX.to_string(),
        "finality": "ffg", "proposals": [], "members": null, "in_force": {},
    }))
    .unwrap();

    assert!(electorate.weighs_stake());
    assert_eq!(electorate.members, None);
    assert_eq!(needed_text(&electorate, &coin()), "more than anyone can vote: no change can pass now");
    assert_eq!(weight(&electorate, &coin(), "1500000000000000000"), "1.5 DEV");
    assert_eq!(by_kind(&electorate, false), "none decided yet", "an older node lists no versions");
}

#[test]
fn a_weight_reads_as_get_governance_weight_writes_it() {
    let found: GovernanceWeight = serde_json::from_value(json!({
        "account": "ed25519:0x02", "weight": "1", "needed": "2", "total": "2",
        "voted": [
            { "kind": "name", "version": 2, "vote": "yes", "epoch": 3, "counts_until": 18, "change": { "name": "third" } },
            { "kind": "consensus", "version": 0, "vote": "no", "epoch": 3, "counts_until": 18, "change": { "finality": { "type": "depth", "depth": 12 } } },
        ],
    }))
    .unwrap();

    assert_eq!((found.weight.as_str(), found.needed.as_str()), ("1", "2"));
    assert_eq!(
        found.voted.iter().map(|voted| voted_text(voted, &coin())).collect::<Vec<_>>(),
        ["Yes on rename the chain to third, for the name's change 3", "No on retune finality: depth, final 12 blocks deep, for the consensus's change 1"],
        "each vote for its kind's next change"
    );
    assert_eq!(found.voted[0].change, json!({ "name": "third" }));

    let stranger: GovernanceWeight = serde_json::from_value(json!({ "account": "ed25519:0x09", "weight": "0", "needed": "2", "voted": [] })).unwrap();
    assert!(stranger.voted.is_empty());
}

#[test]
fn a_card_rings_what_yes_has_dots_each_voter_and_marks_what_the_change_moves() {
    let view: GovernanceView = serde_json::from_value(listing()).unwrap();
    let names = |account: &str| (account == "ed25519:0x02").then(|| "alice.dev".to_string());
    let card = card_of(&view, &view.proposals[0], &coin(), &names, Some(29));

    assert_eq!((card.title.as_str(), card.what.as_str()), ("Name", "Rename the chain to third"));
    assert_eq!(card.standing, "1 more Yes needed");
    assert_eq!(card.weights, ["1".to_string(), "0".to_string(), "1".to_string()]);
    assert_eq!(card.voters.iter().map(|dot| (dot.label.as_str(), dot.vote)).collect::<Vec<_>>(), [("alice.dev", Some(Choice::Yes)), ("ed25519:0x03", Some(Choice::Abstain))], "names where the explorer knows them");
    assert_eq!(card.closing.as_deref(), Some("The oldest votes stop counting in 160 blocks, about 5 min"), "through epoch 18 of ten blocks, a block every 2 s");
    assert_eq!(card.fields, [votes::Field { path: "value".to_string(), before: Some("second".to_string()), after: Some("third".to_string()) }], "the name as it stands, and as it would be");

    // Finality's tuning beside finality in force: only its depth moves.
    let retune = card_of(&view, &view.proposals[1], &coin(), &names, None);
    assert_eq!(retune.fields.iter().filter(|field| field.changed()).map(|field| field.path.as_str()).collect::<Vec<_>>(), ["depth"]);
    assert_eq!(retune.closing, None, "no tip known yet");

    let rejected = card_of(&view, &view.proposals[2], &coin(), &names, None);
    assert_eq!(rejected.standing, "Can no longer pass: too much has voted No");
}

#[test]
fn turnout_counts_each_voter_s_votes_against_every_change_voted_on() {
    let view: GovernanceView = serde_json::from_value(listing()).unwrap();
    let vote = |height: u64, version: u64, name: &str, vote: &str| -> crate::model::FoundTx {
        serde_json::from_value(json!({
            "id": format!("0x{height:02x}"), "sender": "ed25519:0x02", "nonce": 0, "signer": "0x03", "signature": "0x04",
            "fee": { "gas_limit": 0, "max_price": "0", "tip": "0" },
            "tx": { "type": "vote_change", "version": version, "vote": vote, "kind": "name", "change": { "name": name } },
            "block": "0xb1", "height": height, "index": 0, "time": 1_000,
        }))
        .unwrap()
    };
    let mut cast = std::collections::HashMap::new();
    cast.insert("ed25519:0x02".to_string(), vec![vote(9, 2, "third", "yes"), vote(5, 1, "second", "yes")]);
    cast.insert("ed25519:0x03".to_string(), vec![vote(8, 2, "third", "abstain"), vote(7, 2, "fourth", "no"), vote(6, 1, "second", "yes")]);
    let tiles = turnouts(&view, &cast, &|_| None, 61_000);

    // Three changes voted on among them: the first voter's two, the second's three.
    assert_eq!(tiles.iter().map(|tile| tile.participation.map(|share| (share * 3.0).round() as u64)).collect::<Vec<_>>(), [Some(2), Some(3)]);
    assert_eq!(tiles[0].last.as_deref(), Some("last: Yes on name, 1m 0s ago"));
    assert_eq!(tiles[1].share, 0.5, "half the council's weight");

    let unread = turnouts(&view, &std::collections::HashMap::new(), &|_| None, 0);
    assert_eq!(unread[0].participation, None, "its votes not read yet");
    assert_eq!(turnout_asks(&Store { governance: Some(view), ..Store::default() }).len(), 2, "each voter's votes asked for");
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
    assert_eq!(said(json!({ "names": { "prices": ["5000000000000000000", "1000000000000000000"], "grace_ms": 7_776_000_000u64, "max_years": 100 } })), "Price names at 5 DEV, 1 DEV a year, by length, 90 days of grace, at most 100 years ahead");
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
    assert_eq!((label("names"), label("reward"), label("consensus")), ("Names".to_string(), "Block reward".to_string(), "Consensus".to_string()));
}

/// A council member's key, and what it signs. What a page shows comes from its receipt and
/// transaction, so no chain here checks the signature.
fn signed(tx: Tx) -> SignedTx {
    SignedTx::sign(tx, 0, FeeOffer::NONE, &Keypair::development(), &chainv4::structs::hash::Hash::zero(chainv4::structs::hash::HashType::default_chain()))
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
        of: None,
        kind_version: None,
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
fn a_decision_checks_when_its_receipt_decided_it_and_its_transaction_voted_on_it() {
    // The chain's fifth decision, the name's second: its approval named the name's version 1.
    let rename = signed(Tx::VoteChange { vote: chainv4::governance::ChangeVote::Yes, version: 1, change: Change::Name("second".to_string()) });
    let decision = listed(5, &rename, json!({ "name": "second" }));
    let check = |checked: &Checked| verdict(&decision, Some(checked), None, HASH);

    assert_eq!(check(&proven(rename.clone(), decided(5, ChangeKind::Name, 2), &decision, true)), Verdict::Proven { final_: true });
    assert_eq!(check(&proven(rename.clone(), decided(5, ChangeKind::Name, 2), &decision, false)), Verdict::Proven { final_: false });
    assert_eq!(
        check(&proven(rename.clone(), CommittedOutcome::Done, &decision, true)),
        Verdict::Wrong("its receipt does not say it decided change 5".to_string()),
        "a vote that only counted"
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
        Verdict::Wrong("its transaction votes on a change of name, but its receipt decided one of bootnodes".to_string())
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
        Verdict::Wrong("its transaction votes on the name's change after version 1, not after 2".to_string())
    );
    let by_the_count = signed(Tx::VoteChange { vote: chainv4::governance::ChangeVote::Yes, version: 4, change: Change::Name("second".to_string()) });
    let counted = listed(5, &by_the_count, json!({ "name": "second" }));
    assert_eq!(
        verdict(&counted, Some(&proven(by_the_count, decided(5, ChangeKind::Name, 2), &counted, true)), None, HASH),
        Verdict::Wrong("its transaction votes on the name's change after version 4, not after 1".to_string())
    );

    let other = Decision { change: Some(json!({ "name": "third" })), ..decision.clone() };
    assert_eq!(
        verdict(&other, Some(&proven(rename.clone(), decided(5, ChangeKind::Name, 2), &decision, true)), None, HASH),
        Verdict::Wrong("its transaction votes on name, not the change the node lists".to_string())
    );

    let elsewhere = Decision { block: Hash::from_bytes(HASH, &[9; 32]).unwrap().to_string(), ..decision.clone() };
    assert!(matches!(verdict(&elsewhere, Some(&proven(rename.clone(), decided(5, ChangeKind::Name, 2), &decision, true)), None, HASH), Verdict::Wrong(why) if why.starts_with("the node lists it in block")));

    let transfer = signed(Tx::Burn { from: Keypair::development().public().address(), amount: chainv4::structs::amount::Amount::U128(1) });
    let burned = listed(5, &transfer, json!({ "name": "second" }));
    assert_eq!(verdict(&burned, Some(&proven(transfer, decided(5, ChangeKind::Name, 2), &burned, true)), None, HASH), Verdict::Wrong("its transaction votes on no change".to_string()));

    let posing = Decision { sender: Some("ed25519:0x09".to_string()), ..decision.clone() };
    assert!(matches!(verdict(&posing, Some(&proven(rename.clone(), decided(5, ChangeKind::Name, 2), &decision, true)), None, HASH), Verdict::Wrong(why) if why.starts_with("its transaction was sent by")));

    // Before and without a proof.
    assert_eq!(verdict(&decision, None, None, HASH), Verdict::Checking);
    assert_eq!(verdict(&decision, Some(&Checked::Waiting(10)), None, HASH), Verdict::Waiting(10));
    assert_eq!(verdict(&decision, Some(&Checked::Refused("no".to_string())), None, HASH), Verdict::Wrong("no".to_string()));
    assert_eq!(verdict(&decision, None, Some("another chain"), HASH), Verdict::Unchecked("the light client stopped: another chain".to_string()));
}

/// A rejection as the node lists it, of this transaction, in a block of a real id.
fn listed_rejection(rejection: u64, signed: &SignedTx, change: Value) -> Rejection {
    Rejection {
        rejection,
        tx: signed.id(HASH).to_string(),
        block: Hash::from_bytes(HASH, &[rejection as u8; 32]).unwrap().to_string(),
        height: 10,
        timestamp: 0,
        confirmations: 1,
        finalized: false,
        sender: Some(signed.sender().to_string()),
        of: Some("name".to_string()),
        kind_version: Some(1),
        kind: Some("name".to_string()),
        change: Some(change),
    }
}

#[test]
fn a_rejection_checks_when_its_receipt_rejected_it_at_the_version_its_vote_named() {
    let no = signed(Tx::VoteChange { vote: chainv4::governance::ChangeVote::No, version: 1, change: Change::Name("second".to_string()) });
    let rejection = listed_rejection(2, &no, json!({ "name": "second" }));
    let as_decision = Decision { version: 2, of: None, kind_version: None, tx: rejection.tx.clone(), block: rejection.block.clone(), height: 10, timestamp: 0, confirmations: 1, finalized: false, sender: rejection.sender.clone(), kind: rejection.kind.clone(), change: rejection.change.clone() };
    let rejected = |number: u64, kind: ChangeKind, version: u64| CommittedOutcome::Rejected { rejection: number, kind, version };
    let check = |outcome: CommittedOutcome| rejection_verdict(&rejection, Some(&proven(no.clone(), outcome, &as_decision, true)), None, HASH);

    assert_eq!(check(rejected(2, ChangeKind::Name, 1)), Verdict::Proven { final_: true });
    assert_eq!(check(decided(2, ChangeKind::Name, 2)), Verdict::Wrong("its receipt does not say it rejected rejection 2".to_string()), "a decision is no rejection");
    assert_eq!(check(rejected(3, ChangeKind::Name, 1)), Verdict::Wrong("its receipt does not say it rejected rejection 2".to_string()), "the chain's count of rejections");
    assert_eq!(check(rejected(2, ChangeKind::Name, 2)), Verdict::Wrong("its transaction votes on the name's change after version 1, not after 2".to_string()), "a rejection leaves the version where the vote found it");
    assert_eq!(check(rejected(2, ChangeKind::Fees, 1)), Verdict::Wrong("the node lists a change of name, but its receipt rejected one of fees".to_string()));

    // And a decision is no rejection either.
    assert_eq!(verdict(&as_decision, Some(&proven(no.clone(), rejected(2, ChangeKind::Name, 1), &as_decision, true)), None, HASH), Verdict::Wrong("its receipt does not say it decided change 2".to_string()));
}

fn rejection(count: u64, height: u64) -> Value {
    json!({
        "rejection": count, "tx": format!("0x{count:02x}"), "block": format!("0xb{count}"), "height": height,
        "timestamp": 1_000, "confirmations": 3, "final": false, "sender": "ed25519:0x03",
        "of": "fees", "kind_version": 0, "kind": "fees", "change": { "fees": { "burn_permille": 900 } },
    })
}

#[test]
fn rejections_read_as_get_governance_rejections_writes_them_and_go_on_the_timeline() {
    let page: RejectionsPage = serde_json::from_value(json!({ "rejection": 2, "from": 1, "rejections": [rejection(2, 25), rejection(1, 15)] })).unwrap();
    let mut rejections = Rejections::default();
    rejections.take(page.rejection, page.from, page.rejections, None);

    assert_eq!((rejections.items.len(), rejections.next(), rejections.gaps()), (2, None, Vec::new()));
    assert_eq!(rejected_item(&rejections.items[&2], &coin()).kind, "fees");

    let mut decisions = Decisions::default();
    decisions.read(self::page(3, 1, &[3, 2, 1]), None);
    let marks = marks(&decisions, &rejections, &coin());

    assert_eq!(marks.len(), 5);
    assert_eq!(marks.iter().filter(|mark| mark.rejected).map(|mark| (mark.height, mark.label.as_str())).collect::<Vec<_>>(), [(15, "Fees rejected"), (25, "Fees rejected")]);
    assert_eq!(votes::cumulative(&marks), [[10.0, 1.0], [20.0, 2.0], [30.0, 3.0]], "decisions alone climb");
    assert_eq!(rejection_gap(4, 4), "Rejection 4 is missing: the node did not list it.");
}

#[test]
fn a_decision_of_finality_checks_on_the_consensus_s_ballot() {
    use chainv4::params::finality::{DepthConfig, FinalityConfig};

    // History names it finality; its kind, and the receipt's, is the consensus.
    let retune = signed(Tx::VoteChange { vote: chainv4::governance::ChangeVote::Yes, version: 0, change: Change::Finality(FinalityConfig::Depth(DepthConfig { depth: 12 })) });
    let decision = Decision { kind: Some("finality".to_string()), ..listed(3, &retune, json!({ "finality": { "type": "depth", "depth": 12 } })) };
    let change = match &retune.tx {
        Tx::VoteChange { change, .. } => chainv4::params::json::change_json(change),
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
fn a_decision_a_multisig_voted_for_checks_by_the_change_inside() {
    let inner = Tx::VoteChange { vote: chainv4::governance::ChangeVote::Yes, version: 0, change: Change::Bootnodes(vec!["/ip4/1".to_string()]) };
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
    let reader = chainv4::abi::known::AbiReader::default();
    let mut output = egui::Context::default().run_ui(egui::RawInput::default(), |ui| {
        let mut reading = crate::reading::Reading::default();
        let mut ctx = Ctx { store, reader: &reader, light: &mut light, reading: &mut reading, api: &mut api, go: &mut go, settings: &mut settings, now: 2_000, look };
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
fn a_governed_chain_shows_its_voters_proposals_settings_and_decisions_on_any_screen() {
    let mut store = Store { governance: Some(serde_json::from_value(listing()).unwrap()), ..Store::default() };
    store.decisions.read(page(3, 1, &[3, 1]), None);
    let rejections: RejectionsPage = serde_json::from_value(json!({ "rejection": 1, "from": 1, "rejections": [rejection(1, 15)] })).unwrap();
    store.rejections.take(rejections.rejection, rejections.from, rejections.rejections, None);

    for narrow in [false, true] {
        let (text, _) = drawn(&store, narrow);

        for wanted in [
            "Rename the chain to third",
            "Rename the chain to fourth",
            "Retune finality: depth, final 12 blocks deep",
            "version 2: voting on the name's change 3",
            "version 0: voting on the consensus's change 1",
            "By kind: upgrade 1 · name 2",
            "1 more Yes needed",
            "Can no longer pass",
            "What it moves",
            "Yes 1",
            "Abstain 1",
            "reading its votes",
            "REJECTIONS",
            "Set the fees",
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

#[test]
fn in_force_on_a_wide_screen_gives_what_a_setting_is_the_room_left_and_says_what_stands() {
    let mut value = listing();
    value["in_force"]["fees"] = json!({ "value": { "block_gas_limit": 50_000_000, "burn_permille": 500, "pricing": { "type": "bid", "min_price": "7" } }, "decided": null });
    let view: GovernanceView = serde_json::from_value(value).unwrap();
    let mut state = GovernanceState::default();
    let store = Store::default();
    // A phone on its side, beside the sidebar: where a grid would have squeezed it to a word a line.
    let (drawn, _) = crate::views::render_frames(&store, &mut Light::default(), 620.0, 4, |ui, ctx| {
        ctx.look.narrow = false;
        in_force(ui, ctx, &view, &mut state, &coin());
    });
    let fees = drawn.iter().find(|(text, _)| text.contains("gas a block")).expect("drawn");

    assert!(fees.1.width() > 300.0, "wide enough to read: {fees:?}");

    let name = drawn.iter().find(|(text, _)| text == "second");

    assert!(name.is_some(), "the name as it stands: {drawn:?}");
    assert!(!drawn.iter().any(|(text, _)| text.starts_with("Rename")), "no change it would make: {drawn:?}");
}

/// The page drawn on a screen of a size, a few frames, as it settles: every text drawn and where.
fn drawn_at(store: &Store, width: f32, height: f32) -> Vec<(String, egui::Rect)> {
    let mut api = Api::new("http://127.0.0.1:1", Err("offline".to_string()), egui::Context::default());
    let mut light = Light::default();
    let mut settings = Settings::default();
    let mut go = None;
    let mut state = GovernanceState::default();
    let palette = crate::theme::DARK;
    let look = Look { palette, roles: crate::theme::Roles::of(&palette), narrow: chainv4_ui::layout::is_narrow(width) };
    let reader = chainv4::abi::known::AbiReader::default();
    let context = egui::Context::default();
    let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, height))), ..Default::default() };
    let mut shown = Vec::new();

    for _ in 0..3 {
        let mut output = context.run_ui(input.clone(), |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                let mut reading = crate::reading::Reading::default();
                let mut ctx = Ctx { store, reader: &reader, light: &mut light, reading: &mut reading, api: &mut api, go: &mut go, settings: &mut settings, now: 2_000, look };
                show(ui, &mut ctx, &mut state);
            });
        });
        output.textures_delta.clear();
        shown = output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::epaint::Shape::Text(text) => Some((text.galley.text().to_string(), text.visual_bounding_rect())),
                _ => None,
            })
            .collect();
    }

    shown
}

#[test]
fn the_page_fits_a_phone_upright_and_on_its_side_and_a_desktop() {
    let mut store = Store { governance: Some(serde_json::from_value(listing()).unwrap()), ..Store::default() };
    store.decisions.read(page(3, 1, &[3, 1]), None);
    let rejections: RejectionsPage = serde_json::from_value(json!({ "rejection": 1, "from": 1, "rejections": [rejection(1, 15)] })).unwrap();
    store.rejections.take(rejections.rejection, rejections.from, rejections.rejections, None);

    for (width, height) in [(360.0, 740.0), (740.0, 360.0), (1200.0, 800.0)] {
        let shown = drawn_at(&store, width, height);

        assert!(shown.iter().any(|(text, _)| text == "Governance"), "the page at {width}x{height}: {shown:?}");

        for (text, rect) in shown.iter().filter(|(text, _)| !text.trim().is_empty()) {
            assert!(rect.right() <= width + 1.0, "{text:?} at {rect:?} runs past {width}x{height}");
        }
    }
}
