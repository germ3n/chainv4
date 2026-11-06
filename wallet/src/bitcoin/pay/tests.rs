use {super::*, chainv4::crypto::bitcoin::tx::p2wpkh};

fn coin(n: u8, value: u64) -> Spendable {
    Spendable { txid: format!("{n:02x}").repeat(32), vout: u32::from(n), value, key: Keypair::from_private(PrivateKey::Bitcoin([7; 32])) }
}

fn script_of(key: &Keypair) -> Vec<u8> {
    let PublicKey::Bitcoin(public) = key.public() else { unreachable!("a bitcoin key") };

    p2wpkh(&hash160(&public))
}

#[test]
fn a_payment_takes_the_largest_coins_first_and_keeps_its_change() {
    let (to, change) = (vec![0x00, 0x14, 7, 7], vec![0x00, 0x14, 8, 8]);
    let built = pay(vec![coin(1, 10_000), coin(2, 500_000), coin(3, 40_000)], &to, Amount::Exactly(100_000), 2, &change).unwrap();
    let (tx, witnessed) = Transaction::read(&built.raw).unwrap();

    assert!(witnessed);
    assert_eq!(built.spends, [("02".repeat(32), 2)], "the largest alone covers it");
    assert_eq!((tx.outputs[0].value, tx.outputs[0].script.clone()), (100_000, to.clone()));
    assert_eq!((tx.outputs[1].value, tx.outputs[1].script.clone()), (built.change.unwrap(), change.clone()));
    assert_eq!(built.sent + built.fee + built.change.unwrap(), 500_000);
    assert_eq!(built.fee, 2 * (11 + 68 + 13 + 13), "at the rate, for one input and two outputs");
    assert!(built.fee >= 2 * built.vsize, "never under the rate: {} for {} vbytes", built.fee, built.vsize);
    assert!(tx.inputs.iter().all(|input| input.sequence == 0xffff_fffd), "replaceable");

    // Two coins where one is short; the txid is the one explorers will show.
    let built = pay(vec![coin(1, 60_000), coin(3, 50_000)], &to, Amount::Exactly(100_000), 1, &change).unwrap();
    assert_eq!(built.spends.len(), 2);
    let mut id = tx_id(&built.raw);
    id.reverse();
    assert_eq!(built.txid, chainv4::structs::hex::to_hex(&id)[2..]);
}

fn tx_id(raw: &[u8]) -> [u8; 32] {
    Transaction::read(raw).unwrap().0.txid()
}

#[test]
fn dust_change_goes_to_the_fee_and_all_sends_everything_less_it() {
    let (to, change) = (vec![0x00, 0x14, 7, 7], vec![0x00, 0x14, 8, 8]);

    // 100 left after the fee with change is dust: no change output, the fee keeps it.
    let fee_one_out = 11 + 68 + 13;
    let built = pay(vec![coin(1, 100_000 + fee_one_out + 100)], &to, Amount::Exactly(100_000), 1, &change).unwrap();
    assert_eq!((built.change, built.fee), (None, fee_one_out + 100));
    assert_eq!(Transaction::read(&built.raw).unwrap().0.outputs.len(), 1);

    let built = pay(vec![coin(1, 30_000), coin(2, 20_000)], &to, Amount::All, 3, &change).unwrap();
    assert_eq!((built.sent + built.fee, built.change, built.spends.len()), (50_000, None, 2));

    assert!(pay(vec![coin(1, 1_000)], &to, Amount::Exactly(5_000), 1, &change).unwrap_err().contains("short"));
    assert!(pay(vec![coin(1, 100_000)], &to, Amount::Exactly(100), 1, &change).unwrap_err().contains("dust"));
    assert!(pay(vec![coin(1, 300)], &to, Amount::All, 1, &change).is_err(), "nothing over the fee");
    assert!(pay(vec![], &to, Amount::All, 1, &change).unwrap_err().contains("no confirmed coins"));
    assert!(pay(vec![coin(1, 100_000)], &to, Amount::All, 0, &change).is_err());
}

#[test]
fn each_input_is_signed_by_its_own_key_over_bip143() {
    let key = Keypair::from_private(PrivateKey::Bitcoin([7; 32]));
    let built = pay(vec![coin(1, 80_000), coin(2, 70_000)], &script_of(&key), Amount::Exactly(120_000), 1, &script_of(&key)).unwrap();
    let (tx, _) = Transaction::read(&built.raw).unwrap();
    let PublicKey::Bitcoin(public) = key.public() else { unreachable!("a bitcoin key") };

    for (index, (input, value)) in tx.inputs.iter().zip([80_000, 70_000]).enumerate() {
        let (signature, sighash) = input.witness[0].split_at(input.witness[0].len() - 1);
        assert_eq!(sighash, [0x01], "SIGHASH_ALL");
        assert_eq!(input.witness[1], public.to_vec());
        let signature = chainv4::crypto::bitcoin::tx::from_der(signature).unwrap();
        assert!(chainv4::crypto::bitcoin::verify(&public, &tx.preimage(index, &hash160(&public), value), &signature), "input {index}");
    }
}
