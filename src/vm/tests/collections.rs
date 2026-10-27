//! The NFT and multi-token standards (docs/nft-standard.md), held against both reference
//! collections in each engine, and the factory that clones them.

use {
    super::*,
    crate::registry::collections::{Collection, CollectionKind, Collections},
};

/// Takes any token it is sent safely, and remembers the last.
const ACCEPTING: &str = r#"
local M = {}

function M.on_nft_received(operator, from, id, amount)
    storage.set("last", operator:text() .. " " .. from:text() .. " " .. tostring(id) .. " " .. tostring(amount))
    return true
end

function M.last()
    return storage.get("last")
end

return M
"#;

/// Exports no on_nft_received, so a safe transfer to it fails.
const PLAIN: &str = "local M = {}\nfunction M.hello() return 1 end\nreturn M";

fn id(value: u64) -> TxCallArgumentType {
    TxCallArgumentType::U256(u256::from(value))
}

fn text(value: &str) -> TxCallArgumentType {
    TxCallArgumentType::String(value.to_string())
}

fn yes() -> Vec<TxCallArgumentType> {
    vec![TxCallArgumentType::Bool(true)]
}

/// One call's result and events, panicking with what failed.
fn ok(world: &World, sender: Address, contract: Address, function: &str, args: &[TxCallArgumentType]) -> (Vec<TxCallArgumentType>, Vec<(String, Vec<TxCallArgumentType>)>) {
    let outcome = world.call_as(sender, contract, function, args);
    let events = outcome.events.iter().map(|event| (event.name.clone(), event.data.clone())).collect();

    (outcome.result.unwrap_or_else(|err| panic!("{function}: {err}")), events)
}

fn refused(world: &World, sender: Address, contract: Address, function: &str, args: &[TxCallArgumentType]) -> String {
    world.call_as(sender, contract, function, args).result.unwrap_err().to_string()
}

/// Runs the NFT standard against an example collection, whichever engine runs it.
fn conforms_to_the_nft_standard(code: Code) {
    let mut world = World::new();
    let nft = world.deploy(code.clone());
    let (accepting, plain) = (world.lua_code(ACCEPTING), world.lua_code(PLAIN));
    let zero = address(Address::Ed25519([0; 32]));
    // Carol holds an Ethereum address: tokens move to it, and prove under its text, the same.
    let (alice, bob, carol, dave) = (Address::Ed25519([0xa; 32]), Address::Ed25519([0xb; 32]), Address::Secp256k1([0xc; 20]), Address::Ed25519([0xd; 32]));
    let call = |sender: Address, function: &str, args: &[TxCallArgumentType]| ok(&world, sender, nft, function, args);
    let no = |sender: Address, function: &str, args: &[TxCallArgumentType]| refused(&world, sender, nft, function, args);
    let ids = |holder: Address| call(DEPLOYER, "tokens_of", &[address(holder)]).0;
    let owner = |token: u64| call(DEPLOYER, "owner_of", &[id(token)]).0;
    let transfer = |from: TxCallArgumentType, to: Address, token: u64| ("Transfer".to_string(), vec![from, address(to), id(token)]);
    // Where a light client proves it: the owners namespace, by the id in decimal.
    let provable = |token: u64| {
        world
            .state
            .get(Column::Namespaces, &crate::light::nft_owner_key(&nft, u256::from(token)))
            .unwrap()
            .map(|value| crate::light::nft_owner(&value).unwrap())
    };

    // It is an NFT collection, as it said.
    assert_eq!(
        Collections::new(&world.state).get(&nft).unwrap(),
        Some(Collection {
            address: nft,
            kind: CollectionKind::Nft,
            name: Some("Example Collection".to_string()),
            symbol: Some("EXC".to_string()),
        })
    );
    assert_eq!(Tokens::new(&world.state).get(&nft).unwrap(), None, "and not a token");

    // Only the owner mints: the next id, with its own URI or the base one.
    assert!(no(alice, "mint", &[address(alice)]).contains("only the collection's owner"));
    assert_eq!(call(DEPLOYER, "mint", &[address(alice)]), (vec![id(1)], vec![transfer(zero.clone(), alice, 1)]));
    assert_eq!(call(DEPLOYER, "mint", &[address(alice), text("ipfs://two")]).0, [id(2)]);
    assert_eq!((owner(1), owner(2)), (vec![address(alice)], vec![address(alice)]));
    assert_eq!(call(DEPLOYER, "balance_of", &[address(alice)]).0, [id(2)]);
    assert_eq!(call(DEPLOYER, "total_supply", &[]).0, [id(2)]);
    assert_eq!(ids(alice), [id(1), id(2)]);
    assert_eq!(call(DEPLOYER, "tokens_of", &[address(alice), id(1), id(5)]).0, [id(2)], "a page");
    assert!(ids(bob).is_empty());
    assert_eq!(call(DEPLOYER, "token_uri", &[id(1)]).0, [text("1")]);
    call(DEPLOYER, "set_base_uri", &[text("https://example.com/")]);
    assert_eq!(call(DEPLOYER, "token_uri", &[id(1)]).0, [text("https://example.com/1")]);
    assert_eq!(call(DEPLOYER, "token_uri", &[id(2)]).0, [text("ipfs://two")]);
    assert!(no(DEPLOYER, "owner_of", &[id(9)]).contains("no such token"));
    assert!(no(DEPLOYER, "token_uri", &[id(9)]).contains("no such token"));

    // transfer moves the caller's own.
    assert!(no(bob, "transfer", &[address(bob), id(1)]).contains("from does not own"));
    assert_eq!(call(alice, "transfer", &[address(bob), id(1)]), (yes(), vec![transfer(address(alice), bob, 1)]));
    assert_eq!((ids(alice), ids(bob)), (vec![id(2)], vec![id(1)]));
    assert!(no(bob, "transfer", &[zero.clone(), id(1)]).contains("zero address"));

    // approve lets one spender move one token, once.
    assert!(no(alice, "approve", &[address(carol), id(1)]).contains("not the owner or an operator"));
    assert_eq!(
        call(bob, "approve", &[address(carol), id(1)]),
        (yes(), vec![("Approval".to_string(), vec![address(bob), address(carol), id(1)])])
    );
    assert_eq!(call(DEPLOYER, "get_approved", &[id(1)]).0, [address(carol)]);
    assert!(no(dave, "transfer_from", &[address(bob), address(dave), id(1)]).contains("not the owner, approved or an operator"));
    assert_eq!(call(carol, "transfer_from", &[address(bob), address(carol), id(1)]).1, [transfer(address(bob), carol, 1)]);
    assert_eq!(call(DEPLOYER, "get_approved", &[id(1)]).0, [TxCallArgumentType::Nil], "gone once it moved");

    // An operator moves everything its owner holds.
    assert_eq!(
        call(alice, "set_approval_for_all", &[address(dave), TxCallArgumentType::Bool(true)]),
        (yes(), vec![("ApprovalForAll".to_string(), vec![address(alice), address(dave), TxCallArgumentType::Bool(true)])])
    );
    assert_eq!(call(DEPLOYER, "is_approved_for_all", &[address(alice), address(dave)]).0, yes());
    call(dave, "transfer_from", &[address(alice), address(bob), id(2)]);
    assert_eq!(owner(2), [address(bob)]);
    call(alice, "set_approval_for_all", &[address(dave), TxCallArgumentType::Bool(false)]);
    assert_eq!(call(DEPLOYER, "is_approved_for_all", &[address(alice), address(dave)]).0, [TxCallArgumentType::Bool(false)]);

    // A safe transfer to a contract asks it first; one without on_nft_received keeps nothing.
    assert!(no(bob, "safe_transfer", &[address(plain), id(2)]).contains("did not take"));
    assert_eq!(owner(2), [address(bob)]);
    assert_eq!(call(bob, "safe_transfer", &[address(accepting), id(2)]).0, yes());
    assert_eq!(owner(2), [address(accepting)]);
    assert_eq!(
        ok(&world, DEPLOYER, accepting, "last", &[]).0,
        [text(&format!("{bob} {bob} 2 1"))],
        "operator, from, id and amount"
    );
    // To a key, safe or not makes no difference; a plain transfer to a contract asks nothing.
    call(carol, "safe_transfer_from", &[address(carol), address(alice), id(1)]);
    call(alice, "transfer", &[address(plain), id(1)]);
    assert_eq!(owner(1), [address(plain)]);

    // Burning takes a token out of existence.
    let three = call(DEPLOYER, "mint", &[address(carol)]).0;

    assert_eq!(three, [id(3)]);
    assert!(no(bob, "burn", &[id(3)]).contains("not the owner"));
    assert_eq!(call(carol, "burn", &[id(3)]).1, [("Transfer".to_string(), vec![address(carol), zero, id(3)])]);
    assert!(no(DEPLOYER, "owner_of", &[id(3)]).contains("no such token"));
    assert_eq!(call(DEPLOYER, "total_supply", &[]).0, [id(2)]);
    assert!(ids(carol).is_empty());

    // An id of the owner's choosing, as a wrapped collection mints; the next goes past it, and an
    // id taken is refused.
    assert_eq!(call(DEPLOYER, "mint", &[address(dave), TxCallArgumentType::Nil, id(9)]).0, [id(9)]);
    assert!(no(DEPLOYER, "mint", &[address(dave), TxCallArgumentType::Nil, id(2)]).contains("exists already"));
    assert_eq!(call(DEPLOYER, "mint", &[address(dave)]).0, [id(10)]);
    call(dave, "burn", &[id(9)]);
    call(dave, "burn", &[id(10)]);

    // Only the owner upgrades, and owners outlive it.
    assert_eq!(world.upgrade(bob, nft, code.clone()), Err(crate::registry::RegistryError::NotPermitted));
    world.upgrade(DEPLOYER, nft, code).unwrap();

    for token in [1, 2, 3] {
        let held = world.call(nft, "owner_of", &[id(token)]).result.ok().map(|owner| owner[0].clone());

        assert_eq!(provable(token).map(address), held, "token {token}");
    }
}

#[test]
fn the_lua_nft_collection_follows_the_standard() {
    conforms_to_the_nft_standard(Code::Lua(include_str!("../../../contracts/lua/nft.lua").to_string()));
}

#[test]
fn the_rust_nft_collection_follows_the_standard() {
    conforms_to_the_nft_standard(Code::Wasm(example("nft")));
}

/// Runs the multi-token standard against an example collection, whichever engine runs it.
fn conforms_to_the_multi_token_standard(code: Code) {
    let mut world = World::new();
    let multi = world.deploy(code.clone());
    let (accepting, plain) = (world.lua_code(ACCEPTING), world.lua_code(PLAIN));
    let zero = address(Address::Ed25519([0; 32]));
    let (alice, bob, carol) = (Address::Ed25519([0xa; 32]), Address::Secp256k1([0xb; 20]), Address::Ed25519([0xc; 32]));
    let call = |sender: Address, function: &str, args: &[TxCallArgumentType]| ok(&world, sender, multi, function, args);
    let no = |sender: Address, function: &str, args: &[TxCallArgumentType]| refused(&world, sender, multi, function, args);
    let balance = |holder: Address, token: u64| call(DEPLOYER, "balance_of", &[address(holder), id(token)]).0;
    let ids = |holder: Address| call(DEPLOYER, "tokens_of", &[address(holder)]).0;
    let moved = |operator: Address, from: TxCallArgumentType, to: TxCallArgumentType, token: u64, amount: u64| {
        ("TransferSingle".to_string(), vec![address(operator), from, to, id(token), id(amount)])
    };
    let provable = |holder: Address, token: u64| {
        let kept = world.state.get(Column::Namespaces, &crate::light::multi_balance_key(&multi, u256::from(token), &holder)).unwrap();

        vec![TxCallArgumentType::U256(kept.map_or(u256::ZERO, |value| crate::light::token_balance(&value).unwrap()))]
    };

    assert_eq!(
        Collections::new(&world.state).get(&multi).unwrap(),
        Some(Collection {
            address: multi,
            kind: CollectionKind::Multi,
            name: Some("Example Items".to_string()),
            symbol: Some("EXI".to_string()),
        })
    );

    // Only the owner mints, any amount of any id.
    assert!(no(alice, "mint", &[address(alice), id(7), id(10)]).contains("only the collection's owner"));
    assert_eq!(
        call(DEPLOYER, "mint", &[address(alice), id(7), id(10), text("ipfs://seven")]),
        (yes(), vec![moved(DEPLOYER, zero.clone(), address(alice), 7, 10)])
    );
    call(DEPLOYER, "mint", &[address(alice), id(8), id(1)]);
    assert_eq!((balance(alice, 7), balance(alice, 8)), (vec![id(10)], vec![id(1)]));
    assert_eq!(call(DEPLOYER, "total_supply", &[id(7)]).0, [id(10)]);
    assert_eq!(call(DEPLOYER, "token_uri", &[id(7)]).0, [text("ipfs://seven")]);
    assert_eq!(call(DEPLOYER, "token_uri", &[id(8)]).0, [text("8")]);
    assert_eq!(ids(alice), [id(7), id(8)]);

    // transfer moves the caller's own; an id is listed while its holder holds some.
    assert_eq!(call(alice, "transfer", &[address(bob), id(7), id(4)]), (yes(), vec![moved(alice, address(alice), address(bob), 7, 4)]));
    assert_eq!((balance(alice, 7), balance(bob, 7)), (vec![id(6)], vec![id(4)]));
    assert!(no(alice, "transfer", &[address(bob), id(8), id(2)]).contains("insufficient balance"));
    call(alice, "transfer", &[address(bob), id(8), id(1)]);
    assert_eq!((ids(alice), ids(bob)), (vec![id(7)], vec![id(7), id(8)]));
    assert!(no(alice, "transfer", &[zero.clone(), id(7), id(1)]).contains("zero address"));

    // An operator moves what its holder holds; nobody else does.
    assert!(no(carol, "transfer_from", &[address(bob), address(carol), id(7), id(1)]).contains("not the holder or an operator"));
    assert_eq!(
        call(bob, "set_approval_for_all", &[address(carol), TxCallArgumentType::Bool(true)]).1,
        [("ApprovalForAll".to_string(), vec![address(bob), address(carol), TxCallArgumentType::Bool(true)])]
    );
    assert_eq!(call(carol, "transfer_from", &[address(bob), address(carol), id(7), id(4)]).1, [moved(carol, address(bob), address(carol), 7, 4)]);
    assert_eq!(ids(bob), [id(8)], "the last id taken out of the list in its place");

    // Safe transfers ask a receiving contract first.
    assert!(no(alice, "safe_transfer", &[address(plain), id(7), id(1)]).contains("did not take"));
    call(alice, "safe_transfer", &[address(accepting), id(7), id(2)]);
    assert_eq!(balance(accepting, 7), [id(2)]);
    assert_eq!(ok(&world, DEPLOYER, accepting, "last", &[]).0, [text(&format!("{alice} {alice} 7 2"))]);

    // Burning takes some out of existence.
    assert!(no(carol, "burn", &[address(alice), id(7), id(1)]).contains("not the holder or an operator"));
    assert_eq!(call(alice, "burn", &[address(alice), id(7), id(4)]).1, [moved(alice, address(alice), zero, 7, 4)]);
    assert_eq!(call(DEPLOYER, "total_supply", &[id(7)]).0, [id(6)]);
    assert!(ids(alice).is_empty());

    assert_eq!(world.upgrade(alice, multi, code.clone()), Err(crate::registry::RegistryError::NotPermitted));
    world.upgrade(DEPLOYER, multi, code).unwrap();

    for holder in [alice, bob, carol, accepting, DEPLOYER] {
        for token in [7, 8] {
            assert_eq!(provable(holder, token), balance(holder, token), "{holder} {token}");
        }
    }
}

#[test]
fn the_lua_multi_token_collection_follows_the_standard() {
    conforms_to_the_multi_token_standard(Code::Lua(include_str!("../../../contracts/lua/multi.lua").to_string()));
}

#[test]
fn the_rust_multi_token_collection_follows_the_standard() {
    conforms_to_the_multi_token_standard(Code::Wasm(example("multi")));
}

#[test]
fn the_factory_clones_collections_their_creators_own() {
    let mut world = World::new();
    let nft = world.lua_code(include_str!("../../../contracts/lua/nft.lua"));
    let multi = world.deploy(Code::Wasm(example("multi")));
    let factory = world.lua_code(include_str!("../../../contracts/lua/nft_factory.lua"));
    let alice = Address::Ed25519([0xa; 32]);
    let create = |kind: &str| world.call_as(alice, factory, "create", &[text(kind), text("Mine"), text("MINE"), text("https://mine.example/")]);

    assert!(create("nft").result.unwrap_err().to_string().contains("the templates are not set"));
    assert!(world.call_as(alice, factory, "setup", &[address(nft), address(multi)]).result.unwrap_err().to_string().contains("only the deployer"));
    world.call(factory, "setup", &[address(nft), address(multi)]).result.unwrap();
    assert!(world.call(factory, "setup", &[address(nft), address(multi)]).result.is_err(), "once");
    assert!(create("fungible").result.unwrap_err().to_string().contains("kind must be nft or multi"));

    for (kind, expected) in [("nft", CollectionKind::Nft), ("multi", CollectionKind::Multi)] {
        let created = create(kind);
        let [TxCallArgumentType::Address(collection)] = created.result.unwrap()[..] else {
            panic!("create returns the collection");
        };

        // The chain registers it once the transaction succeeds, as it does every clone.
        assert_eq!(created.refresh_tokens, [collection]);
        Contracts::new(&world.state).refresh(&world.vm, &collection, alice, LIMITS, &standard(), crate::vm::BlockAt::at(0)).unwrap();
        let registered = Collections::new(&world.state).get(&collection).unwrap().unwrap();

        assert_eq!((registered.kind, registered.name.as_deref(), registered.symbol.as_deref()), (expected, Some("Mine"), Some("MINE")));

        // Its creator owns it: they mint, the factory does not.
        let mint: Vec<TxCallArgumentType> = match expected {
            CollectionKind::Nft => vec![address(alice)],
            CollectionKind::Multi => vec![address(alice), id(1), id(1)],
        };

        world.call_as(alice, collection, "mint", &mint).result.unwrap();
        assert!(world.call(collection, "mint", &mint).result.is_err());
        assert_eq!(world.call(collection, "token_uri", &[id(1)]).result.unwrap(), [text("https://mine.example/1")]);
    }

    assert_eq!(world.call(factory, "count", &[]).result.unwrap(), [id(2)]);
    assert!(matches!(world.call(factory, "at", &[id(1)]).result.unwrap()[..], [TxCallArgumentType::Address(_)]));
}

#[test]
fn collections_need_both_lists_or_the_registry_and_answer_within_limits() {
    let mut world = World::new();
    let source = include_str!("../../../contracts/lua/nft.lua");
    let deploy = |world: &mut World, code: &str, standards: &crate::registry::Standards| {
        world.nonce += 1;

        Contracts::new(&world.state).deploy(&world.vm, &DEPLOYER, world.nonce, &Code::Lua(code.to_string()), LIMITS, standards, crate::vm::BlockAt::at(0))
    };

    // A chain without the registry keeps none.
    let (unlisted, _) = deploy(&mut world, source, &crate::registry::Standards::default()).unwrap();

    assert_eq!(Collections::new(&world.state).get(&unlisted).unwrap(), None);

    // A name the registry cannot keep fails the deploy.
    let long = source.replace("name or \"Example Collection\"", "string.rep(\"n\", 65)");
    let failed = deploy(&mut world, &long, &standard()).unwrap_err();

    assert!(matches!(&failed.error, crate::registry::RegistryError::BadCollection(why) if why.contains("name() must give 1 to 64 bytes")), "{}", failed.error);

    // An upgrade that drops a standard function takes it off; tokens_of is one.
    let (listed, _) = deploy(&mut world, source, &standard()).unwrap();
    let without = source.replace("function M.tokens_of(", "local function tokens_of(");

    assert!(Collections::new(&world.state).get(&listed).unwrap().is_some());
    world.upgrade(DEPLOYER, listed, Code::Lua(without)).unwrap();
    assert_eq!(Collections::new(&world.state).get(&listed).unwrap(), None);
}

#[cfg(feature = "sha256")]
#[test]
fn the_swap_contract_locks_and_pays_nfts_of_either_standard() {
    let mut world = World::new();
    let swap = world.lua_code(include_str!("../../../contracts/lua/swap.lua"));
    let art = world.lua_code(include_str!("../../../contracts/lua/nft.lua"));
    let items = world.deploy(Code::Wasm(example("multi")));
    let alice = Address::Ed25519([0xa; 32]);
    let secret = [0x7a; 32];
    let hash = to_hex(&crate::crypto::bitcoin::sha256(&secret));
    let now = 1_793_000_000_000u64;
    let deadline = TxCallArgumentType::I64((now + 3_600_000) as i64);
    world.call(art, "mint", &[address(DEPLOYER)]).result.unwrap();
    world.call(items, "mint", &[address(DEPLOYER), id(9), id(50)]).result.unwrap();

    // One NFT: refused until the contract may move it, then pulled in, and the secret pays it out.
    let nft = [text(&hash), address(alice), deadline.clone(), address(art), id(1)];

    assert!(world.call_at(DEPLOYER, swap, "lock_nft", &nft, 0, now).result.unwrap_err().to_string().contains("not the owner, approved"));
    world.call(art, "approve", &[address(swap), id(1)]).result.unwrap();
    assert!(world.call_at(DEPLOYER, swap, "lock_nft", &nft, 5, now).result.unwrap_err().to_string().contains("send none"));
    let locked = world.call_at(DEPLOYER, swap, "lock_nft", &nft, 0, now);

    assert_eq!(locked.events.iter().map(|event| event.name.as_str()).collect::<Vec<_>>(), ["Transfer", "Locked", "LockedItem"]);
    assert_eq!(locked.events[2].data, [text(&hash), text("nft"), id(1)]);
    assert_eq!(world.call(art, "owner_of", &[id(1)]).result.unwrap(), [address(swap)]);
    assert_eq!(world.call(swap, "item", &[text(&hash)]).result.unwrap(), [text("nft"), id(1)]);
    let got = world.call(swap, "get", &[text(&hash)]).result.unwrap();

    assert_eq!((got[3].clone(), got[4].clone()), (address(art), id(1)), "the collection, and how many");
    world.call_at(alice, swap, "claim", &[text(&to_hex(&secret))], 0, now + 1).result.unwrap();
    assert_eq!(world.call(art, "owner_of", &[id(1)]).result.unwrap(), [address(alice)]);

    // An amount of a multi-token id: the contract an operator, then locked, then refunded.
    let other = [0x6f; 32];
    let other_hash = to_hex(&crate::crypto::bitcoin::sha256(&other));
    let multi = [text(&other_hash), address(alice), deadline, address(items), id(9), id(20)];

    world.call(items, "set_approval_for_all", &[address(swap), TxCallArgumentType::Bool(true)]).result.unwrap();
    world.call_at(DEPLOYER, swap, "lock_multi", &multi, 0, now).result.unwrap();
    assert_eq!(world.call(items, "balance_of", &[address(swap), id(9)]).result.unwrap(), [id(20)]);
    assert_eq!(world.call(swap, "item", &[text(&other_hash)]).result.unwrap(), [text("multi"), id(9)]);
    world.call_at(alice, swap, "refund", &[text(&other_hash)], 0, now + 3_600_000).result.unwrap();
    assert_eq!(world.call(items, "balance_of", &[address(DEPLOYER), id(9)]).result.unwrap(), [id(50)]);

    // Coins say so, and nothing locked is none.
    let coin = to_hex(&crate::crypto::bitcoin::sha256(&[1; 32]));

    assert_eq!(world.call(swap, "item", &[text(&coin)]).result.unwrap(), [text("none"), id(0)]);
    world.call_at(DEPLOYER, swap, "lock", &[text(&coin), address(alice), TxCallArgumentType::I64((now + 3_600_000) as i64)], 5, now).result.unwrap();
    assert_eq!(world.call(swap, "item", &[text(&coin)]).result.unwrap(), [text("coin"), id(0)]);
}
