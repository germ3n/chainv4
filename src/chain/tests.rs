use super::*;
use crate::consensus::{AuthorityProducer, DevAuthority, Producer, meets, work_hash};
use crate::crypto::keypair::Keypair;
use crate::db::MemoryDb;
use crate::registry::contracts::contract_address;
use crate::registry::RegistryError;
use crate::structs::amount::Amount;
use crate::structs::code::{Code, Engine};
use crate::structs::tx::{FeeOffer, Tx, TxCallArgumentType};
use crate::vm::selector;

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
    let mut block = chain.build(txs, Vec::new(), producer().address(), now).unwrap();
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

    assert!(matches!(
        Chain::open(db.clone(), &genesis()),
        Err(ChainError::OldFormat { found: Some(found), expected: STORAGE_FORMAT }) if found == STORAGE_FORMAT + 1
    ));

    let state = State::new(db.clone());
    state.delete(Column::Meta, FORMAT_KEY.to_vec());
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

    for expected in [5.0, 10.0] {
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
            Ok(Effect::Returned(vec![TxCallArgumentType::Number(expected)]))
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
/// chain hash, as every proof-of-work chain in these tests is.
fn mine_on(chain: &Chain, parent: &Block, txs: Vec<SignedTx>, miner: Address) -> Block {
    let timestamp = parent.header.timestamp + 1_000;
    let header = BlockHeader {
        parent: parent.id(),
        height: parent.header.height + 1,
        timestamp,
        producer: miner,
        difficulty: chain.consensus().difficulty(&parent.header, timestamp),
        tx_root: Block::tx_root(chain.hash(), &txs),
        proof_root: Block::proof_root(chain.hash(), &[]),
        gas_used: 0,
        base_price: u256::ZERO,
    };
    let nonce = (0..)
        .find(|nonce| meets(work_hash(chain.hash(), &header.id(), *nonce), header.difficulty))
        .unwrap();

    Block {
        header,
        txs,
        proofs: Vec::new(),
        seal: Seal::Work { nonce },
    }
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
    let a3 = mine_on(&chain, &a2, Vec::new(), miner_a);

    assert!(matches!(chain.import(&a2, LATER), Ok(Imported::Side)));
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

    let leaves = HashSet::from([a1.id(), b1.id()]);

    assert_eq!(stored_tips(&*db).unwrap().into_iter().collect::<HashSet<_>>(), leaves);
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
fn branches_that_split_off_too_deep_are_never_followed() {
    let db = Arc::new(MemoryDb::default());
    let mut chain = Chain::open(db.clone(), &pow_genesis()).unwrap();
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

    let mut side = genesis;
    let tip = chain.tip().id();

    for _ in 0..DEV_MAX_REORG_DEPTH + 4 {
        side = mine_on(&chain, &side, Vec::new(), Address::Ed25519([2; 32]));
        assert!(matches!(chain.import(&side, LATER), Ok(Imported::Side)));
    }

    assert_eq!(chain.tip().id(), tip);
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
    let ping = call("ping", 1, vec![TxCallArgumentType::I64(2)]);
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
            (emitter, "Ping", vec![TxCallArgumentType::Number(1.0), sender.clone()]),
            (emitter, "Ping", vec![TxCallArgumentType::Number(2.0), sender]),
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
    assert_eq!(version(&mut chain, 2), Ok(Effect::Returned(vec![TxCallArgumentType::Number(1.0)])));

    let receipts = advance(&mut chain, vec![upgrade(V2, 3, &owner)]);

    assert_eq!(receipts[0].result, Ok(Effect::Upgraded(contract)));
    assert_eq!(version(&mut chain, 4), Ok(Effect::Returned(vec![TxCallArgumentType::Number(2.0)])));
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

    assert_eq!(stored_events_from(&*db).unwrap(), None);
    assert_eq!(chain.index_events().unwrap(), 2);
    assert_eq!(chain.index_events().unwrap(), 2, "turning it on again changes nothing");

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
    assert_eq!((mentions[0].height, mentions[0].block, mentions[0].event.data[1].clone()), (2, a2.id(), TxCallArgumentType::Number(3.0)));
    assert_eq!(mentions[0].tx, ping(alice, 3, 3).id(HashType::default_chain()));

    // Pages pick up where the last stopped.
    let (first, next) = stored_events(&*db, &EventFilter::Contract(emitter), None, 3).unwrap();
    let (rest, after) = stored_events(&*db, &EventFilter::Contract(emitter), next.as_deref(), 3).unwrap();

    assert_eq!((first.len(), rest.len(), after), (3, 1, None));
    assert_eq!((rest[0].index, rest[0].position), (0, 0));
    assert_eq!(Chain::open(db.clone(), &pow_genesis()).unwrap().events_from(), Some(2), "it stays on");

    // A heavier branch takes the indexed block, and its events, off.
    let b2 = mine_on(&chain, &a1, Vec::new(), miner);
    let b3 = mine_on(&chain, &b2, Vec::new(), miner);
    chain.import(&b2, LATER).unwrap();
    assert!(matches!(chain.import(&b3, LATER), Ok(Imported::Reorganized { .. })));

    assert!(listed(EventFilter::Contract(emitter)).is_empty());
    assert!(stored_events(&*db, &EventFilter::Mentions(bob), None, 100).unwrap().0.is_empty());
}
