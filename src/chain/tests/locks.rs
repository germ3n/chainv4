//! Locks: what genesis locks and governance adds, holding its settings against every vote, the one
//! that would decide a change included.

use {
    super::*,
    crate::governance::{Change, ChangeVote, GovernanceConfig},
    crate::registry::changes::Changes,
};

/// A second member of a council.
fn member() -> Keypair {
    Keypair::from_seed([41; 32])
}

/// The dev chain with these locks, governed by a council of the authority alone, deciding with one
/// Yes, or of the authority and member, needing both.
fn governed(locked: &[&str], pair: bool) -> Chain {
    Chain::open(Arc::new(MemoryDb::default()), &locked_genesis(locked, pair)).unwrap()
}

fn locked_genesis(locked: &[&str], pair: bool) -> Genesis {
    let mut members = vec![authority().public().address()];

    if pair {
        members.push(member().public().address());
    }

    Genesis {
        governance: Some(GovernanceConfig::Council { threshold: members.len() as u32, members, approval_epochs: 8, quorum_permille: 0 }),
        locked: locked.iter().map(|path| path.to_string()).collect(),
        ..genesis()
    }
}

/// Votes Yes on a change as a key, at its kind's version: what the vote did, or why it failed.
fn vote(chain: &mut Chain, key: &Keypair, change: Change) -> Result<Effect, String> {
    let version = Changes::new(&chain.state()).version_of(change.kind()).unwrap();
    let tx = Tx::VoteChange { version, change, vote: ChangeVote::Yes };
    let receipts = advance(chain, vec![SignedTx::sign(tx, nonce(chain, key), FeeOffer::NONE, key, &chain.genesis())]);

    receipts[0].result.clone().map_err(|failure| failure.to_string())
}

fn decided(result: Result<Effect, String>) {
    assert!(matches!(result, Ok(Effect::Decided { .. })), "{result:?}");
}

fn locks(chain: &Chain) -> Vec<String> {
    Changes::new(&chain.state()).locks().unwrap()
}

fn paths(list: &[&str]) -> Vec<String> {
    list.iter().map(|path| path.to_string()).collect()
}

#[test]
fn genesis_locks_refuse_a_change_to_their_field_while_the_rest_of_the_kind_still_changes() {
    let mut chain = governed(&["reward.initial", "limits.tx_limits.fuel"], false);
    let reward = genesis().reward;
    let limits = genesis().block_limits();

    assert_eq!(locks(&chain), ["limits.tx_limits.fuel", "reward.initial"], "in state from the start, in order");

    assert_eq!(vote(&mut chain, &authority(), Change::Reward(Reward { initial: u256::from(5u64), ..reward })), Err("reward.initial is locked".to_string()));
    decided(vote(&mut chain, &authority(), Change::Reward(Reward { halving_interval: Some(1_000), ..reward })));
    assert_eq!(Changes::new(&chain.state()).reward().unwrap().map(|decided| decided.reward), Some(Reward { halving_interval: Some(1_000), ..reward }));

    // Two levels down, the same: the fuel holds, the block's size moves.
    let fuel = crate::vm::Limits { fuel: limits.tx_limits.fuel * 2, ..limits.tx_limits };
    assert_eq!(vote(&mut chain, &authority(), Change::Limits(BlockLimits { tx_limits: fuel, ..limits })), Err("limits.tx_limits.fuel is locked".to_string()));
    decided(vote(&mut chain, &authority(), Change::Limits(BlockLimits { max_block_bytes: Some(2 << 20), ..limits })));

    // What was decided stands now, and the lock holds it still.
    let decided_reward = Reward { halving_interval: Some(1_000), ..reward };
    assert_eq!(vote(&mut chain, &authority(), Change::Reward(Reward { initial: u256::ONE, ..decided_reward })), Err("reward.initial is locked".to_string()));
}

#[test]
fn a_change_of_locks_adds_paths_and_none_is_ever_taken_away() {
    let mut chain = governed(&["reward"], false);
    let dex = |protocol_permille: u32| Change::Dex(crate::params::vm::DexSettings { protocol_permille, ..crate::params::vm::DexSettings::DEFAULT });

    decided(vote(&mut chain, &authority(), Change::Locks(paths(&["anchoring.enforce", "dex"]))));
    assert_eq!(locks(&chain), ["anchoring.enforce", "dex", "reward"]);
    assert_eq!(vote(&mut chain, &authority(), dex(50)), Err("dex is locked".to_string()), "from the next vote on");

    // What stands already is refused, by itself or under a wider lock.
    assert_eq!(vote(&mut chain, &authority(), Change::Locks(paths(&["dex"]))), Err("dex is locked already".to_string()));
    assert_eq!(vote(&mut chain, &authority(), Change::Locks(paths(&["dex.protocol_permille"]))), Err("dex.protocol_permille is locked already, under dex".to_string()));

    // A wider lock joins the narrower one under it; neither goes.
    decided(vote(&mut chain, &authority(), Change::Locks(paths(&["anchoring"]))));
    assert_eq!(locks(&chain), ["anchoring", "anchoring.enforce", "dex", "reward"]);

    // Governance changing itself clears every open vote, and leaves every lock.
    decided(vote(&mut chain, &authority(), Change::Governance(GovernanceConfig::Council { members: vec![authority().public().address()], threshold: 1, approval_epochs: 16, quorum_permille: 0 })));
    assert_eq!(locks(&chain), ["anchoring", "anchoring.enforce", "dex", "reward"]);
}

#[test]
fn locks_locked_freeze_the_set() {
    let mut chain = governed(&[], false);

    decided(vote(&mut chain, &authority(), Change::Locks(paths(&["locks"]))));
    assert_eq!(vote(&mut chain, &authority(), Change::Locks(paths(&["reward"]))), Err("locks is locked".to_string()));

    // Genesis may freeze it from the start.
    let mut chain = governed(&["locks"], false);
    assert_eq!(vote(&mut chain, &authority(), Change::Locks(paths(&["dex"]))), Err("locks is locked".to_string()));
    decided(vote(&mut chain, &authority(), Change::Dex(crate::params::vm::DexSettings { protocol_permille: 50, ..crate::params::vm::DexSettings::DEFAULT })));
}

#[test]
fn variant_kinds_are_held_by_what_each_variant_holds_and_consensus_switch_and_upgrade_lock_whole() {
    use crate::consensus::{ConsensusConfig, PowConfig};

    let pow = Change::Consensus(ConsensusConfig::ProofOfWork(PowConfig { hash: crate::structs::hash::HashType::default_chain(), target_block_ms: 2_000, initial_difficulty: 1_000, min_difficulty: 1_000, tuning: None, window: None }));
    let ConsensusConfig::Authority { slot_ms, .. } = genesis().consensus else { panic!("the dev chain's is an authority") };
    let other_key = |slot_ms: u64| Change::Consensus(ConsensusConfig::Authority { key: member().public(), slot_ms });

    // A field of the variant that stands: a change keeping it goes, one moving it or leaving it out
    // with another variant does not.
    let mut chain = governed(&["consensus.slot_ms"], false);
    assert_eq!(vote(&mut chain, &authority(), other_key(slot_ms + 1_000)), Err("consensus.slot_ms is locked".to_string()));
    assert_eq!(vote(&mut chain, &authority(), pow.clone()), Err("consensus.slot_ms is locked".to_string()));
    decided(vote(&mut chain, &authority(), other_key(slot_ms)));

    // A field the standing variant lacks keeps it lacking.
    let mut chain = governed(&["consensus.target_block_ms"], false);
    assert_eq!(vote(&mut chain, &authority(), pow.clone()), Err("consensus.target_block_ms is locked".to_string()));
    decided(vote(&mut chain, &authority(), other_key(slot_ms + 1_000)));

    // The consensus whole: nothing changes it.
    let mut chain = governed(&["consensus"], false);
    assert_eq!(vote(&mut chain, &authority(), other_key(slot_ms)), Err("consensus is locked".to_string()));

    // Any switch of finality, and any upgrade.
    let mut chain = governed(&["switch_finality", "upgrade"], false);
    #[cfg(feature = "finality-authority")]
    {
        let ffg = genesis().finality.unwrap();
        let config = crate::chain::FinalityConfig::Authority(crate::chain::AuthorityConfig { checkpoint_interval: ffg.checkpoint_interval(), activation_delay: ffg.activation_delay(), ..crate::chain::AuthorityConfig::dev(authority().public()) });
        let switch = Change::SwitchFinality { epoch: 1_000, config };
        assert_eq!(vote(&mut chain, &authority(), switch), Err("switch_finality is locked".to_string()));
    }
    let upgrade = Change::Upgrade(crate::structs::version::Upgrade { least: "0.2.0".parse().unwrap(), from: chain.tip().height + 1_000 });
    assert_eq!(vote(&mut chain, &authority(), upgrade.clone()), Err("upgrade is locked".to_string()));

    // Unlocked, the same upgrade is decided.
    decided(vote(&mut governed(&[], false), &authority(), upgrade));
}

#[test]
fn a_lock_is_checked_again_at_the_vote_that_would_decide_a_change_cast_before_it() {
    let mut chain = governed(&[], true);
    let raised = Change::Dex(crate::params::vm::DexSettings { protocol_permille: 50, ..crate::params::vm::DexSettings::DEFAULT });

    // One of the two Yes votes it needs, before any lock.
    assert_eq!(vote(&mut chain, &authority(), raised.clone()), Ok(Effect::Done));

    // Both lock the DEX's fee.
    let lock = Change::Locks(paths(&["dex.protocol_permille"]));
    assert_eq!(vote(&mut chain, &authority(), lock.clone()), Ok(Effect::Done));
    decided(vote(&mut chain, &member(), lock));

    // The second Yes would decide the fee, but what stands is locked since.
    assert_eq!(vote(&mut chain, &member(), raised), Err("dex.protocol_permille is locked".to_string()));
    assert_eq!(Changes::new(&chain.state()).dex().unwrap(), None, "genesis's fee stands");
}

#[test]
fn bad_paths_are_refused_in_genesis_and_in_a_change_of_locks() {
    let refused = |genesis: &Genesis| match Chain::open(Arc::new(MemoryDb::default()), genesis) {
        Ok(_) => panic!("opened a chain genesis should refuse"),
        Err(err) => err.to_string(),
    };

    assert!(refused(&locked_genesis(&["reward.initia"], false)).contains("names nothing in reward"));
    assert!(refused(&locked_genesis(&["colour"], false)).contains("not something governance changes"));
    assert!(refused(&locked_genesis(&["dex", "dex"], false)).contains("twice"));
    assert!(refused(&Genesis { locked: paths(&["dex"]), ..genesis() }).contains("without governance"));

    // Genesis files are refused as they are read.
    let mut file = crate::chain::json::to_json(&locked_genesis(&["dex"], false));
    assert_eq!(crate::chain::json::from_json(&file).unwrap().locked, ["dex"]);
    file["locked"] = serde_json::json!(["fees.burn"]);
    assert!(crate::chain::json::from_json(&file).unwrap_err().contains("names nothing in fees"));

    // And so is a change of locks, sent past the tools.
    let mut chain = governed(&[], false);
    assert!(vote(&mut chain, &authority(), Change::Locks(paths(&["fees.burn"]))).unwrap_err().contains("names nothing in fees"));
    assert!(vote(&mut chain, &authority(), Change::Locks(paths(&["reward", "dex"]))).unwrap_err().contains("in order"));
    assert!(vote(&mut chain, &authority(), Change::Locks(Vec::new())).unwrap_err().contains("at least one"));
    assert!(vote(&mut chain, &authority(), Change::Locks(paths(&["switch_finality.epoch"]))).unwrap_err().contains("locked whole"));
    assert!(locks(&chain).is_empty());
}

#[test]
fn a_genesis_without_locks_keeps_its_id_and_its_state() {
    let plain = locked_genesis(&[], false);
    let locked = locked_genesis(&["reward"], false);

    assert_ne!(locked.block().id(), plain.block().id(), "a chain's locks are part of it");
    assert_ne!(locked.block().header.state_root, plain.block().header.state_root, "and of its state");

    // A file from before the field reads as none, with the same id.
    let mut without = crate::chain::json::to_json(&plain);
    without.as_object_mut().unwrap().remove("locked");
    assert_eq!(crate::chain::json::from_json(&without).unwrap().block().id(), plain.block().id());

    // Empty, the field adds nothing to what genesis hashes; set, a 1 and the list.
    let list = wincode::serialize(&paths(&["reward"])).unwrap();
    assert_eq!(wincode::serialize(&locked).unwrap().len(), wincode::serialize(&plain).unwrap().len() + 1 + list.len());
}
