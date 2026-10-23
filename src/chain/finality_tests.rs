//! Finality on a chain: votes, justification and finalization, the honest-vote rule, evidence and
//! slashing, staking by transaction, proofs that pay voters, and fork choice around finality.

use super::*;
use crate::consensus::{AuthorityProducer, DevAuthority, Producer, meets, work_hash};
use crate::crypto::keypair::{Keypair, PrivateKey};
use crate::db::MemoryDb;
use crate::registry::validators::Validators;
use crate::structs::amount::Amount;
use crate::structs::tx::Tx;
use crate::structs::vote::{Checkpoint, SignedVote, Vote};

const SLOT: u64 = 2_000;
/// Blocks per epoch in these chains.
const INTERVAL: u64 = 4;
/// Each genesis validator's bond. Two of the three make two thirds.
const BOND: u64 = 10;
const VOTE_REWARD: u64 = 7;
/// Far enough ahead that no block in these tests is from the future.
const LATER: u64 = 10_000_000_000;

fn key(seed: u8) -> Keypair {
    #[cfg(feature = "ed25519")]
    let private = PrivateKey::Ed25519([seed; 32]);
    #[cfg(not(feature = "ed25519"))]
    let private = PrivateKey::EdBlake([seed; 32]);

    Keypair::from_private(private)
}

/// Three genesis validators, one key each.
fn validators() -> [Keypair; 3] {
    [key(11), key(12), key(13)]
}

fn authority() -> Keypair {
    DevAuthority::dev_keypair()
}

fn config() -> FinalityConfig {
    FinalityConfig {
        checkpoint_interval: INTERVAL,
        vote_depth: 1,
        min_bond: u256::new(100),
        activation_delay: 2,
        withdrawal_delay: 3,
        reporter_divisor: 4,
        max_validators: 10,
        vote_reward: u256::new(VOTE_REWARD.into()),
        max_proofs_per_block: 4,
        validators: validators()
            .iter()
            .map(|keypair| GenesisValidator {
                account: keypair.public().address(),
                voter: keypair.public(),
                bond: u256::new(BOND.into()),
            })
            .collect(),
    }
}

fn genesis() -> Genesis {
    Genesis {
        finality: Some(config()),
        ..Genesis::dev_authority(authority().public())
    }
}

fn open() -> Chain {
    Chain::open(Arc::new(MemoryDb::default()), &genesis()).unwrap()
}

/// Builds, seals and imports the next block with these transactions and proofs.
fn advance(chain: &mut Chain, txs: Vec<SignedTx>, proofs: Vec<Justification>) -> Result<Vec<Receipt>, ImportError> {
    let now = chain.tip().timestamp + SLOT;
    let mut producer = AuthorityProducer::new(authority(), SLOT);
    let mut block = chain.build(txs, proofs, producer.address(), now);
    block.seal = producer.seal(&block.header).unwrap();

    match chain.import(&block, now)? {
        Imported::Extended(receipts) => Ok(receipts),
        other => panic!("expected the tip to be extended, got {other:?}"),
    }
}

/// Adds empty blocks until the tip is at a height.
fn grow_to(chain: &mut Chain, height: u64) {
    while chain.tip().height < height {
        advance(chain, Vec::new(), Vec::new()).unwrap();
    }
}

/// The vote an honest validator signs now, remembered so it is never contradicted.
fn honest_vote(chain: &Chain, keypair: &Keypair) -> SignedVote {
    let vote = chain.vote_for(&keypair.public()).unwrap().expect("a vote is due");
    chain.record_own_vote(&vote).unwrap();

    SignedVote::sign(vote, keypair, &chain.genesis())
}

fn checkpoint(chain: &Chain, epoch: u64) -> Checkpoint {
    Checkpoint {
        epoch,
        block: chain.block_at(epoch * INTERVAL).unwrap().unwrap().id(),
    }
}

fn start(chain: &Chain) -> Checkpoint {
    Checkpoint {
        epoch: 0,
        block: chain.genesis(),
    }
}

/// Justifies the checkpoint of every epoch up to last, two validators voting each time.
fn finalize_through(chain: &mut Chain, last: u64) {
    for epoch in chain.justified().unwrap().epoch + 1..=last {
        grow_to(chain, epoch * INTERVAL + 1);

        for keypair in &validators()[..2] {
            let vote = honest_vote(chain, keypair);
            chain.add_vote(vote).unwrap();
        }
    }
}

fn balance(chain: &Chain, address: &Address) -> u256 {
    Accounts::new(&chain.state()).get(address).unwrap().balance
}

fn nonce(chain: &Chain, keypair: &Keypair) -> u64 {
    Accounts::new(&chain.state()).get(&keypair.public().address()).unwrap().nonce
}

#[test]
fn two_thirds_justify_and_the_next_checkpoint_finalizes() {
    let mut chain = open();
    grow_to(&mut chain, INTERVAL + 1);
    let [alice, bob, carol] = validators();
    let first = honest_vote(&chain, &alice);

    assert_eq!(first.vote.source, start(&chain));
    assert_eq!(first.vote.target, checkpoint(&chain, 1));

    let one = chain.add_vote(first).unwrap();

    assert!(one.justified.is_empty(), "a third is not enough");

    let two = chain.add_vote(honest_vote(&chain, &bob)).unwrap();

    assert_eq!(two.justified, [checkpoint(&chain, 1)]);
    assert_eq!(chain.justified(), Some(checkpoint(&chain, 1)));
    // Genesis is the source and already final; nothing new is.
    assert_eq!(two.finalized, None);

    // Nothing is left for the third to vote on, and a late vote for the same link changes nothing.
    assert_eq!(chain.vote_for(&carol.public()).unwrap(), None);

    let late = Vote {
        source: start(&chain),
        target: checkpoint(&chain, 1),
        voter: carol.public(),
    };

    assert!(chain.add_vote(SignedVote::sign(late, &carol, &chain.genesis())).unwrap().justified.is_empty());

    grow_to(&mut chain, 2 * INTERVAL + 1);
    chain.add_vote(honest_vote(&chain, &alice)).unwrap();
    let finalizing = chain.add_vote(honest_vote(&chain, &bob)).unwrap();

    assert_eq!(finalizing.justified, [checkpoint(&chain, 2)]);
    assert_eq!(finalizing.finalized, Some(checkpoint(&chain, 1)));
    assert_eq!(chain.finalized(), Some(checkpoint(&chain, 1)));
    assert_eq!(stored_finality(&*chain.db).unwrap(), (Some(checkpoint(&chain, 2)), Some(checkpoint(&chain, 1))));

    let proof = chain.justification(&checkpoint(&chain, 2).block).unwrap().unwrap();

    assert_eq!(proof.source, checkpoint(&chain, 1));
    assert_eq!(proof.votes.len(), 2);
    assert!(proof.votes.windows(2).all(|pair| pair[0].vote.voter < pair[1].vote.voter));
}

#[test]
fn finality_survives_a_restart() {
    let db = Arc::new(MemoryDb::default());
    let mut chain = Chain::open(db.clone(), &genesis()).unwrap();
    finalize_through(&mut chain, 2);
    let (justified, finalized) = (chain.justified(), chain.finalized());
    let reopened = Chain::open(db, &genesis()).unwrap();

    assert_eq!((reopened.justified(), reopened.finalized()), (justified, finalized));
    assert_eq!(finalized.unwrap().epoch, 1);
}

#[test]
fn votes_that_cannot_count_yet_wait_and_count_later() {
    let mut chain = open();
    grow_to(&mut chain, 2 * INTERVAL + 1);
    let [alice, bob, _] = validators();
    // Votes from epoch 1 to epoch 2, before epoch 1 is justified.
    let early = |keypair: &Keypair, chain: &Chain| {
        let vote = Vote {
            source: checkpoint(chain, 1),
            target: checkpoint(chain, 2),
            voter: keypair.public(),
        };

        SignedVote::sign(vote, keypair, &chain.genesis())
    };

    assert!(chain.add_vote(early(&alice, &chain)).unwrap().pending);
    assert!(chain.add_vote(early(&bob, &chain)).unwrap().pending);

    // Justifying epoch 1 lets the held votes count, which justifies 2 and finalizes 1.
    for keypair in [&alice, &bob] {
        let vote = Vote {
            source: start(&chain),
            target: checkpoint(&chain, 1),
            voter: keypair.public(),
        };
        let update = chain.add_vote(SignedVote::sign(vote, keypair, &chain.genesis())).unwrap();

        if keypair.public() == bob.public() {
            assert_eq!(update.justified, [checkpoint(&chain, 1), checkpoint(&chain, 2)]);
            assert_eq!(update.finalized, Some(checkpoint(&chain, 1)));
        }
    }
}

#[test]
fn bad_votes_are_refused() {
    let mut chain = open();
    grow_to(&mut chain, INTERVAL + 1);
    let outsider = key(99);
    let mut vote = honest_vote(&chain, &validators()[0]);
    let forged = SignedVote {
        vote: Vote {
            voter: outsider.public(),
            ..vote.vote
        },
        signature: vote.signature,
    };
    let not_validator = Vote {
        voter: outsider.public(),
        ..vote.vote
    };

    assert!(matches!(chain.add_vote(forged), Err(FinalityError::Invalid(_))));
    assert!(matches!(
        chain.add_vote(SignedVote::sign(not_validator, &outsider, &chain.genesis())),
        Err(FinalityError::NotValidator)
    ));

    // Signed for another chain.
    vote = SignedVote::sign(vote.vote, &validators()[0], &Hash::zero(chain.hash()));

    assert!(matches!(chain.add_vote(vote), Err(FinalityError::Invalid(_))));
}

#[test]
fn an_honest_validator_never_contradicts_itself() {
    let mut chain = open();
    grow_to(&mut chain, INTERVAL + 1);
    let alice = &validators()[0];
    honest_vote(&chain, alice);

    // Already voted for epoch 1.
    assert_eq!(chain.vote_for(&alice.public()).unwrap(), None);

    // Once it has voted from epoch 1, a vote from an older source could surround that one.
    chain
        .record_own_vote(&Vote {
            source: checkpoint(&chain, 1),
            target: checkpoint(&chain, 1),
            voter: alice.public(),
        })
        .unwrap();
    grow_to(&mut chain, 3 * INTERVAL + 1);

    assert_eq!(chain.vote_for(&alice.public()).unwrap(), None);
    assert_eq!(chain.vote_for(&key(99).public()).unwrap(), None, "not a validator");
    assert!(chain.vote_for(&validators()[1].public()).unwrap().is_some());
}

#[test]
fn conflicting_votes_are_evidence_and_cost_the_bond() {
    let mut chain = open();
    grow_to(&mut chain, 2 * INTERVAL + 1);
    let [alice, _, carol] = validators();
    let sign = |source: Checkpoint, target: Checkpoint| {
        SignedVote::sign(Vote { source, target, voter: carol.public() }, &carol, &chain.genesis())
    };
    let honest = sign(start(&chain), checkpoint(&chain, 2));
    let other_target = Checkpoint {
        epoch: 2,
        block: Hash::zero(chain.hash()),
    };
    let double = sign(start(&chain), other_target);

    chain.add_vote(honest).unwrap();
    let caught = chain.add_vote(double).unwrap().evidence.expect("a double vote is evidence");

    assert_eq!((caught.first, caught.second), (honest, double));
    assert_eq!(stored_evidence(&*chain.db).unwrap(), [caught]);

    // Surrounding: from genesis to epoch 3 around a vote from 1 to 2.
    let inner = SignedVote::sign(
        Vote {
            source: checkpoint(&chain, 1),
            target: checkpoint(&chain, 2),
            voter: alice.public(),
        },
        &alice,
        &chain.genesis(),
    );
    grow_to(&mut chain, 3 * INTERVAL + 1);
    let outer = SignedVote::sign(
        Vote {
            source: start(&chain),
            target: checkpoint(&chain, 3),
            voter: alice.public(),
        },
        &alice,
        &chain.genesis(),
    );
    chain.add_vote(inner).unwrap();

    assert!(chain.add_vote(outer).unwrap().evidence.is_some());

    // Anyone can report it; the reporter gets a quarter of the bond, and the rest is gone.
    let reporter = key(50);
    let fund = SignedTx::sign(
        Tx::Transfer {
            from: authority().public().address(),
            to: reporter.public().address(),
            amount: Amount::U128(1),
        },
        nonce(&chain, &authority()),
        &authority(),
    );
    advance(&mut chain, vec![fund], Vec::new()).unwrap();
    let slash = |first: SignedVote, second: SignedVote, nonce: u64| {
        SignedTx::sign(
            Tx::Slash {
                first: Box::new(first),
                second: Box::new(second),
            },
            nonce,
            &reporter,
        )
    };
    let receipts = advance(&mut chain, vec![slash(honest, double, 0), slash(honest, double, 1)], Vec::new()).unwrap();

    assert_eq!(receipts[0].result, Ok(Effect::Done));
    assert!(matches!(&receipts[1].result, Err(failure) if failure.to_string().contains("slashed already")));
    assert_eq!(balance(&chain, &reporter.public().address()), u256::new(1) + u256::new(BOND.into()) / u256::new(4));

    let record = Validators::new(&chain.state()).get(&carol.public()).unwrap().unwrap();

    assert!(record.slashed);
    assert_eq!(record.bond, u256::ZERO);

    // Not two conflicting votes: refused before it runs.
    assert!(matches!(
        advance(&mut chain, vec![slash(honest, honest, 2)], Vec::new()),
        Err(ImportError::Invalid(BlockError::Tx { error: TxError::Invalid(_), .. }))
    ));
}

#[test]
fn validators_join_and_leave_by_transaction() {
    let mut chain = open();
    let owner = authority();
    let newcomer = key(60);
    let tx = |tx: Tx, chain: &Chain| SignedTx::sign(tx, nonce(chain, &owner), &owner);
    let funds = balance(&chain, &owner.public().address());
    let small = tx(Tx::Bond { voter: newcomer.public(), amount: Amount::U128(99) }, &chain);
    let receipts = advance(&mut chain, vec![small], Vec::new()).unwrap();

    assert!(matches!(&receipts[0].result, Err(failure) if failure.to_string().contains("at least 100")));

    let bond = tx(Tx::Bond { voter: newcomer.public(), amount: Amount::U128(500) }, &chain);
    advance(&mut chain, vec![bond], Vec::new()).unwrap();
    let record = Validators::new(&chain.state()).get(&newcomer.public()).unwrap().unwrap();

    // Bonded in epoch 0 (height 2): votes from epoch 0 + 2 + 1.
    assert_eq!((record.account, record.bond, record.active_from), (owner.public().address(), u256::new(500), 3));
    assert_eq!(balance(&chain, &owner.public().address()), funds - u256::new(500));

    let in_set = |chain: &Chain, epoch: u64| {
        Validators::new(&chain.state())
            .set(epoch)
            .unwrap()
            .is_some_and(|set| set.member(&newcomer.public()).is_some())
    };
    grow_to(&mut chain, 3 * INTERVAL);

    assert!(!in_set(&chain, 2));
    assert!(in_set(&chain, 3));

    let unbond = tx(Tx::Unbond { voter: newcomer.public() }, &chain);
    advance(&mut chain, vec![unbond], Vec::new()).unwrap();
    let record = Validators::new(&chain.state()).get(&newcomer.public()).unwrap().unwrap();

    // Unbonded in epoch 3: stops after epoch 5, unlocks in epoch 9.
    assert_eq!((record.active_until, record.withdrawable_from), (Some(6), Some(9)));

    let early = tx(Tx::Withdraw { voter: newcomer.public() }, &chain);
    let receipts = advance(&mut chain, vec![early], Vec::new()).unwrap();

    assert!(matches!(&receipts[0].result, Err(failure) if failure.to_string().contains("unlocks in epoch 9")));

    grow_to(&mut chain, 9 * INTERVAL);
    let withdraw = tx(Tx::Withdraw { voter: newcomer.public() }, &chain);
    advance(&mut chain, vec![withdraw], Vec::new()).unwrap();

    assert_eq!(balance(&chain, &owner.public().address()), funds);
    assert_eq!(Validators::new(&chain.state()).get(&newcomer.public()).unwrap(), None);
    assert!(!in_set(&chain, 10));
}

#[test]
fn staking_needs_a_chain_with_finality() {
    let bare = Genesis {
        finality: None,
        ..genesis()
    };
    let mut chain = Chain::open(Arc::new(MemoryDb::default()), &bare).unwrap();
    let bond = SignedTx::sign(Tx::Bond { voter: key(60).public(), amount: Amount::U128(500) }, 0, &authority());
    let receipts = advance(&mut chain, vec![bond], Vec::new()).unwrap();

    assert_eq!(receipts[0].result, Err(Failure::NoFinality));
    assert_eq!((chain.justified(), chain.finalized()), (None, None));
    assert!(matches!(chain.add_vote(honest_vote_on(&chain)), Err(FinalityError::Disabled)));
}

/// Any well-formed vote, for chains where nothing counts it.
fn honest_vote_on(chain: &Chain) -> SignedVote {
    let keypair = &validators()[0];
    let vote = Vote {
        source: start(chain),
        target: Checkpoint {
            epoch: 1,
            block: Hash::zero(chain.hash()),
        },
        voter: keypair.public(),
    };

    SignedVote::sign(vote, keypair, &chain.genesis())
}

#[test]
fn proofs_in_blocks_pay_every_voter_once() {
    let mut chain = open();
    finalize_through(&mut chain, 1);
    let proofs = chain.unpaid_proofs().unwrap();
    let [alice, bob, carol] = validators();
    let before: Vec<u256> = [&alice, &bob, &carol].iter().map(|keypair| balance(&chain, &keypair.public().address())).collect();

    assert_eq!(proofs.len(), 1);

    advance(&mut chain, Vec::new(), proofs.clone()).unwrap();

    let reward = u256::new(VOTE_REWARD.into());
    let voted: Vec<PublicKey> = proofs[0].votes.iter().map(|vote| vote.vote.voter).collect();

    for (keypair, before) in [&alice, &bob, &carol].iter().zip(before) {
        let expected = if voted.contains(&keypair.public()) { before + reward } else { before };

        assert_eq!(balance(&chain, &keypair.public().address()), expected);
    }

    assert!(chain.unpaid_proofs().unwrap().is_empty());
    assert!(matches!(
        advance(&mut chain, Vec::new(), proofs.clone()),
        Err(ImportError::Invalid(BlockError::Proof(why))) if why.contains("paid already")
    ));

    // A proof without two thirds behind it.
    grow_to(&mut chain, 2 * INTERVAL + 1);
    let mut thin = Justification {
        source: checkpoint(&chain, 1),
        target: checkpoint(&chain, 2),
        votes: vec![honest_vote(&chain, &alice)],
    };

    assert!(matches!(
        advance(&mut chain, Vec::new(), vec![thin.clone()]),
        Err(ImportError::Invalid(BlockError::Proof(why))) if why.contains("under two thirds")
    ));

    thin.votes.push(honest_vote(&chain, &bob));
    thin.votes[1].signature = thin.votes[0].signature;
    thin.votes.sort_by_key(|vote| vote.vote.voter);

    assert!(matches!(
        advance(&mut chain, Vec::new(), vec![thin]),
        Err(ImportError::Invalid(BlockError::Proof(why))) if why.contains("bad vote")
    ));
}

#[test]
fn a_peer_s_justification_is_checked_before_it_counts() {
    let db = Arc::new(MemoryDb::default());
    let mut voted = Chain::open(db, &genesis()).unwrap();
    finalize_through(&mut voted, 2);
    let mut follower = open();

    for height in 1..=voted.tip().height {
        let block = voted.block_at(height).unwrap().unwrap();
        follower.import(&block, LATER).unwrap();
    }

    let proofs = voted.justifications_from(1, 10).unwrap();

    assert_eq!(proofs.len(), 2);

    // Out of order: epoch 2's source is not justified here yet.
    assert!(matches!(follower.add_justification(proofs[1].clone()), Err(FinalityError::Unverifiable(_))));

    let mut short = proofs[0].clone();
    short.votes.pop();

    assert!(matches!(follower.add_justification(short), Err(FinalityError::Unverifiable(why)) if why.contains("two thirds")));

    for proof in proofs {
        follower.add_justification(proof).unwrap();
    }

    assert_eq!(follower.finalized(), voted.finalized());
    assert_eq!(follower.justified(), voted.justified());
}

// Proof of work with finality: fork choice follows work only where finality allows.

fn pow_genesis() -> Genesis {
    let hash = HashType::default_chain();

    Genesis {
        consensus: crate::consensus::ConsensusConfig::ProofOfWork(crate::consensus::PowConfig {
            hash,
            target_block_ms: 1_000,
            initial_difficulty: 1,
            min_difficulty: 1,
        }),
        reward: Reward::NONE,
        finality: Some(config()),
        ..Genesis::dev_pow(authority().public())
    }
}

fn mine_on(chain: &Chain, parent: &Block, miner: u8) -> Block {
    let timestamp = parent.header.timestamp + 1_000;
    let header = BlockHeader {
        parent: parent.id(),
        height: parent.header.height + 1,
        timestamp,
        producer: Address::Ed25519([miner; 32]),
        difficulty: chain.consensus().difficulty(&parent.header, timestamp),
        tx_root: Block::tx_root(chain.hash(), &[]),
        proof_root: Block::proof_root(chain.hash(), &[]),
    };
    let nonce = (0..)
        .find(|nonce| meets(work_hash(chain.hash(), &header.id(), *nonce), header.difficulty))
        .unwrap();

    Block {
        header,
        txs: Vec::new(),
        proofs: Vec::new(),
        seal: Seal::Work { nonce },
    }
}

/// Mines count blocks on top of from, importing each, and returns the last.
fn mine_branch(chain: &mut Chain, from: &Block, count: u64, miner: u8) -> Block {
    let mut tip = from.clone();

    for _ in 0..count {
        tip = mine_on(chain, &tip, miner);
        let _ = chain.import(&tip, LATER).unwrap();
    }

    tip
}

#[test]
fn heavier_branches_never_undo_finality_or_skip_the_justified_checkpoint() {
    let mut chain = Chain::open(Arc::new(MemoryDb::default()), &pow_genesis()).unwrap();
    let genesis = chain.block_at(0).unwrap().unwrap();
    let main = mine_branch(&mut chain, &genesis, 2 * INTERVAL + 1, 1);

    for epoch in 1..=2 {
        for keypair in &validators()[..2] {
            let vote = SignedVote::sign(
                Vote {
                    source: if epoch == 1 { start(&chain) } else { checkpoint(&chain, 1) },
                    target: checkpoint(&chain, epoch),
                    voter: keypair.public(),
                },
                keypair,
                &chain.genesis(),
            );
            chain.add_vote(vote).unwrap();
        }
    }

    assert_eq!(chain.finalized(), Some(checkpoint(&chain, 1)));
    assert_eq!(chain.justified(), Some(checkpoint(&chain, 2)));

    // A sibling at or below the finalized checkpoint is refused outright.
    let below = mine_on(&chain, &genesis, 2);

    assert!(matches!(chain.import(&below, LATER), Err(ImportError::Invalid(BlockError::ConflictsWithFinality))));

    // A much heavier branch from between the finalized and justified checkpoints is kept aside.
    let fork = chain.block_at(INTERVAL + 1).unwrap().unwrap();
    let heavy = mine_branch(&mut chain, &fork, 3 * INTERVAL, 3);

    assert_eq!(chain.tip().id(), main.id());
    assert!(chain.tip_work() < super::work_of(&*chain.db, &heavy.id()).unwrap());

    // From the justified checkpoint on, work decides again.
    let at_justified = chain.block_at(2 * INTERVAL).unwrap().unwrap();
    let heavier = mine_branch(&mut chain, &at_justified, 4, 4);

    assert_eq!(chain.tip().id(), heavier.id());
}

#[test]
fn a_justified_branch_wins_over_more_work() {
    let mut voters = Chain::open(Arc::new(MemoryDb::default()), &pow_genesis()).unwrap();
    let mut follower = Chain::open(Arc::new(MemoryDb::default()), &pow_genesis()).unwrap();
    let genesis = voters.block_at(0).unwrap().unwrap();
    // The voters' branch, which they justify at epoch 1.
    let voted = mine_branch(&mut voters, &genesis, INTERVAL + 1, 1);

    for keypair in &validators()[..2] {
        let vote = SignedVote::sign(
            Vote {
                source: start(&voters),
                target: checkpoint(&voters, 1),
                voter: keypair.public(),
            },
            keypair,
            &voters.genesis(),
        );
        voters.add_vote(vote).unwrap();
    }

    // The follower sits on a heavier branch, and has the voters' blocks only as a side branch.
    let heavier = mine_branch(&mut follower, &genesis, INTERVAL + 4, 2);

    for height in 1..=voted.header.height {
        let _ = follower.import(&voters.block_at(height).unwrap().unwrap(), LATER).unwrap();
    }

    assert_eq!(follower.tip().id(), heavier.id());

    let proof = voters.justifications_from(1, 1).unwrap().remove(0);
    let update = follower.add_justification(proof).unwrap();
    let (undone, applied) = update.reorganized.expect("the justification moves the chain");

    assert_eq!(follower.tip().id(), voted.id());
    assert_eq!(undone.len() as u64, INTERVAL + 4);
    assert_eq!(applied.len() as u64, INTERVAL + 1);
    assert_eq!(follower.justified(), voters.justified());
}
