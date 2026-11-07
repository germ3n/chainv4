//! The restaking fork through the chain: before it a restake setting is no transaction and a bond
//! on a validator already bonded is refused; from it a validator's account turns restaking on, its
//! vote rewards go into its bond up to any cap, it tops its bond up by hand, and the attestations
//! of the justifications blocks carry are checked as gossiped ones are.

use {
    super::voting::{INTERVAL, VOTE_REWARD, checkpoint, finalize_through, honest_vote, nonce, voters},
    super::*,
    crate::params::finality::{FfgConfig, GenesisValidator},
    crate::params::forks::{ForkSchedule, RESTAKING, ScheduledFork},
    crate::registry::validators::{Restake, Validators},
    crate::structs::amount::Amount,
    crate::structs::tx::Tx,
};

/// FFG of the three voters, its validators attesting with these keys if given, and the restaking
/// fork from a height.
fn restaking_from(height: u64, attesters: Option<&[Keypair; 3]>) -> Chain {
    let FinalityConfig::Ffg(config) = super::voting::ffg() else { unreachable!() };
    let validators = voters()
        .iter()
        .enumerate()
        .map(|(at, keypair)| GenesisValidator {
            account: keypair.public().address(),
            voter: keypair.public(),
            bond: u256::new(10),
            attester: attesters.map(|keys| match keys[at].public().address() {
                Address::Secp256k1(bytes) => bytes,
                other => panic!("{other} is no Ethereum address"),
            }),
        })
        .collect();
    let genesis = Genesis {
        finality: Some(FinalityConfig::Ffg(FfgConfig { validators, ..config })),
        fees: None,
        forks: ForkSchedule { forks: vec![ScheduledFork { name: RESTAKING.name.to_string(), height, epoch: None, least: None }], lead: None },
        ..Genesis::dev_authority(dev().public())
    };

    Chain::open(Arc::new(MemoryDb::default()), &genesis).unwrap()
}

/// Sends one transaction from a voter's own account, in a block of its own.
fn send(chain: &mut Chain, from: &Keypair, tx: Tx) -> Vec<Receipt> {
    let signed = SignedTx::sign(tx, nonce(chain, from), FeeOffer::NONE, from, &chain.genesis());

    advance(chain, vec![signed], Vec::new()).unwrap()
}

fn bond(chain: &Chain, voter: &Keypair) -> u256 {
    Validators::new(&chain.state()).get(&voter.public()).unwrap().unwrap().bond
}

fn pay_unpaid(chain: &mut Chain) {
    let proofs = chain.unpaid_proofs().unwrap();
    assert!(!proofs.is_empty());
    advance(chain, Vec::new(), proofs).unwrap();
}

#[test]
fn from_the_fork_a_validator_restakes_its_rewards_up_to_its_cap_and_tops_its_bond_up() {
    let fork = 3 * INTERVAL;
    let mut chain = restaking_from(fork, None);
    let [alice, bob, _] = voters();
    let restake = |on: bool, cap: u64| Tx::SetRestake { voter: alice.public(), on, cap: u256::new(cap.into()) };

    // Before the fork: a restake setting is no transaction, and topping up is refused.
    assert!(send(&mut chain, &alice, restake(true, 0)).is_empty(), "left out of the block");
    let topped = send(&mut chain, &bob, Tx::Bond { voter: bob.public(), amount: Amount::U128(100) });
    assert!(topped[0].result.as_ref().is_err_and(|why| why.to_string().contains("already")), "{:?}", topped[0].result);

    grow_to(&mut chain, fork);
    assert!(send(&mut chain, &alice, restake(true, 0))[0].result.is_ok());
    assert_eq!(Validators::new(&chain.state()).restake(&alice.public()).unwrap(), Some(Restake { on: true, cap: u256::ZERO }));

    // Alice's reward goes into her bond, Bob's to his account.
    let reward = u256::new(VOTE_REWARD.into());
    let (alice_bond, bob_balance) = (bond(&chain, &alice), super::voting::balance(&chain, &bob.public().address()));
    let alice_balance = super::voting::balance(&chain, &alice.public().address());
    let next = chain.justified().unwrap().epoch + 1;
    finalize_through(&mut chain, next);
    pay_unpaid(&mut chain);

    assert_eq!(bond(&chain, &alice), alice_bond + reward);
    assert_eq!(super::voting::balance(&chain, &alice.public().address()), alice_balance);
    assert_eq!(super::voting::balance(&chain, &bob.public().address()), bob_balance + reward);

    // Up to a cap: 3 more into the bond, the rest to the account.
    let capped = bond(&chain, &alice) + u256::new(3);
    assert!(send(&mut chain, &alice, Tx::SetRestake { voter: alice.public(), on: true, cap: capped })[0].result.is_ok());
    let before = super::voting::balance(&chain, &alice.public().address());
    let next = chain.justified().unwrap().epoch + 1;
    finalize_through(&mut chain, next);
    pay_unpaid(&mut chain);

    assert_eq!(bond(&chain, &alice), capped);
    assert_eq!(super::voting::balance(&chain, &alice.public().address()), before + reward - u256::new(3));

    // Topping up by hand, from the account that bonded it, and only that one.
    let bob_bond = bond(&chain, &bob);
    assert!(send(&mut chain, &bob, Tx::Bond { voter: bob.public(), amount: Amount::U128(2) })[0].result.is_ok());
    assert_eq!(bond(&chain, &bob), bob_bond + u256::new(2));
    assert!(send(&mut chain, &alice, Tx::Bond { voter: bob.public(), amount: Amount::U128(1) })[0].result.is_err(), "not Alice's to top up");

    // The bond weighs in the sets fixed after.
    let later = chain.justified().unwrap().epoch + 3;
    finalize_through(&mut chain, later);
    let set = Validators::new(&chain.state()).set(later).unwrap().unwrap();
    assert_eq!(set.member(&bob.public()).unwrap().weight, bond(&chain, &bob));
}

#[cfg(feature = "secp256k1")]
#[test]
fn from_the_fork_a_block_whose_justification_carries_another_keys_attestation_is_invalid() {
    use crate::crypto::keypair::PrivateKey;

    let attesters = [41u8, 42, 43].map(|seed| Keypair::from_private(PrivateKey::Secp256k1([seed; 32])));
    let wrong = [9u8; 32];
    let fork = 3 * INTERVAL;
    let mut chain = restaking_from(fork, Some(&attesters));
    let voters = voters();

    // Votes whose attestations are another key's, put in a block's justification directly.
    let forged = |chain: &mut Chain, epoch: u64| {
        grow_to(chain, epoch * INTERVAL + 1);
        let mut votes: Vec<SignedVote> = voters[..2].iter().map(|voter| honest_vote(chain, voter)).collect();
        let link = chain.attested_link(votes[0].vote.source, votes[0].vote.target).unwrap().unwrap();

        for vote in &mut votes {
            vote.attestation = Some(link.sign(&wrong));
        }

        votes.sort_by_key(|vote| vote.vote.voter);

        Justification { source: votes[0].vote.source, target: votes[0].vote.target, votes }
    };

    // Before the fork the block holding it is taken, as the release before the fork takes it.
    let before = forged(&mut chain, 1);
    assert!(advance(&mut chain, Vec::new(), vec![before]).is_ok());

    // From it, the same is refused.
    let after = forged(&mut chain, (fork / INTERVAL) + 1);
    assert!(chain.tip().height >= fork);
    assert!(matches!(
        advance(&mut chain, Vec::new(), vec![after.clone()]),
        Err(ImportError::Invalid(BlockError::Proof(why))) if why.contains("not its attester's")
    ));

    // With each validator's own attester's attestation, it is taken.
    let link = chain.attested_link(after.source, after.target).unwrap().unwrap();
    let mut honest = after;

    for vote in &mut honest.votes {
        let at = voters.iter().position(|voter| voter.public() == vote.vote.voter).unwrap();
        let PrivateKey::Secp256k1(secret) = attesters[at].private() else { unreachable!() };
        vote.attestation = Some(link.sign(secret));
    }

    assert!(advance(&mut chain, Vec::new(), vec![honest]).is_ok());
    let _ = checkpoint;
}
