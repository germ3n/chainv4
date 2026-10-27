use {
    super::*,
    bitcoin::absolute::LockTime,
    bitcoin::hashes::Hash,
    bitcoin::transaction::Version,
    bitcoin::{Amount, OutPoint, Sequence, TxIn, TxOut, Txid, Witness},
};

fn tx(inputs: Vec<(Txid, u32, Vec<Vec<u8>>)>, outputs: Vec<(Vec<u8>, u64)>) -> Transaction {
    Transaction {
        version: Version::TWO,
        lock_time: LockTime::ZERO,
        input: inputs
            .into_iter()
            .map(|(txid, vout, witness)| TxIn { previous_output: OutPoint { txid, vout }, script_sig: ScriptBuf::new(), sequence: Sequence::MAX, witness: Witness::from_slice(&witness) })
            .collect(),
        output: outputs.into_iter().map(|(script, sats)| TxOut { value: Amount::from_sat(sats), script_pubkey: ScriptBuf::from_bytes(script) }).collect(),
    }
}

fn block(txdata: Vec<Transaction>) -> Block {
    let header = bitcoin::block::Header {
        version: bitcoin::block::Version::TWO,
        prev_blockhash: BlockHash::all_zeros(),
        merkle_root: bitcoin::TxMerkleNode::all_zeros(),
        time: 0,
        bits: bitcoin::CompactTarget::from_consensus(0),
        nonce: 0,
    };

    Block { header, txdata }
}

#[test]
fn blocks_add_the_coins_paying_watched_scripts_and_mark_what_spends_them() {
    let (ours, htlc, other) = (vec![0x00, 0x14, 1, 2, 3], vec![0x00, 0x20, 9, 9], vec![0x51]);
    let watched: BTreeSet<Vec<u8>> = [ours.clone(), htlc.clone()].into();
    let mut seen = Seen::default();

    // Pays us and the HTLC, and someone else.
    let funding = tx(vec![(Txid::all_zeros(), 0, vec![])], vec![(ours.clone(), 5_000), (other, 7), (htlc.clone(), 80_000)]);
    assert!(scan(&mut seen, &block(vec![funding.clone()]), 100, &watched));
    assert_eq!(seen.coins.len(), 2);
    assert_eq!((seen.coins[1].vout, seen.coins[1].value, seen.coins[1].height), (2, 80_000, 100));
    assert_eq!(seen.coins[0].txid, funding.compute_txid().to_string(), "as explorers write it");
    assert!(!scan(&mut seen, &block(vec![funding.clone()]), 100, &watched) && seen.coins.len() == 2, "a block seen twice adds nothing");

    // The HTLC claimed, its secret in the witness.
    let secret = [0x5e; 32];
    let claim = tx(vec![(funding.compute_txid(), 2, vec![vec![0x30; 71], secret.to_vec(), vec![1], vec![0x63]])], vec![(vec![0x00, 0x14, 7], 79_000)]);
    assert!(scan(&mut seen, &block(vec![claim.clone()]), 103, &watched));
    let spent = seen.coins[1].spent.clone().unwrap();
    assert_eq!((spent.txid, spent.height), (claim.compute_txid().to_string(), 103));
    assert_eq!(secret_in(&seen, &htlc, &sha256(&secret)), Some(secret));
    assert_eq!(secret_in(&seen, &htlc, &[0; 32]), None);
    assert_eq!(secret_in(&seen, &ours, &sha256(&secret)), None, "only the script's own spends");

    // A reorganization from 103 takes the claim back; from 100, the coins too.
    undo(&mut seen, 103);
    assert_eq!((seen.coins.len(), seen.coins[1].spent.clone()), (2, None));
    undo(&mut seen, 100);
    assert!(seen.coins.is_empty());
}

#[test]
fn a_coin_keeps_what_proves_it_to_a_contract_following_bitcoin() {
    let deposit = vec![0x00, 0x20, 4, 4];
    let watched: BTreeSet<Vec<u8>> = [deposit.clone()].into();
    let mut seen = Seen::default();
    let others: Vec<Transaction> = (0..4u32).map(|n| tx(vec![(Txid::all_zeros(), n, vec![vec![n as u8]])], vec![(vec![0x51], 1)])).collect();
    // Paid at the third place of five, with a witness the proof leaves out.
    let paying = tx(vec![(Txid::all_zeros(), 9, vec![vec![0x30; 71], vec![2; 33]])], vec![(vec![0x51], 3), (deposit.clone(), 50_000)]);
    let txdata = vec![others[0].clone(), others[1].clone(), paying.clone(), others[2].clone(), others[3].clone()];
    let mut mined = block(txdata.clone());
    mined.header.merkle_root = mined.compute_merkle_root().unwrap();
    assert!(scan(&mut seen, &mined, 200, &watched));

    let proof = seen.coins[0].proof.clone().unwrap();
    let bytes = |text: &str| chainv4::structs::hex::from_hex(&format!("0x{text}")).unwrap();
    let raw = bytes(&proof.raw);

    // The raw transaction is the base one, hashing to the coin's id; the path leads to the
    // block's Merkle root, as the relay's includes checks it.
    assert_eq!(chainv4::crypto::bitcoin::sha256d(&raw), paying.compute_txid().to_byte_array());
    assert_eq!(proof.index, 2);
    assert_eq!(bytes(&proof.block), mined.block_hash().to_byte_array().to_vec());
    let (mut hash, mut at) = (paying.compute_txid().to_byte_array(), proof.index);

    for sibling in bytes(&proof.path).chunks(32) {
        hash = chainv4::crypto::bitcoin::sha256d(&if at % 2 == 0 { [&hash[..], sibling] } else { [sibling, &hash[..]] }.concat());
        at /= 2;
    }

    assert_eq!(hash, mined.header.merkle_root.to_byte_array());
}

#[test]
fn what_is_found_is_kept_between_runs() {
    let seen = Seen {
        height: 972_500,
        hash: "00000000000000000001ba187a086ecd0029f8ac4bc7b445822216850844a9ce".into(),
        coins: vec![Coin { txid: "ab".repeat(32), vout: 1, value: 9, script: "0014".into(), height: 972_450, spent: Some(Spent { txid: "cd".repeat(32), height: 972_460, witness: vec!["00".into()] }), proof: None }],
    };

    assert_eq!(serde_json::from_str::<Seen>(&serde_json::to_string(&seen).unwrap()).unwrap(), seen);
}

#[test]
fn only_a_socks5_proxy_is_used_and_peers_read_as_written() {
    assert_eq!(proxy("").unwrap(), None);
    assert_eq!(proxy("socks5h://127.0.0.1:9050").unwrap(), Some("127.0.0.1:9050".parse().unwrap()));
    assert_eq!(proxy("socks5://localhost:4447/").unwrap(), Some("127.0.0.1:4447".parse().unwrap()));
    assert!(proxy("http://127.0.0.1:8118").unwrap_err().contains("SOCKS5"), "not gone around");
    assert!(proxy("socks5h://tor.example:9050").is_err());

    assert!(peer("", Net::Main).is_none());
    assert_eq!(peer("10.0.0.2", Net::Main).unwrap().port(), None, "the network's own port");
    assert_eq!(peer("10.0.0.2:18444", Net::Regtest).unwrap().port(), Some(18444));
    assert_eq!(peer("node.example", Net::Testnet4).unwrap().port(), Some(48333));
    assert_eq!(Net::test("signet"), Net::Signet);
    assert_eq!(Net::test(""), Net::Testnet4);
    assert!(Net::Signet.testnet() && !Net::Main.testnet());
}

#[test]
fn the_checkpoints_are_blocks_of_their_own_networks() {
    assert_eq!(Net::Main.checkpoint().height, 972_410);
    assert_eq!(Net::Regtest.checkpoint().height, 0);
    // Proof of work alone: a mainnet block hash starts with many zeros.
    assert!(Net::Main.checkpoint().hash.to_string().starts_with("0000000000000000000"));
}

/// Bitcoin Core on a private regtest chain, serving compact filters, for the light client to sync
/// from.
struct Regtest {
    dir: std::path::PathBuf,
}

impl Regtest {
    const RPC: &str = "-rpcport=18953";
    const P2P: u16 = 18954;

    fn start() -> Option<Regtest> {
        let dir = std::env::temp_dir().join(format!("chainv4-light-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).ok()?;
        let started = std::process::Command::new("bitcoind")
            .args(["-regtest", "-daemonwait", "-fallbackfee=0.0001", "-blockfilterindex=1", "-peerblockfilters=1", "-listen=1", Self::RPC])
            .arg(format!("-port={}", Self::P2P))
            .arg(format!("-datadir={}", dir.display()))
            .status()
            .ok()?;

        started.success().then_some(Regtest { dir })
    }

    fn cli(&self, args: &[&str]) -> String {
        let out = std::process::Command::new("bitcoin-cli").args(["-regtest", Self::RPC, "-rpcwait"]).arg(format!("-datadir={}", self.dir.display())).args(args).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));

        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn mine(&self, blocks: u32) {
        let address = self.cli(&["getnewaddress"]);
        self.cli(&["generatetoaddress", &blocks.to_string(), &address]);
    }
}

impl Drop for Regtest {
    fn drop(&mut self) {
        let _ = std::process::Command::new("bitcoin-cli").args(["-regtest", Self::RPC]).arg(format!("-datadir={}", self.dir.display())).arg("stop").output();
        std::thread::sleep(std::time::Duration::from_secs(2));
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn until(what: &str, done: impl Fn() -> bool) {
    let started = std::time::Instant::now();

    while !done() {
        assert!(started.elapsed() < std::time::Duration::from_secs(90), "{what} within 90 s");
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

#[test]
#[ignore = "runs Bitcoin Core on a private regtest chain"]
fn the_light_client_finds_payments_to_its_scripts_and_broadcasts_to_peers() {
    let Some(core) = Regtest::start() else {
        panic!("bitcoind did not start: is Bitcoin Core installed?");
    };
    core.cli(&["createwallet", "miner"]);
    core.mine(101);

    // A script of ours, paid once before the light client starts and once after.
    let ours = core.cli(&["getnewaddress", "", "bech32"]);
    let info: serde_json::Value = serde_json::from_str(&core.cli(&["getaddressinfo", &ours])).unwrap();
    let script = chainv4::structs::hex::from_hex(&format!("0x{}", info["scriptPubKey"].as_str().unwrap())).unwrap();
    core.cli(&["sendtoaddress", &ours, "0.5"]);
    core.mine(1);

    let light = Light::start(Net::Regtest, Seen::default(), None, Some(format!("127.0.0.1:{}", Regtest::P2P)), [script.clone()].into(), egui::Context::default());
    until("the first payment found", || light.coins(&script).len() == 1);
    assert_eq!(light.coins(&script)[0].value, 50_000_000);
    until("synced", || light.status().synced);

    // A transaction the light client broadcasts, paying us again: mined from its peer's mempool.
    let raw = core.cli(&["createrawtransaction", "[]", &format!("{{\"{ours}\":0.25}}")]);
    let funded: serde_json::Value = serde_json::from_str(&core.cli(&["fundrawtransaction", &raw])).unwrap();
    let signed: serde_json::Value = serde_json::from_str(&core.cli(&["signrawtransactionwithwallet", funded["hex"].as_str().unwrap()])).unwrap();
    let raw = chainv4::structs::hex::from_hex(&format!("0x{}", signed["hex"].as_str().unwrap())).unwrap();
    let mut light = light;
    let id = light.broadcast(raw);
    until("the broadcast sent", || light.broadcasted(id).is_some());
    let txid = light.broadcasted(id).unwrap().unwrap();
    until("Bitcoin Core has it", || core.cli(&["getrawmempool"]).contains(&txid));
    core.mine(2);
    until("the second payment found", || light.coins(&script).len() == 2);
    let second = light.coins(&script).into_iter().find(|coin| coin.txid == txid).unwrap();
    assert_eq!((second.value, light.confirmations(second.height)), (25_000_000, 2));

    // Where it got to is kept: a second run from there finds nothing new, and nothing lost.
    let seen = light.seen();
    assert_eq!(seen.coins.len(), 2);
    drop(light);
    let again = Light::start(Net::Regtest, seen.clone(), None, Some(format!("127.0.0.1:{}", Regtest::P2P)), [script.clone()].into(), egui::Context::default());
    core.mine(1);
    until("synced again", || again.status().synced && again.seen().height > seen.height);
    assert_eq!(again.coins(&script).len(), 2);
}
