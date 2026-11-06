use {
    super::*,
    chainv4::crypto::bitcoin::psbt::{GLOBAL_VERSION, IN_NON_WITNESS_UTXO, IN_WITNESS_UTXO},
    chainv4::crypto::bitcoin::tx::{Reader, Transaction, compact},
    crate::airgap::Message,
    chainv4::crypto::bitcoin::sha256d,
    chainv4::crypto::bitcoin::tx::base58check,
    chainv4::crypto::mnemonic::Phrase,
    chainv4::structs::hex::from_hex,
    serde_json::Value,
};

/// PSBTs as a Bitcoin wallet makes them, with bitcoinjs-lib and Keystone's registry, and the
/// signatures bitcoinjs-lib makes for them: made by wallets/make.js.
const WALLETS: &str = include_str!("wallets.json");

fn wallets() -> Value {
    serde_json::from_str(WALLETS).unwrap()
}

fn seed() -> Seed {
    Phrase::parse("abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about").unwrap().seed("")
}

fn hex(text: &str) -> Vec<u8> {
    from_hex(text).unwrap()
}

fn psbt(text: &str) -> Psbt {
    Psbt::read(&hex(text)).unwrap()
}

/// A network's part of the fixture.
fn made(network: Network) -> Value {
    wallets()[match network {
        Network::Main => "main",
        Network::Test => "test",
    }]
    .clone()
}

#[test]
fn bip143s_native_segwit_example_hashes_and_signs_alike() {
    // BIP143's native P2WPKH example: its second input spends 6 BTC of a native segwit key.
    let (tx, witnessed) = Transaction::read(&hex(
        "0x0100000002fff7f7881a8099afa6940d42d1e7f6362bec38171ea3edf433541db4e4ad969f0000000000eeffffffef51e1b804cc89d182d279655c3aa89e815b1b309fe287d9b2b55d57b90ec68a0100000000ffffffff02202cb206000000001976a9148280b37df378db99f66f85c95a783a76ac7a6d5988ac9093510d000000001976a9143bde42dbee7e4dbe6a21b2d50ce2f0167faa815988ac11000000",
    ))
    .unwrap();
    let secret: [u8; 32] = hex("0x619c335025c7f4012e556c2a58b2506e30b8511b53ade95ea316fd8c3286feb9").try_into().unwrap();
    let key = chainv4::crypto::secp256k1::public_key(&secret).unwrap();
    let preimage = tx.preimage(1, &hash160(&key), 600_000_000);

    assert!(!witnessed);
    assert_eq!(key.to_vec(), hex("0x025476c2e83188368da1ff3e292e7acafcdb3566bb0ad253f62fc70f07aeee6357"));
    assert_eq!(
        preimage,
        hex("0x0100000096b827c8483d4e9b96712b6713a7b68d6e8003a781feba36c31143470b4efd3752b0a642eea2fb7ae638c36f6252b6750293dbe574a806984b8e4d8548339a3bef51e1b804cc89d182d279655c3aa89e815b1b309fe287d9b2b55d57b90ec68a010000001976a9141d0f172a0ecb48aee1be1f2687d2963ae33f71a188ac0046c32300000000ffffffff863ef3e1a92afbfdb97f31ad0fc7683ee943e9abcf2501590ff8f6551f47e5e51100000001000000")
    );
    assert_eq!(sha256d(&preimage).to_vec(), hex("0xc37af31116d1b27caf68aae9e3ac82f1477929014d5b917657d0eb49478cb670"));
    // RFC 6979's nonce, as Bitcoin Core's then, so the very signature.
    assert_eq!(
        der(&chainv4::crypto::bitcoin::sign(&secret, &preimage)),
        hex("0x304402203609e17b84f6a7d30c80bfa610b5b4542f32a8a0d5447a12fb1366d7f01cc44a0220573a954c4518331561406f90300e8f3358f51928d43c212a8caed02de67eebee")
    );
    assert_eq!(tx.base(), Transaction::read(&tx.base()).unwrap().0.base(), "it writes back as it read");
}

#[test]
fn der_writes_each_number_in_its_fewest_bytes() {
    let mut signature = [0u8; 64];
    signature[31] = 1;
    signature[32] = 0x80;

    // r is 1, in a byte; s has its top bit set, so a 0 goes before its 32 bytes.
    assert_eq!(der(&signature), [&hex("0x3026020101022100")[..], &signature[32..]].concat());
}

#[test]
fn every_kind_of_output_is_shown_by_its_address() {
    // The key of private key 1's hash, as Bitcoin's oldest examples have it.
    assert_eq!(address(&hex("0x76a914751e76e8199196d454941c45d1b3a323f1433bd688ac"), Network::Main), "1BgGZ9tcN4rm9KBzDn7KprQz87SZ26SAMH");

    for network in Network::ALL {
        for output in made(network)["outputs"].as_array().unwrap() {
            let shown = address(&hex(output["script"].as_str().unwrap()), network);

            match output["address"].as_str() {
                Some(expected) => assert_eq!(shown, expected, "{}", output["label"]),
                None => assert_eq!(shown, "data, burned: 0x07636861696e7634"),
            }
        }
    }

    assert_eq!(address(&[0x51], Network::Main), "a script with no address: 0x51");
}

#[test]
fn the_pairing_is_the_account_bitcoins_wallets_derive() {
    let seed = seed();

    for network in Network::ALL {
        let made = made(network);
        let cbor = account(&seed, network).unwrap();
        let mut d = Decoder::new(&cbor);

        // {1: fingerprint, 2: [wpkh(hdkey)]}, the hdkey as crypto-hdkey writes it.
        assert_eq!(d.map().unwrap(), Some(2));
        assert_eq!((d.u8().unwrap(), d.u32().unwrap().to_be_bytes().to_vec()), (1, hex(wallets()["xfp"].as_str().unwrap())));
        assert_eq!((d.u8().unwrap(), d.array().unwrap()), (2, Some(1)));
        assert_eq!((d.tag().unwrap().as_u64(), d.tag().unwrap().as_u64()), (WPKH, HDKEY));

        // The key below, as the wallet's xpub, or tpub, writes it: what the wallet derived from.
        let key = seed.bitcoin_account(network.coin());
        let version: u32 = match network {
            Network::Main => 0x0488_b21e,
            Network::Test => 0x0435_87cf,
        };
        let serialized = [&version.to_be_bytes()[1..], &[key.depth], &key.parent_fingerprint, &HARDENED.to_be_bytes(), &key.chain_code, &key.public].concat();

        assert_eq!(base58check(version.to_be_bytes()[0], &serialized), made["account"].as_str().unwrap());
        assert_eq!(receiving(&seed, network, 0), made["receiving"][0].as_str().unwrap());
        assert_eq!(receiving(&seed, network, 1), made["receiving"][1].as_str().unwrap());
        assert!(pairing(&seed, network).unwrap().starts_with("UR:CRYPTO-ACCOUNT/"));
    }

    assert_ne!(account(&seed, Network::Main), account(&seed, Network::Test));
}

#[test]
fn a_wallets_psbt_reads_whole_and_writes_back_byte_for_byte() {
    for network in Network::ALL {
        let made = made(network);

        for field in ["psbt", "witness_only"] {
            let bytes = hex(made[field].as_str().unwrap());

            assert_eq!(Psbt::read(&bytes).unwrap().to_bytes(), bytes, "{field}");
        }

        assert_eq!(psbt(made["psbt"].as_str().unwrap()).transaction().base(), hex(made["transaction"].as_str().unwrap()));

        // Its animated code, from any parts enough, as the scanner takes them.
        let mut assembly = crate::hardware::Assembly::default();
        // Starting late, and missing every third.
        let parts = made["parts"].as_array().unwrap().iter().enumerate().skip(2).filter(|(index, _)| index % 3 != 0);
        let read = parts.map(|(_, part)| part).find_map(|part| assembly.add(part.as_str().unwrap()).unwrap());
        let (kind, cbor) = read.expect("enough parts");

        match Message::from_ur(&kind, &cbor).unwrap() {
            Message::Bitcoin(request) => {
                assert_eq!(request.kind, PSBT_TYPE);
                assert_eq!(request.psbt.to_bytes(), hex(made["psbt"].as_str().unwrap()));
            }
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn the_review_lays_out_what_leaves_and_spots_the_change() {
    let seed = seed();

    for network in Network::ALL {
        let made = made(network);
        let review = psbt(made["psbt"].as_str().unwrap()).review(&seed).unwrap();
        let coin = network.coin();

        assert_eq!(review.network, network);
        assert_eq!(review.inputs, vec![Spent { value: 50_000, yours: Some(format!("m/84'/{coin}'/0'/0/0")), swap: None }, Spent { value: 30_000, yours: Some(format!("m/84'/{coin}'/0'/0/1")), swap: None }]);
        assert_eq!(review.outputs.len(), 6);
        assert_eq!(review.outputs[5], Paid { value: 38_000, to: made["change"].as_str().unwrap().to_string(), yours: Some(format!("m/84'/{coin}'/0'/1/0")) });
        assert!(review.outputs[..5].iter().all(|paid| paid.yours.is_none()), "others' outputs are no change");
        assert_eq!((review.fee, review.sent(), review.unproven, review.costly()), (2_000, 40_000, false, false));
        assert_eq!(review.lock_time, 0);
        assert!(review.vsize.is_some_and(|vsize| (300..400).contains(&vsize)), "{:?}", review.vsize);

        // Without the transaction the coins came in, the amounts are the wallet's word.
        assert!(psbt(made["witness_only"].as_str().unwrap()).review(&seed).unwrap().unproven);
    }
}

#[test]
fn it_signs_as_bitcoinjs_does_and_gives_back_all_it_was_given() {
    let seed = seed();

    for network in Network::ALL {
        let made = made(network);
        let asked = psbt(made["psbt"].as_str().unwrap());
        let signed = asked.sign(&seed).unwrap();

        for (index, expected) in made["signatures"].as_array().unwrap().iter().enumerate() {
            let key = public(&seed.bitcoin_key(network.coin(), 0, index as u32).public());

            // RFC 6979's nonces, both, so the same signatures.
            assert_eq!(signed.signatures(index), vec![(key, hex(expected.as_str().unwrap()))], "input {index}");
        }

        // All it held, and the signatures; signing again replaces them.
        assert_eq!(Psbt { inputs: asked.inputs.clone(), ..signed.clone() }, asked);
        assert_eq!(signed.sign(&seed).unwrap(), signed);
        assert_eq!(Psbt::read(&signed.to_bytes()).unwrap(), signed);
        assert!(psbt(made["witness_only"].as_str().unwrap()).sign(&seed).is_ok(), "signed, with the warning shown");
    }
}

#[test]
fn a_psbt_answers_in_the_name_it_came_in() {
    let made = made(Network::Main);
    let request = Request { psbt: psbt(made["psbt"].as_str().unwrap()), kind: NEW_PSBT_TYPE };
    let message = Message::Bitcoin(Box::new(request.clone()));

    assert_eq!(message.ur_type(), "psbt");
    assert_eq!(Message::from_ur("psbt", &message.bytes()).unwrap(), message);
    assert_eq!(Request::from_cbor(PSBT_TYPE, &request.to_cbor()).unwrap().kind, PSBT_TYPE);
    assert!(Request::from_cbor(PSBT_TYPE, &[request.to_cbor(), vec![0]].concat()).is_err(), "bytes after it");
}

/// The fixture's mainnet PSBT, changed.
fn changed(change: impl FnOnce(&mut Psbt)) -> Psbt {
    let mut psbt = psbt(made(Network::Main)["psbt"].as_str().unwrap());
    change(&mut psbt);

    psbt
}

fn pair_mut<'a>(pairs: &'a mut [Pair], key: &[u8]) -> &'a mut Pair {
    pairs.iter_mut().find(|pair| pair.key == key).unwrap()
}

fn refused(psbt: &Psbt, why: &str) {
    let seed = seed();
    let error = psbt.review(&seed).unwrap_err();

    assert!(error.contains(why), "{error}");
    assert_eq!(psbt.sign(&seed).unwrap_err(), error, "nothing signed it would not show");
}

#[test]
fn what_it_cannot_sign_safely_is_refused() {
    // Another seed's wallet.
    let other = Phrase::parse("test test test test test test test test test test test junk").unwrap().seed("");
    assert!(psbt(made(Network::Main)["psbt"].as_str().unwrap()).review(&other).unwrap_err().starts_with("nothing here for this wallet to sign"));

    // A previous transaction that is not the one spent from.
    refused(&changed(|psbt| *pair_mut(&mut psbt.inputs[0], &[IN_NON_WITNESS_UTXO]).value.last_mut().unwrap() ^= 1), "not the one it spends from");

    // An amount told two ways.
    refused(&changed(|psbt| pair_mut(&mut psbt.inputs[1], &[IN_WITNESS_UTXO]).value[0] ^= 1), "told two ways that disagree");

    // Less than all of it signed.
    refused(&changed(|psbt| psbt.inputs[0].push(Pair { key: vec![IN_SIGHASH_TYPE], value: 2u32.to_le_bytes().to_vec() })), "SIGHASH_ALL");

    // No amount told at all.
    refused(&changed(|psbt| psbt.inputs[0].retain(|pair| pair.key != [IN_WITNESS_UTXO] && pair.key != [IN_NON_WITNESS_UTXO])), "the fee cannot be known");

    // A key of this seed's at a path it does not sign at, and the wrong key at its path.
    let at = |path: [u32; 5]| move |psbt: &mut Psbt| {
        let derivation = psbt.inputs[0].iter_mut().find(|pair| pair.key[0] == IN_BIP32_DERIVATION).unwrap();
        derivation.value.truncate(4);
        derivation.value.extend(path.iter().flat_map(|step| step.to_le_bytes()));
    };
    refused(&changed(at([44 | HARDENED, HARDENED, HARDENED, 0, 0])), "where this signer holds none");
    refused(&changed(at([84 | HARDENED, HARDENED, HARDENED, 0, 5])), "not this wallet's");

    // Change on the other network.
    refused(
        &changed(|psbt| {
            let derivation = psbt.outputs[5].iter_mut().find(|pair| pair.key[0] == OUT_BIP32_DERIVATION).unwrap();
            derivation.value[8..12].copy_from_slice(&(1 | HARDENED).to_le_bytes());
        }),
        "Bitcoin and its testnet both",
    );

    // Paying out more than it spends.
    refused(
        &changed(|psbt| {
            psbt.tx.outputs[0].value += 2_001;
            psbt.global[0].value = psbt.tx.base();
        }),
        "more than it spends",
    );
}

#[test]
fn psbts_that_do_not_read_are_refused() {
    let bytes = hex(made(Network::Main)["psbt"].as_str().unwrap());
    let refused = |bytes: &[u8], why: &str| {
        let error = Psbt::read(bytes).unwrap_err();
        assert!(error.contains(why), "{error}");
    };

    refused(&bytes[1..], "not a PSBT");
    refused(&bytes[..bytes.len() - 1], "cut short");
    refused(&[bytes.clone(), vec![0]].concat(), "more after it");
    refused(&hex(made(Network::Main)["transaction"].as_str().unwrap()), "not a PSBT");

    // A key twice, and version 2.
    let with = |pair: &[u8]| [&bytes[..5], pair, &bytes[5..]].concat();
    let unsigned = Psbt::read(&bytes).unwrap().global[0].clone();
    let mut twice = vec![1, 0];
    compact(&mut twice, unsigned.value.len());
    twice.extend(&unsigned.value);
    refused(&with(&twice), "a key twice");
    refused(&with(&[1, GLOBAL_VERSION, 4, 2, 0, 0, 0]), "version 2");

    // A transaction signed already.
    let mut signed = Psbt::read(&bytes).unwrap();
    signed.tx.inputs[0].script_sig = vec![0];
    signed.global[0].value = signed.tx.base();
    refused(&signed.to_bytes(), "signed already");

    // A size not in its fewest bytes.
    let mut reader = Reader::new(&[0xfd, 0x10, 0x00]);
    assert!(reader.compact().is_err());
}

/// Writes the wallet's codes for wallets/check.js to read back with bitcoinjs-lib and Keystone's
/// registry, when CHAINV4_BITCOIN_OUT names where.
#[test]
fn bitcoin_wallets_own() {
    let Ok(out) = std::env::var("CHAINV4_BITCOIN_OUT") else {
        return;
    };
    let seed = seed();
    let mut written = serde_json::Map::new();

    for network in Network::ALL {
        let made = made(network);
        let mut signed = Vec::new();

        for field in ["psbt", "witness_only"] {
            let message = Message::Bitcoin(Box::new(Request { psbt: psbt(made[field].as_str().unwrap()).sign(&seed).unwrap(), kind: PSBT_TYPE }));
            let mut encoder = ur::Encoder::new(&message.bytes(), crate::airgap::FRAGMENT, PSBT_TYPE).unwrap();
            let parts = (0..encoder.fragment_count() * 2).map(|_| encoder.next_part().unwrap()).collect::<Vec<_>>();
            signed.push(serde_json::json!({ "name": field, "parts": parts, "whole": message.to_text() }));
        }

        written.insert(made["name"].as_str().unwrap().to_string(), serde_json::json!({ "pairing": pairing(&seed, network).unwrap(), "signed": signed }));
    }

    std::fs::write(out, serde_json::to_string_pretty(&written).unwrap()).unwrap();
}

#[cfg(feature = "swap")]
#[test]
fn the_signer_signs_a_swaps_htlc_spend_as_the_swap_tools_do() {
    use chainv4::swap::htlc::{Coins, Htlc, Spend};

    let seed = seed();
    // This wallet's first testnet key claims; someone else's refunds.
    let keypair = seed.bitcoin_key(1, 0, 0);
    let PrivateKey::Bitcoin(secret_key) = keypair.private() else { unreachable!() };
    let ours = public(&keypair.public());
    let theirs = chainv4::crypto::secp256k1::public_key(&[0x22; 32]).unwrap();
    let secret = [0x5e; 32];
    let htlc = Htlc { hash: chainv4::crypto::bitcoin::sha256(&secret), claim: chainv4::swap::BitcoinParty::Key(ours), refund: chainv4::swap::BitcoinParty::Key(theirs), locktime: 1_793_000_000 };
    let to = p2wpkh(&hash160(&ours));
    let unsigned = htlc.unsigned(Coins { txid: [7; 32], vout: 0, value: 80_000 }, to.clone(), 2, Spend::Claim { secret }).unwrap();
    let path = [84 | HARDENED, 1 | HARDENED, HARDENED, 0, 0];
    let asked = Psbt::for_htlc(&unsigned, 80_000, &htlc, ours, seed.bitcoin_account(0).master_fingerprint, &path);

    // The signer shows it as a claim of a swap's coins, and signs it.
    let review = Psbt::read(&asked.to_bytes()).unwrap().review(&seed).unwrap();
    assert_eq!(review.network, Network::Test);
    assert_eq!(review.inputs[0].swap, Some(Swapped::Claim));
    let signed = asked.sign(&seed).unwrap();

    // Finished with the secret, it is the very spend the swap tools sign with the key itself.
    let finished = signed.finish_htlc(&htlc, 80_000, Spend::Claim { secret }).unwrap();
    let direct = htlc.spend(Coins { txid: [7; 32], vout: 0, value: 80_000 }, to, 2, Spend::Claim { secret }, secret_key).unwrap();
    assert_eq!(finished.encode(), direct.encode());

    // A signature for other coins does not finish it, and a key the HTLC does not name is refused.
    assert!(signed.finish_htlc(&htlc, 80_001, Spend::Claim { secret }).unwrap_err().contains("does not hold"));
    assert!(asked.finish_htlc(&htlc, 80_000, Spend::Claim { secret }).unwrap_err().contains("no signature"));
    let stranger = Htlc { claim: chainv4::swap::BitcoinParty::Key(theirs), refund: chainv4::swap::BitcoinParty::Key(chainv4::crypto::secp256k1::public_key(&[0x33; 32]).unwrap()), ..htlc.clone() };
    let wrong = Psbt::for_htlc(&stranger.unsigned(Coins { txid: [7; 32], vout: 0, value: 80_000 }, p2wpkh(&hash160(&ours)), 2, Spend::Refund).unwrap(), 80_000, &stranger, ours, seed.bitcoin_account(0).master_fingerprint, &path);
    assert!(wrong.review(&seed).unwrap_err().contains("neither of its swap's keys"));
}

#[test]
fn the_signer_signs_its_part_of_a_multisig_spend_and_sees_its_change_come_back() {
    use chainv4::crypto::bitcoin::psbt::{MultisigCoin, MultisigWallet};
    use chainv4::crypto::bitcoin::tx::{TxIn, TxOut, p2wsh};

    let ours = seed();
    let other = Phrase::parse("legal winner thank year wave sausage worth useful legal winner thank yellow").unwrap().seed("");
    let third = Phrase::parse("letter advice cage absurd amount doctor acoustic avoid letter advice cage above").unwrap().seed("");
    let wallet = MultisigWallet { threshold: 2, xpubs: [&ours, &other, &third].iter().map(|seed| seed.multisig_account(1, 0)).collect(), network: Network::Test };
    let change = p2wsh(&chainv4::crypto::bitcoin::multisig::witness_script(2, &wallet.xpubs, 1, 0).unwrap());
    let tx = Transaction {
        version: 2,
        inputs: vec![TxIn { txid: [7; 32], vout: 0, script_sig: Vec::new(), sequence: 0xffff_fffd, witness: Vec::new() }],
        outputs: vec![TxOut { value: 60_000, script: p2wpkh(&[0x11; 20]) }, TxOut { value: 38_000, script: change }],
        lock_time: 0,
    };
    let psbt = Psbt::for_multisig(&tx, &wallet, &[MultisigCoin { value: 100_000, chain: 0, index: 0 }], &[(1, 0)]).unwrap();
    let review = psbt.review(&ours).unwrap();

    assert_eq!((review.network, review.fee, review.vsize), (Network::Test, 2_000, None));
    assert_eq!(review.inputs[0].yours.as_deref(), Some("m/48'/1'/0'/2'/0/0"));
    assert!(review.outputs[1].yours.as_deref().is_some_and(|path| path.contains("multisig")), "the change comes back");
    assert_eq!(review.sent(), 60_000);

    // Signed here and by another member, it finishes.
    let mut signed = psbt.sign(&ours).unwrap();
    let theirs = psbt.sign(&other).unwrap();

    assert_eq!(signed.multisig_progress(), vec![(1, 2)]);
    signed.combine(&theirs).unwrap();
    assert_eq!(signed.finish_multisig().unwrap().inputs[0].witness.len(), 4);

    // A script that does not hold this key, or is not the coins', is refused.
    let mut forged = psbt.clone();
    let script = chainv4::crypto::bitcoin::multisig::multisig_script(1, &[[2; 33]]);
    forged.inputs[0].retain(|pair| pair.key != [0x05]);
    forged.inputs[0].push(chainv4::crypto::bitcoin::psbt::Pair { key: vec![0x05], value: script });

    assert!(forged.review(&ours).unwrap_err().contains("does not hold"));
}

/// The words of a second seed in the wallet.
fn second() -> Seed {
    Phrase::parse("test test test test test test test test test test test junk").unwrap().seed("")
}

/// The fixture's mainnet PSBT with its second input moved to the second seed's receiving key 3: its
/// coins that key's, and its path named under that seed's fingerprint, as a wallet paired with it
/// would have written it.
fn across_two_seeds() -> Psbt {
    changed(|psbt| {
        let seed = second();
        let key = public(&seed.bitcoin_key(0, 0, 3).public());
        let input = &mut psbt.inputs[1];
        let amount = pair_mut(input, &[IN_WITNESS_UTXO]).value[..8].to_vec();
        let script = p2wpkh(&hash160(&key));
        let witness = [amount, vec![script.len() as u8], script].concat();
        let path = [84 | HARDENED, HARDENED, HARDENED, 0, 3];
        let derivation = [seed.bitcoin_account(0).master_fingerprint.to_vec(), path.iter().flat_map(|step: &u32| step.to_le_bytes()).collect()].concat();

        input.retain(|pair| pair.key[0] != IN_BIP32_DERIVATION && pair.key != [IN_WITNESS_UTXO] && pair.key != [IN_NON_WITNESS_UTXO]);
        input.push(Pair { key: vec![IN_WITNESS_UTXO], value: witness });
        input.push(Pair { key: [&[IN_BIP32_DERIVATION][..], &key].concat(), value: derivation });
    })
}

#[test]
fn a_wallet_of_two_seeds_reviews_and_signs_what_each_owns() {
    let (first, other) = (seed(), second());
    let asked = across_two_seeds();
    let inputs = asked.inputs.len();

    assert!(inputs >= 2, "the fixture's PSBT spends at least two");

    // Each alone calls the other's input another's, and cannot weigh the whole.
    let alone = asked.review(&first).unwrap();

    assert!(alone.inputs[0].yours.is_some() && alone.inputs[1].yours.is_none());
    assert_eq!(alone.vsize, None);
    assert!(asked.review(&other).unwrap().inputs[1].yours.is_some());

    // Together they own every input it names of theirs, whichever is asked first, and the same fee.
    let both = asked.review_all(&[&first, &other]).unwrap();

    assert!(both.inputs.iter().all(|spent| spent.yours.is_some()), "{both:?}");
    assert_eq!(both, asked.review_all(&[&other, &first]).unwrap());
    assert_eq!(both.fee, alone.fee);
    assert!(both.vsize.is_some(), "this signer signs every input now");

    // Each signs its own input with the signature it makes alone, and nothing else changes.
    let signed = asked.sign_all(&[&first, &other]).unwrap();
    let (by_first, by_other) = (asked.sign(&first).unwrap(), asked.sign(&other).unwrap());

    assert_eq!(signed.signatures(0), by_first.signatures(0));
    assert_eq!(signed.signatures(1), by_other.signatures(1));
    assert_eq!(signed.signatures(1)[0].0, public(&other.bitcoin_key(0, 0, 3).public()));
    assert!(by_first.signatures(1).is_empty() && by_other.signatures(0).is_empty());
}

#[test]
fn a_seed_with_nothing_here_is_passed_over_and_nothing_for_any_is_refused() {
    let (first, other) = (seed(), second());
    let spare = Phrase::parse("legal winner thank year wave sausage worth useful legal winner thank yellow").unwrap().seed("");
    let asked = psbt(made(Network::Main)["psbt"].as_str().unwrap());

    assert!(other_is_nothing(&asked, &other));
    assert_eq!(asked.review_all(&[&other, &first]).unwrap(), asked.review(&first).unwrap(), "the same review as without it");
    assert_eq!(asked.sign_all(&[&other, &first]).unwrap(), asked.sign(&first).unwrap(), "the same signatures, and none of its own");
    assert!(asked.review_all(&[&other, &spare]).unwrap_err().starts_with("nothing here for this wallet to sign"));
    assert!(asked.sign_all(&[&other, &spare]).unwrap_err().starts_with("nothing here for this wallet to sign"));
    assert!(asked.review_all(&[]).unwrap_err().starts_with("nothing here for this wallet to sign"));

    // What one seed refuses is still refused with another beside it.
    let wrong = changed(|psbt| {
        let derivation = psbt.inputs[0].iter_mut().find(|pair| pair.key[0] == IN_BIP32_DERIVATION).unwrap();
        derivation.value.truncate(4);
        derivation.value.extend([84 | HARDENED, HARDENED, HARDENED, 0, 5u32].iter().flat_map(|step| step.to_le_bytes()));
    });

    assert!(wrong.review_all(&[&other, &first]).unwrap_err().contains("not this wallet's"));
    assert!(wrong.sign_all(&[&other, &first]).unwrap_err().contains("not this wallet's"));
}

fn other_is_nothing(asked: &Psbt, seed: &Seed) -> bool {
    asked.review(seed).is_err_and(|why| why.starts_with("nothing here for this wallet to sign"))
}

#[test]
fn change_back_to_another_of_the_wallets_seeds_is_shown_as_change() {
    let (first, other) = (seed(), second());
    let key = public(&other.bitcoin_key(0, 1, 7).public());
    let asked = changed(|psbt| {
        let at = psbt.outputs.iter().position(|pairs| pairs.iter().any(|pair| pair.key[0] == OUT_BIP32_DERIVATION)).unwrap();
        let path = [84 | HARDENED, HARDENED, HARDENED, 1, 7];
        let derivation = [other.bitcoin_account(0).master_fingerprint.to_vec(), path.iter().flat_map(|step: &u32| step.to_le_bytes()).collect()].concat();

        psbt.outputs[at].retain(|pair| pair.key[0] != OUT_BIP32_DERIVATION);
        psbt.outputs[at].push(Pair { key: [&[OUT_BIP32_DERIVATION][..], &key].concat(), value: derivation });
        psbt.tx.outputs[at].script = p2wpkh(&hash160(&key));
        psbt.global[0].value = psbt.tx.base();
    });
    let at = asked.outputs.iter().position(|pairs| pairs.iter().any(|pair| pair.key[0] == OUT_BIP32_DERIVATION)).unwrap();

    assert_eq!(asked.review(&first).unwrap().outputs[at].yours, None, "to the first seed alone, it leaves");
    assert_eq!(asked.review_all(&[&first, &other]).unwrap().outputs[at].yours.as_deref(), Some("m/84'/0'/0'/1/7"), "to the wallet, it comes back");

    // A key at that path that is not the seed's is not change, whatever it claims.
    let forged = changed(|psbt| {
        let path = [84 | HARDENED, HARDENED, HARDENED, 1, 8];
        let derivation = [other.bitcoin_account(0).master_fingerprint.to_vec(), path.iter().flat_map(|step: &u32| step.to_le_bytes()).collect()].concat();
        let at = psbt.outputs.iter().position(|pairs| pairs.iter().any(|pair| pair.key[0] == OUT_BIP32_DERIVATION)).unwrap();

        psbt.outputs[at].retain(|pair| pair.key[0] != OUT_BIP32_DERIVATION);
        psbt.outputs[at].push(Pair { key: [&[OUT_BIP32_DERIVATION][..], &key].concat(), value: derivation });
        psbt.tx.outputs[at].script = p2wpkh(&hash160(&key));
        psbt.global[0].value = psbt.tx.base();
    });

    assert_eq!(forged.review_all(&[&first, &other]).unwrap().outputs[at].yours, None);
}
