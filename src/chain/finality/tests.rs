//! Finality on a chain, for every kind this build has. The vote-based kinds, ffg and authority,
//! run the same tests set up alike: three voters, two of whom are enough. Then what only one kind
//! does: staking and slashing under FFG, thresholds under authority, and depth finality.

use super::*;
use crate::chain::*;
use crate::consensus::{AuthorityProducer, ConsensusConfig, DevAuthority, PowConfig, Producer, meets, work_hash};
use crate::registry::signers::Signer;
#[cfg(any(feature = "finality-ffg", feature = "finality-authority"))]
use crate::consensus::ConsensusError;
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
    let mut block = chain.build(txs, proofs, producer.address(&[]), now).unwrap();
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

    /// The version an approval of a change names, and what the approval that decides it says, as
    /// the chain stands: the chain's next decision, and its kind's next version.
    #[cfg(all(feature = "finality-ffg", feature = "governance-council"))]
    fn deciding(chain: &Chain, change: &crate::governance::Change) -> (u64, Effect) {
        let state = chain.state();
        let changes = Changes::new(&state);
        let kind = change.kind();
        let version = changes.version_of(kind).unwrap();

        (version, Effect::Decided { decision: changes.decided().unwrap().version + 1, kind, version: version + 1 })
    }
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
            leak: None,
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
            signers: voters().iter().map(|keypair| Signer::own(keypair.public())).collect(),
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
    fn vote_rewards_come_from_the_pool_when_genesis_says_so() {
        use crate::chain::{FeeConfig, REWARD_POOL, VoteRewardSource};
        use crate::structs::amount::Amount;

        for finality in kinds() {
            // Fees on, half of every base part pooled rather than burned, and a reward no pool
            // of a few fees can cover, so the pool runs dry on the first voter.
            let huge = u256::new(10u128.pow(30));
            let finality = match finality {
                #[cfg(feature = "finality-ffg")]
                FinalityConfig::Ffg(config) => FinalityConfig::Ffg(FfgConfig { vote_reward: huge, ..config }),
                #[cfg(feature = "finality-authority")]
                FinalityConfig::Authority(config) => FinalityConfig::Authority(AuthorityConfig { vote_reward: huge, ..config }),
                #[cfg(feature = "finality-depth")]
                other => other,
            };
            let genesis = Genesis {
                fees: Some(FeeConfig::dev()),
                vote_reward_source: Some(VoteRewardSource::Fees),
                ..genesis_with(Some(finality))
            };
            let mut chain = Chain::open(Arc::new(MemoryDb::default()), &genesis).unwrap();
            let pool = |chain: &Chain| balance(chain, &REWARD_POOL);
            let supply = |chain: &Chain| {
                let voters: u256 = voters().iter().map(|keypair| balance(chain, &keypair.public().address())).fold(u256::ZERO, |sum, next| sum + next);

                voters + balance(chain, &dev().public().address()) + pool(chain) + balance(chain, &key(99).public().address())
            };
            let before = supply(&chain);

            // A fee-paying transfer: the burned half of its base part lands in the pool.
            let fee = FeeOffer {
                gas_limit: 100_000,
                max_price: chain.next_base_price() * u256::new(2),
                tip: u256::ZERO,
            };
            let tx = Tx::Transfer {
                from: dev().public().address(),
                to: key(99).public().address(),
                amount: Amount::U128(1),
            };
            let signed = SignedTx::sign(tx, nonce(&chain, &dev()), fee, &dev());
            let receipts = advance(&mut chain, vec![signed], Vec::new()).unwrap();
            let fed = pool(&chain);

            assert_eq!(receipts[0].result, Ok(Effect::Done));
            assert!(fed > u256::ZERO, "the pool got the burned half");
            assert_eq!(supply(&chain), before, "nothing burned, nothing minted");

            // The proof pays its first voter, by key, all the pool holds, and the rest nothing.
            finalize_through(&mut chain, 1);
            let proofs = chain.unpaid_proofs().unwrap();
            let mut voted: Vec<PublicKey> = proofs[0].votes.iter().map(|vote| vote.vote.voter).collect();
            voted.sort();
            let balances = |chain: &Chain| voted.iter().map(|voter| balance(chain, &voter.address())).collect::<Vec<_>>();
            let fed = pool(&chain);
            let owed = balances(&chain);
            let before = supply(&chain);

            advance(&mut chain, Vec::new(), proofs).unwrap();
            let paid = balances(&chain);

            assert_eq!(pool(&chain), u256::ZERO);
            assert_eq!(paid[0], owed[0] + fed);
            assert_eq!(&paid[1..], &owed[1..]);
            assert_eq!(supply(&chain), before, "paid from the pool, not minted");
            assert_eq!(crate::registry::supply::adds_up(&chain.state()).unwrap(), Ok(()), "minted less burned is what is held and bonded");
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

    #[test]
    fn a_justified_branch_wins_however_far_back_this_node_mined_alone() {
        for finality in kinds() {
            // Reorganizations by work alone go at most six blocks deep here.
            let genesis = Genesis { max_reorg_depth: 6, ..pow_genesis(finality) };
            let mut voters_chain = Chain::open(Arc::new(MemoryDb::default()), &genesis).unwrap();
            let mut follower = Chain::open(Arc::new(MemoryDb::default()), &genesis).unwrap();
            let first = voters_chain.block_at(0).unwrap().unwrap();
            let voted = mine_branch(&mut voters_chain, &first, INTERVAL + 1, 1);

            for keypair in &voters()[..2] {
                let vote = signed(&voters_chain, keypair, start(&voters_chain), checkpoint(&voters_chain, 1));
                voters_chain.add_vote(vote).unwrap();
            }

            // The follower, cut off, mined far past the reorganization depth on its own.
            let alone = mine_branch(&mut follower, &first, INTERVAL + 20, 2);

            for height in 1..=voted.header.height {
                let _ = follower.import(&voters_chain.block_at(height).unwrap().unwrap(), LATER).unwrap();
            }

            assert_eq!(follower.tip().id(), alone.id(), "work alone does not move it that far");

            // It mines on, its tip moving: the voters' branch is kept all the same.
            let alone = mine_branch(&mut follower, &alone, 3, 2);
            assert_eq!(follower.tip().id(), alone.id());

            // The voters' justification does: what they chose is the chain, however deep.
            let proof = voters_chain.justifications_from(1, 1).unwrap().remove(0);
            let update = follower.add_justification(proof).unwrap();
            let (undone, _) = update.reorganized.expect("the justification moves the chain");

            assert_eq!(follower.tip().id(), voted.id());
            assert_eq!(undone.len() as u64, INTERVAL + 23);
        }
    }

    #[cfg(feature = "finality-ffg")]
    #[test]
    fn quiet_validators_leak_until_the_rest_can_finalize_again() {
        use crate::chain::Leak;
        use crate::registry::validators::Validators;

        // Three equal bonds of 1,000; two make two thirds. Two epochs without a proven checkpoint
        // let bundles in, and from the third on half of a quiet validator's bond leaks at each
        // checkpoint.
        let config = match ffg() {
            FinalityConfig::Ffg(config) => config,
            #[allow(unreachable_patterns)]
            _ => unreachable!(),
        };
        let mut config = FfgConfig {
            leak: Some(Leak { after_epochs: 2, permille: 500 }),
            ..config
        };
        config.validators.iter_mut().for_each(|validator| validator.bond = u256::new(1_000));
        let mut chain = open(&FinalityConfig::Ffg(config));
        let [alice, bob, carol] = voters();
        let bond = |chain: &Chain, keypair: &Keypair| Validators::new(&chain.state()).get(&keypair.public()).unwrap().unwrap().bond;
        // Grows the chain to a height, every block carrying what proofs the node has for it.
        let grow = |chain: &mut Chain, height: u64| {
            while chain.tip().height < height {
                let proofs = chain.unpaid_proofs().unwrap();
                advance(chain, Vec::new(), proofs).unwrap();
            }
        };

        // Epoch 1 is justified by two voters, and the next block proves it to the chain's state.
        grow(&mut chain, INTERVAL + 1);

        for keypair in [&alice, &bob] {
            chain.add_vote(honest_vote(&chain, keypair)).unwrap();
        }

        grow(&mut chain, INTERVAL + 2);
        assert_eq!(Validators::new(&chain.state()).proven().unwrap(), 1);

        // From then on Alice alone votes: a third, so nothing justifies, and bundles of her votes
        // land in blocks once two epochs have passed unproven, which no block takes before.
        grow(&mut chain, 2 * INTERVAL + 1);
        chain.add_vote(honest_vote(&chain, &alice)).unwrap();
        let early = Justification {
            source: checkpoint(&chain, 1),
            target: checkpoint(&chain, 2),
            votes: vec![signed(&chain, &alice, checkpoint(&chain, 1), checkpoint(&chain, 2))],
        };

        assert!(chain.unpaid_proofs().unwrap().is_empty(), "not stalled yet");
        assert!(matches!(
            advance(&mut chain, Vec::new(), vec![early]),
            Err(ImportError::Invalid(BlockError::Proof(why))) if why.contains("falls short")
        ));

        let mut justified_again = None;

        for epoch in 3..=12 {
            grow(&mut chain, epoch * INTERVAL + 1);
            chain.add_vote(honest_vote(&chain, &alice)).unwrap();
            grow(&mut chain, epoch * INTERVAL + 2);

            if chain.justified().unwrap().epoch > 1 {
                justified_again = Some(epoch);

                break;
            }
        }

        // Bob's and Carol's bonds leaked by half at each stalled checkpoint; Alice's did not. Once
        // her 1,000 are two thirds of what is left, her votes alone justify, and the chain finalizes
        // again.
        let leaked = justified_again.expect("Alice alone comes to justify");

        assert_eq!(bond(&chain, &alice), u256::new(1_000));
        assert!(bond(&chain, &bob) <= u256::new(250), "{}", bond(&chain, &bob));
        assert_eq!(bond(&chain, &bob), bond(&chain, &carol));
        assert!(Validators::new(&chain.state()).participation(&alice.public()).unwrap() >= leaked - 1);
        assert_eq!(Validators::new(&chain.state()).participation(&bob.public()).unwrap(), 1);

        grow(&mut chain, (leaked + 1) * INTERVAL + 1);
        chain.add_vote(honest_vote(&chain, &alice)).unwrap();

        assert_eq!(chain.finalized().unwrap().epoch, leaked, "the next checkpoint finalizes the one before");

        // Under the minimum bond, a leaked validator is on its way out, as if it had unbonded.
        let record = Validators::new(&chain.state()).get(&bob.public()).unwrap().unwrap();

        assert!(record.active_until.is_some() == (record.bond < u256::new(100)));

        assert_eq!(crate::registry::supply::adds_up(&chain.state()).unwrap(), Ok(()), "minted less burned is what is held and bonded");
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

        // Slashed in epoch 2, with sets fixed through epoch 4: she keeps her weight in epochs 2
        // and 3, and is out of epoch 4's set at once. Checkpoint 3's light update re-states the
        // sets of epochs 4 and 5, so a light client learns it the usual way.
        let in_set = |epoch: u64| Validators::new(&chain.state()).set(epoch).unwrap().unwrap().member(&carol.public()).is_some();

        assert!(in_set(2) && in_set(3) && !in_set(4));

        grow_to(&mut chain, 3 * INTERVAL);
        let checkpoint = chain.block_at(3 * INTERVAL).unwrap().unwrap().id();
        let update = stored_light_update(&*chain.db, &checkpoint).unwrap().unwrap();
        let epochs: Vec<u64> = update.voters.iter().map(|voter| voter.epoch).collect();
        let set_of = |epoch: u64| {
            let proof = update.voters.iter().find(|voter| voter.epoch == epoch).unwrap();

            wincode::deserialize::<crate::registry::validators::ValidatorSet>(&proof.value).unwrap()
        };

        assert_eq!(epochs, [4, 5]);
        assert!(set_of(4).member(&carol.public()).is_none());
        assert!(set_of(5).member(&carol.public()).is_none());

        // Not two conflicting votes: refused before it runs, so a block holding it is invalid.
        let now = chain.tip().timestamp + SLOT;
        let mut producer = AuthorityProducer::new(dev(), SLOT);
        let mut block = chain.build(Vec::new(), Vec::new(), producer.address(&[]), now).unwrap();
        block.txs = vec![slash(honest, honest, 2)];
        block.header.tx_root = Block::tx_root(chain.hash(), &block.txs);
        block.seal = producer.seal(&block.header).unwrap();

        assert!(matches!(
            chain.import(&block, now),
            Err(ImportError::Invalid(BlockError::Tx { error: TxError::Invalid(_), .. }))
        ));

        assert_eq!(crate::registry::supply::adds_up(&chain.state()).unwrap(), Ok(()), "minted less burned is what is held and bonded");
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

    /// A chain whose signers produce in turn, every two seconds.
    #[cfg(feature = "finality-authority")]
    fn rotating() -> Chain {
        let genesis = Genesis {
            consensus: ConsensusConfig::Rotating { slot_ms: SLOT },
            ..genesis_with(Some(authority()))
        };

        Chain::open(Arc::new(MemoryDb::default()), &genesis).unwrap()
    }

    /// The signer whose turn the next block is, among the three voters.
    #[cfg(feature = "finality-authority")]
    fn in_turn(chain: &Chain) -> Keypair {
        let producers = chain.producers_for(chain.tip().height + 1).unwrap();
        let mut keys: Vec<PublicKey> = producers.iter().map(|signer| signer.voter).collect();
        keys.sort();
        let wanted = keys[((chain.tip().height + 1) % keys.len() as u64) as usize];

        voters().into_iter().chain([key(14)]).find(|keypair| keypair.public() == wanted).unwrap()
    }

    /// Builds the next block as a signer, at its slot or at a time given, and imports it.
    #[cfg(feature = "finality-authority")]
    fn produce_as(chain: &mut Chain, keypair: &Keypair, at: Option<u64>) -> Result<Imported, ImportError> {
        use crate::consensus::RotatingProducer;

        let tip = chain.tip().clone();
        let producers = chain.producers_for(tip.height + 1).unwrap();
        let mut producer = RotatingProducer::new(keypair.clone(), keypair.public().address(), SLOT);
        let now = at.unwrap_or_else(|| producer.start_at(&tip, &producers));
        let declared = producer.difficulty(&tip, &producers);
        let mut block = chain.build_declaring(Vec::new(), Vec::new(), producer.address(&[]), now, None, declared, None).unwrap();
        block.seal = producer.seal(&block.header).unwrap();

        chain.import(&block, now)
    }

    #[cfg(feature = "finality-authority")]
    #[test]
    fn signers_produce_in_turn_and_step_in_for_one_that_misses() {
        let mut chain = rotating();
        let [alice, bob, carol] = voters();

        // Ten blocks in turn, each weighing 2.
        for _ in 0..10 {
            let signer = in_turn(&chain);

            assert!(matches!(produce_as(&mut chain, &signer, None), Ok(Imported::Extended(_))));
            assert_eq!(chain.tip().difficulty, 0);
            assert_eq!(chain.tip().producer, signer.public().address());
        }

        assert_eq!(chain.tip_work(), u256::new(20));

        // The next in line is early at one slot and steps in at two, weighing 1; a stranger never.
        let signer = in_turn(&chain);
        let next = [alice.clone(), bob.clone(), carol.clone()]
            .into_iter()
            .find(|keypair| {
                let mut keys: Vec<PublicKey> = voters().iter().map(Keypair::public).collect();
                keys.sort();
                let index = keys.iter().position(|key| *key == signer.public()).unwrap();

                keypair.public() == keys[(index + 1) % 3]
            })
            .unwrap();
        let tip = chain.tip().timestamp;

        assert!(matches!(produce_as(&mut chain, &next, Some(tip + SLOT)), Err(ImportError::Invalid(BlockError::Consensus(ConsensusError::Early)))));
        assert!(matches!(produce_as(&mut chain, &next, Some(tip + 2 * SLOT)), Ok(Imported::Extended(_))));
        assert_eq!(chain.tip().difficulty, 1);
        assert_eq!(chain.tip_work(), u256::new(21));
        assert!(matches!(produce_as(&mut chain, &key(60), None), Err(ImportError::Invalid(BlockError::Consensus(ConsensusError::WrongProducer)))));

        // The set the schedule follows is the epoch's: once Dave replaces Carol, Dave produces
        // and Carol may not, from the epoch the change is fixed for.
        let dave = key(14);
        let approve = |chain: &Chain, keypair: &Keypair| {
            let tx = Tx::ApproveSigners {
                version: 0,
                signers: vec![Signer::own(alice.public()), Signer::own(bob.public()), Signer::own(dave.public())],
                threshold: 2,
                change_threshold: 2,
            };

            SignedTx::sign(tx, nonce(chain, keypair), FeeOffer::NONE, keypair)
        };
        let txs = vec![approve(&chain, &alice), approve(&chain, &bob)];
        let tip = chain.tip().clone();
        let signer = in_turn(&chain);
        let producers = chain.producers_for(tip.height + 1).unwrap();
        let mut producer = crate::consensus::RotatingProducer::new(signer.clone(), signer.public().address(), SLOT);
        let now = producer.start_at(&tip, &producers);
        let mut block = chain.build_declaring(txs, Vec::new(), producer.address(&[]), now, None, producer.difficulty(&tip, &producers), None).unwrap();
        block.seal = producer.seal(&block.header).unwrap();
        chain.import(&block, now).unwrap();

        while chain.producers_for(chain.tip().height + 1).unwrap().iter().any(|signer| signer.voter == carol.public()) {
            let signer = in_turn(&chain);
            produce_as(&mut chain, &signer, None).unwrap();
        }

        let producers = chain.producers_for(chain.tip().height + 1).unwrap();

        assert!(producers.iter().any(|signer| signer.voter == dave.public()));
        assert!(chain.producers_for(chain.tip().height).unwrap().iter().any(|signer| signer.voter == carol.public()) || chain.tip().height.is_multiple_of(INTERVAL));

        for _ in 0..3 {
            let signer = in_turn(&chain);

            assert_ne!(signer.public(), carol.public());
            produce_as(&mut chain, &signer, None).unwrap();
        }
    }

    #[cfg(feature = "finality-authority")]
    #[test]
    fn a_light_client_waits_for_the_voters_before_it_checks_their_blocks() {
        use crate::light::{Extended, LightClient, LightRequest};

        let mut chain = rotating();

        // Three epochs of blocks, and one more: epochs 0 to 2 are fixed at genesis, epoch 3 at
        // checkpoint 1, whose update a client trusts once epoch 1 is justified.
        for _ in 0..3 * INTERVAL + 1 {
            let signer = in_turn(&chain);
            produce_as(&mut chain, &signer, None).unwrap();
        }

        for keypair in &voters()[..2] {
            chain.add_vote(signed(&chain, keypair, start(&chain), checkpoint(&chain, 1))).unwrap();
        }

        assert_eq!(chain.justified().unwrap().epoch, 1);

        let genesis = chain.block_at(0).unwrap().unwrap();
        let params = genesis_with(Some(authority())).light_params();
        let params = crate::light::LightParams {
            consensus: ConsensusConfig::Rotating { slot_ms: SLOT },
            ..params
        };
        let mut client = LightClient::new(params, genesis.sealed_header(), chain.genesis()).unwrap();
        let headers_from = |from: u64| -> Vec<SealedHeader> { chain.headers_from(from, 1_000).unwrap() };

        // It cannot check block 1 before genesis's update names epoch 0's signers.
        assert_eq!(client.next(), Some(LightRequest::Headers { from: 1, count: crate::light::MAX_LIGHT_HEADERS }));
        assert_eq!(client.deliver_headers(1, headers_from(1), LATER).unwrap(), Extended::Waiting);
        assert_eq!(client.tip().height, 0);
        assert_eq!(client.next(), Some(LightRequest::Finality { from: 0 }));

        let steps = |from: u64| stored_light_finality(&*chain.db, INTERVAL, from, 64, chain.tip().height).unwrap();

        // Genesis's update with one epoch's voters left out is refused, though every proof in it
        // holds: a node could otherwise stall a client by holding voters back.
        let mut short = steps(0);
        short[0].update.as_mut().unwrap().voters.pop();

        assert_eq!(client.deliver_finality(0, short), Err(crate::light::LightError::BadProof));
        client.keep_checked(crate::light::Kept::default());
        client.deliver_finality(0, steps(0)).unwrap();

        // Now through epoch 2, where it waits for checkpoint 1's update: epoch 1's justification
        // makes it trusted, and the rest follows.
        assert_eq!(client.next(), Some(LightRequest::Headers { from: 1, count: crate::light::MAX_LIGHT_HEADERS }));
        assert_eq!(client.deliver_headers(1, headers_from(1), LATER).unwrap(), Extended::Added((3 * INTERVAL - 1) as usize));
        assert_eq!(client.tip().height, 3 * INTERVAL - 1);
        assert!(matches!(client.next(), Some(LightRequest::Finality { .. })));

        while let Some(request) = client.next() {
            match request {
                LightRequest::Headers { from, .. } => {
                    client.deliver_headers(from, headers_from(from), LATER).unwrap();
                }
                LightRequest::Finality { from } => {
                    client.deliver_finality(from, steps(from)).unwrap();
                }
            }
        }

        assert_eq!(client.tip().id(), chain.tip().id());
        assert_eq!(client.justified().epoch, 1);

        // What it checked, kept: genesis's and checkpoint 1's updates, and epoch 1's justification.
        let kept = client.take_checked();
        let checkpoint_1 = chain.block_at(INTERVAL).unwrap().unwrap().id();

        assert_eq!(kept.updates.iter().map(|update| update.block).collect::<Vec<_>>(), [genesis.id(), checkpoint_1]);
        assert_eq!(kept.justifications.iter().map(|justification| justification.target.block).collect::<Vec<_>>(), [checkpoint_1]);
        assert_eq!(client.take_checked(), crate::light::Kept::default(), "taken");
    }

    /// A chain whose FFG validators produce by stake, every two seconds, under a schedule.
    #[cfg(feature = "finality-ffg")]
    fn staked(schedule: crate::consensus::Schedule) -> (Chain, crate::consensus::Staked) {
        let genesis = Genesis {
            consensus: ConsensusConfig::Staked { slot_ms: SLOT, schedule },
            ..genesis_with(Some(ffg()))
        };
        let chain = Chain::open(Arc::new(MemoryDb::default()), &genesis).unwrap();
        let rules = crate::consensus::Staked::new(SLOT, schedule, chain.hash(), INTERVAL);

        (chain, rules)
    }

    /// The keypair of a validator's voting key, among the three voters and the newcomer.
    #[cfg(feature = "finality-ffg")]
    fn validator_key(voter: &PublicKey) -> Keypair {
        voters().into_iter().chain([key(60)]).find(|keypair| keypair.public() == *voter).unwrap()
    }

    /// Builds the next block as a validator, with these transactions, at its slot or at a time
    /// given, and imports it.
    #[cfg(feature = "finality-ffg")]
    fn produce_by_stake(chain: &mut Chain, rules: crate::consensus::Staked, keypair: &Keypair, txs: Vec<SignedTx>, at: Option<u64>) -> Result<Imported, ImportError> {
        use crate::consensus::StakedProducer;

        let tip = chain.tip().clone();
        let producers = chain.producers_for(tip.height + 1).unwrap();
        let mut producer = StakedProducer::new(keypair.clone(), rules);
        let now = at.unwrap_or_else(|| producer.start_at(&tip, &producers));
        let declared = producer.difficulty(&tip, &producers);
        let mut block = chain.build_declaring(txs, Vec::new(), producer.address(&producers), now, None, declared, None).unwrap();
        block.seal = producer.seal(&block.header).unwrap();

        chain.import(&block, now)
    }

    /// Produces the next block as whoever is in turn.
    #[cfg(feature = "finality-ffg")]
    fn produce_in_turn(chain: &mut Chain, rules: crate::consensus::Staked, txs: Vec<SignedTx>) -> Keypair {
        let height = chain.tip().height + 1;
        let producers = chain.producers_for(height).unwrap();
        let keypair = validator_key(&rules.producer_at(&producers, height, 0).unwrap().voter);

        assert!(matches!(produce_by_stake(chain, rules, &keypair, txs, None), Ok(Imported::Extended(_))));

        keypair
    }

    #[cfg(feature = "finality-ffg")]
    #[test]
    fn validators_produce_by_stake_and_a_new_bond_joins_the_schedule_from_its_epoch() {
        use crate::consensus::Schedule;
        use crate::registry::validators::Validators;
        use crate::structs::amount::Amount;

        for schedule in [Schedule::Draw, Schedule::Priority] {
            let (mut chain, rules) = staked(schedule);

            // Ten blocks in turn, each weighing 2, from the three genesis validators alone.
            for _ in 0..10 {
                let keypair = produce_in_turn(&mut chain, rules, Vec::new());

                assert_eq!(chain.tip().difficulty, 0);
                assert_eq!(chain.tip().producer, keypair.public().address());
                assert!(voters().iter().any(|voter| voter.public() == keypair.public()));
            }

            assert_eq!(chain.tip_work(), u256::new(20));

            // The next in line is early at its slot less one and steps in at its slot, weighing 1;
            // a stranger never.
            let height = chain.tip().height + 1;
            let producers = chain.producers_for(height).unwrap();
            let in_turn = rules.producer_at(&producers, height, 0).unwrap();
            let (wait, next) = (1..).map(|offset| (offset, rules.producer_at(&producers, height, offset).unwrap())).find(|(_, member)| member.voter != in_turn.voter).unwrap();
            let next = validator_key(&next.voter);
            let tip = chain.tip().timestamp;

            assert!(matches!(
                produce_by_stake(&mut chain, rules, &next, Vec::new(), Some(tip + wait * SLOT)),
                Err(ImportError::Invalid(BlockError::Consensus(ConsensusError::Early)))
            ));
            assert!(matches!(produce_by_stake(&mut chain, rules, &next, Vec::new(), Some(tip + (wait + 1) * SLOT)), Ok(Imported::Extended(_))));
            assert_eq!(chain.tip().difficulty, u128::from(wait));
            assert_eq!(chain.tip_work(), u256::new(21));
            assert!(matches!(
                produce_by_stake(&mut chain, rules, &key(60), Vec::new(), None),
                Err(ImportError::Invalid(BlockError::Consensus(ConsensusError::WrongProducer)))
            ));

            // The dev key bonds a newcomer with fifty times the others' stake: it produces from
            // the epoch its bond first counts in, as the heaviest, and not before.
            let newcomer = key(60);
            let bond = Tx::Bond {
                voter: newcomer.public(),
                amount: Amount::U128(500),
            };
            let signed = SignedTx::sign(bond, nonce(&chain, &dev()), FeeOffer::NONE, &dev());
            produce_in_turn(&mut chain, rules, vec![signed]);
            let record = Validators::new(&chain.state()).get(&newcomer.public()).unwrap().unwrap();
            let joins = record.active_from;

            assert_eq!((record.account, record.bond), (dev().public().address(), u256::new(500)));

            while chain.tip().height + 1 < joins * INTERVAL {
                let keypair = produce_in_turn(&mut chain, rules, Vec::new());

                assert_ne!(keypair.public(), newcomer.public());
            }

            let producers = chain.producers_for(joins * INTERVAL).unwrap();

            assert!(producers.iter().any(|member| member.voter == newcomer.public() && member.weight == u256::new(500)));

            let mut turns = 0;

            for _ in 0..INTERVAL {
                turns += u64::from(produce_in_turn(&mut chain, rules, Vec::new()).public() == newcomer.public());
            }

            match schedule {
                Schedule::Priority => assert_eq!(turns, INTERVAL, "fifty times the stake is every turn of a short epoch"),
                Schedule::Draw => assert!(turns >= INTERVAL / 2, "{turns} of {INTERVAL} turns with fifty times the stake"),
            }
        }
    }

    #[cfg(feature = "finality-ffg")]
    #[test]
    fn a_light_client_follows_a_staked_chain_by_the_validators_it_proved() {
        use crate::consensus::Schedule;
        use crate::light::{Extended, LightClient, LightRequest};

        let (mut chain, rules) = staked(Schedule::Priority);

        // Three epochs of blocks, and one more: epochs 0 to 2 are fixed at genesis, epoch 3 at
        // checkpoint 1, whose update a client trusts once epoch 1 is justified.
        for _ in 0..3 * INTERVAL + 1 {
            produce_in_turn(&mut chain, rules, Vec::new());
        }

        for keypair in &voters()[..2] {
            chain.add_vote(signed(&chain, keypair, start(&chain), checkpoint(&chain, 1))).unwrap();
        }

        assert_eq!(chain.justified().unwrap().epoch, 1);

        let genesis = chain.block_at(0).unwrap().unwrap();
        let params = crate::light::LightParams {
            consensus: ConsensusConfig::Staked { slot_ms: SLOT, schedule: Schedule::Priority },
            ..genesis_with(Some(ffg())).light_params()
        };
        let mut client = LightClient::new(params, genesis.sealed_header(), chain.genesis()).unwrap();
        let headers_from = |from: u64| -> Vec<SealedHeader> { chain.headers_from(from, 1_000).unwrap() };

        // It cannot check block 1 before genesis's update names epoch 0's validators.
        assert_eq!(client.deliver_headers(1, headers_from(1), LATER).unwrap(), Extended::Waiting);
        assert_eq!(client.tip().height, 0);
        assert_eq!(client.next(), Some(LightRequest::Finality { from: 0 }));

        let steps = |from: u64| stored_light_finality(&*chain.db, INTERVAL, from, 64, chain.tip().height).unwrap();
        client.deliver_finality(0, steps(0)).unwrap();

        // Now through epoch 2, where it waits for checkpoint 1's update, and then the rest.
        assert_eq!(client.deliver_headers(1, headers_from(1), LATER).unwrap(), Extended::Added((3 * INTERVAL - 1) as usize));

        while let Some(request) = client.next() {
            match request {
                LightRequest::Headers { from, .. } => {
                    client.deliver_headers(from, headers_from(from), LATER).unwrap();
                }
                LightRequest::Finality { from } => {
                    client.deliver_finality(from, steps(from)).unwrap();
                }
            }
        }

        assert_eq!(client.tip().id(), chain.tip().id());
        assert_eq!(client.justified().epoch, 1);
    }

    /// The dev authority chain with FFG finality and governance, so the consensus can change.
    #[cfg(all(feature = "finality-ffg", any(feature = "governance-electorate", feature = "governance-council")))]
    fn governed(governance: crate::governance::GovernanceConfig) -> Chain {
        let genesis = Genesis {
            governance: Some(governance),
            ..genesis_with(Some(ffg()))
        };

        Chain::open(Arc::new(MemoryDb::default()), &genesis).unwrap()
    }

    /// A change to production by stake, drawn.
    #[cfg(all(feature = "finality-ffg", any(feature = "governance-electorate", feature = "governance-council")))]
    fn to_staked() -> crate::governance::Change {
        crate::governance::Change::Consensus(ConsensusConfig::Staked {
            slot_ms: SLOT,
            schedule: crate::consensus::Schedule::Draw,
        })
    }

    /// Sends a transaction from a key in the next authority block, and returns its receipt.
    #[cfg(all(feature = "finality-ffg", any(feature = "governance-electorate", feature = "governance-council")))]
    fn send_as(chain: &mut Chain, keypair: &Keypair, tx: Tx) -> Receipt {
        let signed = SignedTx::sign(tx, nonce(chain, keypair), FeeOffer::NONE, keypair);

        advance(chain, vec![signed], Vec::new()).unwrap().remove(0)
    }

    #[cfg(all(feature = "finality-ffg", feature = "governance-electorate"))]
    #[test]
    fn the_electorate_changes_the_consensus_and_the_change_takes_effect_epochs_later() {
        use crate::consensus::Schedule;
        use crate::governance::{Change, GovernanceConfig};
        use crate::light::{Extended, LightClient, LightRequest};
        use crate::registry::changes::Changes;

        let mut chain = governed(GovernanceConfig::Electorate { approval_epochs: 8 });
        let [alice, bob, _] = voters();
        let rules = crate::consensus::Staked::new(SLOT, Schedule::Draw, chain.hash(), INTERVAL);
        let approve = |version: u64, change: Change| Tx::ApproveChange { version, change };

        assert_eq!(chain.consensus_config_at(1).unwrap(), genesis_with(None).consensus);

        // The dev key holds the funds but no stake; a rotating authority needs other finality;
        // Alice's approval alone is a third of the stake.
        let refused = send_as(&mut chain, &dev(), approve(0, to_staked()));

        assert!(matches!(&refused.result, Err(failure) if failure.to_string().contains("governance lets approve")), "{:?}", refused.result);

        let unfit = send_as(&mut chain, &alice, approve(0, Change::Consensus(ConsensusConfig::Rotating { slot_ms: SLOT })));

        assert!(matches!(&unfit.result, Err(failure) if failure.to_string().contains("needs finality of type authority")), "{:?}", unfit.result);
        assert_eq!(send_as(&mut chain, &alice, approve(0, to_staked())).result, Ok(Effect::Done));

        let ballot = Changes::new(&chain.state()).ballot(crate::governance::ChangeKind::Consensus).unwrap();

        assert_eq!(ballot.approvers(&to_staked()), [alice.public().address()]);
        assert_eq!(Changes::new(&chain.state()).decided().unwrap().version, 0);

        // Bob's makes two thirds: decided, version 1. The epochs fixed already keep the
        // authority; the checkpoint after the decision fixes stake for the epoch two ahead.
        assert_eq!(send_as(&mut chain, &bob, approve(0, to_staked())).result, Ok(Effect::Decided { decision: 1, kind: crate::governance::ChangeKind::Consensus, version: 1 }));

        let decided = Changes::new(&chain.state()).decided().unwrap();
        let Change::Consensus(staked) = to_staked() else { unreachable!() };

        assert_eq!((decided.version, &decided.consensus), (1, &staked));

        // The checkpoint after the decision, the block it landed in when that is one, fixes it
        // activation_delay epochs ahead.
        let checkpoint = chain.tip().height.div_ceil(INTERVAL) * INTERVAL;
        let switch = checkpoint + 2 * INTERVAL;
        // Two voters vote whenever a checkpoint is due, so every checkpoint is justified in turn,
        // as a light client needs them.
        let vote = |chain: &mut Chain| {
            while chain.vote_for(&voters()[0].public()).unwrap().is_some() {
                for keypair in &voters()[..2] {
                    let vote = honest_vote(chain, keypair);
                    chain.add_vote(vote).unwrap();
                }
            }
        };

        while chain.tip().height < switch - 1 {
            advance(&mut chain, Vec::new(), Vec::new()).unwrap();
            vote(&mut chain);
        }

        for height in [1, chain.tip().height] {
            assert_eq!(chain.consensus_config_at(height).unwrap(), genesis_with(None).consensus, "the authority through height {height}");
        }

        assert_eq!(chain.consensus_config_at(switch).unwrap(), staked);

        // The authority may not produce the switch block; a validator in turn may, and the chain
        // weighs its blocks as the new rules say.
        let work = chain.tip_work();

        assert!(matches!(advance(&mut chain, Vec::new(), Vec::new()), Err(ImportError::Invalid(BlockError::Consensus(ConsensusError::WrongProducer)))));

        for _ in 0..2 * INTERVAL {
            produce_in_turn(&mut chain, rules, Vec::new());
            vote(&mut chain);
        }

        assert_eq!(chain.tip().height, switch + 2 * INTERVAL - 1);
        assert_eq!(chain.tip_work(), work + u256::new(2 * 2 * u128::from(INTERVAL)), "in turn, each weighs 2");
        assert!(matches!(&send_as_staked(&mut chain, rules, &alice, approve(1, to_staked())).result, Err(failure) if failure.to_string().contains("decided already")));

        // The checkpoints up to the one before the tip's are justified, so a light client learns
        // every epoch's consensus from their updates, and follows the chain across the switch.
        assert_eq!(chain.justified().unwrap().epoch + 1, chain.tip().height / INTERVAL);

        let genesis = chain.block_at(0).unwrap().unwrap();
        let params = Genesis {
            governance: Some(GovernanceConfig::Electorate { approval_epochs: 8 }),
            ..genesis_with(Some(ffg()))
        }
        .light_params();

        assert_eq!(params.governance, Some(crate::governance::GovernanceKind::Electorate));

        let mut client = LightClient::new(params, genesis.sealed_header(), chain.genesis()).unwrap();
        let headers_from = |from: u64| -> Vec<SealedHeader> { chain.headers_from(from, 1_000).unwrap() };
        let steps = |from: u64| stored_light_finality(&*chain.db, INTERVAL, from, 64, chain.tip().height).unwrap();

        // Under governance even the first epoch's rules come from genesis's update.
        assert_eq!(client.deliver_headers(1, headers_from(1), LATER).unwrap(), Extended::Waiting);
        assert_eq!(client.consensus_at(1), None);

        while let Some(request) = client.next() {
            match request {
                LightRequest::Headers { from, .. } => {
                    client.deliver_headers(from, headers_from(from), LATER).unwrap();
                }
                LightRequest::Finality { from } => {
                    client.deliver_finality(from, steps(from)).unwrap();
                }
            }
        }

        assert_eq!(client.tip().id(), chain.tip().id());
        assert_eq!(client.consensus_at(1), Some(&genesis_with(None).consensus));
        assert_eq!(client.consensus_at(switch), Some(&staked));

        // Saved and restored, it keeps the eras it needs and follows on.
        let restored = LightClient::restore(client.save(), chain.genesis()).unwrap();

        assert_eq!(restored.consensus_at(chain.tip().height), Some(&staked));
        assert_eq!(restored.tip().id(), chain.tip().id());
    }

    /// Sends a transaction in the next block produced by stake.
    #[cfg(all(feature = "finality-ffg", feature = "governance-electorate"))]
    fn send_as_staked(chain: &mut Chain, rules: crate::consensus::Staked, keypair: &Keypair, tx: Tx) -> Receipt {
        let signed = SignedTx::sign(tx, nonce(chain, keypair), FeeOffer::NONE, keypair);
        let height = chain.tip().height + 1;
        let producers = chain.producers_for(height).unwrap();
        let producer = validator_key(&rules.producer_at(&producers, height, 0).unwrap().voter);

        match produce_by_stake(chain, rules, &producer, vec![signed], None).unwrap() {
            Imported::Extended(mut receipts) => receipts.remove(0),
            other => panic!("expected the tip to be extended, got {other:?}"),
        }
    }

    #[cfg(all(feature = "finality-ffg", feature = "governance-council"))]
    #[test]
    fn a_council_decides_alone_and_an_approval_is_withdrawn_or_moved() {
        use crate::governance::{Change, GovernanceConfig};
        use crate::registry::changes::Changes;

        let council = GovernanceConfig::Council {
            members: vec![dev().public().address(), key(70).public().address()],
            threshold: 2,
            approval_epochs: 8,
        };
        let mut chain = governed(council);
        let other = Change::Consensus(ConsensusConfig::Staked {
            slot_ms: 2 * SLOT,
            schedule: crate::consensus::Schedule::Priority,
        });

        // Nothing to withdraw yet; one of two approves; moving it leaves the first change empty.
        assert!(matches!(&send_as(&mut chain, &dev(), Tx::WithdrawChange { kind: crate::governance::ChangeKind::Consensus, version: 0 }).result, Err(failure) if failure.to_string().contains("no approval")));
        assert_eq!(send_as(&mut chain, &dev(), Tx::ApproveChange { version: 0, change: to_staked() }).result, Ok(Effect::Done));
        assert_eq!(send_as(&mut chain, &dev(), Tx::ApproveChange { version: 0, change: other.clone() }).result, Ok(Effect::Done));

        let ballot = Changes::new(&chain.state()).ballot(crate::governance::ChangeKind::Consensus).unwrap();

        assert!(ballot.approvers(&to_staked()).is_empty());
        assert_eq!(ballot.approvers(&other), [dev().public().address()]);
        assert_eq!(send_as(&mut chain, &dev(), Tx::WithdrawChange { kind: crate::governance::ChangeKind::Consensus, version: 0 }).result, Ok(Effect::Done));
        assert!(Changes::new(&chain.state()).ballot(crate::governance::ChangeKind::Consensus).unwrap().approvals.is_empty());

        // A validator's account is no council member here, and a stale version is refused.
        let [alice, ..] = voters();

        assert!(matches!(&send_as(&mut chain, &alice, Tx::ApproveChange { version: 0, change: to_staked() }).result, Err(failure) if failure.to_string().contains("governance lets approve")));
        assert!(matches!(&send_as(&mut chain, &dev(), Tx::ApproveChange { version: 1, change: to_staked() }).result, Err(failure) if failure.to_string().contains("decided version is 0")));
        assert_eq!(Changes::new(&chain.state()).decided().unwrap().version, 0);
    }

    #[cfg(all(feature = "finality-ffg", feature = "governance-council", feature = "names"))]
    #[test]
    fn a_council_changes_what_names_cost_from_the_block_that_decides_it() {
        use crate::governance::{Change, GovernanceConfig};
        use crate::registry::changes::Changes;
        use crate::registry::names::{NameAction, NamesConfig};
        use crate::structs::u256::u256;

        let council = GovernanceConfig::Council {
            members: vec![dev().public().address(), key(70).public().address()],
            threshold: 2,
            approval_epochs: 8,
        };
        let mut chain = governed(council);
        let prices = Change::NamePrices(vec![u256::from(7u64)]);
        let register = |name: &str| Tx::Name { name: name.to_string(), action: NameAction::Register { years: 2 } };
        let balance = |chain: &Chain| crate::registry::Accounts::new(&chain.state()).get(&dev().public().address()).unwrap().balance;
        let paid = |chain: &mut Chain, name: &str| {
            let before = balance(chain);

            assert_eq!(send_as(chain, &dev(), register(name)).result, Ok(Effect::Done));

            before - balance(chain)
        };

        assert_eq!(paid(&mut chain, "alice"), NamesConfig::dev().prices[2] * u256::from(2u64), "genesis's price for five characters");

        // A price list no chain could have is refused; one member is not enough; two decide.
        assert!(matches!(&send_as(&mut chain, &dev(), Tx::ApproveChange { version: 0, change: Change::NamePrices(Vec::new()) }).result, Err(failure) if failure.to_string().contains("at least one price")));
        assert_eq!(send_as(&mut chain, &dev(), Tx::ApproveChange { version: 0, change: prices.clone() }).result, Ok(Effect::Done));
        assert_eq!(paid(&mut chain, "bobby"), NamesConfig::dev().prices[2] * u256::from(2u64), "not decided yet");
        assert_eq!(send_as(&mut chain, &key(70), Tx::ApproveChange { version: 0, change: prices.clone() }).result, Ok(Effect::Decided { decision: 1, kind: crate::governance::ChangeKind::NamePrices, version: 1 }));
        assert_eq!(Changes::new(&chain.state()).name_prices().unwrap(), Some(vec![u256::from(7u64)]));

        // From the next block on, one price for every length, the shortest too.
        assert_eq!(paid(&mut chain, "car"), u256::from(14u64));
        assert!(matches!(&send_as(&mut chain, &dev(), Tx::ApproveChange { version: 1, change: prices }).result, Err(failure) if failure.to_string().contains("decided already")));
    }

    #[cfg(all(feature = "finality-ffg", feature = "governance-council"))]
    #[test]
    fn a_council_changes_the_reward_limits_and_fees_from_the_next_block() {
        use crate::chain::fees::FeeConfig;
        use crate::governance::{Change, GovernanceConfig};

        let council = GovernanceConfig::Council {
            members: vec![dev().public().address(), key(70).public().address()],
            threshold: 2,
            approval_epochs: 8,
        };
        let mut chain = governed(council);
        let decide = |chain: &mut Chain, _: u64, change: Change| {
            let (version, decided) = deciding(chain, &change);

            for (member, effect) in [(dev(), Effect::Done), (key(70), decided)] {
                assert_eq!(send_as(chain, &member, Tx::ApproveChange { version, change: change.clone() }).result, Ok(effect));
            }
        };
        let refused = |chain: &mut Chain, change: Change, why: &str| {
            assert!(matches!(&send_as(chain, &dev(), Tx::ApproveChange { version: 0, change }).result, Err(failure) if failure.to_string().contains(why)), "{why}");
        };

        // What could not run a chain is refused before anyone counts it.
        refused(&mut chain, Change::Reward(Reward { initial: u256::new(1), halving_interval: Some(0) }), "halves every 1 block");
        refused(&mut chain, Change::Limits(BlockLimits { tx_limits: Limits { fuel: 10, memory: 1 << 20 }, max_block_bytes: None }), "100,000 fuel");
        refused(&mut chain, Change::Fees(FeeConfig { block_gas_limit: 0, ..FeeConfig::dev() }), "block_gas_limit");

        // A reward of 1,000 halving every two blocks, counted from the block after the decision.
        let producer = dev().public().address();
        decide(&mut chain, 0, Change::Reward(Reward { initial: u256::new(1_000), halving_interval: Some(2) }));
        let mut paid = Vec::new();

        for _ in 0..4 {
            let before = balance(&chain, &producer);
            advance(&mut chain, Vec::new(), Vec::new()).unwrap();
            paid.push(balance(&chain, &producer) - before);
        }

        assert_eq!(paid, [u256::new(1_000), u256::new(1_000), u256::new(500), u256::new(500)]);

        // Limits: the next block's, and what the node admits by.
        let limits = BlockLimits { tx_limits: Limits { fuel: 200_000, memory: 2 << 20 }, max_block_bytes: Some(2 << 20) };
        decide(&mut chain, 1, Change::Limits(limits));

        assert_eq!((chain.next.rules.limits, chain.max_block_bytes()), (limits.tx_limits, limits.max_block_bytes));

        // Fees, on a chain that had none: the next block charges, at the genesis price.
        assert!(chain.fees().is_none());
        decide(&mut chain, 2, Change::Fees(FeeConfig::dev()));

        assert_eq!(chain.fees().map(|fees| fees.block_gas_limit()), Some(FeeConfig::dev().block_gas_limit));
        assert!(chain.next_base_price() > u256::ZERO, "priced from the next block on");
        // Reopened, the node reads them all again from state.
        let reopened = Chain::open(chain.db().clone(), &Genesis { governance: chain_governance(), ..genesis_with(Some(ffg())) }).unwrap();
        assert_eq!((reopened.fees().is_some(), reopened.max_block_bytes()), (true, limits.max_block_bytes));

        assert_eq!(crate::registry::supply::adds_up(&chain.state()).unwrap(), Ok(()), "minted less burned is what is held and bonded");
    }

    #[cfg(all(feature = "finality-ffg", feature = "governance-council"))]
    fn chain_governance() -> Option<crate::governance::GovernanceConfig> {
        Some(crate::governance::GovernanceConfig::Council {
            members: vec![dev().public().address(), key(70).public().address()],
            threshold: 2,
            approval_epochs: 8,
        })
    }

    #[cfg(all(feature = "finality-ffg", feature = "governance-council"))]
    #[test]
    fn a_council_retunes_finality_from_an_epoch_far_enough_ahead() {
        use crate::governance::Change;
        use crate::registry::changes::Changes;

        let mut chain = governed(chain_governance().unwrap());
        let FinalityConfig::Ffg(genesis) = ffg() else { unreachable!("ffg") };
        let tuned = FinalityConfig::Ffg(FfgConfig { min_bond: u256::new(500), max_proofs_per_block: 2, ..genesis.clone() });
        let refused = |chain: &mut Chain, change: Change, why: &str| {
            assert!(matches!(&send_as(chain, &dev(), Tx::ApproveChange { version: 0, change }).result, Err(failure) if failure.to_string().contains(why)), "{why}");
        };

        // What the epochs, the fixed-ahead sets and the history rest on stays.
        refused(&mut chain, Change::Finality(FinalityConfig::Ffg(FfgConfig { checkpoint_interval: INTERVAL + 1, ..genesis.clone() })), "keeps checkpoint_interval");
        refused(&mut chain, Change::Finality(FinalityConfig::Ffg(FfgConfig { validators: Vec::new(), ..genesis.clone() })), "keeps checkpoint_interval");

        for (member, effect) in [(dev(), Effect::Done), (key(70), Effect::Decided { decision: 1, kind: crate::governance::ChangeKind::Consensus, version: 1 })] {
            assert_eq!(send_as(&mut chain, &member, Tx::ApproveChange { version: 0, change: Change::Finality(tuned.clone()) }).result, Ok(effect));
        }

        // Decided in this epoch, it stands from activation_delay + 1 epochs on, not before.
        let decided = chain.tip().height / INTERVAL;
        let from = decided + 2 + 1;
        let state = chain.state();

        assert_eq!(Changes::new(&state).finality_at(from - 1).unwrap(), None);
        assert_eq!(Changes::new(&state).finality_at(from).unwrap(), Some(tuned.clone()));
        let min_bond = |chain: &Chain, height: u64| chain.params_in(&chain.state(), height).unwrap().rules.staking.unwrap().staking.min_bond;

        assert_eq!(min_bond(&chain, from * INTERVAL - 1), u256::new(100));
        assert_eq!(min_bond(&chain, from * INTERVAL), u256::new(500));

        // At that epoch the chain runs with it: blocks take at most two proofs now.
        grow_to(&mut chain, from * INTERVAL - 1);
        assert_eq!(chain.next.rules.staking.as_ref().map(|staking| staking.staking.min_bond), Some(u256::new(500)));
    }

    #[cfg(all(feature = "finality-ffg", feature = "governance-council", feature = "secp256k1"))]
    #[test]
    fn a_council_names_the_federation_and_each_one_counts_up_a_version() {
        use crate::governance::{Change, GovernanceConfig};
        use crate::registry::changes::Changes;
        use crate::registry::federation::Federation;

        let council = GovernanceConfig::Council {
            members: vec![dev().public().address(), key(70).public().address()],
            threshold: 2,
            approval_epochs: 8,
        };
        let mut chain = governed(council);
        let federation = |threshold: u32, keys: &[u8]| Federation {
            signers: keys.iter().map(|byte| crate::crypto::secp256k1::public_key(&[*byte; 32]).unwrap()).collect(),
            threshold,
            chain_depth: 20,
            evm_depth: 12,
            bitcoin_depth: 6,
        };
        let decide = |chain: &mut Chain, _: u64, change: Change| {
            let (version, decided) = deciding(chain, &change);

            for (member, effect) in [(dev(), Effect::Done), (key(70), decided)] {
                assert_eq!(send_as(chain, &member, Tx::ApproveChange { version, change: change.clone() }).result, Ok(effect));
            }
        };

        assert_eq!(Changes::new(&chain.state()).federation().unwrap(), None);

        // One that could not sign is refused; two members decide one, then another.
        let broken = Change::Federation(federation(3, &[1, 2]));

        assert!(matches!(&send_as(&mut chain, &dev(), Tx::ApproveChange { version: 0, change: broken }).result, Err(failure) if failure.to_string().contains("threshold")));
        decide(&mut chain, 0, Change::Federation(federation(2, &[1, 2, 3])));
        let first = Changes::new(&chain.state()).federation().unwrap().unwrap();

        assert_eq!((first.version, first.federation.threshold), (1, 2));
        decide(&mut chain, 1, Change::Federation(federation(3, &[1, 2, 3, 4])));
        assert_eq!(Changes::new(&chain.state()).federation().unwrap().map(|decided| decided.version), Some(2));
        assert!(matches!(&send_as(&mut chain, &dev(), Tx::ApproveChange { version: 2, change: Change::Federation(federation(3, &[1, 2, 3, 4])) }).result, Err(failure) if failure.to_string().contains("decided already")));
    }

    #[cfg(all(feature = "finality-ffg", feature = "governance-council", feature = "secp256k1"))]
    #[test]
    fn a_council_decides_the_directory_without_touching_the_federation() {
        use crate::governance::{Change, GovernanceConfig};
        use crate::registry::directory::{Directory, EvmGateway};
        use crate::registry::changes::Changes;

        let council = GovernanceConfig::Council {
            members: vec![dev().public().address(), key(70).public().address()],
            threshold: 2,
            approval_epochs: 8,
        };
        let mut chain = governed(council);
        let directory = Directory { gateway: Some(Address::Contract([1; 32])), swap: Some(Address::Contract([2; 32])), evm_gateways: vec![EvmGateway { chain_id: 1, gateway: [3; 20] }], ..Directory::default() };

        assert_eq!(Changes::new(&chain.state()).directory().unwrap(), None);

        // A gateway that is no contract is refused; two members decide it.
        let broken = Change::Directory(Directory { gateway: Some(dev().public().address()), ..directory.clone() });

        assert!(matches!(&send_as(&mut chain, &dev(), Tx::ApproveChange { version: 0, change: broken }).result, Err(failure) if failure.to_string().contains("contracts")));

        for (member, effect) in [(dev(), Effect::Done), (key(70), Effect::Decided { decision: 1, kind: crate::governance::ChangeKind::Directory, version: 1 })] {
            assert_eq!(send_as(&mut chain, &member, Tx::ApproveChange { version: 0, change: Change::Directory(directory.clone()) }).result, Ok(effect));
        }

        assert_eq!(Changes::new(&chain.state()).directory().unwrap(), Some(directory.clone()));
        assert_eq!(Changes::new(&chain.state()).federation().unwrap(), None, "the federation's version never moves");
        assert!(matches!(&send_as(&mut chain, &dev(), Tx::ApproveChange { version: 1, change: Change::Directory(directory) }).result, Err(failure) if failure.to_string().contains("decided already")));
    }

    #[cfg(all(feature = "finality-ffg", feature = "governance-council"))]
    #[test]
    fn a_council_turns_the_collection_registry_on_for_a_chain_that_kept_none() {
        use crate::governance::{Change, GovernanceConfig};
        use crate::registry::changes::Changes;
        use crate::registry::collections::{CollectionFunctions, CollectionKind, Collections};
        use crate::structs::code::Code;

        let council = GovernanceConfig::Council {
            members: vec![dev().public().address(), key(70).public().address()],
            threshold: 2,
            approval_epochs: 8,
        };
        let genesis = Genesis {
            governance: Some(council),
            collections: None,
            ..genesis_with(Some(ffg()))
        };
        let mut chain = Chain::open(Arc::new(MemoryDb::default()), &genesis).unwrap();
        let deploy = |chain: &mut Chain| {
            let deployed = send_as(chain, &dev(), Tx::Deploy { code: Code::Lua(include_str!("../../../contracts/lua/nft.lua").to_string()) });
            let Ok(Effect::Deployed(address)) = deployed.result else {
                panic!("{:?}", deployed.result);
            };

            address
        };
        let standard = Change::Collections(CollectionFunctions::standard());
        let before = deploy(&mut chain);

        assert_eq!(Collections::new(&chain.state()).get(&before).unwrap(), None, "genesis kept no registry");

        // Lists no chain could have are refused; one member is not enough; two decide.
        let twice = Change::Collections(CollectionFunctions { nft: vec!["a".into(), "a".into()], multi: Vec::new() });

        assert!(matches!(&send_as(&mut chain, &dev(), Tx::ApproveChange { version: 0, change: twice }).result, Err(failure) if failure.to_string().contains("each function once")));
        assert_eq!(send_as(&mut chain, &dev(), Tx::ApproveChange { version: 0, change: standard.clone() }).result, Ok(Effect::Done));
        assert_eq!(send_as(&mut chain, &key(70), Tx::ApproveChange { version: 0, change: standard.clone() }).result, Ok(Effect::Decided { decision: 1, kind: crate::governance::ChangeKind::Collections, version: 1 }));
        assert_eq!(Changes::new(&chain.state()).collections().unwrap(), Some(CollectionFunctions::standard()));

        // Deploys register from then on; what came before waits for its next upgrade or refresh.
        let after = deploy(&mut chain);

        assert_eq!(Collections::new(&chain.state()).get(&after).unwrap().map(|found| found.kind), Some(CollectionKind::Nft));
        assert_eq!(Collections::new(&chain.state()).get(&before).unwrap(), None);
        assert!(matches!(&send_as(&mut chain, &dev(), Tx::ApproveChange { version: 1, change: standard }).result, Err(failure) if failure.to_string().contains("decided already")));
    }

    /// A chain the council governs, starting with this finality, and its genesis.
    #[cfg(all(feature = "finality-ffg", feature = "finality-authority", feature = "governance-council"))]
    fn council_chain(finality: FinalityConfig) -> (Chain, Genesis) {
        let genesis = Genesis {
            governance: chain_governance(),
            ..genesis_with(Some(finality))
        };

        (Chain::open(Arc::new(MemoryDb::default()), &genesis).unwrap(), genesis)
    }

    /// Both members approve a change, which decides it.
    #[cfg(all(feature = "finality-ffg", feature = "finality-authority", feature = "governance-council"))]
    fn council_decides(chain: &mut Chain, change: crate::governance::Change) {
        let (version, decided) = deciding(chain, &change);

        for (member, effect) in [(dev(), Effect::Done), (key(70), decided)] {
            assert_eq!(send_as(chain, &member, Tx::ApproveChange { version, change: change.clone() }).result, Ok(effect));
        }
    }

    #[cfg(all(feature = "finality-ffg", feature = "finality-authority", feature = "governance-council"))]
    fn refused_change(chain: &mut Chain, change: crate::governance::Change, why: &str) {
        let version = crate::registry::changes::Changes::new(&chain.state()).decided().unwrap().version;
        let receipt = send_as(chain, &dev(), Tx::ApproveChange { version, change });

        assert!(matches!(&receipt.result, Err(failure) if failure.to_string().contains(why)), "{why}: {:?}", receipt.result);
    }

    /// FFG for a switch into it: the test settings, naming no validators.
    #[cfg(all(feature = "finality-ffg", feature = "finality-authority", feature = "governance-council"))]
    fn ffg_to_bond_for() -> FinalityConfig {
        let FinalityConfig::Ffg(config) = ffg() else { unreachable!("ffg") };

        FinalityConfig::Ffg(FfgConfig { validators: Vec::new(), ..config })
    }

    #[cfg(all(feature = "finality-ffg", feature = "finality-authority", feature = "governance-council"))]
    #[test]
    fn a_council_switches_ffg_to_authority_and_the_validators_unbond_there() {
        use crate::governance::Change;
        use crate::light::LightClient;
        use crate::registry::changes::Changes;
        use crate::registry::signers::Signers;
        use crate::structs::amount::Amount;

        let (mut chain, genesis) = council_chain(ffg());
        let switch = |epoch: u64, config: FinalityConfig| Change::SwitchFinality { epoch, config };

        refused_change(&mut chain, switch(9, ffg()), "other kind");
        refused_change(&mut chain, switch(9, FinalityConfig::Authority(AuthorityConfig { checkpoint_interval: INTERVAL + 1, ..signers() })), "keeps genesis's checkpoint_interval");
        refused_change(&mut chain, switch(1, authority()), "at the earliest");

        // Decided in epoch 1, for epoch 5.
        const SWITCH: u64 = 5;
        council_decides(&mut chain, switch(SWITCH, authority()));
        let state = chain.state();

        assert_eq!(Changes::new(&state).switch_ahead(1, &ffg()).unwrap().map(|era| era.from), Some(SWITCH));
        refused_change(&mut chain, Change::Finality(ffg()), "has yet to take effect");
        refused_change(&mut chain, to_staked(), "has yet to take effect");

        // The same keys vote throughout: as validators to epoch 4, as signers from 5, the old
        // electorate's last justified checkpoint the new one's first source.
        finalize_through(&mut chain, SWITCH + 2);

        assert!(chain.finalized().unwrap().epoch > SWITCH, "finality carries on over the switch");
        let state = chain.state();
        let validators = crate::registry::validators::Validators::new(&state);

        assert!(validators.set(SWITCH - 1).unwrap().is_some());
        assert_eq!(validators.set(SWITCH).unwrap(), None, "an epoch has one kind's voters");
        assert!(Signers::new(&state).set(SWITCH).unwrap().is_some());
        assert_eq!(Changes::new(&state).finality_in(SWITCH, &ffg()).unwrap().name(), "authority");

        // Every validator is unbonding from the switch, withdrawable withdrawal_delay epochs on.
        let alice = &voters()[0];
        let record = validators.get(&alice.public()).unwrap().unwrap();

        assert_eq!((record.active_until, record.withdrawable_from), (Some(SWITCH), Some(SWITCH + 3)));
        let params = chain.params_in(&state, chain.tip().height + 1).unwrap().rules;

        assert!(params.signers.is_some());
        assert!(params.staking.is_some_and(|staking| !staking.bonding), "withdrawing and slashing stay, bonding closes");
        assert_eq!(send(&mut chain, Tx::Bond { voter: key(14).public(), amount: Amount::U128(100) })[0].result, Err(Failure::BondingClosed));

        // A double vote from an FFG epoch, reported under authority, still costs the bond: the
        // evidence is judged by the kind its epoch ran.
        let carol = &voters()[2];
        let honest = signed(&chain, carol, start(&chain), checkpoint(&chain, 2));
        let double = signed(&chain, carol, start(&chain), Checkpoint { epoch: 2, block: Hash::zero(chain.hash()) });

        assert_eq!(send(&mut chain, Tx::Slash { first: Box::new(honest), second: Box::new(double) })[0].result, Ok(Effect::Done));
        let record = crate::registry::validators::Validators::new(&chain.state()).get(&carol.public()).unwrap().unwrap();

        assert!(record.slashed && record.bond == u256::ZERO);
        assert!(Signers::new(&chain.state()).decided().unwrap().set.signers.iter().any(|signer| signer.key == carol.public()), "not thrown out as a signer");

        grow_to(&mut chain, (SWITCH + 3) * INTERVAL);
        let before = balance(&chain, &alice.public().address());

        assert_eq!(send_as(&mut chain, alice, Tx::Withdraw { voter: alice.public() }).result, Ok(Effect::Done));
        assert_eq!(balance(&chain, &alice.public().address()), before + u256::new(10));

        // A light client follows from the genesis id alone, the update that fixed the switch
        // epoch naming it switched, and refuses one that hides it.
        let header = stored_header_at(&*chain.db, 0).unwrap().unwrap();
        let mut client = LightClient::new(genesis.light_params(), header, chain.genesis()).unwrap();
        serve(&chain, &mut client);

        assert!(client.finalized().epoch > SWITCH);
        let fixing = stored_header_at(&*chain.db, (SWITCH - 2) * INTERVAL).unwrap().unwrap();
        let finality = genesis.light_params().finality.unwrap();
        let mut update = stored_light_update(&*chain.db, &fixing.id()).unwrap().unwrap();

        assert_eq!(update.switched, vec![SWITCH]);
        assert!(update.check(&fixing.id(), &finality, fixing.header.height, &fixing.header.state_root).is_ok());
        update.switched.clear();

        assert_eq!(update.check(&fixing.id(), &finality, fixing.header.height, &fixing.header.state_root).err(), Some(crate::light::LightError::BadProof));
    }

    #[cfg(all(feature = "finality-ffg", feature = "finality-authority", feature = "governance-council"))]
    #[test]
    fn a_council_switches_authority_to_ffg_once_validators_bond_for_it() {
        use crate::registry::changes::Changes;
        use crate::registry::signers::Signers;
        use crate::structs::amount::Amount;

        let (mut chain, _) = council_chain(authority());
        let bond = |chain: &mut Chain, voter: &Keypair| send(chain, Tx::Bond { voter: voter.public(), amount: Amount::U128(100) }).remove(0).result;

        assert_eq!(bond(&mut chain, &voters()[0]), Err(Failure::NoStaking), "no bonds under authority");
        refused_change(&mut chain, crate::governance::Change::SwitchFinality { epoch: 9, config: ffg() }, "names no validators");

        // Decided in epoch 1, for epoch 5: bonds from then on vote from it.
        const SWITCH: u64 = 5;
        council_decides(&mut chain, crate::governance::Change::SwitchFinality { epoch: SWITCH, config: ffg_to_bond_for() });

        for voter in &voters()[..2] {
            assert_eq!(bond(&mut chain, voter), Ok(Effect::Done));
        }

        finalize_through(&mut chain, SWITCH + 2);

        assert!(chain.finalized().unwrap().epoch > SWITCH);
        let state = chain.state();

        assert_eq!(crate::registry::validators::Validators::new(&state).set(SWITCH).unwrap().map(|set| set.members.len()), Some(2));
        assert_eq!(Signers::new(&state).set(SWITCH).unwrap(), None);
        assert_eq!(Changes::new(&state).finality_in(SWITCH, &authority()).unwrap().name(), "ffg");
        assert!(chain.params_in(&state, chain.tip().height + 1).unwrap().rules.signers.is_none());
        assert_eq!(send(&mut chain, Tx::ApproveSigners { version: 0, signers: Vec::new(), threshold: 1, change_threshold: 1 })[0].result, Err(Failure::NoSigners));
    }

    #[cfg(all(feature = "finality-ffg", feature = "finality-authority", feature = "governance-council"))]
    #[test]
    fn a_switch_into_ffg_nobody_bonded_for_in_time_lapses() {
        use crate::registry::changes::Changes;
        use crate::registry::signers::Signers;
        use crate::structs::amount::Amount;

        let (mut chain, _) = council_chain(authority());
        const SWITCH: u64 = 5;
        council_decides(&mut chain, crate::governance::Change::SwitchFinality { epoch: SWITCH, config: ffg_to_bond_for() });

        // A bond in the checkpoint that fixes the switch epoch's voters votes only from the epoch
        // after it: too late, so that checkpoint finds nobody and the switch lapses.
        let fixing = (SWITCH - 2) * INTERVAL;
        finalize_through(&mut chain, SWITCH - 3);
        grow_to(&mut chain, fixing - 1);
        let voter = key(14).public();
        let before = balance(&chain, &dev().public().address());

        assert_eq!(send(&mut chain, Tx::Bond { voter, amount: Amount::U128(100) })[0].result, Ok(Effect::Done));
        assert_eq!(chain.tip().height, fixing);
        let state = chain.state();
        let changes = Changes::new(&state);

        assert_eq!(changes.finality_at(SWITCH).unwrap(), None, "the switch lapsed");
        assert_eq!(changes.switch_ahead(SWITCH - 2, &authority()).unwrap(), None);
        assert!(Signers::new(&state).set(SWITCH).unwrap().is_some());
        assert_eq!(crate::registry::validators::Validators::new(&state).set(SWITCH).unwrap(), None);

        // The bond came back with the lapse, and the signers carry on.
        assert_eq!(crate::registry::validators::Validators::new(&state).get(&voter).unwrap(), None);
        assert_eq!(balance(&chain, &dev().public().address()), before);
        finalize_through(&mut chain, SWITCH + 1);

        assert!(chain.finalized().unwrap().epoch >= SWITCH);
    }

    #[cfg(feature = "finality-authority")]
    #[test]
    fn signers_cannot_stake_but_a_double_voter_is_thrown_out() {
        use crate::registry::signers::Signers;
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

        // Anyone may report her, here in epoch 2: she leaves the decided set, version 1, and
        // epoch 4's set, while epochs 2 and 3 keep her; the thresholds stay two of the two left.
        assert_eq!(send(&mut chain, slash.clone())[0].result, Ok(Effect::Done));

        for tx in [bond, Tx::Unbond { voter: carol.public() }] {
            assert_eq!(send(&mut chain, tx)[0].result, Err(Failure::NoStaking));
        }

        let decided = Signers::new(&chain.state()).decided().unwrap();
        let in_set = |epoch: u64| Signers::new(&chain.state()).set(epoch).unwrap().unwrap().keys().contains(&carol.public());

        assert_eq!(decided.version, 1);
        assert_eq!(decided.set.keys().len(), 2);
        assert!(!decided.set.keys().contains(&carol.public()));
        assert_eq!((decided.set.threshold, decided.set.change_threshold), (2, 2));
        assert!(in_set(2) && in_set(3) && !in_set(4));
        assert!(matches!(&send(&mut chain, slash)[0].result, Err(failure) if failure.to_string().contains("not one of the decided signers")));

        // Evidence whose signatures are for another chain is refused for what it is.
        let forged = Tx::Slash {
            first: Box::new(SignedVote::sign(honest.vote, carol, &Hash::zero(chain.hash()))),
            second: Box::new(double),
        };

        assert_eq!(send(&mut chain, forged)[0].result, Err(Failure::BadEvidence));

        // Two of two justify now: neither can go without leaving the threshold out of reach.
        grow_to(&mut chain, 3 * INTERVAL + 1);
        let bob = &voters()[1];
        let bob_double = Tx::Slash {
            first: Box::new(signed(&chain, bob, start(&chain), checkpoint(&chain, 3))),
            second: Box::new(signed(&chain, bob, start(&chain), Checkpoint { epoch: 3, block: Hash::zero(chain.hash()) })),
        };

        assert!(matches!(&send(&mut chain, bob_double)[0].result, Err(failure) if failure.to_string().contains("threshold")));
        assert_eq!(Signers::new(&chain.state()).decided().unwrap().version, 1);
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
            (AuthorityConfig { signers: vec![Signer::own(signer), Signer::own(signer)], threshold: 2, ..good.clone() }, "named twice"),
            (AuthorityConfig { signers: vec![Signer::own(signer), Signer { key: key(14).public(), account: signer.address() }], ..good.clone() }, "share an account"),
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
            signers: voters().iter().map(Keypair::public).chain([key(14).public()]).map(Signer::own).collect(),
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
                signers: vec![Signer::own(alice.public()), Signer::own(bob.public()), Signer::own(dave.public())],
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
        assert!(decided.set.keys().contains(&dave.public()) && !decided.set.keys().contains(&carol.public()));

        // Decided in epoch 0, fixed at epoch 1's checkpoint for epoch 1 + 2: until then the old
        // signers vote.
        finalize_through(&mut chain, 2);
        let fixed = |epoch: u64| Signers::new(&chain.state()).set(epoch).unwrap().unwrap();

        assert!(fixed(2).keys().contains(&carol.public()));
        assert!(fixed(3).keys().contains(&dave.public()));

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
            signers: vec![Signer::own(alice.public()), Signer::own(bob.public()), Signer::own(key(14).public())],
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

    /// The finality a node sends a light client from an epoch on, as GET /light/finality does.
    fn finality_from(chain: &Chain, from: u64) -> Vec<crate::light::FinalityStep> {
        crate::chain::stored_light_finality(&*chain.db, INTERVAL, from, crate::light::MAX_FINALITY_STEPS, chain.tip().height).unwrap()
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
                LightRequest::Finality { from } => client.deliver_finality(from, finality_from(chain, from)).map(|_| ()).unwrap(),
            }
        }
    }

    /// A light client of the chain, holding every header the chain has and nothing else.
    fn with_headers(chain: &Chain, finality: &FinalityConfig) -> crate::light::LightClient {
        use crate::light::{LightClient, LightRequest};

        let params = genesis_with(Some(finality.clone())).light_params();
        let genesis = stored_header_at(&*chain.db, 0).unwrap().unwrap();
        let mut client = LightClient::new(params, genesis, chain.genesis()).unwrap();

        while let Some(LightRequest::Headers { from, count }) = client.next() {
            let headers = (from..from + u64::from(count)).map_while(|height| stored_header_at(&*chain.db, height).unwrap()).collect();
            client.deliver_headers(from, headers, LATER).unwrap();
        }

        client
    }

    /// What GET /light/checkpoint sends for the main-chain checkpoint at this height.
    fn start_at(chain: &Chain, finality: &FinalityConfig, height: u64) -> crate::light::CheckpointStart {
        let params = genesis_with(Some(finality.clone())).light_params();
        let (_, from) = crate::light::checkpoint_start(params.finality, height).unwrap();
        let headers: Vec<_> = (from..=height).map(|at| stored_header_at(&*chain.db, at).unwrap().unwrap()).collect();
        let updates = headers.iter().step_by(INTERVAL as usize).map(|header| stored_light_update(&*chain.db, &header.id()).unwrap()).collect();

        crate::light::CheckpointStart { headers, updates }
    }

    #[test]
    fn a_client_far_behind_takes_up_the_chain_at_a_later_checkpoint_and_checks_voters_from_there() {
        use crate::light::{LightClient, LightError, LightRequest};

        for finality in kinds() {
            let mut chain = open(&finality);
            finalize_through(&mut chain, 4);
            let params = genesis_with(Some(finality.clone())).light_params();
            let genesis = stored_header_at(&*chain.db, 0).unwrap().unwrap();

            // Followed so far, then saved: what a wallet resumes from.
            let mut behind = LightClient::new(params.clone(), genesis.clone(), chain.genesis()).unwrap();
            serve(&chain, &mut behind);
            let saved = behind.save();
            let saved_at = behind.finalized();

            // Far further on, by the time it opens again.
            finalize_through(&mut chain, 12);
            let height = 8 * INTERVAL;
            let checkpoint = stored_header_at(&*chain.db, height).unwrap().unwrap().id();
            let start = start_at(&chain, &finality, height);

            // The genesis trusted still holds: a node on another chain gets nowhere.
            assert_eq!(
                LightClient::from_checkpoint(params.clone(), genesis.clone(), Hash::zero(chain.genesis().algorithm()), checkpoint, start.clone()).err(),
                Some(LightError::WrongGenesis),
                "{}",
                finality.name(),
            );

            let mut resumed = LightClient::from_checkpoint(params.clone(), genesis.clone(), chain.genesis(), checkpoint, start).unwrap();
            let restored = LightClient::restore(saved, chain.genesis()).unwrap();

            assert!(resumed.finalized().epoch > restored.finalized().epoch, "{}: further on than what it saved", finality.name());
            assert_eq!(restored.finalized(), saved_at);

            // It has yet to reach the tip, by headers each checked, and by justifications each
            // checked against the voters the checkpoint's updates fixed.
            assert!(matches!(resumed.next(), Some(LightRequest::Headers { .. })));
            serve(&chain, &mut resumed);

            assert_eq!(resumed.tip().id(), chain.tip().id());
            assert_eq!(Some(resumed.finalized()), chain.finalized());

            // So the same forged justification a client from genesis refuses, it refuses too.
            let mut forged = LightClient::from_checkpoint(params, genesis, chain.genesis(), checkpoint, start_at(&chain, &finality, height)).unwrap();
            let mut followed = forged.next();

            while let Some(LightRequest::Headers { from, count }) = followed {
                let headers = (from..from + u64::from(count)).map_while(|height| stored_header_at(&*chain.db, height).unwrap()).collect();
                forged.deliver_headers(from, headers, LATER).unwrap();
                followed = forged.next();
            }

            let next_epoch = forged.justified().epoch + 1;
            let target = stored_header_at(&*chain.db, next_epoch * INTERVAL).unwrap().unwrap().id();
            let mut justification = stored_justification(&*chain.db, &target).unwrap().expect("the chain justified it");
            justification.votes.truncate(1);

            assert!(forged.deliver_justifications(&[justification]).is_err() || forged.justified().epoch < next_epoch, "{}: one vote does not justify", finality.name());
        }
    }

    #[test]
    fn a_light_client_starts_at_a_checkpoint_it_trusts_and_follows_on_from_there() {
        use crate::light::LightClient;

        for finality in kinds() {
            let mut chain = open(&finality);
            finalize_through(&mut chain, 12);
            let params = genesis_with(Some(finality.clone())).light_params();
            let genesis = stored_header_at(&*chain.db, 0).unwrap().unwrap();
            let height = 8 * INTERVAL;
            let checkpoint = stored_header_at(&*chain.db, height).unwrap().unwrap().id();
            let start = start_at(&chain, &finality, height);
            let mut client = LightClient::from_checkpoint(params, genesis, chain.genesis(), checkpoint, start.clone()).unwrap();

            assert_eq!(client.lowest(), start.headers[0].header.height, "only the headers its voters' updates need");
            assert!(client.lowest() > 0, "{}: none from genesis", finality.name());
            assert_eq!((client.justified().epoch, client.finalized().block), (8, checkpoint));

            // From there it follows as any client: headers, then finality, to where the chain is.
            serve(&chain, &mut client);

            assert_eq!(client.tip().id(), chain.tip().id());
            assert_eq!(Some(client.justified()), chain.justified());
            assert_eq!(Some(client.finalized()), chain.finalized());

            // And it keeps and restores as one from genesis does.
            let restored = LightClient::restore(client.save(), chain.genesis()).unwrap();

            assert_eq!(restored.justified(), client.justified());
        }
    }

    #[test]
    fn a_checkpoint_start_that_does_not_lead_to_the_checkpoint_trusted_is_refused() {
        use crate::light::{LightClient, LightError};

        for finality in kinds() {
            let mut chain = open(&finality);
            finalize_through(&mut chain, 8);
            let params = genesis_with(Some(finality.clone())).light_params();
            let genesis = stored_header_at(&*chain.db, 0).unwrap().unwrap();
            let height = 6 * INTERVAL;
            let id = |height: u64| stored_header_at(&*chain.db, height).unwrap().unwrap().id();
            let start = start_at(&chain, &finality, height);
            let from = |checkpoint, start| LightClient::from_checkpoint(params.clone(), genesis.clone(), chain.genesis(), checkpoint, start).err();

            assert_eq!(from(id(height - INTERVAL), start.clone()), Some(LightError::WrongCheckpoint), "{}: another block trusted", finality.name());

            let mut late = start.clone();
            late.headers.drain(..INTERVAL as usize);
            late.updates.remove(0);

            assert_eq!(from(id(height), late), Some(LightError::WrongCheckpoint), "too late to prove the voters");

            let mut gap = start.clone();
            gap.headers.remove(2);

            assert_eq!(from(id(height), gap), Some(LightError::WrongCheckpoint), "a header missing");

            let mut forged = start.clone();
            forged.updates[0].as_mut().unwrap().voters[0].value.push(0);

            assert_eq!(from(id(height), forged), Some(LightError::BadProof));

            let mut past = start.clone();
            past.headers.push(stored_header_at(&*chain.db, height + 1).unwrap().unwrap());

            assert_eq!(from(id(height + 1), past), Some(LightError::WrongCheckpoint), "a block that opens no epoch");
            assert_eq!(from(id(height), start), None, "and the one sent holds");
        }
    }

    #[test]
    fn finality_comes_many_epochs_to_a_request() {
        use crate::light::{LightRequest, MAX_FINALITY_STEPS};

        for finality in kinds() {
            let mut chain = open(&finality);
            let last = 2 * MAX_FINALITY_STEPS + 5;
            finalize_through(&mut chain, last);
            let mut client = with_headers(&chain, &finality);
            let mut asked = 0;

            while let Some(request) = client.next() {
                let LightRequest::Finality { from } = request else {
                    panic!("it holds every header already");
                };

                asked += 1;
                client.deliver_finality(from, finality_from(&chain, from)).unwrap();
            }

            // Genesis and 133 epochs in three answers, where asking for each update on its own
            // took a request an epoch, and more for the justifications between them.
            assert_eq!(asked, (last + 1).div_ceil(MAX_FINALITY_STEPS), "{}", finality.name());
            assert_eq!(Some(client.justified()), chain.justified());
            assert_eq!(Some(client.finalized()), chain.finalized());
        }
    }

    #[test]
    fn finality_out_of_place_or_tampered_with_is_refused() {
        use crate::light::LightError;

        for finality in kinds() {
            let mut chain = open(&finality);
            finalize_through(&mut chain, 4);
            let steps = finality_from(&chain, 0);

            // Each step is the epoch it says: these are epoch 1 on, sent as from 2 on.
            let mut client = with_headers(&chain, &finality);
            client.deliver_finality(0, steps[..1].to_vec()).unwrap();

            assert_eq!(client.deliver_finality(2, steps[1..].to_vec()), Err(LightError::Misplaced));

            // Voters that are not what genesis kept.
            let mut forged = steps.clone();
            forged[0].update.as_mut().unwrap().voters[0].value.push(0);

            assert_eq!(with_headers(&chain, &finality).deliver_finality(0, forged), Err(LightError::BadProof));

            // A justification with too few votes behind it.
            let mut thin = steps.clone();
            thin[1].justification.as_mut().unwrap().votes.truncate(1);

            assert!(matches!(with_headers(&chain, &finality).deliver_finality(0, thin), Err(LightError::BadJustification(_))));

            // What does hold is taken, up to where the node justified no further.
            let mut client = with_headers(&chain, &finality);

            assert_eq!(client.deliver_finality(0, steps), Ok(4));
            assert_eq!(Some(client.justified()), chain.justified());
            assert_eq!(client.next(), None, "and it asks for nothing more");
        }
    }

    #[test]
    fn a_justification_past_the_headers_followed_waits_for_them() {
        use crate::light::{LightClient, LightRequest};

        for finality in kinds() {
            let mut chain = open(&finality);
            finalize_through(&mut chain, 3);
            let params = genesis_with(Some(finality.clone())).light_params();
            let genesis = stored_header_at(&*chain.db, 0).unwrap().unwrap();
            let mut client = LightClient::new(params, genesis, chain.genesis()).unwrap();
            serve(&chain, &mut client);

            // Caught up, it knows the voters of the epochs after the next already. The node sends
            // headers short of the checkpoint after the next, then justifies it before it is asked
            // for justifications, and sends them all.
            let justified = client.justified().epoch;
            let short = (justified + 2) * INTERVAL - 1;
            finalize_through(&mut chain, justified + 4);
            client.refresh();

            while let Some(request) = client.next() {
                match request {
                    LightRequest::Headers { from, count } => {
                        let headers = (from..(from + u64::from(count)).min(short + 1))
                            .map_while(|height| stored_header_at(&*chain.db, height).unwrap())
                            .collect();
                        client.deliver_headers(from, headers, LATER).unwrap();
                    }
                    LightRequest::Finality { from } => client.deliver_finality(from, finality_from(&chain, from)).map(|_| ()).unwrap(),
                }
            }

            assert_eq!(client.tip().height, short, "{}", finality.name());
            assert_eq!(client.justified().epoch, justified + 1, "it takes what it can place, and waits");

            // Once the headers come, so does the rest.
            client.refresh();
            serve(&chain, &mut client);

            assert_eq!(Some(client.justified()), chain.justified());
            assert_eq!(Some(client.finalized()), chain.finalized());
        }
    }

    #[test]
    fn a_light_client_on_a_branch_the_chain_left_goes_back_once_the_voters_justify_the_other() {
        for finality in kinds() {
            // Two copies of one chain, the same to its justified checkpoint at epoch 2; then one
            // skips a slot, so their next blocks differ, and only it is justified on.
            let (mut kept, mut left) = (open(&finality), open(&finality));

            for chain in [&mut kept, &mut left] {
                finalize_through(chain, 2);
            }

            assert_eq!(kept.tip().id(), left.tip().id());
            let later = kept.tip().timestamp + 2 * SLOT;
            let mut producer = AuthorityProducer::new(dev(), SLOT);
            let mut skipped = kept.build(Vec::new(), Vec::new(), producer.address(&[]), later).unwrap();
            skipped.seal = producer.seal(&skipped.header).unwrap();
            kept.import(&skipped, later).unwrap();
            finalize_through(&mut kept, 3);
            grow_to(&mut left, kept.tip().height);

            // The client followed the branch left, past epoch 3's checkpoint.
            let params = genesis_with(Some(finality.clone())).light_params();
            let genesis = stored_header_at(&*left.db, 0).unwrap().unwrap();
            let mut client = crate::light::LightClient::new(params, genesis, left.genesis()).unwrap();
            serve(&left, &mut client);

            assert_eq!(client.tip().id(), left.tip().id(), "{}", finality.name());
            assert_eq!(client.justified().epoch, 2);

            // The voters justify the other's checkpoint: no error, it goes back to its own
            // justified one, and follows the chain they chose from there.
            assert!(client.deliver_finality(3, finality_from(&kept, 3)).is_ok(), "{}", finality.name());
            assert_eq!(client.tip().height, 2 * INTERVAL, "back to its justified checkpoint");

            serve(&kept, &mut client);

            assert_eq!(client.tip().id(), kept.tip().id(), "{}", finality.name());
            assert_eq!(Some(client.justified()), kept.justified());
        }
    }

    #[test]
    fn a_light_client_whose_tip_the_chain_left_follows_it_again_from_its_justified_checkpoint() {
        for finality in kinds() {
            // As above, but the node only sends headers: the client's tip is on the branch left,
            // and the node's next headers do not link to it.
            let (mut kept, mut left) = (open(&finality), open(&finality));

            for chain in [&mut kept, &mut left] {
                finalize_through(chain, 2);
            }

            grow_to(&mut left, kept.tip().height + 3);
            let later = kept.tip().timestamp + 2 * SLOT;
            let mut producer = AuthorityProducer::new(dev(), SLOT);
            let mut skipped = kept.build(Vec::new(), Vec::new(), producer.address(&[]), later).unwrap();
            skipped.seal = producer.seal(&skipped.header).unwrap();
            kept.import(&skipped, later).unwrap();
            grow_to(&mut kept, left.tip().height + 4);

            let params = genesis_with(Some(finality.clone())).light_params();
            let genesis = stored_header_at(&*left.db, 0).unwrap().unwrap();
            let mut client = crate::light::LightClient::new(params, genesis, left.genesis()).unwrap();
            serve(&left, &mut client);

            assert_eq!(client.tip().id(), left.tip().id(), "{}", finality.name());

            // Served the other branch, it goes back to its justified checkpoint and follows it.
            client.refresh();
            serve(&kept, &mut client);

            assert_eq!(client.tip().id(), kept.tip().id(), "{}", finality.name());
            assert_eq!(client.justified().epoch, 2);
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
            signers: vec![Signer::own(key(11).public())],
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
