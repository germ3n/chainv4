use {
    super::*,
    crate::crypto::bitcoin::tx::p2wpkh,
    crate::swap::htlc::Coins,
    crate::crypto::bitcoin::hash160,
    crate::crypto::secp256k1,
};

fn key(byte: u8) -> ([u8; 32], [u8; 33]) {
    let secret = [byte; 32];

    (secret, secp256k1::public_key(&secret).unwrap())
}

fn htlc(locktime: u32) -> (Htlc, [u8; 32], [u8; 32], [u8; 32]) {
    let ((claimer, claim), (refunder, refund)) = (key(0x11), key(0x22));
    let secret = [0x5e; 32];

    (Htlc { hash: sha256(&secret), claim, refund, locktime }, secret, claimer, refunder)
}

#[test]
fn the_script_is_the_template_and_reads_back() {
    let (htlc, ..) = htlc(1_793_000_000);
    let script = htlc.script();

    // OP_IF OP_SIZE 32 OP_EQUALVERIFY OP_SHA256 <32> OP_EQUALVERIFY <33> OP_ELSE <4> OP_CLTV
    // OP_DROP <33> OP_ENDIF OP_CHECKSIG.
    assert_eq!(&script[..7], &[0x63, 0x82, 0x01, 0x20, 0x88, 0xa8, 0x20]);
    assert_eq!(script.len(), 7 + 32 + 2 + 33 + 1 + 5 + 3 + 33 + 2);
    assert_eq!(Htlc::parse(&script), Some(htlc));
    assert_eq!(htlc.output()[..2], [0x00, 0x20], "P2WSH");
    assert!(htlc.address(Network::Main).starts_with("bc1q") && htlc.address(Network::Main).len() == 62);
    assert!(htlc.address(Network::Test).starts_with("tb1q"));

    // Anything else is no HTLC of ours.
    let mut other = script.clone();
    other[1] = 0x87;
    assert_eq!(Htlc::parse(&other), None);
    assert_eq!(Htlc::parse(&script[..script.len() - 1]), None);
    assert_eq!(Htlc::parse(&[script.clone(), vec![0x75]].concat()), None);
}

#[test]
fn numbers_are_pushed_as_scripts_write_them() {
    let pushed = |number| {
        let mut out = Vec::new();
        push_number(&mut out, number);
        out
    };

    assert_eq!(pushed(1), [1, 0x01]);
    assert_eq!(pushed(127), [1, 0x7f]);
    assert_eq!(pushed(128), [2, 0x80, 0x00], "the top bit would read as a sign");
    assert_eq!(pushed(-1), [1, 0x81]);
    assert_eq!(pushed(1_793_000_000), [4, 0x40, 0x02, 0xdf, 0x6a], "0x6adf0240, little-end first");
    assert_eq!(pushed(i64::from(u32::MAX)), [5, 0xff, 0xff, 0xff, 0xff, 0x00]);

    for number in [1, 127, 128, 255, 256, -1, -128, 1_793_000_000, i64::from(u32::MAX)] {
        assert_eq!(read_number(&pushed(number)[1..]), Some(number), "{number}");
    }
}

#[test]
fn spends_are_signed_for_their_branch_and_refused_otherwise() {
    let (htlc, secret, claimer, refunder) = htlc(1_793_000_000);
    let to = p2wpkh(&hash160(&key(0x33).1));
    let claim = htlc.spend(Coins { txid: [7; 32], vout: 1, value: 100_000 }, to.clone(), 2, Spend::Claim { secret }, &claimer).unwrap();

    assert_eq!((claim.lock_time, claim.inputs[0].sequence), (0, 0xffff_fffd));
    assert_eq!(&claim.inputs[0].witness[1..], &[secret.to_vec(), vec![1], htlc.script()]);
    assert_eq!(claim.outputs[0].value, 100_000 - 2 * claim.vsize());
    assert_eq!(revealed(&claim, &htlc.hash), Some(secret), "what the other side reads");

    // The signature is over BIP143's hash of the spend, by the claim key.
    let signature = &claim.inputs[0].witness[0];
    assert_eq!(signature.last(), Some(&1), "SIGHASH_ALL");
    let preimage = claim.script_preimage(0, &htlc.script(), 100_000);
    assert!(der_holds(&signature[..signature.len() - 1], &htlc.claim, &preimage));

    let refund = htlc.spend(Coins { txid: [7; 32], vout: 1, value: 100_000 }, to.clone(), 2, Spend::Refund, &refunder).unwrap();
    assert_eq!((refund.lock_time, refund.inputs[0].sequence), (1_793_000_000, 0xffff_fffe));
    assert_eq!(refund.inputs[0].witness[1], Vec::<u8>::new(), "the ELSE branch");

    // The wrong key, the wrong secret, and a fee past the coins.
    assert!(htlc.spend(Coins { txid: [7; 32], vout: 1, value: 100_000 }, to.clone(), 2, Spend::Claim { secret }, &refunder).unwrap_err().contains("claim key"));
    assert!(htlc.spend(Coins { txid: [7; 32], vout: 1, value: 100_000 }, to.clone(), 2, Spend::Refund, &claimer).unwrap_err().contains("refund key"));
    assert!(htlc.spend(Coins { txid: [7; 32], vout: 1, value: 100_000 }, to.clone(), 2, Spend::Claim { secret: [0; 32] }, &claimer).unwrap_err().contains("not the swap's hash"));
    assert!(htlc.spend(Coins { txid: [7; 32], vout: 1, value: 1_000 }, to, 50, Spend::Refund, &refunder).unwrap_err().contains("dust"));
}

/// Whether a DER signature over BIP143's preimage is a key's.
fn der_holds(der: &[u8], key: &[u8; 33], preimage: &[u8]) -> bool {
    let signature = crate::crypto::bitcoin::tx::from_der(der).unwrap();

    crate::crypto::bitcoin::verify(key, preimage, &signature)
}

/// Bitcoin Core on a private regtest chain, in a directory of its own, stopped when dropped.
#[cfg(feature = "node")]
struct Regtest {
    dir: std::path::PathBuf,
}

#[cfg(feature = "node")]
impl Regtest {
    fn start() -> Option<Regtest> {
        let dir = std::env::temp_dir().join(format!("chainv4-regtest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).ok()?;
        let started = std::process::Command::new("bitcoind")
            .args(["-regtest", "-daemonwait", "-fallbackfee=0.0001", "-txindex=1", "-rpcport=18943", "-port=18944"])
            .arg(format!("-datadir={}", dir.display()))
            .status()
            .ok()?;

        started.success().then_some(Regtest { dir })
    }

    fn cli(&self, args: &[&str]) -> Result<String, String> {
        let out = std::process::Command::new("bitcoin-cli")
            .args(["-regtest", "-rpcport=18943", "-rpcwait"])
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

    /// Pays an HTLC and mines it: the outpoint and amount it holds.
    fn fund(&self, htlc: &Htlc, sats: u64) -> ([u8; 32], u32) {
        let address = bech32::encode_segwit("bcrt", &sha256(&htlc.script()));
        let txid = self.cli(&["sendtoaddress", &address, &format!("{}.{:08}", sats / 100_000_000, sats % 100_000_000)]).unwrap();
        self.mine(1);
        let tx: serde_json::Value = serde_json::from_str(&self.cli(&["getrawtransaction", &txid, "true"]).unwrap()).unwrap();
        let vout = tx["vout"].as_array().unwrap().iter().find(|out| out["scriptPubKey"]["hex"] == crate::structs::hex::to_hex(&htlc.output())[2..]).unwrap();
        let mut id: [u8; 32] = crate::structs::hex::from_hex(&format!("0x{txid}")).unwrap().try_into().unwrap();
        // Explorers and RPC write a txid backwards.
        id.reverse();

        (id, vout["n"].as_u64().unwrap() as u32)
    }

    fn send(&self, tx: &Transaction) -> Result<String, String> {
        self.cli(&["sendrawtransaction", &crate::structs::hex::to_hex(&tx.encode())[2..]])
    }
}

#[cfg(feature = "node")]
impl Drop for Regtest {
    fn drop(&mut self) {
        let _ = self.cli(&["stop"]);
        std::thread::sleep(std::time::Duration::from_secs(2));
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[cfg(feature = "node")]
#[test]
#[ignore = "runs Bitcoin Core on a private regtest chain"]
fn bitcoin_core_takes_the_claim_and_the_refund_and_refuses_the_rest() {
    let Some(node) = Regtest::start() else {
        panic!("bitcoind did not start: is Bitcoin Core installed?");
    };
    node.cli(&["createwallet", "test"]).unwrap();
    node.mine(101);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as u32;
    let to = p2wpkh(&hash160(&key(0x33).1));

    // A claim with the secret is taken, and mined; the secret is there for the other side.
    let (open, secret, claimer, refunder) = htlc(now + 86_400);
    let (txid, vout) = node.fund(&open, 50_000);
    let claim = open.spend(Coins { txid, vout, value: 50_000 }, to.clone(), 2, Spend::Claim { secret }, &claimer).unwrap();
    node.send(&claim).unwrap();
    node.mine(1);
    assert_eq!(node.cli(&["getrawtransaction", &crate::structs::hex::to_hex(&{ let mut id = claim.txid(); id.reverse(); id })[2..], "true"]).map(|text| text.contains("\"confirmations\": 1")), Ok(true));

    // Before its lock time a refund is not final, and the wrong secret fails the script.
    let (txid, vout) = node.fund(&open, 50_000);
    let early = open.spend(Coins { txid, vout, value: 50_000 }, to.clone(), 2, Spend::Refund, &refunder).unwrap();
    let refused = node.send(&early).unwrap_err();
    assert!(refused.contains("non-final") || refused.contains("Locktime requirement not satisfied"), "{refused}");

    let mut forged = open.spend(Coins { txid, vout, value: 50_000 }, to.clone(), 2, Spend::Claim { secret }, &claimer).unwrap();
    forged.inputs[0].witness[1] = vec![0x5f; 32];
    let refused = node.send(&forged).unwrap_err();
    assert!(refused.contains("Script failed") || refused.contains("script-verify-flag-failed"), "{refused}");

    // Past its lock time, by the chain's median time, the refund is taken.
    let (past, _, _, refunder) = htlc(now - 7_200);
    let (txid, vout) = node.fund(&past, 50_000);
    let refund = past.spend(Coins { txid, vout, value: 50_000 }, to, 2, Spend::Refund, &refunder).unwrap();
    node.send(&refund).unwrap();
}
