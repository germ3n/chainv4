use super::*;
use crate::consensus::{AuthorityProducer, DevAuthority, Producer, meets, work_hash};
use crate::crypto::keypair::Keypair;
use crate::db::{Committed, MemoryDb};
use crate::registry::contracts::contract_address;
use crate::registry::RegistryError;
use crate::structs::amount::Amount;
use crate::structs::code::{Code, Engine};
use crate::structs::tx::{FeeOffer, Tx, TxCallArgumentType};
use crate::vm::selector;
use crate::structs::block::SealedHeader;

const SLOT: u64 = 2_000;

const COUNTER: &str = r#"
local M = {}

function M.increment(by)
    local count = tonumber(storage.get("count") or "0") + by
    storage.set("count", tostring(count))
    return count
end

return M
"#;

fn authority() -> Keypair {
    DevAuthority::dev_keypair()
}

fn stranger() -> Keypair {
    Keypair::generate(authority().public().key_type()).unwrap()
}

/// The dev authority chain, free of fees so balances move only as the tests say; fees have tests
/// of their own.
fn genesis() -> Genesis {
    Genesis {
        fees: None,
        ..Genesis::dev_authority(authority().public())
    }
}

fn producer() -> AuthorityProducer {
    AuthorityProducer::new(authority(), SLOT)
}

fn open(db: &Arc<MemoryDb>) -> Chain {
    Chain::open(db.clone(), &genesis()).unwrap()
}

fn next_block(chain: &Chain, txs: Vec<SignedTx>) -> (Block, u64) {
    let now = chain.tip().timestamp + SLOT;
    let mut block = chain.build(txs, Vec::new(), producer().address(&[]), now).unwrap();
    block.seal = producer().seal(&block.header).unwrap();

    (block, now)
}

/// A block holding exactly these transactions, whether or not they can run: what a producer that
/// breaks the rules would send, since building leaves out what cannot run.
fn forced_block(chain: &Chain, txs: Vec<SignedTx>) -> (Block, u64) {
    let (mut block, now) = next_block(chain, Vec::new());
    block.header.tx_root = Block::tx_root(chain.hash(), &txs);
    block.txs = txs;
    block.seal = producer().seal(&block.header).unwrap();

    (block, now)
}

/// The receipts of an import that extended the tip.
fn extended(imported: Result<Imported, ImportError>) -> Vec<Receipt> {
    match imported.unwrap() {
        Imported::Extended(receipts) => receipts,
        other => panic!("expected the tip to be extended, got {other:?}"),
    }
}

/// Builds and imports the next block.
fn advance(chain: &mut Chain, txs: Vec<SignedTx>) -> Vec<Receipt> {
    let (block, now) = next_block(chain, txs);

    extended(chain.import(&block, now))
}

fn balance(chain: &Chain, address: &Address) -> u256 {
    Accounts::new(&chain.state()).get(address).unwrap().balance
}

fn nonce(chain: &Chain, keypair: &Keypair) -> u64 {
    Accounts::new(&chain.state()).get(&keypair.public().address()).unwrap().nonce
}

fn transfer(chain: &Chain, from: &Keypair, to: &Address, amount: u128) -> SignedTx {
    let tx = Tx::Transfer {
        from: from.public().address(),
        to: *to,
        amount: Amount::U128(amount),
    };

    SignedTx::sign(tx, nonce(chain, from), FeeOffer::NONE, from)
}

#[test]
fn genesis_funds_the_authority_and_survives_reopening() {
    let db = Arc::new(MemoryDb::default());
    let mut chain = open(&db);
    let funds = genesis().allocations[0].amount;

    assert_eq!(chain.tip().height, 0);
    assert_eq!(balance(&chain, &authority().public().address()), funds);

    advance(&mut chain, Vec::new());
    let reopened = open(&db);

    assert_eq!(reopened.tip().height, 1);
    assert_eq!(reopened.genesis(), chain.genesis());
    assert_eq!(balance(&reopened, &authority().public().address()), funds);
}

#[test]
fn a_dry_run_keeps_nothing_and_runs_at_the_next_nonce() {
    let db = Arc::new(MemoryDb::default());
    let genesis = genesis();
    let chain = Chain::open(db.clone(), &genesis).unwrap();
    let vm = Arc::new(Vm::new(&genesis.engines, genesis.hash).unwrap());
    let deployer = authority().public().address();
    // Signed at a nonce far from the sender's: a probe's own nonce does not matter.
    let probe = SignedTx::sign(Tx::Deploy { code: Code::Lua(COUNTER.to_string()) }, 99, FeeOffer::NONE, &authority());
    let ran = dry_run(db.clone(), &genesis.rules(), &vm, &probe, 0, u256::ZERO).unwrap();
    let state = chain.state();

    assert_eq!(ran.failure, None);
    assert!(ran.fuel_used > 0, "loading the script ran its top level");
    assert_eq!(Accounts::new(&state).get(&deployer).unwrap().nonce, 0, "nothing it did was kept");
    assert!(crate::registry::Contracts::new(&Arc::new(chain.state())).get(&contract_address(&deployer, 0, genesis.hash)).unwrap().is_none());

    let call = Tx::ContractCall { to: Address::Contract([9; 32]), func: Hash::Xxh3_64(selector("nope")), amount: Amount::U128(0), data: Vec::new() };
    let failing = dry_run(db, &genesis.rules(), &vm, &SignedTx::sign(call, 0, FeeOffer::NONE, &authority()), 0, u256::ZERO).unwrap();

    assert!(failing.failure.is_some_and(|why| why.contains("no contract")), "it says why it would fail");
}

#[test]
fn max_block_bytes_must_hold_the_largest_deploy() {
    let small = Genesis { max_block_bytes: Some(crate::chain::MIN_BLOCK_BYTES - 1), ..genesis() };
    let err = Chain::open(Arc::new(MemoryDb::default()), &small).err().unwrap();

    assert!(err.to_string().contains("max_block_bytes must be at least 1 MiB"), "{err}");
    assert!(Chain::open(Arc::new(MemoryDb::default()), &Genesis { max_block_bytes: Some(crate::chain::MIN_BLOCK_BYTES), ..genesis() }).is_ok());

    let big = Genesis { max_block_bytes: Some(crate::chain::MAX_BLOCK_BYTES + 1), ..genesis() };
    let err = Chain::open(Arc::new(MemoryDb::default()), &big).err().unwrap();

    assert!(err.to_string().contains("at most 6 MiB"), "so a full block fits one gossip message: {err}");
    assert!(Chain::open(Arc::new(MemoryDb::default()), &Genesis { max_block_bytes: Some(crate::chain::MAX_BLOCK_BYTES), ..genesis() }).is_ok());
}

#[test]
fn blocks_past_max_block_bytes_are_refused_and_never_built() {
    let authority = DevAuthority::dev_keypair();
    let genesis = Genesis { max_block_bytes: Some(crate::chain::MIN_BLOCK_BYTES), ..genesis() };
    let mut chain = Chain::open(Arc::new(MemoryDb::default()), &genesis).unwrap();
    // Calls carrying 300 KiB each: four pass the limit.
    let big = |nonce: u64| {
        let tx = Tx::ContractCall {
            to: Address::Contract([9; 32]),
            func: Hash::Xxh3_64(1),
            amount: Amount::U128(0),
            data: vec![TxCallArgumentType::Bytes(vec![7; 300 * 1024])],
        };

        SignedTx::sign(tx, nonce, FeeOffer::NONE, &authority)
    };
    let txs: Vec<SignedTx> = (0..4).map(big).collect();
    let total: u64 = txs.iter().map(crate::chain::tx_bytes).sum();

    assert!(total > crate::chain::MIN_BLOCK_BYTES && crate::chain::tx_bytes(&txs[0]) * 3 < crate::chain::MIN_BLOCK_BYTES);

    // Building leaves out what would pass it.
    let built = chain.build(txs.clone(), Vec::new(), authority.public().address(), 1_000).unwrap();

    assert_eq!(built.txs.len(), 3);

    // A block that holds them all anyway is refused on import, before anything runs.
    let mut stuffed = built.clone();
    stuffed.txs = txs;
    stuffed.header.tx_root = Block::tx_root(chain.hash(), &stuffed.txs);
    let err = chain.import(&stuffed, 1_000).unwrap_err();

    assert!(matches!(err, ImportError::Invalid(BlockError::TooBig { bytes, limit }) if bytes == total && limit == crate::chain::MIN_BLOCK_BYTES), "{err}");
}

#[test]
fn a_different_genesis_is_refused() {
    let db = Arc::new(MemoryDb::default());
    open(&db);
    let other = Genesis {
        consensus: ConsensusConfig::Authority {
            key: authority().public(),
            slot_ms: 1_000,
        },
        ..genesis()
    };

    assert!(matches!(
        Chain::open(db.clone(), &other),
        Err(ChainError::WrongGenesis { .. })
    ));
}

#[test]
fn collection_functions_named_twice_or_blank_are_refused() {
    use crate::registry::collections::CollectionFunctions;

    for collections in [
        CollectionFunctions { nft: vec!["owner_of".to_string(), "owner_of".to_string()], multi: Vec::new() },
        CollectionFunctions { nft: Vec::new(), multi: vec![String::new()] },
    ] {
        let genesis = Genesis { collections: Some(collections), ..genesis() };

        assert!(matches!(
            Chain::open(Arc::new(MemoryDb::default()), &genesis),
            Err(ChainError::BadGenesis(why)) if why.contains("collections must name each function once")
        ));
    }
}

#[test]
fn token_functions_named_twice_or_blank_are_refused() {
    for token_functions in [vec!["transfer".to_string(), "transfer".to_string()], vec![String::new()]] {
        let genesis = Genesis { token_functions, ..genesis() };

        assert!(matches!(
            Chain::open(Arc::new(MemoryDb::default()), &genesis),
            Err(ChainError::BadGenesis(why)) if why.contains("token_functions")
        ));
    }
}

#[test]
fn a_deployed_token_is_registered_and_leaves_with_its_block() {
    let token_code = Code::Lua(include_str!("../../contracts/lua/token.lua").to_string());
    let deploy = SignedTx::sign(Tx::Deploy { code: token_code }, 0, FeeOffer::NONE, &authority());
    let token = contract_address(&authority().public().address(), 0, HashType::default_chain());
    let miner = Address::Ed25519([9; 32]);
    let registered = |chain: &Chain| crate::registry::Tokens::new(&chain.state()).get(&token).unwrap();

    let mut chain = Chain::open(Arc::new(MemoryDb::default()), &pow_genesis()).unwrap();
    let genesis_block = chain.block_at(0).unwrap().unwrap();
    let a1 = mine_on(&chain, &genesis_block, vec![deploy.clone()], miner);
    extended(chain.import(&a1, LATER));

    assert_eq!(registered(&chain).and_then(|token| token.symbol).as_deref(), Some("EXT"));

    // A heavier branch without the deploy takes the token off.
    let b1 = mine_on(&chain, &genesis_block, Vec::new(), miner);
    chain.import(&b1, LATER).unwrap();
    let b2 = mine_on(&chain, &b1, Vec::new(), miner);
    assert!(matches!(chain.import(&b2, LATER), Ok(Imported::Reorganized { .. })));

    assert_eq!(registered(&chain), None);

    // A chain whose genesis names no token functions keeps no registry.
    let unlisted = Genesis { token_functions: Vec::new(), ..pow_genesis() };
    let mut chain = Chain::open(Arc::new(MemoryDb::default()), &unlisted).unwrap();
    let genesis_block = chain.block_at(0).unwrap().unwrap();
    let deployed = mine_on(&chain, &genesis_block, vec![deploy], miner);
    extended(chain.import(&deployed, LATER));

    assert!(chain.state().get_value::<crate::registry::contracts::ContractRecord>(Column::Contracts, &crate::db::address_key(&token)).unwrap().is_some());
    assert_eq!(registered(&chain), None);
}

#[test]
fn a_token_that_asks_is_asked_again_once_its_transaction_succeeds() {
    let source = format!(
        "{}\n{}",
        r#"local M = {}
function M.total_supply() return 0 end
function M.balance_of(owner) return 0 end
function M.transfer(to, amount) return true end
function M.approve(spender, amount) return true end
function M.allowance(owner, spender) return 0 end
function M.transfer_from(from, to, amount) return true end
function M.name() return storage.get("name") or "First" end"#,
        r#"function M.rename(name) storage.set("name", name) tokens.refresh() end
return M"#
    );
    let token = contract_address(&authority().public().address(), 0, HashType::default_chain());
    let rename = |name: &str, nonce: u64| {
        let tx = Tx::ContractCall {
            to: token,
            func: Hash::Xxh3_64(selector("rename")),
            amount: Amount::U128(0),
            data: vec![TxCallArgumentType::String(name.to_string())],
        };

        SignedTx::sign(tx, nonce, FeeOffer::NONE, &authority())
    };
    let named = |chain: &Chain| crate::registry::Tokens::new(&chain.state()).get(&token).unwrap().and_then(|token| token.name);
    let miner = Address::Ed25519([9; 32]);
    let mut chain = Chain::open(Arc::new(MemoryDb::default()), &pow_genesis()).unwrap();
    let genesis_block = chain.block_at(0).unwrap().unwrap();
    let deployed = mine_on(&chain, &genesis_block, vec![SignedTx::sign(Tx::Deploy { code: Code::Lua(source) }, 0, FeeOffer::NONE, &authority())], miner);
    extended(chain.import(&deployed, LATER));

    assert_eq!(named(&chain).as_deref(), Some("First"));

    let renamed = mine_on(&chain, &deployed, vec![rename("Better", 1)], miner);
    let receipts = extended(chain.import(&renamed, LATER));

    assert!(receipts[0].result.is_ok(), "{:?}", receipts[0].result);
    assert_eq!(named(&chain).as_deref(), Some("Better"));

    // An answer that does not fit fails the whole transaction: the rename is gone too.
    let emptied = mine_on(&chain, &renamed, vec![rename("", 2)], miner);
    let receipts = extended(chain.import(&emptied, LATER));

    assert!(
        matches!(&receipts[0].result, Err(Failure::Registry(RegistryError::BadToken(why))) if why.contains("name()")),
        "{:?}",
        receipts[0].result
    );
    assert!(receipts[0].fuel_used > 0);
    assert_eq!(named(&chain).as_deref(), Some("Better"));
}

#[test]
fn a_coin_nobody_could_show_is_refused() {
    for coin in [
        Coin { symbol: String::new(), decimals: 18 },
        Coin { symbol: "TOO-LONG".to_string(), decimals: 18 },
        Coin { symbol: "DEV".to_string(), decimals: 78 },
    ] {
        let genesis = Genesis { coin, ..genesis() };

        assert!(matches!(Chain::open(Arc::new(MemoryDb::default()), &genesis), Err(ChainError::BadGenesis(_))));
    }
}

#[test]
fn a_database_kept_in_another_format_is_refused() {
    let db = Arc::new(MemoryDb::default());
    open(&db);
    let state = State::new(db.clone());
    state.put_value(Column::Meta, FORMAT_KEY.to_vec(), &FormatRecord { format: STORAGE_FORMAT + 1 });
    db.commit(state.take_changes()).unwrap();

    let newer = Chain::open(db.clone(), &genesis()).err().unwrap();

    assert!(matches!(newer, ChainError::OldFormat { found: Some(found), expected: STORAGE_FORMAT } if found == STORAGE_FORMAT + 1));
    assert!(newer.to_string().contains(&format!("take it back with `chainv4 db migrate --to {STORAGE_FORMAT}`")), "{newer}");

    // A migration stopped part way leaves the format saying what the database no longer wholly is.
    let state = State::new(db.clone());
    state.put_value(Column::Meta, FORMAT_KEY.to_vec(), &FormatRecord { format: STORAGE_FORMAT });
    let stopped = migrate::MigratingRecord {
        from: STORAGE_FORMAT,
        to: STORAGE_FORMAT - 1,
        cursor: Some(b"halfway".to_vec()),
    };
    state.put_value(Column::Meta, migrate::MIGRATING_KEY.to_vec(), &stopped);
    db.commit(state.take_changes()).unwrap();

    assert!(matches!(Chain::open(db.clone(), &genesis()), Err(ChainError::Migrating { from: STORAGE_FORMAT, .. })));

    let state = State::new(db.clone());
    state.delete(Column::Meta, FORMAT_KEY.to_vec());
    state.delete(Column::Meta, migrate::MIGRATING_KEY.to_vec());
    db.commit(state.take_changes()).unwrap();

    assert!(matches!(Chain::open(db.clone(), &genesis()), Err(ChainError::OldFormat { found: None, .. })));
}

#[test]
fn transfers_run_and_failures_still_use_the_nonce() {
    let db = Arc::new(MemoryDb::default());
    let mut chain = open(&db);
    let bob = stranger().public().address();
    let funds = genesis().allocations[0].amount;
    let sent = transfer(&chain, &authority(), &bob, 100);
    let receipts = advance(&mut chain, vec![sent]);

    assert_eq!(receipts[0].result, Ok(Effect::Done));
    assert_eq!(balance(&chain, &bob), u256::new(100));

    let too_much = transfer(&chain, &authority(), &bob, u128::MAX);
    let receipts = advance(&mut chain, vec![too_much]);

    assert_eq!(
        receipts[0].result,
        Err(Failure::Registry(RegistryError::InsufficientBalance))
    );
    assert_eq!(nonce(&chain, &authority()), 2);
    assert_eq!(balance(&chain, &authority().public().address()), funds - u256::new(100));
}

#[cfg(feature = "secp256k1")]
#[test]
fn an_ethereum_key_receives_and_sends_as_any_other() {
    use crate::crypto::keypair::KeyType;
    use crate::crypto::verifiable::{Verifiable, VerifyError};

    let db = Arc::new(MemoryDb::default());
    let mut chain = open(&db);
    let eth = Keypair::generate(KeyType::Secp256k1).unwrap();
    let address = eth.public().address();
    let me = authority().public().address();

    assert!(matches!(address, Address::Secp256k1(_)), "{address}");
    let fund = transfer(&chain, &authority(), &address, 1_000);

    assert_eq!(advance(&mut chain, vec![fund])[0].result, Ok(Effect::Done));

    let back = transfer(&chain, &eth, &me, 400);

    assert_eq!(back.verify(), Ok(()));
    assert_eq!(advance(&mut chain, vec![back])[0].result, Ok(Effect::Done));
    assert_eq!((balance(&chain, &address), nonce(&chain, &eth)), (u256::new(600), 1));

    // Signed by another key in the address's name, it never verifies.
    let mut forged = transfer(&chain, &stranger(), &me, 1);
    forged.signer = eth.public();

    assert_eq!(forged.verify(), Err(VerifyError::InvalidSignature));
}

#[test]
fn only_the_authority_mints() {
    let db = Arc::new(MemoryDb::default());
    let mut chain = open(&db);
    let bob = stranger();
    let mint = |keypair: &Keypair, chain: &Chain| {
        let tx = Tx::Mint {
            to: bob.public().address(),
            amount: Amount::U128(5),
        };

        SignedTx::sign(tx, nonce(chain, keypair), FeeOffer::NONE, keypair)
    };
    let tx = mint(&bob, &chain);
    let receipts = advance(&mut chain, vec![tx]);

    assert_eq!(receipts[0].result, Err(Failure::NotMinter));

    let tx = mint(&authority(), &chain);
    let receipts = advance(&mut chain, vec![tx]);

    assert_eq!(receipts[0].result, Ok(Effect::Done));
    assert_eq!(balance(&chain, &bob.public().address()), u256::new(5));
}

#[test]
fn contracts_deploy_and_run_through_transactions() {
    let db = Arc::new(MemoryDb::default());
    let mut chain = open(&db);
    let deployer = authority();
    let deploy = SignedTx::sign(
        Tx::Deploy {
            code: Code::Lua(COUNTER.to_string()),
        },
        nonce(&chain, &deployer),
        FeeOffer::NONE,
        &deployer,
    );
    let receipts = advance(&mut chain, vec![deploy]);
    let counter = contract_address(&deployer.public().address(), 0, HashType::default_chain());

    assert_eq!(receipts[0].result, Ok(Effect::Deployed(counter)));

    for expected in [5, 10] {
        let call = SignedTx::sign(
            Tx::ContractCall {
                to: counter,
                func: Hash::Xxh3_64(selector("increment")),
                amount: Amount::U128(0),
                data: vec![TxCallArgumentType::I64(5)],
            },
            nonce(&chain, &deployer),
            FeeOffer::NONE,
            &deployer,
        );
        let receipts = advance(&mut chain, vec![call]);

        assert_eq!(
            receipts[0].result,
            Ok(Effect::Returned(vec![TxCallArgumentType::I64(expected)]))
        );
        assert!(receipts[0].fuel_used > 0);
    }
}

#[test]
fn blocks_must_follow_the_rules() {
    let db = Arc::new(MemoryDb::default());
    let mut chain = open(&db);
    let (block, now) = next_block(&chain, Vec::new());
    let (sibling, _) = next_block(&chain, vec![transfer(&chain, &authority(), &stranger().public().address(), 1)]);
    let mut bad_root = block.clone();
    bad_root.header.tx_root = Hash::from_bytes(HashType::default_chain(), &[9; 32]).unwrap();
    let mut bad_height = block.clone();
    bad_height.header.height = 5;

    assert!(matches!(
        chain.import(&bad_root, now),
        Err(ImportError::Invalid(BlockError::TxRoot))
    ));
    assert!(matches!(
        chain.import(&bad_height, now),
        Err(ImportError::Invalid(BlockError::Height))
    ));

    extended(chain.import(&block, now));
    let (grandchild, later) = next_block(&chain, Vec::new());
    let mut orphan = grandchild.clone();
    orphan.header.parent = Hash::from_bytes(HashType::default_chain(), &[7; 32]).unwrap();

    assert!(matches!(chain.import(&block, now), Err(ImportError::Known)));
    assert!(matches!(chain.import(&sibling, now), Ok(Imported::Side)));
    assert!(matches!(chain.import(&orphan, later), Err(ImportError::UnknownParent(_))));
}

#[test]
fn a_block_with_an_invalid_transaction_changes_nothing() {
    let db = Arc::new(MemoryDb::default());
    let mut chain = open(&db);
    let bob = stranger().public().address();
    let good = transfer(&chain, &authority(), &bob, 10);
    let mut replayed = transfer(&chain, &authority(), &bob, 20);
    replayed = SignedTx::sign(replayed.tx, 0, FeeOffer::NONE, &authority());

    // Building leaves the replay out.
    assert_eq!(next_block(&chain, vec![good.clone(), replayed.clone()]).0.txs, std::slice::from_ref(&good));

    let (block, now) = forced_block(&chain, vec![good, replayed]);

    assert!(matches!(
        chain.import(&block, now),
        Err(ImportError::Invalid(BlockError::Tx {
            index: 1,
            error: TxError::WrongNonce { expected: 1, got: 0 },
        }))
    ));
    assert_eq!(chain.tip().height, 0);
    assert_eq!(balance(&chain, &bob), u256::ZERO);
    assert_eq!(nonce(&chain, &authority()), 0);
}

#[test]
fn deploys_for_engines_genesis_leaves_out_fail() {
    let db = Arc::new(MemoryDb::default());
    let lua_only = Genesis {
        engines: vec![Engine::Lua],
        ..genesis()
    };
    let mut chain = Chain::open(db, &lua_only).unwrap();
    let module = vec![0, b'a', b's', b'm', 1, 0, 0, 0];
    let deploy = SignedTx::sign(Tx::Deploy { code: Code::Wasm(module) }, 0, FeeOffer::NONE, &authority());
    let (block, now) = next_block(&chain, vec![deploy]);
    let receipts = extended(chain.import(&block, now));

    assert_eq!(receipts[0].result, Err(Failure::EngineNotAllowed(Engine::Wasm)));
    assert_eq!(nonce(&chain, &authority()), 1);
}

#[test]
fn contracts_pass_on_what_a_transaction_sends_them() {
    const SPLITTER: &str = r#"
local M = {}

function M.split(first, second)
    local half = ctx.value / 2
    accounts.transfer(first, half)
    accounts.transfer(second, ctx.value - half)
end

function M.relay(splitter, first, second)
    contracts.call_value(splitter, ctx.value, "split", first, second)
end

function M.split_then_fail(first)
    accounts.transfer(first, ctx.value)
    error("changed my mind")
end

function M.try_split(splitter, first)
    pcall(contracts.call_value, splitter, ctx.value, "split_then_fail", first)
end

return M
"#;
    let db = Arc::new(MemoryDb::default());
    let mut chain = open(&db);
    let deployer = authority();
    let deploy = SignedTx::sign(Tx::Deploy { code: Code::Lua(SPLITTER.to_string()) }, 0, FeeOffer::NONE, &deployer);
    advance(&mut chain, vec![deploy]);
    let splitter = contract_address(&deployer.public().address(), 0, HashType::default_chain());
    let (first, second) = (stranger().public().address(), stranger().public().address());
    let call = SignedTx::sign(
        Tx::ContractCall {
            to: splitter,
            func: Hash::Xxh3_64(selector("split")),
            amount: Amount::U128(101),
            data: vec![TxCallArgumentType::Address(first), TxCallArgumentType::Address(second)],
        },
        1,
        FeeOffer::NONE,
        &deployer,
    );
    let receipts = advance(&mut chain, vec![call]);

    assert_eq!(receipts[0].result, Ok(Effect::Returned(Vec::new())));
    assert_eq!(balance(&chain, &first), u256::new(50));
    assert_eq!(balance(&chain, &second), u256::new(51));
    assert_eq!(balance(&chain, &splitter), u256::ZERO);

    // What it paid out, line by line, in the receipt and the root it goes in.
    let paid = |from: Address, to: Address, amount: u128| crate::structs::receipt::Payment { from, to, amount: u256::new(amount) };

    assert_eq!(receipts[0].payments, [paid(splitter, first, 50), paid(splitter, second, 51)]);
    assert_eq!(crate::structs::receipt::CommittedReceipt::of(&receipts[0]).payments, receipts[0].payments);

    // Through another contract: the value it sent with its call first, then what the callee paid.
    let deploy = SignedTx::sign(Tx::Deploy { code: Code::Lua(SPLITTER.to_string()) }, 2, FeeOffer::NONE, &deployer);
    advance(&mut chain, vec![deploy]);
    let relay = contract_address(&deployer.public().address(), 2, HashType::default_chain());
    let call = |function: &str, data: Vec<TxCallArgumentType>, nonce: u64| {
        let tx = Tx::ContractCall { to: relay, func: Hash::Xxh3_64(selector(function)), amount: Amount::U128(10), data };

        SignedTx::sign(tx, nonce, FeeOffer::NONE, &deployer)
    };
    let (a, b) = (TxCallArgumentType::Address(first), TxCallArgumentType::Address(second));
    let receipts = advance(&mut chain, vec![
        call("relay", vec![TxCallArgumentType::Address(splitter), a.clone(), b.clone()], 3),
        call("split_then_fail", vec![a.clone()], 4),
        call("try_split", vec![TxCallArgumentType::Address(splitter), a.clone()], 5),
    ]);

    assert_eq!(receipts[0].payments, [paid(relay, splitter, 10), paid(splitter, first, 5), paid(splitter, second, 5)]);
    assert!(receipts[1].result.is_err() && receipts[1].payments.is_empty(), "a call that failed paid nothing");
    assert!(receipts[2].result.is_ok() && receipts[2].payments.is_empty(), "nor one whose caller caught its failure");
}

// Proof of work: forks, reorgs and rewards. Difficulty 1 lets any nonce through, and blocks a
// target apart keep it there, so every block weighs the same and the longer branch wins.

/// Far enough ahead that no block in these tests is from the future.
const LATER: u64 = 10_000_000;

fn pow_genesis() -> Genesis {
    Genesis {
        consensus: ConsensusConfig::ProofOfWork(PowConfig {
            hash: HashType::default_chain(),
            target_block_ms: 1_000,
            initial_difficulty: 1,
            min_difficulty: 1,
        }),
        reward: Reward {
            initial: u256::new(100),
            halving_interval: Some(2),
        },
        fees: None,
        ..Genesis::dev_pow(authority().public())
    }
}

/// Mines a block on any stored block, one target after it, on a chain whose work hash is its
/// chain hash, as every proof-of-work chain in these tests is. Import the parent first: the roots
/// need its state. A block meant to be invalid, with a transaction that cannot run, on a branch
/// that does not, or on one never imported, gets zero roots by hand: it is refused for what is
/// wrong with it before its roots are looked at.
fn mine_on(chain: &Chain, parent: &Block, txs: Vec<SignedTx>, miner: Address) -> Block {
    mine_after(chain, parent, 1_000, txs, miner)
}

/// mine_on, this many milliseconds after the parent: closer than the target raises difficulty
/// over the window, further lowers it.
fn mine_after(chain: &Chain, parent: &Block, gap_ms: u64, txs: Vec<SignedTx>, miner: Address) -> Block {
    let timestamp = parent.header.timestamp + gap_ms;
    let mut block = match chain.build_on(&parent.id(), txs.clone(), Vec::new(), miner, timestamp) {
        Ok(built) if built.txs == txs => built,
        _ => Block {
            header: BlockHeader {
                parent: parent.id(),
                height: parent.header.height + 1,
                timestamp,
                producer: miner,
                difficulty: chain.difficulty_after(&parent.header, timestamp).unwrap(),
                tx_root: Block::tx_root(chain.hash(), &txs),
                proof_root: Block::proof_root(chain.hash(), &[]),
                pool_root: Block::pool_root(chain.hash(), None),
                receipts_root: Hash::zero(chain.hash()),
                state_root: Hash::zero(chain.hash()),
                gas_used: 0,
                base_price: u256::ZERO,
            },
            txs,
            proofs: Vec::new(),
            pool: None,
            seal: Seal::None,
        },
    };
    let nonce = (0..)
        .find(|nonce| meets(work_hash(chain.hash(), &block.id(), *nonce), block.header.difficulty))
        .unwrap();
    block.seal = Seal::Work { nonce };

    block
}

fn send(to: &Address, amount: u128, nonce: u64) -> SignedTx {
    let tx = Tx::Transfer {
        from: authority().public().address(),
        to: *to,
        amount: Amount::U128(amount),
    };

    SignedTx::sign(tx, nonce, FeeOffer::NONE, &authority())
}

#[test]
fn the_heavier_branch_wins_and_state_follows_it() {
    let db = Arc::new(MemoryDb::default());
    let mut chain = Chain::open(db, &pow_genesis()).unwrap();
    let genesis = chain.block_at(0).unwrap().unwrap();
    let (bob, carol) = (stranger().public().address(), stranger().public().address());
    let (miner_a, miner_b) = (Address::Ed25519([1; 32]), Address::Ed25519([2; 32]));
    let to_bob = send(&bob, 10, 0);
    let to_carol = send(&carol, 20, 0);
    let a1 = mine_on(&chain, &genesis, vec![to_bob.clone()], miner_a);
    let b1 = mine_on(&chain, &genesis, vec![to_carol.clone()], miner_b);

    extended(chain.import(&a1, LATER));
    assert!(matches!(chain.import(&b1, LATER), Ok(Imported::Side)));
    assert_eq!(balance(&chain, &bob), u256::new(10));

    let b2 = mine_on(&chain, &b1, Vec::new(), miner_b);

    match chain.import(&b2, LATER).unwrap() {
        Imported::Reorganized { undone, applied } => {
            assert_eq!(undone, std::slice::from_ref(&a1));
            assert_eq!(applied, [b1.clone(), b2.clone()]);
        }
        other => panic!("{other:?}"),
    }

    assert_eq!(chain.tip().id(), b2.id());
    assert_eq!(chain.block_at(1).unwrap().unwrap(), b1);
    assert_eq!((balance(&chain, &bob), balance(&chain, &carol)), (u256::ZERO, u256::new(20)));
    // 100 for height 1, halved to 50 for height 2; nothing for the block that was taken off.
    assert_eq!((balance(&chain, &miner_a), balance(&chain, &miner_b)), (u256::ZERO, u256::new(150)));
    assert_eq!(chain.receipt(&to_bob.id(HashType::default_chain())).unwrap(), None);
    assert_eq!(chain.receipt(&to_carol.id(HashType::default_chain())).unwrap().unwrap().block, b1.id());

    let a2 = mine_on(&chain, &a1, Vec::new(), miner_a);

    assert!(matches!(chain.import(&a2, LATER), Ok(Imported::Side)));

    let a3 = mine_on(&chain, &a2, Vec::new(), miner_a);

    assert!(matches!(chain.import(&a3, LATER), Ok(Imported::Reorganized { .. })));
    assert_eq!((balance(&chain, &bob), balance(&chain, &carol)), (u256::new(10), u256::ZERO));
    assert_eq!((balance(&chain, &miner_a), balance(&chain, &miner_b)), (u256::new(200), u256::ZERO));
    assert_eq!(chain.receipt(&to_bob.id(HashType::default_chain())).unwrap().unwrap().block, a1.id());
    assert_eq!(chain.receipt(&to_carol.id(HashType::default_chain())).unwrap(), None);
    assert_eq!(chain.tip_work(), u256::new(3));
}

#[test]
fn side_branches_can_be_found_in_the_database() {
    let db = Arc::new(MemoryDb::default());
    let mut chain = Chain::open(db.clone(), &pow_genesis()).unwrap();
    let genesis = chain.block_at(0).unwrap().unwrap();
    let a1 = mine_on(&chain, &genesis, Vec::new(), Address::Ed25519([1; 32]));
    let b1 = mine_on(&chain, &genesis, Vec::new(), Address::Ed25519([2; 32]));

    extended(chain.import(&a1, LATER));
    assert!(matches!(chain.import(&b1, LATER), Ok(Imported::Side)));

    let leaves = std::collections::BTreeSet::from([a1.id(), b1.id()]);

    assert_eq!(stored_tips(&*db).unwrap(), leaves.iter().copied().collect::<Vec<_>>(), "kept in order");
    assert!(stored_on_main_chain(&*db, &a1).unwrap());
    assert!(!stored_on_main_chain(&*db, &b1).unwrap());
    assert_eq!(Chain::open(db.clone(), &pow_genesis()).unwrap().tips, leaves);
}

#[test]
fn a_heavier_branch_that_fails_to_run_is_dropped() {
    let db = Arc::new(MemoryDb::default());
    let mut chain = Chain::open(db, &pow_genesis()).unwrap();
    let genesis = chain.block_at(0).unwrap().unwrap();
    let bob = stranger().public().address();
    let a1 = mine_on(&chain, &genesis, vec![send(&bob, 10, 0)], Address::Ed25519([1; 32]));
    // Nonce 7 is not the sender's next, which only running the block finds out.
    let b1 = mine_on(&chain, &genesis, vec![send(&bob, 99, 7)], Address::Ed25519([2; 32]));
    let b2 = mine_on(&chain, &b1, Vec::new(), Address::Ed25519([2; 32]));
    let b3 = mine_on(&chain, &b2, Vec::new(), Address::Ed25519([2; 32]));

    extended(chain.import(&a1, LATER));
    assert!(matches!(chain.import(&b1, LATER), Ok(Imported::Side)));
    assert!(matches!(
        chain.import(&b2, LATER),
        Err(ImportError::Invalid(BlockError::Tx { index: 0, .. }))
    ));
    assert!(matches!(chain.import(&b3, LATER), Err(ImportError::Invalid(BlockError::Bad))));
    assert_eq!(chain.tip().id(), a1.id());
    assert_eq!(balance(&chain, &bob), u256::new(10));
}

#[test]
fn bad_blocks_are_remembered_across_restarts() {
    let db = Arc::new(MemoryDb::default());
    let mut chain = Chain::open(db.clone(), &pow_genesis()).unwrap();
    let genesis = chain.block_at(0).unwrap().unwrap();
    let bob = stranger().public().address();
    let a1 = mine_on(&chain, &genesis, Vec::new(), Address::Ed25519([1; 32]));
    let b1 = mine_on(&chain, &genesis, vec![send(&bob, 99, 7)], Address::Ed25519([2; 32]));
    let b2 = mine_on(&chain, &b1, Vec::new(), Address::Ed25519([2; 32]));

    extended(chain.import(&a1, LATER));
    assert!(matches!(chain.import(&b1, LATER), Ok(Imported::Side)));
    assert!(matches!(chain.import(&b2, LATER), Err(ImportError::Invalid(BlockError::Tx { .. }))), "running the branch finds the bad nonce");
    assert_eq!(chain.bad_blocks(), [b1.id(), b2.id()]);

    // Reopened, the chain refuses the branch without running it again.
    let mut reopened = Chain::open(db, &pow_genesis()).unwrap();
    let b3 = mine_on(&reopened, &b2, Vec::new(), Address::Ed25519([2; 32]));

    assert_eq!(reopened.bad_blocks(), [b1.id(), b2.id()]);
    assert!(matches!(reopened.import(&b2, LATER), Err(ImportError::Invalid(BlockError::Bad))));
    assert!(matches!(reopened.import(&b3, LATER), Err(ImportError::Invalid(BlockError::Bad))));
    assert_eq!(reopened.bad_blocks(), [b1.id(), b2.id(), b3.id()]);
}

#[test]
fn side_branches_the_chain_can_never_follow_are_pruned() {
    let db = Arc::new(MemoryDb::default());
    // Without finality, the reorganization depth decides what can be followed; with it, side
    // branches are kept down to the finalized checkpoint, as finality's tests show.
    let mut chain = Chain::open(db.clone(), &Genesis { finality: None, ..pow_genesis() }).unwrap();
    let genesis = chain.block_at(0).unwrap().unwrap();
    let mut main = genesis.clone();

    for _ in 0..DEV_MAX_REORG_DEPTH {
        main = mine_on(&chain, &main, Vec::new(), Address::Ed25519([1; 32]));
        extended(chain.import(&main, LATER));
    }

    // A branch off genesis, exactly max_reorg_depth below the tip: still followable, so it stays
    // through imports and a tip move, and so does a fresh one off the tip's parent.
    let old = mine_on(&chain, &genesis, Vec::new(), Address::Ed25519([2; 32]));
    let old_child = mine_on(&chain, &old, Vec::new(), Address::Ed25519([2; 32]));
    let fresh = mine_on(&chain, &chain.block_at(DEV_MAX_REORG_DEPTH - 1).unwrap().unwrap(), Vec::new(), Address::Ed25519([3; 32]));

    for side in [&old, &old_child, &fresh] {
        assert!(matches!(chain.import(side, LATER), Ok(Imported::Side)));
    }

    assert!(chain.block(&old_child.id()).unwrap().is_some());

    // One more main block puts the old branch's fork past max_reorg_depth: it goes, the fresh one stays.
    main = mine_on(&chain, &main, Vec::new(), Address::Ed25519([1; 32]));
    extended(chain.import(&main, LATER));

    assert_eq!(chain.block(&old.id()).unwrap(), None);
    assert_eq!(chain.block(&old_child.id()).unwrap(), None);
    assert_eq!(stored_header(&*db, &old_child.id()).unwrap(), None);
    assert_eq!(stored_work(&*db, &old_child.id()).unwrap(), None);
    assert!(chain.block(&fresh.id()).unwrap().is_some());
    assert_eq!(stored_tips(&*db).unwrap().into_iter().collect::<HashSet<_>>(), HashSet::from([main.id(), fresh.id()]));
    assert_eq!(chain.tips, std::collections::BTreeSet::from([main.id(), fresh.id()]));

    // The main chain keeps every block, and a block on the pruned branch is just unknown now.
    assert!(chain.block_at(0).unwrap().is_some());
    assert!(matches!(chain.import(&old_child, LATER), Err(ImportError::UnknownParent(_))));
}

#[test]
fn branches_that_split_off_too_deep_are_never_followed() {
    let db = Arc::new(MemoryDb::default());
    // Without finality, depth alone decides how far back the chain can go.
    let mut chain = Chain::open(db.clone(), &Genesis { finality: None, ..pow_genesis() }).unwrap();
    let genesis = chain.block_at(0).unwrap().unwrap();
    let mut main = genesis.clone();

    for _ in 0..DEV_MAX_REORG_DEPTH + 2 {
        main = mine_on(&chain, &main, Vec::new(), Address::Ed25519([1; 32]));
        extended(chain.import(&main, LATER));
    }

    let first = chain.block_at(1).unwrap().unwrap();
    let second = chain.block_at(2).unwrap().unwrap();

    // Undo records only reach back DEV_MAX_REORG_DEPTH blocks.
    assert_eq!(db.read(Column::Undo, &id_key(&first.id())).unwrap(), None);
    assert!(db.read(Column::Undo, &id_key(&second.id())).unwrap().is_some());

    // With finality and nothing finalized, they all stay: a justified branch may need them.
    let finality_db = Arc::new(MemoryDb::default());
    let mut voting = Chain::open(finality_db.clone(), &pow_genesis()).unwrap();
    let mut tip = voting.block_at(0).unwrap().unwrap();

    for _ in 0..DEV_MAX_REORG_DEPTH + 2 {
        tip = mine_on(&voting, &tip, Vec::new(), Address::Ed25519([1; 32]));
        extended(voting.import(&tip, LATER));
    }

    let unfinalized = voting.block_at(1).unwrap().unwrap();
    assert!(finality_db.read(Column::Undo, &id_key(&unfinalized.id())).unwrap().is_some());

    let mut side = genesis;
    let tip = chain.tip().id();

    for _ in 0..DEV_MAX_REORG_DEPTH + 4 {
        side = mine_on(&chain, &side, Vec::new(), Address::Ed25519([2; 32]));
        assert!(matches!(chain.import(&side, LATER), Ok(Imported::Side)));
    }

    assert_eq!(chain.tip().id(), tip);
}

#[test]
fn a_heavier_branch_far_longer_than_the_reorg_depth_is_followed_when_it_splits_off_near_the_tip() {
    // A node that mined a few heavy blocks of its own, as a fast miner does before it hears its
    // peers, and a peer's chain from genesis that outweighs them only once it is far longer than
    // max_reorg_depth. The reorg goes 8 blocks deep, well within it, so the node follows.
    let mut chain = Chain::open(Arc::new(MemoryDb::default()), &pow_genesis()).unwrap();
    let genesis = chain.block_at(0).unwrap().unwrap();
    let mut own = genesis.clone();

    for _ in 0..8 {
        own = mine_after(&chain, &own, 250, Vec::new(), Address::Ed25519([1; 32]));
        extended(chain.import(&own, LATER));
    }

    let work = chain.tip_work();
    let mut peers = genesis;
    let mut length = 0;

    while chain.tip().id() == own.id() {
        peers = mine_after(&chain, &peers, 4_000, Vec::new(), Address::Ed25519([2; 32]));
        chain.import(&peers, LATER).unwrap();
        length += 1;
        assert!(length < 1_000, "never followed the heavier branch");
    }

    assert!(length > DEV_MAX_REORG_DEPTH, "the branch outweighed the node's only at {length} blocks, {work} work");
    assert_eq!(chain.tip().id(), peers.id());
    assert_eq!(chain.tip().height, length);
}

#[test]
fn rewards_are_fixed_or_halve() {
    let fixed = Reward {
        initial: u256::new(40),
        halving_interval: None,
    };
    let halving = Reward {
        initial: u256::new(40),
        halving_interval: Some(10),
    };

    assert_eq!(fixed.at(1_000_000), u256::new(40));
    assert_eq!([halving.at(9), halving.at(10), halving.at(25), halving.at(10 * 300)], [
        u256::new(40),
        u256::new(20),
        u256::new(10),
        u256::ZERO
    ]);
    assert_eq!(Reward::NONE.at(0), u256::ZERO);
}

/// Seals a block on a chain of difficulty 1, where the first nonce that meets it is near.
fn sealed(chain: &Chain, mut block: Block) -> Block {
    let nonce = (0..).find(|nonce| meets(work_hash(chain.hash(), &block.id(), *nonce), block.header.difficulty)).unwrap();
    block.seal = Seal::Work { nonce };

    block
}

#[test]
fn a_pool_block_sends_its_miners_share_to_the_escrow_and_each_claims_what_it_earned() {
    use crate::registry::pools::{Pools, claim_leaf, pool_escrow};
    use crate::structs::block::{PAYOUT_PARTS, PoolCommitment, parts_of};

    // Fees as the dev chains charge them, half of the base burned and half the producer's.
    let genesis = Genesis { fees: Some(crate::chain::fees::FeeConfig::dev()), ..pow_genesis() };
    let db = Arc::new(MemoryDb::default());
    let mut chain = Chain::open(db.clone(), &genesis).unwrap();
    let hash = chain.hash();
    // Real keys' addresses, as a claim checks the miner's is one.
    let (pool, alice, bob) = (Address::Ed25519([7; 32]), Keypair::from_seed([8; 32]).public().address(), Keypair::from_seed([9; 32]).public().address());
    let escrow = pool_escrow(&pool, hash);
    let fee = |chain: &Chain| FeeOffer { gas_limit: 100_000, max_price: chain.next_base_price() * u256::new(4), tip: u256::new(3) };
    let mut now = chain.tip().timestamp;
    let mut next = |chain: &mut Chain, txs: Vec<SignedTx>, pool_commitment: Option<PoolCommitment>| {
        now += 1_000;
        let block = sealed(chain, chain.build_declaring(txs, Vec::new(), pool, now, None, None, pool_commitment).unwrap());
        let receipts = extended(chain.import(&block, LATER));

        (block, receipts)
    };

    // The pool's first block: 90% to the escrow, nothing credited yet.
    let tx = SignedTx::sign(Tx::Transfer { from: authority().public().address(), to: Address::Ed25519([1; 32]), amount: Amount::U128(5) }, 0, fee(&chain), &authority());
    let first = PoolCommitment { parts: 900_000_000, root: Hash::zero(hash), count: 0 };
    let (block, _) = next(&mut chain, vec![tx], Some(first));
    assert_eq!(block.header.pool_root, Block::pool_root(hash, Some(&first)));

    let shared = stored_split(&*db, &block.id()).unwrap().expect("a pool's block keeps what it shared out");
    let escrowed = parts_of(shared, 900_000_000);
    assert!(shared > u256::new(100), "the reward and the producer's fees: {shared}");
    assert_eq!(balance(&chain, &escrow), escrowed);
    assert_eq!(balance(&chain, &pool) + escrowed, shared, "the producer keeps the rest: the pool's fee, and rounding");

    // The next commits to what its miners earned: alice two thirds of it, bob the rest.
    let earned = [(alice, escrowed * u256::new(2) / u256::new(3)), (bob, escrowed / u256::new(3))];
    let leaves: Vec<Hash> = earned.iter().map(|(to, earned)| claim_leaf(hash, to, *earned)).collect();
    let root = crate::crypto::merkle::root(hash, &leaves);
    let (committed, _) = next(&mut chain, Vec::new(), Some(PoolCommitment { parts: 0, root, count: 2 }));

    // Anyone claims for alice, who gets what she earned; again, and nothing is left.
    let claim = |nonce: u64, chain: &Chain| {
        let tx = Tx::Claim { height: committed.header.height, to: alice, earned: earned[0].1, index: 0, path: crate::crypto::merkle::audit_path(hash, &leaves, 0) };

        SignedTx::sign(tx, nonce, fee(chain), &authority())
    };
    let tx = claim(1, &chain);
    let (_, receipts) = next(&mut chain, vec![tx], None);
    assert!(receipts[0].result.is_ok(), "{:?}", receipts[0].result);
    assert_eq!(balance(&chain, &alice), earned[0].1);
    assert_eq!(balance(&chain, &escrow), escrowed - earned[0].1);
    assert_eq!(Pools::new(&chain.state(), hash).claimed(&pool, &alice), Ok(earned[0].1));

    let tx = claim(2, &chain);
    let (_, receipts) = next(&mut chain, vec![tx], None);
    assert_eq!(receipts[0].result, Err(Failure::Claim("nothing left to claim: all it earned is claimed".to_string())));
    assert_eq!(balance(&chain, &alice), earned[0].1);

    // Bob's own claim names a leaf that is not his place.
    let wrong = Tx::Claim { height: committed.header.height, to: bob, earned: earned[1].1, index: 0, path: crate::crypto::merkle::audit_path(hash, &leaves, 0) };
    let tx = SignedTx::sign(wrong, 3, fee(&chain), &authority());
    let (_, receipts) = next(&mut chain, vec![tx], None);
    assert!(matches!(&receipts[0].result, Err(Failure::Claim(why)) if why.contains("path")));
    let _ = PAYOUT_PARTS;
}

#[test]
fn a_pool_commitment_the_header_does_not_hold_or_past_the_whole_reward_is_refused() {
    use crate::structs::block::{PAYOUT_PARTS, PoolCommitment};

    let db = Arc::new(MemoryDb::default());
    let mut chain = Chain::open(db, &pow_genesis()).unwrap();
    let pool = Address::Ed25519([7; 32]);
    let now = chain.tip().timestamp + 1_000;
    let build = |chain: &Chain, commitment: PoolCommitment| sealed(chain, chain.build_declaring(Vec::new(), Vec::new(), pool, now, None, None, Some(commitment)).unwrap());
    let commitment = PoolCommitment { parts: 500_000_000, root: Hash::zero(chain.hash()), count: 0 };

    // Changed after sealing: the header committed to another.
    let mut changed = build(&chain, commitment);
    changed.pool = Some(PoolCommitment { parts: PAYOUT_PARTS, ..commitment });
    assert!(matches!(chain.import(&changed, LATER), Err(ImportError::Invalid(BlockError::PoolRoot))));

    let past = build(&chain, PoolCommitment { parts: PAYOUT_PARTS + 1, ..commitment });
    assert!(matches!(chain.import(&past, LATER), Err(ImportError::Invalid(BlockError::Pool(_)))));

    // The whole reward to the escrow leaves the producer nothing, and is fine.
    let whole = build(&chain, PoolCommitment { parts: PAYOUT_PARTS, ..commitment });
    extended(chain.import(&whole, LATER));
    assert_eq!(balance(&chain, &crate::registry::pools::pool_escrow(&pool, chain.hash())), u256::new(100));
    assert_eq!(balance(&chain, &pool), u256::ZERO);
}

#[test]
fn a_pool_block_settles_once_finalized_or_past_the_reorg_depth() {
    use crate::consensus::PoolSettled;
    use crate::structs::block::PoolCommitment;

    let db = Arc::new(MemoryDb::default());
    let mut chain = Chain::open(db, &Genesis { max_reorg_depth: 3, ..pow_genesis() }).unwrap();
    let pool = Address::Ed25519([7; 32]);
    let commitment = PoolCommitment { parts: 500_000_000, root: Hash::zero(chain.hash()), count: 0 };
    let now = chain.tip().timestamp + 1_000;
    let block = sealed(&chain, chain.build_declaring(Vec::new(), Vec::new(), pool, now, None, None, Some(commitment)).unwrap());
    extended(chain.import(&block, LATER));
    assert_eq!(chain.pool_settled(&block.id(), 1), Ok(PoolSettled::Pending));

    for step in 1..=4 {
        let next = sealed(&chain, chain.build(Vec::new(), Vec::new(), pool, now + 1_000 * step).unwrap());
        extended(chain.import(&next, LATER));
    }

    assert_eq!(chain.pool_settled(&block.id(), 1), Ok(PoolSettled::Final(u256::new(100))));
    assert_eq!(chain.pool_settled(&Hash::from_bytes(chain.hash(), &[1; 32]).unwrap(), 1), Ok(PoolSettled::Dropped), "a block never on the main chain");
}

#[test]
fn receipts_keep_what_contracts_emitted() {
    const EMITTER: &str = r#"
local M = {}

function M.ping(times)
    for i = 1, times do
        events.emit("Ping", i, ctx.sender)
    end
    return "pong"
end

function M.ping_then_fail()
    events.emit("Lost")
    error("no")
end

return M
"#;
    let db = Arc::new(MemoryDb::default());
    let mut chain = open(&db);
    let deployer = authority();
    let deploy = SignedTx::sign(Tx::Deploy { code: Code::Lua(EMITTER.to_string()) }, 0, FeeOffer::NONE, &deployer);
    advance(&mut chain, vec![deploy]);
    let emitter = contract_address(&deployer.public().address(), 0, HashType::default_chain());
    let call = |function: &str, nonce: u64, data: Vec<TxCallArgumentType>| {
        let tx = Tx::ContractCall {
            to: emitter,
            func: Hash::Xxh3_64(selector(function)),
            amount: Amount::U128(0),
            data,
        };

        SignedTx::sign(tx, nonce, FeeOffer::NONE, &deployer)
    };
    let ping = call("ping", 1, vec![TxCallArgumentType::Number(2.0)]);
    let failing = call("ping_then_fail", 2, Vec::new());
    advance(&mut chain, vec![ping.clone(), failing.clone()]);
    let receipt = chain.receipt(&ping.id(HashType::default_chain())).unwrap().unwrap();
    let sender = TxCallArgumentType::Address(deployer.public().address());

    assert_eq!(receipt.block, chain.tip().id());
    assert_eq!(receipt.index, 0);
    assert_eq!(receipt.status, TxStatus::Returned(vec![TxCallArgumentType::String("pong".to_string())]));
    assert_eq!(
        receipt.events.iter().map(|event| (event.contract, event.name.as_str(), event.data.clone())).collect::<Vec<_>>(),
        [
            (emitter, "Ping", vec![TxCallArgumentType::I64(1), sender.clone()]),
            (emitter, "Ping", vec![TxCallArgumentType::I64(2), sender]),
        ]
    );

    let failed = chain.receipt(&failing.id(HashType::default_chain())).unwrap().unwrap();

    assert!(matches!(&failed.status, TxStatus::Failed(error) if error.contains("no")), "{failed:?}");
    assert!(failed.events.is_empty());
}

#[test]
fn upgrade_transactions_ask_the_contract_and_fail_whole() {
    const V1: &str = r#"
local M = {}
function M.init() storage.set("owner", ctx.sender:hex()); events.emit("Created") end
function M.check_perms(permission, who) return who:hex() == storage.get("owner") end
function M.version() return 1 end
return M
"#;
    const BROKEN: &str = r#"
local M = {}
function M.migrate() storage.set("half", "done"); error("migration failed") end
function M.version() return 2 end
return M
"#;
    const V2: &str = r#"
local M = {}
function M.version() return 2 end
return M
"#;
    let db = Arc::new(MemoryDb::default());
    let mut chain = open(&db);
    let owner = authority();
    let deploy = SignedTx::sign(Tx::Deploy { code: Code::Lua(V1.to_string()) }, 0, FeeOffer::NONE, &owner);
    let receipts = advance(&mut chain, vec![deploy]);
    let contract = contract_address(&owner.public().address(), 0, HashType::default_chain());

    assert_eq!(receipts[0].result, Ok(Effect::Deployed(contract)));
    assert!(receipts[0].fuel_used > 0);
    assert_eq!(receipts[0].events[0].name, "Created");

    let upgrade = |code: &str, nonce: u64, signer: &Keypair| {
        let tx = Tx::Upgrade {
            contract,
            code: Code::Lua(code.to_string()),
        };

        SignedTx::sign(tx, nonce, FeeOffer::NONE, signer)
    };
    let version = |chain: &mut Chain, nonce: u64| {
        let tx = Tx::ContractCall {
            to: contract,
            func: Hash::Xxh3_64(selector("version")),
            amount: Amount::U128(0),
            data: Vec::new(),
        };

        advance(chain, vec![SignedTx::sign(tx, nonce, FeeOffer::NONE, &authority())])[0].result.clone()
    };
    let receipts = advance(&mut chain, vec![upgrade(BROKEN, 1, &owner), upgrade(V2, 0, &stranger())]);

    assert!(matches!(&receipts[0].result, Err(failure) if failure.to_string().contains("migration failed")));
    assert!(matches!(&receipts[1].result, Err(failure) if failure.to_string().contains("does not permit")));
    assert_eq!(version(&mut chain, 2), Ok(Effect::Returned(vec![TxCallArgumentType::I64(1)])));

    let receipts = advance(&mut chain, vec![upgrade(V2, 3, &owner)]);

    assert_eq!(receipts[0].result, Ok(Effect::Upgraded(contract)));
    assert_eq!(version(&mut chain, 4), Ok(Effect::Returned(vec![TxCallArgumentType::I64(2)])));
}

/// A proof-of-work chain that hashes everything, its work included, with one algorithm.
fn hashed_with(algorithm: HashType) -> Genesis {
    let mut genesis = pow_genesis();
    genesis.hash = algorithm;

    if let ConsensusConfig::ProofOfWork(pow) = &mut genesis.consensus {
        pow.hash = algorithm;
    }

    genesis
}

#[test]
fn every_secure_hash_can_run_a_chain() {
    let secure: Vec<HashType> = HashType::ALL.iter().copied().filter(|algorithm| algorithm.secure()).collect();
    let mut genesis_ids = HashSet::new();
    let mut contracts = HashSet::new();

    for &algorithm in &secure {
        let mut chain = Chain::open(Arc::new(MemoryDb::default()), &hashed_with(algorithm)).unwrap();
        let genesis = chain.block_at(0).unwrap().unwrap();
        let bob = stranger().public().address();
        let tx = send(&bob, 10, 0);
        let block = mine_on(&chain, &genesis, vec![tx.clone()], Address::Ed25519([1; 32]));

        assert_eq!(chain.genesis().algorithm(), algorithm, "{}", algorithm.name());
        assert_eq!(block.id().algorithm(), algorithm);
        assert_eq!(block.header.tx_root.algorithm(), algorithm);
        extended(chain.import(&block, LATER));
        assert_eq!(chain.receipt(&tx.id(algorithm)).unwrap().unwrap().block, block.id());
        assert_eq!(balance(&chain, &bob), u256::new(10));

        genesis_ids.insert(chain.genesis());
        contracts.insert(contract_address(&authority().public().address(), 0, algorithm));
    }

    // Each algorithm makes its own chain, with its own ids and addresses.
    assert_eq!(genesis_ids.len(), secure.len());
    assert_eq!(contracts.len(), secure.len());
}

#[test]
fn a_block_hashed_with_another_algorithm_never_connects() {
    let secure: Vec<HashType> = HashType::ALL.iter().copied().filter(|algorithm| algorithm.secure()).collect();

    let [first, second, ..] = secure[..] else {
        return;
    };

    let mut chain = Chain::open(Arc::new(MemoryDb::default()), &hashed_with(first)).unwrap();
    let other = Chain::open(Arc::new(MemoryDb::default()), &hashed_with(second)).unwrap();
    let foreign = mine_on(&other, &other.block_at(0).unwrap().unwrap(), Vec::new(), Address::Ed25519([1; 32]));

    assert!(matches!(chain.import(&foreign, LATER), Err(ImportError::UnknownParent(_))));
}

#[cfg(feature = "xxh3")]
#[test]
fn xxh3_is_never_a_chain_or_work_hash() {
    let mut weak_chain = pow_genesis();
    weak_chain.hash = HashType::Xxh3_64;
    let mut weak_work = pow_genesis();

    if let ConsensusConfig::ProofOfWork(pow) = &mut weak_work.consensus {
        pow.hash = HashType::Xxh3_64;
    }

    for genesis in [weak_chain, weak_work] {
        let err = Chain::open(Arc::new(MemoryDb::default()), &genesis).err().unwrap();

        assert!(matches!(&err, ChainError::BadGenesis(why) if why.contains("xxh3")), "{err}");
    }
}

#[test]
fn a_deploy_that_never_finishes_loading_pays_for_all_its_fuel() {
    let db = Arc::new(MemoryDb::default());
    let mut chain = open(&db);
    let deployer = authority();
    let deploy = SignedTx::sign(
        Tx::Deploy {
            code: Code::Lua("while true do end".to_string()),
        },
        nonce(&chain, &deployer),
        FeeOffer::NONE,
        &deployer,
    );
    let receipts = advance(&mut chain, vec![deploy]);

    assert!(matches!(&receipts[0].result, Err(failure) if failure.to_string().contains("out of fuel")));
    assert!(receipts[0].fuel_used > DEV_TX_LIMITS.fuel - 1_000, "it burned {}", receipts[0].fuel_used);
}

/// Emits an event naming an address, and one that names none.
const EMITTER: &str = r#"
local M = {}

function M.ping(to, n)
    events.emit("Ping", to, n)
    events.emit("Pong", n)
end

return M
"#;

#[test]
fn indexed_events_are_found_by_contract_name_and_address_and_leave_with_their_block() {
    let db = Arc::new(MemoryDb::default());
    let mut chain = Chain::open(db.clone(), &pow_genesis()).unwrap();
    let genesis_block = chain.block_at(0).unwrap().unwrap();
    let (alice, bob) = (stranger().public().address(), stranger().public().address());
    let miner = Address::Ed25519([9; 32]);
    let emitter = contract_address(&authority().public().address(), 0, HashType::default_chain());
    let deploy = SignedTx::sign(Tx::Deploy { code: Code::Lua(EMITTER.to_string()) }, 0, FeeOffer::NONE, &authority());
    let ping = |to: Address, n: i64, nonce: u64| {
        let tx = Tx::ContractCall {
            to: emitter,
            func: Hash::Xxh3_64(selector("ping")),
            amount: Amount::U128(0),
            data: vec![TxCallArgumentType::Address(to), TxCallArgumentType::I64(n)],
        };

        SignedTx::sign(tx, nonce, FeeOffer::NONE, &authority())
    };

    // Events before the index is turned on are not in it.
    let a1 = mine_on(&chain, &genesis_block, vec![deploy, ping(alice, 1, 1)], miner);
    extended(chain.import(&a1, LATER));

    let index = IndexWindow { from: 2, keep: KeepFrom::Height(2) };

    assert_eq!(stored_index(&*db, IndexKind::Events).unwrap(), None);
    assert_eq!(chain.keep_index(IndexKind::Events, Some(KeepFrom::Height(2))).unwrap(), index);
    assert_eq!(chain.keep_index(IndexKind::Events, None).unwrap(), index, "turning it on again changes nothing");
    assert!(!chain.catch_up_index(IndexKind::Events).unwrap(), "it holds what it keeps");

    let a2 = mine_on(&chain, &a1, vec![ping(bob, 2, 2), ping(alice, 3, 3)], miner);
    extended(chain.import(&a2, LATER));
    let listed = |filter: EventFilter| {
        stored_events(&*db, &filter, None, 100)
            .unwrap()
            .0
            .iter()
            .map(|found| (found.event.name.clone(), found.index, found.position))
            .collect::<Vec<_>>()
    };

    // Newest first: the later transaction, and within it the later event, first.
    assert_eq!(
        listed(EventFilter::Contract(emitter)),
        [("Pong".to_string(), 1, 1), ("Ping".to_string(), 1, 0), ("Pong".to_string(), 0, 1), ("Ping".to_string(), 0, 0)]
    );
    assert_eq!(listed(EventFilter::Named(emitter, "Ping".to_string())), [("Ping".to_string(), 1, 0), ("Ping".to_string(), 0, 0)]);
    assert!(listed(EventFilter::Named(emitter, "Pin".to_string())).is_empty(), "a name never matches a longer one");

    let mentions = stored_events(&*db, &EventFilter::Mentions(alice), None, 100).unwrap().0;

    assert_eq!(mentions.len(), 1);
    assert_eq!((mentions[0].height, mentions[0].block, mentions[0].event.data[1].clone()), (2, a2.id(), TxCallArgumentType::I64(3)));
    assert_eq!(mentions[0].tx, ping(alice, 3, 3).id(HashType::default_chain()));

    // Pages pick up where the last stopped.
    let (first, next) = stored_events(&*db, &EventFilter::Contract(emitter), None, 3).unwrap();
    let (rest, after) = stored_events(&*db, &EventFilter::Contract(emitter), next.as_deref(), 3).unwrap();

    assert_eq!((first.len(), rest.len(), after), (3, 1, None));
    assert_eq!((rest[0].index, rest[0].position), (0, 0));
    assert_eq!(Chain::open(db.clone(), &pow_genesis()).unwrap().index_window(IndexKind::Events), Some(index), "it stays on");

    // A heavier branch takes the indexed block, and its events, off.
    let b2 = mine_on(&chain, &a1, Vec::new(), miner);
    chain.import(&b2, LATER).unwrap();
    let b3 = mine_on(&chain, &b2, Vec::new(), miner);
    assert!(matches!(chain.import(&b3, LATER), Ok(Imported::Reorganized { .. })));

    assert!(listed(EventFilter::Contract(emitter)).is_empty());
    assert!(stored_events(&*db, &EventFilter::Mentions(bob), None, 100).unwrap().0.is_empty());
}

#[test]
fn the_event_index_catches_up_on_older_blocks_and_keeps_its_window() {
    // The same blocks go to a chain that indexes them as they come, and to one that catches up.
    let live_db = Arc::new(MemoryDb::default());
    let mut live = Chain::open(live_db.clone(), &pow_genesis()).unwrap();
    let late_db = Arc::new(MemoryDb::default());
    let mut late = Chain::open(late_db.clone(), &pow_genesis()).unwrap();
    let ahead_db = Arc::new(MemoryDb::default());
    let mut ahead = Chain::open(ahead_db.clone(), &pow_genesis()).unwrap();
    let genesis_block = late.block_at(0).unwrap().unwrap();
    let alice = stranger().public().address();
    let miner = Address::Ed25519([9; 32]);
    let emitter = contract_address(&authority().public().address(), 0, HashType::default_chain());
    let ping = |n: i64, nonce: u64| {
        let tx = Tx::ContractCall {
            to: emitter,
            func: Hash::Xxh3_64(selector("ping")),
            amount: Amount::U128(0),
            data: vec![TxCallArgumentType::Address(alice), TxCallArgumentType::I64(n)],
        };

        SignedTx::sign(tx, nonce, FeeOffer::NONE, &authority())
    };
    let entries = |db: &MemoryDb| {
        let mut all = Vec::new();
        db.scan(Column::Events, &[], &[], &mut |key, value| {
            all.push((key.to_vec(), value.to_vec()));

            true
        })
        .unwrap();

        all
    };
    let heights = |chain: &Chain| {
        stored_events(&*chain.db, &EventFilter::Contract(emitter), None, 100)
            .unwrap()
            .0
            .iter()
            .map(|found| found.height)
            .collect::<Vec<_>>()
    };
    let caught_up = |chain: &mut Chain| while chain.catch_up_index(IndexKind::Events).unwrap() {};

    live.keep_index(IndexKind::Events, None).unwrap();
    assert_eq!(ahead.keep_index(IndexKind::Events, Some(KeepFrom::Height(3))).unwrap().from, 3, "it waits for height 3");

    let deploy = SignedTx::sign(Tx::Deploy { code: Code::Lua(EMITTER.to_string()) }, 0, FeeOffer::NONE, &authority());
    let mut main = vec![mine_on(&late, &genesis_block, vec![deploy, ping(1, 1)], miner)];

    for nonce in 2..=6 {
        for chain in [&mut live, &mut late, &mut ahead] {
            extended(chain.import(main.last().unwrap(), LATER));
        }

        if nonce < 6 {
            main.push(mine_on(&late, main.last().unwrap(), vec![ping(nonce as i64, nonce)], miner));
        }
    }

    assert_eq!(heights(&ahead), [5, 5, 4, 4, 3, 3]);
    assert!(!ahead.catch_up_index(IndexKind::Events).unwrap());

    // Turned on late, it starts empty at the next block, then reads the older receipts back.
    assert_eq!(late.keep_index(IndexKind::Events, None).unwrap(), IndexWindow { from: 6, keep: KeepFrom::Height(0) });
    assert!(entries(&late_db).is_empty());
    caught_up(&mut live);
    caught_up(&mut late);

    assert_eq!(late.index_window(IndexKind::Events).unwrap().from, 0);
    assert_eq!(heights(&late), [5, 5, 4, 4, 3, 3, 2, 2, 1, 1]);
    assert_eq!(entries(&late_db), entries(&live_db), "catching up builds what indexing as blocks came does");

    // A heavier branch takes caught-up blocks off, and their events with them. Its first block is
    // empty, so nothing it indexes lands where theirs were.
    let mut branch = vec![mine_on(&late, &main[1], Vec::new(), miner)];

    for nonce in 3..=6 {
        for chain in [&mut live, &mut late] {
            chain.import(branch.last().unwrap(), LATER).unwrap();
        }

        if nonce < 6 {
            branch.push(mine_on(&late, branch.last().unwrap(), vec![ping(10 + nonce as i64, nonce)], miner));
        }
    }

    assert_eq!(late.tip().id(), branch[3].header.id());
    assert_eq!(heights(&late), [6, 6, 5, 5, 4, 4, 2, 2, 1, 1]);
    assert_eq!(entries(&late_db), entries(&live_db));

    // A window of the latest two blocks drops the rest, and moves with the tip.
    late.keep_index(IndexKind::Events, Some(KeepFrom::Latest(2))).unwrap();
    caught_up(&mut late);

    assert_eq!(heights(&late), [6, 6, 5, 5]);

    let next = mine_on(&late, &branch[3], vec![ping(17, 6)], miner);

    for chain in [&mut live, &mut late] {
        extended(chain.import(&next, LATER));
    }

    assert!(!late.catch_up_index(IndexKind::Events).unwrap(), "one step drops the block that left the window");
    assert_eq!(heights(&late), [7, 7, 6, 6]);
    assert_eq!(late.index_window(IndexKind::Events).unwrap().from, 6);

    // Asking for every block again reads the dropped ones back, and it stays as asked.
    late.keep_index(IndexKind::Events, Some(KeepFrom::Height(0))).unwrap();
    caught_up(&mut live);
    caught_up(&mut late);

    assert_eq!(entries(&late_db), entries(&live_db));
    assert_eq!(
        Chain::open(late_db.clone(), &pow_genesis()).unwrap().index_window(IndexKind::Events),
        Some(IndexWindow { from: 0, keep: KeepFrom::Height(0) })
    );

    // Turned off, it is gone at once and stays gone; turned on again, it is built afresh.
    assert!(late.drop_index(IndexKind::Events).unwrap());
    assert!(!late.drop_index(IndexKind::Events).unwrap(), "nothing left to drop");
    assert!(entries(&late_db).is_empty());
    assert_eq!(Chain::open(late_db.clone(), &pow_genesis()).unwrap().index_window(IndexKind::Events), None);

    late.keep_index(IndexKind::Events, None).unwrap();
    caught_up(&mut late);

    assert_eq!(entries(&late_db), entries(&live_db));
}

#[test]
fn an_index_takes_genesis_a_height_or_a_window() {
    assert_eq!("genesis".parse(), Ok(KeepFrom::Height(0)));
    assert_eq!("GENESIS".parse(), Ok(KeepFrom::Height(0)));
    assert_eq!("5000".parse(), Ok(KeepFrom::Height(5000)));
    assert_eq!("LATEST-100000".parse(), Ok(KeepFrom::Latest(100_000)));
    assert_eq!("Off".parse(), Ok(Indexing::Off));
    assert_eq!("latest-5".parse(), Ok(Indexing::Keep(KeepFrom::Latest(5))));

    for bad in ["latest", "latest-0", "latest-x", "-5", "soon", ""] {
        assert!(bad.parse::<KeepFrom>().is_err(), "{bad}");
    }

    assert_eq!(KeepFrom::Latest(3).lowest(10), 8, "heights 8, 9 and 10");
    assert_eq!(KeepFrom::Latest(100).lowest(10), 0);
    assert_eq!(KeepFrom::Height(7).lowest(10), 7);
}

#[test]
fn a_transaction_touches_its_sender_its_parties_and_the_addresses_it_names() {
    let sender = authority();
    let me = sender.public().address();
    let (alice, bob) = (stranger().public().address(), stranger().public().address());
    let (contract, deployed) = (Address::Contract([7; 32]), Address::Contract([8; 32]));
    let signed = |tx: Tx| SignedTx::sign(tx, 0, FeeOffer::NONE, &sender);
    let set = |addresses: &[Address]| addresses.iter().copied().collect::<std::collections::BTreeSet<_>>();
    let code = || Code::Lua(EMITTER.to_string());
    let call = Tx::ContractCall {
        to: contract,
        func: Hash::Xxh3_64(selector("transfer")),
        amount: Amount::U128(0),
        data: vec![TxCallArgumentType::Address(alice), TxCallArgumentType::U256(u256::new(5)), TxCallArgumentType::Address(bob)],
    };

    assert_eq!(touched(&signed(Tx::Transfer { from: me, to: alice, amount: Amount::U128(1) }), &TxStatus::Done, &[]), set(&[me, alice]));
    assert_eq!(touched(&signed(Tx::Mint { to: bob, amount: Amount::U128(1) }), &TxStatus::Done, &[]), set(&[me, bob]));
    assert_eq!(touched(&signed(Tx::Burn { from: me, amount: Amount::U128(1) }), &TxStatus::Done, &[]), set(&[me]));
    assert_eq!(touched(&signed(call), &TxStatus::Returned(Vec::new()), &[]), set(&[me, contract, alice, bob]), "every address among the arguments");
    assert_eq!(touched(&signed(Tx::Deploy { code: code() }), &TxStatus::Deployed(deployed), &[]), set(&[me, deployed]));
    assert_eq!(touched(&signed(Tx::Deploy { code: code() }), &TxStatus::Failed("no".to_string()), &[]), set(&[me]), "a failed deploy landed nowhere");
    assert_eq!(touched(&signed(Tx::Upgrade { contract, code: code() }), &TxStatus::Upgraded(contract), &[]), set(&[me, contract]));
    assert_eq!(touched(&signed(Tx::WithdrawApproval { version: 1 }), &TxStatus::Done, &[]), set(&[me]));
    // Whom a contract paid, and the contract, though the transaction named neither.
    let paid = crate::structs::receipt::Payment { from: contract, to: bob, amount: u256::new(5) };
    assert_eq!(touched(&signed(Tx::WithdrawApproval { version: 1 }), &TxStatus::Done, &[paid]), set(&[me, contract, bob]));
}

#[test]
fn the_address_index_lists_what_touched_each_address_newest_first_and_follows_reorgs() {
    // The same blocks go to a chain that indexes them as they come, and to one that catches up.
    let live_db = Arc::new(MemoryDb::default());
    let mut live = Chain::open(live_db.clone(), &pow_genesis()).unwrap();
    let late_db = Arc::new(MemoryDb::default());
    let mut late = Chain::open(late_db.clone(), &pow_genesis()).unwrap();
    let genesis_block = late.block_at(0).unwrap().unwrap();
    let hash = HashType::default_chain();
    let me = authority().public().address();
    let (alice, bob) = (stranger().public().address(), stranger().public().address());
    let miner = Address::Ed25519([9; 32]);
    let emitter = contract_address(&me, 0, hash);
    let pay = |to: Address, nonce: u64| SignedTx::sign(Tx::Transfer { from: me, to, amount: Amount::U128(5) }, nonce, FeeOffer::NONE, &authority());
    let ping = |to: Address, nonce: u64| {
        let tx = Tx::ContractCall {
            to: emitter,
            func: Hash::Xxh3_64(selector("ping")),
            amount: Amount::U128(0),
            data: vec![TxCallArgumentType::Address(to), TxCallArgumentType::I64(1)],
        };

        SignedTx::sign(tx, nonce, FeeOffer::NONE, &authority())
    };
    let deploy = SignedTx::sign(Tx::Deploy { code: Code::Lua(EMITTER.to_string()) }, 0, FeeOffer::NONE, &authority());
    let listed = |chain: &Chain, address: &Address| {
        stored_address_txs(&*chain.db, address, None, 100)
            .unwrap()
            .0
            .iter()
            .map(|found| (found.height, found.index))
            .collect::<Vec<_>>()
    };
    let entries = |db: &MemoryDb| {
        let mut all = Vec::new();
        db.scan(Column::Addresses, &[], &[], &mut |key, value| {
            all.push((key.to_vec(), value.to_vec()));

            true
        })
        .unwrap();

        all
    };
    let caught_up = |chain: &mut Chain| while chain.catch_up_index(IndexKind::Addresses).unwrap() {};

    live.keep_index(IndexKind::Addresses, None).unwrap();

    let b1 = mine_on(&late, &genesis_block, vec![deploy.clone(), pay(alice, 1)], miner);
    let mut main = vec![b1];

    for txs in [vec![ping(bob, 2), pay(bob, 3)], vec![pay(alice, 4)]] {
        for chain in [&mut live, &mut late] {
            extended(chain.import(main.last().unwrap(), LATER));
        }

        main.push(mine_on(&late, main.last().unwrap(), txs, miner));
    }

    for chain in [&mut live, &mut late] {
        extended(chain.import(main.last().unwrap(), LATER));
    }

    assert_eq!(listed(&live, &alice), [(3, 0), (1, 1)], "a plain transfer to it, which no event tells of");
    assert_eq!(listed(&live, &bob), [(2, 1), (2, 0)], "named among a call's arguments, too");
    assert_eq!(listed(&live, &emitter), [(2, 0), (1, 0)], "called, and deployed");
    assert_eq!(listed(&live, &me), [(3, 0), (2, 1), (2, 0), (1, 1), (1, 0)]);

    let found = &stored_address_txs(&*live_db, &alice, None, 1).unwrap().0[0];

    assert_eq!((found.tx, found.block), (pay(alice, 4).id(hash), main[2].id()));

    // Pages pick up where the last stopped.
    let (first, next) = stored_address_txs(&*live_db, &me, None, 3).unwrap();
    let (rest, after) = stored_address_txs(&*live_db, &me, next.as_deref(), 3).unwrap();

    assert_eq!((first.len(), rest.len(), after), (3, 2, None));
    assert_eq!((rest[1].height, rest[1].index), (1, 0));

    // Turned on late, it starts empty at the next block, then reads the older blocks back.
    assert_eq!(late.keep_index(IndexKind::Addresses, None).unwrap(), IndexWindow { from: 4, keep: KeepFrom::Height(0) });
    assert!(entries(&late_db).is_empty());
    assert_eq!(late.index_window(IndexKind::Events), None, "the other index stays as it was");
    caught_up(&mut live);
    caught_up(&mut late);

    assert_eq!(entries(&late_db), entries(&live_db), "catching up builds what indexing as blocks came does");

    // A heavier branch takes blocks 2 and 3 off, and what they touched with them.
    let mut branch = vec![mine_on(&late, &main[0], Vec::new(), miner)];

    for _ in 0..3 {
        for chain in [&mut live, &mut late] {
            chain.import(branch.last().unwrap(), LATER).unwrap();
        }

        branch.push(mine_on(&late, branch.last().unwrap(), Vec::new(), miner));
    }

    assert_eq!(late.tip().id(), branch[2].header.id());
    assert_eq!(listed(&late, &alice), [(1, 1)]);
    assert!(listed(&late, &bob).is_empty());
    assert_eq!(entries(&late_db), entries(&live_db));

    // A window of the latest block keeps nothing older, and dropping the index takes it all.
    late.keep_index(IndexKind::Addresses, Some(KeepFrom::Latest(1))).unwrap();
    caught_up(&mut late);

    assert!(listed(&late, &me).is_empty(), "the tip is empty");
    assert!(late.drop_index(IndexKind::Addresses).unwrap());
    assert!(entries(&late_db).is_empty());
    assert_eq!(Chain::open(late_db.clone(), &pow_genesis()).unwrap().index_window(IndexKind::Addresses), None);
    assert_eq!(Chain::open(live_db.clone(), &pow_genesis()).unwrap().index_window(IndexKind::Addresses), Some(IndexWindow { from: 0, keep: KeepFrom::Height(0) }));
}

/// What the tip's header commits to, worked out from the database alone.
fn roots_hold(chain: &Chain) {
    let tip = chain.tip();
    let block = chain.block(&tip.id()).unwrap().unwrap();
    let committed: Vec<CommittedReceipt> = block
        .txs
        .iter()
        .map(|tx| {
            let id = tx.id(chain.hash());

            chain.receipt(&id).unwrap().unwrap().committed(id)
        })
        .collect();

    assert_eq!(crate::db::stored_root(&*chain.db, chain.hash()).unwrap(), tip.state_root, "at height {}", tip.height);
    assert_eq!(StateTree::new(&chain.state(), chain.hash()).root().unwrap(), tip.state_root);
    assert_eq!(receipts_root(chain.hash(), &committed), tip.receipts_root);
}

#[test]
fn headers_commit_to_the_state_and_what_their_transactions_did() {
    let db = Arc::new(MemoryDb::default());
    let mut chain = open(&db);
    let bob = stranger().public().address();

    roots_hold(&chain);
    assert_ne!(chain.tip().state_root, Hash::zero(chain.hash()), "genesis holds the allocations");
    assert_eq!(chain.tip().receipts_root, Hash::zero(chain.hash()));

    let deploy = SignedTx::sign(Tx::Deploy { code: Code::Lua(COUNTER.to_string()) }, 0, FeeOffer::NONE, &authority());
    let counter = contract_address(&authority().public().address(), 0, chain.hash());
    let increment = |nonce: u64| {
        let tx = Tx::ContractCall {
            to: counter,
            func: Hash::Xxh3_64(selector("increment")),
            amount: Amount::U128(0),
            data: vec![TxCallArgumentType::I64(5)],
        };

        SignedTx::sign(tx, nonce, FeeOffer::NONE, &authority())
    };
    advance(&mut chain, vec![deploy, send(&bob, 10, 1)]);
    roots_hold(&chain);
    advance(&mut chain, vec![increment(2), increment(3)]);
    roots_hold(&chain);
    advance(&mut chain, Vec::new());
    roots_hold(&chain);

    // A light client trusting the header can check a value with a proof alone.
    let root = chain.tip().state_root;
    let key = crate::db::address_key(&bob);
    let account = chain.state().get(Column::Accounts, &key).unwrap().unwrap();
    let proof = StateTree::new(&chain.state(), chain.hash()).prove(Column::Accounts, &key).unwrap();

    assert!(proof.verify(&root, Column::Accounts, &key, Some(&account)));
    assert!(!proof.verify(&root, Column::Accounts, &key, None));

    // A header that claims another root is refused, and nothing it ran is kept.
    let refused = |chain: &mut Chain, tamper: &dyn Fn(&mut BlockHeader)| {
        let (mut block, now) = next_block(chain, vec![send(&bob, 1, 4)]);
        tamper(&mut block.header);
        block.seal = producer().seal(&block.header).unwrap();

        chain.import(&block, now)
    };

    assert!(matches!(
        refused(&mut chain, &|header| header.state_root = Hash::zero(header.state_root.algorithm())),
        Err(ImportError::Invalid(BlockError::StateRoot { .. }))
    ));
    assert!(matches!(
        refused(&mut chain, &|header| header.receipts_root = Hash::zero(header.receipts_root.algorithm())),
        Err(ImportError::Invalid(BlockError::ReceiptsRoot))
    ));
    assert_eq!(chain.tip().state_root, root);
    roots_hold(&chain);
}

#[test]
fn reorgs_take_the_state_tree_along() {
    let db = Arc::new(MemoryDb::default());
    let mut chain = Chain::open(db, &pow_genesis()).unwrap();
    let genesis = chain.block_at(0).unwrap().unwrap();
    let bob = stranger().public().address();
    let a1 = mine_on(&chain, &genesis, vec![send(&bob, 10, 0)], Address::Ed25519([1; 32]));
    extended(chain.import(&a1, LATER));
    roots_hold(&chain);

    // A heavier branch, sending more to someone else, takes over, and then the first comes back.
    let carol = stranger().public().address();
    let b1 = mine_on(&chain, &genesis, vec![send(&bob, 30, 0), send(&carol, 1, 1)], Address::Ed25519([2; 32]));
    assert!(matches!(chain.import(&b1, LATER), Ok(Imported::Side)));
    let b2 = mine_on(&chain, &b1, Vec::new(), Address::Ed25519([2; 32]));
    assert!(matches!(chain.import(&b2, LATER), Ok(Imported::Reorganized { .. })));
    roots_hold(&chain);

    let a2 = mine_on(&chain, &a1, Vec::new(), Address::Ed25519([1; 32]));
    assert!(matches!(chain.import(&a2, LATER), Ok(Imported::Side)));
    let a3 = mine_on(&chain, &a2, Vec::new(), Address::Ed25519([1; 32]));
    assert!(matches!(chain.import(&a3, LATER), Ok(Imported::Reorganized { .. })));
    roots_hold(&chain);
    assert_eq!(balance(&chain, &bob), u256::new(10));
}

#[test]
fn a_light_client_follows_proof_of_work_through_a_reorg() {
    use crate::light::{Extended, LightClient, LightRequest};

    let mut chain = Chain::open(Arc::new(MemoryDb::default()), &pow_genesis()).unwrap();
    let genesis = chain.block_at(0).unwrap().unwrap();
    let headers_from = |chain: &Chain, from: u64| -> Vec<SealedHeader> {
        (from..).map_while(|height| stored_header_at(&*chain.db, height).unwrap()).collect()
    };
    let mut client = LightClient::new(pow_genesis().light_params(), genesis.sealed_header(), chain.genesis()).unwrap();
    let follow = |chain: &Chain, client: &mut LightClient| {
        let mut outcomes = Vec::new();

        while let Some(LightRequest::Headers { from, .. }) = client.next() {
            outcomes.push(client.deliver_headers(from, headers_from(chain, from), LATER).unwrap());
        }

        outcomes
    };

    let a1 = mine_on(&chain, &genesis, Vec::new(), Address::Ed25519([1; 32]));
    extended(chain.import(&a1, LATER));
    let a2 = mine_on(&chain, &a1, Vec::new(), Address::Ed25519([1; 32]));
    extended(chain.import(&a2, LATER));
    follow(&chain, &mut client);

    assert_eq!(client.tip().id(), a2.id());

    // A heavier branch from genesis takes over. Under the dev chain's voting finality nothing past
    // the justified checkpoint, here genesis, is settled: the client goes back to it and follows
    // the node's branch from there.
    let mut branch = genesis.clone();

    for _ in 0..3 {
        branch = mine_on(&chain, &branch, Vec::new(), Address::Ed25519([2; 32]));
        chain.import(&branch, LATER).unwrap();
    }

    client.refresh();
    let outcomes = follow(&chain, &mut client);

    assert_eq!(chain.tip().id(), branch.id());
    assert_eq!(client.tip().id(), branch.id());
    assert_eq!(outcomes[..2], [Extended::Unlinked, Extended::Added(3)]);

    // A header consensus refuses is refused: here, one declaring a difficulty its parent does not
    // make it.
    let mut forged = stored_header_at(&*chain.db, 3).unwrap().unwrap();
    forged.header.difficulty += 1;
    let mut fresh = LightClient::new(pow_genesis().light_params(), genesis.sealed_header(), chain.genesis()).unwrap();
    let mut headers = headers_from(&chain, 1);
    headers[2] = forged;

    assert!(matches!(fresh.deliver_headers(1, headers, LATER), Err(crate::light::LightError::BadHeader { height: 3, .. })));
}

#[test]
fn a_full_block_past_wincodes_default_limit_builds_imports_and_reads_back() {
    // 16,000 funded senders, each with one transfer: more than 4 MiB of them in memory.
    let keys: Vec<Keypair> = (0..16_000u64)
        .map(|index| {
            let mut seed = [0x5a; 32];
            seed[..8].copy_from_slice(&index.to_le_bytes());

            Keypair::from_seed(seed)
        })
        .collect();
    let mut genesis = Genesis { max_block_bytes: Some(crate::chain::MAX_BLOCK_BYTES), ..Genesis::dev_authority(authority().public()) };
    genesis.allocations.extend(keys.iter().map(|key| Allocation { address: key.public().address(), amount: u256::new(10u128.pow(21)) }));
    let db = Arc::new(MemoryDb::default());
    let mut chain = Chain::open(db.clone(), &genesis).unwrap();
    let base = chain.next_base_price();
    // An Ed25519 transfer's gas exactly; other schemes' cost less.
    let gas_limit = 3_620;
    let txs: Vec<SignedTx> = keys
        .iter()
        .enumerate()
        .map(|(index, key)| {
            let to = keys[(index + 1) % keys.len()].public().address();
            let fee = FeeOffer { gas_limit, max_price: base * u256::new(100), tip: u256::ZERO };

            SignedTx::sign(Tx::Transfer { from: key.public().address(), to, amount: Amount::U128(1) }, 0, fee, key)
        })
        .collect();
    let now = chain.tip().timestamp + SLOT;
    let mut block = chain.build(txs, Vec::new(), producer().address(&[]), now).unwrap();
    block.seal = producer().seal(&block.header).unwrap();

    // The block's gas limit binds, at 13,812 transfers with Ed25519 keys: over 4 MiB in memory.
    let block_gas_limit = genesis.fees.as_ref().unwrap().block_gas_limit;

    assert!(block.txs.len() < keys.len());
    assert!(block.header.gas_used + gas_limit > block_gas_limit, "the next would not fit");
    assert!(block.txs.len() * std::mem::size_of::<SignedTx>() > 4 << 20);

    if cfg!(feature = "ed25519") {
        assert_eq!(block.txs.len(), 13_812);
    }

    // Encoded as any list is: the transactions' own encoding right after the header.
    let bytes = wincode::serialize(&block).unwrap();
    let header = wincode::serialized_size(&block.header).unwrap() as usize;

    assert!(bytes[header..].starts_with(&wincode::serialize(&block.txs).unwrap_or_else(|_| {
        // wincode's own Vec refuses this many, which is the point: compare a short prefix instead.
        let mut prefix = (block.txs.len() as u64).to_le_bytes().to_vec();
        prefix.extend(wincode::serialize(&block.txs[0]).unwrap());
        prefix
    })));
    assert_eq!(wincode::deserialize::<Block>(&bytes).unwrap(), block);

    assert!(matches!(chain.import(&block, now), Ok(Imported::Extended(receipts)) if receipts.len() == block.txs.len()));
    assert_eq!(chain.block(&block.id()).unwrap(), Some(block), "and it reads back from the database");
}

#[cfg(all(feature = "evm", feature = "secp256k1"))]
mod dex;
#[cfg(feature = "evm")]
mod evm;
#[cfg(feature = "names")]
mod names;
mod multisig;

/// The state at a main-chain height, through history.
fn state_at(db: &Arc<MemoryDb>, chain: &Chain, height: u64) -> Option<Arc<State>> {
    let id = chain.block_at(height).unwrap().unwrap().id();

    stored_state_at(db.clone(), &id).unwrap()
}

#[test]
fn state_reads_and_proves_at_every_kept_height() {
    let db = Arc::new(MemoryDb::default());
    let mut chain = open(&db);
    let to = stranger().public().address();

    // Each block pays 10 more; the third deploys a contract, the fifth calls it.
    for amount in 1..=6u128 {
        let mut txs = vec![transfer(&chain, &authority(), &to, amount * 10)];

        if amount == 3 {
            let deploy = Tx::Deploy { code: Code::Lua(COUNTER.to_string()) };
            txs.push(SignedTx::sign(deploy, nonce(&chain, &authority()) + 1, FeeOffer::NONE, &authority()));
        }

        advance(&mut chain, txs);
    }

    let counted: u128 = (1..=6).map(|amount| amount * 10).sum();
    assert_eq!(balance(&chain, &to), u256::from(counted));

    for height in 0..=6u64 {
        let state = state_at(&db, &chain, height).unwrap();
        let header = chain.block_at(height).unwrap().unwrap().header;
        let paid: u128 = (1..=u128::from(height)).map(|amount| amount * 10).sum();

        // What it held then, and a tree whose root is the one that block committed to.
        assert_eq!(Accounts::new(&state).get(&to).unwrap().balance, u256::from(paid), "at {height}");
        assert_eq!(StateTree::new(&state, chain.hash()).root().unwrap(), header.state_root, "the root at {height}");

        let key = crate::db::address_key(&to);
        let proof = StateTree::new(&state, chain.hash()).prove(Column::Accounts, &key).unwrap();
        let value = state.get(Column::Accounts, &key).unwrap();

        assert!(proof.verify(&header.state_root, Column::Accounts, &key, value.as_deref()), "a proof at {height}");
    }

    // The contract is there from the block that deployed it, its code read as it was.
    let contract = contract_address(&authority().public().address(), 3, chain.hash());
    assert!(crate::registry::Contracts::new(&state_at(&db, &chain, 2).unwrap()).get(&contract).unwrap().is_none());
    assert!(crate::registry::Contracts::new(&state_at(&db, &chain, 3).unwrap()).get(&contract).unwrap().is_some());

    // Reopened, the node still reads its history.
    let reopened = open(&db);
    assert_eq!(reopened.history_from().unwrap(), 0);
    assert_eq!(Accounts::new(&state_at(&db, &reopened, 2).unwrap()).get(&to).unwrap().balance, u256::from(30u64));
}

#[test]
fn history_past_the_window_goes_and_reads_below_it_are_refused() {
    let db = Arc::new(MemoryDb::default());
    let mut chain = open(&db);
    chain.keep_state_history(Some(3));
    let to = stranger().public().address();

    for amount in 1..=8u128 {
        let tx = transfer(&chain, &authority(), &to, amount);
        advance(&mut chain, vec![tx]);
    }

    assert_eq!(chain.history_from().unwrap(), 5, "three blocks back from 8");
    assert!(state_at(&db, &chain, 4).is_none());
    assert_eq!(Accounts::new(&state_at(&db, &chain, 5).unwrap()).get(&to).unwrap().balance, u256::from(15u64));

    // Nothing of the heights pruned is left in the column: only 6 to 8's entries and lists.
    let mut heights = std::collections::BTreeSet::new();
    db.scan(Column::History, &[0xff], &[0xff], &mut |key, _| {
        heights.insert(u64::from_be_bytes(key[1..9].try_into().unwrap()));
        true
    })
    .unwrap();
    assert_eq!(heights, std::collections::BTreeSet::from([6, 7, 8]));

    // An archive keeps every height from now on.
    chain.keep_state_history(None);

    for amount in 9..=12u128 {
        let tx = transfer(&chain, &authority(), &to, amount);
        advance(&mut chain, vec![tx]);
    }

    assert_eq!(chain.history_from().unwrap(), 5);
    assert!(state_at(&db, &chain, 5).is_some());
}

#[test]
fn a_reorg_takes_the_history_of_the_blocks_it_undoes() {
    let db = Arc::new(MemoryDb::default());
    let mut chain = Chain::open(db.clone(), &Genesis { finality: None, ..pow_genesis() }).unwrap();
    let genesis = chain.block_at(0).unwrap().unwrap();
    let (a, b) = (Address::Ed25519([1; 32]), Address::Ed25519([2; 32]));
    // The main chain: two blocks, each paying a its reward.
    let mut main = genesis.clone();

    for _ in 0..2 {
        main = mine_on(&chain, &main, Vec::new(), a);
        extended(chain.import(&main, LATER));
    }

    let rewarded = |state: &State, miner: &Address| Accounts::new(state).get(miner).unwrap().balance;
    assert!(rewarded(&state_at(&db, &chain, 1).unwrap(), &a) > u256::ZERO);

    // A branch of three off genesis, paying b, outweighs it.
    let mut branch = genesis;

    for _ in 0..3 {
        branch = mine_on(&chain, &branch, Vec::new(), b);
        let _ = chain.import(&branch, LATER).unwrap();
    }

    assert_eq!(chain.tip().id(), branch.id());

    for height in 1..=3u64 {
        let state = state_at(&db, &chain, height).unwrap();

        assert_eq!(rewarded(&state, &a), u256::ZERO, "the undone blocks' rewards are gone at {height}");
        assert!(rewarded(&state, &b) > u256::ZERO);
        assert_eq!(StateTree::new(&state, chain.hash()).root().unwrap(), chain.block_at(height).unwrap().unwrap().header.state_root);
    }
}

#[test]
fn light_updates_stored_before_eras_or_switched_epochs_read_back() {
    use crate::light::{ErasLightUpdate, LegacyLightUpdate, LightUpdate};

    let db = Arc::new(MemoryDb::default());
    let block = Hash::zero(HashType::default_chain());
    let current = LightUpdate { block, voters: Vec::new(), eras: Vec::new(), switched: vec![7, 8] };
    let stored = [
        wincode::serialize(&current).unwrap(),
        wincode::serialize(&ErasLightUpdate { block, voters: Vec::new(), eras: Vec::new() }).unwrap(),
        wincode::serialize(&LegacyLightUpdate { block, voters: Vec::new() }).unwrap(),
    ];

    for (bytes, switched) in stored.into_iter().zip([vec![7, 8], Vec::new(), Vec::new()]) {
        let state = State::new(db.clone());
        state.put(Column::Light, id_key(&block), bytes);
        db.commit(state.take_changes()).unwrap();

        assert_eq!(stored_light_update(&*db, &block).unwrap(), Some(LightUpdate { switched, ..current.clone() }));
    }
}
