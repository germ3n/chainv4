//! Native multisig accounts through the chain: one made by a transaction, its transactions run on
//! members' signatures, approvals on chain, or both, refused before they run when they fall short,
//! and its members changed by its own quorum.

use {
    super::*,
    crate::crypto::keypair::KeyType,
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
    let create = Tx::CreateMultisig { members: members().iter().map(member).collect(), threshold: 2, testnet: false };
    let receipts = advance(chain, vec![SignedTx::sign(create, at, FeeOffer::NONE, &creator, &chain.genesis())]);
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

    SignedTx::sign(Tx::Multisig { account: *account, tx: Box::new(tx), signatures }, body.nonce, FeeOffer::NONE, &carrier, &chain.genesis())
}

fn refused(chain: &mut Chain, tx: SignedTx, why: &str) {
    let (block, now) = forced_block(chain, vec![tx]);

    match chain.import(&block, now) {
        Err(ImportError::Invalid(BlockError::Tx { error, .. })) => assert!(error.to_string().contains(why), "{why}: {error}"),
        other => panic!("{why}: expected the block refused, got {other:?}"),
    }
}

#[test]
fn a_bundle_for_another_chain_is_refused_with_its_carrier_or_its_members_signing_for_that_chain() {
    let mut chain = open(&Arc::new(MemoryDb::default()));
    let elsewhere = Chain::open(Arc::new(MemoryDb::default()), &Genesis { name: Some("another chain".to_string()), ..genesis() }).unwrap().genesis();
    let account = two_of_three(&mut chain);
    let [alice, bob, _] = members();
    let to = Address::Ed25519([9; 32]);
    let body = MultisigBody { account, tx: Tx::Transfer { from: account, to, amount: Amount::U128(10) }, nonce: 0, fee: FeeOffer::NONE };
    let bundled = |genesis: Hash| {
        let mut bundle = crate::registry::multisig::MultisigBundle { genesis, body: body.clone(), signatures: Vec::new() };
        bundle.sign(&alice);
        bundle.sign(&bob);

        bundle
    };

    // All of it for the other chain: the carrier's envelope is what fails first.
    refused(&mut chain, bundled(elsewhere).envelope(&alice), "signature does not match");

    // The members' for this chain, carried under a signature for the other: as refused.
    let here = bundled(chain.genesis());
    let carried = SignedTx::sign(here.envelope(&alice).tx, 0, FeeOffer::NONE, &alice, &elsewhere);
    refused(&mut chain, carried, "signature does not match");

    // A carrier for this chain carrying the members' for the other: their signatures do not hold.
    let theirs = bundled(elsewhere);
    let carried = SignedTx::sign(theirs.envelope(&alice).tx, 0, FeeOffer::NONE, &alice, &chain.genesis());
    refused(&mut chain, carried, "does not check out");

    // And all of it for this one runs.
    let carrier = stranger();
    assert_eq!(advance(&mut chain, vec![here.envelope(&carrier)])[0].result, Ok(Effect::Done));
    assert_eq!(balance(&chain, &to), u256::new(10));
}

#[test]
fn a_multisig_pays_on_its_members_signatures_and_never_on_too_few() {
    let mut chain = open(&Arc::new(MemoryDb::default()));
    let genesis_id = chain.genesis();
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
    refused(&mut chain, SignedTx::sign(Tx::Multisig { account, tx: Box::new(tx), signatures }, 0, FeeOffer::NONE, &alice, &genesis_id), "does not check out");

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
    let signed = SignedTx::sign(Tx::Multisig { account, tx: Box::new(theft), signatures }, 1, FeeOffer::NONE, &alice, &genesis_id);

    assert_eq!(signed.verify_on(&chain.genesis()), Err(crate::crypto::verifiable::VerifyError::NotSender));
}

#[test]
fn approvals_on_chain_count_with_signatures_until_the_nonce_moves() {
    let mut chain = open(&Arc::new(MemoryDb::default()));
    let genesis_id = chain.genesis();
    let account = two_of_three(&mut chain);
    let [alice, bob, carol] = members();
    let to = Address::Ed25519([9; 32]);
    let tx = Tx::Transfer { from: account, to, amount: Amount::U128(25) };
    let approve = |chain: &Chain, keypair: &Keypair, tx: &Tx, nonce: u64| {
        let approval = Tx::ApproveMultisig { account, nonce, fee: FeeOffer::NONE, tx: Box::new(tx.clone()) };

        SignedTx::sign(approval, self::nonce(chain, keypair), FeeOffer::NONE, keypair, &genesis_id)
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
    let withdraw = SignedTx::sign(Tx::WithdrawMultisig { account, proposal: id }, nonce(&chain, &carol), FeeOffer::NONE, &carol, &genesis_id);
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
    let genesis_id = chain.genesis();
    let account = two_of_three(&mut chain);
    let [alice, bob, carol] = members();
    let dave = Keypair::from_seed([34; 32]);
    let change = Tx::ChangeMultisig { members: vec![member(&carol), member(&dave)], threshold: 2, testnet: false };

    // A key's account cannot send it; nor can the multisig hand itself a policy no multisig has.
    let sent = SignedTx::sign(change.clone(), nonce(&chain, &alice), FeeOffer::NONE, &alice, &genesis_id);
    let direct = advance(&mut chain, vec![sent]);

    assert!(matches!(&direct[0].result, Err(failure) if failure.to_string().contains("only a multisig")));
    let sign = |chain: &Chain, tx: Tx, signers: &[&Keypair]| {
        let body = MultisigBody { account, tx: tx.clone(), nonce: multisig_nonce(chain, &account), fee: FeeOffer::NONE };
        let signatures = signers.iter().map(|keypair| MemberSignature::sign(&body, keypair, &chain.genesis())).collect();

        SignedTx::sign(Tx::Multisig { account, tx: Box::new(tx), signatures }, body.nonce, FeeOffer::NONE, signers[0], &genesis_id)
    };
    let sent = sign(&chain, Tx::ChangeMultisig { members: vec![member(&dave)], threshold: 2, testnet: false }, &[&alice, &bob]);
    let bad = advance(&mut chain, vec![sent]);

    assert!(matches!(&bad[0].result, Err(failure) if failure.to_string().contains("threshold")));
    assert_eq!(multisig_nonce(&chain, &account), 1, "a failure still takes the nonce, as any account's");

    let sent = sign(&chain, change, &[&alice, &bob]);
    advance(&mut chain, vec![sent]);
    let policy = Multisigs::new(&chain.state()).get(&account).unwrap().unwrap();

    assert_eq!(policy, MultisigPolicy { members: vec![member(&carol), member(&dave)], threshold: 2, testnet: false });
    let sent = pay(&chain, &account, &Address::Ed25519([9; 32]), 1, &[&alice, &bob]);
    refused(&mut chain, sent, "not one of the multisig's members");
    let sent = pay(&chain, &account, &Address::Ed25519([9; 32]), 1, &[&carol, &dave]);
    advance(&mut chain, vec![sent]);

    assert_eq!(balance(&chain, &account), u256::new(999));
}

#[test]
fn multisig_policies_are_checked() {
    let policy = |members: Vec<MultisigMember>, threshold: u32| MultisigPolicy { members, threshold, testnet: false }.problem();
    let [alice, bob, _] = members();

    assert_eq!(policy(vec![member(&alice), member(&bob)], 2), None);
    assert!(policy(Vec::new(), 1).is_some());
    assert!(policy(vec![member(&alice)], 0).is_some());
    assert!(policy(vec![member(&alice)], 2).is_some());
    assert!(policy(vec![member(&alice), member(&alice)], 1).is_some(), "a key twice");
    assert!(policy((1..=21).map(|seed| member(&Keypair::from_seed([seed; 32]))).collect(), 1).is_some(), "more than 20");

    // Every member names its other chains' keys, or none does.
    #[cfg(feature = "secp256k1")]
    {
        let xpub = |seed: u8| crate::registry::multisig::BitcoinXpub {
            fingerprint: [seed; 4],
            parent: [0; 4],
            public: crate::crypto::secp256k1::public_key(&[seed; 32]).unwrap(),
            chain_code: [seed; 32],
        };
        let with = |keypair: &Keypair, seed: u8| MultisigMember { bitcoin: Some(xpub(seed)), evm: Some([seed; 20]), ..member(keypair) };

        assert!(policy(vec![with(&alice, 1), member(&bob)], 1).is_some());
        assert!(policy(vec![with(&alice, 1), with(&bob, 1)], 1).is_some(), "the same Bitcoin key twice");
    }
}

#[test]
fn a_multisig_pays_its_own_fee() {
    let genesis = Genesis::dev_authority(authority().public());
    let mut chain = Chain::open(Arc::new(MemoryDb::default()), &genesis).unwrap();
    let genesis_id = chain.genesis();
    let creator = authority();
    let price = chain.next_base_price().saturating_mul(u256::new(2));
    let fee = FeeOffer { gas_limit: 200_000, max_price: price, tip: u256::ZERO };
    let at = nonce(&chain, &creator);
    let create = Tx::CreateMultisig { members: members().iter().map(member).collect(), threshold: 2, testnet: false };
    advance(&mut chain, vec![SignedTx::sign(create, at, fee, &creator, &genesis_id)]);
    let account = multisig_address(&creator.public().address(), at, chain.hash());
    let funding = Tx::Transfer { from: creator.public().address(), to: account, amount: Amount::U128(10u128.pow(18)) };
    let sent = SignedTx::sign(funding, nonce(&chain, &creator), fee, &creator, &genesis_id);
    advance(&mut chain, vec![sent]);

    let [alice, bob, _] = members();
    let to = Address::Ed25519([9; 32]);
    let tx = Tx::Transfer { from: account, to, amount: Amount::U128(1) };
    let body = MultisigBody { account, tx: tx.clone(), nonce: 0, fee };
    let signatures = [&alice, &bob].iter().map(|keypair| MemberSignature::sign(&body, keypair, &chain.genesis())).collect();
    let before = balance(&chain, &account);
    let receipts = advance(&mut chain, vec![SignedTx::sign(Tx::Multisig { account, tx: Box::new(tx), signatures }, 0, fee, &alice, &genesis_id)]);

    assert_eq!(receipts[0].result, Ok(Effect::Done));
    assert!(receipts[0].fee > u256::ZERO);
    assert_eq!(balance(&chain, &account), before - u256::new(1) - receipts[0].fee, "the multisig paid its fee");
    assert_eq!(balance(&chain, &alice.public().address()), u256::ZERO, "the carrier paid nothing");

    assert_eq!(crate::registry::supply::adds_up(&chain.state()).unwrap(), Ok(()), "minted less burned is what is held and bonded");
}

#[test]
fn its_evm_contracts_first_owners_are_kept_whatever_its_members_become() {
    let mut chain = open(&Arc::new(MemoryDb::default()));
    let genesis_id = chain.genesis();
    let account = two_of_three(&mut chain);
    let [alice, bob, carol] = members();
    let named = |keypair: &Keypair, evm: u8| MultisigMember { evm: Some([evm; 20]), ..member(keypair) };
    let sign = |chain: &Chain, tx: Tx| {
        let body = MultisigBody { account, tx: tx.clone(), nonce: multisig_nonce(chain, &account), fee: FeeOffer::NONE };
        let signatures = [&alice, &bob].iter().map(|keypair| MemberSignature::sign(&body, keypair, &chain.genesis())).collect();

        SignedTx::sign(Tx::Multisig { account, tx: Box::new(tx), signatures }, body.nonce, FeeOffer::NONE, &alice, &genesis_id)
    };

    assert_eq!(Multisigs::new(&chain.state()).evm_start(&account).unwrap(), None, "made naming no EVM addresses");

    // The first change that names them starts its contract.
    let first = Tx::ChangeMultisig { members: vec![named(&alice, 3), named(&bob, 1), named(&carol, 2)], threshold: 2, testnet: false };
    let sent = sign(&chain, first);
    advance(&mut chain, vec![sent]);
    let start = crate::registry::multisig::EvmStart { owners: vec![[1; 20], [2; 20], [3; 20]], threshold: 2 };

    assert_eq!(Multisigs::new(&chain.state()).evm_start(&account).unwrap(), Some(start.clone()), "sorted");

    // Later ones leave it: the contract's address commits to it.
    let later = Tx::ChangeMultisig { members: vec![named(&alice, 3), named(&bob, 4)], threshold: 1, testnet: false };
    let sent = sign(&chain, later);
    advance(&mut chain, vec![sent]);
    let state = chain.state();
    let multisigs = Multisigs::new(&state);

    assert_eq!(multisigs.evm_start(&account).unwrap(), Some(start));
    assert_eq!(multisigs.get(&account).unwrap().unwrap().evm().unwrap().owners, [[3; 20], [4; 20]]);
}

/// A key of a scheme, from a seed: one of each for a multisig whose members use every scheme.
fn of_scheme(scheme: KeyType, seed: u8) -> Keypair {
    use crate::crypto::keypair::PrivateKey;

    Keypair::from_private(match scheme {
        KeyType::Ed25519 => PrivateKey::Ed25519([seed; 32]),
        KeyType::EdBlake => PrivateKey::EdBlake([seed; 32]),
        KeyType::Secp256k1 => PrivateKey::Secp256k1([seed; 32]),
        KeyType::Bitcoin => PrivateKey::Bitcoin([seed; 32]),
    })
}

#[test]
fn a_multisig_of_one_member_of_every_scheme_signs_and_approves_with_each() {
    let mut chain = open(&Arc::new(MemoryDb::default()));
    let genesis_id = chain.genesis();
    let creator = authority();
    let keys: Vec<Keypair> = KeyType::ALL.iter().enumerate().map(|(place, scheme)| of_scheme(*scheme, 51 + place as u8)).collect();

    assert_eq!(keys.len(), 4, "Ed25519, EdBlake, secp256k1 and bitcoin");

    let at = nonce(&chain, &creator);
    let create = Tx::CreateMultisig { members: keys.iter().map(member).collect(), threshold: 3, testnet: false };
    let receipts = advance(&mut chain, vec![SignedTx::sign(create, at, FeeOffer::NONE, &creator, &genesis_id)]);
    let account = multisig_address(&creator.public().address(), at, chain.hash());

    assert_eq!(receipts[0].result, Ok(Effect::Deployed(account)));
    let funding = transfer(&chain, &creator, &account, 1_000);
    advance(&mut chain, vec![funding]);

    let [ed, blake, ethereum, bitcoin] = [&keys[0], &keys[1], &keys[2], &keys[3]];

    // Each scheme's member signs off chain, and approves on chain from its own account, in turn.
    for (signers, approvers) in [(vec![ed, blake, ethereum], Vec::new()), (vec![bitcoin], vec![ethereum, blake]), (Vec::new(), vec![bitcoin, ed, ethereum])] {
        let to = Address::Ed25519([9; 32]);
        let tx = Tx::Transfer { from: account, to, amount: Amount::U128(1) };
        let next = multisig_nonce(&chain, &account);
        let approvals: Vec<SignedTx> = approvers
            .iter()
            .map(|keypair| SignedTx::sign(Tx::ApproveMultisig { account, nonce: next, fee: FeeOffer::NONE, tx: Box::new(tx.clone()) }, nonce(&chain, keypair), FeeOffer::NONE, keypair, &genesis_id))
            .collect();

        for receipt in advance(&mut chain, approvals) {
            assert!(receipt.result.is_ok(), "{receipt:?}");
        }

        let body = MultisigBody { account, tx: tx.clone(), nonce: next, fee: FeeOffer::NONE };
        let signatures = signers.iter().map(|keypair| MemberSignature::sign(&body, keypair, &genesis_id)).collect();
        let carrier = signers.first().copied().unwrap_or(bitcoin);
        let receipts = advance(&mut chain, vec![SignedTx::sign(Tx::Multisig { account, tx: Box::new(tx), signatures }, next, FeeOffer::NONE, carrier, &genesis_id)]);

        assert_eq!(receipts.len(), 1, "{} signing, {} approving", signers.len(), approvers.len());
        assert_eq!(receipts[0].result, Ok(Effect::Done));
    }

    assert_eq!(multisig_nonce(&chain, &account), 3);
    assert_eq!(balance(&chain, &Address::Ed25519([9; 32])), u256::new(3));
}

#[cfg(feature = "bitcoin")]
#[test]
fn a_multisig_reaches_here_evm_alone_bitcoin_alone_or_both() {
    use crate::crypto::mnemonic::Phrase;

    let mut chain = open(&Arc::new(MemoryDb::default()));
    let creator = authority();
    let keys = [of_scheme(KeyType::Ed25519, 61), of_scheme(KeyType::Bitcoin, 62)];
    // Each member's account key on Bitcoin is its own words', apart from its key here, a bitcoin:
    // one too, which derives elsewhere.
    let xpub = |words: &str| Phrase::parse(words).unwrap().seed("").multisig_account(0);
    let xpubs = [xpub("abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"), xpub("legal winner thank year wave sausage worth useful legal winner thank yellow")];

    for (bitcoin, evm) in [(false, false), (false, true), (true, false), (true, true)] {
        let members = keys
            .iter()
            .enumerate()
            .map(|(place, keypair)| MultisigMember { key: keypair.public(), bitcoin: bitcoin.then_some(xpubs[place]), evm: evm.then_some([place as u8 + 1; 20]) })
            .collect();
        let at = nonce(&chain, &creator);
        let create = SignedTx::sign(Tx::CreateMultisig { members, threshold: 2, testnet: false }, at, FeeOffer::NONE, &creator, &chain.genesis());
        let receipts = advance(&mut chain, vec![create]);
        let account = multisig_address(&creator.public().address(), at, chain.hash());
        let policy = Multisigs::new(&chain.state()).get(&account).unwrap().unwrap();

        assert_eq!(receipts[0].result, Ok(Effect::Deployed(account)), "Bitcoin {bitcoin}, EVM {evm}");
        assert_eq!((policy.members.iter().all(|member| member.bitcoin.is_some()), policy.evm().is_some()), (bitcoin, evm));
    }
}

#[test]
fn the_member_index_lists_each_multisig_under_its_members_through_changes_reorgs_and_a_late_start() {
    // The same blocks go to a chain that keeps the index from the start, and one that starts late.
    let live_db = Arc::new(MemoryDb::default());
    let mut live = Chain::open(live_db.clone(), &pow_genesis()).unwrap();
    let late_db = Arc::new(MemoryDb::default());
    let mut late = Chain::open(late_db.clone(), &pow_genesis()).unwrap();
    let genesis_block = late.block_at(0).unwrap().unwrap();
    let genesis_id = pow_genesis().block().id();
    let hash = HashType::default_chain();
    let creator = authority();
    let miner = Address::Ed25519([9; 32]);
    let [alice, bob, carol] = members();
    let (ethereum, dave) = (of_scheme(KeyType::Secp256k1, 34), Keypair::from_seed([35; 32]));
    let make = |keys: &[&Keypair], threshold: u32, nonce: u64| {
        let create = Tx::CreateMultisig { members: keys.iter().map(|keypair| member(keypair)).collect(), threshold, testnet: false };

        SignedTx::sign(create, nonce, FeeOffer::NONE, &creator, &genesis_id)
    };
    let (first, second) = (multisig_address(&creator.public().address(), 0, hash), multisig_address(&creator.public().address(), 1, hash));
    let listed = |chain: &Chain, keypair: &Keypair| {
        let mut found = stored_multisigs_of(&*chain.db, &keypair.public().address(), None, 10).unwrap().0;
        found.sort();

        found
    };
    let entries = |db: &MemoryDb| {
        let mut all = Vec::new();
        db.scan(Column::Members, &[], &[], &mut |key, value| {
            all.push((key.to_vec(), value.to_vec()));

            true
        })
        .unwrap();

        all
    };
    let both = |found: [Address; 2]| {
        let mut found = found.to_vec();
        found.sort();

        found
    };

    assert_eq!(live.keep_member_index().unwrap(), Some(0), "nothing to build it from yet");
    assert_eq!(live.keep_member_index().unwrap(), None, "kept already");

    // Two multisigs: alice is in both, and the Ethereum account is found by its address.
    let b1 = mine_on(&late, &genesis_block, vec![make(&[&alice, &bob, &carol], 2, 0), make(&[&alice, &ethereum], 1, 1)], miner);

    for chain in [&mut live, &mut late] {
        extended(chain.import(&b1, LATER));
    }

    assert_eq!(listed(&live, &alice), both([first, second]));
    assert_eq!((listed(&live, &bob), listed(&live, &ethereum)), (vec![first], vec![second]));

    // The second's quorum swaps the Ethereum account for dave: the one removed goes.
    let change = Tx::ChangeMultisig { members: vec![member(&alice), member(&dave)], threshold: 1, testnet: false };
    let body = MultisigBody { account: second, tx: change.clone(), nonce: 0, fee: FeeOffer::NONE };
    let signed = SignedTx::sign(Tx::Multisig { account: second, tx: Box::new(change), signatures: vec![MemberSignature::sign(&body, &alice, &genesis_id)] }, 0, FeeOffer::NONE, &alice, &genesis_id);
    let b2 = mine_on(&late, &b1, vec![signed], miner);

    for chain in [&mut live, &mut late] {
        extended(chain.import(&b2, LATER));
    }

    assert!(listed(&live, &ethereum).is_empty(), "no longer a member");
    assert_eq!((listed(&live, &dave), listed(&live, &alice)), (vec![second], both([first, second])));

    // Pages go on from the last multisig listed.
    let (page, next) = stored_multisigs_of(&*live_db, &alice.public().address(), None, 1).unwrap();
    let (rest, after) = stored_multisigs_of(&*live_db, &alice.public().address(), next.as_ref(), 1).unwrap();

    assert_eq!((page.len(), next, after), (1, Some(page[0]), None));
    assert_eq!(both([page[0], rest[0]]), both([first, second]));

    // Turned on late, on a chain that has run: built whole from the registry as it is now.
    assert!(entries(&late_db).is_empty());
    assert_eq!(late.keep_member_index().unwrap(), Some(2));
    assert_eq!(entries(&late_db), entries(&live_db), "building it builds what following the blocks did");

    // A heavier branch from the first block takes the change off, and what it did to the index.
    let mut branch = vec![mine_on(&late, &b1, Vec::new(), miner)];

    for _ in 0..2 {
        for chain in [&mut live, &mut late] {
            chain.import(branch.last().unwrap(), LATER).unwrap();
        }

        branch.push(mine_on(&late, branch.last().unwrap(), Vec::new(), miner));
    }

    assert_eq!(late.tip().id(), branch[1].header.id());
    assert_eq!((listed(&late, &ethereum), listed(&late, &dave)), (vec![second], Vec::new()));
    assert_eq!(entries(&late_db), entries(&live_db));

    // Kept across a restart, until dropped.
    assert!(Chain::open(live_db.clone(), &pow_genesis()).unwrap().member_index());
    assert!(late.drop_member_index().unwrap());
    assert!(entries(&late_db).is_empty());
    assert!(!Chain::open(late_db.clone(), &pow_genesis()).unwrap().member_index());
}
