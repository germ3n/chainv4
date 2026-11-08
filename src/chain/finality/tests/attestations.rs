//! Attestations through the chain: a validator names an attester, its votes carry attestations
//! that must recover to it, and ChainV4Finality.sol, run in this chain's own EVM, takes the links
//! they sign, so the Rust and Solidity sides are held to the same digest and set hash.

use {
    super::voting::{INTERVAL, checkpoint, honest_vote, open, start, voters},
    crate::structs::tx::{Tx, TxCallArgumentType},
    super::*,
    crate::abi::{AbiType, AbiValue, encode_values},
    crate::crypto::keypair::{KeyType, PrivateKey},
    crate::crypto::verifiable::VerifyError,
    crate::params::finality::{FfgConfig, GenesisValidator},
    crate::structs::attestation::{AttestedLink, AttestingSet, word},
    crate::structs::code::Code,
};

/// The attesters of the first two voters; the third names none.
fn attesters() -> [Keypair; 2] {
    [Keypair::from_private(PrivateKey::Secp256k1([41; 32])), Keypair::from_private(PrivateKey::Secp256k1([42; 32]))]
}

fn address_of(keypair: &Keypair) -> [u8; 20] {
    match keypair.public().address() {
        Address::Secp256k1(bytes) => bytes,
        other => panic!("{other} is no Ethereum address"),
    }
}

fn secret(keypair: &Keypair) -> [u8; 32] {
    match keypair.private() {
        PrivateKey::Secp256k1(secret) => *secret,
        _ => panic!("a secp256k1 key"),
    }
}

/// FFG of the three voters, the first two attesting, the third weighing more than either; the
/// proofs fork from the first block, so attesting sets name the weight a link needs, as
/// ChainV4Finality.sol takes them.
fn attested() -> Chain {
    attested_with(None)
}

/// The same, with a justification threshold of its own in thousandths, or two thirds.
fn attested_with(justify_permille: Option<u32>) -> Chain {
    let attesters = attesters();
    let bonds = [10u64, 10, 15];
    let validators = voters()
        .iter()
        .enumerate()
        .map(|(at, keypair)| GenesisValidator { account: keypair.public().address(), voter: keypair.public(), bond: u256::new(bonds[at].into()), attester: attesters.get(at).map(address_of) })
        .collect();
    let FinalityConfig::Ffg(config) = super::voting::ffg() else { unreachable!() };
    let later = crate::params::finality::FfgLater { justify_permille, ..config.later };
    let genesis = Genesis {
        forks: crate::params::forks::ForkSchedule { forks: vec![crate::params::forks::ScheduledFork { name: crate::params::forks::PROOFS.name.to_string(), height: 1, epoch: None, least: None }], lead: None },
        ..genesis_with(Some(FinalityConfig::Ffg(FfgConfig { validators, later, ..config })))
    };

    Chain::open(Arc::new(MemoryDb::default()), &genesis).unwrap()
}

/// The honest vote of a voter, with its attestation signed by this key.
fn attesting(chain: &Chain, voter: &Keypair, attester: &Keypair) -> SignedVote {
    let mut vote = honest_vote(chain, voter);
    let link = chain.attested_link(vote.vote.source, vote.vote.target).unwrap().expect("the link's sets are fixed");
    vote.attestation = Some(link.sign(&secret(attester)));

    vote
}

#[test]
fn a_votes_attestation_must_be_its_validators_attesters_and_the_set_says_who_attests() {
    let mut chain = attested();
    let [first, second] = attesters();
    let set = chain.attesting_set(1).unwrap().unwrap();

    // The whole set's weight, and the two attesters by address, each with its validator's weight.
    let mut expected = vec![(address_of(&first), u256::new(10)), (address_of(&second), u256::new(10))];
    expected.sort();
    assert_eq!(set, AttestingSet { total: u256::new(35), needed: Some(u256::new(24)), attesters: expected });
    assert!(!set.enough(u256::new(23)) && set.enough(u256::new(24)), "two thirds of 35, rounded up");

    grow_to(&mut chain, INTERVAL + 1);
    let voters = voters();

    let own = attesting(&chain, &voters[0], &first);
    let link = chain.attested_link(own.vote.source, own.vote.target).unwrap().unwrap();
    let plain = honest_vote(&chain, &voters[2]);

    // Another key's attestation, or one from a validator that names no attester, is refused.
    let forged = SignedVote { attestation: Some(link.sign(&secret(&second))), ..own };
    assert!(matches!(chain.add_vote(forged), Err(FinalityError::Invalid(VerifyError::BadAttestation))));
    let unnamed = SignedVote { attestation: Some(link.sign(&secret(&first))), ..plain };
    assert!(matches!(chain.add_vote(unnamed), Err(FinalityError::Invalid(VerifyError::BadAttestation))));

    // Its own counts, and a vote without one counts as ever.
    chain.add_vote(own).unwrap();
    chain.add_vote(plain).unwrap();
    assert_eq!(chain.justified().unwrap(), checkpoint(&chain, 1));
}

/// Deploys init code as the dev authority, and says where it landed.
fn deploy(chain: &mut Chain, init: Vec<u8>) -> Address {
    let nonce = super::voting::nonce(chain, &dev());
    let deploy = SignedTx::sign(Tx::Deploy { code: Code::Evm(init) }, nonce, FeeOffer::NONE, &dev(), &chain.genesis());
    let receipts = advance(chain, vec![deploy], Vec::new()).unwrap();

    match &receipts[0].result {
        Ok(Effect::Deployed(address)) => *address,
        other => panic!("the contract did not deploy: {other:?}"),
    }
}

/// Calls an EVM contract as the dev authority, and says what it returned or why it reverted.
fn call(chain: &mut Chain, to: Address, input: Vec<u8>) -> Result<Vec<u8>, String> {
    let nonce = super::voting::nonce(chain, &dev());
    let tx = Tx::EvmCall { to, value: crate::structs::amount::Amount::U128(0), input };
    let receipts = advance(chain, vec![SignedTx::sign(tx, nonce, FeeOffer::NONE, &dev(), &chain.genesis())], Vec::new()).unwrap();

    match &receipts[0].result {
        Ok(Effect::Returned(values)) => match values.as_slice() {
            [TxCallArgumentType::Bytes(bytes)] => Ok(bytes.clone()),
            other => Err(format!("returned {other:?}")),
        },
        Ok(other) => Err(format!("{other:?}")),
        Err(failure) => Err(failure.to_string()),
    }
}

fn selector(signature: &str) -> Vec<u8> {
    crate::crypto::secp256k1::keccak(signature.as_bytes())[..4].to_vec()
}

fn read(chain: &mut Chain, contract: Address, getter: &str) -> u256 {
    let output = call(chain, contract, selector(getter)).unwrap();

    u256::from_be_bytes(output[..32].try_into().unwrap())
}

/// submit's calldata for a link: the set that signs it, the next set's hash, and the attestations.
fn submit(link: &AttestedLink, set: &AttestingSet, signed: &[([u8; 20], [u8; 65])]) -> Vec<u8> {
    let words = [u256::from(link.source.epoch).to_be_bytes(), word(&link.source.block), u256::from(link.target.epoch).to_be_bytes(), word(&link.target.block), u256::from(link.height).to_be_bytes(), word(&link.state_root)];
    let places: Vec<AbiValue> = signed.iter().map(|(attester, _)| AbiValue::Uint(u256::from(set.attesters.iter().position(|(address, _)| address == attester).unwrap() as u64))).collect();
    let types = [
        AbiType::FixedArray(Box::new(AbiType::FixedBytes(32)), 6),
        AbiType::Uint(256),
        AbiType::Uint(256),
        AbiType::Array(Box::new(AbiType::Address)),
        AbiType::Array(Box::new(AbiType::Uint(256))),
        AbiType::FixedBytes(32),
        AbiType::Array(Box::new(AbiType::Uint(256))),
        AbiType::Bytes,
    ];
    let values = [
        AbiValue::Array(words.iter().map(|word| AbiValue::FixedBytes(word.to_vec())).collect()),
        AbiValue::Uint(set.total),
        AbiValue::Uint(set.needed.expect("a set from the proofs fork on")),
        AbiValue::Array(set.attesters.iter().map(|(address, _)| AbiValue::Address(*address)).collect()),
        AbiValue::Array(set.attesters.iter().map(|(_, weight)| AbiValue::Uint(*weight)).collect()),
        AbiValue::FixedBytes(link.next_set.to_vec()),
        AbiValue::Array(places),
        AbiValue::Bytes(signed.iter().flat_map(|(_, attestation)| attestation.to_vec()).collect()),
    ];

    [selector("submit(bytes32[6],uint256,uint256,address[],uint256[],bytes32,uint256[],bytes)"), encode_values(&types, &values).unwrap()].concat()
}

#[test]
fn chainv4_finality_sol_takes_the_links_two_thirds_of_the_attesting_set_signed_and_nothing_less() {
    // All three attest here, so two thirds can be reached by attesters alone.
    let mut chain = {
        let FinalityConfig::Ffg(config) = super::voting::ffg() else { unreachable!() };
        let attesters = [attesters()[0].clone(), attesters()[1].clone(), Keypair::from_private(PrivateKey::Secp256k1([43; 32]))];
        let validators = voters().iter().zip(&attesters).map(|(keypair, attester)| GenesisValidator { account: keypair.public().address(), voter: keypair.public(), bond: u256::new(10), attester: Some(address_of(attester)) }).collect();
        // The proofs fork from the first block: sets name the weight a link needs, as the
        // contract takes them.
        let genesis = Genesis {
            forks: crate::params::forks::ForkSchedule { forks: vec![crate::params::forks::ScheduledFork { name: crate::params::forks::PROOFS.name.to_string(), height: 1, epoch: None, least: None }], lead: None },
            ..genesis_with(Some(FinalityConfig::Ffg(FfgConfig { validators, ..config })))
        };

        Chain::open(Arc::new(MemoryDb::default()), &genesis).unwrap()
    };
    let attesters = [attesters()[0].clone(), attesters()[1].clone(), Keypair::from_private(PrivateKey::Secp256k1([43; 32]))];
    let voters = voters();
    assert_eq!(Keypair::generate(KeyType::Secp256k1).unwrap().public().key_type(), KeyType::Secp256k1);

    // Deployed from the genesis checkpoint, trusted, with the set that signs epoch 1, proofs
    // starting there too, checked under program 5s by a list of verifiers as SP1's gateways are:
    // the first refuses every proof, having no verifyProof, and the test verifier after it takes
    // them, so a proof stands when any verifier takes it.
    let genesis_header = chain.block_at(0).unwrap().unwrap().header;
    let Address::Evm(refuses) = deploy(&mut chain, crate::structs::hex::from_hex(include_str!("../../../../contracts/evm/MockFinality.bin").trim()).unwrap()) else { panic!("an EVM contract") };
    let verifier = deploy(&mut chain, crate::structs::hex::from_hex(include_str!("../../../../contracts/evm/MockSp1Verifier.bin").trim()).unwrap());
    let Address::Evm(verifier) = verifier else { panic!("an EVM contract") };
    let constructor = encode_values(
        &[AbiType::Uint(256), AbiType::Uint(256), AbiType::FixedBytes(32), AbiType::Uint(256), AbiType::FixedBytes(32), AbiType::FixedBytes(32), AbiType::Array(Box::new(AbiType::Address)), AbiType::FixedBytes(32), AbiType::FixedBytes(32), AbiType::Uint(256), AbiType::FixedBytes(32), AbiType::FixedBytes(32), AbiType::FixedBytes(32)],
        &[
            AbiValue::Uint(u256::from(crate::crypto::ethereum::chain_id(&chain.genesis()))),
            AbiValue::Uint(u256::ZERO),
            AbiValue::FixedBytes(word(&start(&chain).block).to_vec()),
            AbiValue::Uint(u256::ZERO),
            AbiValue::FixedBytes(word(&genesis_header.state_root).to_vec()),
            AbiValue::FixedBytes(chain.attesting_set(1).unwrap().unwrap().hash().to_vec()),
            AbiValue::Array(vec![AbiValue::Address(refuses), AbiValue::Address(verifier)]),
            AbiValue::FixedBytes(word(&chain.genesis()).to_vec()),
            AbiValue::FixedBytes([5; 32].to_vec()),
            AbiValue::Uint(u256::ZERO),
            AbiValue::FixedBytes(word(&chain.genesis()).to_vec()),
            AbiValue::FixedBytes(word(&genesis_header.state_root).to_vec()),
            AbiValue::FixedBytes([0; 32].to_vec()),
        ],
    )
    .unwrap();
    let init = [crate::structs::hex::from_hex(include_str!("../../../../contracts/evm/ChainV4Finality.bin").trim()).unwrap(), constructor].concat();
    let contract = deploy(&mut chain, init);

    // Epoch 1, attested by two of three: justified there, as here.
    let mut links = Vec::new();

    for epoch in 1..=5 {
        grow_to(&mut chain, epoch * INTERVAL + 1);
        let mut signed = Vec::new();

        for (voter, attester) in voters.iter().zip(&attesters).take(2) {
            let vote = attesting(&chain, voter, attester);
            signed.push((address_of(attester), vote.attestation.unwrap()));
            chain.add_vote(vote).unwrap();
        }

        signed.sort_by_key(|(address, _)| *address);
        let target = checkpoint(&chain, epoch);
        let source = if epoch == 1 { start(&chain) } else { checkpoint(&chain, epoch - 1) };
        let link = chain.attested_link(source, target).unwrap().unwrap();
        links.push((link, chain.attesting_set(epoch).unwrap().unwrap(), signed));
    }

    // One attestation is short of two thirds; one changed is not its attester's.
    let (link, set, signed) = &links[0];
    assert!(call(&mut chain, contract, submit(link, set, &signed[..1])).unwrap_err().contains("revert"));
    let mut tampered = signed.clone();
    tampered[0].1[10] ^= 1;
    assert!(call(&mut chain, contract, submit(link, set, &tampered)).is_err());

    // Two: epoch 1 justified, with its state root; then epoch 2 finalizes epoch 1.
    call(&mut chain, contract, submit(link, set, signed)).unwrap();
    assert_eq!(read(&mut chain, contract, "justifiedEpoch()"), u256::ONE);
    let root = call(&mut chain, contract, [selector("stateRootAt(uint256)"), u256::ONE.to_be_bytes().to_vec()].concat()).unwrap();
    assert_eq!(root, word(&link.state_root).to_vec());

    let (link, set, signed) = &links[1];
    call(&mut chain, contract, submit(link, set, signed)).unwrap();
    assert_eq!((read(&mut chain, contract, "justifiedEpoch()"), read(&mut chain, contract, "finalizedEpoch()")), (u256::new(2), u256::ONE));

    // The same link again is no later checkpoint.
    assert!(call(&mut chain, contract, submit(link, set, signed)).is_err());

    // It skips ahead: epoch 4's link from 3, which it never took, records 4 and finalizes
    // nothing, as 3's root is not known there; then 5's finalizes 4.
    let (link, set, signed) = &links[3];
    call(&mut chain, contract, submit(link, set, signed)).unwrap();
    assert_eq!((read(&mut chain, contract, "justifiedEpoch()"), read(&mut chain, contract, "finalizedEpoch()")), (u256::new(4), u256::ONE));
    let (link, set, signed) = &links[4];
    call(&mut chain, contract, submit(link, set, signed)).unwrap();
    assert_eq!((read(&mut chain, contract, "justifiedEpoch()"), read(&mut chain, contract, "finalizedEpoch()")), (u256::new(5), u256::new(4)));

    // A proof of the range from genesis to epoch 4's final checkpoint, two transfers leaving in it.
    let ends: Vec<crate::proofs::RangeEnd> = (0..=5)
        .map(|epoch| {
            let header = chain.block_at(epoch * INTERVAL).unwrap().unwrap().header;
            crate::proofs::RangeEnd { height: header.height, block: header.id(), state_root: header.state_root }
        })
        .collect();
    let end = |epoch: u64| ends[epoch as usize];
    let digests = [[0xd1; 32], [0xd2; 32]];
    let range = crate::proofs::ProvenRange {
        chain_id: crate::crypto::ethereum::chain_id(&chain.genesis()),
        genesis: chain.genesis(),
        start: crate::proofs::RangeEnd { height: 0, block: chain.genesis(), state_root: genesis_header.state_root },
        end: end(4),
        withdrawals: crate::proofs::withdrawals_root(&digests),
        program: [6; 32],
        gateway: crate::proofs::GatewayMode::Both,
    };
    let prove = |range: &crate::proofs::ProvenRange, program: [u8; 32], epoch: u64| {
        let values = range.public_values();
        let proof = crate::proofs::TestProofs::proof(&program, &values);
        let types = [AbiType::Bytes, AbiType::Bytes, AbiType::Uint(256)];

        [selector("prove(bytes,bytes,uint256)"), encode_values(&types, &[AbiValue::Bytes(values), AbiValue::Bytes(proof), AbiValue::Uint(u256::from(epoch))]).unwrap()].concat()
    };

    // Refused: by another program, to a checkpoint not final here, from elsewhere than the tip.
    assert!(call(&mut chain, contract, prove(&range, [7; 32], 4)).is_err(), "another program");
    assert!(call(&mut chain, contract, prove(&crate::proofs::ProvenRange { end: end(5), ..range }, [5; 32], 5)).is_err(), "5 is justified, not final");
    assert!(call(&mut chain, contract, prove(&crate::proofs::ProvenRange { start: end(1), ..range }, [5; 32], 4)).is_err(), "not the proven tip");
    let mut other_root = range;
    other_root.end.state_root = genesis_header.state_root;
    assert!(call(&mut chain, contract, prove(&other_root, [5; 32], 4)).is_err(), "a root the validators did not sign");

    // Taken: the tip moves, the withdrawals root is kept, the program and mode follow chainv4's.
    assert_eq!(read(&mut chain, contract, "verifierCount()"), u256::from(2u8));
    call(&mut chain, contract, prove(&range, [5; 32], 4)).unwrap();
    assert_eq!(read(&mut chain, contract, "provenHeight()"), u256::from(4 * INTERVAL));
    assert_eq!(read(&mut chain, contract, "program()"), u256::from_be_bytes([6; 32]));
    assert_eq!(read(&mut chain, contract, "gatewayMode()"), u256::ONE);
    assert_eq!(read(&mut chain, contract, "provenCount()"), u256::ONE);
    let first_end = call(&mut chain, contract, [selector("provenEnds(uint256)"), u256::ZERO.to_be_bytes().to_vec()].concat()).unwrap();
    assert_eq!(u256::from_be_bytes(first_end.try_into().unwrap()), u256::from(4 * INTERVAL));

    let withdrew = |chain: &mut Chain, digest: [u8; 32], path: Vec<[u8; 32]>| {
        let types = [AbiType::FixedBytes(32), AbiType::FixedBytes(32), AbiType::Array(Box::new(AbiType::FixedBytes(32)))];
        let values = [AbiValue::FixedBytes(word(&range.end.block).to_vec()), AbiValue::FixedBytes(digest.to_vec()), AbiValue::Array(path.into_iter().map(|node| AbiValue::FixedBytes(node.to_vec())).collect())];
        let answer = call(chain, contract, [selector("withdrew(bytes32,bytes32,bytes32[])"), encode_values(&types, &values).unwrap()].concat()).unwrap();

        answer.last() == Some(&1)
    };

    for (index, digest) in digests.iter().enumerate() {
        assert!(withdrew(&mut chain, *digest, crate::proofs::withdrawal_proof(&digests, index).unwrap()), "{index}");
    }

    assert!(!withdrew(&mut chain, [0xd3; 32], crate::proofs::withdrawal_proof(&digests, 0).unwrap()), "one that never left");

    // The same range again starts behind the tip now.
    assert!(call(&mut chain, contract, prove(&range, [6; 32], 4)).is_err());
}

/// FFG of the three voters, the first two attesting, governed by the dev key alone, with the
/// proofs fork from a height.
fn governed(fork: u64) -> Chain {
    let attesters = attesters();
    let FinalityConfig::Ffg(config) = super::voting::ffg() else { unreachable!() };
    let validators = config.validators.iter().enumerate().map(|(at, validator)| GenesisValidator { attester: attesters.get(at).map(address_of), ..validator.clone() }).collect();
    let genesis = Genesis {
        finality: Some(FinalityConfig::Ffg(FfgConfig { validators, ..config })),
        governance: Some(crate::governance::GovernanceConfig::Council { members: vec![dev().public().address()], threshold: 1, approval_epochs: 8, quorum_permille: 0 }),
        directory_admin: None,
        forks: crate::params::forks::ForkSchedule { forks: vec![crate::params::forks::ScheduledFork { name: crate::params::forks::PROOFS.name.to_string(), height: fork, epoch: None, least: None }], lead: None },
        ..genesis_with(None)
    };

    Chain::open(Arc::new(MemoryDb::default()), &genesis).unwrap()
}

/// Votes Yes on a change as the council of one, in the next block, carrying the unpaid
/// justifications: what the vote came to, None when the block left it out.
fn vote_on(chain: &mut Chain, change: crate::governance::Change) -> Option<Result<Effect, String>> {
    let version = crate::registry::changes::Changes::new(&chain.state()).version_of(change.kind()).unwrap();
    let tx = SignedTx::sign(Tx::VoteChange { version, change, vote: crate::governance::ChangeVote::Yes }, super::voting::nonce(chain, &dev()), FeeOffer::NONE, &dev(), &chain.genesis());
    let proofs = chain.unpaid_proofs().unwrap();

    advance(chain, vec![tx], proofs).unwrap().first().map(|receipt| receipt.result.clone().map_err(|failure| failure.to_string()))
}

fn decided(answer: Option<Result<Effect, String>>) -> bool {
    matches!(answer, Some(Ok(Effect::Decided { .. })))
}

#[test]
fn governance_turns_attestations_off_and_votes_and_blocks_then_carry_none() {
    use crate::governance::Change;
    use crate::proofs::{GatewayMode, ProofKind, ProofSettings};

    let mut chain = governed(INTERVAL);
    let at = |chain: &Chain| crate::registry::changes::Changes::new(&chain.state()).attestations_at(chain.tip().height + 1).unwrap();
    let proofs = |gateway: GatewayMode| Change::Proofs(ProofSettings { kinds: vec![ProofKind::Test], program: [5; 32], reward_per_block: u256::ZERO, gateway, finality_program: [0; 32], headers_program: [0; 32], reward_per_epoch: u256::ZERO });

    // Before the fork the change is no transaction; on until governance says otherwise.
    assert_eq!(vote_on(&mut chain, Change::Attestations(false)), None, "left out of the block");
    assert!(at(&chain));
    grow_to(&mut chain, INTERVAL);

    // Not off while Gateway.sol needs proofs, which could reach it no more.
    assert!(decided(vote_on(&mut chain, proofs(GatewayMode::Both))));
    let refused = vote_on(&mut chain, Change::Attestations(false));
    assert!(refused.as_ref().is_some_and(|answer| answer.as_ref().is_err_and(|why| why.contains("Gateway.sol needs proofs"))), "{refused:?}");
    assert!(decided(vote_on(&mut chain, proofs(GatewayMode::FederationOnly))));
    assert!(decided(vote_on(&mut chain, Change::Attestations(false))));
    assert!(!at(&chain));
    let refused = vote_on(&mut chain, proofs(GatewayMode::Both));
    assert!(refused.as_ref().is_some_and(|answer| answer.as_ref().is_err_and(|why| why.contains("attestations are off"))), "{refused:?}");
    assert_eq!(crate::params::json::change_json(&Change::Attestations(false)), serde_json::json!({ "attestations": false }));
    assert_eq!(crate::params::json::change_from_json(&serde_json::json!({ "attestations": false })), Ok(Change::Attestations(false)));

    // Off: a vote's attestation, its attester's own, is dropped as the vote is taken, so the
    // justification and the block that carries it hold none.
    grow_to(&mut chain, 2 * INTERVAL + 1);
    let [first, second] = attesters();
    let voters = voters();
    let attested = [attesting(&chain, &voters[0], &first), attesting(&chain, &voters[1], &second)];
    let link = chain.attested_link(attested[0].vote.source, attested[0].vote.target).unwrap().unwrap();
    for vote in attested {
        chain.add_vote(vote).unwrap();
    }
    let unpaid = chain.unpaid_proofs().unwrap();
    let justification = unpaid.iter().find(|proof| proof.target.epoch == 2).expect("epoch 2 justified");
    assert!(justification.votes.iter().all(|vote| vote.attestation.is_none()), "dropped");

    // A block carrying one anyway is invalid.
    let mut carrying = unpaid.clone();
    let place = carrying.iter().position(|proof| proof.target.epoch == 2).unwrap();
    let voter = carrying[place].votes.iter().position(|vote| vote.vote.voter == voters[0].public()).unwrap();
    carrying[place].votes[voter].attestation = Some(link.sign(&secret(&first)));
    let why = advance(&mut chain, Vec::new(), carrying).expect_err("refused");
    assert!(why.to_string().contains("attestations are off"), "{why}");
    advance(&mut chain, Vec::new(), unpaid).unwrap();

    // On again, from the next block: attestations ride and count as before.
    assert!(decided(vote_on(&mut chain, Change::Attestations(true))));
    assert!(at(&chain));
    grow_to(&mut chain, 3 * INTERVAL + 1);
    for (voter, attester) in voters[..2].iter().zip([&first, &second]) {
        let vote = attesting(&chain, voter, attester);
        chain.add_vote(vote).unwrap();
    }
    let unpaid = chain.unpaid_proofs().unwrap();
    let justification = unpaid.iter().find(|proof| proof.target.epoch == 3).expect("epoch 3 justified");
    assert!(justification.votes.iter().all(|vote| vote.attestation.is_some()), "kept");
    advance(&mut chain, Vec::new(), unpaid).unwrap();
}

/// ChainV4Finality.sol takes links by finality proofs, the voters' own votes checked as the
/// source's state fixed them, with no attestation at all, from any checkpoint it recorded: after
/// links by attestations as well, and the attesters' path goes on after them. It refuses a link by
/// another program, from a checkpoint it did not record or with another state root, or not later.
#[cfg(feature = "finality-proofs")]
#[test]
fn chainv4_finality_sol_takes_links_by_finality_proofs_of_the_voters_own_votes() {
    let mut chain = attested();
    let genesis_header = chain.block_at(0).unwrap().unwrap().header;
    let finality_program = [7; 32];
    let Address::Evm(verifier) = deploy(&mut chain, crate::structs::hex::from_hex(include_str!("../../../../contracts/evm/MockSp1Verifier.bin").trim()).unwrap()) else { panic!("an EVM contract") };
    let values = vec![
        AbiValue::Uint(u256::from(crate::crypto::ethereum::chain_id(&chain.genesis()))),
        AbiValue::Uint(u256::ZERO),
        AbiValue::FixedBytes(word(&start(&chain).block).to_vec()),
        AbiValue::Uint(u256::ZERO),
        AbiValue::FixedBytes(word(&genesis_header.state_root).to_vec()),
        AbiValue::FixedBytes(chain.attesting_set(1).unwrap().unwrap().hash().to_vec()),
        AbiValue::Array(vec![AbiValue::Address(verifier)]),
        AbiValue::FixedBytes(word(&chain.genesis()).to_vec()),
        AbiValue::FixedBytes([0; 32].to_vec()),
        AbiValue::Uint(u256::ZERO),
        AbiValue::FixedBytes(word(&chain.genesis()).to_vec()),
        AbiValue::FixedBytes(word(&genesis_header.state_root).to_vec()),
        AbiValue::FixedBytes(finality_program.to_vec()),
    ];
    let types = [
        AbiType::Uint(256), AbiType::Uint(256), AbiType::FixedBytes(32), AbiType::Uint(256), AbiType::FixedBytes(32), AbiType::FixedBytes(32), AbiType::Array(Box::new(AbiType::Address)),
        AbiType::FixedBytes(32), AbiType::FixedBytes(32), AbiType::Uint(256), AbiType::FixedBytes(32), AbiType::FixedBytes(32), AbiType::FixedBytes(32),
    ];
    let init = [crate::structs::hex::from_hex(include_str!("../../../../contracts/evm/ChainV4Finality.bin").trim()).unwrap(), encode_values(&types, &values).unwrap()].concat();
    let contract = deploy(&mut chain, init);

    // Epochs justified by plain votes, no attestation: the first voter's and the third's, who
    // names no attester at all, together two thirds of the weight.
    let voters = voters();
    for epoch in 1..=5 {
        grow_to(&mut chain, epoch * INTERVAL + 1);
        for voter in [&voters[0], &voters[2]] {
            let vote = honest_vote(&chain, voter);
            chain.add_vote(vote).unwrap();
        }
    }
    let prove = |values: Vec<u8>, program: [u8; 32]| {
        let proof = crate::proofs::TestProofs::proof(&program, &values);

        [selector("proveLink(bytes,bytes)"), encode_values(&[AbiType::Bytes, AbiType::Bytes], &[AbiValue::Bytes(values), AbiValue::Bytes(proof)]).unwrap()].concat()
    };
    let link = |chain: &Chain, epoch: u64| {
        let input = crate::proofs::LinkInput::build(chain, chain.db().snapshot(), epoch).unwrap();

        crate::proofs::check_link(chain, &input).unwrap()
    };

    let one = link(&chain, 1);
    assert_eq!(one.source_root, genesis_header.state_root, "genesis's state fixed epoch 1's voters");

    // Refused: by another program, from a source root it did not record, from a source it never
    // recorded.
    assert!(call(&mut chain, contract, prove(one.public_values(), [8; 32])).is_err(), "another program");
    let other = crate::proofs::ProvenLink { source_root: one.state_root, ..one };
    assert!(call(&mut chain, contract, prove(other.public_values(), finality_program)).is_err(), "another state root");
    let three = link(&chain, 3);
    assert!(call(&mut chain, contract, prove(three.public_values(), finality_program)).is_err(), "epoch 2 not recorded yet");

    // Taken, one link after another: justified, then its source final, the attesters' set too.
    for epoch in 1..=3 {
        let proven = link(&chain, epoch);
        call(&mut chain, contract, prove(proven.public_values(), finality_program)).unwrap();
        assert_eq!(read(&mut chain, contract, "justifiedEpoch()"), u256::from(epoch));
        assert_eq!(read(&mut chain, contract, "setHash()"), u256::from_be_bytes(proven.next_attesting_set));
    }
    assert_eq!(read(&mut chain, contract, "finalizedEpoch()"), u256::from(2u8));

    // The same link again is no later checkpoint; the next one goes on.
    let (again, four) = (link(&chain, 3), link(&chain, 4));
    assert!(call(&mut chain, contract, prove(again.public_values(), finality_program)).is_err(), "not later");
    call(&mut chain, contract, prove(four.public_values(), finality_program)).unwrap();
    assert_eq!(read(&mut chain, contract, "finalizedEpoch()"), u256::from(3u8));
}

/// A threshold above two thirds, from FFG's settings: the third voter and the first, 25 of 35, no
/// longer justify at 800 thousandths, which need 28; the attesting set names it, so
/// ChainV4Finality.sol takes no link short of it either; and settings under two thirds are refused.
#[test]
fn a_threshold_of_ffgs_own_holds_for_votes_and_for_attestations_alike() {
    let mut chain = attested_with(Some(800));
    let voters = voters();
    let set = chain.attesting_set(1).unwrap().unwrap();
    assert_eq!(set.needed, Some(u256::new(28)), "800 thousandths of 35, rounded up");
    assert!(!set.enough(u256::new(25)) && set.enough(u256::new(28)));

    grow_to(&mut chain, INTERVAL + 1);
    for voter in [&voters[0], &voters[2]] {
        let vote = honest_vote(&chain, voter);
        chain.add_vote(vote).unwrap();
    }
    assert_eq!(chain.justified().unwrap(), start(&chain), "25 of 35 is short of 800 thousandths");
    let vote = honest_vote(&chain, &voters[1]);
    chain.add_vote(vote).unwrap();
    assert_eq!(chain.justified().unwrap(), checkpoint(&chain, 1), "all three, 35");

    // A threshold under two thirds, or over all of the weight, is no FFG setting.
    let FinalityConfig::Ffg(config) = super::voting::ffg() else { unreachable!() };
    for permille in [666, 1_001] {
        let low = FfgConfig { later: crate::params::finality::FfgLater { justify_permille: Some(permille), ..config.later }, ..config.clone() };
        assert!(low.problem().is_some(), "{permille}");
    }
    let json = crate::params::json::finality_json(&FinalityConfig::Ffg(FfgConfig { later: crate::params::finality::FfgLater { justify_permille: Some(800), ..config.later }, ..config.clone() }));
    assert_eq!(json["justify_permille"], 800);
}

/// FFG's later fields encode as they did while the threshold is unset, nothing for neither and the
/// leak's own seventeen bytes for a leak alone, so chains from before keep their genesis ids; and
/// every pairing of the two reads back.
#[test]
fn ffgs_later_fields_encode_as_before_until_a_threshold_is_set_and_every_pairing_reads_back() {
    use crate::params::finality::FfgLater;

    let FinalityConfig::Ffg(config) = super::voting::ffg() else { unreachable!() };
    let bare = wincode::serialize(&FfgConfig { later: FfgLater::default(), ..config.clone() }).unwrap();
    let leak = Leak { after_epochs: 3, permille: 40 };
    let leaked = wincode::serialize(&FfgConfig { later: FfgLater { leak: Some(leak), justify_permille: None }, ..config.clone() }).unwrap();
    let mut expected = bare.clone();
    expected.push(1);
    expected.extend(3u64.to_le_bytes());
    expected.extend(40u64.to_le_bytes());
    assert_eq!(leaked, expected, "a leak alone, as before the threshold");

    for later in [FfgLater::default(), FfgLater { leak: Some(leak), justify_permille: None }, FfgLater { leak: None, justify_permille: Some(800) }, FfgLater { leak: Some(leak), justify_permille: Some(1_000) }] {
        let tuned = FfgConfig { later, ..config.clone() };
        assert_eq!(wincode::deserialize::<FfgConfig>(&wincode::serialize(&tuned).unwrap()).unwrap(), tuned, "{later:?}");
    }
}
