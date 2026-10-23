use super::*;
use crate::consensus::DevAuthority;
use crate::crypto::keypair::Keypair;
use crate::db::MemoryDb;
use crate::registry::contracts::contract_address;
use crate::registry::RegistryError;
use crate::structs::amount::Amount;
use crate::structs::tx::{Tx, TxCallArgumentType};
use crate::vm::lvm::selector;

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

fn genesis() -> Genesis {
    Genesis::dev(authority().public())
}

fn producer() -> DevAuthority {
    DevAuthority::new(authority().public(), SLOT, Some(authority()))
}

fn open(db: &Arc<MemoryDb>) -> Chain {
    Chain::open(db.clone(), &genesis()).unwrap()
}

fn next_block(chain: &Chain, txs: Vec<SignedTx>) -> (Block, u64) {
    let now = chain.tip().timestamp + SLOT;

    (chain.build(txs, &producer(), now).unwrap(), now)
}

/// Builds and imports the next block.
fn advance(chain: &mut Chain, txs: Vec<SignedTx>) -> Vec<Receipt> {
    let (block, now) = next_block(chain, txs);

    chain.import(&block, &producer(), now).unwrap()
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

    SignedTx::sign(tx, nonce(chain, from), from)
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
        slot_ms: 1_000,
        ..genesis()
    };

    assert!(matches!(
        Chain::open(db.clone(), &other),
        Err(ChainError::WrongGenesis { .. })
    ));
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

        SignedTx::sign(tx, nonce(chain, keypair), keypair)
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
            source: COUNTER.to_string(),
        },
        nonce(&chain, &deployer),
        &deployer,
    );
    let receipts = advance(&mut chain, vec![deploy]);
    let counter = contract_address(&deployer.public().address(), 0);

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
    bad_root.header.tx_root = Hash::Blake3_256([9; 32]);
    let mut bad_height = block.clone();
    bad_height.header.height = 5;

    assert!(matches!(
        chain.import(&bad_root, &producer(), now),
        Err(ImportError::Invalid(BlockError::TxRoot))
    ));
    assert!(matches!(
        chain.import(&bad_height, &producer(), now),
        Err(ImportError::Invalid(BlockError::Height))
    ));

    chain.import(&block, &producer(), now).unwrap();
    let (grandchild, later) = next_block(&chain, Vec::new());
    let mut orphan = grandchild.clone();
    orphan.header.parent = Hash::Blake3_256([7; 32]);

    assert!(matches!(chain.import(&block, &producer(), now), Err(ImportError::Known)));
    assert!(matches!(chain.import(&sibling, &producer(), now), Err(ImportError::NotOnTip(_))));
    assert!(matches!(chain.import(&orphan, &producer(), later), Err(ImportError::UnknownParent(_))));
}

#[test]
fn a_block_with_an_invalid_transaction_changes_nothing() {
    let db = Arc::new(MemoryDb::default());
    let mut chain = open(&db);
    let bob = stranger().public().address();
    let good = transfer(&chain, &authority(), &bob, 10);
    let mut replayed = transfer(&chain, &authority(), &bob, 20);
    replayed = SignedTx::sign(replayed.tx, 0, &authority());
    let (block, now) = next_block(&chain, vec![good, replayed]);

    assert!(matches!(
        chain.import(&block, &producer(), now),
        Err(ImportError::Invalid(BlockError::Tx {
            index: 1,
            error: TxError::WrongNonce { expected: 1, got: 0 },
        }))
    ));
    assert_eq!(chain.tip().height, 0);
    assert_eq!(balance(&chain, &bob), u256::ZERO);
    assert_eq!(nonce(&chain, &authority()), 0);
}
