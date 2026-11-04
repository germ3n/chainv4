extern crate std;

use {
    crate::{
        forks::{DOMAIN_SYNC_COMMITTEE, compute_domain},
        ssz::{self, chunk_u64, merkleize, mix_in_length, zero_hash},
        testing::{self, BlockRoots, Committee, Native, State},
        tx::{self, decode_transaction},
        *,
    },
    alloc::{vec, vec::Vec},
};

const C: Native = Native;

fn hex(text: &str) -> Vec<u8> {
    let text = text.trim_start_matches("0x");

    (0..text.len()).step_by(2).map(|at| u8::from_str_radix(&text[at..at + 2], 16).unwrap()).collect()
}

fn root(text: &str) -> Root {
    hex(text).try_into().unwrap()
}

/// The first slot of a period, plus some.
fn slot(period: u64, plus: u64) -> u64 {
    period * MAINNET.slots_per_period() + plus
}

/// Mainnet periods in each fork: Deneb's last, Electra's first, and one well into Fulu.
const DENEB_LAST: u64 = 1421;
const ELECTRA_FIRST: u64 = 1422;
const FULU: u64 = 1610;

#[test]
fn zero_hashes_and_empty_list_roots_match_the_specs() {
    assert_eq!(zero_hash(&C, 1), root("f5a5fd42d16a20302798ef6ed309979b43003d2320d9f0e8ea9831a92759fb4b"));
    assert_eq!(zero_hash(&C, 2), root("db56114e00fdd4c1f85c892bf35ac9a89289aaecb1ebd0a96cde606a748b5d71"));
    assert_eq!(zero_hash(&C, 3), root("c78009fdf07fc56a11f122370658a353aaa542ed63e44c4bc15ff4cd105ab33c"));
    assert_eq!(merkleize(&C, &[], 8), zero_hash(&C, 3), "no chunks under a limit are its zero tree");
    assert_eq!(merkleize(&C, &[[7; 32]], 1), [7; 32], "one chunk is its own root");

    // The transactions_root and withdrawals_root of every execution payload with none: an empty
    // List[Transaction, 2**20] and List[Withdrawal, 16].
    assert_eq!(tx::transactions_root(&C, &[] as &[&[u8]]), root("7ffe241ea60187fdb0187bfa22de35d1f9bed7ab061d9401fd47e34a54fbede1"));
    assert_eq!(mix_in_length(&C, &zero_hash(&C, 4), 0), root("792930bbd5baac43bcc798ee49aa8185ef76bb3b44ba62b91d86ae569e4bb535"));
}

#[test]
fn mainnet_s_genesis_block_root_is_rebuilt_from_an_empty_body() {
    // Phase 0's BeaconBlockBody, all defaults, as the genesis block's is.
    let empty_list = |depth: u32| mix_in_length(&C, &zero_hash(&C, depth), 0);
    let body = [
        merkleize(&C, &ssz::pack(&[0; 96]), 3), // randao_reveal
        merkleize(&C, &[[0; 32], chunk_u64(0), [0; 32]], 3), // eth1_data
        [0; 32],                                // graffiti
        empty_list(4),                          // proposer_slashings, at most 16
        empty_list(1),                          // attester_slashings, at most 2
        empty_list(7),                          // attestations, at most 128
        empty_list(4),                          // deposits, at most 16
        empty_list(4),                          // voluntary_exits, at most 16
    ];
    let header = BeaconBlockHeader {
        slot: 0,
        proposer_index: 0,
        parent_root: [0; 32],
        state_root: root("7e76880eb67bbdc86250aa578958e9d0675e64e714337855204fb5abaaf82c2b"),
        body_root: merkleize(&C, &body, 8),
    };

    assert_eq!(header.hash_tree_root(&C), root("4d611d5b93fdab69013a7f0a2f961caca0c853f87cfe9595fe50038163079360"));
    assert_eq!(BeaconBlockHeader::decode(&header.encode()).unwrap(), header);
    assert!(BeaconBlockHeader::decode(&[0; 111]).is_err());
}

#[test]
fn merkle_branches_check_at_their_generalized_index_and_nowhere_else() {
    let leaves: Vec<Root> = (0..13u8).map(|byte| [byte; 32]).collect();
    let top = merkleize(&C, &leaves, 16);

    for index in 0..13 {
        let branch = ssz::merkle_branch(&C, &leaves, 4, index);
        assert!(ssz::is_valid_merkle_branch(&C, &leaves[index as usize], &branch, 4, index, &top));
        assert!(ssz::is_valid_normalized_merkle_branch(&C, &leaves[index as usize], &branch, 16 + index, &top));
        assert!(!ssz::is_valid_merkle_branch(&C, &leaves[index as usize], &branch, 4, index ^ 1, &top), "not at its neighbour's place");
        assert!(!ssz::is_valid_merkle_branch(&C, &leaves[index as usize], &branch[..3], 4, index, &top), "not short");

        // A branch a level longer, its extra node zero, is the same branch normalized.
        let padded = [vec![[0; 32]], branch.clone()].concat();
        assert!(ssz::is_valid_normalized_merkle_branch(&C, &leaves[index as usize], &padded, 16 + index, &top));
        let mut dirty = padded.clone();
        dirty[0][0] = 1;
        assert!(!ssz::is_valid_normalized_merkle_branch(&C, &leaves[index as usize], &dirty, 16 + index, &top));
    }

    // Generalized indices: Electra's moved, by the state's growing from 32 leaves to 64.
    assert_eq!((ssz::floorlog2(105), ssz::subtree_index(105)), (6, 41));
    assert_eq!((ssz::floorlog2(169), ssz::subtree_index(169)), (7, 41));
    assert_eq!(ssz::concat_gindices(52, 3), 105, "the checkpoint's root under the state's 21st field");
    assert_eq!(ssz::concat_gindices(84, 3), 169);
    assert_eq!(ssz::concat_gindices(2, (1 << 20) + 5), (1 << 21) + 5, "a transaction under its list's data root");
}

#[test]
fn networks_are_picked_by_name_and_forks_by_epoch() {
    assert_eq!(Network::by_name("mainnet"), Some(&MAINNET));
    assert_eq!(Network::by_name("sepolia").unwrap().genesis_validators_root, SEPOLIA.genesis_validators_root);
    assert_eq!(Network::by_name("hoodi").unwrap().name, "hoodi");
    assert_eq!(Network::by_name("goerli"), None);

    assert_eq!(MAINNET.fork_at_epoch(364_031).fork, Fork::Deneb);
    assert_eq!(MAINNET.fork_at_epoch(364_032).fork, Fork::Electra);
    assert_eq!(MAINNET.fork_at_epoch(411_392), ForkAt { fork: Fork::Fulu, version: [6, 0, 0, 0], epoch: 411_392 });
    assert_eq!(SEPOLIA.fork_at_epoch(272_640).version, [0x90, 0, 0, 0x75]);
    assert_eq!(HOODI.fork_at_epoch(0).fork, Fork::Deneb, "Hoodi began at Deneb");
    assert_eq!(HOODI.fork_at_epoch(2_048).fork, Fork::Electra);
    assert_eq!(MAINNET.slot_at(1_606_824_023 + 12 * 1000 + 11), 1000);

    // Each network's schedule is in order and its versions its own.
    for network in NETWORKS {
        assert!(network.forks.windows(2).all(|pair| pair[0].epoch <= pair[1].epoch && pair[0].fork < pair[1].fork), "{}", network.name);
        assert_eq!(network.forks.last().unwrap().fork, Fork::Fulu, "{}", network.name);
    }

    // A domain starts with its type, and differs by fork and network.
    let domain = MAINNET.domain(&C, DOMAIN_SYNC_COMMITTEE, [6, 0, 0, 0]);
    assert_eq!(domain[..4], DOMAIN_SYNC_COMMITTEE);
    assert_ne!(domain, MAINNET.domain(&C, DOMAIN_SYNC_COMMITTEE, [5, 0, 0, 0]));
    assert_ne!(domain, compute_domain(&C, DOMAIN_SYNC_COMMITTEE, [6, 0, 0, 0], &SEPOLIA.genesis_validators_root));
}

#[test]
fn eth2_bls_test_keys_derive_and_sign_as_ethereum_s_clients_do() {
    // The consensus spec tests' first private key, its public key, and its signature over 32 zero
    // bytes (bls/sign/sign_case_84d45c9c7cca6b92).
    let secret = bls12_381::Scalar::from_bytes(&hex("263dbd792f5b1be47ed85f8938c0f29586af0d3ac7b977f21c278fe1462040e3").into_iter().rev().collect::<Vec<_>>().try_into().unwrap()).unwrap();
    let public = testing::public_key(&secret);
    assert_eq!(public.to_vec(), hex("a491d1b0ecd9bb917989f0e74f0dea0422eac4a873e5e2644f368dffb9a6e20fd6e10c1b77654d067c0618f6e5a7f79a"));

    let signature = testing::sign(&secret, &[0; 32]);
    assert_eq!(
        signature.to_vec(),
        hex("b6ed936746e01f8ecf281f020953fbf1f01debd5657c4a383940b020b26507f6076334f91e2366c96e9ab279fb5158090352ea1c5b0c9274504f4f0e7053af24802e51e4568d164fe986834f41e55c8e850ce1f98458c0cfc9ab380b55285a55")
    );
    assert!(testing::fast_aggregate_verify(&[public], &[0; 32], &signature));
    assert!(!testing::fast_aggregate_verify(&[public], &[1; 32], &signature));
}

#[test]
fn fast_aggregate_verify_refuses_infinity_bad_points_and_no_keys() {
    let secrets = [bls12_381::Scalar::from(5u64), bls12_381::Scalar::from(9u64)];
    let keys = secrets.map(|secret| testing::public_key(&secret));
    let message = [3u8; 32];
    let aggregate = testing::sign(&(secrets[0] + secrets[1]), &message);

    assert!(testing::fast_aggregate_verify(&keys, &message, &aggregate));
    assert!(!testing::fast_aggregate_verify(&keys[..1], &message, &aggregate), "not one member's");
    assert!(!testing::fast_aggregate_verify(&[], &message, &aggregate), "no keys");

    let mut infinity_key = [0u8; 48];
    infinity_key[0] = 0xc0;
    let mut infinity_signature = [0u8; 96];
    infinity_signature[0] = 0xc0;
    assert!(!testing::fast_aggregate_verify(&[keys[0], infinity_key], &message, &aggregate), "a key at infinity");
    assert!(!testing::fast_aggregate_verify(&[infinity_key], &message, &infinity_signature), "infinity over infinity");

    // A key and its negation sum to infinity: an aggregate key at infinity is refused too.
    let negated = bls12_381::G1Affine::from(-bls12_381::G1Projective::from(bls12_381::G1Affine::from_compressed(&keys[0]).unwrap())).to_compressed();
    assert!(!testing::fast_aggregate_verify(&[keys[0], negated], &message, &infinity_signature));

    let mut off_curve = keys[0];
    off_curve[47] ^= 1;
    assert!(!testing::fast_aggregate_verify(&[off_curve], &message, &aggregate), "a key that is no point");
    let mut uncompressed_flag = keys[0];
    uncompressed_flag[0] &= 0x7f;
    assert!(!testing::fast_aggregate_verify(&[uncompressed_flag], &message, &aggregate), "a key without its compression flag");
    let mut bad_signature = aggregate;
    bad_signature[95] ^= 1;
    assert!(!testing::fast_aggregate_verify(&keys, &message, &bad_signature));
}

/// A store bootstrapped at a period, with its committee and the next one's.
fn started(period: u64) -> (Store, Committee, Committee) {
    let (current, next) = (Committee::new(period as u32), Committee::new(period as u32 + 1));
    let (bootstrap, trusted) = testing::bootstrap(&C, &MAINNET, slot(period, 64), &current.committee);

    (Store::bootstrap(&C, &MAINNET, &trusted, &bootstrap).unwrap(), current, next)
}

/// A finalized header made up at a slot.
fn finalized_at(at: u64) -> LightClientHeader {
    testing::header(&C, &MAINNET, at, testing::filler(0x77, at, 0), testing::execution(&C, at, &[b"anchor".as_slice()]))
}

#[test]
fn updates_and_bootstraps_encode_and_decode_in_each_fork_s_layout() {
    let committee = Committee::new(1);
    let next = Committee::new(2);

    for period in [DENEB_LAST, ELECTRA_FIRST, FULU] {
        let (bootstrap, _) = testing::bootstrap(&C, &MAINNET, slot(period, 64), &committee.committee);
        let bytes = bootstrap.encode(&MAINNET);
        assert_eq!(LightClientBootstrap::decode(&bytes, &MAINNET).unwrap(), bootstrap, "period {period}");

        let electra = period >= ELECTRA_FIRST;
        assert_eq!(bootstrap.current_sync_committee_branch.len(), if electra { 6 } else { 5 });
        assert_eq!(bytes.len(), 4 + SyncCommittee::SSZ_LEN + 32 * bootstrap.current_sync_committee_branch.len() + LightClientHeader::FIXED_LEN + 584 + 8);

        for brought in [Some(&next.committee), None] {
            let update = testing::update(&C, &MAINNET, &committee, 400, slot(period, 200), slot(period, 201), &finalized_at(slot(period, 128)), brought);
            let bytes = update.encode(&MAINNET);
            assert_eq!(LightClientUpdate::decode(&bytes, &MAINNET).unwrap(), update);
            assert_eq!((update.finality_branch.len(), update.next_sync_committee_branch.len()), if electra { (7, 6) } else { (6, 5) });

            // Cut into its fixed part, it does not read.
            assert!(LightClientUpdate::decode(&bytes[..100], &MAINNET).is_err());
            assert!(LightClientUpdate::decode(&bytes[..SyncCommittee::SSZ_LEN], &MAINNET).is_err());
        }
    }

    // Capella's execution header has no blob gas: 16 bytes shorter.
    let capella = 6_500_000;
    assert_eq!(MAINNET.fork_at_slot(capella).fork, Fork::Capella);
    let header = finalized_at(capella);
    assert_eq!(header.encode(Fork::Capella).len(), LightClientHeader::FIXED_LEN + 568 + 8);
    assert_eq!(LightClientHeader::decode_for(&header.encode(Fork::Capella), &MAINNET).unwrap(), header);
    header.check(&C, &MAINNET).unwrap();

    // A finality update's zeros, with no finalized header, is not taken.
    let mut update = testing::update(&C, &MAINNET, &committee, 400, slot(FULU, 200), slot(FULU, 201), &finalized_at(slot(FULU, 128)), None);
    update.finality_branch = vec![[0; 32]; 7];
    assert_eq!(LightClientUpdate::decode(&update.encode(&MAINNET), &MAINNET), Err(Error::NotFinality));

    // Nor a header from before Capella.
    let bellatrix = 5_000_000;
    let (old, _) = testing::bootstrap(&C, &MAINNET, bellatrix, &committee.committee);
    assert_eq!(LightClientBootstrap::decode(&old.encode(&MAINNET), &MAINNET), Err(Error::BeforeCapella));
}

#[test]
fn a_bootstrap_at_a_trusted_root_starts_the_store_and_others_are_refused() {
    let committee = Committee::new(7);
    let (bootstrap, trusted) = testing::bootstrap(&C, &MAINNET, slot(FULU, 64), &committee.committee);
    let store = Store::bootstrap(&C, &MAINNET, &trusted, &bootstrap).unwrap();

    assert_eq!((store.finalized.beacon.slot, store.finalized.root, store.period(), store.next.is_none()), (slot(FULU, 64), trusted, FULU, true));
    assert_eq!(store.finalized.transactions_root, bootstrap.header.execution.transactions_root);
    assert_eq!(Store::bootstrap(&C, &MAINNET, &[1; 32], &bootstrap), Err(Error::NotTrusted));

    let mut wrong = bootstrap.clone();
    wrong.current_sync_committee = Committee::new(8).committee;
    assert_eq!(Store::bootstrap(&C, &MAINNET, &trusted, &wrong), Err(Error::BadCommitteeBranch));

    let mut wrong = bootstrap.clone();
    wrong.header.execution.block_number += 1;
    assert_eq!(Store::bootstrap(&C, &MAINNET, &trusted, &wrong), Err(Error::BadExecutionBranch));

    // Electra's committee sits at gindex 86, not Deneb's 54.
    let (deneb, deneb_root) = testing::bootstrap(&C, &MAINNET, slot(DENEB_LAST, 64), &committee.committee);
    Store::bootstrap(&C, &MAINNET, &deneb_root, &deneb).unwrap();
    let mut misplaced = bootstrap.clone();
    misplaced.current_sync_committee_branch.remove(0);
    assert_eq!(Store::bootstrap(&C, &MAINNET, &trusted, &misplaced), Err(Error::BadCommitteeBranch));
}

#[test]
fn an_update_two_thirds_signed_moves_finality_and_less_is_refused() {
    let (store, current, next) = started(FULU);
    let finalized = finalized_at(slot(FULU, 128));
    let make = |signers: usize| testing::update(&C, &MAINNET, &current, signers, slot(FULU, 200), slot(FULU, 201), &finalized, Some(&next.committee));

    // 341 of 512 is under two thirds; 342 is not.
    let mut kept = store.clone();
    assert_eq!(kept.process_update(&C, &make(341), slot(FULU, 300)), Err(Error::LowParticipation));
    assert_eq!(kept, store, "a refused update changes nothing");

    let applied = kept.process_update(&C, &make(342), slot(FULU, 300)).unwrap();
    assert_eq!(applied, Applied { finalized: true, rotated: false, learned_next: true });
    assert_eq!(kept.finalized, Finalized::from_header(&C, &finalized));
    assert_eq!(kept.finalized.block_number, slot(FULU, 128));
    assert_eq!(kept.next.as_ref(), Some(&next.committee));
    assert_eq!(kept.current, current.committee);

    // An update with no next committee finalizes too, and leaves the next one unknown.
    let mut plain = store.clone();
    let update = testing::update(&C, &MAINNET, &current, 512, slot(FULU, 200), slot(FULU, 201), &finalized, None);
    assert_eq!(plain.process_update(&C, &update, slot(FULU, 300)).unwrap(), Applied { finalized: true, rotated: false, learned_next: false });
    assert!(plain.next.is_none());
}

#[test]
fn updates_from_a_wrong_committee_or_fork_domain_are_refused() {
    let (store, current, next) = started(FULU);
    let finalized = finalized_at(slot(FULU, 128));
    let signed_by = |signer: &Committee| testing::update(&C, &MAINNET, signer, 512, slot(FULU, 200), slot(FULU, 201), &finalized, None);

    assert_eq!(store.clone().process_update(&C, &signed_by(&next), slot(FULU, 300)), Err(Error::BadSignature), "the next period's committee, early");
    assert_eq!(store.clone().process_update(&C, &signed_by(&Committee::new(99)), slot(FULU, 300)), Err(Error::BadSignature));

    // Signed under Electra's version, though the slot is Fulu's; under Sepolia's genesis; or as
    // another domain.
    for (version, genesis) in [([5, 0, 0, 0], MAINNET.genesis_validators_root), ([6, 0, 0, 0], SEPOLIA.genesis_validators_root)] {
        let mut update = signed_by(&current);
        testing::resign(&C, &current, 512, &mut update, version, &genesis);
        assert_eq!(store.clone().process_update(&C, &update, slot(FULU, 300)), Err(Error::BadSignature), "{version:?}");
    }

    // The right signature over another header does not hold either.
    let mut update = signed_by(&current);
    update.attested_header.beacon.proposer_index += 1;
    assert!(store.clone().process_update(&C, &update, slot(FULU, 300)).is_err());

    // Bits that name members who did not sign.
    let mut update = signed_by(&current);
    update.sync_aggregate.bits[0] ^= 1;
    assert_eq!(store.clone().process_update(&C, &update, slot(FULU, 300)), Err(Error::BadSignature));
}

#[test]
fn bad_branches_are_refused() {
    let (store, current, next) = started(FULU);
    let finalized = finalized_at(slot(FULU, 128));
    let update = testing::update(&C, &MAINNET, &current, 512, slot(FULU, 200), slot(FULU, 201), &finalized, Some(&next.committee));
    let refused = |update: &LightClientUpdate| store.clone().process_update(&C, update, slot(FULU, 300)).unwrap_err();

    let mut bad = update.clone();
    bad.finality_branch[3][0] ^= 1;
    assert_eq!(refused(&bad), Error::BadFinalityBranch);

    let mut bad = update.clone();
    bad.finalized_header = finalized_at(slot(FULU, 129));
    assert_eq!(refused(&bad), Error::BadFinalityBranch, "a header the state did not finalize");

    let mut bad = update.clone();
    bad.next_sync_committee_branch[2][5] ^= 1;
    assert_eq!(refused(&bad), Error::BadNextCommitteeBranch);

    let mut bad = update.clone();
    bad.next_sync_committee = Some(Committee::new(55).committee);
    assert_eq!(refused(&bad), Error::BadNextCommitteeBranch, "a committee the state did not name");

    let mut bad = update.clone();
    bad.finalized_header.execution_branch[1][0] ^= 1;
    assert_eq!(refused(&bad), Error::BadExecutionBranch);

    let mut bad = update.clone();
    bad.attested_header.execution.gas_used += 1;
    assert_eq!(refused(&bad), Error::BadExecutionBranch);

    // A Deneb-length branch for an Electra state is refused.
    let mut bad = update.clone();
    bad.finality_branch.remove(0);
    assert_eq!(refused(&bad), Error::BadFinalityBranch);
}

#[test]
fn the_store_crosses_a_period_with_the_next_committee_it_learned() {
    let (mut store, current, next) = started(FULU);
    let after = Committee::new(FULU as u32 + 2);

    // Signed in the next period before its committee is known: refused.
    let early = testing::update(&C, &MAINNET, &next, 512, slot(FULU + 1, 10), slot(FULU + 1, 11), &finalized_at(slot(FULU + 1, 0)), None);
    assert_eq!(store.clone().process_update(&C, &early, slot(FULU + 1, 50)), Err(Error::UnknownPeriod));

    // The current committee names the next one, finalizing a header in its own period.
    let learn = testing::update(&C, &MAINNET, &current, 512, slot(FULU, 8000), slot(FULU, 8001), &finalized_at(slot(FULU, 7936)), Some(&next.committee));
    store.process_update(&C, &learn, slot(FULU, 8100)).unwrap();

    // Another update naming another next committee for the same period is refused.
    let other = testing::update(&C, &MAINNET, &current, 512, slot(FULU, 8050), slot(FULU, 8051), &finalized_at(slot(FULU, 8000)), Some(&after.committee));
    assert_eq!(store.clone().process_update(&C, &other, slot(FULU, 8100)), Err(Error::NextCommitteeMismatch));

    // Signed by the next committee, finalizing in the next period: the store moves on.
    let crossing = testing::update(&C, &MAINNET, &next, 400, slot(FULU + 1, 100), slot(FULU + 1, 101), &finalized_at(slot(FULU + 1, 32)), Some(&after.committee));
    assert_eq!(store.clone().process_update(&C, &crossing, slot(FULU + 1, 200)).unwrap(), Applied { finalized: true, rotated: true, learned_next: false });

    // Signed in the next period but finalizing in this one: no rotation yet. The next one's
    // committee does not sign for this one.
    let mut straddling = store.clone();
    let late = testing::update(&C, &MAINNET, &next, 512, slot(FULU + 1, 2), slot(FULU + 1, 3), &finalized_at(slot(FULU, 8100)), None);
    assert_eq!(straddling.process_update(&C, &late, slot(FULU + 1, 200)).unwrap(), Applied { finalized: true, rotated: false, learned_next: false });
    assert_eq!(straddling.current, current.committee);

    store.process_update(&C, &crossing, slot(FULU + 1, 200)).unwrap();
    assert_eq!((store.period(), &store.current, store.next.as_ref()), (FULU + 1, &next.committee, Some(&after.committee)));

    // The old committee signs for nothing any more.
    let stale = testing::update(&C, &MAINNET, &current, 512, slot(FULU + 1, 300), slot(FULU + 1, 301), &finalized_at(slot(FULU + 1, 256)), None);
    assert_eq!(store.clone().process_update(&C, &stale, slot(FULU + 1, 400)), Err(Error::BadSignature));

    // Two periods on is out of reach.
    let far = testing::update(&C, &MAINNET, &after, 512, slot(FULU + 3, 300), slot(FULU + 3, 301), &finalized_at(slot(FULU + 3, 256)), None);
    assert_eq!(store.clone().process_update(&C, &far, slot(FULU + 3, 400)), Err(Error::UnknownPeriod));
}

#[test]
fn replayed_older_and_future_updates_are_refused() {
    let (mut store, current, next) = started(FULU);
    let update = testing::update(&C, &MAINNET, &current, 512, slot(FULU, 200), slot(FULU, 201), &finalized_at(slot(FULU, 128)), Some(&next.committee));
    store.process_update(&C, &update, slot(FULU, 300)).unwrap();

    assert_eq!(store.clone().process_update(&C, &update, slot(FULU, 300)), Err(Error::Stale), "replayed");

    let older = testing::update(&C, &MAINNET, &current, 512, slot(FULU, 190), slot(FULU, 191), &finalized_at(slot(FULU, 96)), None);
    assert_eq!(store.clone().process_update(&C, &older, slot(FULU, 300)), Err(Error::Stale), "older");

    let same = testing::update(&C, &MAINNET, &current, 512, slot(FULU, 250), slot(FULU, 251), &finalized_at(slot(FULU, 128)), None);
    assert_eq!(store.clone().process_update(&C, &same, slot(FULU, 300)), Err(Error::Stale), "the same finalized header, attested later");

    let future = testing::update(&C, &MAINNET, &current, 512, slot(FULU, 400), slot(FULU, 401), &finalized_at(slot(FULU, 352)), None);
    assert_eq!(store.clone().process_update(&C, &future, slot(FULU, 400)), Err(Error::FromTheFuture));
    store.clone().process_update(&C, &future, slot(FULU, 401)).unwrap();

    let backwards = testing::update(&C, &MAINNET, &current, 512, slot(FULU, 400), slot(FULU, 400), &finalized_at(slot(FULU, 352)), None);
    assert_eq!(store.clone().process_update(&C, &backwards, slot(FULU, 500)), Err(Error::BadSlots));
}

#[test]
fn electra_s_longer_branches_follow_deneb_s_across_the_fork() {
    let (mut store, current, next) = started(DENEB_LAST);
    assert_eq!(MAINNET.fork_at_slot(store.finalized.beacon.slot).fork, Fork::Deneb);

    // In Deneb: gindices 105 and 55, branches of 6 and 5.
    let deneb = testing::update(&C, &MAINNET, &current, 512, slot(DENEB_LAST, 8000), slot(DENEB_LAST, 8001), &finalized_at(slot(DENEB_LAST, 7936)), Some(&next.committee));
    assert_eq!(deneb.finality_branch.len(), 6);
    store.process_update(&C, &deneb, slot(DENEB_LAST, 9000)).unwrap();

    // In Electra's first period: gindices 169 and 87, a Deneb header finalized under an Electra
    // state, signed under Electra's version.
    let after = Committee::new(77);
    let electra = testing::update(&C, &MAINNET, &next, 512, slot(ELECTRA_FIRST, 40), slot(ELECTRA_FIRST, 41), &finalized_at(slot(DENEB_LAST, 8160)), Some(&after.committee));
    assert_eq!((electra.finality_branch.len(), electra.next_sync_committee_branch.len()), (7, 6));
    let bytes = electra.encode(&MAINNET);
    let decoded = LightClientUpdate::decode(&bytes, &MAINNET).unwrap();
    assert_eq!(store.process_update(&C, &decoded, slot(ELECTRA_FIRST, 100)).unwrap(), Applied { finalized: true, rotated: false, learned_next: false });

    // Signed in Electra's first slot, its fork version is still Deneb's: the slot before.
    let first = testing::update(&C, &MAINNET, &next, 512, slot(ELECTRA_FIRST, 0) - 1, slot(ELECTRA_FIRST, 0), &finalized_at(slot(DENEB_LAST, 8170)), None);
    store.clone().process_update(&C, &first, slot(ELECTRA_FIRST, 100)).unwrap();
}

#[test]
fn ancestors_within_8192_slots_are_proven_by_block_roots() {
    let held_slot = slot(FULU, 9000);
    let ancestor = finalized_at(held_slot - 5000);
    let mut state = State::new(Fork::Fulu, held_slot);
    let mut roots = BlockRoots::new(held_slot);
    roots.set(ancestor.beacon.slot, ancestor.beacon.hash_tree_root(&C));
    state.fields[testing::BLOCK_ROOTS as usize] = roots.root(&C);
    let held = Finalized::from_header(&C, &testing::header(&C, &MAINNET, held_slot, state.root(&C), testing::execution(&C, 1, &[] as &[&[u8]])));
    let branch = roots.branch(&C, &state, ancestor.beacon.slot);

    assert_eq!(branch.len(), 19);
    assert_eq!(prove_ancestor(&C, &MAINNET, &held, &ancestor, &branch), Ok(Finalized::from_header(&C, &ancestor)));

    let other = finalized_at(held_slot - 5001);
    assert_eq!(prove_ancestor(&C, &MAINNET, &held, &other, &branch), Err(Error::BadAncestorBranch));
    let far = finalized_at(held_slot - 8193);
    assert_eq!(prove_ancestor(&C, &MAINNET, &held, &far, &branch), Err(Error::NotAncestor));
    assert_eq!(prove_ancestor(&C, &MAINNET, &held, &finalized_at(held_slot), &branch), Err(Error::NotAncestor));
}

#[test]
fn transactions_are_proven_under_a_payload_s_transactions_root() {
    let transactions: Vec<Vec<u8>> = (0..5u8).map(|index| vec![index; 40 + 37 * usize::from(index)]).collect();
    let transactions_root = tx::transactions_root(&C, &transactions);

    for (index, transaction) in transactions.iter().enumerate() {
        let branch = tx::transaction_branch(&C, &transactions, index as u64);
        assert_eq!(branch.len(), tx::TRANSACTION_BRANCH_LEN);
        assert!(tx::verify_transaction(&C, &transactions_root, transaction, index as u64, &branch));
        assert!(!tx::verify_transaction(&C, &transactions_root, &transactions[(index + 1) % 5], index as u64, &branch), "another transaction");
        assert!(!tx::verify_transaction(&C, &transactions_root, transaction, index as u64 ^ 1, &branch), "another place");
        assert!(!tx::verify_transaction(&C, &transactions_root, transaction, index as u64, &branch[..20]), "short");
    }

    // A place past the list's length is refused even with a branch of zeros that would lead there.
    let branch = tx::transaction_branch(&C, &transactions, 6);
    assert!(!tx::verify_transaction(&C, &transactions_root, &[], 6, &branch));
}

use testing::{number, rlp_list, rlp_string};

#[test]
fn each_transaction_type_s_recipient_and_calldata_are_read() {
    let to = [0xaa; 20];
    let data: Vec<u8> = (0..80).collect();
    let (signature, empty_list) = (vec![number(1), rlp_string(&[0x11; 32]), rlp_string(&[0x22; 32])], rlp_list(&[]));
    let typed = |tx_type: u8, fields: Vec<Vec<u8>>| [vec![tx_type], rlp_list(&fields)].concat();

    // Legacy, EIP-155 on chain 1: v = 37.
    let legacy = rlp_list(&[number(9), number(20_000_000_000), number(21_000), rlp_string(&to), number(0), rlp_string(&data), number(37), rlp_string(&[1; 32]), rlp_string(&[2; 32])]);
    // EIP-2930.
    let access = typed(1, [vec![number(11_155_111), number(9), number(1), number(21_000), rlp_string(&to), number(5), rlp_string(&data), empty_list.clone()], signature.clone()].concat());
    // EIP-1559.
    let dynamic = typed(2, [vec![number(1), number(9), number(1), number(2), number(21_000), rlp_string(&to), number(0), rlp_string(&data), empty_list.clone()], signature.clone()].concat());
    // EIP-4844, without its blobs as payloads carry it.
    let blob = typed(3, [vec![number(1), number(9), number(1), number(2), number(21_000), rlp_string(&to), number(0), rlp_string(&data), empty_list.clone(), number(1), rlp_list(&[rlp_string(&[1; 32])])], signature.clone()].concat());
    // EIP-7702, with one authorization.
    let authorization = rlp_list(&[number(1), rlp_string(&[0xbb; 20]), number(0), number(1), rlp_string(&[3; 32]), rlp_string(&[4; 32])]);
    let delegating = typed(4, [vec![number(17_000), number(9), number(1), number(2), number(21_000), rlp_string(&to), number(0), rlp_string(&data), empty_list.clone(), rlp_list(&[authorization])], signature.clone()].concat());

    for (bytes, tx_type, chain_id) in [(&legacy, 0, 1), (&access, 1, 11_155_111), (&dynamic, 2, 1), (&blob, 3, 1), (&delegating, 4, 17_000)] {
        assert_eq!(decode_transaction(bytes), Ok(tx::Call { tx_type, chain_id: Some(chain_id), to: Some(to), data: &data }), "type {tx_type}");
    }

    // A creation has no recipient, and a legacy one from before EIP-155 no chain.
    let creation = typed(2, [vec![number(1), number(9), number(1), number(2), number(21_000), rlp_string(&[]), number(0), rlp_string(&[0x60, 0x80]), empty_list.clone()], signature.clone()].concat());
    assert_eq!(decode_transaction(&creation).unwrap().to, None);
    let unprotected = rlp_list(&[number(9), number(1), number(21_000), rlp_string(&to), number(0), rlp_string(&[]), number(27), rlp_string(&[1; 32]), rlp_string(&[2; 32])]);
    assert_eq!(decode_transaction(&unprotected).unwrap(), tx::Call { tx_type: 0, chain_id: None, to: Some(to), data: &[] });

    // A blob or 7702 transaction always has a recipient.
    for tx_type in [3, 4] {
        let extra = match tx_type {
            3 => vec![number(1), rlp_list(&[rlp_string(&[1; 32])])],
            _ => vec![rlp_list(&[])],
        };
        let no_recipient = typed(tx_type, [vec![number(1), number(9), number(1), number(2), number(21_000), rlp_string(&[]), number(0), rlp_string(&data), empty_list.clone()], extra, signature.clone()].concat());
        assert!(decode_transaction(&no_recipient).is_err(), "type {tx_type}");
    }
}

#[test]
fn malformed_rlp_is_refused() {
    let to = rlp_string(&[0xaa; 20]);
    let fields = |count: usize| (0..count).map(|index| if index == 5 { to.clone() } else { number(index as u64 + 1) }).collect::<Vec<_>>();
    let good = [vec![2], rlp_list(&fields(12))].concat();
    assert!(decode_transaction(&good).is_ok());

    for (bad, why) in [
        ([vec![2], rlp_list(&fields(11))].concat(), "a field short"),
        ([vec![2], rlp_list(&fields(13))].concat(), "a field more"),
        ([good.clone(), vec![0]].concat(), "a byte after"),
        (good[..good.len() - 1].to_vec(), "cut short"),
        ([vec![5], rlp_list(&fields(12))].concat(), "an unknown type"),
        (vec![], "nothing"),
        ([vec![2], rlp_string(&[1, 2, 3])].concat(), "a string, not a list"),
    ] {
        assert!(decode_transaction(&bad).is_err(), "{why}");
    }

    // Non-canonical forms: a byte under 0x80 written long, and a long length under 56.
    assert!(tx::rlp::item(&[0x81, 0x05]).is_err());
    assert!(tx::rlp::item(&[0xb8, 0x02, 1, 2]).is_err());
    assert!(tx::rlp::item(&[0xb9, 0x00, 0x40]).is_err(), "a length with a leading zero");
    assert!(tx::rlp::uint(&[0, 1]).is_err());
    assert_eq!(tx::rlp::item(&[0x82, 1, 2, 9]).unwrap(), (tx::rlp::Item::String(&[1, 2]), &[9u8][..]));
}

/// A network shaped like Gnosis's beacon chain, as one not built in: 5-second slots, 16-slot
/// epochs, and periods of 512 epochs. Its roots and epochs are as remembered, for the test alone,
/// not a preset to follow Gnosis by.
fn gnosis() -> Network {
    Network {
        name: "gnosis".into(),
        genesis_validators_root: root("f5dcb5564e829aab27264b9becd5dfaa017085611224cb3036f573368dbb9d47"),
        genesis_time: 1_638_993_340,
        seconds_per_slot: 5,
        slots_per_epoch: 16,
        epochs_per_sync_committee_period: 512,
        forks: vec![
            ForkAt { fork: Fork::Phase0, version: [0, 0, 0, 0x64], epoch: 0 },
            ForkAt { fork: Fork::Altair, version: [1, 0, 0, 0x64], epoch: 512 },
            ForkAt { fork: Fork::Bellatrix, version: [2, 0, 0, 0x64], epoch: 385_536 },
            ForkAt { fork: Fork::Capella, version: [3, 0, 0, 0x64], epoch: 648_704 },
            ForkAt { fork: Fork::Deneb, version: [4, 0, 0, 0x64], epoch: 889_856 },
            ForkAt { fork: Fork::Electra, version: [5, 0, 0, 0x64], epoch: 1_337_856 },
        ]
        .into(),
    }
}

#[test]
fn a_network_is_a_whole_definition_its_rules_take_their_timing_from() {
    let gnosis = gnosis();
    assert_eq!(gnosis.problem(), None);
    assert_eq!((gnosis.slots_per_period(), MAINNET.slots_per_period()), (8_192, 8_192));
    assert_eq!(gnosis.epoch_at_slot(32), 2, "16-slot epochs");
    assert_eq!(gnosis.slot_at(1_638_993_340 + 50), 10, "5-second slots");
    assert_eq!(gnosis.fork_at_slot(1_337_856 * 16).fork, Fork::Electra);
    assert_eq!(gnosis.fork_at_slot(1_337_856 * 16 - 1).fork, Fork::Deneb);

    // A store on it bootstraps and follows updates as on mainnet, by its own epochs and domain.
    let period = 1_337_856 * 16 / 8_192 + 3;
    let (current, next) = (Committee::new(40), Committee::new(41));
    let at = |plus: u64| period * gnosis.slots_per_period() + plus;
    let (bootstrap, trusted) = testing::bootstrap(&C, &gnosis, at(64), &current.committee);
    let mut store = Store::bootstrap(&C, &gnosis, &trusted, &LightClientBootstrap::decode(&bootstrap.encode(&gnosis), &gnosis).unwrap()).unwrap();
    let finalized = testing::header(&C, &gnosis, at(128), [0x77; 32], testing::execution(&C, 1, &[] as &[&[u8]]));
    let update = testing::update(&C, &gnosis, &current, 400, at(200), at(201), &finalized, Some(&next.committee));
    assert!(store.clone().process_update(&C, &LightClientUpdate::decode(&update.encode(&gnosis), &gnosis).unwrap(), at(300)).unwrap().finalized);

    // Signed under mainnet's domain, the same update does not hold.
    let mut wrong = update.clone();
    testing::resign(&C, &current, 400, &mut wrong, [5, 0, 0, 0], &MAINNET.genesis_validators_root);
    assert_eq!(store.process_update(&C, &wrong, at(300)), Err(Error::BadSignature));
}

#[test]
fn networks_encode_as_ssz_and_decode_only_when_whole() {
    for network in [MAINNET.clone(), SEPOLIA.clone(), HOODI.clone(), gnosis()] {
        let bytes = network.encode();
        assert_eq!(bytes.len(), 72 + network.name.len() + 13 * network.forks.len());
        assert_eq!(Network::decode(&bytes).unwrap(), network, "{}", network.name);
        assert!(Network::decode(&bytes[..bytes.len() - 1]).is_err(), "{}: a fork cut short", network.name);
    }

    // Each part of a definition moves its root.
    let base = gnosis().hash_tree_root(&C);
    assert_ne!(base, MAINNET.hash_tree_root(&C));
    assert_ne!(base, Network { seconds_per_slot: 6, ..gnosis() }.hash_tree_root(&C));
    assert_ne!(base, Network { name: "gnosis2".into(), ..gnosis() }.hash_tree_root(&C));

    // What problem refuses, decode refuses.
    let refused = |network: Network| Network::decode(&network.encode()).unwrap_err();
    let mut forks = gnosis().forks.into_owned();
    assert!(matches!(refused(Network { slots_per_epoch: 0, ..gnosis() }), Error::Malformed(why) if why.contains("at least 1")));
    assert!(matches!(refused(Network { name: "".into(), ..gnosis() }), Error::Malformed(why) if why.contains("1 to 64")));
    assert!(matches!(refused(Network { forks: forks[1..].to_vec().into(), ..gnosis() }), Error::Malformed(why) if why.contains("phase0")));
    forks.swap(2, 3);
    assert!(matches!(refused(Network { forks: forks.clone().into(), ..gnosis() }), Error::Malformed(why) if why.contains("in order")));
    forks.swap(2, 3);
    forks[3].version = forks[2].version;
    assert!(matches!(refused(Network { forks: forks.into(), ..gnosis() }), Error::Malformed(why) if why.contains("version of its own")));

    // A fork tag this build lacks, and a name that is not UTF-8.
    let mut bytes = gnosis().encode();
    let last = bytes.len() - 13;
    bytes[last] = 200;
    assert!(Network::decode(&bytes).is_err());
    let mut bytes = gnosis().encode();
    bytes[72] = 0xff;
    assert!(Network::decode(&bytes).is_err());
}
