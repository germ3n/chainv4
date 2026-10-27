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

#[cfg(feature = "bitcoin")]
#[test]
fn a_bitcoin_account_sends_and_the_evm_pays_it_under_its_alias() {
    let db = Arc::new(MemoryDb::default());
    let mut chain = open(&db);
    let counter = deploy(&mut chain, 0);
    let deposit = call(&chain, &authority(), counter, 1_000, calldata("deposit()", &[]));
    let dave = Keypair::generate(KeyType::Bitcoin).unwrap();
    let address = dave.public().address();

    assert!(matches!(address, Address::Bitcoin(_)) && address.to_string().starts_with("bc1q"));

    let fund = SignedTx::sign(
        Tx::Transfer { from: authority().public().address(), to: address, amount: Amount::U128(10u128.pow(21)) },
        nonce(&chain, &authority()) + 1,
        FeeOffer::NONE,
        &authority(),
    );
    advance(&mut chain, vec![deposit, fund]);

    // Dave signs with his bitcoin key, over double SHA-256; the chain checks it and writes his
    // alias down, as for any account that is not its own 20 EVM bytes.
    let back = SignedTx::sign(
        Tx::Transfer { from: address, to: authority().public().address(), amount: Amount::U128(5) },
        nonce(&chain, &dave),
        FeeOffer::NONE,
        &dave,
    );

    assert!(crate::crypto::verifiable::Verifiable::verify(&back).is_ok());
    assert!(advance(&mut chain, vec![back])[0].result.is_ok());
    assert_eq!(nonce(&chain, &dave), 1);
    assert_eq!(evm::account_of(&chain.state(), evm::evm_alias(&address)).unwrap(), address);
    assert_ne!(evm::evm_alias(&address), *<&[u8; 20]>::try_from(address.bytes()).unwrap(), "an alias, not its HASH160");

    let before = balance(&chain, &address);
    let pay = call(&chain, &authority(), counter, 0, calldata("pay(address,uint256)", &[address_word(&address), word(300)]));

    assert!(advance(&mut chain, vec![pay])[0].result.is_ok());
    assert_eq!(balance(&chain, &address), before + u256::new(300));
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

const BRIDGE_BIN: &str = include_str!("../../../contracts/evm/Bridge.bin");

/// A native counter for the bridge to call, and to call it back.
const NATIVE_COUNTER: &str = r#"
local M = {}

-- Adds to the total, keeping what was sent, and returns the total.
function M.add(by)
    local total = u64(storage.get("total") or "0") + by
    storage.set("total", tostring(total))
    events.emit("Added", total, ctx.sender, ctx.value)
    return total
end

function M.total()
    return u64(storage.get("total") or "0")
end

-- Writes, as a view never may.
function M.touch()
    storage.set("touched", "yes")
    return u64(1)
end

-- Bumps the bridge's count through the EVM, and returns the count.
function M.poke(bridge, by)
    return evm.decode("uint256", evm.call(bridge, evm.encode("bump(uint256)", u256(by))))
end

-- Bumps it, then fails: the EVM's change goes with this call's.
function M.poke_then_fail(bridge)
    evm.call(bridge, evm.encode("bump(uint256)", u256(1)))
    error("then failed")
end

-- Comes back to a native counter through the bridge: Lua, then the EVM, then Lua.
function M.round_trip(bridge, target, by)
    return evm.decode("(uint64)", evm.call(bridge, evm.encode("relay(bytes32,uint64)", target, u64(by))))
end

return M
"#;

/// Deploys a Lua contract and returns its address.
fn deploy_lua(chain: &mut Chain, source: &str) -> Address {
    let deploy = sign(chain, &authority(), Tx::Deploy { code: Code::Lua(source.to_string()) });
    let receipt = advance(chain, vec![deploy]).remove(0);

    match receipt.result {
        Ok(Effect::Deployed(address)) => address,
        other => panic!("expected a deploy, got {other:?}"),
    }
}

/// Deploys the bridge, and two native counters for it.
fn bridged(chain: &mut Chain) -> (Address, Address, Address) {
    let bridge = evm::create_address(&authority().public().address(), nonce(chain, &authority()));
    let deploy = sign(chain, &authority(), Tx::Deploy { code: Code::Evm(crate::structs::hex::from_hex(BRIDGE_BIN.trim()).unwrap()) });
    assert!(advance(chain, vec![deploy])[0].result.is_ok());

    (bridge, deploy_lua(chain, NATIVE_COUNTER), deploy_lua(chain, NATIVE_COUNTER))
}

/// A native contract as bytes32, as the bridge takes it.
fn contract_word(address: &Address) -> Vec<u8> {
    match address {
        Address::Contract(bytes) => bytes.to_vec(),
        other => panic!("{other} is no native contract"),
    }
}

fn total(chain: &Chain, counter: &Address) -> Option<String> {
    chain
        .state()
        .get(crate::db::Column::Storage, &crate::db::storage_key(counter, b"total"))
        .unwrap()
        .map(|bytes| String::from_utf8(bytes).unwrap())
}

fn bridge_count(chain: &Chain, bridge: &Address) -> Vec<u8> {
    let Address::Evm(bytes) = bridge else { unreachable!() };

    evm::storage_word(&chain.state(), *bytes, [0; 32]).unwrap().to_vec()
}

fn lua_call(chain: &Chain, to: Address, function: &str, data: Vec<TxCallArgumentType>) -> SignedTx {
    sign(chain, &authority(), Tx::ContractCall { to, func: Hash::Xxh3_64(crate::vm::selector(function)), amount: Amount::U128(0), data })
}

#[test]
fn evm_contracts_call_native_ones_and_send_them_coin() {
    let db = Arc::new(MemoryDb::default());
    let mut chain = open(&db);
    let (bridge, counter, _) = bridged(&mut chain);
    let add = call(&chain, &authority(), bridge, 300, calldata("add(bytes32,uint64)", &[contract_word(&counter), word(5)]));
    let receipt = advance(&mut chain, vec![add]).remove(0);

    assert_eq!(returned(&receipt), word(5), "{:?}", receipt.result);
    assert_eq!(total(&chain, &counter).as_deref(), Some("5"));
    assert_eq!(balance(&chain, &counter), u256::new(300), "what the EVM call carried reached the native contract");
    assert_eq!(balance(&chain, &bridge), u256::ZERO);
    assert_eq!(bridge_count(&chain, &bridge), word(1));

    // The native event comes where the native call ran: before the bridge's own log.
    let names: Vec<&str> = receipt.events.iter().map(|event| event.name.as_str()).collect();

    assert_eq!(names, ["Added", LOG]);
    assert_eq!(receipt.events[0].contract, counter);
    assert_eq!(receipt.events[0].data[1], TxCallArgumentType::Address(bridge), "the bridge called it, at its own address");
    assert_eq!(receipt.events[1].data[1], TxCallArgumentType::Bytes(word(5)));

    // A native view reads in a static call; a native function that writes is refused there.
    let peek = call(&chain, &authority(), bridge, 0, calldata("peek(bytes32,string)", &[contract_word(&counter), word(64), word(5), [b"total".to_vec(), vec![0; 27]].concat()]));
    let touch = next_nonce(call(&chain, &authority(), bridge, 0, calldata("peek(bytes32,string)", &[contract_word(&counter), word(64), word(5), [b"touch".to_vec(), vec![0; 27]].concat()])), &authority());
    let receipts = advance(&mut chain, vec![peek, touch]);

    assert_eq!(returned(&receipts[0]), word(5));
    assert!(matches!(&receipts[1].result, Err(failure) if failure.to_string().contains("changed state inside a static call")), "{:?}", receipts[1].result);
}

#[test]
fn a_revert_on_either_side_unwinds_both() {
    let db = Arc::new(MemoryDb::default());
    let mut chain = open(&db);
    let (bridge, counter, _) = bridged(&mut chain);
    let first = call(&chain, &authority(), bridge, 0, calldata("add(bytes32,uint64)", &[contract_word(&counter), word(2)]));
    advance(&mut chain, vec![first]);

    // The EVM reverts after the native call: the native addition goes with it.
    let revert = call(&chain, &authority(), bridge, 0, calldata("addThenRevert(bytes32,uint64)", &[contract_word(&counter), word(7)]));
    let receipt = advance(&mut chain, vec![revert]).remove(0);

    assert_eq!(receipt.result, Err(Failure::Contract(VmError::Runtime("reverted: changed my mind".to_string()))));
    assert_eq!(total(&chain, &counter).as_deref(), Some("2"));

    // An inner frame reverts and the outer one carries on: only the inner frame's native call is undone.
    let tried = call(&chain, &authority(), bridge, 0, calldata("tryAdd(bytes32,uint64)", &[contract_word(&counter), word(7)]));
    let receipt = advance(&mut chain, vec![tried]).remove(0);

    assert_eq!(returned(&receipt), word(0), "the try caught the revert");
    assert_eq!(total(&chain, &counter).as_deref(), Some("2"));
    assert_eq!(bridge_count(&chain, &bridge), word(11));
    assert!(receipt.events.is_empty(), "{:?}", receipt.events);

    // A native call that fails reverts the EVM's call with why.
    let nothing = Address::Contract([7; 32]);
    let missing = call(&chain, &authority(), bridge, 0, calldata("add(bytes32,uint64)", &[contract_word(&nothing), word(1)]));
    let receipt = advance(&mut chain, vec![missing]).remove(0);

    assert!(matches!(&receipt.result, Err(failure) if failure.to_string().contains("add on contract:")), "{:?}", receipt.result);

    // The other way: a native call fails after its EVM call, and the EVM's change goes too.
    let failing = lua_call(&chain, counter, "poke_then_fail", vec![TxCallArgumentType::Address(bridge)]);
    let receipt = advance(&mut chain, vec![failing]).remove(0);

    assert!(receipt.result.is_err());
    assert_eq!(bridge_count(&chain, &bridge), word(11));
}

#[test]
fn native_contracts_call_evm_ones_and_calls_nest_across_engines() {
    let db = Arc::new(MemoryDb::default());
    let mut chain = open(&db);
    let (bridge, counter, other) = bridged(&mut chain);
    let poke = lua_call(&chain, counter, "poke", vec![TxCallArgumentType::Address(bridge), TxCallArgumentType::U64(7)]);
    let receipt = advance(&mut chain, vec![poke]).remove(0);

    assert_eq!(receipt.result, Ok(Effect::Returned(vec![TxCallArgumentType::U256(u256::new(7))])));
    assert_eq!(bridge_count(&chain, &bridge), word(7));

    // Lua, then the EVM, then another Lua contract: each sees the one before as its sender.
    let round = lua_call(&chain, counter, "round_trip", vec![TxCallArgumentType::Address(bridge), TxCallArgumentType::Address(other), TxCallArgumentType::U64(4)]);
    let receipt = advance(&mut chain, vec![round]).remove(0);

    assert_eq!(receipt.result, Ok(Effect::Returned(vec![TxCallArgumentType::U64(4)])), "{:?}", receipt.result);
    assert_eq!(total(&chain, &other).as_deref(), Some("4"));
    assert_eq!(bridge_count(&chain, &bridge), word(8));
    let names: Vec<&str> = receipt.events.iter().map(|event| event.name.as_str()).collect();

    assert_eq!(names, ["Added", LOG]);
    assert_eq!(receipt.events[0].data[1], TxCallArgumentType::Address(bridge));

    // Coming back to the contract that started it is reentry, which add is not marked for.
    let back = lua_call(&chain, counter, "round_trip", vec![TxCallArgumentType::Address(bridge), TxCallArgumentType::Address(counter), TxCallArgumentType::U64(1)]);
    let receipt = advance(&mut chain, vec![back]).remove(0);

    assert!(matches!(&receipt.result, Err(failure) if failure.to_string().contains("reentr")), "{:?}", receipt.result);
    assert_eq!(total(&chain, &counter), None);
    assert_eq!(bridge_count(&chain, &bridge), word(8));
}

/// A native contract whose add calls back into the EVM: the bridge, then this, then the bridge.
const ECHO: &str = r#"
local M = {}

function M.set_bridge(bridge)
    storage.set("bridge", bridge:text())
end

-- Bumps the bridge's count by as much, from inside the bridge's own call to it.
function M.add(by)
    local bridge = address.parse(storage.get("bridge"))
    evm.call(bridge, evm.encode("bump(uint256)", u256(by)))
    return by
end

return M
"#;

#[test]
fn the_evm_calls_back_into_itself_through_a_native_contract() {
    let db = Arc::new(MemoryDb::default());
    let mut chain = open(&db);
    let (bridge, _, _) = bridged(&mut chain);
    let echo = deploy_lua(&mut chain, ECHO);
    let set = lua_call(&chain, echo, "set_bridge", vec![TxCallArgumentType::Address(bridge)]);
    advance(&mut chain, vec![set]);

    // add asks echo, which bumps the count by 3 in an EVM call of its own; add then adds its 1 to
    // what that left in the journal.
    let add = call(&chain, &authority(), bridge, 0, calldata("add(bytes32,uint64)", &[contract_word(&echo), word(3)]));
    let receipt = advance(&mut chain, vec![add]).remove(0);

    assert_eq!(returned(&receipt), word(3), "{:?}", receipt.result);
    assert_eq!(bridge_count(&chain, &bridge), word(4));

    // Reverted after, the inner EVM call's bump goes too.
    let revert = call(&chain, &authority(), bridge, 0, calldata("addThenRevert(bytes32,uint64)", &[contract_word(&echo), word(5)]));
    let tried = next_nonce(call(&chain, &authority(), bridge, 0, calldata("tryAdd(bytes32,uint64)", &[contract_word(&echo), word(5)])), &authority());
    let receipts = advance(&mut chain, vec![revert, tried]);

    assert!(receipts[0].result.is_err());
    assert_eq!(returned(&receipts[1]), word(0));
    assert_eq!(bridge_count(&chain, &bridge), word(14), "only tryAdd's own 10");
}

#[test]
#[cfg(all(feature = "lvm", feature = "wasm"))]
fn webassembly_contracts_call_the_evm_and_speak_its_abi() {
    let db = Arc::new(MemoryDb::default());
    let mut chain = open(&db);
    let (bridge, _, _) = bridged(&mut chain);
    let wasm = sign(&chain, &authority(), Tx::Deploy { code: Code::Wasm(crate::vm::tests::example("evm")) });
    let Ok(Effect::Deployed(caller)) = advance(&mut chain, vec![wasm]).remove(0).result else { panic!("the example deploys") };
    let bump = lua_call(&chain, caller, "bump", vec![TxCallArgumentType::Address(bridge), TxCallArgumentType::U64(6)]);
    let receipt = advance(&mut chain, vec![bump]).remove(0);

    assert_eq!(receipt.result, Ok(Effect::Returned(vec![TxCallArgumentType::U256(u256::new(6))])), "{:?}", receipt.result);
    assert_eq!(bridge_count(&chain, &bridge), word(6));

    // A revert comes back with Solidity's reason read out.
    let counter = deploy(&mut chain, 0);
    let why = lua_call(&chain, caller, "why", vec![TxCallArgumentType::Address(counter), TxCallArgumentType::Bytes(calldata("bump(uint256)", &[word(0)]))]);
    let receipt = advance(&mut chain, vec![why]).remove(0);

    assert!(matches!(&receipt.result, Ok(Effect::Returned(values)) if values == &[TxCallArgumentType::String("reverted: nothing to add".to_string())]), "{:?}", receipt.result);
}

const ABI_USER: &str = r#"
local M = {}

-- Calldata for transfer(address,uint256), as Lua writes it.
function M.transfer(to, amount)
    return evm.encode("transfer(address,uint256)", to, amount)
end

-- Arrays and tuples as tables, there and back.
function M.round_trip()
    local bytes = evm.encode("(uint64[],(bool,string),bytes2)", { u64(1), 2, 3 }, { true, "hi" }, "ab")
    local xs, pair, two = evm.decode("(uint64[],(bool,string),bytes2)", bytes)
    local to, amount = evm.decode("transfer(address,uint256)", evm.encode("transfer(address,uint256)", ctx.sender, u256(9)))
    return #xs, xs[3], pair[1], pair[2], two, to, amount
end

-- What does not fit its type is refused.
function M.too_big()
    return evm.encode("(uint8)", 256)
end

return M
"#;

#[test]
fn lua_speaks_solidity_s_abi() {
    use crate::abi::{AbiType, AbiValue, encode_values};

    let db = Arc::new(MemoryDb::default());
    let mut chain = open(&db);
    let user = deploy_lua(&mut chain, ABI_USER);
    let alice = Keypair::generate(KeyType::Ed25519).unwrap().public().address();
    let calls = vec![
        lua_call(&chain, user, "transfer", vec![TxCallArgumentType::Address(alice), TxCallArgumentType::U256(u256::new(5))]),
        next_nonce(lua_call(&chain, user, "round_trip", Vec::new()), &authority()),
        next_nonce(next_nonce(lua_call(&chain, user, "too_big", Vec::new()), &authority()), &authority()),
    ];
    let receipts = advance(&mut chain, calls);
    let expected = [
        selector("transfer(address,uint256)"),
        encode_values(&[AbiType::Address, AbiType::Uint(256)], &[AbiValue::Address(evm::evm_alias(&alice)), AbiValue::Uint(u256::new(5))]).unwrap(),
    ]
    .concat();

    assert_eq!(receipts[0].result, Ok(Effect::Returned(vec![TxCallArgumentType::Bytes(expected)])));
    assert_eq!(evm::account_of(&chain.state(), evm::evm_alias(&alice)).unwrap(), alice, "encoding her address wrote her alias down");
    assert_eq!(
        receipts[1].result,
        Ok(Effect::Returned(vec![
            TxCallArgumentType::I64(3),
            TxCallArgumentType::U64(3),
            TxCallArgumentType::Bool(true),
            TxCallArgumentType::String("hi".to_string()),
            TxCallArgumentType::String("ab".to_string()),
            TxCallArgumentType::Address(authority().public().address()),
            TxCallArgumentType::U256(u256::new(9)),
        ])),
        "{:?}",
        receipts[1].result
    );
    assert!(matches!(&receipts[2].result, Err(failure) if failure.to_string().contains("uint8")), "{:?}", receipts[2].result);
}

#[cfg(feature = "swap")]
mod swap {
    //! Swap.sol, the EVM chains' side of a swap, run here on chainv4's EVM as it runs on Ethereum,
    //! with contracts/evm/Token.sol for an ERC-20.

    use {
        super::*,
        crate::abi::{AbiType, AbiValue, encode_values},
        crate::swap::evm::{get_input, init_code, read_lock},
        crate::swap::{EvmLeg, Leg, LockState},
    };

    const TOKEN_BIN: &str = include_str!("../../../contracts/evm/Token.bin");

    fn create(chain: &mut Chain, init: Vec<u8>) -> [u8; 20] {
        let deployer = authority();
        let at = evm::create_address(&deployer.public().address(), nonce(chain, &deployer));
        let receipts = advance(chain, vec![sign(chain, &deployer, Tx::Deploy { code: Code::Evm(init) })]);
        assert_eq!(receipts[0].result, Ok(Effect::Deployed(at)));

        match at {
            Address::Evm(bytes) => bytes,
            other => panic!("an EVM contract at {other}"),
        }
    }

    fn run(chain: &mut Chain, to: [u8; 20], value: u128, input: Vec<u8>) -> Receipt {
        let tx = call(chain, &authority(), Address::Evm(to), value, input);

        advance(chain, vec![tx]).remove(0)
    }

    fn refused(receipt: &Receipt, why: &str) {
        assert!(format!("{:?}", receipt.result).contains(why), "{:?}", receipt.result);
    }

    fn token_balance(chain: &mut Chain, token: [u8; 20], owner: [u8; 20]) -> u256 {
        let input = calldata("balanceOf(address)", &[[vec![0; 12], owner.to_vec()].concat()]);

        u256::from_be_bytes(returned(&run(chain, token, 0, input)).try_into().unwrap())
    }

    #[test]
    fn swap_sol_pays_a_token_with_the_secret_and_the_coin_back_after_the_deadline() {
        let db = Arc::new(MemoryDb::default());
        let mut chain = open(&db);
        let me = authority().public().address();
        let swap = create(&mut chain, init_code());
        let supply = encode_values(&[AbiType::String, AbiType::String, AbiType::Uint(256)], &[AbiValue::String("Test".into()), AbiValue::String("TST".into()), AbiValue::Uint(u256::new(1_000_000))]).unwrap();
        let token = create(&mut chain, [crate::structs::hex::from_hex(TOKEN_BIN.trim()).unwrap(), supply].concat());
        let receiver = [0xb2; 20];
        let leg = EvmLeg { chain_id: 1, contract: swap, token: Some(token), amount: u256::new(1_000), payer: Some(evm::evm_alias(&me)), receiver: Some(receiver) };
        let secret = [0x5e; 32];
        let hash = crate::crypto::bitcoin::sha256(&secret);
        let deadline = chain.tip().timestamp + 30 * SLOT;
        let [approve, lock] = <[_; 2]>::try_from(leg.lock_calls(&hash, deadline).unwrap()).unwrap();

        // The token moves only once approved; then it is locked, all of it.
        refused(&run(&mut chain, swap, 0, lock.input.clone()), "the token would not move");
        assert!(run(&mut chain, token, 0, approve.input).result.is_ok());
        assert!(run(&mut chain, swap, 0, lock.input.clone()).result.is_ok());
        assert_eq!(token_balance(&mut chain, token, swap), u256::new(1_000));
        refused(&run(&mut chain, swap, 0, lock.input), "already exists");

        let locked = read_lock(&returned(&run(&mut chain, swap, 0, get_input(&hash)))).unwrap();
        assert_eq!(locked.state, LockState::Locked);
        assert_eq!(Leg::Evm(leg.clone()).stands(&locked, deadline), Ok(()));

        // The wrong secret claims nothing; the right one pays the receiver and keeps the secret.
        refused(&run(&mut chain, swap, 0, leg.claim_call(&[0; 32]).input), "no swap waits");
        assert!(run(&mut chain, swap, 0, leg.claim_call(&secret).input).result.is_ok());
        assert_eq!(token_balance(&mut chain, token, receiver), u256::new(1_000));
        let claimed = read_lock(&returned(&run(&mut chain, swap, 0, get_input(&hash)))).unwrap();
        assert_eq!((claimed.state, claimed.secret), (LockState::Claimed, Some(secret)));
        refused(&run(&mut chain, swap, 0, leg.claim_call(&secret).input), "no swap waits");
        refused(&run(&mut chain, swap, 0, leg.refund_call(&hash).input), "no swap is locked");

        // The coin, locked until a few blocks on: too early to refund, then too late to claim,
        // and back to who locked it.
        let coin = EvmLeg { token: None, amount: u256::new(500), ..leg.clone() };
        let other = [0x6f; 32];
        let other_hash = crate::crypto::bitcoin::sha256(&other);
        let soon = chain.tip().timestamp + 4 * SLOT;
        let before = balance(&chain, &me);
        let lock = coin.lock_calls(&other_hash, soon).unwrap().remove(0);
        assert!(run(&mut chain, swap, 500, lock.input).result.is_ok());
        assert_eq!(balance(&chain, &me), before - u256::new(500));
        refused(&run(&mut chain, swap, 0, coin.refund_call(&other_hash).input), "too early");

        while chain.tip().timestamp / 1_000 < soon / 1_000 {
            advance(&mut chain, Vec::new());
        }

        refused(&run(&mut chain, swap, 0, coin.claim_call(&other).input), "too late");
        assert!(run(&mut chain, swap, 0, coin.refund_call(&other_hash).input).result.is_ok());
        assert_eq!(balance(&chain, &me), before);
        assert_eq!(read_lock(&returned(&run(&mut chain, swap, 0, get_input(&other_hash)))).unwrap().state, LockState::Refunded);

        // What does not hold locks nothing.
        let third = crate::crypto::bitcoin::sha256(&[1; 32]);
        let past = coin.lock_calls(&third, chain.tip().timestamp).unwrap().remove(0);
        refused(&run(&mut chain, swap, 1, past.input), "after this block's time");
        let nothing = coin.lock_calls(&third, deadline + 60 * SLOT).unwrap().remove(0);
        refused(&run(&mut chain, swap, 0, nothing.input), "lock something");
        assert_eq!(read_lock(&returned(&run(&mut chain, swap, 0, get_input(&third)))).unwrap().state, LockState::None);
    }
}
