//! What a transaction's bytes are, new kind beside old: the encodings every chain's old
//! transactions and receipts were written in, pinned, and what a batch's hostile bytes come to.

use {
    super::*,
};

#[cfg(any(feature = "node", all(feature = "blake3", feature = "ed25519", feature = "xxh3")))]
use crate::structs::hash::HashType;
#[cfg(all(feature = "blake3", feature = "ed25519", feature = "xxh3"))]
use crate::structs::hex::{from_hex, to_hex};
#[cfg(all(feature = "node", feature = "blake3", feature = "ed25519"))]
use crate::structs::receipt::Payment;
#[cfg(feature = "node")]
use {
    crate::chain::{TxReceipt, TxStatus},
    crate::structs::receipt::{CommittedOutcome, StepEffect, StepOutcome},
};

const TO: Address = Address::Ed25519([9; 32]);
const ACCOUNT: Address = Address::Multisig([6; 32]);

fn key() -> Keypair {
    Keypair::from_seed([7; 32])
}

/// The chain they are signed for.
fn chain() -> Hash {
    Hash::zero(crate::structs::hash::HashType::default_chain())
}

fn fee() -> FeeOffer {
    FeeOffer { gas_limit: 21000, max_price: u256::new(2), tip: u256::ONE }
}

/// Transactions of old kinds, with the encoding and id each had before a batch existed: taken from
/// the code as it stood then, so a change to any existing kind's tag or fields shows here. They hold
/// for the scheme and hash they were pinned under, the defaults.
#[cfg(all(feature = "blake3", feature = "ed25519", feature = "xxh3"))]
fn old() -> Vec<(Tx, &'static str, &'static str)> {
    let me = key().public().address();

    vec![
        (
            Tx::Transfer { from: me, to: TO, amount: Amount::U128(5) },
            "0x0000ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c0009090909090909090909090909090909090909090909090909090909090909090005000000000000000000000000000000",
            "0xabe8d99b71b4f95ef40467231413cc134afc7066c0bdf74a876168ae72bb6b56",
        ),
        (Tx::Burn { from: me, amount: Amount::U128(5) }, "0x0200ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c0005000000000000000000000000000000", "0xd29f5e3d5ed7a9aec6b8447d9c276331804dbf517970ad4753ae84cf7451433a"),
        (
            Tx::ContractCall {
                to: Address::Contract([3; 32]),
                func: crate::crypto::hasher::Hasher::digest(HashType::Xxh3_64, b"transfer"),
                amount: Amount::U128(0),
                data: vec![TxCallArgumentType::Address(TO), TxCallArgumentType::U256(u256::new(9))],
            },
            "0x03010303030303030303030303030303030303030303030303030303030303030303042ecc75d24755867800000000000000000000000000000000000200000000000000000009090909090909090909090909090909090909090909090909090909090909090a0900000000000000000000000000000000000000000000000000000000000000",
            "0x5c379832c02808451e99a9deeb3e4b0d07457e8bc31f8d1d86cf6e3b3fc2b81e",
        ),
        (Tx::Deploy { code: Code::Lua("return {}".to_string()) }, "0x0400090000000000000072657475726e207b7d", "0x293915d553adb2c061b6d0018fc05faaa06b91aa3d7fd0515f8fa9d194995a5b"),
        (
            Tx::EvmCall { to: Address::Evm([4; 20]), value: Amount::U128(1), input: vec![1, 2, 3] },
            "0x0e04040404040404040404040404040404040404040400010000000000000000000000000000000300000000000000010203",
            "0x2470a4a436f83b7fb231674fe9ab935bb896f865cebd2f7bbed7963f2cee8727",
        ),
        (Tx::Name { name: "alice".to_string(), action: NameAction::Register { years: 1 } }, "0x0f0500000000000000616c6963650001000000", "0x1072185b90f5333abe1d2989e469865a70977eee4886b1cd80fb900d303c7de7"),
        (
            Tx::Claim { height: 3, to: TO, earned: u256::new(8), index: 1, path: vec![Hash::zero(HashType::default_chain())] },
            "0x170300000000000000000909090909090909090909090909090909090909090909090909090909090909080000000000000000000000000000000000000000000000000000000000000001000000000000000100000000000000010000000000000000000000000000000000000000000000000000000000000000",
            "0x23dea5448aa417e6b334a4ecd6c83298bb4526883f3d1992e10d0e5e6342cf98",
        ),
        (Tx::HandDirectory { to: None }, "0x1900", "0x77c6ddab7b8c37d0d77e5ffe82d66cedd495383da803fce31bbfac62db0de926"),
        (
            Tx::Multisig { account: ACCOUNT, tx: Box::new(Tx::Transfer { from: ACCOUNT, to: TO, amount: Amount::U128(5) }), signatures: Vec::new() },
            "0x120606060606060606060606060606060606060606060606060606060606060606060006060606060606060606060606060606060606060606060606060606060606060600090909090909090909090909090909090909090909090909090909090909090900050000000000000000000000000000000000000000000000",
            "0xd1cbdc121372e3653d5834d50d763dcfff567faa8e58d4e998f38ab1e9187591",
        ),
        (
            Tx::ApproveMultisig { account: ACCOUNT, nonce: 2, fee: FeeOffer::NONE, tx: Box::new(Tx::Burn { from: ACCOUNT, amount: Amount::U128(1) }) },
            "0x150606060606060606060606060606060606060606060606060606060606060606060200000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000020606060606060606060606060606060606060606060606060606060606060606060001000000000000000000000000000000",
            "0x2b6265ff2003460d2c14c67adc73855a7daf5705b6487056afa1aec4c06a0dda",
        ),
    ]
}

/// A transaction signed as it was before a signature named its chain: what the ids old() pins were
/// made from, and what an older build's rows still hold.
#[cfg(all(feature = "blake3", feature = "ed25519", feature = "xxh3"))]
fn signed_for_no_chain(tx: Tx) -> SignedTx {
    let (nonce, fee, signer) = (3u64, fee(), key().public());
    let bytes = [b"chainv4 transaction".as_slice(), &tx.hashable_bytes_with_tag(), &nonce.to_le_bytes(), &fee.hashable_bytes_with_tag(), &signer.hashable_bytes_with_tag()].concat();
    let signature = key().sign(&bytes);

    SignedTx { tx, nonce, fee, signer, signature }
}

// A signature now names its chain, which changes what it signs and so every transaction's id. No
// byte of how a transaction is encoded moved, and this is what shows it: the bytes pinned stand as
// they were, and so does each id, from a signature made as it was then.
#[cfg(all(feature = "blake3", feature = "ed25519", feature = "xxh3"))]
#[test]
fn every_old_transaction_keeps_its_exact_bytes_and_id() {
    for (tx, bytes, id) in old() {
        assert_eq!(to_hex(&wincode::serialize(&tx).unwrap()), bytes, "{tx:?}");
        assert_eq!(wincode::deserialize::<Tx>(&from_hex(bytes).unwrap()).unwrap(), tx);

        let signed = signed_for_no_chain(tx);

        assert_eq!(signed.id(HashType::default_chain()).to_string(), id, "{signed:?}");
    }
}

// The same transactions signed as they are now, for the chain chain() names: what changing the
// bytes a signature covers would change shows here, as a new genesis is the only way to do that.
#[cfg(all(feature = "blake3", feature = "ed25519", feature = "xxh3"))]
#[test]
fn every_transaction_signed_for_a_chain_differs_from_the_old_one_in_its_signature_alone() {
    let ids = [
        "0x101c6210787596850b8919291b20ae698e5654558cab2bc814c6cfd3772f7828",
        "0x82ba4509071dc632b5b2c66518be0c4a2d6c332dc3e161fb24d85018ed906ba1",
        "0x1115111ba10d2329ee83707a05ce9cfe7feba2f74b548407c234ff749b3687db",
        "0x08adda65df652d19b1bec02c3b12e213520e6fa72bf8008c808853f538d6868a",
        "0x18a25d38b38d3e5d05cc3a333024288d6f03060213dd964dce395d2c3fb36014",
        "0x8a3dd07ddfa52022ab152bf1f04327975c36577131ee1c218a2ccc3eace19686",
        "0x1255e363cd49ce7a0cac3efe48e8805e17380377ad15b7fb15890b801c321428",
        "0x4c183f277f6d9abc38695366d319689543e52df563b76097beee274adfa0ecfb",
        "0x1aa75b0de476c79494207c11e86b794cebd34415be3e85389b1fbb3ff34de53b",
        "0xceb92caf22a8f091ae94f9ff5921612f71f89d1ea920736c41aacb0d5ef02719",
    ];

    assert_eq!(old().len(), ids.len());

    for ((tx, _, old_id), id) in old().into_iter().zip(ids) {
        let then = wincode::serialize(&signed_for_no_chain(tx.clone())).unwrap();
        let signed = SignedTx::sign(tx, 3, fee(), &key(), &chain());
        let now = wincode::serialize(&signed).unwrap();

        assert_eq!(signed.id(HashType::default_chain()).to_string(), id, "{signed:?}");
        assert_ne!(old_id, id);
        assert_eq!(now.len(), then.len());
        assert_eq!(now[..now.len() - 64], then[..then.len() - 64], "the tag, the transaction, nonce, fee offer and signer stand");
        assert_ne!(now[now.len() - 64..], then[then.len() - 64..]);
        assert_eq!(signed.verify_on(&chain()), Ok(()));
    }
}

#[cfg(all(feature = "node", feature = "blake3", feature = "ed25519"))]
#[test]
fn every_old_receipt_keeps_its_exact_bytes_and_leaf() {
    let block = Hash::zero(HashType::default_chain());
    let held = |status| TxReceipt { block, index: 1, fuel_used: 2, gas_used: 3, fee: u256::new(4), status, events: Vec::new(), trace: Default::default(), payments: vec![Payment { from: TO, to: TO, amount: u256::ONE }] };
    let pinned = [
        (TxStatus::Done, "0x0100000000000000000000000000000000000000000000000000000000000000000100000002000000000000000300000000000000040000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000001000000000000000009090909090909090909090909090909090909090909090909090909090909090009090909090909090909090909090909090909090909090909090909090909090100000000000000000000000000000000000000000000000000000000000000", "0x4fa1e1abaedcd1fa657420f2483f8743828d92892a64b3c0ec88614b9229ed7f"),
        (TxStatus::Deployed(TO), "0x0100000000000000000000000000000000000000000000000000000000000000000100000002000000000000000300000000000000040000000000000000000000000000000000000000000000000000000000000001000909090909090909090909090909090909090909090909090909090909090909000000000000000000000000000000000000000001000000000000000009090909090909090909090909090909090909090909090909090909090909090009090909090909090909090909090909090909090909090909090909090909090100000000000000000000000000000000000000000000000000000000000000", "0x2e2bc4d79657aa14b10938734086a135d8645a12739bba610b6ee137ed596433"),
        (TxStatus::Returned(vec![TxCallArgumentType::U64(4)]), "0x01000000000000000000000000000000000000000000000000000000000000000001000000020000000000000003000000000000000400000000000000000000000000000000000000000000000000000000000000020100000000000000070400000000000000000000000000000000000000000000000000000001000000000000000009090909090909090909090909090909090909090909090909090909090909090009090909090909090909090909090909090909090909090909090909090909090100000000000000000000000000000000000000000000000000000000000000", "0x054fb88c880cd8dc47f58c80e03f1dcba568cde0b13b9ec234b2b3b72b8f921d"),
        (TxStatus::Failed("no".to_string()), "0x010000000000000000000000000000000000000000000000000000000000000000010000000200000000000000030000000000000004000000000000000000000000000000000000000000000000000000000000000302000000000000006e6f000000000000000000000000000000000000000001000000000000000009090909090909090909090909090909090909090909090909090909090909090009090909090909090909090909090909090909090909090909090909090909090100000000000000000000000000000000000000000000000000000000000000", "0x9eacfcb7009724ead1d6d0a751f8dc91888bbc1ed11e81d42d712f1782ea5836"),
        (TxStatus::Upgraded(TO), "0x0100000000000000000000000000000000000000000000000000000000000000000100000002000000000000000300000000000000040000000000000000000000000000000000000000000000000000000000000004000909090909090909090909090909090909090909090909090909090909090909000000000000000000000000000000000000000001000000000000000009090909090909090909090909090909090909090909090909090909090909090009090909090909090909090909090909090909090909090909090909090909090100000000000000000000000000000000000000000000000000000000000000", "0xd77bb0220adf26ad0bc0ea95aec3872b427ef1d6a0b73cc4ddfad55f442e1a1e"),
        (TxStatus::Decided { decision: 1, kind: crate::governance::ChangeKind::Name, version: 2 }, "0x010000000000000000000000000000000000000000000000000000000000000000010000000200000000000000030000000000000004000000000000000000000000000000000000000000000000000000000000000501000000000000000a0200000000000000000000000000000000000000000000000000000001000000000000000009090909090909090909090909090909090909090909090909090909090909090009090909090909090909090909090909090909090909090909090909090909090100000000000000000000000000000000000000000000000000000000000000", "0x5bfb3418859e8d369ae4ba6cc23ad1b37f49ec68c1e242ca636f2d51cbd803d9"),
    ];

    for (status, bytes, leaf) in pinned {
        let receipt = held(status);

        assert_eq!(to_hex(&wincode::serialize(&receipt).unwrap()), bytes, "{receipt:?}");
        assert_eq!(wincode::deserialize::<TxReceipt>(&from_hex(bytes).unwrap()).unwrap(), receipt);
        assert_eq!(receipt.committed(block).leaf(HashType::default_chain()).to_string(), leaf, "{receipt:?}");
    }
}

fn batch(steps: Vec<Tx>) -> Tx {
    Tx::Batch { steps }
}

fn paying() -> Tx {
    Tx::Transfer { from: key().public().address(), to: TO, amount: Amount::U128(5) }
}

/// A batch's bytes, for a given count of steps whatever it holds: its tag, its length, its steps.
fn batch_bytes(count: u64, steps: &[Tx]) -> Vec<u8> {
    let mut bytes = vec![26];
    bytes.extend_from_slice(&count.to_le_bytes());

    for step in steps {
        bytes.extend(wincode::serialize(step).unwrap());
    }

    bytes
}

#[test]
fn a_batch_reads_back_as_it_was_written_under_the_next_free_tag() {
    let tx = batch(vec![paying(), Tx::Deploy { code: Code::Lua("return {}".to_string()) }]);
    let bytes = wincode::serialize(&tx).unwrap();

    assert_eq!(bytes[0], 26);
    assert_eq!(bytes, batch_bytes(2, &[paying(), Tx::Deploy { code: Code::Lua("return {}".to_string()) }]));
    assert_eq!(wincode::deserialize::<Tx>(&bytes).unwrap(), tx);
    assert_eq!(wincode::serialized_size(&tx).unwrap(), bytes.len() as u64);

    let signed = SignedTx::sign(tx.clone(), 1, fee(), &key(), &chain());
    let round: SignedTx = wincode::deserialize(&wincode::serialize(&signed).unwrap()).unwrap();

    assert_eq!(round, signed);
    assert_eq!(round.verify_on(&chain()), Ok(()));

    let carried = Tx::Multisig { account: ACCOUNT, tx: Box::new(batch(vec![Tx::Burn { from: ACCOUNT, amount: Amount::U128(1) }])), signatures: Vec::new() };

    assert_eq!(wincode::deserialize::<Tx>(&wincode::serialize(&carried).unwrap()).unwrap(), carried, "the deepest a transaction goes");
}

#[test]
fn a_batch_of_thirty_three_steps_encodes_for_its_error_to_say_so_and_never_decodes() {
    let long = batch(vec![paying(); MAX_BATCH_STEPS + 1]);
    let bytes = wincode::serialize(&long).unwrap();

    assert!(wincode::deserialize::<Tx>(&bytes).is_err());
    assert_eq!(long.verify(), Err(VerifyError::BatchSize));
    assert!(wincode::deserialize::<Tx>(&wincode::serialize(&batch(vec![paying(); MAX_BATCH_STEPS])).unwrap()).is_ok());
}

#[test]
fn a_length_of_anything_past_a_batch_sets_nothing_aside_and_does_not_decode() {
    for count in [33, 1 << 20, u64::from(u32::MAX), u64::MAX] {
        assert!(wincode::deserialize::<Tx>(&batch_bytes(count, &[paying()])).is_err(), "{count}");
    }

    // Short of what it says it holds.
    assert!(wincode::deserialize::<Tx>(&batch_bytes(2, &[paying()])).is_err());
}

#[test]
fn bytes_nesting_without_end_stop_decoding_rather_than_the_node() {
    // A multisig holding a multisig, far past any thread's stack had each level cost a frame: this
    // crashed the decoder, and with it the node, before it counted.
    let account = wincode::serialize(&ACCOUNT).unwrap();
    let multisigs: Vec<u8> = std::iter::repeat_n([&[18u8][..], &account].concat(), 200_000).flatten().collect();

    assert!(wincode::deserialize::<Tx>(&multisigs).is_err());

    // Approvals, which also hold a transaction.
    let approval = [&[21u8][..], &account, &[0; 8], &wincode::serialize(&FeeOffer::NONE).unwrap()].concat();
    let approvals: Vec<u8> = std::iter::repeat_n(approval, 100_000).flatten().collect();

    assert!(wincode::deserialize::<Tx>(&approvals).is_err());

    // Batches of a step each, and the two taking turns.
    let one_step = [&[26u8][..], &1u64.to_le_bytes()].concat();
    let batches: Vec<u8> = std::iter::repeat_n(one_step.clone(), 500_000).flatten().collect();

    assert!(wincode::deserialize::<Tx>(&batches).is_err());

    let turns: Vec<u8> = std::iter::repeat_n([&one_step[..], &[18u8][..], &account].concat(), 100_000).flatten().collect();

    assert!(wincode::deserialize::<Tx>(&turns).is_err());

    // What it refused is forgotten: the next decode on this thread starts from nothing deep.
    assert!(wincode::deserialize::<Tx>(&wincode::serialize(&Tx::Multisig { account: ACCOUNT, tx: Box::new(batch(vec![Tx::Burn { from: ACCOUNT, amount: Amount::U128(1) }])), signatures: Vec::new() }).unwrap()).is_ok());
}

#[test]
fn what_nests_only_as_far_as_decoding_lets_it_is_refused_by_verifying() {
    let inner = batch(vec![paying()]);

    // A batch in a batch decodes, its depth bounded, and is invalid.
    let nested = batch(vec![inner.clone()]);
    let read: Tx = wincode::deserialize(&wincode::serialize(&nested).unwrap()).unwrap();

    assert_eq!(read.verify(), Err(VerifyError::BatchStep));

    // One level more does not decode at all.
    let deeper = batch(vec![nested]);

    assert!(wincode::deserialize::<Tx>(&wincode::serialize(&deeper).unwrap()).is_err());

    let double = Tx::Multisig { account: ACCOUNT, tx: Box::new(Tx::Multisig { account: ACCOUNT, tx: Box::new(paying()), signatures: Vec::new() }), signatures: Vec::new() };

    assert_eq!(wincode::deserialize::<Tx>(&wincode::serialize(&double).unwrap()).unwrap().verify(), Err(VerifyError::BadMultisig));
}

#[test]
fn random_bytes_after_the_batch_tag_never_panic() {
    // A small generator of our own: nothing here needs to be cryptographic, only to cover shapes.
    let mut state = 0x1234_5678_9abc_def0u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;

        state
    };

    for _ in 0..2_000 {
        let length = (next() % 400) as usize;
        let mut bytes = vec![26];
        bytes.extend((0..length).map(|_| next() as u8));

        // Small counts are the interesting ones: a reader that gets as far as the steps.
        if bytes.len() > 9 {
            bytes[1..9].copy_from_slice(&(next() % 40).to_le_bytes());
        }

        if let Ok(tx) = wincode::deserialize::<Tx>(&bytes) {
            let _ = tx.verify();
        }
    }
}

#[cfg(feature = "node")]
#[test]
fn a_batchs_receipts_read_back_and_its_tags_follow_the_old_ones() {
    let block = Hash::zero(HashType::default_chain());
    let done = StepOutcome { effect: StepEffect::Deployed(TO), events: 1, payments: 2 };
    let receipts = [TxStatus::Batch(vec![done.clone()]), TxStatus::BatchFailed { step: 2, why: "no".to_string() }];
    let committed = [CommittedOutcome::Batch(vec![done]), CommittedOutcome::BatchFailed { step: 2 }];

    for (status, outcome) in receipts.into_iter().zip(committed) {
        let receipt = TxReceipt { block, index: 0, fuel_used: 0, gas_used: 0, fee: u256::ZERO, status, events: Vec::new(), trace: Default::default(), payments: Vec::new() };

        assert_eq!(wincode::deserialize::<TxReceipt>(&wincode::serialize(&receipt).unwrap()).unwrap(), receipt);
        assert_eq!(receipt.committed(block).outcome, outcome);
    }

    assert_eq!(wincode::serialize(&CommittedOutcome::Batch(Vec::new())).unwrap()[0], 6);
    assert_eq!(wincode::serialize(&CommittedOutcome::BatchFailed { step: 0 }).unwrap()[0], 7);
}

#[test]
fn a_later_deploy_s_nonce_is_none_an_account_ever_has_and_differs_by_batch_and_place() {
    assert_eq!(batch_deploy_nonce(7, 0), 7);

    let nonces: std::collections::BTreeSet<u64> = (0..1_000u64).flat_map(|nonce| (1..32u32).map(move |before| batch_deploy_nonce(nonce, before))).collect();

    assert_eq!(nonces.len(), 1_000 * 31, "each batch's each place has its own");
    assert!(nonces.iter().all(|nonce| *nonce >= 1 << 63), "no account's nonce reaches them, one a transaction");
}
