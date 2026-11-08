use {
    super::*,
    crate::crypto::bitcoin::tx::from_der,
    crate::structs::address::Address,
};

fn secret(byte: u8) -> [u8; 32] {
    [byte; 32]
}

fn key(byte: u8) -> [u8; 33] {
    crate::crypto::secp256k1::public_key(&secret(byte)).unwrap()
}

/// Three signers, the federation the same three ruling two of three, and an operator.
fn terms(peg_in: ([u8; 32], u32), funding: ([u8; 32], u32)) -> GraphTerms {
    let signers = vec![key(11), key(12), key(13)];
    let script = peg_in_script(&Address::Ed25519([7; 32]), 1_234, &Address::Contract([9; 32]), &signers);

    GraphTerms {
        federation: signers.clone(),
        signers,
        threshold: 2,
        operator: key(21),
        funding,
        funding_value: 1_000_000,
        peg_in,
        peg_in_value: 10_000_000,
        peg_in_script: script,
        stake: 200_000,
        fee: 2_000,
        challenge_window: 6,
        dispute_window: 4,
    }
}

#[test]
fn a_graph_holds_the_peg_in_to_take_after_the_window_or_to_a_verdict_after_a_challenge() {
    let graph = Graph::build(terms(([1; 32], 0), ([2; 32], 1))).unwrap();
    let kickoff = graph.kickoff.txid();

    // The kickoff: the claim connector, the challenge connector, the stake in the verdict connector,
    // and the operator's change.
    assert_eq!(graph.kickoff.outputs.iter().map(|output| output.value).collect::<Vec<_>>(), [2_000, 2_000, 200_000, 1_000_000 - 2 * 2_000 - 200_000 - 2_000]);
    assert_eq!(graph.kickoff.outputs[2].script, p2wsh(&graph.verdict_script()));

    // Take: the peg-in and both connectors, the claim connector only after the challenge window.
    assert_eq!(graph.take.inputs.iter().map(|input| (input.txid, input.vout)).collect::<Vec<_>>(), [([1; 32], 0), (kickoff, 0), (kickoff, 1)]);
    assert_eq!(graph.take.inputs[1].sequence, 6, "BIP68: six blocks");
    assert_eq!(graph.take.outputs[0].value, 10_000_000 + 4_000 - 2_000);

    // The challenge spends the challenge connector alone, so Take can no longer be; take after a
    // challenge and the default spend the verdict connector, the default only 10 blocks on.
    assert_eq!(graph.challenge.inputs.iter().map(|input| (input.txid, input.vout)).collect::<Vec<_>>(), [(kickoff, 1)]);
    assert_eq!(graph.vindicated.inputs.iter().map(|input| (input.txid, input.vout)).collect::<Vec<_>>(), [([1; 32], 0), (kickoff, 2)]);
    assert_eq!((graph.default.inputs[0].vout, graph.default.inputs[0].sequence), (2, 10));
    assert_eq!(graph.vindicated.outputs[0].value, 10_000_000 + 200_000 - 2_000);

    // Funding short of the stake, the connectors and the fee builds nothing.
    assert!(Graph::build(GraphTerms { funding_value: 205_000, ..terms(([1; 32], 0), ([2; 32], 1)) }).is_err());
    assert!(Graph::build(GraphTerms { threshold: 4, ..terms(([1; 32], 0), ([2; 32], 1)) }).is_err());
    // Amounts past all there will ever be, which would overflow the sums, build nothing either.
    assert!(Graph::build(GraphTerms { stake: u64::MAX, ..terms(([1; 32], 0), ([2; 32], 1)) }).is_err());
    assert!(Graph::build(GraphTerms { funding_value: MAX_MONEY + 1, ..terms(([1; 32], 0), ([2; 32], 1)) }).is_err());
}

#[test]
fn each_signers_presignatures_are_its_own_and_cover_what_they_must() {
    let graph = Graph::build(terms(([1; 32], 0), ([2; 32], 1))).unwrap();
    let presigned = graph.presign(&secret(11));

    assert!(graph.check_presigned(&key(11), &presigned));
    assert!(!graph.check_presigned(&key(12), &presigned), "another signer's");
    assert!(!graph.check_presigned(&key(11), &presigned[..4]), "one short");
    let mut swapped = presigned.clone();
    swapped.swap(0, 1);
    assert!(!graph.check_presigned(&key(11), &swapped), "out of place");

    // The challenge's presignature is SINGLE|ANYONECANPAY: a challenger's own inputs and outputs
    // change nothing it signed.
    assert_eq!(*presigned[2].last().unwrap() as u32, SIGHASH_SINGLE | SIGHASH_ANYONECANPAY);
    let mut funded = graph.challenge.clone();
    funded.inputs.push(input([5; 32], 3, NO_LOCK));
    funded.outputs.push(TxOut { value: 1_000, script: vec![0x51] });
    let hash = |tx: &Transaction| crate::crypto::bitcoin::sha256d(&tx.sighash_preimage(0, &graph.challenge_script(), graph.kickoff.outputs[1].value, SIGHASH_SINGLE | SIGHASH_ANYONECANPAY));
    assert_eq!(hash(&funded), hash(&graph.challenge));
    let (_, body) = presigned[2].split_last().unwrap();
    assert!(crate::crypto::secp256k1::verify_hash(&key(11), &hash(&funded), &from_der(body).unwrap()));

    // Take's presignatures are SIGHASH_ALL: another output, and they no longer hold.
    let mut robbed = graph.take.clone();
    robbed.outputs[0].script = vec![0x51];
    let all = crate::crypto::bitcoin::sha256d(&robbed.sighash_preimage(0, &graph.terms.peg_in_script, graph.terms.peg_in_value, SIGHASH_ALL));
    let (_, body) = presigned[0].split_last().unwrap();
    assert!(!crate::crypto::secp256k1::verify_hash(&key(11), &all, &from_der(body).unwrap()));
}

#[test]
fn a_sighash_preimage_leaves_out_what_its_type_does_not_sign() {
    let graph = Graph::build(terms(([1; 32], 0), ([2; 32], 1))).unwrap();
    let tx = &graph.take;
    let code = graph.terms.peg_in_script.clone();
    let all = tx.sighash_preimage(0, &code, 1, SIGHASH_ALL);

    assert_eq!(all, tx.script_preimage(0, &code, 1), "ALL is as it was");
    let single = tx.sighash_preimage(0, &code, 1, SIGHASH_SINGLE | SIGHASH_ANYONECANPAY);
    assert_eq!(single[4..68], [0; 64], "no other input, no sequence");
    assert_ne!(single[single.len() - 40..single.len() - 8], [0; 32], "its own output");
    let beyond = tx.sighash_preimage(2, &code, 1, SIGHASH_SINGLE);
    assert_eq!(beyond[beyond.len() - 40..beyond.len() - 8], [0; 32], "no output at its place");
}

/// Bitcoin Core on a private regtest chain, stopped when dropped.
struct Regtest {
    dir: std::path::PathBuf,
    port: u16,
}

impl Regtest {
    fn start(port: u16) -> Regtest {
        let dir = std::env::temp_dir().join(format!("chainv4-bitvm-{}-{port}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let started = std::process::Command::new("bitcoind")
            .args(["-regtest", "-daemonwait", "-fallbackfee=0.0001", "-txindex=1", &format!("-rpcport={port}"), &format!("-port={}", port + 1)])
            .arg(format!("-datadir={}", dir.display()))
            .status()
            .expect("is Bitcoin Core installed?");
        assert!(started.success());
        let node = Regtest { dir, port };
        node.cli(&["createwallet", "test"]).unwrap();
        node.mine(101);

        node
    }

    fn cli(&self, args: &[&str]) -> Result<String, String> {
        let out = std::process::Command::new("bitcoin-cli")
            .args(["-regtest", &format!("-rpcport={}", self.port), "-rpcwait"])
            .arg(format!("-datadir={}", self.dir.display()))
            .args(args)
            .output()
            .map_err(|err| err.to_string())?;

        match out.status.success() {
            true => Ok(String::from_utf8_lossy(&out.stdout).trim().to_string()),
            false => Err(String::from_utf8_lossy(&out.stderr).trim().to_string()),
        }
    }

    fn mine(&self, blocks: u32) {
        let address = self.cli(&["getnewaddress"]).unwrap();
        self.cli(&["generatetoaddress", &blocks.to_string(), &address]).unwrap();
    }

    /// Pays an output script from the wallet, mined, and says where the payment is.
    fn pay(&self, script: &[u8], sats: u64) -> ([u8; 32], u32) {
        let address = crate::structs::bech32::encode_segwit("bcrt", &script[2..]);
        let txid = self.cli(&["sendtoaddress", &address, &format!("{}.{:08}", sats / 100_000_000, sats % 100_000_000)]).unwrap();
        self.mine(1);
        let raw = self.cli(&["getrawtransaction", &txid, "true"]).unwrap();
        let tx: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let vout = tx["vout"].as_array().unwrap().iter().find(|out| out["scriptPubKey"]["hex"].as_str() == Some(&crate::structs::hex::to_hex(script)[2..])).unwrap()["n"].as_u64().unwrap();
        let mut id: [u8; 32] = crate::structs::hex::from_hex(&format!("0x{txid}")).unwrap().try_into().unwrap();
        id.reverse();

        (id, vout as u32)
    }

    fn send(&self, tx: &Transaction) -> Result<String, String> {
        self.cli(&["sendrawtransaction", &crate::structs::hex::to_hex(&tx.encode())[2..]])
    }
}

impl Drop for Regtest {
    fn drop(&mut self) {
        let _ = self.cli(&["stop"]);
    }
}

/// A peg-in requested by a depositor and confirmed by every signer, and an operator's funding: a
/// graph ready to presign, on the node.
fn pegged_in(node: &Regtest) -> Graph {
    let signers = [11u8, 12, 13];
    let keys: Vec<[u8; 33]> = signers.iter().map(|byte| key(*byte)).collect();
    let request = request_script(&keys, &key(31), 20);
    let requested = node.pay(&p2wsh(&request), 10_010_000);
    let base = terms(([0; 32], 0), ([0; 32], 0));
    let mut confirm = transaction(vec![input(requested.0, requested.1, NO_LOCK)], vec![TxOut { value: 10_000_000, script: p2wsh(&base.peg_in_script) }]);
    let hash = crate::crypto::bitcoin::sha256d(&confirm.sighash_preimage(0, &request, 10_010_000, SIGHASH_ALL));
    confirm.inputs[0].witness = multisig_witness(signers.iter().map(|byte| sign(&secret(*byte), &hash, SIGHASH_ALL)).collect(), &request, Some(true));
    node.send(&confirm).unwrap();
    node.mine(1);
    let funding = node.pay(&p2wpkh(&hash160(&key(21))), 1_000_000);

    Graph::build(GraphTerms { peg_in: (confirm.txid(), 0), funding, ..base }).unwrap()
}

/// The paths of the graph through Bitcoin Core itself: what only a real node says, that every
/// script, witness, sighash and relative lock is as Bitcoin runs it.
#[test]
#[ignore = "runs Bitcoin Core on a private regtest chain"]
fn bitcoin_core_runs_every_path_of_the_graph() {
    let node = Regtest::start(18993);
    let presign = |graph: &Graph| -> Vec<Vec<Vec<u8>>> {
        let signed: Vec<Vec<Vec<u8>>> = [11u8, 12, 13].iter().map(|byte| graph.presign(&secret(*byte))).collect();
        for (byte, signatures) in [11u8, 12, 13].iter().zip(&signed) {
            assert!(graph.check_presigned(&key(*byte), signatures));
        }
        signed
    };

    // Unchallenged: Take waits out the challenge window, then holds.
    let graph = pegged_in(&node);
    let signatures = presign(&graph);
    node.send(&graph.signed_kickoff(&secret(21))).unwrap();
    node.mine(1);
    let early = node.send(&graph.signed_take(&signatures, &secret(21))).expect_err("refused");
    assert!(early.contains("non-BIP68-final"), "{early}");
    node.mine(5);
    node.send(&graph.signed_take(&signatures, &secret(21))).unwrap();
    node.mine(1);

    // Challenged: the challenger pays its fee from its own coins; Take no longer holds; the
    // federation's two of three vindicate the operator.
    let graph = pegged_in(&node);
    let signatures = presign(&graph);
    node.send(&graph.signed_kickoff(&secret(21))).unwrap();
    node.mine(1);
    let challenger = node.pay(&p2wpkh(&hash160(&key(41))), 50_000);
    let mut challenge = graph.challenge_with(&signatures, vec![input(challenger.0, challenger.1, NO_LOCK)], vec![TxOut { value: 40_000, script: p2wpkh(&hash160(&key(41))) }]);
    let preimage = challenge.preimage(1, &hash160(&key(41)), 50_000);
    challenge.inputs[1].witness = vec![sign(&secret(41), &crate::crypto::bitcoin::sha256d(&preimage), SIGHASH_ALL), key(41).to_vec()];
    node.send(&challenge).unwrap();
    node.mine(6);
    let killed = node.send(&graph.signed_take(&signatures, &secret(21))).expect_err("refused");
    assert!(killed.contains("missingorspent") || killed.contains("missing-inputs"), "{killed}");
    let verdict = vec![graph.verdict_signature(&secret(11)), graph.verdict_signature(&secret(13))];
    node.send(&graph.signed_vindicated(&signatures, verdict)).unwrap();
    node.mine(1);

    // Challenged and no verdict: the default gives the stake back once its blocks pass, and the
    // peg-in stays for another operator.
    let graph = pegged_in(&node);
    let signatures = presign(&graph);
    node.send(&graph.signed_kickoff(&secret(21))).unwrap();
    node.mine(1);
    let early = node.send(&graph.signed_default(&signatures)).expect_err("refused");
    assert!(early.contains("non-BIP68-final"), "{early}");
    node.mine(9);
    node.send(&graph.signed_default(&signatures)).unwrap();
    node.mine(1);
    let spent = node.cli(&["gettxout", &{
        let mut id = graph.terms.peg_in.0;
        id.reverse();
        crate::structs::hex::to_hex(&id)[2..].to_string()
    }, "0"]).unwrap();
    assert!(!spent.is_empty(), "the peg-in is still there");

    // A request the signers never confirm: the depositor takes it back after its blocks.
    let keys = vec![key(11), key(12), key(13)];
    let request = request_script(&keys, &key(31), 20);
    let requested = node.pay(&p2wsh(&request), 1_000_000);
    let mut refund = transaction(vec![input(requested.0, requested.1, 20)], vec![TxOut { value: 990_000, script: p2wpkh(&hash160(&key(31))) }]);
    let hash = crate::crypto::bitcoin::sha256d(&refund.sighash_preimage(0, &request, 1_000_000, SIGHASH_ALL));
    refund.inputs[0].witness = vec![sign(&secret(31), &hash, SIGHASH_ALL), Vec::new(), request.clone()];
    assert!(node.send(&refund).expect_err("refused").contains("non-BIP68-final"));
    node.mine(20);
    node.send(&refund).unwrap();
}
