//! EVM contracts through the whole chain: deploys and calls as transactions in blocks, their
//! receipts, and the state they leave. The contract is contracts/evm/Counter.sol, built with solc
//! into Counter.bin beside it.

use {
    super::*,
    crate::crypto::keypair::KeyType,
    crate::structs::event::Event,
    crate::vm::VmError,
    crate::vm::evm::{self, LOG},
    revm::primitives::keccak256,
};

const COUNTER_BIN: &str = include_str!("../../../contracts/evm/Counter.bin");

/// The first four bytes of a function's keccak256: what Solidity dispatches on.
fn selector(signature: &str) -> Vec<u8> {
    keccak256(signature.as_bytes())[..4].to_vec()
}

/// A number as one ABI word.
fn word(value: u128) -> Vec<u8> {
    u256::new(value).to_be_bytes().to_vec()
}

/// An address as one ABI word: its 20 EVM bytes, padded.
fn address_word(address: &Address) -> Vec<u8> {
    [vec![0; 12], evm::evm_alias(address).to_vec()].concat()
}

fn calldata(signature: &str, args: &[Vec<u8>]) -> Vec<u8> {
    [selector(signature), args.concat()].concat()
}

fn sign(chain: &Chain, from: &Keypair, tx: Tx) -> SignedTx {
    SignedTx::sign(tx, nonce(chain, from), FeeOffer::NONE, from)
}

fn call(chain: &Chain, from: &Keypair, to: Address, value: u128, input: Vec<u8>) -> SignedTx {
    sign(chain, from, Tx::EvmCall { to, value: Amount::U128(value), input })
}

/// Deploys the counter, starting at start, and returns its address.
fn deploy(chain: &mut Chain, start: u128) -> Address {
    let deployer = authority();
    let init = [crate::structs::hex::from_hex(COUNTER_BIN.trim()).unwrap(), word(start)].concat();
    let expected = evm::create_address(&deployer.public().address(), nonce(chain, &deployer));
    let deploy = sign(chain, &deployer, Tx::Deploy { code: Code::Evm(init) });
    let receipts = advance(chain, vec![deploy]);

    assert_eq!(receipts[0].result, Ok(Effect::Deployed(expected)));
    assert!(matches!(expected, Address::Evm(_)));

    expected
}

/// A transaction signed again one nonce later, for the second one a sender puts in a block.
fn next_nonce(signed: SignedTx, from: &Keypair) -> SignedTx {
    SignedTx::sign(signed.tx, signed.nonce + 1, FeeOffer::NONE, from)
}

/// What a call returned, as bytes.
fn returned(receipt: &Receipt) -> Vec<u8> {
    match &receipt.result {
        Ok(Effect::Returned(values)) => match values.as_slice() {
            [TxCallArgumentType::Bytes(bytes)] => bytes.clone(),
            other => panic!("expected bytes, got {other:?}"),
        },
        other => panic!("expected a return, got {other:?}"),
    }
}

fn count(chain: &mut Chain, counter: Address) -> Vec<u8> {
    let read = call(chain, &authority(), counter, 0, calldata("count()", &[]));

    returned(&advance(chain, vec![read])[0])
}

#[test]
fn an_evm_contract_deploys_counts_emits_and_reverts_with_its_reason() {
    let db = Arc::new(MemoryDb::default());
    let mut chain = open(&db);
    let me = authority().public().address();
    let counter = deploy(&mut chain, 5);

    assert_eq!(count(&mut chain, counter), word(5), "the constructor's argument");

    let bump = call(&chain, &authority(), counter, 0, calldata("bump(uint256)", &[word(3)]));
    let receipt = advance(&mut chain, vec![bump]).remove(0);

    assert_eq!(receipt.result, Ok(Effect::Returned(vec![TxCallArgumentType::Bytes(Vec::new())])));
    assert!(receipt.fuel_used > 21_000, "EVM gas, its intrinsic 21,000 included");
    assert_eq!(receipt.events, vec![Event {
        contract: counter,
        name: LOG.to_string(),
        data: vec![
            TxCallArgumentType::Bytes(keccak256(b"Bumped(address,uint256)").to_vec()),
            TxCallArgumentType::Bytes(address_word(&me)),
            TxCallArgumentType::Bytes(word(8)),
        ],
    }]);
    assert_eq!(count(&mut chain, counter), word(8));

    // Adding nothing reverts with Solidity's reason, and changes nothing.
    let nothing = call(&chain, &authority(), counter, 0, calldata("bump(uint256)", &[word(0)]));
    let receipt = advance(&mut chain, vec![nothing]).remove(0);

    assert_eq!(receipt.result, Err(Failure::Contract(VmError::Runtime("reverted: nothing to add".to_string()))));
    assert!(receipt.events.is_empty());
    assert_eq!(count(&mut chain, counter), word(8));

    // The Ed25519 deployer took part under its alias, written down so the EVM can pay it.
    assert_eq!(evm::account_of(&chain.state(), evm::evm_alias(&me)).unwrap(), me);
}

#[test]
fn the_coin_moves_into_evm_contracts_and_back_out_to_any_account() {
    let db = Arc::new(MemoryDb::default());
    let mut chain = open(&db);
    let me = authority().public().address();
    let ethereum = Keypair::generate(KeyType::Secp256k1).unwrap().public().address();
    let counter = deploy(&mut chain, 0);
    let before = balance(&chain, &me);

    let deposit = call(&chain, &authority(), counter, 1_000, calldata("deposit()", &[]));
    advance(&mut chain, vec![deposit]);

    assert_eq!(balance(&chain, &counter), u256::new(1_000));
    assert_eq!(balance(&chain, &me), before - u256::new(1_000));

    // Paid out to the Ed25519 sender's alias, it reaches the sender; to a secp256k1 account, that.
    let pay_me = call(&chain, &authority(), counter, 0, calldata("pay(address,uint256)", &[address_word(&me), word(400)]));
    let pay_them = call(&chain, &authority(), counter, 0, calldata("pay(address,uint256)", &[address_word(&ethereum), word(100)]));
    let pay_them = next_nonce(pay_them, &authority());
    let paid = advance(&mut chain, vec![pay_me, pay_them]);

    assert!(paid.iter().all(|receipt| receipt.result.is_ok()), "{paid:?}");

    assert_eq!(balance(&chain, &me), before - u256::new(600));
    assert_eq!(balance(&chain, &ethereum), u256::new(100));
    assert_eq!(balance(&chain, &counter), u256::new(500));

    // A call with no input to an account with no code is a plain transfer.
    let plain = call(&chain, &authority(), ethereum, 50, Vec::new());
    advance(&mut chain, vec![plain]);

    assert_eq!(balance(&chain, &ethereum), u256::new(150));

    // Paying more than the contract holds fails, and moves nothing.
    let too_much = call(&chain, &authority(), counter, 0, calldata("pay(address,uint256)", &[address_word(&me), word(10_000)]));
    let receipt = advance(&mut chain, vec![too_much]).remove(0);

    assert_eq!(receipt.result, Err(Failure::Contract(VmError::Runtime("reverted: the payment failed".to_string()))));
    assert_eq!(balance(&chain, &counter), u256::new(500));
}

#[test]
fn any_transaction_writes_its_senders_alias_down_so_the_evm_can_pay_it() {
    let db = Arc::new(MemoryDb::default());
    let mut chain = open(&db);
    let counter = deploy(&mut chain, 0);
    let deposit = call(&chain, &authority(), counter, 1_000, calldata("deposit()", &[]));
    let bob = Keypair::generate(KeyType::Ed25519).unwrap();
    let carol = Keypair::generate(KeyType::Ed25519).unwrap();
    let fund = |to: &Keypair, nonce: u64| {
        SignedTx::sign(
            Tx::Transfer { from: authority().public().address(), to: to.public().address(), amount: Amount::U128(10u128.pow(21)) },
            nonce,
            FeeOffer::NONE,
            &authority(),
        )
    };
    let first = nonce(&chain, &authority()) + 1;
    advance(&mut chain, vec![deposit, fund(&bob, first), fund(&carol, first + 1)]);

    // Bob sends a plain transfer, nothing to do with the EVM; Carol sends nothing at all.
    let plain = SignedTx::sign(
        Tx::Transfer { from: bob.public().address(), to: authority().public().address(), amount: Amount::U128(1) },
        nonce(&chain, &bob),
        FeeOffer::NONE,
        &bob,
    );
    advance(&mut chain, vec![plain]);

    assert_eq!(evm::account_of(&chain.state(), evm::evm_alias(&bob.public().address())).unwrap(), bob.public().address());

    // Paid out to his alias, the coin reaches Bob's own account. Carol's alias is read as the
    // secp256k1 account of those 20 bytes until she sends something, so what is paid to it lands
    // there, out of her reach.
    let bob_before = balance(&chain, &bob.public().address());
    let carol_before = balance(&chain, &carol.public().address());
    let pay_bob = call(&chain, &authority(), counter, 0, calldata("pay(address,uint256)", &[address_word(&bob.public().address()), word(300)]));
    let pay_carol =
        next_nonce(call(&chain, &authority(), counter, 0, calldata("pay(address,uint256)", &[address_word(&carol.public().address()), word(200)])), &authority());
    let paid = advance(&mut chain, vec![pay_bob, pay_carol]);

    assert!(paid.iter().all(|receipt| receipt.result.is_ok()), "{paid:?}");
    assert_eq!(balance(&chain, &bob.public().address()), bob_before + u256::new(300));
    assert_eq!(balance(&chain, &carol.public().address()), carol_before);
    assert_eq!(balance(&chain, &Address::Secp256k1(evm::evm_alias(&carol.public().address()))), u256::new(200));
}

#[test]
fn an_evm_contract_creates_contracts_and_reads_the_block() {
    let db = Arc::new(MemoryDb::default());
    let mut chain = open(&db);
    let counter = deploy(&mut chain, 0);

    // Its first CREATE goes where Ethereum puts it: from the contract and its nonce, 1.
    let spawn = call(&chain, &authority(), counter, 0, calldata("spawn()", &[]));
    let made = returned(&advance(&mut chain, vec![spawn])[0]);
    let child = evm::create_address(&counter, 1);

    assert_eq!(made, address_word(&child));

    let made = call(&chain, &authority(), child, 0, calldata("made()", &[]));

    assert_eq!(returned(&advance(&mut chain, vec![made])[0]), word(7));

    let info = call(&chain, &authority(), counter, 0, calldata("blockInfo()", &[]));
    let (block, now) = next_block(&chain, vec![info]);
    let receipt = extended(chain.import(&block, now)).remove(0);
    let expected = [
        word(u128::from(block.header.height)),
        word(u128::from(block.header.timestamp / 1_000)),
        word(u128::from(evm::chain_id(&chain.genesis()))),
    ]
    .concat();

    assert_eq!(returned(&receipt), expected);
}

#[test]
fn running_out_of_gas_burns_what_it_may_and_keeps_nothing() {
    let db = Arc::new(MemoryDb::default());
    let mut chain = open(&db);
    let counter = deploy(&mut chain, 3);
    let spin = call(&chain, &authority(), counter, 0, calldata("spin()", &[]));
    let receipt = advance(&mut chain, vec![spin]).remove(0);

    assert_eq!(receipt.result, Err(Failure::Contract(VmError::OutOfFuel)));
    assert_eq!(count(&mut chain, counter), word(3));
}

#[test]
fn evm_contracts_are_called_and_upgraded_only_the_evm_way() {
    let db = Arc::new(MemoryDb::default());
    let mut chain = open(&db);
    let counter = deploy(&mut chain, 0);
    let native = sign(&chain, &authority(), Tx::ContractCall {
        to: counter,
        func: Hash::Xxh3_64(crate::vm::selector("count")),
        amount: Amount::U128(0),
        data: Vec::new(),
    });
    let receipt = advance(&mut chain, vec![native]).remove(0);

    assert!(matches!(receipt.result, Err(Failure::Contract(VmError::Runtime(ref why))) if why.contains("EVM call")), "{:?}", receipt.result);

    let upgrade = sign(&chain, &authority(), Tx::Upgrade { contract: counter, code: Code::Lua(COUNTER.to_string()) });
    let receipt = advance(&mut chain, vec![upgrade]).remove(0);

    assert!(matches!(receipt.result, Err(Failure::Contract(VmError::Runtime(ref why))) if why.contains("proxy")), "{:?}", receipt.result);
}

#[test]
fn a_chain_without_the_evm_refuses_evm_transactions() {
    let db = Arc::new(MemoryDb::default());
    let genesis = Genesis {
        engines: Engine::compiled().into_iter().filter(|engine| *engine != Engine::Evm).collect(),
        ..super::genesis()
    };
    let mut chain = Chain::open(db, &genesis).unwrap();
    let init = crate::structs::hex::from_hex(COUNTER_BIN.trim()).unwrap();
    let deploy = sign(&chain, &authority(), Tx::Deploy { code: Code::Evm(init) });
    let receipts = advance(&mut chain, vec![deploy]);
    let call = call(&chain, &authority(), Address::Evm([1; 20]), 0, Vec::new());
    let more = advance(&mut chain, vec![call]);

    assert_eq!(receipts[0].result, Err(Failure::EngineNotAllowed(Engine::Evm)));
    assert_eq!(more[0].result, Err(Failure::EngineNotAllowed(Engine::Evm)));
}

#[test]
fn evm_transactions_in_one_block_run_side_by_side_as_they_would_one_by_one() {
    let db = Arc::new(MemoryDb::default());
    let mut chain = open(&db);
    let counter = deploy(&mut chain, 0);
    let others: Vec<Keypair> = (0..4).map(|_| stranger()).collect();
    let first = nonce(&chain, &authority());
    let funding = others
        .iter()
        .zip(first..)
        .map(|(other, nonce)| {
            let tx = Tx::Transfer { from: authority().public().address(), to: other.public().address(), amount: Amount::U128(1_000) };

            SignedTx::sign(tx, nonce, FeeOffer::NONE, &authority())
        })
        .collect();
    advance(&mut chain, funding);

    // Each bumps the same counter and deposits: every one reads what the one before wrote.
    let mut txs = Vec::new();

    for other in &others {
        txs.push(call(&chain, other, counter, 0, calldata("bump(uint256)", &[word(2)])));
        txs.push(next_nonce(call(&chain, other, counter, 10, calldata("deposit()", &[])), other));
    }

    let receipts = advance(&mut chain, txs);

    assert!(receipts.iter().all(|receipt| receipt.result.is_ok()), "{receipts:?}");
    assert_eq!(count(&mut chain, counter), word(8));
    assert_eq!(balance(&chain, &counter), u256::new(40));
}

#[test]
fn under_fees_evm_gas_is_the_chains_gas_one_for_one() {
    let db = Arc::new(MemoryDb::default());
    let paid = Genesis::dev_authority(authority().public());
    let mut chain = Chain::open(db, &paid).unwrap();
    let fees = paid.fees.clone().unwrap().build();
    let offer = FeeOffer { gas_limit: 300_000, max_price: u256::new(1_000_000_000), tip: u256::ZERO };
    let init = [crate::structs::hex::from_hex(COUNTER_BIN.trim()).unwrap(), word(0)].concat();
    // Storing code costs gas by the byte, as on Ethereum.
    let deploy_offer = FeeOffer { gas_limit: 3_000_000, ..offer };
    let deploy = SignedTx::sign(Tx::Deploy { code: Code::Evm(init) }, nonce(&chain, &authority()), deploy_offer, &authority());
    let counter = evm::create_address(&authority().public().address(), deploy.nonce);
    let deployed = advance(&mut chain, vec![deploy]).remove(0);

    assert_eq!(deployed.result, Ok(Effect::Deployed(counter)));

    let bump = SignedTx::sign(
        Tx::EvmCall { to: counter, value: Amount::U128(0), input: calldata("bump(uint256)", &[word(1)]) },
        nonce(&chain, &authority()),
        offer,
        &authority(),
    );
    let intrinsic = fees.schedule().intrinsic(&bump);
    let receipt = advance(&mut chain, vec![bump]).remove(0);
    let evm_gas = fees.schedule().for_fuel(receipt.fuel_used);

    assert!(receipt.result.is_ok(), "{:?}", receipt.result);
    assert!(evm_gas > 21_000, "the EVM's own gas, its intrinsic 21,000 included: {evm_gas}");
    assert_eq!(receipt.gas_used, intrinsic + evm_gas);
    assert!(receipt.fee > u256::ZERO);
}

/// A string as one ABI argument, for a function taking only it: its offset, length and bytes.
#[cfg(feature = "names")]
fn string_arg(text: &str) -> Vec<u8> {
    let mut bytes = text.as_bytes().to_vec();
    bytes.resize(text.len().div_ceil(32) * 32, 0);

    [word(32), word(text.len() as u128), bytes].concat()
}

/// A string an EVM call returned, ABI-decoded.
#[cfg(feature = "names")]
fn returned_string(receipt: &Receipt) -> String {
    let bytes = returned(receipt);
    let len = u256::from_be_bytes(bytes[32..64].try_into().unwrap()).as_usize();

    String::from_utf8(bytes[64..64 + len].to_vec()).unwrap()
}

#[cfg(feature = "names")]
#[test]
fn evm_contracts_resolve_and_pay_names_through_the_precompile() {
    use crate::registry::names::NameAction;

    const NAME_PAYER_BIN: &str = include_str!("../../../contracts/evm/NamePayer.bin");

    let db = Arc::new(MemoryDb::default());
    let mut chain = Chain::open(db, &genesis()).unwrap();
    // Alice holds an Ed25519 key, and has never sent an EVM transaction: a name resolves to her
    // alias all the same.
    let alice = Keypair::generate(KeyType::Ed25519).unwrap();
    let fund = SignedTx::sign(
        Tx::Transfer { from: authority().public().address(), to: alice.public().address(), amount: Amount::U128(10u128.pow(21)) },
        nonce(&chain, &authority()),
        FeeOffer::NONE,
        &authority(),
    );
    advance(&mut chain, vec![fund]);
    let register = sign(&chain, &alice, Tx::Name { name: "alice".into(), action: NameAction::Register { years: 1 } });
    let primary = next_nonce(sign(&chain, &alice, Tx::PrimaryName { name: Some("alice".into()) }), &alice);
    let payer = evm::create_address(&authority().public().address(), nonce(&chain, &authority()));
    let deploy = sign(&chain, &authority(), Tx::Deploy { code: Code::Evm(crate::structs::hex::from_hex(NAME_PAYER_BIN.trim()).unwrap()) });
    let receipts = advance(&mut chain, vec![register, primary, deploy]);

    assert!(receipts.iter().all(|receipt| receipt.result.is_ok()), "{receipts:?}");

    let before = balance(&chain, &alice.public().address());
    let pay = call(&chain, &authority(), payer, 1_000, calldata("pay(string)", &[string_arg("Alice.dev")]));
    let lookup = next_nonce(call(&chain, &authority(), payer, 0, calldata("lookup(string)", &[string_arg("alice")])), &authority());
    let receipts = advance(&mut chain, vec![pay, lookup]);

    assert_eq!(receipts[0].result.as_ref().map(|_| ()), Ok(()), "{:?}", receipts[0].result);
    assert_eq!(balance(&chain, &alice.public().address()), before + u256::new(1_000), "paid to her own account, through her alias");
    assert_eq!(returned(&receipts[1])[12..], evm::evm_alias(&alice.public().address()), "the name resolves to her EVM address");

    // The other way: her alias goes by her primary name. Nobody's resolves to nothing.
    let whois = call(&chain, &authority(), payer, 0, calldata("whois(address)", &[address_word(&alice.public().address())]));
    let nobody = next_nonce(call(&chain, &authority(), payer, 5, calldata("pay(string)", &[string_arg("nobody.dev")])), &authority());
    let receipts = advance(&mut chain, vec![whois, nobody]);

    assert_eq!(returned_string(&receipts[0]), "alice.dev");
    assert!(matches!(&receipts[1].result, Err(failure) if failure.to_string().contains("no such name")), "{:?}", receipts[1].result);

    // On a chain without names there is no precompile at its address: a call there is a call to an
    // empty account, which returns nothing, so the payer's decoding reverts.
    let nameless = Genesis { names: None, ..genesis() };
    let mut chain = Chain::open(Arc::new(MemoryDb::default()), &nameless).unwrap();
    let payer = evm::create_address(&authority().public().address(), nonce(&chain, &authority()));
    let deploy = sign(&chain, &authority(), Tx::Deploy { code: Code::Evm(crate::structs::hex::from_hex(NAME_PAYER_BIN.trim()).unwrap()) });
    advance(&mut chain, vec![deploy]);
    let lookup = call(&chain, &authority(), payer, 0, calldata("lookup(string)", &[string_arg("alice")]));

    assert!(advance(&mut chain, vec![lookup])[0].result.is_err());
}

/// An Ethereum account with coin to send, funded from the authority.
fn ethereum_account(chain: &mut Chain, secret: [u8; 32]) -> Address {
    let address = Keypair::from_private(crate::crypto::keypair::PrivateKey::Secp256k1(secret)).public().address();
    let fund = SignedTx::sign(
        Tx::Transfer { from: authority().public().address(), to: address, amount: Amount::U128(10u128.pow(21)) },
        nonce(chain, &authority()),
        FeeOffer::NONE,
        &authority(),
    );
    advance(chain, vec![fund]);

    address
}

/// An Ethereum transaction signed for this chain, of a type, as a wallet signs one.
fn ethereum(chain: &Chain, secret: &[u8; 32], kind: crate::crypto::ethereum::EthereumKind, nonce: u64, to: Option<[u8; 20]>, value: u128, input: Vec<u8>) -> SignedTx {
    use crate::crypto::ethereum::{AccessItem, EthereumKind, EthereumTx};

    let unsigned = EthereumTx {
        kind,
        chain_id: evm::chain_id(&chain.genesis()),
        nonce,
        max_price: u256::ZERO,
        tip: u256::ZERO,
        gas_limit: 3_000_000,
        to,
        value: u256::new(value),
        input,
        access_list: match (kind, to) {
            (EthereumKind::Legacy, _) | (_, None) => Vec::new(),
            (_, Some(to)) => vec![AccessItem { address: to, slots: vec![[0; 32]] }],
        },
        signature: [0; 64],
        parity: 0,
    };

    SignedTx::from_ethereum(unsigned.signed(secret).encode()).unwrap()
}

#[test]
fn ethereum_signed_transactions_deploy_call_and_pay_as_on_ethereum() {
    use crate::crypto::ethereum::EthereumKind;

    let db = Arc::new(MemoryDb::default());
    let mut chain = Chain::open(db, &genesis()).unwrap();
    let secret = [0x46; 32];
    let sender = ethereum_account(&mut chain, secret);
    let init = [crate::structs::hex::from_hex(COUNTER_BIN.trim()).unwrap(), word(5)].concat();

    // A create, of EIP-1559's type, lands where Ethereum puts the sender's first contract.
    let deploy = ethereum(&chain, &secret, EthereumKind::DynamicFee, 0, None, 0, init);
    let counter = evm::create_address(&sender, 0);
    let receipts = advance(&mut chain, vec![deploy.clone()]);

    assert_eq!(receipts[0].result, Ok(Effect::Deployed(counter)));
    assert_eq!(deploy.sender(), sender, "the key its signature recovers sends it");
    assert_eq!(nonce(&chain, &Keypair::from_private(crate::crypto::keypair::PrivateKey::Secp256k1(secret))), 1, "its nonce is the account's");

    // A legacy call and an EIP-2930 one, with an access list, in one block.
    let Address::Evm(to) = counter else { unreachable!() };
    let bump = ethereum(&chain, &secret, EthereumKind::Legacy, 1, Some(to), 0, calldata("bump(uint256)", &[word(3)]));
    let deposit = ethereum(&chain, &secret, EthereumKind::AccessList, 2, Some(to), 700, calldata("deposit()", &[]));
    let receipts = advance(&mut chain, vec![bump, deposit]);

    assert!(receipts.iter().all(|receipt| receipt.result.is_ok()), "{receipts:?}");
    assert_eq!(count(&mut chain, counter), word(8));
    assert_eq!(balance(&chain, &counter), u256::new(700));

    // A plain payment to another Ethereum account runs through the EVM too.
    let other = [0x77; 20];
    let pay = ethereum(&chain, &secret, EthereumKind::DynamicFee, 3, Some(other), 1_000, Vec::new());
    assert!(advance(&mut chain, vec![pay])[0].result.is_ok());
    assert_eq!(balance(&chain, &Address::Secp256k1(other)), u256::new(1_000));
}

#[test]
fn ethereum_signatures_hold_on_their_own_chain_alone_and_say_what_they_carry() {
    use crate::crypto::ethereum::{EthereumKind, decode};
    use crate::crypto::verifiable::VerifyError;

    let db = Arc::new(MemoryDb::default());
    let chain = Chain::open(db, &genesis()).unwrap();
    let secret = [0x46; 32];
    let good = ethereum(&chain, &secret, EthereumKind::DynamicFee, 0, Some([1; 20]), 0, Vec::new());

    assert_eq!(good.verify_on(&chain.genesis()), Ok(()));

    // Signed for another chain.
    let Tx::Ethereum { raw } = &good.tx else { unreachable!() };
    let mut elsewhere = decode(raw).unwrap();
    elsewhere.chain_id += 1;
    let elsewhere = SignedTx::from_ethereum(elsewhere.signed(&secret).encode()).unwrap();

    assert_eq!(elsewhere.verify_on(&chain.genesis()), Err(VerifyError::BadEthereum));

    // A nonce, fee or signer that is not what the bytes say.
    let wrong_nonce = SignedTx { nonce: 9, ..good.clone() };
    let wrong_fee = SignedTx { fee: FeeOffer { gas_limit: 1, ..good.fee }, ..good.clone() };
    let wrong_signer = SignedTx { signer: Keypair::generate(KeyType::Secp256k1).unwrap().public(), ..good.clone() };

    for wrong in [wrong_nonce, wrong_fee, wrong_signer] {
        assert_eq!(wrong.verify_on(&chain.genesis()), Err(VerifyError::BadEthereum));
    }

    // And an Ethereum transaction under a signature of chainv4's own never holds.
    let disguised = SignedTx::sign(good.tx.clone(), 0, FeeOffer::NONE, &authority());

    assert_eq!(disguised.verify_on(&chain.genesis()), Err(VerifyError::BadEthereum));
}
