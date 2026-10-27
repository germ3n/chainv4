//! contracts/lua/btc_gateway.lua over the relay: a deposit to an address's deposit address minted
//! as wBTC on its proof, a withdrawal paid by a release the gateway checks and the federation
//! signs input by input as BIP143 has it, and a sweep of an old federation's coins.

use {
    super::btc_relay::{REGTEST, mined},
    super::*,
    crate::crypto::bitcoin::sha256d,
    crate::crypto::bitcoin::tx::{Transaction, TxIn, TxOut},
    crate::registry::federation::{DecidedFederation, Federation},
};

const NOW: u64 = 1_800_000_000_000;

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
    gateway: Address,
    wbtc: Address,
    /// The relay's tip and its time, to mine on.
    tip: [u8; 32],
    time: u32,
}

impl Btc {
    fn new() -> Btc {
        let mut world = World {
            vm: Arc::new(Vm::new(&[Engine::Lua, Engine::Wasm], HashType::default_chain()).unwrap().with_federation(Some(federation(&[21, 22, 23], 2)))),
            ..World::new()
        };
        let relay = world.deploy(Code::Wasm(example("btc_relay")));
        let template = world.lua_code(include_str!("../../../contracts/lua/wrapped.lua"));
        let gateway = world.lua_code(include_str!("../../../contracts/lua/btc_gateway.lua"));
        let start = mined(&[0; 32], &[0; 32], 1_700_000_000, REGTEST);
        world.call_at(DEPLOYER, relay, "start", &[bytes(&start), number(0), TxCallArgumentType::String("regtest".into())], 0, NOW).result.unwrap();
        let made = world.call_at(DEPLOYER, gateway, "setup", &[address(relay), address(template), number(10_000), number(3_600_000)], 0, NOW).result.unwrap();
        let [TxCallArgumentType::Address(wbtc)] = made[..] else { panic!("{made:?}") };

        Btc { world, relay, gateway, wbtc, tip: sha256d(&start), time: 1_700_000_000 }
    }

    fn call_as(&self, sender: Address, function: &str, args: &[TxCallArgumentType]) -> Result<Vec<TxCallArgumentType>, String> {
        self.world.call_at(sender, self.gateway, function, args, 0, NOW).result.map_err(|err| err.to_string())
    }

    fn call(&self, function: &str, args: &[TxCallArgumentType]) -> Result<Vec<TxCallArgumentType>, String> {
        self.call_as(DEPLOYER, function, args)
    }

    /// A block holding a coinbase and the transaction, on the relay's tip: its hash, and the
    /// transaction's place and Merkle path in it.
    fn mine(&mut self, tx: &[u8]) -> ([u8; 32], u64, Vec<u8>) {
        let coinbase = sha256d(&self.time.to_le_bytes());
        let txid = sha256d(tx);
        let root = sha256d(&[coinbase, txid].concat());
        self.time += 600;
        let header = mined(&self.tip, &root, self.time, REGTEST);
        self.world.call_at(DEPLOYER, self.relay, "submit", &[bytes(&header)], 0, NOW).result.unwrap();
        self.tip = sha256d(&header);

        (self.tip, 1, coinbase.to_vec())
    }

    /// An empty block on the tip, burying what came before one deeper.
    fn bury(&mut self) {
        self.time += 600;
        let header = mined(&self.tip, &[9; 32], self.time, REGTEST);
        self.world.call_at(DEPLOYER, self.relay, "submit", &[bytes(&header)], 0, NOW).result.unwrap();
        self.tip = sha256d(&header);
    }

    fn wbtc_of(&self, holder: Address) -> u256 {
        match self.world.call(self.wbtc, "balance_of", &[address(holder)]).result.unwrap()[..] {
            [TxCallArgumentType::U256(held)] => held,
            ref other => panic!("{other:?}"),
        }
    }
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
    let scripts = btc.call("deposit_script", &[address(alice)]).unwrap();
    let [TxCallArgumentType::Bytes(witness), TxCallArgumentType::Bytes(output), TxCallArgumentType::I64(0)] = &scripts[..] else { panic!("{scripts:?}") };

    // The deposit script: Alice's 33 bytes, dropped, then two of three.
    assert_eq!((witness[0], &witness[1..34], witness[34], witness[35]), (33, alice.to_abi().as_slice(), 0x75, 0x52));
    assert_eq!(output[..2], [0, 32]);

    // Paid on Bitcoin, mined, but one block deep: not yet.
    let deposit = pays(output, 50_000);
    let (block, index, path) = btc.mine(&deposit);
    let show = |btc: &Btc, recipient: Address| btc.call("deposit", &[bytes(&block), bytes(&deposit), number(index), bytes(&path), number(0), address(recipient), TxCallArgumentType::Nil]);

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
    assert!(btc.call_as(alice, "withdraw", &[bytes(&bob), number(500)]).unwrap_err().contains("at least"));
    assert_eq!(btc.call_as(alice, "withdraw", &[bytes(&bob), number(20_000)]).unwrap(), [TxCallArgumentType::I64(1)]);
    assert_eq!(btc.wbtc_of(alice), u256::from(30_000u64));

    // A release spends the deposit: Bob's 20,000, change to the federation, 1,000 in fees.
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
    let propose = |btc: &Btc, tx: &Transaction| btc.call("propose", &[bytes(&tx.base()), number(1)]);

    assert!(propose(&btc, &release(19_000, 30_000, deposit_id)).unwrap_err().contains("does not pay its withdrawal"));
    assert!(propose(&btc, &release(20_000, 10_000, deposit_id)).unwrap_err().contains("past the cap"));
    assert!(propose(&btc, &release(20_000, 29_000, [1; 32])).unwrap_err().contains("not an output the gateway holds"));
    let good = release(20_000, 29_000, deposit_id);

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
    let (block, index, path) = btc.mine(&good.base());
    btc.bury();
    btc.call("released", &[number(1), bytes(&block), bytes(&good.base()), number(index), bytes(&path)]).unwrap();

    assert_eq!(btc.call("withdrawal", &[number(1)]).unwrap()[2], TxCallArgumentType::String("paid".into()));
    assert_eq!(btc.call("utxo_count", &[]).unwrap(), [TxCallArgumentType::I64(2)]);
    assert_eq!(btc.call("utxo", &[number(0)]).unwrap()[4], TxCallArgumentType::String("spent".into()));
    assert_eq!(btc.call("utxo", &[number(1)]).unwrap()[1], TxCallArgumentType::I64(29_000));

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
        outputs: vec![TxOut { value: 28_500, script: new_change }],
        lock_time: 0,
    };

    assert_eq!(btc.call("propose", &[bytes(&sweep.base())]).unwrap(), [TxCallArgumentType::I64(2)]);

    // Deposits to the old federation's addresses still mint within the grace period.
    let old_output = match &btc.call("deposit_script", &[address(alice)]).unwrap()[..] {
        [_, TxCallArgumentType::Bytes(output), TxCallArgumentType::I64(1)] => output.clone(),
        other => panic!("{other:?}"),
    };

    assert_ne!(&old_output, output, "a new federation, new deposit addresses");
    let late = pays(output, 7_000);
    let (block, index, path) = btc.mine(&late);
    btc.bury();
    btc.call("deposit", &[bytes(&block), bytes(&late), number(index), bytes(&path), number(0), address(alice), number(0)]).unwrap();

    assert_eq!(btc.wbtc_of(alice), u256::from(37_000u64));
}
