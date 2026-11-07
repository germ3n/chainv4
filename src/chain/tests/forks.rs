//! The fork schedule through the chain: blocks below a fork run by the rules before it and blocks
//! from it by the fork's, so a node replaying history comes out as the chain did; governance
//! schedules, moves and cancels one, which also stops older releases there; and a node whose
//! release does not know a fork the chain scheduled stops before it. The fork is the one that
//! exists in tests alone, a transfer's intrinsic gas one higher.

use {
    super::*,
    crate::params::forks::{ForkAt, ForkChange, ForkSchedule, ScheduledFork, TEST_FORK},
    crate::structs::version::Version,
};

/// The dev chain with its fees, so a transfer's gas shows, and these forks.
fn with_forks(forks: ForkSchedule) -> Genesis {
    Genesis { finality: None, forks, ..Genesis::dev_authority(authority().public()) }
}

/// The test fork from a height, as genesis lists one.
fn from(height: u64) -> ForkSchedule {
    ForkSchedule { forks: vec![ScheduledFork { name: TEST_FORK.name.to_string(), height, epoch: None, least: None }], lead: None }
}

/// A transfer from the authority, paying well above the base price.
fn paid(chain: &Chain) -> SignedTx {
    let tx = Tx::Transfer { from: authority().public().address(), to: Address::Ed25519([9; 32]), amount: Amount::U128(1) };
    let fee = FeeOffer { gas_limit: 100_000, max_price: chain.next_base_price() * u256::from(2u64), tip: u256::ZERO };

    SignedTx::sign(tx, nonce(chain, &authority()), fee, &authority(), &chain.genesis())
}

#[test]
fn a_chain_replays_across_a_fork_each_block_by_its_own_rules() {
    let mut chain = Chain::open(Arc::new(MemoryDb::default()), &with_forks(from(3))).unwrap();
    let mut blocks = Vec::new();
    let mut gas = Vec::new();

    for _ in 0..4 {
        let (block, now) = next_block(&chain, vec![paid(&chain)]);
        gas.push(extended(chain.import(&block, now))[0].gas_used);
        blocks.push((block, now));
    }

    // Blocks 1 and 2 by the rules before it; 3 and 4 by the fork's.
    assert_eq!(gas[1], gas[0]);
    assert_eq!(gas[2], gas[1] + 1, "{gas:?}");
    assert_eq!(gas[3], gas[2]);
    assert!(!chain.params_in(&chain.state(), 2).unwrap().rules.vm.forks.has(&TEST_FORK));
    assert!(chain.params_in(&chain.state(), 3).unwrap().rules.vm.forks.has(&TEST_FORK));

    // Another node replays every block from genesis and comes out at the same tip.
    let mut replay = Chain::open(Arc::new(MemoryDb::default()), &with_forks(from(3))).unwrap();

    for (block, now) in &blocks {
        extended(replay.import(block, *now));
    }

    assert_eq!(replay.tip().id(), chain.tip().id());

    // The schedule is the genesis's: another schedule is another chain, and none the one it was.
    let without = Chain::open(Arc::new(MemoryDb::default()), &with_forks(ForkSchedule::default())).unwrap();
    let plain = Chain::open(Arc::new(MemoryDb::default()), &Genesis { finality: None, ..Genesis::dev_authority(authority().public()) }).unwrap();

    assert_ne!(without.genesis(), chain.genesis());
    assert_eq!(without.genesis(), plain.genesis(), "no forks adds nothing to genesis");
    assert!(Changes::new(&without.state()).forks().unwrap().forks.is_empty());
    assert!(Chain::open(Arc::new(MemoryDb::default()), &with_forks(ForkSchedule { forks: vec![ScheduledFork { name: "from-the-future".to_string(), height: 9, epoch: None, least: None }], lead: None })).is_err(), "a genesis lists only forks its release knows");
}

/// Votes Yes on a change as the council of one, paying for it, and says why it was refused.
#[cfg(all(feature = "finality-ffg", feature = "governance-council"))]
fn vote_on(chain: &mut Chain, change: crate::governance::Change) -> Result<(), String> {
    let version = Changes::new(&chain.state()).version_of(change.kind()).unwrap();
    let vote = Tx::VoteChange { version, change, vote: crate::governance::ChangeVote::Yes };
    let fee = FeeOffer { gas_limit: 1_000_000, max_price: chain.next_base_price() * u256::from(2u64), tip: u256::ZERO };
    let signed = SignedTx::sign(vote, nonce(chain, &authority()), fee, &authority(), &chain.genesis());

    advance(chain, vec![signed])[0].result.as_ref().map(|_| ()).map_err(|why| why.to_string())
}

#[cfg(all(feature = "finality-ffg", feature = "governance-council"))]
#[test]
fn governance_schedules_moves_and_cancels_a_fork_and_a_node_stops_at_one_it_does_not_know() {
    use super::settings::{decide, governed};
    use crate::governance::Change;

    let least = Version::new(0, 3, 0);
    let schedule = |name: &str, height| Change::Fork(ForkChange::Schedule { name: name.to_string(), at: ForkAt::Height(height), least });
    let mut chain = governed(Genesis { fees: Some(crate::chain::fees::FeeConfig::dev()), ..genesis() });

    // A day of blocks ahead unless set: too far for a test, so a lead of 2.
    let soon = chain.tip().height + 5;
    let refused = vote_on(&mut chain, schedule("test-fork", soon)).expect_err("within a day");
    assert!(refused.contains("lead"), "{refused}");

    decide(&mut chain, Change::Fork(ForkChange::Lead { blocks: 2 }));
    let at = chain.tip().height + 6;
    decide(&mut chain, schedule("test-fork", at + 4));
    // Moved nearer, while it is ahead of the lead.
    decide(&mut chain, schedule("test-fork", at));

    let state = chain.state();
    let changes = Changes::new(&state);
    assert_eq!(changes.forks().unwrap().forks.iter().map(|fork| (fork.name.as_str(), fork.height)).collect::<Vec<_>>(), [("test-fork", at)]);
    assert_eq!(changes.upgrade().unwrap(), Some(crate::structs::version::Upgrade { least, from: at }), "older releases stop there");

    // The transfer before it and the one at it.
    let mut gas = Vec::new();
    let mut blocks = Vec::new();

    while chain.tip().height < at {
        let (block, now) = next_block(&chain, vec![paid(&chain)]);
        gas.push((block.header.height, extended(chain.import(&block, now))[0].gas_used));
        blocks.push((block, now));
    }

    let before = gas.iter().find(|(height, _)| *height == at - 1).unwrap().1;
    assert_eq!(gas.last().unwrap(), &(at, before + 1), "{gas:?}");

    // Passed, it is fixed.
    assert!(vote_on(&mut chain, Change::Fork(ForkChange::Cancel { name: "test-fork".to_string() })).unwrap_err().contains("passed"));

    // One this release does not know: scheduled as any, and no block runs from its height here.
    let unknown = chain.tip().height + 6;
    decide(&mut chain, schedule("from-the-future", unknown));
    decide(&mut chain, Change::Fork(ForkChange::Schedule { name: "later-still".to_string(), at: ForkAt::Height(unknown + 50), least }));
    decide(&mut chain, Change::Fork(ForkChange::Cancel { name: "later-still".to_string() }));

    while chain.tip().height + 1 < unknown {
        advance(&mut chain, Vec::new());
    }

    let (block, now) = next_block(&chain, Vec::new());
    assert!(matches!(chain.import(&block, now), Err(ImportError::UnknownFork(name)) if name == "from-the-future"));
    assert_eq!(Changes::new(&chain.state()).forks().unwrap().forks.iter().map(|fork| fork.name.as_str()).collect::<Vec<_>>(), ["test-fork", "from-the-future"]);
}

#[test]
fn a_genesis_file_writes_its_forks_and_reads_them_back_by_height_or_epoch() {
    let genesis = with_forks(ForkSchedule { forks: vec![ScheduledFork { name: TEST_FORK.name.to_string(), height: 40, epoch: None, least: None }], lead: Some(9) });
    let written = crate::chain::json::to_json(&genesis);

    assert_eq!(written["forks"]["scheduled"][0]["name"], "test-fork");
    assert_eq!(crate::chain::json::from_json(&written).unwrap().forks, genesis.forks);

    // None written as null, and a file from before the field reads as none.
    let mut plain = crate::chain::json::to_json(&with_forks(ForkSchedule::default()));
    assert_eq!(plain["forks"], serde_json::Value::Null);
    plain.as_object_mut().unwrap().remove("forks");
    assert_eq!(crate::chain::json::from_json(&plain).unwrap().forks, ForkSchedule::default());

    // By epoch, on a chain with finality, at the epoch's first block; not without finality.
    let mut by_epoch = crate::chain::json::to_json(&Genesis::dev_authority(authority().public()));
    by_epoch["forks"] = serde_json::json!({ "lead": null, "scheduled": [{ "name": "test-fork", "epoch": 3 }] });

    match crate::chain::json::from_json(&by_epoch) {
        Ok(read) => {
            let interval = read.finality.as_ref().unwrap().checkpoint_interval();
            assert_eq!((read.forks.forks[0].height, read.forks.forks[0].epoch), (3 * interval, Some(3)));
        }
        Err(why) => assert!(why.contains("no finality"), "{why}"),
    }

    let mut unknown = plain.clone();
    unknown["forks"] = serde_json::json!({ "lead": null, "scheduled": [{ "name": "from-the-future", "height": 3 }] });
    assert!(crate::chain::json::from_json(&unknown).unwrap_err().contains("does not know"));
}
