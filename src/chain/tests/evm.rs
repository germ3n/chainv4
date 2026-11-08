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
const TOKEN_BIN: &str = include_str!("../../../contracts/evm/Token.bin");

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
    SignedTx::sign(tx, nonce(chain, from), FeeOffer::NONE, from, &chain.genesis())
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
fn next_nonce(genesis: &Hash, signed: SignedTx, from: &Keypair) -> SignedTx {
    SignedTx::sign(signed.tx, signed.nonce + 1, FeeOffer::NONE, from, genesis)
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

/// Deploys the ERC-20, its whole supply to the authority, whose Transfer from nowhere is its first
/// log.
fn deploy_token(chain: &mut Chain) -> Address {
    use crate::abi::{AbiType, AbiValue, encode_values};

    let deployer = authority();
    let args = encode_values(&[AbiType::String, AbiType::String, AbiType::Uint(256)], &[AbiValue::String("Token".to_string()), AbiValue::String("TKN".to_string()), AbiValue::Uint(u256::new(1_000))]).unwrap();
    let init = [crate::structs::hex::from_hex(TOKEN_BIN.trim()).unwrap(), args].concat();
    let token = evm::create_address(&deployer.public().address(), nonce(chain, &deployer));
    let deploy = sign(chain, &deployer, Tx::Deploy { code: Code::Evm(init) });

    assert_eq!(advance(chain, vec![deploy])[0].result, Ok(Effect::Deployed(token)));

    token
}

fn send_token(chain: &Chain, token: Address, to: &Address, value: u128) -> SignedTx {
    call(chain, &authority(), token, 0, calldata("transfer(address,uint256)", &[address_word(to), word(value)]))
}

fn found(db: &MemoryDb, filter: EventFilter) -> Vec<Event> {
    stored_events(db, &filter, None, 100).unwrap().0.into_iter().map(|found| found.event).collect()
}

#[test]
fn an_erc20_s_logs_are_found_by_their_name_their_topic_and_the_addresses_in_them() {
    let db = Arc::new(MemoryDb::default());
    let mut chain = open(&db);
    chain.keep_index(IndexKind::Events, None).unwrap();

    let me = authority().public().address();
    let (alice, secp) = (stranger().public().address(), Address::Secp256k1([7; 20]));
    let token = deploy_token(&mut chain);
    let sent = send_token(&chain, token, &alice, 5);
    advance(&mut chain, vec![sent]);
    let sent = send_token(&chain, token, &secp, 6);
    advance(&mut chain, vec![sent]);
    let approved = call(&chain, &authority(), token, 0, calldata("approve(address,uint256)", &[address_word(&alice), word(9)]));
    advance(&mut chain, vec![approved]);

    let transfer: [u8; 32] = keccak256(b"Transfer(address,address,uint256)").0;

    // By its name, spelled as the EVM names logs, and no other way: names match by bytes.
    assert_eq!(found(&db, EventFilter::Named(token, "log".to_string())).len(), 4, "the mint, two transfers and an approval");
    assert!(found(&db, EventFilter::Named(token, "LOG".to_string())).is_empty());

    // By its signature, from any contract or from one.
    assert_eq!(found(&db, EventFilter::Topic(transfer)).len(), 3);
    assert_eq!(found(&db, EventFilter::ContractTopic(token, transfer)).len(), 3);
    assert!(found(&db, EventFilter::ContractTopic(Address::Evm([9; 20]), transfer)).is_empty());

    // By who it names: a secp256k1 account by its own 20 bytes, an Ed25519 one by its alias.
    let to_secp = found(&db, EventFilter::Mentions(secp));
    let (topics, data) = to_secp[0].evm_log().unwrap();

    assert_eq!(to_secp.len(), 1);
    assert_eq!((topics[2].to_vec(), data.to_vec()), (address_word(&secp), word(6)));
    assert_eq!(found(&db, EventFilter::Mentions(alice)).len(), 2, "its transfer in, and what it was let spend");
    assert_eq!(found(&db, EventFilter::Mentions(me)).len(), 4);

    // A topic that is an address is found as one, in any place.
    let alice_topic: [u8; 32] = address_word(&alice).try_into().unwrap();
    assert_eq!(found(&db, EventFilter::Topic(alice_topic)).len(), 2);

    // Read as what they fit.
    let (topics, data) = to_secp[0].evm_log().unwrap();
    let known = crate::abi::known::known_abis();
    let read = crate::abi::known::read_log(None, &known, &topics, data).unwrap();

    assert_eq!((read.name.as_str(), read.by.label().as_str(), read.moves[0].amount), ("Transfer", "read as ERC-20", u256::new(6)));

    // A page at a time across both places an address can be.
    let (first, next) = stored_events(&*db, &EventFilter::Mentions(me), None, 3).unwrap();
    let (rest, after) = stored_events(&*db, &EventFilter::Mentions(me), next.as_deref(), 3).unwrap();

    assert_eq!((first.len(), rest.len(), after), (3, 1, None));
    assert!(first.iter().map(|found| found.height).is_sorted_by(|newer, older| newer >= older));
    assert!(first.last().unwrap().height > rest[0].height);

    // Only an EVM contract's logs are logs, whatever a contract calls its event.
    let named_log = Event { contract: Address::Contract([1; 32]), name: LOG.to_string(), data: vec![TxCallArgumentType::Bytes(vec![1; 32]), TxCallArgumentType::Bytes(Vec::new())] };

    assert_eq!(named_log.evm_log(), None);
    assert_eq!(Event { contract: token, name: LOG.to_string(), data: Vec::new() }.evm_log(), None);
}

#[test]
fn a_log_s_topics_leave_the_index_with_its_block() {
    let db = Arc::new(MemoryDb::default());
    let mut chain = open(&db);
    chain.keep_index(IndexKind::Events, None).unwrap();

    let token = deploy_token(&mut chain);
    let alice = stranger().public().address();
    let sent = send_token(&chain, token, &alice, 5);
    advance(&mut chain, vec![sent]);
    let transfer: [u8; 32] = keccak256(b"Transfer(address,address,uint256)").0;

    assert_eq!(found(&db, EventFilter::Topic(transfer)).len(), 2);

    // Only the latest block kept: the mint's entries go, its pointers included, with nothing left
    // pointing at nothing.
    chain.keep_index(IndexKind::Events, Some(KeepFrom::Latest(1))).unwrap();
    while chain.catch_up_index(IndexKind::Events).unwrap() {}

    assert_eq!(found(&db, EventFilter::Topic(transfer)).len(), 1);
    assert_eq!(found(&db, EventFilter::ContractTopic(token, transfer)).len(), 1);
    assert_eq!(found(&db, EventFilter::Mentions(alice)).len(), 1);
}

#[test]
fn an_index_from_before_topics_starts_over_keeping_what_it_kept_and_reads_them_back() {
    let db = Arc::new(MemoryDb::default());
    let mut chain = open(&db);
    chain.keep_index(IndexKind::Events, Some(KeepFrom::Height(1))).unwrap();

    let token = deploy_token(&mut chain);
    let alice = stranger().public().address();
    let sent = send_token(&chain, token, &alice, 5);
    advance(&mut chain, vec![sent]);
    while chain.catch_up_index(IndexKind::Events).unwrap() {}
    let transfer: [u8; 32] = keccak256(b"Transfer(address,address,uint256)").0;
    let tip = chain.tip().height;

    assert_eq!(found(&db, EventFilter::Topic(transfer)).len(), 2);

    // As a node left it before topics: no pointers by them, and nothing to say what its entries
    // hold.
    let old = State::new(db.clone());
    let mut keys = Vec::new();

    db.scan(Column::Events, &[], &[], &mut |key, _| {
        if matches!(key[0], b't' | b'k') {
            keys.push(key.to_vec());
        }

        true
    })
    .unwrap();

    assert!(!keys.is_empty());

    for key in keys {
        old.delete(Column::Events, key);
    }

    old.delete(Column::Meta, b"events_schema".to_vec());
    db.commit(old.take_changes()).unwrap();
    assert!(found(&db, EventFilter::Topic(transfer)).is_empty(), "which is what would stay wrong");
    drop(chain);

    // Opening it starts it over: nothing below the next block, keeping the same blocks.
    let mut chain = open(&db);
    let window = chain.index_window(IndexKind::Events).unwrap();

    assert_eq!((window.from, window.keep), (tip + 1, KeepFrom::Height(1)));
    assert_eq!(stored_index(&*db, IndexKind::Events).unwrap(), Some(window), "and writes that down");
    assert!(found(&db, EventFilter::Contract(token)).is_empty(), "every entry went, in one write");

    // A new block's log is indexed right away, and the rest catch up from their receipts.
    let sent = send_token(&chain, token, &alice, 1);
    advance(&mut chain, vec![sent]);
    assert_eq!(found(&db, EventFilter::Topic(transfer)).len(), 1);

    while chain.catch_up_index(IndexKind::Events).unwrap() {}

    assert_eq!(found(&db, EventFilter::Topic(transfer)).len(), 3);
    assert_eq!(found(&db, EventFilter::Named(token, LOG.to_string())).len(), 3);
    assert_eq!(chain.index_window(IndexKind::Events).unwrap().from, 1);

    // It only starts over the once.
    drop(chain);
    let chain = open(&db);

    assert_eq!(chain.index_window(IndexKind::Events).unwrap().from, 1);
    assert_eq!(found(&db, EventFilter::Topic(transfer)).len(), 3);
}

#[test]
fn indexing_logs_topics_touches_neither_blocks_nor_state() {
    let (indexed_db, plain_db) = (Arc::new(MemoryDb::default()), Arc::new(MemoryDb::default()));
    let (mut indexed, mut plain) = (open(&indexed_db), open(&plain_db));
    indexed.keep_index(IndexKind::Events, None).unwrap();

    let init = [crate::structs::hex::from_hex(COUNTER_BIN.trim()).unwrap(), word(5)].concat();
    let counter = evm::create_address(&authority().public().address(), 0);
    let deploy = sign(&indexed, &authority(), Tx::Deploy { code: Code::Evm(init) });
    let mut blocks = vec![deploy];

    blocks.push(next_nonce(&indexed.genesis(), call(&indexed, &authority(), counter, 0, calldata("bump(uint256)", &[word(3)])), &authority()));

    for tx in blocks {
        let (block, now) = next_block(&indexed, vec![tx]);

        extended(indexed.import(&block, now));
        // The same block, with every root its header commits to, is good for a node with no index.
        extended(plain.import(&block, now));
        assert_eq!(indexed.tip(), plain.tip());
    }

    assert_eq!(found(&indexed_db, EventFilter::Named(counter, LOG.to_string())).len(), 1);
    assert!(stored_index(&*plain_db, IndexKind::Events).unwrap().is_none());
    assert_eq!(indexed.state().read(Column::Accounts, &crate::db::address_key(&authority().public().address())).unwrap(), plain.state().read(Column::Accounts, &crate::db::address_key(&authority().public().address())).unwrap());
}

#[cfg(feature = "bitcoin")]
#[test]
fn a_transaction_reaches_the_crypto_precompile() {
    let db = Arc::new(MemoryDb::default());
    let mut chain = open(&db);
    let input = [&keccak256(b"sha256d(bytes)")[..4], &word(32), &word(3), &[b"abc".as_slice(), &[0; 29]].concat()].concat();
    let asked = call(&chain, &authority(), Address::Evm(evm::CRYPTO_PRECOMPILE), 0, input);
    let receipt = advance(&mut chain, vec![asked]).remove(0);

    assert_eq!(returned(&receipt), crate::crypto::bitcoin::sha256d(b"abc"));
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
    let genesis_id = chain.genesis();
    let me = authority().public().address();
    let ethereum = Keypair::generate(KeyType::Secp256k1).unwrap().public().address();
    let counter = deploy(&mut chain, 0);
    let before = balance(&chain, &me);

    let deposit = call(&chain, &authority(), counter, 1_000, calldata("deposit()", &[]));
    let deposited = advance(&mut chain, vec![deposit]).remove(0);

    assert!(deposited.payments.is_empty(), "what the sender sent is the transaction's own, not a payout");

    assert_eq!(balance(&chain, &counter), u256::new(1_000));
    assert_eq!(balance(&chain, &me), before - u256::new(1_000));

    // Paid out to the Ed25519 sender's alias, it reaches the sender; to a secp256k1 account, that.
    let pay_me = call(&chain, &authority(), counter, 0, calldata("pay(address,uint256)", &[address_word(&me), word(400)]));
    let pay_them = call(&chain, &authority(), counter, 0, calldata("pay(address,uint256)", &[address_word(&ethereum), word(100)]));
    let pay_them = next_nonce(&genesis_id, pay_them, &authority());
    let paid = advance(&mut chain, vec![pay_me, pay_them]);

    assert!(paid.iter().all(|receipt| receipt.result.is_ok()), "{paid:?}");
    // The EVM's own payouts, each told as the account it reached: the Ed25519 sender under its
    // alias, the secp256k1 account by its own address.
    let payment = |to: Address, amount: u128| crate::structs::receipt::Payment { from: counter, to, amount: u256::new(amount) };

    assert_eq!(paid[0].payments, [payment(me, 400)]);
    assert_eq!(paid[1].payments, [payment(ethereum, 100)]);
    assert_eq!(crate::structs::receipt::CommittedReceipt::of(&paid[0]).payments, paid[0].payments, "committed in the receipts root");

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
    assert!(receipt.payments.is_empty());
}

#[test]
fn any_transaction_writes_its_senders_alias_down_so_the_evm_can_pay_it() {
    let db = Arc::new(MemoryDb::default());
    let mut chain = open(&db);
    let genesis_id = chain.genesis();
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
            &genesis_id,
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
        &genesis_id,
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
        next_nonce(&genesis_id, call(&chain, &authority(), counter, 0, calldata("pay(address,uint256)", &[address_word(&carol.public().address()), word(200)])), &authority());
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
    let genesis_id = chain.genesis();
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
        &genesis_id,
    );
    advance(&mut chain, vec![deposit, fund]);

    // Dave signs with his bitcoin key, over double SHA-256; the chain checks it and writes his
    // alias down, as for any account that is not its own 20 EVM bytes.
    let back = SignedTx::sign(
        Tx::Transfer { from: address, to: authority().public().address(), amount: Amount::U128(5) },
        nonce(&chain, &dave),
        FeeOffer::NONE,
        &dave,
        &genesis_id,
    );

    assert!(back.verify_on(&genesis_id).is_ok());
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

    assert_eq!(crate::registry::supply::adds_up(&chain.state()).unwrap(), Ok(()), "minted less burned is what is held and bonded");
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
        // The deployment proxy is EVM code: a chain without the EVM has none.
        preinstalls: Vec::new(),
        forks: Default::default(),
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
    let genesis_id = chain.genesis();
    let counter = deploy(&mut chain, 0);
    let others: Vec<Keypair> = (0..4).map(|_| stranger()).collect();
    let first = nonce(&chain, &authority());
    let funding = others
        .iter()
        .zip(first..)
        .map(|(other, nonce)| {
            let tx = Tx::Transfer { from: authority().public().address(), to: other.public().address(), amount: Amount::U128(1_000) };

            SignedTx::sign(tx, nonce, FeeOffer::NONE, &authority(), &genesis_id)
        })
        .collect();
    advance(&mut chain, funding);

    // Each bumps the same counter and deposits: every one reads what the one before wrote.
    let mut txs = Vec::new();

    for other in &others {
        txs.push(call(&chain, other, counter, 0, calldata("bump(uint256)", &[word(2)])));
        txs.push(next_nonce(&genesis_id, call(&chain, other, counter, 10, calldata("deposit()", &[])), other));
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
    let genesis_id = chain.genesis();
    let fees = paid.fees.clone().unwrap().build();
    let offer = FeeOffer { gas_limit: 300_000, max_price: u256::new(1_000_000_000), tip: u256::ZERO };
    let init = [crate::structs::hex::from_hex(COUNTER_BIN.trim()).unwrap(), word(0)].concat();
    // Storing code costs gas by the byte, as on Ethereum.
    let deploy_offer = FeeOffer { gas_limit: 3_000_000, ..offer };
    let deploy = SignedTx::sign(Tx::Deploy { code: Code::Evm(init) }, nonce(&chain, &authority()), deploy_offer, &authority(), &genesis_id);
    let counter = evm::create_address(&authority().public().address(), deploy.nonce);
    let deployed = advance(&mut chain, vec![deploy]).remove(0);

    assert_eq!(deployed.result, Ok(Effect::Deployed(counter)));

    let bump = SignedTx::sign(
        Tx::EvmCall { to: counter, value: Amount::U128(0), input: calldata("bump(uint256)", &[word(1)]) },
        nonce(&chain, &authority()),
        offer,
        &authority(),
        &genesis_id,
    );
    let intrinsic = fees.schedule().intrinsic(&bump);
    let receipt = advance(&mut chain, vec![bump]).remove(0);
    let evm_gas = fees.schedule().for_fuel(receipt.fuel_used);

    assert!(receipt.result.is_ok(), "{:?}", receipt.result);
    assert!(evm_gas > 21_000, "the EVM's own gas, its intrinsic 21,000 included: {evm_gas}");
    assert_eq!(receipt.gas_used, intrinsic + evm_gas);
    assert!(receipt.fee > u256::ZERO);

    assert_eq!(crate::registry::supply::adds_up(&chain.state()).unwrap(), Ok(()), "minted less burned is what is held and bonded");
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
    let genesis_id = chain.genesis();
    // Alice holds an Ed25519 key, and has never sent an EVM transaction: a name resolves to her
    // alias all the same.
    let alice = Keypair::generate(KeyType::Ed25519).unwrap();
    let fund = SignedTx::sign(
        Tx::Transfer { from: authority().public().address(), to: alice.public().address(), amount: Amount::U128(10u128.pow(21)) },
        nonce(&chain, &authority()),
        FeeOffer::NONE,
        &authority(),
        &genesis_id,
    );
    advance(&mut chain, vec![fund]);
    let register = sign(&chain, &alice, Tx::Name { name: "alice".into(), action: NameAction::Register { years: 1 } });
    let primary = next_nonce(&genesis_id, sign(&chain, &alice, Tx::PrimaryName { name: Some("alice".into()) }), &alice);
    let payer = evm::create_address(&authority().public().address(), nonce(&chain, &authority()));
    let deploy = sign(&chain, &authority(), Tx::Deploy { code: Code::Evm(crate::structs::hex::from_hex(NAME_PAYER_BIN.trim()).unwrap()) });
    let receipts = advance(&mut chain, vec![register, primary, deploy]);

    assert!(receipts.iter().all(|receipt| receipt.result.is_ok()), "{receipts:?}");

    let before = balance(&chain, &alice.public().address());
    let pay = call(&chain, &authority(), payer, 1_000, calldata("pay(string)", &[string_arg("Alice.dev")]));
    let lookup = next_nonce(&genesis_id, call(&chain, &authority(), payer, 0, calldata("lookup(string)", &[string_arg("alice")])), &authority());
    let receipts = advance(&mut chain, vec![pay, lookup]);

    assert_eq!(receipts[0].result.as_ref().map(|_| ()), Ok(()), "{:?}", receipts[0].result);
    assert_eq!(balance(&chain, &alice.public().address()), before + u256::new(1_000), "paid to her own account, through her alias");
    assert_eq!(returned(&receipts[1])[12..], evm::evm_alias(&alice.public().address()), "the name resolves to her EVM address");

    // The other way: her alias goes by her primary name. Nobody's resolves to nothing.
    let whois = call(&chain, &authority(), payer, 0, calldata("whois(address)", &[address_word(&alice.public().address())]));
    let nobody = next_nonce(&genesis_id, call(&chain, &authority(), payer, 5, calldata("pay(string)", &[string_arg("nobody.dev")])), &authority());
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
        &chain.genesis(),
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
        authorizations: Vec::new(),
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

/// An EIP-7702 transaction, signed by one key, carrying authorizations other keys signed.
#[cfg(feature = "secp256k1")]
fn set_code(chain: &Chain, secret: &[u8; 32], nonce: u64, to: [u8; 20], input: Vec<u8>, authorizations: Vec<crate::crypto::ethereum::Authorization>) -> SignedTx {
    use crate::crypto::ethereum::{EthereumKind, EthereumTx};

    let unsigned = EthereumTx {
        kind: EthereumKind::SetCode,
        chain_id: evm::chain_id(&chain.genesis()),
        nonce,
        max_price: u256::ZERO,
        tip: u256::ZERO,
        gas_limit: 3_000_000,
        to: Some(to),
        value: u256::ZERO,
        input,
        access_list: Vec::new(),
        signature: [0; 64],
        parity: 0,
        authorizations,
    };

    SignedTx::from_ethereum(unsigned.signed(secret).encode()).unwrap()
}

#[cfg(feature = "secp256k1")]
fn authorize(chain: &Chain, secret: &[u8; 32], to: [u8; 20], nonce: u64) -> crate::crypto::ethereum::Authorization {
    crate::crypto::ethereum::Authorization { chain_id: u256::from(evm::chain_id(&chain.genesis())), address: to, nonce, parity: 0, signature: [0; 64] }.signed(secret)
}

#[cfg(feature = "secp256k1")]
fn key_of(secret: [u8; 32]) -> Keypair {
    Keypair::from_private(crate::crypto::keypair::PrivateKey::Secp256k1(secret))
}

#[cfg(feature = "secp256k1")]
#[test]
fn an_account_that_delegates_by_eip_7702_runs_its_delegates_code_on_its_own_storage() {
    let db = Arc::new(MemoryDb::default());
    let mut chain = Chain::open(db, &genesis()).unwrap();
    let counter = deploy(&mut chain, 5);
    let Address::Evm(target) = counter else { unreachable!() };
    let (alice_secret, bob_secret) = ([0x51; 32], [0x52; 32]);
    let alice = ethereum_account(&mut chain, alice_secret);
    ethereum_account(&mut chain, bob_secret);
    let Address::Secp256k1(alice_bytes) = alice else { unreachable!() };

    // Bob sends it: Alice's signature lets her account run the counter's code, and his call bumps
    // her own count, which starts at nothing, not the counter's 5.
    let signed = authorize(&chain, &alice_secret, target, nonce(&chain, &key_of(alice_secret)));
    let tx = set_code(&chain, &bob_secret, nonce(&chain, &key_of(bob_secret)), alice_bytes, calldata("bump(uint256)", &[word(3)]), vec![signed]);
    let receipt = advance(&mut chain, vec![tx]).remove(0);

    assert!(receipt.result.is_ok(), "{receipt:?}");
    assert_eq!(evm::delegated_to(&chain.state(), alice_bytes).unwrap(), Some(target));
    assert_eq!(count(&mut chain, alice), word(3), "her own storage");
    assert_eq!(count(&mut chain, counter), word(5), "the counter's untouched");
    assert_eq!(nonce(&chain, &key_of(alice_secret)), 1, "her authorization took her nonce");

    // She is still an account: she sends a plain transfer, signed natively.
    let pay = SignedTx::sign(Tx::Transfer { from: alice, to: Address::Secp256k1([9; 20]), amount: Amount::U128(10) }, 1, FeeOffer::NONE, &key_of(alice_secret), &chain.genesis());
    assert!(advance(&mut chain, vec![pay])[0].result.is_ok());
}

#[cfg(feature = "secp256k1")]
#[test]
fn an_authorization_stands_when_the_call_reverts_and_its_own_sender_bumps_its_nonce_twice() {
    let db = Arc::new(MemoryDb::default());
    let mut chain = Chain::open(db, &genesis()).unwrap();
    let counter = deploy(&mut chain, 5);
    let Address::Evm(target) = counter else { unreachable!() };
    let secret = [0x53; 32];
    let account = ethereum_account(&mut chain, secret);
    let Address::Secp256k1(bytes) = account else { unreachable!() };

    // Her own transaction, at nonce 0, so her authorization is for 1, as Ethereum has it; the call
    // adds nothing and reverts, but the delegation and both nonces stand.
    let signed = authorize(&chain, &secret, target, 1);
    let tx = set_code(&chain, &secret, 0, bytes, calldata("bump(uint256)", &[word(0)]), vec![signed]);
    let receipt = advance(&mut chain, vec![tx]).remove(0);

    assert_eq!(receipt.result, Err(Failure::Contract(VmError::Runtime("reverted: nothing to add".to_string()))));
    assert_eq!(evm::delegated_to(&chain.state(), bytes).unwrap(), Some(target), "kept through the revert");
    assert_eq!(nonce(&chain, &key_of(secret)), 2, "the transaction's and the authorization's");

    // An authorization for the address zero clears it; one for a used nonce, or another chain, is
    // skipped and leaves things as they are.
    let sponsor = [0x54; 32];
    ethereum_account(&mut chain, sponsor);
    let stale = authorize(&chain, &secret, [7; 20], 0);
    let elsewhere = crate::crypto::ethereum::Authorization { chain_id: u256::from(1u64), address: [7; 20], nonce: 2, parity: 0, signature: [0; 64] }.signed(&secret);
    let skipped = set_code(&chain, &sponsor, nonce(&chain, &key_of(sponsor)), bytes, calldata("count()", &[]), vec![stale, elsewhere]);
    assert!(advance(&mut chain, vec![skipped])[0].result.is_ok());
    assert_eq!(evm::delegated_to(&chain.state(), bytes).unwrap(), Some(target), "neither took");
    assert_eq!(nonce(&chain, &key_of(secret)), 2);

    let cleared = set_code(&chain, &sponsor, nonce(&chain, &key_of(sponsor)), bytes, Vec::new(), vec![authorize(&chain, &secret, [0; 20], 2)]);
    assert!(advance(&mut chain, vec![cleared])[0].result.is_ok());
    assert_eq!(evm::delegation(&chain.state(), bytes).unwrap(), None, "cleared");
    assert_eq!(nonce(&chain, &key_of(secret)), 3);
}

#[test]
fn ethereum_signatures_hold_on_their_own_chain_alone_and_say_what_they_carry() {
    use crate::crypto::ethereum::{EthereumKind, decode};
    use crate::crypto::verifiable::VerifyError;

    let db = Arc::new(MemoryDb::default());
    let chain = Chain::open(db, &genesis()).unwrap();
    let genesis_id = chain.genesis();
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
    let disguised = SignedTx::sign(good.tx.clone(), 0, FeeOffer::NONE, &authority(), &genesis_id);

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
    let genesis_id = chain.genesis();
    let (bridge, counter, _) = bridged(&mut chain);
    let add = call(&chain, &authority(), bridge, 300, calldata("add(bytes32,uint64)", &[contract_word(&counter), word(5)]));
    let receipt = advance(&mut chain, vec![add]).remove(0);

    assert_eq!(returned(&receipt), word(5), "{:?}", receipt.result);
    assert_eq!(total(&chain, &counter).as_deref(), Some("5"));
    assert_eq!(balance(&chain, &counter), u256::new(300), "what the EVM call carried reached the native contract");
    assert_eq!(receipt.payments, [crate::structs::receipt::Payment { from: bridge, to: counter, amount: u256::new(300) }], "the bridge paid it on, through the native precompile");
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
    let touch = next_nonce(&genesis_id, call(&chain, &authority(), bridge, 0, calldata("peek(bytes32,string)", &[contract_word(&counter), word(64), word(5), [b"touch".to_vec(), vec![0; 27]].concat()])), &authority());
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
    assert!(receipt.payments.is_empty());

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
    let genesis_id = chain.genesis();
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
    let tried = next_nonce(&genesis_id, call(&chain, &authority(), bridge, 0, calldata("tryAdd(bytes32,uint64)", &[contract_word(&echo), word(5)])), &authority());
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

-- A bytes[] with every offset at one long value, which fuel pays for only once.
function M.shared()
    local function word(number)
        return evm.encode("(uint256)", u256(number))
    end

    local count, long = 2048, 262144
    local bytes = word(32) .. word(count) .. string.rep(word(count * 32), count) .. word(long) .. string.rep("x", long)

    return #evm.decode("(bytes[])", bytes)
end

return M
"#;

#[test]
fn lua_speaks_solidity_s_abi() {
    use crate::abi::{AbiType, AbiValue, encode_values};

    let db = Arc::new(MemoryDb::default());
    let mut chain = open(&db);
    let genesis_id = chain.genesis();
    let user = deploy_lua(&mut chain, ABI_USER);
    let alice = Keypair::generate(KeyType::Ed25519).unwrap().public().address();
    let calls = vec![
        lua_call(&chain, user, "transfer", vec![TxCallArgumentType::Address(alice), TxCallArgumentType::U256(u256::new(5))]),
        next_nonce(&genesis_id, lua_call(&chain, user, "round_trip", Vec::new()), &authority()),
        next_nonce(&genesis_id, next_nonce(&genesis_id, lua_call(&chain, user, "too_big", Vec::new()), &authority()), &authority()),
        next_nonce(&genesis_id, next_nonce(&genesis_id, next_nonce(&genesis_id, lua_call(&chain, user, "shared", Vec::new()), &authority()), &authority()), &authority()),
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
    assert!(matches!(&receipts[3].result, Err(failure) if failure.to_string().contains("honestly")), "fails like any bad bytes, and nothing more is made: {:?}", receipts[3].result);
}

#[cfg(feature = "swap")]
mod swap {
    //! Swap.sol, the EVM chains' side of a swap, run here on chainv4's EVM as it runs on Ethereum,
    //! with contracts/evm/Token.sol for an ERC-20.

    use {
        super::*,
        crate::abi::{AbiType, AbiValue, encode_values},
        crate::swap::evm::{get_input, init_code, item_input, read_item, read_lock},
        crate::swap::{EvmLeg, Item, Leg, LockState},
    };

    const TOKEN_BIN: &str = include_str!("../../../contracts/evm/Token.bin");
    const COLLECTIBLE_BIN: &str = include_str!("../../../contracts/evm/Collectible.bin");
    const ITEMS_BIN: &str = include_str!("../../../contracts/evm/Items.bin");

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
        let leg = EvmLeg { item: None, chain_id: 1, contract: swap, token: Some(token), amount: u256::new(1_000), payer: Some(evm::evm_alias(&me)), receiver: Some(receiver) };
        let secret = [0x5e; 32];
        let hash = crate::crypto::bitcoin::sha256(&secret);
        let deadline = chain.tip().timestamp + 30 * SLOT;
        let id = leg.swap_id(&hash, deadline).unwrap();
        let [approve, lock] = <[_; 2]>::try_from(leg.lock_calls(&hash, deadline).unwrap()).unwrap();

        // The token moves only once approved; then it is locked, all of it.
        refused(&run(&mut chain, swap, 0, lock.input.clone()), "the token would not move");
        assert!(run(&mut chain, token, 0, approve.input).result.is_ok());
        assert!(run(&mut chain, swap, 0, lock.input.clone()).result.is_ok());
        assert_eq!(token_balance(&mut chain, token, swap), u256::new(1_000));
        refused(&run(&mut chain, swap, 0, lock.input), "already exists");

        let locked = read_lock(&returned(&run(&mut chain, swap, 0, get_input(&id)))).unwrap();
        assert_eq!(locked.state, LockState::Locked);
        assert_eq!(Leg::Evm(leg.clone()).stands(&locked, deadline), Ok(()));

        // The wrong secret claims nothing; the right one pays the receiver and keeps the secret.
        refused(&run(&mut chain, swap, 0, leg.claim_call(&id, &[0; 32]).input), "no swap waits");
        assert!(run(&mut chain, swap, 0, leg.claim_call(&id, &secret).input).result.is_ok());
        assert_eq!(token_balance(&mut chain, token, receiver), u256::new(1_000));
        let claimed = read_lock(&returned(&run(&mut chain, swap, 0, get_input(&id)))).unwrap();
        assert_eq!((claimed.state, claimed.secret, claimed.owed), (LockState::Claimed, Some(secret), false));
        refused(&run(&mut chain, swap, 0, leg.claim_call(&id, &secret).input), "no swap waits");
        refused(&run(&mut chain, swap, 0, leg.refund_call(&id).input), "no swap is locked");

        // The coin, locked until a few blocks on: too early to refund, then too late to claim,
        // and back to who locked it.
        let coin = EvmLeg { item: None, token: None, amount: u256::new(500), ..leg.clone() };
        let other = [0x6f; 32];
        let other_hash = crate::crypto::bitcoin::sha256(&other);
        let soon = chain.tip().timestamp + 4 * SLOT;
        let other_id = coin.swap_id(&other_hash, soon).unwrap();
        let before = balance(&chain, &me);
        let lock = coin.lock_calls(&other_hash, soon).unwrap().remove(0);
        assert!(run(&mut chain, swap, 500, lock.input).result.is_ok());
        assert_eq!(balance(&chain, &me), before - u256::new(500));
        refused(&run(&mut chain, swap, 0, coin.refund_call(&other_id).input), "too early");

        while chain.tip().timestamp / 1_000 < soon / 1_000 {
            advance(&mut chain, Vec::new());
        }

        refused(&run(&mut chain, swap, 0, coin.claim_call(&other_id, &other).input), "too late");
        assert!(run(&mut chain, swap, 0, coin.refund_call(&other_id).input).result.is_ok());
        assert_eq!(balance(&chain, &me), before);
        assert_eq!(read_lock(&returned(&run(&mut chain, swap, 0, get_input(&other_id)))).unwrap().state, LockState::Refunded);

        // What does not hold locks nothing.
        let third = crate::crypto::bitcoin::sha256(&[1; 32]);
        let past = coin.lock_calls(&third, chain.tip().timestamp).unwrap().remove(0);
        refused(&run(&mut chain, swap, 1, past.input), "after this block's time");
        let nothing = coin.lock_calls(&third, deadline + 60 * SLOT).unwrap().remove(0);
        refused(&run(&mut chain, swap, 0, nothing.input), "lock something");
        assert_eq!(read_lock(&returned(&run(&mut chain, swap, 0, get_input(&coin.swap_id(&third, deadline + 60 * SLOT).unwrap())))).unwrap().state, LockState::None);
    }

    fn word(address: [u8; 20]) -> Vec<u8> {
        [vec![0; 12], address.to_vec()].concat()
    }

    fn number(value: u64) -> Vec<u8> {
        u256::from(value).to_be_bytes().to_vec()
    }

    #[test]
    fn swap_sol_locks_an_erc721_token_and_an_erc1155_amount() {
        let db = Arc::new(MemoryDb::default());
        let mut chain = open(&db);
        let me = evm::evm_alias(&authority().public().address());
        let swap = create(&mut chain, init_code());
        let names = encode_values(&[AbiType::String, AbiType::String], &[AbiValue::String("Art".into()), AbiValue::String("ART".into())]).unwrap();
        let art = create(&mut chain, [crate::structs::hex::from_hex(COLLECTIBLE_BIN.trim()).unwrap(), names].concat());
        let items = create(&mut chain, crate::structs::hex::from_hex(ITEMS_BIN.trim()).unwrap());
        let receiver = [0xb2; 20];
        let owner_of = |chain: &mut Chain, id: u64| -> [u8; 20] { returned(&run(chain, art, 0, calldata("ownerOf(uint256)", &[number(id)])))[12..].try_into().unwrap() };
        let held = |chain: &mut Chain, owner: [u8; 20], id: u64| u256::from_be_bytes(returned(&run(chain, items, 0, calldata("balanceOf(address,uint256)", &[word(owner), number(id)]))).try_into().unwrap());

        assert!(run(&mut chain, art, 0, calldata("mint(address)", &[word(me)])).result.is_ok());
        assert!(run(&mut chain, items, 0, calldata("mint(address,uint256,uint256)", &[word(me), number(9), number(50)])).result.is_ok());

        // ERC-721: approved, then pulled in; the secret pays it to the receiver.
        let nft = EvmLeg { item: Some(Item { id: u256::new(1), multi: false }), chain_id: 1, contract: swap, token: Some(art), amount: u256::new(1), payer: Some(me), receiver: Some(receiver) };
        let secret = [0x5e; 32];
        let hash = crate::crypto::bitcoin::sha256(&secret);
        let deadline = chain.tip().timestamp + 30 * SLOT;
        let id = nft.swap_id(&hash, deadline).unwrap();
        let [approve, lock] = <[_; 2]>::try_from(nft.lock_calls(&hash, deadline).unwrap()).unwrap();

        refused(&run(&mut chain, swap, 0, lock.input.clone()), "not the owner, approved or an operator");
        assert!(run(&mut chain, art, 0, approve.input).result.is_ok());
        assert!(run(&mut chain, swap, 0, lock.input).result.is_ok());
        assert_eq!(owner_of(&mut chain, 1), swap);
        let mut locked = read_lock(&returned(&run(&mut chain, swap, 0, get_input(&id)))).unwrap();
        locked.item = read_item(&returned(&run(&mut chain, swap, 0, item_input(&id)))).unwrap();

        assert_eq!(locked.item, nft.item);
        assert_eq!(Leg::Evm(nft.clone()).stands(&locked, deadline), Ok(()));
        assert!(run(&mut chain, swap, 0, nft.claim_call(&id, &secret).input).result.is_ok());
        assert_eq!(owner_of(&mut chain, 1), receiver);

        // ERC-1155: an operator, then an amount pulled in; the deadline passes and it goes back.
        let multi = EvmLeg { item: Some(Item { id: u256::new(9), multi: true }), token: Some(items), amount: u256::new(20), ..nft.clone() };
        let other = [0x6f; 32];
        let other_hash = crate::crypto::bitcoin::sha256(&other);
        let soon = chain.tip().timestamp + 4 * SLOT;
        let other_id = multi.swap_id(&other_hash, soon).unwrap();
        let [operator, lock] = <[_; 2]>::try_from(multi.lock_calls(&other_hash, soon).unwrap()).unwrap();

        assert!(run(&mut chain, items, 0, operator.input).result.is_ok());
        assert!(run(&mut chain, swap, 0, lock.input).result.is_ok());
        assert_eq!((held(&mut chain, swap, 9), held(&mut chain, me, 9)), (u256::new(20), u256::new(30)));
        assert_eq!(read_item(&returned(&run(&mut chain, swap, 0, item_input(&other_id)))).unwrap(), multi.item);

        // Tokens sent to it any other way are refused.
        let stray = calldata("safeTransferFrom(address,address,uint256,uint256,bytes)", &[word(me), word(swap), number(9), number(1), number(160), number(0)]);
        refused(&run(&mut chain, items, 0, stray), "send tokens with lockMulti");

        while chain.tip().timestamp / 1_000 < soon / 1_000 {
            advance(&mut chain, Vec::new());
        }

        assert!(run(&mut chain, swap, 0, multi.refund_call(&other_id).input).result.is_ok());
        assert_eq!((held(&mut chain, swap, 9), held(&mut chain, me, 9)), (u256::ZERO, u256::new(50)));
        assert_eq!(read_item(&returned(&run(&mut chain, swap, 0, item_input(&crate::crypto::bitcoin::sha256(&[1; 32]))))).unwrap(), None);
    }
}

#[cfg(feature = "bridge")]
mod gateway {
    //! Gateway.sol, the bridges' gateway on EVM chains, run on chainv4's own EVM: what arrives
    //! with the federation's signatures is minted wrapped or released, what leaves is held or
    //! burned, with digests as chainv4::bridge makes them, and the signer set moves only as the
    //! one in force signs.

    use {
        super::*,
        crate::abi::{AbiType, AbiValue, decode_values, encode_values},
        crate::bridge::{AssetKind, Transfer, WRAPPED_CODES, gateway_constructor, refund_input, send_input, set_signers_input, sign_digest, signers_digest},
        crate::structs::hex::from_hex,
    };

    /// chainv4's own chain id, as the gateways would name a chain it bridges to, and its gateway.
    const OTHER: u64 = 0x12_3456;
    const PEER: [u8; 33] = [0x01; 33];

    fn create(chain: &mut Chain, init: Vec<u8>) -> [u8; 20] {
        let deployer = authority();
        let at = evm::create_address(&deployer.public().address(), nonce(chain, &deployer));
        let receipts = advance(chain, vec![sign(chain, &deployer, Tx::Deploy { code: Code::Evm(init) })]);
        assert_eq!(receipts[0].result, Ok(Effect::Deployed(at)), "{:?}", receipts[0].result);

        match at {
            Address::Evm(bytes) => bytes,
            other => panic!("an EVM contract at {other}"),
        }
    }

    fn run(chain: &mut Chain, to: [u8; 20], value: u128, input: Vec<u8>) -> Receipt {
        let tx = call(chain, &authority(), Address::Evm(to), value, input);

        advance(chain, vec![tx]).remove(0)
    }

    fn read(chain: &mut Chain, to: [u8; 20], signature: &str, values: &[AbiValue], returns: &str) -> Vec<AbiValue> {
        let (_, types) = crate::abi::parse_signature(signature).unwrap();
        let input = [keccak256(signature.as_bytes())[..4].to_vec(), encode_values(&types, values).unwrap()].concat();
        let receipt = run(chain, to, 0, input);
        let (_, returned_types) = crate::abi::parse_signature(returns).unwrap();

        decode_values(&returned_types, &returned(&receipt)).unwrap_or_else(|err| panic!("{signature}: {err} {:?}", receipt.result))
    }

    fn signer(index: u8) -> [u8; 20] {
        crate::crypto::secp256k1::address(&crate::crypto::secp256k1::public_key(&[index; 32]).unwrap()).unwrap()
    }

    /// A gateway with its templates, under a federation of three, two to sign, peered with OTHER.
    fn gateway(chain: &mut Chain) -> [u8; 20] {
        let templates = WRAPPED_CODES.map(|code| create(chain, from_hex(code.trim()).unwrap()));
        let code = from_hex(crate::bridge::GATEWAY_CODE.trim()).unwrap();
        let gateway = create(chain, [code, gateway_constructor(&[signer(1), signer(2), signer(3)], 2, 0, ("Dev", "DEV"), templates)].concat());
        let peer = [keccak256(b"setPeer(uint64,bytes)")[..4].to_vec(), encode_values(&[AbiType::Uint(64), AbiType::Bytes], &[AbiValue::Uint(u256::from(OTHER)), AbiValue::Bytes(PEER.to_vec())]).unwrap()].concat();
        assert!(run(chain, gateway, 0, peer).result.is_ok());

        gateway
    }

    fn sent(chain: &mut Chain, gateway: [u8; 20], nonce: u64) -> Transfer {
        let values = read(chain, gateway, "sent(uint64)", &[AbiValue::Uint(u256::from(nonce))], &format!("({})", crate::bridge::TRANSFER_TUPLE));

        Transfer::from_evm(&values[0]).unwrap()
    }

    fn refused(receipt: &Receipt, why: &str) {
        assert!(format!("{:?}", receipt.result).contains(why), "{:?}", receipt.result);
    }

    #[test]
    fn a_token_from_chainv4_is_minted_wrapped_here_and_goes_back_burned() {
        let db = Arc::new(MemoryDb::default());
        let mut chain = open(&db);
        let here = evm::chain_id(&chain.genesis());
        let gateway = gateway(&mut chain);
        let me = evm::evm_alias(&authority().public().address());
        let arriving = Transfer {
            from: PEER.to_vec(),
            to: gateway.to_vec(),
            source: OTHER,
            target: here,
            nonce: 1,
            kind: AssetKind::Token,
            origin: OTHER,
            asset: vec![0x07; 33],
            id: u256::ZERO,
            amount: u256::from(500u64),
            recipient: me.to_vec(),
            name: "Example Token".to_string(),
            symbol: "EXT".to_string(),
            decimals: 18,
            uri: String::new(),
        };
        let signed = |transfer: &Transfer, signers: &[u8]| signers.iter().map(|index| transfer.sign(&[*index; 32])).collect::<Vec<_>>();

        // One signature is not enough, nor two of one signer, nor a stranger's.
        refused(&run(&mut chain, gateway, 0, arriving.deliver_input(&signed(&arriving, &[1]))), "not enough of the federation");
        refused(&run(&mut chain, gateway, 0, arriving.deliver_input(&signed(&arriving, &[1, 1]))), "not enough of the federation");
        refused(&run(&mut chain, gateway, 0, arriving.deliver_input(&signed(&arriving, &[1, 9]))), "not enough of the federation");
        refused(&run(&mut chain, gateway, 0, Transfer { from: vec![2; 33], ..arriving.clone() }.deliver_input(&signed(&arriving, &[1, 2]))), "not from this gateway's peer");

        // Two of three, and it is minted: wEXT, to its recipient, once.
        assert!(run(&mut chain, gateway, 0, arriving.deliver_input(&signed(&arriving, &[1, 3]))).result.is_ok());
        refused(&run(&mut chain, gateway, 0, arriving.deliver_input(&signed(&arriving, &[1, 3]))), "delivered already");
        let salt = keccak256(encode_values(&[AbiType::Uint(64), AbiType::Uint(8), AbiType::Bytes], &[AbiValue::Uint(u256::from(OTHER)), AbiValue::Uint(u256::from(1u64)), AbiValue::Bytes(vec![0x07; 33])]).unwrap());
        let [AbiValue::Address(wrapped)] = read(&mut chain, gateway, "wrappedOf(bytes32)", &[AbiValue::FixedBytes(salt.to_vec())], "(address)")[..] else { panic!("made") };

        assert_eq!(read(&mut chain, wrapped, "balanceOf(address)", &[AbiValue::Address(me)], "(uint256)"), [AbiValue::Uint(u256::from(500u64))]);
        assert_eq!(read(&mut chain, wrapped, "symbol()", &[], "(string)"), [AbiValue::String("wEXT".into())]);

        // Back: burned here, and the transfer names the token on chainv4, digest as Rust makes it.
        assert!(run(&mut chain, gateway, 0, send_input(OTHER, &[0x01; 33], AssetKind::Token, wrapped, u256::ZERO, u256::from(200u64))).result.is_ok());
        assert_eq!(read(&mut chain, wrapped, "balanceOf(address)", &[AbiValue::Address(me)], "(uint256)"), [AbiValue::Uint(u256::from(300u64))]);
        let back = sent(&mut chain, gateway, 1);

        assert_eq!((back.origin, back.asset.as_slice(), back.amount, back.name.as_str()), (OTHER, [0x07; 33].as_slice(), u256::from(200u64), "Example Token"));
        assert_eq!((back.from.as_slice(), back.to.as_slice()), (gateway.as_slice(), PEER.as_slice()));
        assert_eq!(read(&mut chain, gateway, &format!("digestOf({})", crate::bridge::TRANSFER_TUPLE), &[back.evm_tuple()], "(bytes32)"), [AbiValue::FixedBytes(back.digest().to_vec())]);
    }

    #[test]
    fn a_transfer_from_chainv4_arrives_by_a_proof_alone_with_signatures_or_by_signatures_alone_as_the_mode_says() {
        let db = Arc::new(MemoryDb::default());
        let mut chain = open(&db);
        let here = evm::chain_id(&chain.genesis());
        let gateway = gateway(&mut chain);
        let finality = create(&mut chain, from_hex(include_str!("../../../contracts/evm/MockFinality.bin").trim()).unwrap());
        let transfer = |nonce: u64| Transfer {
            from: PEER.to_vec(),
            to: gateway.to_vec(),
            source: OTHER,
            target: here,
            nonce,
            kind: AssetKind::Token,
            origin: OTHER,
            asset: vec![0x07; 33],
            id: u256::ZERO,
            amount: u256::from(500u64),
            recipient: evm::evm_alias(&authority().public().address()).to_vec(),
            name: "Example Token".to_string(),
            symbol: "EXT".to_string(),
            decimals: 18,
            uri: String::new(),
        };
        let signed = |transfer: &Transfer, signers: &[u8]| signers.iter().map(|index| transfer.sign(&[*index; 32])).collect::<Vec<_>>();
        let address_word = |address: [u8; 20]| [[0u8; 12].as_slice(), &address].concat();
        let prove = |chain: &mut Chain, transfer: &Transfer, mode: u8| {
            let input = [keccak256(b"set(bytes32,uint8)")[..4].to_vec(), transfer.digest().to_vec(), u256::from(mode).to_be_bytes().to_vec()].concat();
            assert!(run(chain, finality, 0, input).result.is_ok());
        };

        // Before it follows a finality, no proof is taken.
        refused(&run(&mut chain, gateway, 0, transfer(1).deliver_proven_input([9; 32], &[], &[])), "no finality to prove by");

        // Its owner points it at one, once.
        let set = [keccak256(b"setFinality(address)")[..4].to_vec(), address_word(finality)].concat();
        assert!(run(&mut chain, gateway, 0, set.clone()).result.is_ok());
        refused(&run(&mut chain, gateway, 0, set), "replaced only by the federation");

        // Proof or federation: a proven transfer alone, an unproven one not; signatures still do.
        prove(&mut chain, &transfer(1), 0);
        refused(&run(&mut chain, gateway, 0, transfer(2).deliver_proven_input([9; 32], &[], &[])), "not in that proven range");
        assert!(run(&mut chain, gateway, 0, transfer(1).deliver_proven_input([9; 32], &[], &[])).result.is_ok());
        refused(&run(&mut chain, gateway, 0, transfer(1).deliver_proven_input([9; 32], &[], &[])), "delivered already");
        assert!(run(&mut chain, gateway, 0, transfer(2).deliver_input(&signed(&transfer(2), &[1, 2]))).result.is_ok());

        // Both: signatures alone are refused, a proof alone too, and the two together arrive.
        prove(&mut chain, &transfer(3), 1);
        refused(&run(&mut chain, gateway, 0, transfer(3).deliver_input(&signed(&transfer(3), &[1, 2]))), "a proof is needed as well");
        refused(&run(&mut chain, gateway, 0, transfer(3).deliver_proven_input([9; 32], &[], &[])), "not enough of the federation");
        assert!(run(&mut chain, gateway, 0, transfer(3).deliver_proven_input([9; 32], &[], &signed(&transfer(3), &[1, 2]))).result.is_ok());

        // Federation only: a proof is refused, signatures alone arrive, as before proofs.
        prove(&mut chain, &transfer(4), 2);
        refused(&run(&mut chain, gateway, 0, transfer(4).deliver_proven_input([9; 32], &[], &signed(&transfer(4), &[1, 2]))), "by the federation alone");
        assert!(run(&mut chain, gateway, 0, transfer(4).deliver_input(&signed(&transfer(4), &[1, 2]))).result.is_ok());
    }

    #[test]
    fn the_coin_and_erc20s_leave_held_and_the_signers_change_only_as_they_sign() {
        let db = Arc::new(MemoryDb::default());
        let mut chain = open(&db);
        let here = evm::chain_id(&chain.genesis());
        let gateway = gateway(&mut chain);

        // ETH, here DEV, leaves held, as the coin.
        assert!(run(&mut chain, gateway, 1_000, send_input(OTHER, &[0x01; 33], AssetKind::Coin, [0; 20], u256::ZERO, u256::from(1_000u64))).result.is_ok());
        let left = sent(&mut chain, gateway, 1);

        assert_eq!((left.kind, left.origin, left.asset.len(), left.symbol.as_str()), (AssetKind::Coin, here, 0, "DEV"));
        refused(&run(&mut chain, gateway, 5, send_input(OTHER, &[0x01; 33], AssetKind::Coin, [0; 20], u256::ZERO, u256::from(6u64))), "as much as amount says");
        refused(&run(&mut chain, gateway, 0, send_input(99, &[0x01; 33], AssetKind::Coin, [0; 20], u256::ZERO, u256::from(1u64))), "no gateway on that chain");

        // An ERC-20 leaves held, with its name, after its approval.
        let supply = encode_values(&[AbiType::String, AbiType::String, AbiType::Uint(256)], &[AbiValue::String("Test".into()), AbiValue::String("TST".into()), AbiValue::Uint(u256::new(1_000_000))]).unwrap();
        let token = create(&mut chain, [from_hex(include_str!("../../../contracts/evm/Token.bin").trim()).unwrap(), supply].concat());
        let approve = [keccak256(b"approve(address,uint256)")[..4].to_vec(), encode_values(&[AbiType::Address, AbiType::Uint(256)], &[AbiValue::Address(gateway), AbiValue::Uint(u256::new(70))]).unwrap()].concat();
        assert!(run(&mut chain, token, 0, approve).result.is_ok());
        assert!(run(&mut chain, gateway, 0, send_input(OTHER, &[0x01; 33], AssetKind::Token, token, u256::ZERO, u256::from(70u64))).result.is_ok());
        let held = sent(&mut chain, gateway, 2);

        assert_eq!((held.kind, held.asset.as_slice(), held.name.as_str(), held.decimals), (AssetKind::Token, token.as_slice(), "Test", 18));

        // It comes back released, with the federation's word.
        let back = Transfer {
            from: PEER.to_vec(),
            to: gateway.to_vec(),
            source: OTHER,
            target: here,
            nonce: 1,
            recipient: vec![0xab; 20],
            amount: u256::from(30u64),
            ..held.clone()
        };
        assert!(run(&mut chain, gateway, 0, back.deliver_input(&[back.sign(&[2; 32]), back.sign(&[3; 32])])).result.is_ok());

        assert_eq!(read(&mut chain, token, "balanceOf(address)", &[AbiValue::Address([0xab; 20])], "(uint256)"), [AbiValue::Uint(u256::from(30u64))]);

        // The next federation needs the one in force's signatures, and a later version.
        let next = [signer(4), signer(5)];
        let digest = signers_digest(gateway, here, 1, &next, 2);
        refused(&run(&mut chain, gateway, 0, set_signers_input(&next, 2, 1, &[sign_digest(&[4; 32], &digest), sign_digest(&[5; 32], &digest)])), "not enough of the federation");
        assert!(run(&mut chain, gateway, 0, set_signers_input(&next, 2, 1, &[sign_digest(&[1; 32], &digest), sign_digest(&[2; 32], &digest)])).result.is_ok());
        refused(&run(&mut chain, gateway, 0, set_signers_input(&next, 2, 1, &[sign_digest(&[4; 32], &digest), sign_digest(&[5; 32], &digest)])), "an older federation");
        assert_eq!(read(&mut chain, gateway, "version()", &[], "(uint64)"), [AbiValue::Uint(u256::from(1u64))]);

        // Now only the new signers deliver.
        let again = Transfer { nonce: 2, ..back.clone() };
        refused(&run(&mut chain, gateway, 0, again.deliver_input(&[again.sign(&[2; 32]), again.sign(&[3; 32])])), "not enough of the federation");
        assert!(run(&mut chain, gateway, 0, again.deliver_input(&[again.sign(&[4; 32]), again.sign(&[5; 32])])).result.is_ok());
    }

    /// A transfer on its way here from the gateway over there.
    #[allow(clippy::too_many_arguments)]
    fn arriving(gateway: [u8; 20], here: u64, nonce: u64, kind: AssetKind, origin: u64, asset: Vec<u8>, id: u64, amount: u64, recipient: [u8; 20]) -> Transfer {
        Transfer {
            from: PEER.to_vec(),
            to: gateway.to_vec(),
            source: OTHER,
            target: here,
            nonce,
            kind,
            origin,
            asset,
            id: u256::from(id),
            amount: u256::from(amount),
            recipient: recipient.to_vec(),
            name: "Example".to_string(),
            symbol: "EXT".to_string(),
            decimals: 18,
            uri: if id > 0 { "ipfs://example".to_string() } else { String::new() },
        }
    }

    /// The first two signers' signatures over a digest: the federation's threshold.
    fn two(digest: &[u8; 32]) -> Vec<[u8; 65]> {
        vec![sign_digest(&[1; 32], digest), sign_digest(&[2; 32], digest)]
    }

    /// Pays back what left at a nonce, as the federation's threshold would.
    fn refund(chain: &mut Chain, gateway: [u8; 20], nonce: u64) -> Receipt {
        let left = sent(chain, gateway, nonce);
        let sender = sender_of(chain, gateway, nonce);

        run(chain, gateway, 0, refund_input(nonce, &two(&left.refund_digest(&sender))))
    }

    fn sender_of(chain: &mut Chain, gateway: [u8; 20], nonce: u64) -> Vec<u8> {
        match read(chain, gateway, "leftBy(uint64)", &[AbiValue::Uint(u256::from(nonce))], "(address,uint64)")[..] {
            [AbiValue::Address(sender), _] => sender.to_vec(),
            ref other => panic!("{other:?}"),
        }
    }

    fn holds(chain: &mut Chain, token: [u8; 20], owner: [u8; 20]) -> u256 {
        match read(chain, token, "balanceOf(address)", &[AbiValue::Address(owner)], "(uint256)")[..] {
            [AbiValue::Uint(amount)] => amount,
            ref other => panic!("{other:?}"),
        }
    }

    fn holds_of(chain: &mut Chain, token: [u8; 20], owner: [u8; 20], id: u64) -> u256 {
        match read(chain, token, "balanceOf(address,uint256)", &[AbiValue::Address(owner), AbiValue::Uint(u256::from(id))], "(uint256)")[..] {
            [AbiValue::Uint(amount)] => amount,
            ref other => panic!("{other:?}"),
        }
    }

    fn owner_of(chain: &mut Chain, token: [u8; 20], id: u64) -> [u8; 20] {
        match read(chain, token, "ownerOf(uint256)", &[AbiValue::Uint(u256::from(id))], "(address)")[..] {
            [AbiValue::Address(owner)] => owner,
            ref other => panic!("{other:?}"),
        }
    }

    /// What a transaction calling a function with these values carries.
    fn input(signature: &str, values: &[AbiValue]) -> Vec<u8> {
        let (_, types) = crate::abi::parse_signature(signature).unwrap();

        [keccak256(signature.as_bytes())[..4].to_vec(), encode_values(&types, values).unwrap()].concat()
    }

    #[test]
    fn a_void_and_a_refund_digest_are_gateway_sol_s_as_they_are_rust_s_and_pass_for_nothing_else() {
        let db = Arc::new(MemoryDb::default());
        let mut chain = open(&db);
        let here = evm::chain_id(&chain.genesis());
        let gateway = gateway(&mut chain);
        let transfer = arriving(gateway, here, 1, AssetKind::Coin, OTHER, Vec::new(), 0, 10, [0xab; 20]);
        let ends = [AbiValue::Bytes(transfer.from.clone()), AbiValue::Bytes(transfer.to.clone()), AbiValue::Uint(u256::from(OTHER)), AbiValue::Uint(u256::from(here)), AbiValue::Uint(u256::from(1u64)), AbiValue::FixedBytes(transfer.digest().to_vec())];

        assert_eq!(read(&mut chain, gateway, "voidDigest(bytes,bytes,uint64,uint64,uint64,bytes32)", &ends, "(bytes32)"), [AbiValue::FixedBytes(transfer.void_digest().to_vec())]);
        let with_sender = [&ends[..], &[AbiValue::Bytes(vec![0xc0; 20])]].concat();
        assert_eq!(read(&mut chain, gateway, "refundDigest(bytes,bytes,uint64,uint64,uint64,bytes32,bytes)", &with_sender, "(bytes32)"), [AbiValue::FixedBytes(transfer.refund_digest(&[0xc0; 20]).to_vec())]);

        // Signatures over a transfer, or over its void, count for nothing as the other.
        refused(&run(&mut chain, gateway, 0, transfer.void_input(&[transfer.sign(&[1; 32]), transfer.sign(&[2; 32])])), "not enough of the federation");
        refused(&run(&mut chain, gateway, 0, transfer.deliver_input(&two(&transfer.void_digest()))), "not enough of the federation");
    }

    #[test]
    fn a_voided_transfer_can_never_be_delivered_however_public_its_signatures() {
        let db = Arc::new(MemoryDb::default());
        let mut chain = open(&db);
        let here = evm::chain_id(&chain.genesis());
        let gateway = gateway(&mut chain);
        let transfer = arriving(gateway, here, 1, AssetKind::Token, OTHER, vec![0x07; 33], 0, 500, [0xab; 20]);
        let delivery = [transfer.sign(&[1; 32]), transfer.sign(&[3; 32])];
        let key = AbiValue::FixedBytes(transfer.delivered_key());
        let flag = |chain: &mut Chain, signature: &str| read(chain, gateway, signature, std::slice::from_ref(&key), "(uint256)");

        // It takes the threshold, of the federation, from this gateway's peer.
        refused(&run(&mut chain, gateway, 0, transfer.void_input(&[sign_digest(&[1; 32], &transfer.void_digest())])), "not enough of the federation");
        refused(&run(&mut chain, gateway, 0, transfer.void_input(&[sign_digest(&[1; 32], &transfer.void_digest()), sign_digest(&[9; 32], &transfer.void_digest())])), "not enough of the federation");
        let forged = Transfer { from: vec![2; 33], ..transfer.clone() };
        refused(&run(&mut chain, gateway, 0, forged.void_input(&two(&forged.void_digest()))), "not from this gateway's peer");
        let elsewhere = Transfer { to: vec![0xee; 20], ..transfer.clone() };
        refused(&run(&mut chain, gateway, 0, elsewhere.void_input(&two(&elsewhere.void_digest()))), "not enough of the federation");
        assert_eq!(flag(&mut chain, "voidedAt(bytes32)"), [AbiValue::Uint(u256::ZERO)]);

        // Voided, once. What signed its delivery, public by then, is refused, and nothing is minted.
        assert!(run(&mut chain, gateway, 0, transfer.void_input(&two(&transfer.void_digest()))).result.is_ok());
        assert_ne!(flag(&mut chain, "voidedAt(bytes32)"), [AbiValue::Uint(u256::ZERO)]);
        refused(&run(&mut chain, gateway, 0, transfer.void_input(&two(&transfer.void_digest()))), "voided already");
        refused(&run(&mut chain, gateway, 0, transfer.deliver_input(&delivery)), "voided");
        assert_eq!(flag(&mut chain, "delivered(bytes32)"), [AbiValue::Uint(u256::ZERO)], "it says what arrived, and this did not");
        let salt = keccak256(encode_values(&[AbiType::Uint(64), AbiType::Uint(8), AbiType::Bytes], &[AbiValue::Uint(u256::from(OTHER)), AbiValue::Uint(u256::from(1u64)), AbiValue::Bytes(vec![0x07; 33])]).unwrap());
        assert_eq!(read(&mut chain, gateway, "wrappedOf(bytes32)", &[AbiValue::FixedBytes(salt.to_vec())], "(address)"), [AbiValue::Address([0; 20])]);

        // What was delivered cannot be voided: whichever lands first wins.
        let delivered = Transfer { nonce: 2, ..transfer.clone() };
        assert!(run(&mut chain, gateway, 0, delivered.deliver_input(&[delivered.sign(&[1; 32]), delivered.sign(&[2; 32])])).result.is_ok());
        refused(&run(&mut chain, gateway, 0, delivered.void_input(&two(&delivered.void_digest()))), "delivered already");

        // A void signed for one source gateway says nothing of another's same nonce, after a reset.
        let peer = input("setPeer(uint64,bytes)", &[AbiValue::Uint(u256::from(OTHER)), AbiValue::Bytes(vec![0x02; 33])]);
        assert!(run(&mut chain, gateway, 0, peer).result.is_ok());
        let renewed = Transfer { from: vec![0x02; 33], ..transfer.clone() };
        assert!(run(&mut chain, gateway, 0, renewed.deliver_input(&[renewed.sign(&[1; 32]), renewed.sign(&[2; 32])])).result.is_ok());
    }

    #[test]
    fn what_left_held_is_released_back_to_who_sent_it_once() {
        let db = Arc::new(MemoryDb::default());
        let mut chain = open(&db);
        let gateway = gateway(&mut chain);
        let me_address = authority().public().address();
        let me = evm::evm_alias(&me_address);
        let to = [0x01; 33];

        // The coin. It takes the threshold, and what it signs is whom the gateway says sent it.
        assert!(run(&mut chain, gateway, 1_000, send_input(OTHER, &to, AssetKind::Coin, [0; 20], u256::ZERO, u256::from(1_000u64))).result.is_ok());
        let left = sent(&mut chain, gateway, 1);
        let before = balance(&chain, &me_address);
        refused(&run(&mut chain, gateway, 0, refund_input(1, &[sign_digest(&[1; 32], &left.refund_digest(&me))])), "not enough of the federation");
        refused(&run(&mut chain, gateway, 0, refund_input(1, &two(&left.refund_digest(&[0xd0; 20])))), "not enough of the federation");
        refused(&run(&mut chain, gateway, 0, refund_input(1, &two(&left.void_digest()))), "not enough of the federation");
        refused(&run(&mut chain, gateway, 0, refund_input(1, &[left.sign(&[1; 32]), left.sign(&[2; 32])])), "not enough of the federation");
        refused(&run(&mut chain, gateway, 0, refund_input(9, &two(&left.refund_digest(&me)))), "no such transfer");
        assert!(refund(&mut chain, gateway, 1).result.is_ok());
        assert_eq!(balance(&chain, &me_address), before + u256::from(1_000u64));
        assert_eq!(balance(&chain, &Address::Evm(gateway)), u256::ZERO);
        refused(&refund(&mut chain, gateway, 1), "refunded already");
        assert_eq!(read(&mut chain, gateway, "refunded(uint64)", &[AbiValue::Uint(u256::from(1u64))], "(bool)"), [AbiValue::Bool(true)]);

        // An ERC-20.
        let supply = encode_values(&[AbiType::String, AbiType::String, AbiType::Uint(256)], &[AbiValue::String("Test".into()), AbiValue::String("TST".into()), AbiValue::Uint(u256::new(1_000))]).unwrap();
        let token = create(&mut chain, [from_hex(include_str!("../../../contracts/evm/Token.bin").trim()).unwrap(), supply].concat());
        assert!(run(&mut chain, token, 0, input("approve(address,uint256)", &[AbiValue::Address(gateway), AbiValue::Uint(u256::new(70))])).result.is_ok());
        assert!(run(&mut chain, gateway, 0, send_input(OTHER, &to, AssetKind::Token, token, u256::ZERO, u256::from(70u64))).result.is_ok());
        assert_eq!(holds(&mut chain, token, me), u256::new(930));
        assert!(refund(&mut chain, gateway, 2).result.is_ok());
        assert_eq!(holds(&mut chain, token, me), u256::new(1_000));

        // An ERC-721.
        let names = encode_values(&[AbiType::String, AbiType::String], &[AbiValue::String("Art".into()), AbiValue::String("ART".into())]).unwrap();
        let art = create(&mut chain, [from_hex(include_str!("../../../contracts/evm/Collectible.bin").trim()).unwrap(), names].concat());
        assert!(run(&mut chain, art, 0, input("mint(address)", &[AbiValue::Address(me)])).result.is_ok());
        assert!(run(&mut chain, art, 0, input("approve(address,uint256)", &[AbiValue::Address(gateway), AbiValue::Uint(u256::new(1))])).result.is_ok());
        assert!(run(&mut chain, gateway, 0, send_input(OTHER, &to, AssetKind::Nft, art, u256::new(1), u256::new(1))).result.is_ok());
        assert_eq!(owner_of(&mut chain, art, 1), gateway);
        assert!(refund(&mut chain, gateway, 3).result.is_ok());
        assert_eq!(owner_of(&mut chain, art, 1), me);

        // An amount of an ERC-1155 id.
        let items = create(&mut chain, from_hex(include_str!("../../../contracts/evm/Items.bin").trim()).unwrap());
        assert!(run(&mut chain, items, 0, input("mint(address,uint256,uint256)", &[AbiValue::Address(me), AbiValue::Uint(u256::new(9)), AbiValue::Uint(u256::new(50))])).result.is_ok());
        assert!(run(&mut chain, items, 0, input("setApprovalForAll(address,bool)", &[AbiValue::Address(gateway), AbiValue::Bool(true)])).result.is_ok());
        assert!(run(&mut chain, gateway, 0, send_input(OTHER, &to, AssetKind::Multi, items, u256::new(9), u256::new(20))).result.is_ok());
        assert_eq!(holds_of(&mut chain, items, me, 9), u256::new(30));
        assert!(refund(&mut chain, gateway, 4).result.is_ok());
        assert_eq!(holds_of(&mut chain, items, me, 9), u256::new(50));
    }

    #[test]
    fn what_left_burned_is_minted_back_to_who_sent_it_once() {
        let db = Arc::new(MemoryDb::default());
        let mut chain = open(&db);
        let here = evm::chain_id(&chain.genesis());
        let gateway = gateway(&mut chain);
        let me = evm::evm_alias(&authority().public().address());
        let to = [0x01; 33];
        let deliver = |chain: &mut Chain, nonce: u64, kind: AssetKind, id: u64, amount: u64| -> [u8; 20] {
            let asset = vec![0x07 + kind as u8; 33];
            let transfer = arriving(gateway, here, nonce, kind, OTHER, asset.clone(), id, amount, me);
            assert!(run(chain, gateway, 0, transfer.deliver_input(&[transfer.sign(&[1; 32]), transfer.sign(&[2; 32])])).result.is_ok());
            let salt = keccak256(encode_values(&[AbiType::Uint(64), AbiType::Uint(8), AbiType::Bytes], &[AbiValue::Uint(u256::from(OTHER)), AbiValue::Uint(u256::from(kind as u64)), AbiValue::Bytes(asset)]).unwrap());

            match read(chain, gateway, "wrappedOf(bytes32)", &[AbiValue::FixedBytes(salt.to_vec())], "(address)")[..] {
                [AbiValue::Address(made)] => made,
                ref other => panic!("{other:?}"),
            }
        };

        // A wrapped ERC-20, burned as it left.
        let token = deliver(&mut chain, 1, AssetKind::Token, 0, 500);
        assert!(run(&mut chain, gateway, 0, send_input(OTHER, &to, AssetKind::Token, token, u256::ZERO, u256::new(200))).result.is_ok());
        assert_eq!(holds(&mut chain, token, me), u256::new(300));
        assert!(refund(&mut chain, gateway, 1).result.is_ok());
        assert_eq!(holds(&mut chain, token, me), u256::new(500));
        refused(&refund(&mut chain, gateway, 1), "refunded already");
        assert_eq!(holds(&mut chain, token, me), u256::new(500));

        // A wrapped ERC-721 comes back with its URI, the same id.
        let art = deliver(&mut chain, 2, AssetKind::Nft, 42, 1);
        assert!(run(&mut chain, gateway, 0, send_input(OTHER, &to, AssetKind::Nft, art, u256::new(42), u256::new(1))).result.is_ok());
        assert_eq!(owner_of(&mut chain, art, 42), [0; 20], "burned");
        assert!(refund(&mut chain, gateway, 2).result.is_ok());
        assert_eq!(owner_of(&mut chain, art, 42), me);
        assert_eq!(read(&mut chain, art, "tokenURI(uint256)", &[AbiValue::Uint(u256::new(42))], "(string)"), [AbiValue::String("ipfs://example".into())]);

        // An amount of a wrapped ERC-1155 id.
        let items = deliver(&mut chain, 3, AssetKind::Multi, 5, 30);
        assert!(run(&mut chain, gateway, 0, send_input(OTHER, &to, AssetKind::Multi, items, u256::new(5), u256::new(10))).result.is_ok());
        assert_eq!(holds_of(&mut chain, items, me, 5), u256::new(20));
        assert!(refund(&mut chain, gateway, 3).result.is_ok());
        assert_eq!(holds_of(&mut chain, items, me, 5), u256::new(30));
    }

    #[test]
    fn a_refund_that_cannot_be_paid_stays_to_try_again_and_its_sender_alone_says_where_else_it_goes() {
        let db = Arc::new(MemoryDb::default());
        let mut chain = open(&db);
        let gateway = gateway(&mut chain);
        let friend = [0xf0; 20];

        // Its sender first names a contract that takes no coin: the payment fails, and all of it with it.
        assert!(run(&mut chain, gateway, 1_000, send_input(OTHER, &[0x01; 33], AssetKind::Coin, [0; 20], u256::ZERO, u256::from(1_000u64))).result.is_ok());
        let name = |to: [u8; 20]| input("setRefundTo(uint64,address)", &[AbiValue::Uint(u256::from(1u64)), AbiValue::Address(to)]);
        assert!(run(&mut chain, gateway, 0, name(gateway)).result.is_ok());
        refused(&refund(&mut chain, gateway, 1), "the coin would not go");
        assert_eq!(read(&mut chain, gateway, "refunded(uint64)", &[AbiValue::Uint(u256::from(1u64))], "(bool)"), [AbiValue::Bool(false)]);

        // No one else may name another, as refused here by one who is not the sender.
        let stranger = Keypair::from_private(crate::crypto::keypair::PrivateKey::Secp256k1([0x55; 32]));
        let fund = sign(&chain, &authority(), Tx::Transfer { from: authority().public().address(), to: stranger.public().address(), amount: Amount::U128(1_000_000_000) });
        advance(&mut chain, vec![fund]);
        let tx = call(&chain, &stranger, Address::Evm(gateway), 0, name(friend));
        refused(&advance(&mut chain, vec![tx]).remove(0), "only who sent it");

        // Named well, the same signatures pay it there, once, as what they sign is who sent it.
        assert!(run(&mut chain, gateway, 0, name(friend)).result.is_ok());
        assert!(refund(&mut chain, gateway, 1).result.is_ok());
        assert_eq!(balance(&chain, &Address::Secp256k1(friend)), u256::from(1_000u64));
        refused(&refund(&mut chain, gateway, 1), "refunded already");
    }

    #[test]
    fn a_refund_after_a_rotation_takes_the_signers_now_in_force() {
        let db = Arc::new(MemoryDb::default());
        let mut chain = open(&db);
        let here = evm::chain_id(&chain.genesis());
        let gateway = gateway(&mut chain);
        assert!(run(&mut chain, gateway, 1_000, send_input(OTHER, &[0x01; 33], AssetKind::Coin, [0; 20], u256::ZERO, u256::from(1_000u64))).result.is_ok());
        let left = sent(&mut chain, gateway, 1);
        let sender = sender_of(&mut chain, gateway, 1);
        let digest = left.refund_digest(&sender);
        let old = two(&digest);
        let next = [signer(4), signer(5)];
        let moved = signers_digest(gateway, here, 1, &next, 2);
        assert!(run(&mut chain, gateway, 0, set_signers_input(&next, 2, 1, &[sign_digest(&[1; 32], &moved), sign_digest(&[2; 32], &moved)])).result.is_ok());

        refused(&run(&mut chain, gateway, 0, refund_input(1, &old)), "not enough of the federation");
        assert!(run(&mut chain, gateway, 0, refund_input(1, &[sign_digest(&[4; 32], &digest), sign_digest(&[5; 32], &digest)])).result.is_ok());
    }
}

/// An owner's EVM address, from its secret.
#[cfg(feature = "secp256k1")]
fn evm_owner(secret: &[u8; 32]) -> [u8; 20] {
    match Keypair::from_private(crate::crypto::keypair::PrivateKey::Secp256k1(*secret)).public().address() {
        Address::Secp256k1(address) => address,
        other => panic!("{other:?}"),
    }
}

/// An EVM key owner of a MultiSig.sol, by its secret.
#[cfg(feature = "secp256k1")]
fn key_owner(secret: &[u8; 32]) -> crate::registry::multisig::EvmOwner {
    crate::registry::multisig::EvmOwner { address: evm_owner(secret), kind: crate::registry::multisig::EvmKind::Key }
}

/// What a MultiSig.sol's reads answer: its owners and threshold, and its nonce.
#[cfg(feature = "secp256k1")]
fn multisig_state(chain: &mut Chain, contract: Address) -> (crate::registry::multisig::EvmStart, u256) {
    let [owners, threshold, nonce] = crate::abi::multisig::multisig_reads().map(|input| {
        let read = call(chain, &authority(), contract, 0, input);

        returned(&advance(chain, vec![read])[0])
    });

    crate::abi::multisig::multisig_read(&owners, &threshold, &nonce).expect("a MultiSig.sol's answers")
}

#[cfg(feature = "secp256k1")]
#[test]
fn a_multisig_contract_runs_what_threshold_of_its_owners_sign_and_changes_owners_only_through_itself() {
    use crate::abi::multisig::{EvmBundle, change_owners_input, multisig_execute_input, multisig_init_code};
    use crate::registry::multisig::{EvmOwner, EvmStart};

    let db = Arc::new(MemoryDb::default());
    let mut chain = open(&db);
    let secrets = [[1u8; 32], [2; 32], [3; 32], [5; 32]];
    let mut owners: Vec<EvmOwner> = secrets[..3].iter().map(key_owner).collect();
    owners.sort();
    let start = EvmStart { owners, threshold: 2 };
    let deployer = authority();
    let contract = evm::create_address(&deployer.public().address(), nonce(&chain, &deployer));
    let home = evm::chain_id(&chain.genesis());
    let deploy = sign(&chain, &deployer, Tx::Deploy { code: Code::Evm(multisig_init_code(&Address::Multisig([4; 32]), &start, home)) });

    assert_eq!(advance(&mut chain, vec![deploy])[0].result, Ok(Effect::Deployed(contract)));
    assert_eq!(multisig_state(&mut chain, contract), (start.clone(), u256::ZERO));

    let Address::Evm(at) = contract else { panic!("{contract:?}") };
    let fund = call(&chain, &authority(), contract, 1_000, Vec::new());
    advance(&mut chain, vec![fund]);
    let payee = evm_owner(&[9; 32]);
    let mut bundle = EvmBundle::new(evm::chain_id(&chain.genesis()), at, payee, u256::new(300), Vec::new(), u256::ZERO);
    let execute = |chain: &mut Chain, input: Vec<u8>| {
        let sent = call(chain, &authority(), contract, 0, input);

        advance(chain, vec![sent]).remove(0).result
    };

    // One of two is not enough.
    bundle.sign(&secrets[0]);
    assert!(execute(&mut chain, bundle.execute_input(&start.owners)).is_err());

    // Two are, once each, by address ascending; the same signatures again are for a nonce gone.
    bundle.sign(&secrets[2]);
    assert!(execute(&mut chain, bundle.execute_input(&start.owners)).is_ok());
    assert_eq!(balance(&chain, &Address::Secp256k1(payee)), u256::new(300));
    assert_eq!(balance(&chain, &contract), u256::new(700));
    assert!(execute(&mut chain, bundle.execute_input(&start.owners)).is_err(), "no replay");

    // Signatures out of order count for nothing.
    bundle.nonce = u256::ONE;
    bundle.signatures.clear();
    bundle.sign(&secrets[0]);
    bundle.sign(&secrets[1]);
    let mut backwards = bundle.signers();
    backwards.reverse();
    assert!(execute(&mut chain, multisig_execute_input(&bundle.to, bundle.value, &bundle.data, &backwards)).is_err());

    // Its owners change through itself alone, signed by those before.
    let mut next: Vec<EvmOwner> = secrets[1..].iter().map(key_owner).collect();
    next.sort();
    let next = EvmStart { owners: next, threshold: 1 };
    assert!(execute(&mut chain, change_owners_input(&next)).is_err(), "not from outside");

    let mut change = EvmBundle { to: at, value: u256::ZERO, data: change_owners_input(&next), ..bundle.clone() };
    change.signatures.clear();
    change.sign(&secrets[0]);
    change.sign(&secrets[1]);
    assert!(execute(&mut chain, change.execute_input(&start.owners)).is_ok());
    assert_eq!(multisig_state(&mut chain, contract), (next.clone(), u256::new(2)));

    // An owner gone signs for nothing; a new one alone now does.
    let mut pay = EvmBundle { nonce: u256::new(2), ..bundle.clone() };
    pay.signatures.clear();
    pay.sign(&secrets[0]);
    assert!(execute(&mut chain, pay.execute_input(&start.owners)).is_err());
    pay.signatures.clear();
    pay.sign(&secrets[3]);
    assert!(execute(&mut chain, pay.execute_input(&next.owners)).is_ok());
    assert_eq!(balance(&chain, &Address::Secp256k1(payee)), u256::new(600));

    // It takes NFTs: it says it is ERC-1155's receiver.
    let asked = call(&chain, &authority(), contract, 0, calldata("supportsInterface(bytes4)", &[[vec![0x4e, 0x23, 0x12, 0xe0], vec![0; 28]].concat()]));
    assert_eq!(returned(&advance(&mut chain, vec![asked])[0]), word(1));
}

#[cfg(feature = "secp256k1")]
#[test]
fn the_deployment_proxy_is_there_from_genesis_and_puts_a_multisigs_contract_where_it_is_everywhere() {
    use crate::abi::multisig::{multisig_contract, multisig_deployment};
    use crate::registry::multisig::{EvmOwner, EvmStart};

    let db = Arc::new(MemoryDb::default());
    let mut chain = open(&db);
    let proxy = Address::Evm([0x4e, 0x59, 0xb4, 0x48, 0x47, 0xb3, 0x79, 0x57, 0x85, 0x88, 0x92, 0x0c, 0xa7, 0x8f, 0xbf, 0x26, 0xc0, 0xb4, 0x95, 0x6c]);
    let mut owners: Vec<EvmOwner> = [[1u8; 32], [2; 32]].iter().map(key_owner).collect();
    owners.sort();
    let (multisig, start) = (Address::Multisig([4; 32]), EvmStart { owners, threshold: 2 });
    let home = evm::chain_id(&chain.genesis());
    let at = Address::Evm(multisig_contract(&multisig, &start, home));

    // Anyone sends it the deploy; it lands where CREATE2 through the proxy puts it on any chain.
    let deploy = call(&chain, &authority(), proxy, 0, multisig_deployment(&multisig, &start, home));
    let receipt = advance(&mut chain, vec![deploy]).remove(0);

    assert!(receipt.result.is_ok(), "{receipt:?}");
    assert_eq!(multisig_state(&mut chain, at), (start, u256::ZERO));
}

#[cfg(feature = "secp256k1")]
#[test]
fn a_contract_made_where_coin_waited_takes_it_over_and_none_is_made_or_lost() {
    use crate::abi::multisig::{multisig_contract, multisig_deployment};
    use crate::registry::multisig::EvmStart;

    let db = Arc::new(MemoryDb::default());
    let mut chain = open(&db);
    let genesis_id = chain.genesis();
    let proxy = Address::Evm([0x4e, 0x59, 0xb4, 0x48, 0x47, 0xb3, 0x79, 0x57, 0x85, 0x88, 0x92, 0x0c, 0xa7, 0x8f, 0xbf, 0x26, 0xc0, 0xb4, 0x95, 0x6c]);
    let (multisig, start) = (Address::Multisig([4; 32]), EvmStart { owners: vec![key_owner(&[1; 32])], threshold: 1 });
    let home = evm::chain_id(&chain.genesis());
    let bytes = multisig_contract(&multisig, &start, home);
    let me = authority().public().address();
    let before = balance(&chain, &me);

    // Sent through the EVM, as a wallet there would, the coin sits with the bytes' account until
    // there is code; sent here to the contract's address, under it.
    let through_evm = call(&chain, &authority(), Address::Secp256k1(bytes), 300, Vec::new());
    advance(&mut chain, vec![through_evm]);
    let here = SignedTx::sign(Tx::Transfer { from: me, to: Address::Evm(bytes), amount: Amount::U128(200) }, nonce(&chain, &authority()), FeeOffer::NONE, &authority(), &genesis_id);
    advance(&mut chain, vec![here]);

    let deploy = call(&chain, &authority(), proxy, 0, multisig_deployment(&multisig, &start, home));
    assert!(advance(&mut chain, vec![deploy])[0].result.is_ok());

    assert_eq!(balance(&chain, &Address::Evm(bytes)), u256::new(500), "both");
    assert_eq!(balance(&chain, &Address::Secp256k1(bytes)), u256::ZERO, "not twice");
    assert_eq!(balance(&chain, &me), before - u256::new(500));
}

mod batch;
mod collections;
#[cfg(all(feature = "secp256k1", feature = "ed25519"))]
mod owners;

/// A hop as depth, kind, from, to, function, whether it failed, and its events, for comparing.
type Seen = (u8, crate::structs::trace::HopKind, Address, Address, String, bool, Vec<u32>);

fn hops(receipt: &crate::chain::execute::Receipt) -> Vec<Seen> {
    receipt.trace.hops.iter().map(|hop| (hop.depth, hop.kind, hop.from, hop.to, hop.function.clone(), hop.error.is_some(), hop.events.clone())).collect()
}

/// The selector of a Solidity signature, in hex, as an EVM hop names its function.
fn selector_hex(signature: &str) -> String {
    crate::structs::hex::to_hex(&calldata(signature, &[])[..4])
}

#[test]
fn an_evm_transaction_s_trace_has_its_frames_and_the_native_calls_under_them() {
    use crate::structs::trace::HopKind::Call;

    let db = Arc::new(MemoryDb::default());
    let mut chain = open(&db);
    let (bridge, counter, other) = bridged(&mut chain);
    let me = authority().public().address();
    let precompile = Address::Evm(crate::vm::evm::NATIVE_PRECOMPILE);
    let call_native = selector_hex("callNative(bytes32,string,string,bytes)");

    // The EVM's frame, its call to the native precompile, and the native call under that: each
    // event where it landed, the native one first as it ran first.
    let add = call(&chain, &authority(), bridge, 300, calldata("add(bytes32,uint64)", &[contract_word(&counter), word(5)]));
    let receipt = advance(&mut chain, vec![add]).remove(0);
    assert_eq!(
        hops(&receipt),
        [
            (0, Call, me, bridge, selector_hex("add(bytes32,uint64)"), false, vec![1]),
            (1, Call, bridge, precompile, call_native.clone(), false, vec![]),
            (2, Call, bridge, counter, "add".to_string(), false, vec![0]),
        ]
    );
    assert_eq!(receipt.trace.hops[2].value, u256::new(300), "the coin it carried");
    assert!(receipt.trace.hops[0].fuel_used >= receipt.trace.hops[1].fuel_used && receipt.trace.hops[2].fuel_used > 0);

    // From Lua: the EVM's frames under the call into it, whose own frame that is, and the native
    // call they make under them, so the tree reads across all three.
    let round = lua_call(&chain, counter, "round_trip", vec![TxCallArgumentType::Address(bridge), TxCallArgumentType::Address(other), TxCallArgumentType::U64(4)]);
    let receipt = advance(&mut chain, vec![round]).remove(0);
    assert_eq!(
        hops(&receipt),
        [
            (0, Call, me, counter, "round_trip".to_string(), false, vec![]),
            (1, Call, counter, bridge, "evm".to_string(), false, vec![1]),
            (2, Call, bridge, precompile, call_native.clone(), false, vec![]),
            (3, Call, bridge, other, "add".to_string(), false, vec![0]),
        ]
    );

    // Reverted after its native call: the frames are all there, failing where it failed, and no
    // hop keeps an event, as none was kept.
    let revert = call(&chain, &authority(), bridge, 0, calldata("addThenRevert(bytes32,uint64)", &[contract_word(&counter), word(7)]));
    let receipt = advance(&mut chain, vec![revert]).remove(0);
    assert_eq!(
        hops(&receipt),
        [
            (0, Call, me, bridge, selector_hex("addThenRevert(bytes32,uint64)"), true, vec![]),
            (1, Call, bridge, precompile, call_native, false, vec![]),
            (2, Call, bridge, counter, "add".to_string(), false, vec![]),
        ]
    );
    assert_eq!(receipt.trace.hops[0].error.as_deref(), Some("reverted: changed my mind"));
}

/// A token whose add renames it and asks to be asked again: what the bridge's add calls.
const RENAMING_TOKEN: &str = r#"
local M = {}
function M.total_supply() return 0 end
function M.balance_of(owner) return 0 end
function M.transfer(to, amount) return true end
function M.approve(spender, amount) return true end
function M.allowance(owner, spender) return 0 end
function M.transfer_from(from, to, amount) return true end
function M.name() return storage.get("name") or "First" end

function M.add(by)
    storage.set("name", "Renamed " .. tostring(by))
    tokens.refresh()
    return by
end

return M
"#;

#[test]
fn a_token_the_evm_calls_that_asks_to_be_asked_again_is() {
    let db = Arc::new(MemoryDb::default());
    let mut chain = open(&db);
    let (bridge, counter, _) = bridged(&mut chain);
    let token = deploy_lua(&mut chain, RENAMING_TOKEN);
    let named = |chain: &Chain| crate::registry::Tokens::new(&chain.state()).get(&token).unwrap().and_then(|token| token.name);
    assert_eq!(named(&chain).as_deref(), Some("First"));

    // An EVM transaction calls it through the native precompile.
    let add = call(&chain, &authority(), bridge, 0, calldata("add(bytes32,uint64)", &[contract_word(&token), word(5)]));
    let receipt = advance(&mut chain, vec![add]).remove(0);
    assert!(receipt.result.is_ok(), "{:?}", receipt.result);
    assert_eq!(named(&chain).as_deref(), Some("Renamed 5"));

    // Lua, then the EVM, then the token: asked again all the same.
    let round = lua_call(&chain, counter, "round_trip", vec![TxCallArgumentType::Address(bridge), TxCallArgumentType::Address(token), TxCallArgumentType::U64(9)]);
    let receipt = advance(&mut chain, vec![round]).remove(0);
    assert!(receipt.result.is_ok(), "{:?}", receipt.result);
    assert_eq!(named(&chain).as_deref(), Some("Renamed 9"));

    // A native call the EVM reverted after asks nothing: its rename is undone, and so is the ask.
    let revert = call(&chain, &authority(), bridge, 0, calldata("addThenRevert(bytes32,uint64)", &[contract_word(&token), word(7)]));
    assert!(advance(&mut chain, vec![revert])[0].result.is_err());
    assert_eq!(named(&chain).as_deref(), Some("Renamed 9"));
}

#[test]
fn the_calldata_an_evm_tool_sends_calls_a_native_contract_straight_from_its_account() {
    let db = Arc::new(MemoryDb::default());
    let mut chain = open(&db);
    let (_, counter, _) = bridged(&mut chain);
    let precompile = Address::Evm(crate::vm::evm::NATIVE_PRECOMPILE);
    let (to, input) = crate::abi::call_native_input(&counter, "add", &[TxCallArgumentType::U64(6)]).unwrap();
    assert_eq!(to, crate::vm::evm::NATIVE_PRECOMPILE);

    // As the precompile's own interface encodes it, so Solidity and the tools agree.
    assert_eq!(input[..4], calldata("callNative(bytes32,string,string,bytes)", &[])[..4]);

    let sent = call(&chain, &authority(), precompile, 0, input);
    let receipt = advance(&mut chain, vec![sent]).remove(0);
    assert!(receipt.result.is_ok(), "{:?}", receipt.result);
    assert_eq!(total(&chain, &counter).as_deref(), Some("6"));
    assert_eq!(receipt.events[0].data[1], TxCallArgumentType::Address(authority().public().address()), "the account that sent it is its sender");

    // What has no ABI value, or is no native contract, does not encode.
    assert!(crate::abi::call_native_input(&counter, "add", &[TxCallArgumentType::Nil]).is_err());
    assert!(crate::abi::call_native_input(&authority().public().address(), "add", &[]).is_err());
}

/// Pentest, work not paid for: a contract that expands its memory to 1 MiB once, then makes
/// CALLS calls passing all of it as input, each for about a hundred gas. Tracing each frame must
/// not cost what its input is long, nor keep more frames than a trace holds.
#[test]
fn many_cheap_calls_with_a_megabyte_of_input_each_cost_the_trace_no_more_than_it_keeps() {
    const CALLS: u16 = 20_000;
    let [high, low] = CALLS.to_be_bytes();
    // MSTORE at 1 MiB less 32 grows memory once; then CALLS times CALL(gas, 0x1234, 0, 0, 1 MiB, 0, 0).
    let runtime = [
        &[0x60, 0x00, 0x62, 0x0f, 0xff, 0xe0, 0x52, 0x61, high, low, 0x5b][..],
        &[0x60, 0x00, 0x60, 0x00, 0x62, 0x10, 0x00, 0x00, 0x60, 0x00, 0x60, 0x00, 0x61, 0x12, 0x34, 0x5a, 0xf1, 0x50],
        &[0x60, 0x01, 0x90, 0x03, 0x80, 0x60, 0x0a, 0x57, 0x00],
    ]
    .concat();
    let length = u8::try_from(runtime.len()).unwrap();
    let init = [&[0x60, length, 0x80, 0x60, 0x0b, 0x60, 0x00, 0x39, 0x60, 0x00, 0xf3][..], &runtime].concat();

    let db = Arc::new(MemoryDb::default());
    let mut chain = open(&db);
    let caller = evm::create_address(&authority().public().address(), nonce(&chain, &authority()));
    let deploy = sign(&chain, &authority(), Tx::Deploy { code: Code::Evm(init) });
    assert_eq!(advance(&mut chain, vec![deploy])[0].result, Ok(Effect::Deployed(caller)));

    let calling = call(&chain, &authority(), caller, 0, Vec::new());
    let started = std::time::Instant::now();
    let receipt = advance(&mut chain, vec![calling]).remove(0);
    let took = started.elapsed();

    assert!(receipt.result.is_ok(), "{:?}", receipt.result);
    assert_eq!(receipt.trace.hops.len(), crate::structs::trace::MAX_HOPS, "as many as a trace holds");
    assert_eq!(receipt.trace.dropped as usize, 1 + usize::from(CALLS) - crate::structs::trace::MAX_HOPS, "the rest only counted");
    // Copying each call's megabyte would be 20 GB; reading its selector is nothing.
    assert!(took < std::time::Duration::from_secs(10), "{took:?}");
}

/// Pentest, work not paid for: a contract calling the native precompile in a loop, each native
/// call with a trace of its own. Past what a trace holds, the native calls' hops are only counted,
/// not kept until the transaction ends and thrown away.
#[test]
fn many_native_calls_from_the_evm_keep_no_more_hops_than_a_trace_holds() {
    const CALLS: u16 = 1_000;
    let db = Arc::new(MemoryDb::default());
    let mut chain = open(&db);
    let (_, counter, _) = bridged(&mut chain);
    let (_, data) = crate::abi::call_native_input(&counter, "total", &[]).unwrap();
    let len = u16::try_from(data.len()).unwrap().to_be_bytes();
    let calls = CALLS.to_be_bytes();
    // CODECOPY the calldata to memory, then CALLS times CALL(gas, 0x4e43, 0, 0, len, 0, 0).
    let runtime = [
        &[0x61, len[0], len[1], 0x61, 0x00, 0x27, 0x60, 0x00, 0x39, 0x61, calls[0], calls[1], 0x5b][..],
        &[0x60, 0x00, 0x60, 0x00, 0x61, len[0], len[1], 0x60, 0x00, 0x60, 0x00, 0x61, 0x4e, 0x43, 0x5a, 0xf1, 0x50],
        &[0x60, 0x01, 0x90, 0x03, 0x80, 0x60, 0x0c, 0x57, 0x00],
        &data,
    ]
    .concat();
    let size = u16::try_from(runtime.len()).unwrap().to_be_bytes();
    let init = [&[0x61, size[0], size[1], 0x80, 0x60, 0x0c, 0x60, 0x00, 0x39, 0x60, 0x00, 0xf3][..], &runtime].concat();
    let caller = evm::create_address(&authority().public().address(), nonce(&chain, &authority()));
    let deploy = sign(&chain, &authority(), Tx::Deploy { code: Code::Evm(init) });
    assert_eq!(advance(&mut chain, vec![deploy])[0].result, Ok(Effect::Deployed(caller)));

    let calling = call(&chain, &authority(), caller, 0, Vec::new());
    let receipt = advance(&mut chain, vec![calling]).remove(0);

    assert!(receipt.result.is_ok(), "{:?}", receipt.result);
    let max = crate::structs::trace::MAX_HOPS;
    assert_eq!(receipt.trace.hops.len(), max);
    // The top frame, a frame for each call to the precompile, and each native call's own hop.
    assert_eq!(receipt.trace.dropped as usize, 1 + 2 * usize::from(CALLS) - max);
    assert_eq!(receipt.trace.hops[1].to, Address::Evm(crate::vm::evm::NATIVE_PRECOMPILE));
    assert_eq!((receipt.trace.hops[2].to, receipt.trace.hops[2].function.as_str()), (counter, "total"));
}

#[test]
fn an_evm_read_over_proven_entries_alone_answers_as_the_node_did() {
    use crate::db::{Recording, STATE_COLUMNS, StateTree};
    use crate::vm::{EvmRead, ProvenRead, ProvenRunError, Vm, evm_read, run_proven_evm};

    let db = Arc::new(MemoryDb::default());
    let mut chain = open(&db);
    let token = deploy_token(&mut chain);
    let alice = stranger().public().address();
    let sent = send_token(&chain, token, &alice, 5);
    advance(&mut chain, vec![sent]);

    let tip = chain.tip().clone();
    let at: Arc<dyn crate::db::Committed> = db.clone();
    let rules = chain.rules.clone();
    let hash = rules.hash;
    let block = evm::EvmBlock { height: tip.height, timestamp: tip.timestamp, producer: tip.producer, chain_id: crate::crypto::ethereum::chain_id(&chain.genesis()), names: None, hash, vm: rules.vm };
    let Address::Evm(to) = token else { unreachable!("an EVM contract") };
    let read = EvmRead { from: [0; 20], to, input: calldata("balanceOf(address)", &[address_word(&alice)]) };
    let vm = Arc::new(Vm::new(&[crate::structs::code::Engine::Evm, crate::structs::code::Engine::Lua], hash).unwrap());

    // The node's run, over what it records.
    let recording = Arc::new(Recording::new(at.clone()));
    let ran = evm_read(&vm, recording.clone(), &read, block.clone(), 64 << 20).unwrap();
    assert_eq!(ran.result, Ok(evm::EvmReturned::Output(word(5))));
    assert!(!recording.scanned());

    // Every entry it read, proven at the tip, and the app's run over those alone.
    let state = Arc::new(State::new(at.clone()));
    let tree = StateTree::new(&state, hash);
    let reads: Vec<ProvenRead> = recording
        .reads()
        .into_iter()
        .filter(|(column, _)| STATE_COLUMNS.contains(column))
        .map(|(column, key)| ProvenRead { value: state.get(column, &key).unwrap(), proof: tree.prove(column, &key).unwrap(), column, key })
        .collect();
    assert_eq!(reads.len(), recording.reads().len(), "an EVM read reads nothing but the chain's state");
    let ours = run_proven_evm(&vm, &tip.state_root, &reads, &read, block.clone(), 64 << 20).unwrap();
    assert_eq!(ours.result, ran.result);

    // An entry left out is named; a value changed fails its proof.
    assert!(matches!(run_proven_evm(&vm, &tip.state_root, &reads[1..], &read, block.clone(), 64 << 20), Err(ProvenRunError::Missing { .. })));
    let mut forged = reads.clone();
    let last = forged.len() - 1;
    forged[last].value = Some(word(1_000_000));
    assert!(matches!(run_proven_evm(&vm, &tip.state_root, &forged, &read, block, 64 << 20), Err(ProvenRunError::Invalid { .. })));
}
