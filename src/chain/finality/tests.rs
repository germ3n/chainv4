//! Finality on a chain, for every kind this build has. The vote-based kinds, ffg and authority,
//! run the same tests set up alike: three voters, two of whom are enough. Then what only one kind
//! does: staking and slashing under FFG, thresholds under authority, and depth finality.

use super::*;
use crate::chain::*;
use crate::consensus::{AuthorityProducer, ConsensusConfig, DevAuthority, PowConfig, Producer, meets, work_hash};
use crate::crypto::keypair::Keypair;
use crate::db::MemoryDb;
use crate::structs::address::Address;
use crate::structs::block::Seal;
use crate::structs::tx::FeeOffer;

const SLOT: u64 = 2_000;
/// Far enough ahead that no block in these tests is from the future.
const LATER: u64 = 10_000_000_000;

/// The development key, which produces every block on the authority chains here.
fn dev() -> Keypair {
    DevAuthority::dev_keypair()
}

fn key(seed: u8) -> Keypair {
    Keypair::from_seed([seed; 32])
}

fn genesis_with(finality: Option<FinalityConfig>) -> Genesis {
    Genesis {
        finality,
        fees: None,
        ..Genesis::dev_authority(dev().public())
    }
}

fn open_with(finality: Option<FinalityConfig>) -> Chain {
    Chain::open(Arc::new(MemoryDb::default()), &genesis_with(finality)).unwrap()
}

/// Builds, seals and imports the next block with these transactions and proofs.
fn advance(chain: &mut Chain, txs: Vec<SignedTx>, proofs: Vec<Justification>) -> Result<Vec<Receipt>, ImportError> {
    let now = chain.tip().timestamp + SLOT;
    let mut producer = AuthorityProducer::new(dev(), SLOT);
    let mut block = chain.build(txs, proofs, producer.address(), now).unwrap();
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

/// A proof-of-work chain where any nonce does, so tests mine instantly.
fn pow_genesis(finality: FinalityConfig) -> Genesis {
    Genesis {
        consensus: ConsensusConfig::ProofOfWork(PowConfig {
            hash: HashType::default_chain(),
            target_block_ms: 1_000,
            initial_difficulty: 1,
            min_difficulty: 1,
        }),
        reward: Reward::NONE,
        finality: Some(finality),
        fees: None,
        ..Genesis::dev_pow(dev().public())
    }
}

fn mine_on(chain: &Chain, parent: &Block, miner: u8) -> Block {
    let timestamp = parent.header.timestamp + 1_000;
    let mut block = chain.build_on(&parent.id(), Vec::new(), Vec::new(), Address::Ed25519([miner; 32]), timestamp).unwrap();
    let nonce = (0..)
        .find(|nonce| meets(work_hash(chain.hash(), &block.id(), *nonce), block.header.difficulty))
        .unwrap();
    block.seal = Seal::Work { nonce };

    block
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

#[cfg(any(feature = "finality-ffg", feature = "finality-authority"))]
mod voting {
    use super::*;
    use crate::chain::finality::votes::{COUNTED, HELD, MAX_HISTORY, stored_votes};
    use crate::structs::tx::Tx;

    /// Blocks per epoch in these chains.
    pub(super) const INTERVAL: u64 = 4;
    /// What every voter in a justification earns here.
    pub(super) const VOTE_REWARD: u64 = 7;

    /// The three voters, one key each.
    pub(super) fn voters() -> [Keypair; 3] {
        [key(11), key(12), key(13)]
    }

    /// FFG with the three voters bonded equally, so two of them make two thirds.
    #[cfg(feature = "finality-ffg")]
    pub(super) fn ffg() -> FinalityConfig {
        FinalityConfig::Ffg(FfgConfig {
            checkpoint_interval: INTERVAL,
            vote_depth: 1,
            min_bond: u256::new(100),
            activation_delay: 2,
            withdrawal_delay: 3,
            reporter_divisor: 4,
            max_validators: 10,
            vote_reward: u256::new(VOTE_REWARD.into()),
            max_proofs_per_block: 4,
            validators: voters()
                .iter()
                .map(|keypair| GenesisValidator {
                    account: keypair.public().address(),
                    voter: keypair.public(),
                    bond: u256::new(10),
                })
                .collect(),
        })
    }

    /// The three voters as signers, two of them justifying and two changing the set.
    #[cfg(feature = "finality-authority")]
    pub(super) fn signers() -> AuthorityConfig {
        AuthorityConfig {
            checkpoint_interval: INTERVAL,
            vote_depth: 1,
            activation_delay: 2,
            max_signers: 10,
            signers: voters().iter().map(Keypair::public).collect(),
            threshold: 2,
            change_threshold: 2,
            approval_epochs: 64,
            vote_reward: u256::new(VOTE_REWARD.into()),
            max_proofs_per_block: 4,
        }
    }

    #[cfg(feature = "finality-authority")]
    pub(super) fn authority() -> FinalityConfig {
        FinalityConfig::Authority(signers())
    }

    /// Every vote-based kind this build has.
    pub(super) fn kinds() -> Vec<FinalityConfig> {
        [
            #[cfg(feature = "finality-ffg")]
            ffg(),
            #[cfg(feature = "finality-authority")]
            authority(),
        ]
        .into()
    }

    pub(super) fn open(finality: &FinalityConfig) -> Chain {
        open_with(Some(finality.clone()))
    }

    /// The vote an honest validator signs now, remembered so it is never contradicted.
    pub(super) fn honest_vote(chain: &Chain, keypair: &Keypair) -> SignedVote {
        let vote = chain.vote_for(&keypair.public()).unwrap().expect("a vote is due");
        chain.record_own_vote(&vote).unwrap();

        SignedVote::sign(vote, keypair, &chain.genesis())
    }

    pub(super) fn signed(chain: &Chain, keypair: &Keypair, source: Checkpoint, target: Checkpoint) -> SignedVote {
        let vote = Vote {
            source,
            target,
            voter: keypair.public(),
        };

        SignedVote::sign(vote, keypair, &chain.genesis())
    }

    pub(super) fn checkpoint(chain: &Chain, epoch: u64) -> Checkpoint {
        Checkpoint {
            epoch,
            block: chain.block_at(epoch * INTERVAL).unwrap().unwrap().id(),
        }
    }

    pub(super) fn start(chain: &Chain) -> Checkpoint {
        Checkpoint {
            epoch: 0,
            block: chain.genesis(),
        }
    }

    /// Justifies the checkpoint of every epoch up to last, two voters voting each time.
    pub(super) fn finalize_through(chain: &mut Chain, last: u64) {
        for epoch in chain.justified().unwrap().epoch + 1..=last {
            grow_to(chain, epoch * INTERVAL + 1);

            for keypair in &voters()[..2] {
                let vote = honest_vote(chain, keypair);
                chain.add_vote(vote).unwrap();
            }
        }
    }

    pub(super) fn balance(chain: &Chain, address: &Address) -> u256 {
        Accounts::new(&chain.state()).get(address).unwrap().balance
    }

    pub(super) fn nonce(chain: &Chain, keypair: &Keypair) -> u64 {
        Accounts::new(&chain.state()).get(&keypair.public().address()).unwrap().nonce
    }

    /// Sends a transaction from the dev key, which holds the funds, in the next block.
    pub(super) fn send(chain: &mut Chain, tx: Tx) -> Vec<Receipt> {
        let signed = SignedTx::sign(tx, nonce(chain, &dev()), FeeOffer::NONE, &dev());

        advance(chain, vec![signed], Vec::new()).unwrap()
    }

    #[test]
    fn enough_votes_justify_and_the_next_checkpoint_finalizes() {
        for finality in kinds() {
            let mut chain = open(&finality);
            grow_to(&mut chain, INTERVAL + 1);
            let [alice, bob, carol] = voters();
            let first = honest_vote(&chain, &alice);

            assert_eq!(first.vote.source, start(&chain));
            assert_eq!(first.vote.target, checkpoint(&chain, 1));

            let one = chain.add_vote(first).unwrap();

            assert!(one.justified.is_empty(), "{}: one of three is not enough", finality.name());

            let two = chain.add_vote(honest_vote(&chain, &bob)).unwrap();

            assert_eq!(two.justified, [checkpoint(&chain, 1)]);
            assert_eq!(chain.justified(), Some(checkpoint(&chain, 1)));
            // Genesis is the source and already final; nothing new is.
            assert_eq!(two.finalized, None);

            // Nothing is left for the third to vote on, and a late vote for the same link changes
            // nothing.
            assert_eq!(chain.vote_for(&carol.public()).unwrap(), None);

            let late = signed(&chain, &carol, start(&chain), checkpoint(&chain, 1));

            assert!(chain.add_vote(late).unwrap().justified.is_empty());

            grow_to(&mut chain, 2 * INTERVAL + 1);
            chain.add_vote(honest_vote(&chain, &alice)).unwrap();
            let finalizing = chain.add_vote(honest_vote(&chain, &bob)).unwrap();

            assert_eq!(finalizing.justified, [checkpoint(&chain, 2)]);
            assert_eq!(finalizing.finalized, Some(checkpoint(&chain, 1)));
            assert_eq!(chain.finalized(), Some(checkpoint(&chain, 1)));
            assert_eq!(
                stored_finality(&*chain.db).unwrap(),
                (Some(checkpoint(&chain, 2)), Some(checkpoint(&chain, 1)))
            );

            let proof = chain.justification(&checkpoint(&chain, 2).block).unwrap().unwrap();

            assert_eq!(proof.source, checkpoint(&chain, 1));
            assert_eq!(proof.votes.len(), 2);
            assert!(proof.votes.windows(2).all(|pair| pair[0].vote.voter < pair[1].vote.voter));
        }
    }

    #[test]
    fn finality_survives_a_restart() {
        for finality in kinds() {
            let db = Arc::new(MemoryDb::default());
            let genesis = genesis_with(Some(finality));
            let mut chain = Chain::open(db.clone(), &genesis).unwrap();
            finalize_through(&mut chain, 2);
            let (justified, finalized) = (chain.justified(), chain.finalized());
            let reopened = Chain::open(db, &genesis).unwrap();

            assert_eq!((reopened.justified(), reopened.finalized()), (justified, finalized));
            assert_eq!(finalized.unwrap().epoch, 1);
        }
    }

    /// The chain in a database, opened again as a restarted node would.
    fn reopen(db: &Arc<MemoryDb>, finality: &FinalityConfig) -> Chain {
        Chain::open(db.clone(), &genesis_with(Some(finality.clone()))).unwrap()
    }

    #[test]
    fn a_tally_survives_a_restart() {
        for finality in kinds() {
            let db = Arc::new(MemoryDb::default());
            let mut chain = reopen(&db, &finality);
            grow_to(&mut chain, INTERVAL + 1);
            let [alice, bob, _] = voters();

            assert!(chain.add_vote(honest_vote(&chain, &alice)).unwrap().justified.is_empty());

            // Alice's vote is counted again on opening, so Bob's makes two.
            let mut chain = reopen(&db, &finality);
            let update = chain.add_vote(honest_vote(&chain, &bob)).unwrap();

            assert_eq!(update.justified, [checkpoint(&chain, 1)], "{}", finality.name());
            assert_eq!(chain.justification(&checkpoint(&chain, 1).block).unwrap().unwrap().votes.len(), 2);
        }
    }

    #[test]
    fn held_votes_survive_a_restart() {
        for finality in kinds() {
            let db = Arc::new(MemoryDb::default());
            let mut chain = reopen(&db, &finality);
            grow_to(&mut chain, 2 * INTERVAL + 1);
            let [alice, bob, _] = voters();

            for keypair in [&alice, &bob] {
                let early = signed(&chain, keypair, checkpoint(&chain, 1), checkpoint(&chain, 2));

                assert!(chain.add_vote(early).unwrap().pending);
            }

            let mut chain = reopen(&db, &finality);
            chain.add_vote(signed(&chain, &alice, start(&chain), checkpoint(&chain, 1))).unwrap();
            let update = chain.add_vote(signed(&chain, &bob, start(&chain), checkpoint(&chain, 1))).unwrap();

            assert_eq!(update.justified, [checkpoint(&chain, 1), checkpoint(&chain, 2)]);
            assert_eq!(update.finalized, Some(checkpoint(&chain, 1)));

            // Counted now, so no longer held: another restart holds nothing back.
            assert!(stored_votes(&*db, HELD, 0).unwrap().is_empty());
            assert_eq!(stored_votes(&*db, COUNTED, 0).unwrap().len(), 4);
        }
    }

    #[test]
    fn cheating_is_caught_across_a_restart() {
        for finality in kinds() {
            let db = Arc::new(MemoryDb::default());
            let mut chain = reopen(&db, &finality);
            grow_to(&mut chain, 3 * INTERVAL + 1);
            let [alice, _, carol] = voters();
            let other_target = Checkpoint {
                epoch: 2,
                block: Hash::zero(chain.hash()),
            };
            chain.add_vote(signed(&chain, &carol, start(&chain), checkpoint(&chain, 2))).unwrap();
            chain.add_vote(signed(&chain, &alice, checkpoint(&chain, 1), checkpoint(&chain, 2))).unwrap();

            let mut chain = reopen(&db, &finality);
            let double = signed(&chain, &carol, start(&chain), other_target);
            let surround = signed(&chain, &alice, start(&chain), checkpoint(&chain, 3));

            assert!(chain.add_vote(double).unwrap().evidence.is_some(), "{}: a double vote", finality.name());
            assert!(chain.add_vote(surround).unwrap().evidence.is_some(), "{}: a surround vote", finality.name());
        }
    }

    #[test]
    fn counted_votes_go_once_they_are_long_final() {
        let finality = kinds().remove(0);
        let db = Arc::new(MemoryDb::default());
        let mut chain = reopen(&db, &finality);
        let last = MAX_HISTORY as u64 + 3;
        finalize_through(&mut chain, last);
        let finalized = chain.finalized().unwrap().epoch;
        let kept = stored_votes(&*db, COUNTED, 0).unwrap();

        assert_eq!(finalized, last - 1);
        assert!(kept.iter().all(|vote| vote.vote.target.epoch > finalized - MAX_HISTORY as u64));
        assert_eq!(kept.len(), 2 * (MAX_HISTORY + 1), "two votes for each epoch kept");
        assert_eq!(reopen(&db, &finality).finalized(), chain.finalized());
    }

    #[test]
    fn votes_that_cannot_count_yet_wait_and_count_later() {
        for finality in kinds() {
            let mut chain = open(&finality);
            grow_to(&mut chain, 2 * INTERVAL + 1);
            let [alice, bob, _] = voters();

            // Votes from epoch 1 to epoch 2, before epoch 1 is justified.
            for keypair in [&alice, &bob] {
                let early = signed(&chain, keypair, checkpoint(&chain, 1), checkpoint(&chain, 2));

                assert!(chain.add_vote(early).unwrap().pending);
            }

            // Justifying epoch 1 lets the held votes count, which justifies 2 and finalizes 1.
            chain.add_vote(signed(&chain, &alice, start(&chain), checkpoint(&chain, 1))).unwrap();
            let update = chain.add_vote(signed(&chain, &bob, start(&chain), checkpoint(&chain, 1))).unwrap();

            assert_eq!(update.justified, [checkpoint(&chain, 1), checkpoint(&chain, 2)]);
            assert_eq!(update.finalized, Some(checkpoint(&chain, 1)));
        }
    }

    #[test]
    fn bad_votes_are_refused() {
        for finality in kinds() {
            let mut chain = open(&finality);
            grow_to(&mut chain, INTERVAL + 1);
            let outsider = key(99);
            let vote = honest_vote(&chain, &voters()[0]);
            let forged = SignedVote {
                vote: Vote {
                    voter: outsider.public(),
                    ..vote.vote
                },
                signature: vote.signature,
            };
            let not_voter = Vote {
                voter: outsider.public(),
                ..vote.vote
            };

            assert!(matches!(chain.add_vote(forged), Err(FinalityError::Invalid(_))));
            assert!(matches!(
                chain.add_vote(SignedVote::sign(not_voter, &outsider, &chain.genesis())),
                Err(FinalityError::NotValidator)
            ));

            // Signed for another chain.
            let elsewhere = SignedVote::sign(vote.vote, &voters()[0], &Hash::zero(chain.hash()));

            assert!(matches!(chain.add_vote(elsewhere), Err(FinalityError::Invalid(_))));
        }
    }

    #[test]
    fn an_honest_validator_never_contradicts_itself() {
        for finality in kinds() {
            let mut chain = open(&finality);
            grow_to(&mut chain, INTERVAL + 1);
            let alice = &voters()[0];
            honest_vote(&chain, alice);

            // Already voted for epoch 1.
            assert_eq!(chain.vote_for(&alice.public()).unwrap(), None);

            // Once it has voted from epoch 1, a vote from an older source could surround that one.
            let from_one = Vote {
                source: checkpoint(&chain, 1),
                target: checkpoint(&chain, 1),
                voter: alice.public(),
            };
            chain.record_own_vote(&from_one).unwrap();
            grow_to(&mut chain, 3 * INTERVAL + 1);

            assert_eq!(chain.vote_for(&alice.public()).unwrap(), None);
            assert_eq!(chain.vote_for(&key(99).public()).unwrap(), None, "not a voter");
            assert!(chain.vote_for(&voters()[1].public()).unwrap().is_some());
        }
    }

    #[test]
    fn conflicting_votes_are_evidence() {
        for finality in kinds() {
            let mut chain = open(&finality);
            grow_to(&mut chain, 2 * INTERVAL + 1);
            let [alice, _, carol] = voters();
            let honest = signed(&chain, &carol, start(&chain), checkpoint(&chain, 2));
            let other_target = Checkpoint {
                epoch: 2,
                block: Hash::zero(chain.hash()),
            };
            let double = signed(&chain, &carol, start(&chain), other_target);

            chain.add_vote(honest).unwrap();
            let caught = chain.add_vote(double).unwrap().evidence.expect("a double vote is evidence");

            assert_eq!((caught.first, caught.second), (honest, double));
            assert_eq!(stored_evidence(&*chain.db).unwrap(), [caught]);

            // Surrounding: from genesis to epoch 3 around a vote from 1 to 2.
            let inner = signed(&chain, &alice, checkpoint(&chain, 1), checkpoint(&chain, 2));
            grow_to(&mut chain, 3 * INTERVAL + 1);
            let outer = signed(&chain, &alice, start(&chain), checkpoint(&chain, 3));
            chain.add_vote(inner).unwrap();

            assert!(chain.add_vote(outer).unwrap().evidence.is_some());
            assert_eq!(stored_evidence(&*chain.db).unwrap().len(), 2);
        }
    }

    #[test]
    fn keys_that_do_not_vote_leave_nothing_behind() {
        for finality in kinds() {
            let mut chain = open(&finality);
            grow_to(&mut chain, 2 * INTERVAL + 1);
            let nowhere = Checkpoint {
                epoch: 2,
                block: Hash::zero(chain.hash()),
            };

            for seed in 100..120 {
                let stranger = key(seed);
                let placeable = signed(&chain, &stranger, start(&chain), checkpoint(&chain, 1));
                let double = signed(&chain, &stranger, start(&chain), Checkpoint {
                    epoch: 1,
                    block: Hash::zero(chain.hash()),
                });

                // Refused, and not remembered: its second, conflicting vote finds nothing to
                // conflict with.
                assert!(matches!(chain.add_vote(placeable), Err(FinalityError::NotValidator)));
                assert!(!matches!(chain.add_vote(double), Ok(FinalityUpdate { evidence: Some(_), .. })));

                // Votes nobody can place yet are held, both of a pair, and neither is evidence.
                let held = signed(&chain, &stranger, checkpoint(&chain, 1), checkpoint(&chain, 2));
                let rival = signed(&chain, &stranger, checkpoint(&chain, 1), nowhere);

                assert!(chain.add_vote(held).unwrap().pending);
                assert!(chain.add_vote(rival).unwrap().pending);
            }

            assert!(stored_evidence(&*chain.db).unwrap().is_empty(), "{}", finality.name());

            // Once epoch 1 is justified the held votes are tried: those that can be placed now
            // turn out to be no voter's and go for good, and only those for blocks nobody has
            // wait on, held votes being capped.
            let [alice, bob, _] = voters();
            chain.add_vote(signed(&chain, &alice, start(&chain), checkpoint(&chain, 1))).unwrap();
            chain.add_vote(signed(&chain, &bob, start(&chain), checkpoint(&chain, 1))).unwrap();

            assert_eq!(chain.justified(), Some(checkpoint(&chain, 1)));
            assert!(stored_evidence(&*chain.db).unwrap().is_empty());
            assert!(
                stored_votes(&*chain.db, HELD, 0)
                    .unwrap()
                    .iter()
                    .all(|vote| vote.vote.target.block == Hash::zero(chain.hash()))
            );
            assert_eq!(stored_votes(&*chain.db, HELD, 0).unwrap().len(), 40);
        }
    }

    #[test]
    fn a_voter_s_held_double_vote_is_evidence_once_one_counts() {
        for finality in kinds() {
            let mut chain = open(&finality);
            grow_to(&mut chain, 2 * INTERVAL + 1);
            let alice = &voters()[0];
            let real = signed(&chain, alice, checkpoint(&chain, 1), checkpoint(&chain, 2));
            let rival = signed(&chain, alice, checkpoint(&chain, 1), Checkpoint {
                epoch: 2,
                block: Hash::zero(chain.hash()),
            });

            // Nobody can tell yet that alice votes: both are held.
            assert!(chain.add_vote(real).unwrap().pending);
            assert!(chain.add_vote(rival).unwrap().pending);
            assert!(stored_evidence(&*chain.db).unwrap().is_empty());

            // Justifying epoch 1 counts the real one, which shows alice votes, and the rival is
            // evidence against her.
            let bob = &voters()[1];
            chain.add_vote(signed(&chain, alice, start(&chain), checkpoint(&chain, 1))).unwrap();
            let update = chain.add_vote(signed(&chain, bob, start(&chain), checkpoint(&chain, 1))).unwrap();
            let evidence = stored_evidence(&*chain.db).unwrap();

            assert!(update.evidence.is_some(), "{}", finality.name());
            assert_eq!(evidence.len(), 1);
            assert_eq!([evidence[0].first, evidence[0].second].map(|vote| vote.vote.voter), [alice.public(); 2]);
            assert!(stored_votes(&*chain.db, HELD, 0).unwrap().is_empty());
        }
    }

    #[test]
    fn proofs_in_blocks_pay_every_voter_once() {
        for finality in kinds() {
            let mut chain = open(&finality);
            finalize_through(&mut chain, 1);
            let proofs = chain.unpaid_proofs().unwrap();
            let all = voters();
            let before: Vec<u256> = all.iter().map(|keypair| balance(&chain, &keypair.public().address())).collect();

            assert_eq!(proofs.len(), 1);

            advance(&mut chain, Vec::new(), proofs.clone()).unwrap();

            let reward = u256::new(VOTE_REWARD.into());
            let voted: Vec<PublicKey> = proofs[0].votes.iter().map(|vote| vote.vote.voter).collect();

            for (keypair, before) in all.iter().zip(before) {
                let expected = if voted.contains(&keypair.public()) { before + reward } else { before };

                assert_eq!(balance(&chain, &keypair.public().address()), expected);
            }

            assert!(chain.unpaid_proofs().unwrap().is_empty());
            assert!(matches!(
                advance(&mut chain, Vec::new(), proofs.clone()),
                Err(ImportError::Invalid(BlockError::Proof(why))) if why.contains("paid already")
            ));

            // A proof without enough votes behind it.
            grow_to(&mut chain, 2 * INTERVAL + 1);
            let [alice, bob, _] = voters();
            let mut thin = Justification {
                source: checkpoint(&chain, 1),
                target: checkpoint(&chain, 2),
                votes: vec![honest_vote(&chain, &alice)],
            };

            assert!(matches!(
                advance(&mut chain, Vec::new(), vec![thin.clone()]),
                Err(ImportError::Invalid(BlockError::Proof(why))) if why.contains("falls short")
            ));

            thin.votes.push(honest_vote(&chain, &bob));
            thin.votes[1].signature = thin.votes[0].signature;
            thin.votes.sort_by_key(|vote| vote.vote.voter);

            assert!(matches!(
                advance(&mut chain, Vec::new(), vec![thin]),
                Err(ImportError::Invalid(BlockError::Proof(why))) if why.contains("bad vote")
            ));
        }
    }

    #[test]
    fn a_peer_s_justification_is_checked_before_it_counts() {
        for finality in kinds() {
            let mut voted = open(&finality);
            finalize_through(&mut voted, 2);
            let mut follower = open(&finality);

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

            assert!(matches!(
                follower.add_justification(short),
                Err(FinalityError::Unverifiable(why)) if why.contains("fall short")
            ));

            for proof in proofs {
                follower.add_justification(proof).unwrap();
            }

            assert_eq!(follower.finalized(), voted.finalized());
            assert_eq!(follower.justified(), voted.justified());
        }
    }

    #[test]
    fn heavier_branches_never_undo_finality_or_skip_the_justified_checkpoint() {
        for finality in kinds() {
            let mut chain = Chain::open(Arc::new(MemoryDb::default()), &pow_genesis(finality)).unwrap();
            let genesis = chain.block_at(0).unwrap().unwrap();
            let main = mine_branch(&mut chain, &genesis, 2 * INTERVAL + 1, 1);

            for epoch in 1..=2 {
                for keypair in &voters()[..2] {
                    let source = if epoch == 1 { start(&chain) } else { checkpoint(&chain, 1) };
                    let vote = signed(&chain, keypair, source, checkpoint(&chain, epoch));
                    chain.add_vote(vote).unwrap();
                }
            }

            assert_eq!(chain.finalized(), Some(checkpoint(&chain, 1)));
            assert_eq!(chain.justified(), Some(checkpoint(&chain, 2)));

            // A sibling at or below the finalized checkpoint is refused outright.
            let below = mine_on(&chain, &genesis, 2);

            assert!(matches!(
                chain.import(&below, LATER),
                Err(ImportError::Invalid(BlockError::ConflictsWithFinality))
            ));

            // A much heavier branch from between the finalized and justified checkpoints is kept
            // aside.
            let fork = chain.block_at(INTERVAL + 1).unwrap().unwrap();
            let heavy = mine_branch(&mut chain, &fork, 3 * INTERVAL, 3);

            assert_eq!(chain.tip().id(), main.id());
            assert!(chain.tip_work() < work_of(&*chain.db, &heavy.id()).unwrap());

            // From the justified checkpoint on, work decides again.
            let at_justified = chain.block_at(2 * INTERVAL).unwrap().unwrap();
            let heavier = mine_branch(&mut chain, &at_justified, 4, 4);

            assert_eq!(chain.tip().id(), heavier.id());
        }
    }

    #[test]
    fn a_justified_branch_wins_over_more_work() {
        for finality in kinds() {
            let genesis = pow_genesis(finality);
            let mut voters_chain = Chain::open(Arc::new(MemoryDb::default()), &genesis).unwrap();
            let mut follower = Chain::open(Arc::new(MemoryDb::default()), &genesis).unwrap();
            let first = voters_chain.block_at(0).unwrap().unwrap();
            // The voters' branch, which they justify at epoch 1.
            let voted = mine_branch(&mut voters_chain, &first, INTERVAL + 1, 1);

            for keypair in &voters()[..2] {
                let vote = signed(&voters_chain, keypair, start(&voters_chain), checkpoint(&voters_chain, 1));
                voters_chain.add_vote(vote).unwrap();
            }

            // The follower sits on a heavier branch, with the voters' blocks only as a side
            // branch.
            let heavier = mine_branch(&mut follower, &first, INTERVAL + 4, 2);

            for height in 1..=voted.header.height {
                let _ = follower.import(&voters_chain.block_at(height).unwrap().unwrap(), LATER).unwrap();
            }

            assert_eq!(follower.tip().id(), heavier.id());

            let proof = voters_chain.justifications_from(1, 1).unwrap().remove(0);
            let update = follower.add_justification(proof).unwrap();
            let (undone, applied) = update.reorganized.expect("the justification moves the chain");

            assert_eq!(follower.tip().id(), voted.id());
            assert_eq!(undone.len() as u64, INTERVAL + 4);
            assert_eq!(applied.len() as u64, INTERVAL + 1);
            assert_eq!(follower.justified(), voters_chain.justified());
        }
    }

    #[cfg(feature = "finality-ffg")]
    #[test]
    fn slashing_costs_the_bond_and_pays_the_reporter() {
        use crate::registry::validators::Validators;
        use crate::structs::amount::Amount;

        let mut chain = open(&ffg());
        grow_to(&mut chain, 2 * INTERVAL + 1);
        let carol = &voters()[2];
        let honest = signed(&chain, carol, start(&chain), checkpoint(&chain, 2));
        let other_target = Checkpoint {
            epoch: 2,
            block: Hash::zero(chain.hash()),
        };
        let double = signed(&chain, carol, start(&chain), other_target);

        // Anyone can report it; the reporter gets a quarter of the bond, and the rest is gone.
        let reporter = key(50);
        send(&mut chain, Tx::Transfer {
            from: dev().public().address(),
            to: reporter.public().address(),
            amount: Amount::U128(1),
        });
        let slash = |first: SignedVote, second: SignedVote, nonce: u64| {
            SignedTx::sign(
                Tx::Slash {
                    first: Box::new(first),
                    second: Box::new(second),
                },
                nonce,
                FeeOffer::NONE,
                &reporter,
            )
        };
        let receipts = advance(&mut chain, vec![slash(honest, double, 0), slash(honest, double, 1)], Vec::new()).unwrap();

        assert_eq!(receipts[0].result, Ok(Effect::Done));
        assert!(matches!(&receipts[1].result, Err(failure) if failure.to_string().contains("slashed already")));
        assert_eq!(balance(&chain, &reporter.public().address()), u256::new(1) + u256::new(10) / u256::new(4));

        let record = Validators::new(&chain.state()).get(&carol.public()).unwrap().unwrap();

        assert!(record.slashed);
        assert_eq!(record.bond, u256::ZERO);
        assert!(chain.has_staking());

        // Not two conflicting votes: refused before it runs, so a block holding it is invalid.
        let now = chain.tip().timestamp + SLOT;
        let mut producer = AuthorityProducer::new(dev(), SLOT);
        let mut block = chain.build(Vec::new(), Vec::new(), producer.address(), now).unwrap();
        block.txs = vec![slash(honest, honest, 2)];
        block.header.tx_root = Block::tx_root(chain.hash(), &block.txs);
        block.seal = producer.seal(&block.header).unwrap();

        assert!(matches!(
            chain.import(&block, now),
            Err(ImportError::Invalid(BlockError::Tx { error: TxError::Invalid(_), .. }))
        ));
    }

    #[cfg(feature = "finality-ffg")]
    #[test]
    fn validators_join_and_leave_by_transaction() {
        use crate::registry::validators::Validators;
        use crate::structs::amount::Amount;

        let mut chain = open(&ffg());
        let owner = dev();
        let newcomer = key(60);
        let funds = balance(&chain, &owner.public().address());
        let receipts = send(&mut chain, Tx::Bond {
            voter: newcomer.public(),
            amount: Amount::U128(99),
        });

        assert!(matches!(&receipts[0].result, Err(failure) if failure.to_string().contains("at least 100")));

        send(&mut chain, Tx::Bond {
            voter: newcomer.public(),
            amount: Amount::U128(500),
        });
        let record = Validators::new(&chain.state()).get(&newcomer.public()).unwrap().unwrap();

        // Bonded in epoch 0 (height 2): votes from epoch 0 + 2 + 1.
        assert_eq!(
            (record.account, record.bond, record.active_from),
            (owner.public().address(), u256::new(500), 3)
        );
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

        send(&mut chain, Tx::Unbond { voter: newcomer.public() });
        let record = Validators::new(&chain.state()).get(&newcomer.public()).unwrap().unwrap();

        // Unbonded in epoch 3: stops after epoch 5, unlocks in epoch 9.
        assert_eq!((record.active_until, record.withdrawable_from), (Some(6), Some(9)));

        let receipts = send(&mut chain, Tx::Withdraw { voter: newcomer.public() });

        assert!(matches!(&receipts[0].result, Err(failure) if failure.to_string().contains("unlocks in epoch 9")));

        grow_to(&mut chain, 9 * INTERVAL);
        send(&mut chain, Tx::Withdraw { voter: newcomer.public() });

        assert_eq!(balance(&chain, &owner.public().address()), funds);
        assert_eq!(Validators::new(&chain.state()).get(&newcomer.public()).unwrap(), None);
        assert!(!in_set(&chain, 10));
    }

    #[cfg(feature = "finality-authority")]
    #[test]
    fn the_threshold_decides_not_two_thirds() {
        let all = AuthorityConfig {
            threshold: 3,
            ..signers()
        };
        let mut chain = open(&FinalityConfig::Authority(all));
        grow_to(&mut chain, INTERVAL + 1);
        let [alice, bob, carol] = voters();
        chain.add_vote(honest_vote(&chain, &alice)).unwrap();

        assert!(chain.add_vote(honest_vote(&chain, &bob)).unwrap().justified.is_empty(), "two of three is not three");
        assert_eq!(chain.add_vote(honest_vote(&chain, &carol)).unwrap().justified, [checkpoint(&chain, 1)]);
    }

    #[cfg(feature = "finality-authority")]
    #[test]
    fn signers_cannot_stake_or_be_slashed() {
        use crate::structs::amount::Amount;

        let mut chain = open(&authority());
        grow_to(&mut chain, 2 * INTERVAL + 1);
        let carol = &voters()[2];
        let honest = signed(&chain, carol, start(&chain), checkpoint(&chain, 2));
        let other_target = Checkpoint {
            epoch: 2,
            block: Hash::zero(chain.hash()),
        };
        let double = signed(&chain, carol, start(&chain), other_target);
        let bond = Tx::Bond {
            voter: key(60).public(),
            amount: Amount::U128(500),
        };
        let slash = Tx::Slash {
            first: Box::new(honest),
            second: Box::new(double),
        };

        assert!(!chain.has_staking());

        for tx in [bond, slash, Tx::Unbond { voter: carol.public() }] {
            assert_eq!(send(&mut chain, tx)[0].result, Err(Failure::NoStaking));
        }
    }

    #[cfg(feature = "finality-authority")]
    #[test]
    fn unsafe_signer_settings_are_refused() {
        let good = signers();
        let signer = voters()[0].public();
        let cases = [
            (AuthorityConfig { threshold: 1, ..good.clone() }, "more than half"),
            (AuthorityConfig { threshold: 4, ..good.clone() }, "at most all"),
            (AuthorityConfig { signers: Vec::new(), ..good.clone() }, "at least one signer"),
            (AuthorityConfig { signers: vec![signer, signer], threshold: 2, ..good.clone() }, "named twice"),
            (AuthorityConfig { checkpoint_interval: 0, ..good.clone() }, "checkpoint_interval"),
            (AuthorityConfig { activation_delay: 0, ..good.clone() }, "activation_delay"),
            (AuthorityConfig { approval_epochs: 0, ..good.clone() }, "approval_epochs"),
            (AuthorityConfig { change_threshold: 1, ..good.clone() }, "change_threshold must"),
            (AuthorityConfig { max_signers: 2, ..good.clone() }, "max_signers"),
        ];

        for (config, expected) in cases {
            let genesis = genesis_with(Some(FinalityConfig::Authority(config)));

            match Chain::open(Arc::new(MemoryDb::default()), &genesis) {
                Err(ChainError::BadGenesis(why)) => assert!(why.contains(expected), "{why}"),
                Err(err) => panic!("expected {expected}, got {err}"),
                Ok(_) => panic!("expected {expected}, got a chain"),
            }
        }

        // An even split needs more than half: two of two, or three of four.
        let even = AuthorityConfig {
            signers: voters().iter().map(Keypair::public).chain([key(14).public()]).collect(),
            threshold: 3,
            change_threshold: 3,
            ..good
        };

        assert!(Chain::open(Arc::new(MemoryDb::default()), &genesis_with(Some(FinalityConfig::Authority(even)))).is_ok());
    }

    #[cfg(feature = "finality-authority")]
    #[test]
    fn signers_change_by_their_own_approval() {
        use crate::registry::signers::Signers;
        use crate::structs::tx::Tx;

        let mut chain = open(&authority());
        let [alice, bob, carol] = voters();
        let dave = key(14);
        let approve = |keypair: &Keypair, chain: &Chain| {
            let tx = Tx::ApproveSigners {
                version: 0,
                signers: vec![alice.public(), bob.public(), dave.public()],
                threshold: 2,
                change_threshold: 2,
            };

            SignedTx::sign(tx, nonce(chain, keypair), FeeOffer::NONE, keypair)
        };

        // Carol out, Dave in: one approval is not enough, and an outsider's does not count.
        let txs = vec![approve(&alice, &chain), approve(&dave, &chain)];
        let receipts = advance(&mut chain, txs, Vec::new()).unwrap();

        assert_eq!(receipts[0].result, Ok(Effect::Done));
        assert!(matches!(&receipts[1].result, Err(failure) if failure.to_string().contains("only a signer")));
        assert_eq!(Signers::new(&chain.state()).decided().unwrap().version, 0);

        let txs = vec![approve(&bob, &chain)];
        advance(&mut chain, txs, Vec::new()).unwrap();
        let decided = Signers::new(&chain.state()).decided().unwrap();

        assert_eq!(decided.version, 1);
        assert!(decided.set.signers.contains(&dave.public()) && !decided.set.signers.contains(&carol.public()));

        // Decided in epoch 0, fixed at epoch 1's checkpoint for epoch 1 + 2: until then the old
        // signers vote.
        finalize_through(&mut chain, 2);
        let fixed = |epoch: u64| Signers::new(&chain.state()).set(epoch).unwrap().unwrap();

        assert!(fixed(2).signers.contains(&carol.public()));
        assert!(fixed(3).signers.contains(&dave.public()));

        grow_to(&mut chain, 3 * INTERVAL + 1);

        assert_eq!(chain.vote_for(&carol.public()).unwrap(), None, "carol is no signer in epoch 3");

        let late = signed(&chain, &carol, checkpoint(&chain, 2), checkpoint(&chain, 3));

        assert!(matches!(chain.add_vote(late), Err(FinalityError::NotValidator)));

        chain.add_vote(honest_vote(&chain, &dave)).unwrap();
        let update = chain.add_vote(honest_vote(&chain, &alice)).unwrap();

        assert_eq!(update.justified, [checkpoint(&chain, 3)]);
        assert_eq!(update.finalized, Some(checkpoint(&chain, 2)));
    }

    #[cfg(feature = "finality-authority")]
    #[test]
    fn approvals_can_be_withdrawn_and_expire() {
        use crate::registry::signers::Signers;
        use crate::structs::tx::Tx;

        // Approvals count only in the epoch they are made in.
        let mut chain = open(&FinalityConfig::Authority(AuthorityConfig { approval_epochs: 1, ..signers() }));
        let [alice, bob, _] = voters();
        let approve = Tx::ApproveSigners {
            version: 0,
            signers: vec![alice.public(), bob.public(), key(14).public()],
            threshold: 2,
            change_threshold: 2,
        };
        let withdraw = Tx::WithdrawApproval { version: 0 };
        let send = |chain: &mut Chain, keypair: &Keypair, tx: &Tx| {
            let signed = SignedTx::sign(tx.clone(), nonce(chain, keypair), FeeOffer::NONE, keypair);

            advance(chain, vec![signed], Vec::new()).unwrap().remove(0).result
        };
        let version = |chain: &Chain| Signers::new(&chain.state()).decided().unwrap().version;

        // Heights 1 to 3, epoch 0: Alice approves and takes it back, and has nothing left to take.
        assert_eq!(send(&mut chain, &alice, &approve), Ok(Effect::Done));
        assert_eq!(send(&mut chain, &alice, &withdraw), Ok(Effect::Done));
        assert!(matches!(send(&mut chain, &alice, &withdraw), Err(failure) if failure.to_string().contains("no approval")));

        // Height 4, epoch 1: she approves again, but it has expired by Bob's approval at height 8,
        // epoch 2, so his decides nothing.
        assert_eq!(send(&mut chain, &alice, &approve), Ok(Effect::Done));
        grow_to(&mut chain, 2 * INTERVAL - 1);
        assert_eq!(send(&mut chain, &bob, &approve), Ok(Effect::Done));
        assert_eq!(version(&chain), 0);

        // Approving again in epoch 2 counts alongside Bob's, and decides it.
        assert_eq!(send(&mut chain, &alice, &approve), Ok(Effect::Done));
        assert_eq!(version(&chain), 1);
    }

    #[cfg(feature = "finality-authority")]
    #[test]
    fn without_rewards_blocks_carry_no_proofs() {
        let unpaid = AuthorityConfig {
            vote_reward: u256::ZERO,
            ..signers()
        };
        let mut chain = open(&FinalityConfig::Authority(unpaid));
        finalize_through(&mut chain, 2);

        assert_eq!(chain.finalized().unwrap().epoch, 1);
        assert!(chain.unpaid_proofs().unwrap().is_empty());
    }

    /// What a node serves a light client, read from a chain's database.
    fn serve(chain: &Chain, client: &mut crate::light::LightClient) {
        use crate::light::LightRequest;

        while let Some(request) = client.next() {
            match request {
                LightRequest::Headers { from, count } => {
                    let headers = (from..from + u64::from(count)).map_while(|height| stored_header_at(&*chain.db, height).unwrap()).collect();
                    client.deliver_headers(from, headers, LATER).unwrap();
                }
                LightRequest::Update { block } => client.take_update(&block, stored_light_update(&*chain.db, &block).unwrap()).unwrap(),
                LightRequest::Justifications { from } => {
                    client.deliver_justifications(&chain.justifications_from(from, 64).unwrap()).unwrap();
                }
            }
        }
    }

    #[test]
    fn a_light_client_follows_headers_and_finality_from_the_genesis_id_alone() {
        use crate::light::{LightClient, LightError};

        for finality in kinds() {
            let mut chain = open(&finality);
            finalize_through(&mut chain, 5);
            let params = genesis_with(Some(finality.clone())).light_params();
            let genesis = stored_header_at(&*chain.db, 0).unwrap().unwrap();
            let mut client = LightClient::new(params.clone(), genesis.clone(), chain.genesis()).unwrap();
            serve(&chain, &mut client);

            assert_eq!(client.tip().id(), chain.tip().id(), "{}", finality.name());
            assert_eq!(Some(client.justified()), chain.justified());
            assert_eq!(Some(client.finalized()), chain.finalized());
            assert!(client.finalized().epoch >= 4);

            // A value proven at the finalized checkpoint is final; at the tip it is not, yet.
            let who = dev().public().address();
            let key = crate::db::address_key(&who);
            let finalized = client.finalized().block;
            let at = |chain: &Chain, block: &Hash| {
                let state = chain.state_at(block).unwrap();
                let value = state.get(Column::Accounts, &key).unwrap();
                let proof = StateTree::new(&state, chain.hash()).prove(Column::Accounts, &key).unwrap();

                (value, proof)
            };
            let (value, proof) = at(&chain, &finalized);

            assert!(client.check_state(&finalized, Column::Accounts, &key, value.as_deref(), &proof).unwrap().final_);
            let (value, proof) = at(&chain, &chain.tip().id());
            assert!(!client.check_state(&chain.tip().id(), Column::Accounts, &key, value.as_deref(), &proof).unwrap().final_);
            assert_eq!(client.check_state(&chain.tip().id(), Column::Accounts, &key, None, &proof), Err(LightError::BadProof));

            // Saved, it keeps only the headers from the finalized block on, and a client restored
            // from it follows on from there as the chain goes on.
            let saved = client.save();
            let finalized_height = client.finalized().epoch * INTERVAL;

            assert_eq!(saved.headers[0].header.height, finalized_height);
            assert_eq!(saved.headers.len() as u64, chain.tip().height - finalized_height + 1);
            assert_eq!(LightClient::restore(saved.clone(), Hash::zero(chain.hash())).err(), Some(LightError::WrongGenesis));

            let mut client = LightClient::restore(saved, chain.genesis()).unwrap();
            finalize_through(&mut chain, 7);
            client.refresh();
            serve(&chain, &mut client);

            assert_eq!(client.tip().id(), chain.tip().id());
            assert_eq!(Some(client.finalized()), chain.finalized());

            let (value, proof) = at(&chain, &client.finalized().block);

            assert!(client.check_state(&client.finalized().block, Column::Accounts, &key, value.as_deref(), &proof).unwrap().final_);

            // A block older than it kept is not on its chain until it takes the headers between,
            // each proven by the one above it. Then what was in it checks, and is final.
            let old = 2;
            let block = stored_header_at(&*chain.db, old).unwrap().unwrap().id();
            let (value, proof) = at(&chain, &block);
            let lowest = client.lowest();
            let between = |to: u64| (old..to).map(|height| stored_header_at(&*chain.db, height).unwrap().unwrap()).collect::<Vec<_>>();
            let mut forged = between(lowest);
            forged.last_mut().unwrap().header.timestamp += 1;

            assert_eq!(lowest, finalized_height);
            assert_eq!(client.check_state(&block, Column::Accounts, &key, value.as_deref(), &proof), Err(LightError::NotOnChain));
            assert_eq!(client.deliver_ancestors(old, between(lowest - 1)), Err(LightError::Misplaced), "one short of what it holds");
            assert!(matches!(client.deliver_ancestors(old, forged), Err(LightError::BadHeader { height, .. }) if height == lowest - 1));
            assert_eq!(client.lowest(), lowest, "nothing taken from a batch that does not link");
            assert_eq!(client.deliver_ancestors(old, between(lowest)), Ok((lowest - old) as usize));
            assert_eq!(client.lowest(), old);
            assert!(client.check_state(&block, Column::Accounts, &key, value.as_deref(), &proof).unwrap().final_);

            // It trusts nothing it cannot check.
            let mut forged = params.clone();
            forged.hash = HashType::default_chain();
            forged.finality = None;

            assert_eq!(LightClient::new(params.clone(), genesis.clone(), Hash::zero(chain.hash())).err(), Some(LightError::WrongGenesis));
            assert_eq!(LightClient::new(forged, genesis.clone(), chain.genesis()).err(), Some(LightError::WrongParams));

            let mut fresh = LightClient::new(params, genesis, chain.genesis()).unwrap();
            let headers: Vec<_> = (1..=chain.tip().height).map(|height| stored_header_at(&*chain.db, height).unwrap().unwrap()).collect();
            fresh.deliver_headers(1, headers, LATER).unwrap();
            fresh.take_update(&chain.genesis(), stored_light_update(&*chain.db, &chain.genesis()).unwrap()).unwrap();
            let mut short = chain.justifications_from(1, 1).unwrap();
            short[0].votes.truncate(1);

            assert_eq!(fresh.deliver_justifications(&short), Err(LightError::BadJustification("its votes fall short")));

            let later = chain.block_at(2 * INTERVAL).unwrap().unwrap().id();

            assert_eq!(fresh.take_update(&later, stored_light_update(&*chain.db, &later).unwrap()), Err(LightError::Untrusted), "not justified yet");
        }
    }
}

#[test]
fn staking_needs_staking() {
    use crate::structs::amount::Amount;
    use crate::structs::tx::Tx;

    let mut chain = open_with(None);
    let bond = SignedTx::sign(
        Tx::Bond {
            voter: key(60).public(),
            amount: Amount::U128(500),
        },
        0,
        FeeOffer::NONE,
        &dev(),
    );
    let receipts = advance(&mut chain, vec![bond], Vec::new()).unwrap();
    let vote = Vote {
        source: Checkpoint {
            epoch: 0,
            block: chain.genesis(),
        },
        target: Checkpoint {
            epoch: 1,
            block: Hash::zero(chain.hash()),
        },
        voter: key(11).public(),
    };

    let approve = SignedTx::sign(
        Tx::ApproveSigners {
            version: 0,
            signers: vec![key(11).public()],
            threshold: 1,
            change_threshold: 1,
        },
        1,
        FeeOffer::NONE,
        &dev(),
    );

    assert_eq!(receipts[0].result, Err(Failure::NoStaking));
    assert_eq!(advance(&mut chain, vec![approve], Vec::new()).unwrap()[0].result, Err(Failure::NoSigners));
    assert_eq!((chain.justified(), chain.finalized()), (None, None));
    assert!(!chain.takes_votes());
    assert!(matches!(
        chain.add_vote(SignedVote::sign(vote, &key(11), &chain.genesis())),
        Err(FinalityError::Disabled)
    ));
}

#[cfg(feature = "finality-depth")]
mod depth {
    use super::*;

    fn deep(depth: u64) -> FinalityConfig {
        FinalityConfig::Depth(DepthConfig { depth })
    }

    fn at(chain: &Chain, height: u64) -> Checkpoint {
        Checkpoint {
            epoch: height,
            block: chain.block_at(height).unwrap().unwrap().id(),
        }
    }

    #[test]
    fn blocks_are_final_once_deep_enough() {
        let db = Arc::new(MemoryDb::default());
        let genesis = genesis_with(Some(deep(3)));
        let mut chain = Chain::open(db.clone(), &genesis).unwrap();
        grow_to(&mut chain, 2);

        assert_eq!(chain.finalized(), Some(at(&chain, 0)), "nothing is three deep yet");

        grow_to(&mut chain, 10);

        assert_eq!(chain.finalized(), Some(at(&chain, 7)));
        assert_eq!(chain.justified(), chain.finalized());
        assert_eq!(stored_finality(&*chain.db).unwrap(), (Some(at(&chain, 7)), Some(at(&chain, 7))));

        let reopened = Chain::open(db, &genesis).unwrap();

        assert_eq!(reopened.finalized(), Some(at(&chain, 7)));
    }

    #[test]
    fn depth_takes_no_votes_or_proofs() {
        let mut chain = open_with(Some(deep(2)));
        grow_to(&mut chain, 4);
        let voter = key(11);
        let vote = Vote {
            source: at(&chain, 0),
            target: at(&chain, 1),
            voter: voter.public(),
        };
        let signed = SignedVote::sign(vote, &voter, &chain.genesis());
        let proof = Justification {
            source: vote.source,
            target: vote.target,
            votes: vec![signed],
        };

        assert!(!chain.takes_votes());
        assert!(matches!(chain.add_vote(signed), Err(FinalityError::NoVotes)));
        assert!(matches!(chain.add_justification(proof.clone()), Err(FinalityError::NoVotes)));
        assert_eq!(chain.vote_for(&voter.public()).unwrap(), None);
        assert!(chain.unpaid_proofs().unwrap().is_empty());
        assert!(chain.justifications_from(0, 10).unwrap().is_empty());
        assert!(matches!(
            advance(&mut chain, Vec::new(), vec![proof]),
            Err(ImportError::Invalid(BlockError::Proof(why))) if why.contains("takes no proofs")
        ));
    }

    #[test]
    fn heavier_branches_from_below_the_depth_are_never_followed() {
        let mut chain = Chain::open(Arc::new(MemoryDb::default()), &pow_genesis(deep(3))).unwrap();
        let genesis = chain.block_at(0).unwrap().unwrap();
        let main = mine_branch(&mut chain, &genesis, 6, 1);

        assert_eq!(chain.finalized(), Some(at(&chain, 3)));

        // A branch from height 2, below the finalized block, is refused once it reaches height 3.
        let from_two = chain.block_at(2).unwrap().unwrap();
        let sibling = mine_on(&chain, &from_two, 2);

        assert!(matches!(
            chain.import(&sibling, LATER),
            Err(ImportError::Invalid(BlockError::ConflictsWithFinality))
        ));

        // From the finalized block itself, a heavier branch takes over.
        let finalized = chain.block_at(3).unwrap().unwrap();
        let heavier = mine_branch(&mut chain, &finalized, 5, 3);

        assert_eq!(chain.tip().id(), heavier.id());
        assert_ne!(chain.tip().id(), main.id());
        assert_eq!(chain.finalized(), Some(at(&chain, 5)));
    }

    #[test]
    fn depth_zero_never_reorganizes() {
        let mut chain = Chain::open(Arc::new(MemoryDb::default()), &pow_genesis(deep(0))).unwrap();
        let genesis = chain.block_at(0).unwrap().unwrap();
        let main = mine_branch(&mut chain, &genesis, 3, 1);
        let below_tip = chain.block_at(2).unwrap().unwrap();

        assert_eq!(chain.finalized(), Some(at(&chain, 3)));
        assert!(matches!(
            chain.import(&mine_on(&chain, &below_tip, 2), LATER),
            Err(ImportError::Invalid(BlockError::ConflictsWithFinality))
        ));
        assert_eq!(chain.tip().id(), main.id());
    }
}
