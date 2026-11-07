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

/// FFG of the three voters, the first two attesting, the third weighing more than either.
fn attested() -> Chain {
    let attesters = attesters();
    let bonds = [10u64, 10, 15];
    let validators = voters()
        .iter()
        .enumerate()
        .map(|(at, keypair)| GenesisValidator { account: keypair.public().address(), voter: keypair.public(), bond: u256::new(bonds[at].into()), attester: attesters.get(at).map(address_of) })
        .collect();
    let FinalityConfig::Ffg(config) = super::voting::ffg() else { unreachable!() };

    open(&FinalityConfig::Ffg(FfgConfig { validators, ..config }))
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
    assert_eq!(set, AttestingSet { total: u256::new(35), attesters: expected });
    assert!(!set.enough(u256::new(20)) && set.enough(u256::new(24)), "two thirds of 35");

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
        AbiType::Array(Box::new(AbiType::Address)),
        AbiType::Array(Box::new(AbiType::Uint(256))),
        AbiType::FixedBytes(32),
        AbiType::Array(Box::new(AbiType::Uint(256))),
        AbiType::Bytes,
    ];
    let values = [
        AbiValue::Array(words.iter().map(|word| AbiValue::FixedBytes(word.to_vec())).collect()),
        AbiValue::Uint(set.total),
        AbiValue::Array(set.attesters.iter().map(|(address, _)| AbiValue::Address(*address)).collect()),
        AbiValue::Array(set.attesters.iter().map(|(_, weight)| AbiValue::Uint(*weight)).collect()),
        AbiValue::FixedBytes(link.next_set.to_vec()),
        AbiValue::Array(places),
        AbiValue::Bytes(signed.iter().flat_map(|(_, attestation)| attestation.to_vec()).collect()),
    ];

    [selector("submit(bytes32[6],uint256,address[],uint256[],bytes32,uint256[],bytes)"), encode_values(&types, &values).unwrap()].concat()
}

#[test]
fn chainv4_finality_sol_takes_the_links_two_thirds_of_the_attesting_set_signed_and_nothing_less() {
    // All three attest here, so two thirds can be reached by attesters alone.
    let mut chain = {
        let FinalityConfig::Ffg(config) = super::voting::ffg() else { unreachable!() };
        let attesters = [attesters()[0].clone(), attesters()[1].clone(), Keypair::from_private(PrivateKey::Secp256k1([43; 32]))];
        let validators = voters().iter().zip(&attesters).map(|(keypair, attester)| GenesisValidator { account: keypair.public().address(), voter: keypair.public(), bond: u256::new(10), attester: Some(address_of(attester)) }).collect();

        open(&FinalityConfig::Ffg(FfgConfig { validators, ..config }))
    };
    let attesters = [attesters()[0].clone(), attesters()[1].clone(), Keypair::from_private(PrivateKey::Secp256k1([43; 32]))];
    let voters = voters();
    assert_eq!(Keypair::generate(KeyType::Secp256k1).unwrap().public().key_type(), KeyType::Secp256k1);

    // Deployed from the genesis checkpoint, trusted, with the set that signs epoch 1.
    let genesis_header = chain.block_at(0).unwrap().unwrap().header;
    let constructor = encode_values(
        &[AbiType::Uint(256), AbiType::Uint(256), AbiType::FixedBytes(32), AbiType::Uint(256), AbiType::FixedBytes(32), AbiType::FixedBytes(32)],
        &[
            AbiValue::Uint(u256::from(crate::crypto::ethereum::chain_id(&chain.genesis()))),
            AbiValue::Uint(u256::ZERO),
            AbiValue::FixedBytes(word(&start(&chain).block).to_vec()),
            AbiValue::Uint(u256::ZERO),
            AbiValue::FixedBytes(word(&genesis_header.state_root).to_vec()),
            AbiValue::FixedBytes(chain.attesting_set(1).unwrap().unwrap().hash().to_vec()),
        ],
    )
    .unwrap();
    let init = [crate::structs::hex::from_hex(include_str!("../../../../contracts/evm/ChainV4Finality.bin").trim()).unwrap(), constructor].concat();
    let contract = deploy(&mut chain, init);

    // Epoch 1, attested by two of three: justified there, as here.
    let mut links = Vec::new();

    for epoch in 1..=2 {
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
}
