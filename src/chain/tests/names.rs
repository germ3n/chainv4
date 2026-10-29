//! Names through the whole chain: registrations, records and primary names as transactions in
//! blocks, what they burn beside their fees, and chains without names.

use {
    super::*,
    crate::crypto::verifiable::{Verifiable, VerifyError},
    crate::registry::names::{NameAction, NameStatus, Names, NamesConfig, YEAR_MS},
};

fn name_tx(chain: &Chain, from: &Keypair, name: &str, action: NameAction) -> SignedTx {
    SignedTx::sign(Tx::Name { name: name.to_string(), action }, nonce(chain, from), FeeOffer::NONE, from)
}

fn grace() -> u64 {
    NamesConfig::dev().grace_ms
}

/// A stranger holding a thousand coins, enough for a few names.
fn funded_stranger(chain: &mut Chain) -> Keypair {
    let stranger = stranger();
    let coins = 1_000 * 10u128.pow(18);
    let funding = transfer(chain, &authority(), &stranger.public().address(), coins);

    assert_eq!(advance(chain, vec![funding])[0].result, Ok(Effect::Done));

    stranger
}

#[test]
fn names_register_resolve_and_burn_their_price() {
    let db = Arc::new(MemoryDb::default());
    let mut chain = open(&db);
    let alice = funded_stranger(&mut chain);
    let before = balance(&chain, &alice.public().address());
    let register = name_tx(&chain, &alice, "alice", NameAction::Register { years: 2 });
    let receipts = advance(&mut chain, vec![register]);
    let now = chain.tip().timestamp;
    let names = chain.state();
    let names = Names::new(&names);

    assert_eq!(receipts[0].result, Ok(Effect::Done));
    assert_eq!(before - balance(&chain, &alice.public().address()), NamesConfig::dev().prices[2] * u256::from(2u64), "two years at five characters' price, burned");
    assert_eq!(names.resolve("alice", now, grace()).unwrap(), Some(alice.public().address()));
    assert_eq!(names.lookup("alice", now, grace()).unwrap().unwrap().expires, now + 2 * YEAR_MS, "from the block's own time");

    // The same name in one block twice: the first takes it, the second fails but uses its nonce.
    let bob = funded_stranger(&mut chain);
    let carol = funded_stranger(&mut chain);
    let first = name_tx(&chain, &bob, "bob", NameAction::Register { years: 1 });
    let second = name_tx(&chain, &carol, "bob", NameAction::Register { years: 1 });
    let carol_before = balance(&chain, &carol.public().address());
    let receipts = advance(&mut chain, vec![first, second]);

    assert_eq!(receipts[0].result, Ok(Effect::Done));
    assert!(matches!(&receipts[1].result, Err(failure) if failure.to_string() == "bob.dev is taken"), "{:?}", receipts[1].result);
    assert_eq!(balance(&chain, &carol.public().address()), carol_before, "a failed registration burns nothing");
    assert_eq!(nonce(&chain, &carol), 1);

    // A primary name, then a subname for someone else.
    let primary = SignedTx::sign(Tx::PrimaryName { name: Some("alice".into()) }, nonce(&chain, &alice), FeeOffer::NONE, &alice);
    let subname = name_tx(&chain, &bob, "bob", NameAction::Subname { label: "pay".into(), owner: carol.public().address() });
    let receipts = advance(&mut chain, vec![primary, subname]);
    let state = chain.state();
    let now = chain.tip().timestamp;

    assert!(receipts.iter().all(|receipt| receipt.result == Ok(Effect::Done)), "{receipts:?}");
    assert_eq!(Names::new(&state).primary(&alice.public().address(), now, grace()).unwrap(), Some("alice".to_string()));
    assert_eq!(Names::new(&state).resolve("pay.bob", now, grace()).unwrap(), Some(carol.public().address()));

    assert_eq!(crate::registry::supply::adds_up(&chain.state()).unwrap(), Ok(()), "minted less burned is what is held and bonded");
}

#[test]
fn a_name_lapses_with_block_time_and_returns_to_anyone() {
    let db = Arc::new(MemoryDb::default());
    let mut chain = open(&db);
    let alice = funded_stranger(&mut chain);
    let bob = funded_stranger(&mut chain);

    let register = name_tx(&chain, &alice, "alice", NameAction::Register { years: 1 });
    advance(&mut chain, vec![register]);

    let expires = Names::new(&chain.state()).lookup("alice", chain.tip().timestamp, grace()).unwrap().unwrap().expires;
    // A block a year and a grace period later, as a long-quiet chain would make.
    let late = |chain: &mut Chain, txs: Vec<SignedTx>, at: u64| {
        let mut block = chain.build(txs, Vec::new(), producer().address(&[]), at).unwrap();
        block.seal = producer().seal(&block.header).unwrap();

        extended(chain.import(&block, at))
    };
    let early = name_tx(&chain, &bob, "alice", NameAction::Register { years: 1 });
    let in_grace = late(&mut chain, vec![early], expires + 1);

    assert!(matches!(&in_grace[0].result, Err(failure) if failure.to_string() == "alice.dev is taken"));
    assert_eq!(Names::new(&chain.state()).lookup("alice", expires + 1, grace()).unwrap().unwrap().status, NameStatus::Grace);

    let after = expires + grace() + 1;
    let again = name_tx(&chain, &bob, "alice", NameAction::Register { years: 1 });
    let taken = late(&mut chain, vec![again], after);

    assert_eq!(taken[0].result, Ok(Effect::Done));
    assert_eq!(Names::new(&chain.state()).resolve("alice", after, grace()).unwrap(), Some(bob.public().address()));
}

#[test]
fn under_fees_a_name_pays_its_gas_and_burns_its_price() {
    let db = Arc::new(MemoryDb::default());
    let paid = Genesis::dev_authority(authority().public());
    let mut chain = Chain::open(db, &paid).unwrap();
    let fees = paid.fees.clone().unwrap().build();
    let offer = FeeOffer { gas_limit: 100_000, max_price: u256::new(1_000_000_000), tip: u256::ZERO };
    let register = SignedTx::sign(Tx::Name { name: "alice".into(), action: NameAction::Register { years: 1 } }, 0, offer, &authority());
    let intrinsic = fees.schedule().intrinsic(&register);
    let before = balance(&chain, &authority().public().address());
    let receipt = advance(&mut chain, vec![register]).remove(0);

    assert_eq!(receipt.result, Ok(Effect::Done));
    assert_eq!(receipt.gas_used, intrinsic, "nothing runs: a name costs its bytes' gas");
    // The authority produces the block too, so it gets the fee's unburned share back.
    assert!(before - balance(&chain, &authority().public().address()) >= NamesConfig::dev().prices[2]);

    assert_eq!(crate::registry::supply::adds_up(&chain.state()).unwrap(), Ok(()), "minted less burned is what is held and bonded");
}

#[test]
fn names_need_a_chain_with_names_and_must_be_names() {
    let db = Arc::new(MemoryDb::default());
    let nameless = Genesis { names: None, ..genesis() };
    let mut chain = Chain::open(db, &nameless).unwrap();
    let register = name_tx(&chain, &authority(), "alice", NameAction::Register { years: 1 });

    assert!(matches!(&advance(&mut chain, vec![register])[0].result, Err(failure) if failure.to_string() == "this chain has no names"));

    // What could never be a name is not a transaction at all.
    let bad = [
        Tx::Name { name: "Alice".into(), action: NameAction::Register { years: 1 } },
        Tx::Name { name: "al".into(), action: NameAction::Register { years: 1 } },
        Tx::Name { name: "alice".into(), action: NameAction::Register { years: 0 } },
        Tx::Name { name: "alice".into(), action: NameAction::Text { key: "URL".into(), value: None } },
        Tx::Name { name: "alice".into(), action: NameAction::Subname { label: "a.b".into(), owner: authority().public().address() } },
        Tx::PrimaryName { name: Some("x".into()) },
    ];

    for tx in bad {
        assert_eq!(tx.verify(), Err(VerifyError::BadName), "{tx:?}");
    }

    assert_eq!(Tx::PrimaryName { name: None }.verify(), Ok(()));
    assert!(Genesis { names: Some(NamesConfig { suffix: "Dev".into(), ..NamesConfig::dev() }), ..genesis() }.problem().is_some());
}

#[test]
fn lua_contracts_resolve_names_and_subnames_at_the_blocks_time() {
    const PAYER: &str = r#"local M = {}

function M.pay(name)
    local to = names.resolve(name)
    assert(to, "no such name")
    accounts.transfer(to, ctx.value)
    return to
end

function M.whois(account)
    return names.name_of(account)
end

return M
"#;
    let db = Arc::new(MemoryDb::default());
    let mut chain = open(&db);
    let alice = funded_stranger(&mut chain);
    let bob = stranger();
    let payer = crate::registry::contracts::contract_address(&authority().public().address(), nonce(&chain, &authority()), chain.hash());
    let deploy = SignedTx::sign(Tx::Deploy { code: Code::Lua(PAYER.to_string()) }, nonce(&chain, &authority()), FeeOffer::NONE, &authority());
    let register = name_tx(&chain, &alice, "alice", NameAction::Register { years: 1 });
    advance(&mut chain, vec![deploy, register]);
    let subname = name_tx(&chain, &alice, "alice", NameAction::Subname { label: "pay".into(), owner: bob.public().address() });
    let primary = SignedTx::sign(Tx::PrimaryName { name: Some("alice".into()) }, nonce(&chain, &alice) + 1, FeeOffer::NONE, &alice);
    advance(&mut chain, vec![subname, primary]);

    let call = |chain: &Chain, function: &str, amount: u128, args: Vec<TxCallArgumentType>, offset: u64| {
        SignedTx::sign(
            Tx::ContractCall { to: payer, func: crate::crypto::hasher::Hasher::digest(HashType::Xxh3_64, function.as_bytes()), amount: Amount::U128(amount), data: args },
            nonce(chain, &authority()) + offset,
            FeeOffer::NONE,
            &authority(),
        )
    };
    let calls = vec![
        call(&chain, "pay", 700, vec![TxCallArgumentType::String("pay.alice.dev".into())], 0),
        call(&chain, "whois", 0, vec![TxCallArgumentType::Address(alice.public().address())], 1),
        call(&chain, "pay", 1, vec![TxCallArgumentType::String("nobody.dev".into())], 2),
    ];
    let receipts = advance(&mut chain, calls);

    assert_eq!(receipts[0].result, Ok(Effect::Returned(vec![TxCallArgumentType::Address(bob.public().address())])), "the subname's owner");
    assert_eq!(balance(&chain, &bob.public().address()), u256::new(700));
    assert_eq!(receipts[1].result, Ok(Effect::Returned(vec![TxCallArgumentType::String("alice.dev".into())])));
    assert!(matches!(&receipts[2].result, Err(failure) if failure.to_string().contains("no such name")));

    // A chain without names has a names library still, so every node runs a script alike; it fails.
    let nameless = Genesis { names: None, ..genesis() };
    let mut chain = Chain::open(Arc::new(MemoryDb::default()), &nameless).unwrap();
    let payer_there = crate::registry::contracts::contract_address(&authority().public().address(), 0, chain.hash());
    let deploy = SignedTx::sign(Tx::Deploy { code: Code::Lua(PAYER.to_string()) }, 0, FeeOffer::NONE, &authority());
    advance(&mut chain, vec![deploy]);
    let lookup = SignedTx::sign(
        Tx::ContractCall { to: payer_there, func: crate::crypto::hasher::Hasher::digest(HashType::Xxh3_64, b"pay"), amount: Amount::U128(0), data: vec![TxCallArgumentType::String("alice.dev".into())] },
        1,
        FeeOffer::NONE,
        &authority(),
    );

    assert!(matches!(&advance(&mut chain, vec![lookup])[0].result, Err(failure) if failure.to_string().contains("this chain has no names")));
}
