use {
    super::*,
    crate::crypto::bitcoin::multisig::descriptor,
    crate::crypto::bitcoin::tx::address,
    crate::crypto::keypair::PrivateKey,
    crate::crypto::mnemonic::{Phrase, Seed},
};

const HARDENED: u32 = 1 << 31;

fn seeds() -> Vec<Seed> {
    [
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
        "legal winner thank year wave sausage worth useful legal winner thank yellow",
        "letter advice cage absurd amount doctor acoustic avoid letter advice cage above",
    ]
    .iter()
    .map(|words| Phrase::parse(words).unwrap().seed(""))
    .collect()
}

fn wallet(seeds: &[Seed], network: Network) -> MultisigWallet {
    MultisigWallet { threshold: 2, xpubs: seeds.iter().map(|seed| seed.multisig_account(network.coin(), 0)).collect(), network }
}

/// What a member's words sign with, at a path below its multisig account.
fn secret(seed: &Seed) -> impl Fn(&[u32]) -> Option<[u8; 32]> + '_ {
    move |path| match *path {
        [_, coin, _, _, chain, index] => match seed.multisig_key(coin - HARDENED, 0, chain, index).private() {
            PrivateKey::Bitcoin(secret) => Some(*secret),
            _ => None,
        },
        _ => None,
    }
}

/// A spend of one coin of the multisig's, at receiving address 0, to someone, its change back.
fn spend(wallet: &MultisigWallet, txid: [u8; 32], vout: u32, value: u64, to: Vec<u8>) -> Psbt {
    let change = p2wsh(&witness_script(wallet.threshold, &wallet.xpubs, 1, 0).unwrap());
    let tx = Transaction {
        version: 2,
        inputs: vec![TxIn { txid, vout, script_sig: Vec::new(), sequence: 0xffff_fffd, witness: Vec::new() }],
        outputs: vec![TxOut { value: 60_000, script: to }, TxOut { value: value - 60_000 - 2_000, script: change }],
        lock_time: 0,
    };

    Psbt::for_multisig(&tx, wallet, &[MultisigCoin { value, chain: 0, index: 0 }], &[(1, 0)]).unwrap()
}

#[test]
fn two_of_three_members_sign_apart_and_the_combined_psbt_finishes() {
    let seeds = seeds();
    let wallet = wallet(&seeds, Network::Test);
    let psbt = spend(&wallet, [7; 32], 0, 100_000, vec![0x00, 0x14, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11]);

    // It reads back as it was written, every pair kept.
    assert_eq!(Psbt::read(&psbt.to_bytes()).unwrap(), psbt);
    assert!(psbt.finish_multisig().unwrap_err().contains("0 of the 2"));

    let mut alice = psbt.clone();
    let mut carol = psbt.clone();

    assert_eq!(alice.sign_multisig(seeds[0].multisig_account(1, 0).fingerprint, secret(&seeds[0])).unwrap(), 1);
    assert_eq!(carol.sign_multisig(seeds[2].multisig_account(1, 0).fingerprint, secret(&seeds[2])).unwrap(), 1);
    assert_eq!(alice.sign_multisig([0xde, 0xad, 0xbe, 0xef], secret(&seeds[1])).unwrap(), 0, "no key at another seed's fingerprint");
    assert_eq!(alice.multisig_progress(), vec![(1, 2)]);
    assert!(alice.finish_multisig().unwrap_err().contains("1 of the 2"));

    alice.combine(&carol).unwrap();
    assert_eq!(alice.multisig_progress(), vec![(2, 2)]);
    let finished = alice.finish_multisig().unwrap();
    let witness = &finished.inputs[0].witness;

    assert_eq!((witness.len(), witness[0].is_empty()), (4, true), "the extra push, two signatures, the script");
    assert_eq!(witness[3], witness_script(2, &wallet.xpubs, 0, 0).unwrap());

    // A PSBT of another transaction is not combined.
    let other = spend(&wallet, [8; 32], 0, 100_000, vec![0x51]);

    assert!(alice.combine(&other).is_err());
}

/// Bitcoin Core on a private regtest chain, in a directory of its own, stopped when dropped.
struct Regtest {
    dir: std::path::PathBuf,
}

impl Regtest {
    fn start() -> Option<Regtest> {
        let dir = std::env::temp_dir().join(format!("chainv4-multisig-regtest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).ok()?;
        let started = std::process::Command::new("bitcoind")
            .args(["-regtest", "-daemonwait", "-fallbackfee=0.0001", "-txindex=1", "-rpcport=18953", "-port=18954"])
            .arg(format!("-datadir={}", dir.display()))
            .status()
            .ok()?;

        started.success().then_some(Regtest { dir })
    }

    fn cli(&self, args: &[&str]) -> Result<String, String> {
        let out = std::process::Command::new("bitcoin-cli")
            .args(["-regtest", "-rpcport=18953", "-rpcwait"])
            .arg(format!("-datadir={}", self.dir.display()))
            .args(args)
            .output()
            .map_err(|err| err.to_string())?;

        match out.status.success() {
            true => Ok(String::from_utf8_lossy(&out.stdout).trim().to_string()),
            false => Err(String::from_utf8_lossy(&out.stderr).trim().to_string()),
        }
    }
}

impl Drop for Regtest {
    fn drop(&mut self) {
        let _ = self.cli(&["stop"]);
        std::thread::sleep(std::time::Duration::from_secs(2));
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
#[ignore = "runs Bitcoin Core on a private regtest chain"]
fn bitcoin_core_derives_the_descriptor_s_addresses_and_takes_a_two_of_three_spend() {
    let Some(node) = Regtest::start() else {
        panic!("bitcoind did not start: is Bitcoin Core installed?");
    };
    let seeds = seeds();
    let wallet = wallet(&seeds, Network::Test);
    let script = witness_script(2, &wallet.xpubs, 0, 0).unwrap();
    let ours = crate::structs::bech32::encode_segwit("bcrt", &crate::crypto::bitcoin::sha256(&script));

    // Bitcoin Core reads the descriptor, checksum and all, and derives the same address.
    let receiving = descriptor(2, &wallet.xpubs, Network::Test, 0);
    let derived: serde_json::Value = serde_json::from_str(&node.cli(&["deriveaddresses", &receiving, "[0,0]"]).unwrap()).unwrap();

    assert_eq!(derived[0].as_str(), Some(ours.as_str()));
    assert!(address(&p2wsh(&script), Network::Test).starts_with("tb1q"), "the same program, written for testnet");

    node.cli(&["createwallet", "test"]).unwrap();
    let mined = node.cli(&["getnewaddress"]).unwrap();
    node.cli(&["generatetoaddress", "101", &mined]).unwrap();
    let txid = node.cli(&["sendtoaddress", &ours, "0.001"]).unwrap();
    node.cli(&["generatetoaddress", "1", &mined]).unwrap();
    let funding: serde_json::Value = serde_json::from_str(&node.cli(&["getrawtransaction", &txid, "true"]).unwrap()).unwrap();
    let vout = funding["vout"].as_array().unwrap().iter().find(|out| out["value"].as_f64() == Some(0.001)).unwrap()["n"].as_u64().unwrap() as u32;
    let mut id: [u8; 32] = crate::structs::hex::from_hex(&format!("0x{txid}")).unwrap().try_into().unwrap();
    id.reverse();

    let mut psbt = spend(&wallet, id, vout, 100_000, p2wsh(&[0x51]));
    let mut bob = psbt.clone();
    psbt.sign_multisig(seeds[0].multisig_account(1, 0).fingerprint, secret(&seeds[0])).unwrap();
    bob.sign_multisig(seeds[1].multisig_account(1, 0).fingerprint, secret(&seeds[1])).unwrap();

    // Bitcoin Core reads our PSBT and finds what it is: a P2WSH multisig input with its members.
    let decoded: serde_json::Value = serde_json::from_str(&node.cli(&["decodepsbt", &psbt.to_base64()]).unwrap()).unwrap();

    assert_eq!(decoded["inputs"][0]["witness_script"]["type"], "multisig");
    assert_eq!(decoded["inputs"][0]["bip32_derivs"].as_array().map(Vec::len), Some(3));

    psbt.combine(&bob).unwrap();
    let finished = psbt.finish_multisig().unwrap();
    let sent = node.cli(&["sendrawtransaction", &crate::structs::hex::to_hex(&finished.encode())[2..]]);

    assert!(sent.is_ok(), "{sent:?}");
}

#[test]
fn a_spend_takes_the_largest_coins_first_and_keeps_its_change_unless_it_is_dust() {
    let seeds = seeds();
    let wallet = wallet(&seeds, Network::Test);
    let utxo = |txid: u8, value: u64, index: u32| Utxo { txid: [txid; 32], vout: 0, coin: MultisigCoin { value, chain: 0, index } };
    let coins = [utxo(1, 30_000, 0), utxo(2, 80_000, 1), utxo(3, 50_000, 2)];
    let to = vec![0x00, 0x14, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11];
    let psbt = wallet.spend(&coins, to.clone(), 100_000, 2, 4).unwrap();

    assert_eq!(psbt.tx.inputs.iter().map(|input| input.txid[0]).collect::<Vec<_>>(), [2, 3], "the largest first");
    assert_eq!(psbt.tx.outputs[0].value, 100_000);
    let fee = 130_000 - psbt.tx.outputs.iter().map(|output| output.value).sum::<u64>();

    assert!((2 * 200..2 * 400).contains(&fee), "about 2 sat/vB: {fee}");
    assert_eq!(psbt.outputs[1].iter().filter(|pair| pair.key.first() == Some(&OUT_BIP32_DERIVATION)).count(), 3, "the change said to be the multisig's");

    // No change worth making: it goes to the fee. Too little: refused.
    assert_eq!(wallet.spend(&coins, to.clone(), 159_500, 1, 4).unwrap().tx.outputs.len(), 1);
    assert!(wallet.spend(&coins, to.clone(), 161_000, 1, 4).unwrap_err().contains("short"));

    // Base64, as other wallets pass it, reads back.
    assert_eq!(Psbt::from_base64(&psbt.to_base64()).unwrap(), psbt);
}
