//! Native multisig accounts through the chain: one made by a transaction, its transactions run on
//! members' signatures, approvals on chain, or both, refused before they run when they fall short,
//! and its members changed by its own quorum.

use {
    super::*,
    crate::registry::multisig::{MultisigBody, MultisigMember, MultisigPolicy, Multisigs},
    crate::structs::address::multisig_address,
    crate::structs::tx::MemberSignature,
};

fn members() -> [Keypair; 3] {
    [Keypair::from_seed([31; 32]), Keypair::from_seed([32; 32]), Keypair::from_seed([33; 32])]
}

fn member(keypair: &Keypair) -> MultisigMember {
    MultisigMember { key: keypair.public(), bitcoin: None, evm: None }
}

/// Makes a 2-of-3 multisig of members(), funded with 1,000 from the authority.
fn two_of_three(chain: &mut Chain) -> Address {
    let creator = authority();
    let at = nonce(chain, &creator);
    let create = Tx::CreateMultisig { members: members().iter().map(member).collect(), threshold: 2 };
    let receipts = advance(chain, vec![SignedTx::sign(create, at, FeeOffer::NONE, &creator)]);
    let account = multisig_address(&creator.public().address(), at, chain.hash());

    assert_eq!(receipts[0].result, Ok(Effect::Deployed(account)));
    advance(chain, vec![transfer(chain, &creator, &account, 1_000)]);

    account
}

fn multisig_nonce(chain: &Chain, account: &Address) -> u64 {
    Accounts::new(&chain.state()).get(account).unwrap().nonce
}

/// The multisig pays someone, signed by these members, carried by the first of them.
fn pay(chain: &Chain, account: &Address, to: &Address, amount: u128, signers: &[&Keypair]) -> SignedTx {
    let tx = Tx::Transfer { from: *account, to: *to, amount: Amount::U128(amount) };
    let body = MultisigBody { account: *account, tx: tx.clone(), nonce: multisig_nonce(chain, account), fee: FeeOffer::NONE };
    let signatures = signers.iter().map(|keypair| MemberSignature::sign(&body, keypair, &chain.genesis())).collect();
    let carrier = signers.first().copied().cloned().unwrap_or_else(stranger);

    SignedTx::sign(Tx::Multisig { account: *account, tx: Box::new(tx), signatures }, body.nonce, FeeOffer::NONE, &carrier)
}

fn refused(chain: &mut Chain, tx: SignedTx, why: &str) {
    let (block, now) = forced_block(chain, vec![tx]);

    match chain.import(&block, now) {
        Err(ImportError::Invalid(BlockError::Tx { error, .. })) => assert!(error.to_string().contains(why), "{why}: {error}"),
        other => panic!("{why}: expected the block refused, got {other:?}"),
    }
}

#[test]
fn a_multisig_pays_on_its_members_signatures_and_never_on_too_few() {
    let mut chain = open(&Arc::new(MemoryDb::default()));
    let account = two_of_three(&mut chain);
    let [alice, bob, carol] = members();
    let to = Address::Ed25519([9; 32]);

    // One signature is not two; a stranger's is no member's; one for another chain does not hold.
    let sent = pay(&chain, &account, &to, 10, &[&alice]);
    refused(&mut chain, sent, "1 of the multisig's members back it, and it needs 2");
    let sent = pay(&chain, &account, &to, 10, &[&alice, &stranger()]);
    refused(&mut chain, sent, "not one of the multisig's members");
    let tx = Tx::Transfer { from: account, to, amount: Amount::U128(10) };
    let body = MultisigBody { account, tx: tx.clone(), nonce: 0, fee: FeeOffer::NONE };
    let elsewhere = Hash::zero(chain.hash());
    let signatures = [&alice, &bob].iter().map(|keypair| MemberSignature::sign(&body, keypair, &elsewhere)).collect();
    refused(&mut chain, SignedTx::sign(Tx::Multisig { account, tx: Box::new(tx), signatures }, 0, FeeOffer::NONE, &alice), "does not check out");

    // None of those took its nonce; two of three pay, carried by anyone, the multisig the sender.
    assert_eq!(multisig_nonce(&chain, &account), 0);
    let sent = pay(&chain, &account, &to, 10, &[&bob, &carol]);
    let receipts = advance(&mut chain, vec![sent]);

    assert_eq!(receipts[0].result, Ok(Effect::Done));
    assert_eq!((balance(&chain, &account), balance(&chain, &to)), (u256::new(990), u256::new(10)));
    assert_eq!(multisig_nonce(&chain, &account), 1);
    assert_eq!(nonce(&chain, &bob), 0, "the carrier's nonce stays");

    // A multisig's own transaction never moves another's coin.
    let theft = Tx::Transfer { from: authority().public().address(), to, amount: Amount::U128(1) };
    let body = MultisigBody { account, tx: theft.clone(), nonce: 1, fee: FeeOffer::NONE };
    let signatures = [&alice, &bob].iter().map(|keypair| MemberSignature::sign(&body, keypair, &chain.genesis())).collect();
    let signed = SignedTx::sign(Tx::Multisig { account, tx: Box::new(theft), signatures }, 1, FeeOffer::NONE, &alice);

    assert_eq!(signed.verify_on(&chain.genesis()), Err(crate::crypto::verifiable::VerifyError::NotSender));
}

#[test]
fn approvals_on_chain_count_with_signatures_until_the_nonce_moves() {
    let mut chain = open(&Arc::new(MemoryDb::default()));
    let account = two_of_three(&mut chain);
    let [alice, bob, carol] = members();
    let to = Address::Ed25519([9; 32]);
    let tx = Tx::Transfer { from: account, to, amount: Amount::U128(25) };
    let approve = |chain: &Chain, keypair: &Keypair, tx: &Tx, nonce: u64| {
        let approval = Tx::ApproveMultisig { account, nonce, fee: FeeOffer::NONE, tx: Box::new(tx.clone()) };

        SignedTx::sign(approval, self::nonce(chain, keypair), FeeOffer::NONE, keypair)
    };

    // Not a member, or not the next nonce: refused as it runs.
    let sent = approve(&chain, &authority(), &tx, 0);
    let strangers = advance(&mut chain, vec![sent]);
    let sent = approve(&chain, &alice, &tx, 5);
    let later = advance(&mut chain, vec![sent]);

    assert!(matches!(&strangers[0].result, Err(failure) if failure.to_string().contains("not one of the multisig's members")));
    assert!(matches!(&later[0].result, Err(failure) if failure.to_string().contains("next nonce is 0")));

    // Alice approves on chain and Bob signs: together, two of three.
    let sent = approve(&chain, &alice, &tx, 0);
    advance(&mut chain, vec![sent]);
    let id = MultisigBody { account, tx: tx.clone(), nonce: 0, fee: FeeOffer::NONE }.id(&chain.genesis());
    let open = Multisigs::new(&chain.state()).proposals(&account, 0).unwrap();

    assert_eq!((open.proposals.len(), open.proposals[0].id, open.proposals[0].approvals.len()), (1, id, 1));
    let sent = pay(&chain, &account, &to, 25, &[&bob]);
    advance(&mut chain, vec![sent]);

    assert_eq!(balance(&chain, &to), u256::new(25));
    assert!(Multisigs::new(&chain.state()).proposals(&account, 1).unwrap().proposals.is_empty(), "the nonce moved: void");

    // Two approvals on chain alone are enough; one withdrawn is not.
    let next = Tx::Transfer { from: account, to, amount: Amount::U128(5) };
    let sent = approve(&chain, &alice, &next, 1);
    advance(&mut chain, vec![sent]);
    let sent = approve(&chain, &carol, &next, 1);
    advance(&mut chain, vec![sent]);
    let id = MultisigBody { account, tx: next.clone(), nonce: 1, fee: FeeOffer::NONE }.id(&chain.genesis());
    let withdraw = SignedTx::sign(Tx::WithdrawMultisig { account, proposal: id }, nonce(&chain, &carol), FeeOffer::NONE, &carol);
    advance(&mut chain, vec![withdraw]);

    let sent = pay(&chain, &account, &to, 5, &[]);
    refused(&mut chain, sent, "1 of the multisig's members back it");
    let sent = approve(&chain, &bob, &next, 1);
    advance(&mut chain, vec![sent]);
    let sent = pay(&chain, &account, &to, 5, &[]);
    let receipts = advance(&mut chain, vec![sent]);

    assert_eq!(receipts[0].result, Ok(Effect::Done));
    assert_eq!(balance(&chain, &to), u256::new(30));
}

#[test]
fn a_multisig_changes_its_members_by_its_own_quorum_and_keeps_its_address() {
    let mut chain = open(&Arc::new(MemoryDb::default()));
    let account = two_of_three(&mut chain);
    let [alice, bob, carol] = members();
    let dave = Keypair::from_seed([34; 32]);
    let change = Tx::ChangeMultisig { members: vec![member(&carol), member(&dave)], threshold: 2 };

    // A key's account cannot send it; nor can the multisig hand itself a policy no multisig has.
    let sent = SignedTx::sign(change.clone(), nonce(&chain, &alice), FeeOffer::NONE, &alice);
    let direct = advance(&mut chain, vec![sent]);

    assert!(matches!(&direct[0].result, Err(failure) if failure.to_string().contains("only a multisig")));
    let sign = |chain: &Chain, tx: Tx, signers: &[&Keypair]| {
        let body = MultisigBody { account, tx: tx.clone(), nonce: multisig_nonce(chain, &account), fee: FeeOffer::NONE };
        let signatures = signers.iter().map(|keypair| MemberSignature::sign(&body, keypair, &chain.genesis())).collect();

        SignedTx::sign(Tx::Multisig { account, tx: Box::new(tx), signatures }, body.nonce, FeeOffer::NONE, signers[0])
    };
    let sent = sign(&chain, Tx::ChangeMultisig { members: vec![member(&dave)], threshold: 2 }, &[&alice, &bob]);
    let bad = advance(&mut chain, vec![sent]);

    assert!(matches!(&bad[0].result, Err(failure) if failure.to_string().contains("threshold")));
    assert_eq!(multisig_nonce(&chain, &account), 1, "a failure still takes the nonce, as any account's");

    let sent = sign(&chain, change, &[&alice, &bob]);
    advance(&mut chain, vec![sent]);
    let policy = Multisigs::new(&chain.state()).get(&account).unwrap().unwrap();

    assert_eq!(policy, MultisigPolicy { members: vec![member(&carol), member(&dave)], threshold: 2 });
    let sent = pay(&chain, &account, &Address::Ed25519([9; 32]), 1, &[&alice, &bob]);
    refused(&mut chain, sent, "not one of the multisig's members");
    let sent = pay(&chain, &account, &Address::Ed25519([9; 32]), 1, &[&carol, &dave]);
    advance(&mut chain, vec![sent]);

    assert_eq!(balance(&chain, &account), u256::new(999));
}

#[test]
fn multisig_policies_are_checked() {
    let policy = |members: Vec<MultisigMember>, threshold: u32| MultisigPolicy { members, threshold }.problem();
    let [alice, bob, _] = members();

    assert_eq!(policy(vec![member(&alice), member(&bob)], 2), None);
    assert!(policy(Vec::new(), 1).is_some());
    assert!(policy(vec![member(&alice)], 0).is_some());
    assert!(policy(vec![member(&alice)], 2).is_some());
    assert!(policy(vec![member(&alice), member(&alice)], 1).is_some(), "a key twice");
    assert!(policy((0..21).map(|seed| member(&Keypair::from_seed([seed; 32]))).collect(), 1).is_some(), "more than 20");

    // Every member names its other chains' keys, or none does.
    let with = |keypair: &Keypair, seed: u8| MultisigMember { bitcoin: Some([[2].as_slice(), &[seed; 32]].concat().try_into().unwrap()), evm: Some([seed; 20]), ..member(keypair) };

    assert!(policy(vec![with(&alice, 1), member(&bob)], 1).is_some());
    assert!(policy(vec![with(&alice, 1), with(&bob, 1)], 1).is_some(), "the same Bitcoin key twice");
}

#[test]
fn a_multisig_pays_its_own_fee() {
    let genesis = Genesis::dev_authority(authority().public());
    let mut chain = Chain::open(Arc::new(MemoryDb::default()), &genesis).unwrap();
    let creator = authority();
    let price = chain.next_base_price().saturating_mul(u256::new(2));
    let fee = FeeOffer { gas_limit: 200_000, max_price: price, tip: u256::ZERO };
    let at = nonce(&chain, &creator);
    let create = Tx::CreateMultisig { members: members().iter().map(member).collect(), threshold: 2 };
    advance(&mut chain, vec![SignedTx::sign(create, at, fee, &creator)]);
    let account = multisig_address(&creator.public().address(), at, chain.hash());
    let funding = Tx::Transfer { from: creator.public().address(), to: account, amount: Amount::U128(10u128.pow(18)) };
    let sent = SignedTx::sign(funding, nonce(&chain, &creator), fee, &creator);
    advance(&mut chain, vec![sent]);

    let [alice, bob, _] = members();
    let to = Address::Ed25519([9; 32]);
    let tx = Tx::Transfer { from: account, to, amount: Amount::U128(1) };
    let body = MultisigBody { account, tx: tx.clone(), nonce: 0, fee };
    let signatures = [&alice, &bob].iter().map(|keypair| MemberSignature::sign(&body, keypair, &chain.genesis())).collect();
    let before = balance(&chain, &account);
    let receipts = advance(&mut chain, vec![SignedTx::sign(Tx::Multisig { account, tx: Box::new(tx), signatures }, 0, fee, &alice)]);

    assert_eq!(receipts[0].result, Ok(Effect::Done));
    assert!(receipts[0].fee > u256::ZERO);
    assert_eq!(balance(&chain, &account), before - u256::new(1) - receipts[0].fee, "the multisig paid its fee");
    assert_eq!(balance(&chain, &alice.public().address()), u256::ZERO, "the carrier paid nothing");
}
