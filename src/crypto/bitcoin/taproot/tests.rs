use {
    super::*,
    crate::crypto::bitcoin::tx::{TxIn, TxOut},
    crate::structs::hex::{from_hex, to_hex},
};

fn bytes32(hex: &str) -> [u8; 32] {
    from_hex(&format!("0x{hex}")).unwrap().try_into().unwrap()
}

#[test]
fn bip340_s_first_test_vector_signs_and_verifies_as_written() {
    let secret = bytes32("0000000000000000000000000000000000000000000000000000000000000003");
    let key = x_only_of(&secret).unwrap();
    assert_eq!(to_hex(&key), "0xf9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9");

    let signature = schnorr_sign(&secret, &[0; 32]).unwrap();
    assert_eq!(to_hex(&signature), "0xe907831f80848d1069a5371b402410364bdf1c5f8307b0084c55f1ce2dca821525f66a4a85ea8b71e482a74f382d2ce5ebeee8fdb2172f477df4900d310536c0");
    assert!(schnorr_verify(&key, &[0; 32], &signature));
    assert!(!schnorr_verify(&key, &[1; 32], &signature), "another message");
}

#[test]
fn a_tree_s_paths_lead_each_leaf_to_its_root() {
    for count in [1usize, 2, 3, 5, 8] {
        let leaves: Vec<Vec<u8>> = (0..count).map(|at| vec![0x51, at as u8]).collect();
        let tree = ScriptTree::of(leaves.clone()).unwrap();

        for (at, leaf) in leaves.iter().enumerate() {
            let root = tree.paths[at].iter().fold(leaf_hash(leaf), |node, sibling| branch_hash(&node, sibling));
            assert_eq!(root, tree.root, "leaf {at} of {count}");
        }
    }

    assert!(ScriptTree::of(Vec::new()).is_none());
    // A key-path output's tweak commits to the key alone; a tree's to its root too.
    let key = x_only_of(&[7; 32]).unwrap();
    let alone = TaprootOutput::new(key, None).unwrap();
    let rooted = TaprootOutput::new(key, ScriptTree::of(vec![vec![0x51]])).unwrap();
    assert_ne!(alone.output, rooted.output);
    let tweaked = tweaked_secret(&[7; 32], None).unwrap();
    assert_eq!(x_only_of(&tweaked).unwrap(), alone.output, "the tweaked secret's key is the output key");
}

/// Bitcoin Core on regtest takes a taproot output spent by its key path and another by a leaf of
/// its tree: what only a real node says, that the tweak, the control block and BIP341's sighash
/// are as Bitcoin has them.
#[test]
#[ignore = "runs Bitcoin Core on a private regtest chain"]
fn bitcoin_core_takes_taproot_spends_by_the_key_and_by_a_leaf() {
    let dir = std::env::temp_dir().join(format!("chainv4-taproot-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let port = 18997u16;
    let cli = |args: &[&str]| -> Result<String, String> {
        let out = std::process::Command::new("bitcoin-cli").args(["-regtest", &format!("-rpcport={port}"), "-rpcwait"]).arg(format!("-datadir={}", dir.display())).args(args).output().unwrap();
        match out.status.success() {
            true => Ok(String::from_utf8_lossy(&out.stdout).trim().to_string()),
            false => Err(String::from_utf8_lossy(&out.stderr).trim().to_string()),
        }
    };
    assert!(std::process::Command::new("bitcoind").args(["-regtest", "-daemonwait", "-fallbackfee=0.0001", "-txindex=1", &format!("-rpcport={port}"), &format!("-port={}", port + 1)]).arg(format!("-datadir={}", dir.display())).status().unwrap().success());
    cli(&["createwallet", "test"]).unwrap();
    let mine = |blocks: u32| {
        let address = cli(&["getnewaddress"]).unwrap();
        cli(&["generatetoaddress", &blocks.to_string(), &address]).unwrap();
    };
    mine(101);
    let pay = |script: &[u8], sats: u64| -> ([u8; 32], u32) {
        let address = crate::structs::bech32::encode_witness("bcrt", 1, &script[2..]);
        let txid = cli(&["sendtoaddress", &address, &format!("0.{sats:08}")]).unwrap();
        mine(1);
        let tx: serde_json::Value = serde_json::from_str(&cli(&["getrawtransaction", &txid, "true"]).unwrap()).unwrap();
        let vout = tx["vout"].as_array().unwrap().iter().find(|out| out["scriptPubKey"]["hex"].as_str() == Some(&to_hex(script)[2..])).unwrap()["n"].as_u64().unwrap();
        let mut id = bytes32(&txid);
        id.reverse();
        (id, vout as u32)
    };
    let send = |tx: &Transaction| cli(&["sendrawtransaction", &to_hex(&tx.encode())[2..]]);
    let back = TxOut { value: 90_000, script: crate::crypto::bitcoin::tx::p2wpkh(&[0xab; 20]) };

    // By the key path: the tweaked secret signs BIP341's hash, SIGHASH_DEFAULT.
    let secret = [7u8; 32];
    let leaf = [vec![0x20], x_only_of(&[8; 32]).unwrap().to_vec(), vec![0xac]].concat();
    let output = TaprootOutput::new(x_only_of(&secret).unwrap(), ScriptTree::of(vec![vec![0x51], leaf.clone()])).unwrap();
    let funded = pay(&output.script(), 100_000);
    let spent = [TxOut { value: 100_000, script: output.script() }];
    let mut tx = Transaction { version: 2, inputs: vec![TxIn { txid: funded.0, vout: funded.1, script_sig: Vec::new(), sequence: 0xffff_fffd, witness: Vec::new() }], outputs: vec![back.clone()], lock_time: 0 };
    let hash = sighash(&tx, 0, &spent, SIGHASH_DEFAULT, None);
    let tweaked = tweaked_secret(&secret, output.tree.as_ref().map(|tree| &tree.root)).unwrap();
    tx.inputs[0].witness = vec![witness_signature(&schnorr_sign(&tweaked, &hash).unwrap(), SIGHASH_DEFAULT)];
    send(&tx).unwrap();
    mine(1);

    // By a leaf: <key> OP_CHECKSIG, the leaf's key signing its own hash under SIGHASH_ALL.
    let funded = pay(&output.script(), 100_000);
    let mut tx = Transaction { version: 2, inputs: vec![TxIn { txid: funded.0, vout: funded.1, script_sig: Vec::new(), sequence: 0xffff_fffd, witness: Vec::new() }], outputs: vec![back], lock_time: 0 };
    let hash = sighash(&tx, 0, &spent, 0x01, Some(&leaf_hash(&leaf)));
    let signature = witness_signature(&schnorr_sign(&[8; 32], &hash).unwrap(), 0x01);
    tx.inputs[0].witness = output.script_witness(1, vec![signature]).unwrap();
    send(&tx).unwrap();
    mine(1);
    let _ = cli(&["stop"]);
}
