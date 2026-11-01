//! contracts/lua/btc_gateway.lua over the relay: a deposit to an address's deposit address minted
//! as wBTC on its proof, a withdrawal paid by a release the gateway checks and the federation
//! signs input by input as BIP143 has it, and a sweep of an old federation's coins.

use {
    super::btc_relay::{REGTEST, coinbase, mined},
    super::*,
    crate::crypto::bitcoin::sha256d,
    crate::crypto::bitcoin::tx::{Transaction, TxIn, TxOut},
    crate::registry::federation::{DecidedFederation, Federation},
};

const NOW: u64 = 1_800_000_000_000;
/// This chain's id.
const HERE: u64 = 0x12_3456;
/// The least a release pays a virtual byte.
const FLOOR: u64 = 2;
/// The most fee a release pays.
const CAP: u64 = 10_000;
/// Who deposits and withdraws.
#[cfg(feature = "bridge")]
const ALICE: Address = Address::Ed25519([0xa; 32]);

fn bytes(value: &[u8]) -> TxCallArgumentType {
    TxCallArgumentType::Bytes(value.to_vec())
}

fn number(value: u64) -> TxCallArgumentType {
    TxCallArgumentType::U64(value)
}

fn federation(secrets: &[u8], threshold: u32) -> Federation {
    Federation {
        signers: secrets.iter().map(|byte| crate::crypto::secp256k1::public_key(&[*byte; 32]).unwrap()).collect(),
        threshold,
        chain_depth: 20,
        evm_depth: 12,
        bitcoin_depth: 2,
    }
}

struct Btc {
    world: World,
    relay: Address,
    template: Address,
    gateway: Address,
    wbtc: Address,
    /// The relay's tip and its time, to mine on.
    tip: [u8; 32],
    time: u32,
}

impl Btc {
    fn new() -> Btc {
        Btc::on_chain(HERE)
    }

    fn on_chain(chain_id: u64) -> Btc {
        let mut btc = Btc::started(chain_id);
        btc.set_up();

        btc
    }

    /// The relay started, and the gateway deployed but not set up.
    fn started(chain_id: u64) -> Btc {
        let mut world = World {
            vm: Arc::new(Vm::new(&[Engine::Lua, Engine::Wasm], HashType::default_chain()).unwrap().with_federation(Some(federation(&[21, 22, 23], 2))).with_chain_id(chain_id)),
            ..World::new()
        };
        let relay = world.deploy(Code::Wasm(example("btc_relay")));
        let template = world.lua_code(include_str!("../../../contracts/lua/wrapped.lua"));
        let gateway = world.lua_code(include_str!("../../../contracts/lua/btc_gateway.lua"));
        let start = mined(&[0; 32], &[0; 32], 1_700_000_000, REGTEST);
        world.call_at(DEPLOYER, relay, "start", &[bytes(&start), number(0), TxCallArgumentType::String("regtest".into())], 0, NOW).result.unwrap();

        Btc { world, relay, template, gateway, wbtc: gateway, tip: sha256d(&start), time: 1_700_000_000 }
    }

    fn set_up(&mut self) {
        let made = self.call("setup", &[address(self.relay), address(self.template), number(CAP), number(FLOOR), number(3_600_000)]).unwrap();
        let [TxCallArgumentType::Address(wbtc)] = made[..] else { panic!("{made:?}") };
        self.wbtc = wbtc;
    }

    fn call_as(&self, sender: Address, function: &str, args: &[TxCallArgumentType]) -> Result<Vec<TxCallArgumentType>, String> {
        self.world.call_at(sender, self.gateway, function, args, 0, NOW).result.map_err(|err| err.to_string())
    }

    fn call(&self, function: &str, args: &[TxCallArgumentType]) -> Result<Vec<TxCallArgumentType>, String> {
        self.call_as(DEPLOYER, function, args)
    }

    /// A block holding a coinbase and the transaction, on the relay's tip: its hash, the
    /// transaction's place and Merkle path in it, and the coinbase with its own path.
    fn mine(&mut self, tx: &[u8]) -> ([u8; 32], u64, Vec<u8>, Vec<u8>, Vec<u8>) {
        let first = coinbase(self.time);
        let txid = sha256d(tx);
        let root = sha256d(&[sha256d(&first), txid].concat());
        self.time += 600;
        let header = mined(&self.tip, &root, self.time, REGTEST);
        self.world.call_at(DEPLOYER, self.relay, "submit", &[bytes(&header)], 0, NOW).result.unwrap();
        self.tip = sha256d(&header);

        (self.tip, 1, sha256d(&first).to_vec(), first, txid.to_vec())
    }

    /// An empty block on the tip, burying what came before one deeper.
    fn bury(&mut self) {
        self.time += 600;
        let header = mined(&self.tip, &[9; 32], self.time, REGTEST);
        self.world.call_at(DEPLOYER, self.relay, "submit", &[bytes(&header)], 0, NOW).result.unwrap();
        self.tip = sha256d(&header);
    }

    /// A release proposed under a signer of the federation in force's signature, for the number it
    /// will take, the withdrawals given as the gateway is given them.
    fn propose(&self, raw: &[u8], ids: &[TxCallArgumentType]) -> Result<Vec<TxCallArgumentType>, String> {
        self.propose_as(21, raw, ids)
    }

    fn propose_as(&self, secret: u8, raw: &[u8], ids: &[TxCallArgumentType]) -> Result<Vec<TxCallArgumentType>, String> {
        self.call("propose", &[&[bytes(raw), bytes(&self.propose_signature(secret, self.next_proposal(), raw, ids))], ids].concat())
    }

    fn next_proposal(&self) -> u64 {
        match self.call("proposal_count", &[]).unwrap()[..] {
            [TxCallArgumentType::I64(count)] => count as u64 + 1,
            ref other => panic!("{other:?}"),
        }
    }

    /// What a signer signs to propose a release: as btc_propose_digest has it, which a build without
    /// the bridges has not.
    fn propose_signature(&self, secret: u8, number: u64, raw: &[u8], ids: &[TxCallArgumentType]) -> Vec<u8> {
        self.signature_for(secret, self.world.vm.chain_id(), self.gateway, number, raw, ids)
    }

    fn signature_for(&self, secret: u8, chain_id: u64, gateway: Address, number: u64, raw: &[u8], ids: &[TxCallArgumentType]) -> Vec<u8> {
        let named: Vec<u8> = ids.iter().flat_map(|id| match id {
            TxCallArgumentType::U64(id) => id.to_le_bytes(),
            TxCallArgumentType::I64(id) => (*id as u64).to_le_bytes(),
            other => panic!("{other:?}"),
        }).collect();
        let digest = sha256d(&[b"chainv4-btc-propose".as_slice(), &chain_id.to_le_bytes(), &gateway.to_abi(), &number.to_le_bytes(), &sha256d(raw), &(ids.len() as u64).to_le_bytes(), &named].concat());
        let (signature, parity) = crate::crypto::secp256k1::sign_recoverable(&[secret; 32], &digest);

        [&signature[..], &[27 + parity]].concat()
    }

    fn wbtc_of(&self, holder: Address) -> u256 {
        match self.world.call(self.wbtc, "balance_of", &[address(holder)]).result.unwrap()[..] {
            [TxCallArgumentType::U256(held)] => held,
            ref other => panic!("{other:?}"),
        }
    }
}

#[cfg(feature = "bridge")]
impl Btc {
    /// What pays an address's deposit script, under the federation in force.
    fn deposit_output(&self, recipient: Address) -> Vec<u8> {
        match &self.call("deposit_script", &[address(recipient)]).unwrap()[..] {
            [_, TxCallArgumentType::Bytes(output), _] => output.clone(),
            other => panic!("{other:?}"),
        }
    }

    /// Satoshis paid to Alice's deposit address, buried, shown and minted: its transaction's id.
    fn deposited(&mut self, sats: u64) -> [u8; 32] {
        let deposit = pays(&self.deposit_output(ALICE), sats);
        let (block, index, path, first, first_path) = self.mine(&deposit);
        self.bury();
        self.call("deposit", &[bytes(&block), bytes(&deposit), number(index), bytes(&path), bytes(&first), bytes(&first_path), number(0), address(ALICE), TxCallArgumentType::Nil]).unwrap();

        sha256d(&deposit)
    }

    /// Alice burns wBTC for a withdrawal to Bob: its id.
    fn withdrawal(&self, sats: u64) -> u64 {
        match self.call_as(ALICE, "withdraw", &[bytes(&bob()), number(sats)]).unwrap()[..] {
            [TxCallArgumentType::I64(id)] => id as u64,
            ref other => panic!("{other:?}"),
        }
    }

    /// The input of a release from what was first deposited.
    fn gateway_input(&self) -> [u8; 32] {
        match &self.call("utxo", &[number(0)]).unwrap()[..] {
            [TxCallArgumentType::String(outpoint), ..] => crate::structs::hex::from_hex(&format!("0x{}", outpoint.split_once(':').unwrap().0)).unwrap().try_into().unwrap(),
            other => panic!("{other:?}"),
        }
    }

    /// The output paying the federation in force's change.
    fn change(&self, sats: u64) -> TxOut {
        match &self.call("change_script", &[]).unwrap()[..] {
            [TxCallArgumentType::Bytes(script)] => TxOut { value: sats, script: crate::crypto::bitcoin::tx::p2wsh(script) },
            other => panic!("{other:?}"),
        }
    }

    /// A signer's signature over one input of a release, as BIP143 has it.
    fn signature(&self, secret: u8, tx: &Transaction, index: usize) -> [u8; 64] {
        let input = &tx.inputs[index];
        let held = self.call("utxo_of", &[TxCallArgumentType::String(format!("{}:{}", crate::structs::hex::to_hex(&input.txid).trim_start_matches("0x"), input.vout))]).unwrap();
        let [TxCallArgumentType::I64(sats), _, _, _, TxCallArgumentType::Bytes(script), _] = &held[..] else { panic!("{held:?}") };

        crate::crypto::secp256k1::sign_hash(&[secret; 32], &sha256d(&tx.script_preimage(index, script, *sats as u64)))
    }

    fn sign(&self, proposal: u64, tx: &Transaction, index: usize, secret: u8) -> Result<Vec<TxCallArgumentType>, String> {
        self.call("sign", &[number(proposal), number(index as u64), bytes(&self.signature(secret, tx, index))])
    }

    /// A signer's vote to cancel a release, on this chain.
    fn vote(&self, proposal: u64, tx: &Transaction, secret: u8) -> Result<Vec<TxCallArgumentType>, String> {
        let digest = crate::bridge::btc_cancel_digest(self.world.vm.chain_id(), &self.gateway, proposal, &sha256d(&tx.base()));

        self.call("cancel", &[number(proposal), bytes(&crate::bridge::sign_digest(&[secret; 32], &digest))])
    }

    fn state_of_proposal(&self, proposal: u64) -> TxCallArgumentType {
        self.call("proposal", &[number(proposal)]).unwrap()[4].clone()
    }

    fn state_of_withdrawal(&self, withdrawal: u64) -> TxCallArgumentType {
        self.call("withdrawal", &[number(withdrawal)]).unwrap()[2].clone()
    }

    fn state_of_output(&self, n: u64) -> TxCallArgumentType {
        self.call("utxo", &[number(n)]).unwrap()[4].clone()
    }
}

#[cfg(feature = "bridge")]
fn bob() -> Vec<u8> {
    crate::crypto::bitcoin::tx::p2wpkh(&[0xb0; 20])
}

#[cfg(feature = "bridge")]
fn text(value: &str) -> TxCallArgumentType {
    TxCallArgumentType::String(value.to_string())
}

#[cfg(feature = "bridge")]
/// A release as the gateway wants one, from these outputs it holds.
fn spending(inputs: &[[u8; 32]], outputs: Vec<TxOut>) -> Transaction {
    Transaction {
        version: 2,
        inputs: inputs.iter().map(|txid| TxIn { txid: *txid, vout: 0, script_sig: Vec::new(), sequence: 0xffff_ffff, witness: Vec::new() }).collect(),
        outputs,
        lock_time: 0,
    }
}

#[cfg(feature = "bridge")]
fn pays_bob(sats: u64) -> TxOut {
    TxOut { value: sats, script: bob() }
}

fn pays(script: &[u8], sats: u64) -> Vec<u8> {
    Transaction {
        version: 2,
        inputs: vec![TxIn { txid: [0x5e; 32], vout: 3, script_sig: Vec::new(), sequence: 0xffff_ffff, witness: Vec::new() }],
        outputs: vec![TxOut { value: sats, script: script.to_vec() }],
        lock_time: 0,
    }
    .base()
}

#[test]
fn a_deposit_mints_wbtc_and_a_withdrawal_is_released_as_the_federation_signs() {
    let mut btc = Btc::new();
    let alice = Address::Ed25519([0xa; 32]);
    assert_eq!(btc.call("relay", &[]).unwrap(), [address(btc.relay)]);
    let scripts = btc.call("deposit_script", &[address(alice)]).unwrap();
    let [TxCallArgumentType::Bytes(witness), TxCallArgumentType::Bytes(output), TxCallArgumentType::I64(0)] = &scripts[..] else { panic!("{scripts:?}") };

    // The deposit script: Alice's 33 bytes, dropped, this chain and gateway, dropped, then two of
    // three; as wallets compute it without asking a node.
    assert_eq!((witness[0], &witness[1..34], witness[34]), (33, alice.to_abi().as_slice(), 0x75));
    assert_eq!((witness[35], &witness[36..44], &witness[44..77], witness[77], witness[78]), (41, HERE.to_le_bytes().as_slice(), btc.gateway.to_abi().as_slice(), 0x75, 0x52));
    assert_eq!(output[..2], [0, 32]);
    #[cfg(feature = "bridge")]
    {
        let fed = federation(&[21, 22, 23], 2);
        assert_eq!(witness, &crate::bridge::btc_deposit_script(&alice, HERE, &btc.gateway, fed.threshold, &fed.signers));
    }
    assert_eq!(output, &crate::crypto::bitcoin::tx::p2wsh(witness));

    // Paid on Bitcoin, mined, but one block deep: not yet.
    let deposit = pays(output, 50_000);
    let (block, index, path, first, first_path) = btc.mine(&deposit);
    let show = |btc: &Btc, recipient: Address| btc.call("deposit", &[bytes(&block), bytes(&deposit), number(index), bytes(&path), bytes(&first), bytes(&first_path), number(0), address(recipient), TxCallArgumentType::Nil]);

    assert!(show(&btc, alice).unwrap_err().contains("not deep enough"));
    btc.bury();

    // Two deep, it mints, once, to whom it was for.
    assert!(show(&btc, Address::Ed25519([0xb; 32])).unwrap_err().contains("does not pay that address"));
    show(&btc, alice).unwrap();

    assert_eq!(btc.wbtc_of(alice), u256::from(50_000u64));
    assert!(show(&btc, alice).unwrap_err().contains("minted already"));
    assert_eq!(btc.world.call(btc.wbtc, "symbol", &[]).result.unwrap(), [TxCallArgumentType::String("wBTC".into())]);

    // Alice withdraws 20,000 to Bob's Bitcoin address: burned, queued.
    let bob = crate::crypto::bitcoin::tx::p2wpkh(&[0xb0; 20]);
    assert!(btc.call_as(alice, "withdraw", &[bytes(&bob), number(9_999)]).unwrap_err().contains("at least"));
    assert_eq!(btc.call_as(alice, "withdraw", &[bytes(&bob), number(20_000)]).unwrap(), [TxCallArgumentType::I64(1)]);
    assert_eq!(
        btc.call("withdrawal", &[number(1)]).unwrap(),
        [bytes(&bob), TxCallArgumentType::I64(20_000), TxCallArgumentType::String("queued".into()), address(alice), TxCallArgumentType::I64(2_000), TxCallArgumentType::Nil],
        "and who asked, for wallets, and the most it may lose, a tenth unless told"
    );
    assert_eq!(btc.wbtc_of(alice), u256::from(30_000u64));

    // A release spends the deposit: Bob's 20,000 less the 1,000 in fees, which come off what he is
    // paid, and all the rest back to the federation.
    let change = match &btc.call("change_script", &[]).unwrap()[..] {
        [TxCallArgumentType::Bytes(script)] => crate::crypto::bitcoin::tx::p2wsh(script),
        other => panic!("{other:?}"),
    };
    let release = |bob_sats: u64, change_sats: u64, from: [u8; 32]| Transaction {
        version: 2,
        inputs: vec![TxIn { txid: from, vout: 0, script_sig: Vec::new(), sequence: 0xffff_ffff, witness: Vec::new() }],
        outputs: vec![TxOut { value: bob_sats, script: bob.clone() }, TxOut { value: change_sats, script: change.clone() }],
        lock_time: 0,
    };
    let deposit_id = sha256d(&deposit);
    let propose = |btc: &Btc, tx: &Transaction| btc.propose(&tx.base(), &[number(1)]);

    assert!(propose(&btc, &release(20_000, 29_000, deposit_id)).unwrap_err().contains("does not pay its withdrawal"), "the pool paying the fee");
    assert!(propose(&btc, &release(18_500, 30_500, deposit_id)).unwrap_err().contains("does not pay its withdrawal"), "Bob paying the pool");
    assert!(propose(&btc, &release(9_000, 30_000, deposit_id)).unwrap_err().contains("past the cap"));
    assert!(propose(&btc, &release(19_000, 30_000, [1; 32])).unwrap_err().contains("not an output the gateway holds"));
    let good = release(19_000, 30_000, deposit_id);

    assert_eq!(propose(&btc, &good).unwrap(), [TxCallArgumentType::I64(1)]);
    assert!(propose(&btc, &good).unwrap_err().contains("not an output the gateway holds unspent"), "its input is pending");

    // Signers sign the input as BIP143 has it; a stranger's signature, or one over another
    // amount, is refused.
    let sighash = |secret: u8, value: u64| crate::crypto::secp256k1::sign_hash(&[secret; 32], &sha256d(&good.script_preimage(0, witness, value)));

    assert!(btc.call("sign", &[number(1), number(0), bytes(&sighash(99, 50_000))]).unwrap_err().contains("not a signature"));
    assert!(btc.call("sign", &[number(1), number(0), bytes(&sighash(21, 49_999))]).unwrap_err().contains("not a signature"));
    btc.call("sign", &[number(1), number(0), bytes(&sighash(23, 50_000))]).unwrap();
    btc.call("sign", &[number(1), number(0), bytes(&sighash(21, 50_000))]).unwrap();

    assert_eq!(
        btc.call("input_signatures", &[number(1), number(0)]).unwrap(),
        [bytes(witness), TxCallArgumentType::I64(2), TxCallArgumentType::I64(0), bytes(&sighash(21, 50_000)), TxCallArgumentType::I64(2), bytes(&sighash(23, 50_000))],
        "in the keys' order"
    );

    // Mined and buried: released, the withdrawal paid, the change held.
    let (block, index, path, first, first_path) = btc.mine(&good.base());
    btc.bury();
    btc.call("released", &[number(1), bytes(&block), bytes(&good.base()), number(index), bytes(&path), bytes(&first), bytes(&first_path)]).unwrap();

    assert_eq!(btc.call("withdrawal", &[number(1)]).unwrap()[2], TxCallArgumentType::String("paid".into()));
    assert_eq!(btc.call("utxo_count", &[]).unwrap(), [TxCallArgumentType::I64(2)]);
    assert_eq!(btc.call("utxo", &[number(0)]).unwrap()[4], TxCallArgumentType::String("spent".into()));
    assert_eq!(btc.call("utxo", &[number(1)]).unwrap()[1], TxCallArgumentType::I64(30_000), "all but what Bob burned: the wBTC left, 30,000, is backed");
    assert_eq!(btc.call("withdrawal", &[number(1)]).unwrap()[5], TxCallArgumentType::I64(1_000), "what it lost, for wallets");

    // A new federation: the old one's change is swept to it, all of it; the new one's is not.
    let next = DecidedFederation { version: 1, federation: federation(&[31, 32], 2) };
    btc.world.state.put_value(Column::Validators, b"change:federation".to_vec(), &next);
    let new_change = match &btc.call("change_script", &[]).unwrap()[..] {
        [TxCallArgumentType::Bytes(script)] => crate::crypto::bitcoin::tx::p2wsh(script),
        other => panic!("{other:?}"),
    };
    let sweep = Transaction {
        version: 2,
        inputs: vec![TxIn { txid: sha256d(&good.base()), vout: 1, script_sig: Vec::new(), sequence: 0xffff_ffff, witness: Vec::new() }],
        outputs: vec![TxOut { value: 29_500, script: new_change }],
        lock_time: 0,
    };

    assert_eq!(btc.propose_as(31, &sweep.base(), &[]).unwrap(), [TxCallArgumentType::I64(2)]);

    // Deposits to the old federation's addresses still mint within the grace period.
    let old_output = match &btc.call("deposit_script", &[address(alice)]).unwrap()[..] {
        [_, TxCallArgumentType::Bytes(output), TxCallArgumentType::I64(1)] => output.clone(),
        other => panic!("{other:?}"),
    };

    assert_ne!(&old_output, output, "a new federation, new deposit addresses");
    let late = pays(output, 7_000);
    let (block, index, path, first, first_path) = btc.mine(&late);
    btc.bury();
    btc.call("deposit", &[bytes(&block), bytes(&late), number(index), bytes(&path), bytes(&first), bytes(&first_path), number(0), address(alice), number(0)]).unwrap();

    assert_eq!(btc.wbtc_of(alice), u256::from(37_000u64));
}

/// A release to propose, and the gateway's answer to everyone who may not propose it.
#[cfg(feature = "bridge")]
fn a_release_to_propose() -> (Btc, Transaction, Vec<TxCallArgumentType>) {
    let mut btc = Btc::new();
    let deposit = btc.deposited(50_000);
    let id = btc.withdrawal(20_000);
    let tx = spending(&[deposit], vec![pays_bob(19_000), btc.change(30_000)]);

    (btc, tx, vec![number(id)])
}

#[cfg(feature = "bridge")]
#[test]
fn a_stranger_cannot_propose_a_release_whoever_posts_it_and_a_signer_can() {
    let (btc, tx, ids) = a_release_to_propose();
    let raw = tx.base();
    let refused = |secret: u8| btc.propose_as(secret, &raw, &ids).unwrap_err();

    assert!(refused(99).contains("only a signer"), "{}", refused(99));
    assert!(btc.call("propose", &[&[bytes(&raw), bytes(&[0; 65])], &ids[..]].concat()).unwrap_err().contains("only a signer"), "no signature that recovers");
    assert_eq!(btc.state_of_withdrawal(1), text("queued"));

    // Anyone may post what a signer signed: the gas is theirs, the word the signer's.
    let signature = btc.propose_signature(22, 1, &raw, &ids);
    assert_eq!(btc.call_as(ALICE, "propose", &[&[bytes(&raw), bytes(&signature)], &ids[..]].concat()).unwrap(), [TxCallArgumentType::I64(1)]);
    assert_eq!(btc.state_of_withdrawal(1), text("proposed"));
}

#[cfg(feature = "bridge")]
#[test]
fn a_signature_to_propose_says_nothing_of_any_other_release_chain_gateway_or_message() {
    let (btc, tx, ids) = a_release_to_propose();
    let raw = tx.base();
    let post = |signature: &[u8], raw: &[u8], ids: &[TxCallArgumentType]| btc.call("propose", &[&[bytes(raw), bytes(signature)], ids].concat());
    let good = btc.propose_signature(21, 1, &raw, &ids);
    let refused = |signature: &[u8], raw: &[u8], ids: &[TxCallArgumentType]| assert!(post(signature, raw, ids).unwrap_err().contains("only a signer"));

    // Another transaction: a higher fee, what a griefer would want.
    let dearer = spending(&[btc.gateway_input()], vec![pays_bob(18_000), btc.change(30_000)]).base();
    refused(&good, &dearer, &ids);
    // Other withdrawals, none, or more of them.
    refused(&good, &raw, &[]);
    refused(&good, &raw, &[number(1), number(2)]);
    // Another number, another chain, another gateway.
    refused(&btc.propose_signature(21, 2, &raw, &ids), &raw, &ids);
    refused(&btc.signature_for(21, HERE + 1, btc.gateway, 1, &raw, &ids), &raw, &ids);
    refused(&btc.signature_for(21, HERE, btc.relay, 1, &raw, &ids), &raw, &ids);

    // What the federation signs for anything else: a vote to cancel, over the same transaction.
    let vote = crate::bridge::sign_digest(&[21; 32], &crate::bridge::btc_cancel_digest(HERE, &btc.gateway, 1, &sha256d(&raw)));
    refused(&vote, &raw, &ids);

    // Nor does what proposes it pass for a vote to cancel it, once it is open.
    assert_eq!(post(&good, &raw, &ids).unwrap(), [TxCallArgumentType::I64(1)]);
    assert!(btc.call("cancel", &[number(1), bytes(&good)]).unwrap_err().contains("not signed by a signer"));

    // The tools sign what the gateway checks.
    let numbers = [1];
    let tools = crate::bridge::sign_digest(&[21; 32], &crate::bridge::btc_propose_digest(HERE, &btc.gateway, 1, &raw, &numbers));
    assert_eq!(tools.as_slice(), good.as_slice());
}

#[cfg(feature = "bridge")]
#[test]
fn what_proposed_a_release_does_not_bring_it_back_once_cancelled_and_signing_it_again_does() {
    let (btc, tx, ids) = a_release_to_propose();
    let raw = tx.base();
    let first = btc.propose_signature(21, 1, &raw, &ids);
    let post = |signature: &[u8]| btc.call("propose", &[&[bytes(&raw), bytes(signature)], &ids[..]].concat());

    post(&first).unwrap();
    btc.vote(1, &tx, 21).unwrap();
    btc.vote(1, &tx, 22).unwrap();
    assert_eq!(btc.state_of_withdrawal(1), text("queued"));

    // Anyone holding the old signature, public since it was posted, gets nowhere: the federation
    // cancelled it, and the griefer would have it hold all it did.
    assert!(post(&first).unwrap_err().contains("only a signer"));
    assert_eq!(btc.state_of_withdrawal(1), text("queued"));

    // A signer's fresh word brings it back.
    assert_eq!(post(&btc.propose_signature(21, 2, &raw, &ids)).unwrap(), [TxCallArgumentType::I64(2)]);
}

#[cfg(feature = "bridge")]
#[test]
fn only_a_signer_of_the_federation_in_force_proposes_a_release_or_a_sweep() {
    let (btc, _, _) = a_release_to_propose();
    let deposit = btc.gateway_input();
    let next = DecidedFederation { version: 1, federation: federation(&[31, 32], 2) };
    btc.world.state.put_value(Column::Validators, b"change:federation".to_vec(), &next);
    let sweep = spending(&[deposit], vec![btc.change(49_000)]).base();

    assert!(btc.propose_as(21, &sweep, &[]).unwrap_err().contains("only a signer"), "of the federation before");
    assert!(btc.propose_as(99, &sweep, &[]).unwrap_err().contains("only a signer"), "of none");
    assert_eq!(btc.propose_as(31, &sweep, &[]).unwrap(), [TxCallArgumentType::I64(1)]);
}

#[cfg(feature = "bridge")]
#[test]
fn a_release_naming_one_withdrawal_twice_is_refused() {
    let mut btc = Btc::new();
    let deposit = btc.deposited(100_000);
    btc.deposited(100_001);
    let id = btc.withdrawal(20_000);
    // It would pay Bob twice for the one burn.
    let tx = spending(&[deposit], vec![pays_bob(19_500), pays_bob(19_500), btc.change(60_000)]);

    assert!(btc.propose(&tx.base(), &[number(id), number(id)]).unwrap_err().contains("named twice"));
    assert_eq!(btc.state_of_withdrawal(id), text("queued"));

    let once = spending(&[deposit], vec![pays_bob(19_000), btc.change(80_000)]);
    btc.propose(&once.base(), &[number(id)]).unwrap();
}

#[cfg(feature = "bridge")]
#[test]
fn a_release_bitcoin_would_refuse_or_delay_is_refused() {
    let mut btc = Btc::new();
    let deposit = btc.deposited(100_000);
    let id = btc.withdrawal(20_000);
    let good = spending(&[deposit], vec![pays_bob(19_000), btc.change(80_000)]);
    let propose = |tx: &[u8]| btc.propose(tx, &[number(id)]);

    // The same output twice: no block can hold it, and it would hold what it spends for good.
    let mut twice = good.clone();
    twice.inputs.push(twice.inputs[0].clone());
    assert!(propose(&twice.base()).unwrap_err().contains("spent twice"));

    // A BIP68 delay, or a version that takes one differently.
    let mut delayed = good.clone();
    delayed.inputs[0].sequence = 144;
    assert!(propose(&delayed.base()).unwrap_err().contains("sequence"));
    let mut opted = good.clone();
    opted.inputs[0].sequence = 0xffff_fffd;
    assert!(propose(&opted.base()).unwrap_err().contains("sequence"));
    assert!(propose(&Transaction { version: 1, ..good.clone() }.base()).unwrap_err().contains("version 2"));
    assert!(propose(&Transaction { version: 3, ..good.clone() }.base()).unwrap_err().contains("version 2"));
    assert!(propose(&Transaction { lock_time: 900_000, ..good.clone() }.base()).unwrap_err().contains("lock time"));

    // A count written longer than it needs: Bitcoin's nodes refuse that, and rewritten it would be
    // another transaction, whose id the gateway would never take as this release's.
    let base = good.base();
    let long = [&base[..4], &[0xfd, 1, 0], &base[5..]].concat();
    assert!(propose(&long).unwrap_err().contains("longer than it needs"));

    // Change under Bitcoin's dust is not relayed.
    for (over, took) in [(545, false), (546, true)] {
        let mut near = Btc::new();
        let small = near.deposited(20_000 + over);
        let id = near.withdrawal(20_000);
        let tx = spending(&[small], vec![pays_bob(19_000), near.change(over)]);
        let answer = near.propose(&tx.base(), &[number(id)]);

        assert_eq!(answer.is_ok(), took, "{answer:?}");
    }

    // Nor is a transaction of 64 bytes or under: one input, and one output of a 4-byte script.
    let mut tiny = Btc::new();
    let exact = tiny.deposited(20_000);
    let id = tiny.call_as(ALICE, "withdraw", &[bytes(&[0x52, 2, 1, 1]), number(20_000)]).unwrap();
    let tx = spending(&[exact], vec![TxOut { value: 19_000, script: vec![0x52, 2, 1, 1] }]);
    assert_eq!(tx.base().len(), 64);
    assert!(tiny.propose(&tx.base(), &[id[0].clone()]).unwrap_err().contains("do not relay"));

    propose(&good.base()).unwrap();
}

#[cfg(feature = "bridge")]
#[test]
fn a_withdrawal_goes_only_to_a_script_bitcoin_s_nodes_relay() {
    let mut btc = Btc::new();
    btc.deposited(100_000);
    let withdraw = |script: &[u8]| btc.call_as(ALICE, "withdraw", &[bytes(script), number(10_000)]);

    for standard in [
        [&[0x76, 0xa9, 20][..], &[1; 20], &[0x88, 0xac]].concat(),
        [&[0xa9, 20][..], &[1; 20], &[0x87]].concat(),
        [&[0, 20][..], &[1; 20]].concat(),
        [&[0, 32][..], &[1; 32]].concat(),
        [&[0x51, 32][..], &[1; 32]].concat(),
        [&[0x52, 2][..], &[1; 2]].concat(),
    ] {
        withdraw(&standard).unwrap();
    }

    for refused in [Vec::new(), vec![0x51], vec![0x6a, 1, 1], [&[0, 21][..], &[1; 21]].concat(), [&[0x51, 41][..], &[1; 41]].concat(), vec![0xff; 34]] {
        assert!(withdraw(&refused).unwrap_err().contains("relay"), "{refused:?}");
    }
}

#[cfg(feature = "bridge")]
#[test]
fn a_release_pays_at_least_the_floor_for_its_size_and_at_most_the_cap() {
    let mut btc = Btc::new();
    let deposit = btc.deposited(100_000);
    let id = btc.withdrawal(20_000);
    let release = |fee: u64| spending(&[deposit], vec![pays_bob(20_000 - fee), btc.change(80_000)]);
    let script = match &btc.call("utxo", &[number(0)]).unwrap()[..] {
        [TxCallArgumentType::String(key), ..] => match &btc.call("utxo_of", &[text(key)]).unwrap()[..] {
            [_, _, _, _, TxCallArgumentType::Bytes(script), _] => script.len(),
            other => panic!("{other:?}"),
        },
        other => panic!("{other:?}"),
    };
    // The floor as the signers' tools reckon it is the gateway's: one satoshi under it is refused,
    // and it is enough.
    let least = crate::bridge::btc_release_vbytes(release(1_000).base().len(), &[(2, script)]) * FLOOR;
    let answer = btc.propose(&release(least - 1).base(), &[number(id)]).unwrap_err();

    assert!(answer.contains("under the floor") && answer.contains(&least.to_string()), "{answer}");
    assert!(btc.propose(&release(CAP + 1).base(), &[number(id)]).unwrap_err().contains("past the cap"));
    btc.propose(&release(least).base(), &[number(id)]).unwrap();

    assert_eq!(btc.call("fee_limits", &[]).unwrap(), [TxCallArgumentType::I64(CAP as i64), TxCallArgumentType::I64(FLOOR as i64)]);
}

#[cfg(feature = "bridge")]
#[test]
fn a_fee_cap_and_floor_that_would_allow_no_release_are_refused() {
    let btc = Btc::started(HERE);
    let setup = |btc: &Btc, cap: u64, floor: u64| btc.call("setup", &[address(btc.relay), address(btc.template), number(cap), number(floor), number(3_600_000)]);

    assert!(setup(&btc, 50_000, 0).unwrap_err().contains("at least 1"));
    assert!(setup(&btc, 1_419, 2).unwrap_err().contains("would allow none"));
    assert_eq!(btc.call("relay", &[]).map_err(|why| why.contains("no relay")), Err(true), "nothing was set");

    // The room asked for covers the largest release of one input there can be: 20 of 20, a
    // witness program of 40 bytes, and change.
    let fed = federation(&(1..=20).collect::<Vec<u8>>(), 20);
    let script = crate::bridge::btc_deposit_script(&ALICE, HERE, &btc.gateway, 20, &fed.signers);
    let change = crate::crypto::bitcoin::tx::p2wsh(&crate::bridge::btc_multisig(20, &fed.signers));
    let largest = spending(&[[1; 32]], vec![TxOut { value: 1_000, script: [&[0x51, 40][..], &[1; 40]].concat() }, TxOut { value: 1_000, script: change }]);

    assert!(crate::bridge::btc_release_vbytes(largest.base().len(), &[(20, script.len())]) <= 710);

    setup(&btc, 1_420, 2).unwrap();
    assert!(setup(&btc, 1_420, 2).unwrap_err().contains("set up already"));
}

#[cfg(feature = "bridge")]
#[test]
fn a_release_short_of_its_threshold_is_cancelled_by_the_federation_and_proposed_again() {
    let mut btc = Btc::new();
    let (first, second) = (btc.deposited(50_000), btc.deposited(60_000));
    let id = btc.withdrawal(20_000);
    let release = spending(&[first], vec![pays_bob(19_000), btc.change(30_000)]);
    btc.propose(&release.base(), &[number(id)]).unwrap();
    btc.sign(1, &release, 0, 21).unwrap();

    // Only the federation in force votes, each once; one vote changes nothing.
    assert!(btc.vote(1, &release, 99).unwrap_err().contains("not signed by a signer"));
    assert_eq!(btc.vote(1, &release, 21).unwrap(), [TxCallArgumentType::Bool(false)]);
    assert_eq!(btc.vote(1, &release, 21).unwrap(), [TxCallArgumentType::Bool(false)]);
    assert_eq!(btc.state_of_proposal(1), text("open"));
    assert_eq!(btc.call("voted", &[number(1), bytes(&crate::crypto::secp256k1::public_key(&[21; 32]).unwrap())]).unwrap(), [TxCallArgumentType::Bool(true)]);

    // A vote for this release on another chain or gateway says nothing here.
    let elsewhere = crate::bridge::btc_cancel_digest(HERE + 1, &btc.gateway, 1, &sha256d(&release.base()));
    assert!(btc.call("cancel", &[number(1), bytes(&crate::bridge::sign_digest(&[22; 32], &elsewhere))]).unwrap_err().contains("not signed by a signer"));

    // Two of three, and it is cancelled: what it spent and paid goes back.
    assert_eq!(btc.vote(1, &release, 22).unwrap(), [TxCallArgumentType::Bool(true)]);
    assert_eq!(btc.state_of_proposal(1), text("cancelled"));
    assert_eq!(btc.state_of_withdrawal(id), text("queued"));
    assert_eq!(btc.state_of_output(0), text("unspent"));
    assert!(btc.vote(1, &release, 23).unwrap_err().contains("no open release"));

    // What was signed for it counts for nothing, and nothing more is taken.
    assert_eq!(btc.call("input_signatures", &[number(1), number(0)]).unwrap().len(), 2, "the script and threshold, no signature");
    assert!(btc.sign(1, &release, 0, 22).unwrap_err().contains("no open release"));

    // Its withdrawal is paid again only by what conflicts with it on Bitcoin, as it may yet be
    // finished there with a signature no one posted: from other inputs alone would pay Bob twice.
    let elsewhere = spending(&[second], vec![pays_bob(19_000), btc.change(40_000)]);
    assert!(btc.propose(&elsewhere.base(), &[number(id)]).unwrap_err().contains("must spend"));
    let hex = crate::structs::hex::to_hex(&first);
    assert_eq!(btc.call("anchor", &[number(id)]).unwrap(), [text(&format!("{}:0", hex.trim_start_matches("0x")))]);

    let again = spending(&[second, first], vec![pays_bob(19_000), btc.change(90_000)]);
    assert_eq!(btc.propose(&again.base(), &[number(id)]).unwrap(), [TxCallArgumentType::I64(2)]);
    assert_eq!(btc.call("input_signatures", &[number(2), number(1)]).unwrap().len(), 2, "signed anew");

    // Paid at last, by the second, the anchor is spent; the first can no longer be mined.
    btc.sign(2, &again, 0, 21).unwrap();
    btc.sign(2, &again, 0, 22).unwrap();
    btc.sign(2, &again, 1, 21).unwrap();
    btc.sign(2, &again, 1, 22).unwrap();
    let (block, index, path, first, first_path) = btc.mine(&again.base());
    btc.bury();
    btc.call("released", &[number(2), bytes(&block), bytes(&again.base()), number(index), bytes(&path), bytes(&first), bytes(&first_path)]).unwrap();

    assert_eq!(btc.state_of_withdrawal(id), text("paid"));
    assert_eq!(btc.call("anchor", &[number(id)]).unwrap(), [TxCallArgumentType::Nil]);
}

#[cfg(feature = "bridge")]
#[test]
fn a_release_with_every_input_at_its_threshold_cannot_be_cancelled() {
    let mut btc = Btc::new();
    let (first, second) = (btc.deposited(50_000), btc.deposited(60_000));
    let id = btc.withdrawal(20_000);
    let release = spending(&[first, second], vec![pays_bob(19_000), btc.change(90_000)]);
    btc.propose(&release.base(), &[number(id)]).unwrap();

    // One input there and one not: it cannot be finished yet, so a vote is taken.
    btc.sign(1, &release, 0, 21).unwrap();
    btc.sign(1, &release, 0, 22).unwrap();
    btc.sign(1, &release, 1, 21).unwrap();
    assert_eq!(btc.vote(1, &release, 21).unwrap(), [TxCallArgumentType::Bool(false)]);

    // Both there: anyone can finish it, so no more vote stands, even with a vote already in.
    btc.sign(1, &release, 1, 23).unwrap();
    assert!(btc.vote(1, &release, 22).unwrap_err().contains("anyone can still finish it"));
    assert!(btc.vote(1, &release, 21).unwrap_err().contains("anyone can still finish it"));
    assert_eq!(btc.state_of_proposal(1), text("open"));
    assert_eq!(btc.state_of_withdrawal(id), text("proposed"));
}

#[cfg(feature = "bridge")]
#[test]
fn a_cancelled_release_finished_anyway_pays_no_one_twice_and_frees_what_waited_on_it() {
    let mut btc = Btc::new();
    let (first, second, third) = (btc.deposited(50_000), btc.deposited(60_000), btc.deposited(70_000));
    let (paid, other) = (btc.withdrawal(20_000), btc.withdrawal(10_000));
    let cancelled = spending(&[first], vec![pays_bob(19_500), TxOut { value: 9_500, script: bob() }, btc.change(20_000)]);
    btc.propose(&cancelled.base(), &[number(paid), number(other)]).unwrap();
    btc.vote(1, &cancelled, 21).unwrap();
    btc.vote(1, &cancelled, 22).unwrap();

    // Proposed again, as it must be, on the same input, and signed in full: it conflicts with the
    // first, so Bitcoin takes one of them.
    let again = spending(&[first, third], vec![pays_bob(19_000), btc.change(100_000)]);
    btc.propose(&again.base(), &[number(paid)]).unwrap();

    for (input, signer) in [(0, 21), (0, 22), (1, 21), (1, 22)] {
        btc.sign(2, &again, input, signer).unwrap();
    }

    // The first was finished with signatures that came too late for the board. Shown, the gateway
    // knows: its input is spent, what it paid is paid, and its change held.
    let (block, index, path, first, first_path) = btc.mine(&cancelled.base());
    btc.bury();
    btc.call("released", &[number(1), bytes(&block), bytes(&cancelled.base()), number(index), bytes(&path), bytes(&first), bytes(&first_path)]).unwrap();

    assert_eq!(btc.state_of_proposal(1), text("released"));
    assert_eq!(btc.state_of_withdrawal(paid), text("paid"));
    assert_eq!(btc.state_of_withdrawal(other), text("paid"));
    assert_eq!(btc.state_of_output(0), text("spent"));
    assert_eq!(btc.call("utxo_count", &[]).unwrap(), [TxCallArgumentType::I64(4)], "and only once");

    // The second can never be mined now, so though it holds every signature, it can be cancelled,
    // and what it held comes back but for what is spent and paid.
    btc.vote(2, &again, 21).unwrap();
    assert_eq!(btc.vote(2, &again, 22).unwrap(), [TxCallArgumentType::Bool(true)]);
    assert_eq!(btc.state_of_withdrawal(paid), text("paid"));
    assert_eq!(btc.state_of_output(0), text("spent"));
    assert_eq!(btc.state_of_output(2), text("unspent"));
    assert_eq!(btc.state_of_output(1), text("unspent"), "never in either");
    assert_eq!(btc.call("utxo", &[number(1)]).unwrap()[0], text(&format!("{}:0", crate::structs::hex::to_hex(&second).trim_start_matches("0x"))));

    // Showing a release again, cancelled or not, changes nothing.
    assert!(btc.call("released", &[number(1), bytes(&block), bytes(&cancelled.base()), number(index), bytes(&path), bytes(&first), bytes(&first_path)]).unwrap_err().contains("no open release"));
}

#[cfg(feature = "bridge")]
#[test]
fn a_cancelled_release_proposed_again_as_it_was_is_shown_once() {
    let mut btc = Btc::new();
    let first = btc.deposited(50_000);
    let id = btc.withdrawal(20_000);
    let release = spending(&[first], vec![pays_bob(19_000), btc.change(30_000)]);
    btc.propose(&release.base(), &[number(id)]).unwrap();
    btc.vote(1, &release, 21).unwrap();
    btc.vote(1, &release, 22).unwrap();
    btc.propose(&release.base(), &[number(id)]).unwrap();
    let (block, index, path, first, first_path) = btc.mine(&release.base());
    btc.bury();
    let show = |proposal: u64| btc.call("released", &[number(proposal), bytes(&block), bytes(&release.base()), number(index), bytes(&path), bytes(&first), bytes(&first_path)]);

    show(2).unwrap();
    assert!(show(1).unwrap_err().contains("shown spent already"));
    assert_eq!(btc.call("utxo_count", &[]).unwrap(), [TxCallArgumentType::I64(2)]);
    assert_eq!(btc.state_of_output(1), text("unspent"), "its change, held once");
}

#[cfg(feature = "bridge")]
#[test]
fn a_proof_of_a_deposit_mints_on_its_own_chain_and_on_no_other() {
    let mut here = Btc::on_chain(HERE);
    let mut elsewhere = Btc::on_chain(HERE + 1);

    // The same keys deploy the same addresses on both, so only the chain tells them apart.
    assert_eq!(here.gateway, elsewhere.gateway);
    assert_ne!(here.deposit_output(ALICE), elsewhere.deposit_output(ALICE));

    let deposit = pays(&here.deposit_output(ALICE), 50_000);
    let (block, index, path, first, first_path) = here.mine(&deposit);
    here.bury();
    assert_eq!(elsewhere.mine(&deposit), (block, index, path.clone(), first.clone(), first_path.clone()), "one Bitcoin");
    elsewhere.bury();
    let show = |btc: &Btc| btc.call("deposit", &[bytes(&block), bytes(&deposit), number(index), bytes(&path), bytes(&first), bytes(&first_path), number(0), address(ALICE), TxCallArgumentType::Nil]);

    assert!(show(&elsewhere).unwrap_err().contains("does not pay that address"));
    assert_eq!(elsewhere.wbtc_of(ALICE), u256::ZERO);
    show(&here).unwrap();
    assert_eq!(here.wbtc_of(ALICE), u256::from(50_000u64));
}

#[cfg(feature = "bridge")]
#[test]
fn a_chain_restarted_from_the_identical_genesis_does_not_mint_a_deposit_from_before_its_gateway() {
    let mut old = Btc::on_chain(HERE);
    let deposit = pays(&old.deposit_output(ALICE), 50_000);
    let (block, index, path, first, first_path) = old.mine(&deposit);
    old.bury();
    let show = |btc: &Btc| btc.call("deposit", &[bytes(&block), bytes(&deposit), number(index), bytes(&path), bytes(&first), bytes(&first_path), number(0), address(ALICE), TxCallArgumentType::Nil]);
    show(&old).unwrap();

    // The same id, the same gateway, the same deposit address, and nothing held: only that its
    // relay had already seen that block when its gateway was set up tells the proof is old.
    let mut new = Btc::started(HERE);
    assert_eq!(new.gateway, old.gateway);
    new.mine(&deposit);
    new.bury();
    new.set_up();

    assert_eq!(new.deposit_output(ALICE), old.deposit_output(ALICE));
    assert!(show(&new).unwrap_err().contains("before this gateway"));
    assert_eq!(new.wbtc_of(ALICE), u256::ZERO);

    // A payment since mints.
    let fresh = pays(&new.deposit_output(ALICE), 7_000);
    let (block, index, path, first, first_path) = new.mine(&fresh);
    new.bury();
    new.call("deposit", &[bytes(&block), bytes(&fresh), number(index), bytes(&path), bytes(&first), bytes(&first_path), number(0), address(ALICE), TxCallArgumentType::Nil]).unwrap();
    assert_eq!(new.wbtc_of(ALICE), u256::from(7_000u64));
}

#[cfg(feature = "bridge")]
#[test]
fn lua_and_rust_write_one_deposit_script_under_any_chain_gateway_and_federation() {
    for chain_id in [1, 255, 0x1234_5678_9abc, (1 << 48) - 1] {
        let btc = Btc::on_chain(chain_id);

        for recipient in [ALICE, Address::Contract([0xc; 32]), Address::Secp256k1([0xd; 20])] {
            let fed = federation(&[21, 22, 23], 2);
            let scripts = btc.call("deposit_script", &[address(recipient)]).unwrap();
            let script = crate::bridge::btc_deposit_script(&recipient, chain_id, &btc.gateway, fed.threshold, &fed.signers);

            assert_eq!(scripts[..2], [bytes(&script), bytes(&crate::crypto::bitcoin::tx::p2wsh(&script))], "chain {chain_id}");
        }
    }
}

#[cfg(feature = "bridge")]
#[test]
fn a_signer_goes_over_a_release_against_the_gateway_s_state() {
    use crate::bridge::{NamedWithdrawal, ReleaseFacts, SpentOutput, check_btc_release};

    let mut btc = Btc::new();
    let (first, second) = (btc.deposited(50_000), btc.deposited(60_000));
    let id = btc.withdrawal(20_000);
    let release = spending(&[first, second], vec![pays_bob(18_000), btc.change(90_000)]);
    btc.propose(&release.base(), &[number(id)]).unwrap();

    let held = |txid: &[u8; 32]| {
        let key = format!("{}:0", crate::structs::hex::to_hex(txid).trim_start_matches("0x"));
        let found = btc.call("utxo_of", &[text(&key)]).unwrap();
        let [TxCallArgumentType::I64(sats), TxCallArgumentType::I64(version), _, TxCallArgumentType::String(state), TxCallArgumentType::Bytes(script), TxCallArgumentType::I64(threshold)] = &found[..] else { panic!("{found:?}") };

        (key, SpentOutput { sats: *sats as u64, version: *version as u64, state: state.clone(), threshold: *threshold as u32, script_len: script.len() })
    };
    let [TxCallArgumentType::Bytes(change_script)] = &btc.call("change_script", &[number(0)]).unwrap()[..] else { panic!("no script") };
    let facts = ReleaseFacts {
        raw: release.base(),
        txid: sha256d(&release.base()).to_vec(),
        version: 0,
        withdrawals: vec![NamedWithdrawal { id, script: bob(), sats: 20_000, most_fee: 2_000, state: "proposed".to_string(), anchor: None }],
        inputs: vec![held(&first), held(&second)],
        change_script: change_script.clone(),
        fee_cap: CAP,
        fee_floor: FLOOR,
        most_rate: 1_000,
    };

    // The fee is 2,000 over what this release weighs: at the limit it is signed, a byte under it not.
    let vbytes = crate::bridge::btc_release_vbytes(facts.raw.len(), &[(facts.inputs[0].1.threshold, facts.inputs[0].1.script_len), (facts.inputs[1].1.threshold, facts.inputs[1].1.script_len)]);
    let exact = 2_000 / vbytes;
    assert!(check_btc_release(&ReleaseFacts { most_rate: exact + 1, ..facts.clone() }).is_ok());
    assert!(check_btc_release(&ReleaseFacts { most_rate: exact, ..facts.clone() }).unwrap_err().contains("past the"), "a release over the rate");

    assert_eq!(check_btc_release(&facts), Ok(()));

    let refused = |change: &dyn Fn(&mut ReleaseFacts), why: &str| {
        let mut changed = facts.clone();
        change(&mut changed);
        let answer = check_btc_release(&changed).unwrap_err();
        assert!(answer.contains(why), "{answer}");
    };
    let with_tx = |tx: Transaction| {
        move |facts: &mut ReleaseFacts| {
            facts.raw = tx.base();
            facts.txid = sha256d(&tx.base()).to_vec();
        }
    };

    // Each of what the gateway should have refused, should it ever have let one by.
    refused(&|facts| facts.withdrawals.push(facts.withdrawals[0].clone()), "twice");
    refused(&|facts| facts.withdrawals[0].state = "paid".to_string(), "is paid");
    refused(&|facts| facts.withdrawals[0].state = "queued".to_string(), "is queued");
    refused(&|facts| facts.withdrawals[0].sats = 19_000, "as asked");
    refused(&|facts| facts.withdrawals[0].most_fee = 1_999, "agreed to lose");
    refused(&|facts| facts.withdrawals[0].script = vec![0x51], "as asked");
    refused(&|facts| facts.withdrawals[0].anchor = Some("00:9".to_string()), "cancelled release");
    refused(&|facts| facts.inputs[1].1.state = "spent".to_string(), "is spent");
    refused(&|facts| facts.inputs[1].0 = facts.inputs[0].0.clone(), "twice");
    refused(&|facts| facts.inputs.truncate(1), "account of its inputs");
    refused(&|facts| facts.change_script = vec![0x51], "not change");
    refused(&|facts| facts.fee_cap = 1_999, "fee");
    refused(&|facts| facts.fee_floor = 1_000, "fee");
    refused(&|facts| facts.txid = vec![0; 32], "the id the gateway keeps");
    refused(&with_tx(Transaction { version: 3, ..release.clone() }), "version 2");
    refused(&with_tx(Transaction { lock_time: 1, ..release.clone() }), "version 2");
    let mut delayed = release.clone();
    delayed.inputs[0].sequence = 144;
    refused(&with_tx(delayed), "delay");
    let mut robbed = release.clone();
    robbed.outputs[1].script = bob();
    refused(&with_tx(robbed), "not change");
    let mut more = release.clone();
    more.outputs.push(pays_bob(1_000));
    refused(&with_tx(more), "more than");
    let mut dust = release.clone();
    dust.outputs[1].value = 10;
    refused(&with_tx(dust), "not change");
    // The fee taken from the pool rather than from the withdrawal.
    let mut pool_pays = release.clone();
    (pool_pays.outputs[0].value, pool_pays.outputs[1].value) = (20_000, 88_000);
    refused(&with_tx(pool_pays), "as asked");
    let mut tiny = facts.clone();
    tiny.raw = vec![2, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    tiny.txid = sha256d(&tiny.raw).to_vec();
    assert!(check_btc_release(&tiny).is_err());

    // A sweep is of older federations alone, and pays something.
    let mut sweep = facts.clone();
    sweep.withdrawals.clear();
    sweep.raw = spending(&[first, second], vec![btc.change(109_000)]).base();
    sweep.txid = sha256d(&sweep.raw).to_vec();
    assert!(check_btc_release(&sweep).unwrap_err().contains("older federation"));
    sweep.version = 1;
    assert_eq!(check_btc_release(&sweep), Ok(()), "its change to its federation, here the same keys");
    sweep.change_script = vec![0x51];
    assert!(check_btc_release(&sweep).unwrap_err().contains("not change"));
}

#[cfg(feature = "bridge")]
#[test]
fn a_fee_comes_off_the_withdrawals_evenly_within_what_each_agreed_to_lose_and_never_off_the_backing() {
    use crate::bridge::btc_fee_share;

    let mut btc = Btc::new();
    let (first, second) = (btc.deposited(100_000), btc.deposited(100_001));
    let (small, large) = (btc.withdrawal(10_000), btc.withdrawal(50_000));
    let (minted, burned) = (200_001u64, 60_000u64);

    // Even, the first taking what will not divide, whatever each is for: 1,001 over two.
    let release = |at_first: u64, at_second: u64| spending(&[first], vec![pays_bob(10_000 - at_first), TxOut { value: 50_000 - at_second, script: bob() }, btc.change(100_000 - burned)]).base();
    let propose = |raw: &[u8]| btc.propose(raw, &[number(small), number(large)]);

    assert_eq!((btc_fee_share(1_001, 2, 0), btc_fee_share(1_001, 2, 1)), (501, 500));
    assert!(propose(&release(500, 501)).unwrap_err().contains("does not pay its withdrawal"), "the rest on the second");
    assert!(propose(&release(1_001, 0)).unwrap_err().contains("does not pay its withdrawal"), "all on one");

    // Past what the small one agreed to lose, a thousand, it is refused, however the large one's
    // limit could take it.
    assert!(propose(&release(1_001, 1_001)).unwrap_err().contains("agreed to lose"));

    // It may say it loses more, as who asked, while it waits; no one else may.
    assert!(btc.call_as(Address::Ed25519([0xb; 32]), "set_most_fee", &[number(small), number(2_000)]).unwrap_err().contains("only who asked"));
    assert!(btc.call_as(ALICE, "set_most_fee", &[number(small), number(9_001)]).unwrap_err().contains("all but"), "not all of it");
    assert!(btc.call_as(ALICE, "set_most_fee", &[number(small), number(0)]).unwrap_err().contains("from 1"));
    btc.call_as(ALICE, "set_most_fee", &[number(small), number(1_001)]).unwrap();

    let taken = release(1_001, 1_001);
    propose(&taken).unwrap();
    assert!(btc.call_as(ALICE, "set_most_fee", &[number(small), number(2_000)]).unwrap_err().contains("not queued"));
    assert_eq!(btc.call("withdrawal", &[number(small)]).unwrap()[5], TxCallArgumentType::I64(1_001));

    // What the gateway holds falls by just what was burned, whatever the fee was: a full release
    // leaves all of it backed.
    let (tx, _) = Transaction::read(&taken).unwrap();
    let (block, index, path, first_tx, first_path) = btc.mine(&taken);
    btc.bury();
    btc.call("released", &[number(1), bytes(&block), bytes(&taken), number(index), bytes(&path), bytes(&first_tx), bytes(&first_path)]).unwrap();
    let change = tx.outputs[2].value;

    assert_eq!(100_001 + change, minted - burned, "held, unspent");
    assert_eq!(btc.wbtc_of(ALICE), u256::from(minted - burned), "what stands against it");
    let _ = second;

    // A withdrawal cannot lose it all, nor be too small to pay for a release of its own.
    assert!(btc.call_as(ALICE, "withdraw", &[bytes(&bob()), number(10_000), number(9_001)]).unwrap_err().contains("all but"));
    assert!(btc.call_as(ALICE, "withdraw", &[bytes(&bob()), number(10_000), number(0)]).unwrap_err().contains("from 1"));
    assert_eq!(btc.call("least_withdrawal", &[]).unwrap(), [TxCallArgumentType::I64(10_000)]);

    // A sweep's fee stays the federation's: no withdrawal to bear it.
    let next = DecidedFederation { version: 1, federation: federation(&[31, 32], 2) };
    btc.world.state.put_value(Column::Validators, b"change:federation".to_vec(), &next);
    let sweep = spending(&[second], vec![btc.change(99_001)]);
    btc.propose_as(31, &sweep.base(), &[]).unwrap();
}

/// A transaction of 64 bytes could be an inner node of a block's tree: not the relay's to tell apart
/// by its id, and nothing Bitcoin's nodes relay. The gateway has its bytes, and refuses it.
#[cfg(feature = "bridge")]
#[test]
fn a_deposit_of_64_bytes_is_refused_as_it_could_be_an_inner_node() {
    let mut btc = Btc::new();
    let raw = [7u8; 64];
    let (block, index, path, first, first_path) = btc.mine(&raw);
    btc.bury();
    let answer = btc.call("deposit", &[bytes(&block), bytes(&raw), number(index), bytes(&path), bytes(&first), bytes(&first_path), number(0), address(ALICE), TxCallArgumentType::Nil]);

    assert!(answer.unwrap_err().contains("64 bytes"));
}

#[cfg(feature = "bridge")]
#[test]
fn the_least_withdrawal_rises_with_a_floor_so_one_can_always_pay_for_its_own_release() {
    let btc = Btc::started(HERE);
    btc.call("setup", &[address(btc.relay), address(btc.template), number(1_000_000), number(100), number(3_600_000)]).unwrap();

    assert_eq!(btc.call("least_withdrawal", &[]).unwrap(), [TxCallArgumentType::I64(100 * 710 + 1_000)]);
}

#[cfg(feature = "bridge")]
#[test]
fn the_largest_release_of_withdrawals_fits_in_a_call_s_fuel_with_all_it_now_checks() {
    let mut btc = Btc::new();
    let deposits: Vec<[u8; 32]> = (0..10).map(|n| btc.deposited(100_000 + n)).collect();
    let ids: Vec<u64> = (0..50).map(|_| btc.withdrawal(10_000)).collect();
    let spent: u64 = (0..10).map(|n| 100_000 + n).sum();
    let fee = 9_000;
    let mut outputs: Vec<TxOut> = ids.iter().map(|_| pays_bob(10_000 - fee / 50)).collect();
    outputs.push(btc.change(spent - 500_000));
    let tx = spending(&deposits, outputs);
    let numbers: Vec<TxCallArgumentType> = ids.iter().map(|id| number(*id)).collect();
    let args = [&[bytes(&tx.base()), bytes(&btc.propose_signature(21, 1, &tx.base(), &numbers))], &numbers[..]].concat();
    let proposed = btc.world.call_at(DEPLOYER, btc.gateway, "propose", &args, 0, NOW);

    proposed.result.unwrap();
    assert!(proposed.fuel_used < 2_000_000, "{}", proposed.fuel_used);
    let voted = btc.vote(1, &tx, 21);
    voted.unwrap();
    let digest = crate::bridge::btc_cancel_digest(HERE, &btc.gateway, 1, &sha256d(&tx.base()));
    let cancelled = btc.world.call_at(DEPLOYER, btc.gateway, "cancel", &[number(1), bytes(&crate::bridge::sign_digest(&[22; 32], &digest))], 0, NOW);

    cancelled.result.unwrap();
    assert!(cancelled.fuel_used < 2_000_000, "{}", cancelled.fuel_used);
}

/// Bitcoin Core on a private regtest chain, stopped when dropped.
#[cfg(feature = "bridge")]
struct Regtest {
    dir: std::path::PathBuf,
    port: u16,
}

#[cfg(feature = "bridge")]
impl Regtest {
    fn start(port: u16) -> Regtest {
        let dir = std::env::temp_dir().join(format!("chainv4-btc-gateway-{}-{port}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let started = std::process::Command::new("bitcoind")
            .args(["-regtest", "-daemonwait", "-fallbackfee=0.0001", "-txindex=1", &format!("-rpcport={port}"), &format!("-port={}", port + 1)])
            .arg(format!("-datadir={}", dir.display()))
            .status()
            .expect("is Bitcoin Core installed?");
        assert!(started.success());

        Regtest { dir, port }
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
}

#[cfg(feature = "bridge")]
impl Drop for Regtest {
    fn drop(&mut self) {
        let _ = self.cli(&["stop"]);
    }
}

/// Only a real node says whether the script commits as Bitcoin runs it, whether what the gateway
/// lets through is what nodes relay, and whether the size the floor goes by is enough.
#[cfg(feature = "bridge")]
#[test]
#[ignore = "runs Bitcoin Core on a private regtest chain"]
fn bitcoin_core_takes_a_release_of_a_deposit_with_every_kind_of_output_the_gateway_allows() {
    use crate::crypto::bitcoin::tx::der;
    use crate::structs::hex::{from_hex, to_hex};

    let node = Regtest::start(18973);
    node.cli(&["createwallet", "test"]).unwrap();
    node.mine(101);

    // A payment by a real wallet to Alice's deposit address, as any wallet makes it.
    let mut btc = Btc::new();
    let output = btc.deposit_output(ALICE);
    let paid = node.cli(&["sendtoaddress", &crate::structs::bech32::encode_segwit("bcrt", &output[2..]), "0.001"]).unwrap();
    node.mine(1);
    let whole = node.cli(&["getrawtransaction", &paid]).unwrap();
    let (paying, _) = Transaction::read(&from_hex(&format!("0x{whole}")).unwrap()).unwrap();
    let vout = paying.outputs.iter().position(|out| out.script == output).unwrap() as u64;
    let deposit = paying.base();
    let (block, index, path, first, first_path) = btc.mine(&deposit);
    btc.bury();
    btc.call("deposit", &[bytes(&block), bytes(&deposit), number(index), bytes(&path), bytes(&first), bytes(&first_path), number(vout), address(ALICE), TxCallArgumentType::Nil]).unwrap();

    // Withdrawals to every kind of script the gateway takes, and change.
    let scripts = [
        [&[0x76, 0xa9, 20][..], &[1; 20], &[0x88, 0xac]].concat(),
        [&[0xa9, 20][..], &[1; 20], &[0x87]].concat(),
        [&[0, 20][..], &[1; 20]].concat(),
        [&[0, 32][..], &[1; 32]].concat(),
        [&[0x51, 32][..], &[1; 32]].concat(),
        [&[0x52, 2][..], &[1; 2]].concat(),
    ];
    let mut outputs = Vec::new();
    let mut args = vec![bytes(&[])];

    for script in &scripts {
        let id = btc.call_as(ALICE, "withdraw", &[bytes(script), number(10_000)]).unwrap();
        args.push(id[0].clone());
        outputs.push(TxOut { value: 10_000 - 200, script: script.clone() });
    }

    // 1,200 in fees, 200 from each of the six, and all the rest back.
    outputs.push(btc.change(100_000 - 60_000));
    let mut release = Transaction { version: 2, inputs: vec![TxIn { txid: sha256d(&deposit), vout: vout as u32, script_sig: Vec::new(), sequence: 0xffff_ffff, witness: Vec::new() }], outputs, lock_time: 0 };
    assert_eq!(btc.propose(&release.base(), &args[1..]).unwrap(), [TxCallArgumentType::I64(1)]);
    btc.sign(1, &release, 0, 21).unwrap();
    btc.sign(1, &release, 0, 23).unwrap();

    // Assembled from the board, as chainv4 bridge btc finish does.
    let posted = btc.call("input_signatures", &[number(1), number(0)]).unwrap();
    let [TxCallArgumentType::Bytes(script), TxCallArgumentType::I64(2), _, TxCallArgumentType::Bytes(first), _, TxCallArgumentType::Bytes(second)] = &posted[..] else { panic!("{posted:?}") };
    let signature = |bytes: &Vec<u8>| [der(&<[u8; 64]>::try_from(bytes.as_slice()).unwrap()), vec![0x01]].concat();
    release.inputs[0].witness = vec![Vec::new(), signature(first), signature(second), script.clone()];
    let raw = to_hex(&release.encode());

    // A forged witness fails as a script; the real one is relayed and mined.
    let mut forged = release.clone();
    forged.inputs[0].witness[3].push(0x51);
    assert!(node.cli(&["sendrawtransaction", &to_hex(&forged.encode())[2..]]).is_err());
    let sent = node.cli(&["sendrawtransaction", &raw[2..]]).unwrap();
    node.mine(1);
    let mined: serde_json::Value = serde_json::from_str(&node.cli(&["getrawtransaction", &sent, "true"]).unwrap()).unwrap();

    assert_eq!(mined["confirmations"], 1);
    let vbytes = crate::bridge::btc_release_vbytes(release.base().len(), &[(2, script.len())]);
    assert!(mined["vsize"].as_u64().unwrap() <= vbytes, "{} over the {vbytes} the floor goes by", mined["vsize"]);
    assert!(vbytes - mined["vsize"].as_u64().unwrap() < 10, "{} well under {vbytes}", mined["vsize"]);
}
