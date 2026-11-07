//! Tx::Remove at the registry, and the permissions check_perms is asked: removal deletes a
//! contract's code and every entry of its storage, pays its balance out, and keeps its address
//! from every other contract; upgrades show check_perms the new code's hash; perms asks a
//! contract's own check_perms about its own functions.

use super::*;
use crate::db::Committed;
use crate::registry::RegistryError;

/// Remembers who deployed it, keeps entries in plain storage and a namespace, and lets its owner
/// remove it with its balance to the owner alone.
const REMOVABLE: &str = r#"
local M = {}
local notes = storage.namespace("notes")

function M.init()
    storage.set("owner", ctx.sender:text())
    storage.set("a", "1")
    notes:set("b", "2")
end

function M.fill(count)
    for i = 1, count:tonumber() do
        storage.set("k" .. i, "v")
    end
end

function M.check_perms(permission, who, detail)
    return permission == "remove" and who:text() == storage.get("owner") and detail:text() == storage.get("owner")
end

function M.hello()
    return "hi"
end

return M
"#;

impl World {
    fn remove(&self, sender: Address, contract: Address, beneficiary: Address, limits: Limits) -> Result<crate::registry::contracts::Removed, RegistryError> {
        Contracts::new(&self.state)
            .remove(&self.vm, &sender, &contract, &beneficiary, limits, crate::vm::BlockAt::at(0))
            .map_err(|failed| failed.error)
    }

    /// How many entries a contract keeps, plain and namespaced.
    fn entries(&self, contract: Address) -> usize {
        let mut count = 0;

        for column in [Column::Storage, Column::Namespaces] {
            self.state.scan(column, &crate::db::address_key(&contract), &[], &mut |_, _| {
                count += 1;

                true
            })
            .unwrap();
        }

        count
    }
}

#[test]
fn a_removal_asks_check_perms_for_the_beneficiary_and_deletes_everything() {
    let mut world = World::new();
    let contract = world.lua_code(REMOVABLE);
    let caller = world.lua();
    let stranger = Address::Ed25519([5; 32]);
    world.fund(contract, 500);
    let kept = world.entries(contract);
    assert_eq!(kept, 3, "owner and a, and b in its namespace");

    // Only its owner, and only with its balance to the owner: check_perms sees the beneficiary.
    assert_eq!(world.remove(stranger, contract, DEPLOYER, LIMITS).unwrap_err(), RegistryError::NotPermitted);
    assert_eq!(world.remove(DEPLOYER, contract, stranger, LIMITS).unwrap_err(), RegistryError::NotPermitted);
    assert_eq!(world.call(contract, "hello", &[]).result.unwrap(), [TxCallArgumentType::String("hi".to_string())], "still there");

    let removed = world.remove(DEPLOYER, contract, DEPLOYER, LIMITS).unwrap();
    assert_eq!(removed.balance, u256::new(500));
    assert_eq!((world.balance(DEPLOYER), world.balance(contract)), (u256::new(500), u256::ZERO));
    assert!(removed.fuel_used >= kept as u64 * crate::params::vm::HostCosts::DEFAULT.storage_write, "each entry paid for");
    assert_eq!(world.entries(contract), 0);
    let contracts = Contracts::new(&world.state);
    assert_eq!(contracts.get(&contract).unwrap(), None);
    let removal = contracts.removed(&contract).unwrap().unwrap();
    assert_eq!((removal.by, removal.beneficiary, removal.balance, removal.entries, removal.engine), (DEPLOYER, DEPLOYER, u256::new(500), 3, Engine::Lua));

    // Gone, and said to be: to a transaction, to another contract, to an upgrade, to a second removal.
    let context = CallContext { sender: DEPLOYER, contract, value: u256::ZERO, block: crate::vm::BlockAt::at(0) };
    assert_eq!(contracts.call(&world.vm, &context, &Hash::Xxh3_64(selector("hello")), &[], LIMITS).unwrap_err(), RegistryError::Removed(contract));
    let tried = world.call(caller, "try", &[address(contract), TxCallArgumentType::String("hello".to_string())]).result.unwrap();
    assert!(matches!(&tried[1], TxCallArgumentType::String(why) if why.contains("was removed")), "{tried:?}");
    assert_eq!(world.upgrade(DEPLOYER, contract, Code::Lua(LUA.to_string())).unwrap_err(), RegistryError::Removed(contract));
    assert_eq!(world.remove(DEPLOYER, contract, DEPLOYER, LIMITS).unwrap_err(), RegistryError::Removed(contract));
}

#[test]
fn a_contract_with_more_storage_than_the_fuel_deletes_is_left_whole() {
    let mut world = World::new();
    let contract = world.lua_code(REMOVABLE);
    world.call(contract, "fill", &[TxCallArgumentType::I64(200)]).result.unwrap();
    let kept = world.entries(contract);
    let short = Limits { fuel: 50 * crate::params::vm::HostCosts::DEFAULT.storage_write, ..LIMITS };

    assert_eq!(world.remove(DEPLOYER, contract, DEPLOYER, short).unwrap_err(), RegistryError::Vm(VmError::OutOfFuel));
    assert_eq!(world.entries(contract), kept, "nothing deleted");
    assert!(Contracts::new(&world.state).removed(&contract).unwrap().is_none());
    assert_eq!(world.call(contract, "hello", &[]).result.unwrap(), [TxCallArgumentType::String("hi".to_string())]);
    world.remove(DEPLOYER, contract, DEPLOYER, LIMITS).unwrap();
}

#[test]
fn a_removed_clones_address_is_never_cloned_again() {
    let mut world = World::new();
    let template = world.lua_code(&CLONE_TEMPLATE.replace("\nreturn M", "\nfunction M.check_perms()\n    return true\nend\n\nreturn M"));
    let factory = world.lua_code(CLONE_FACTORY);
    let text = |value: &str| TxCallArgumentType::String(value.to_string());
    let clone = match world.call(factory, "make", &[address(template), text("a"), text("first")]).result.as_deref() {
        Ok([TxCallArgumentType::Address(clone)]) => *clone,
        other => panic!("{other:?}"),
    };

    world.remove(DEPLOYER, clone, DEPLOYER, LIMITS).unwrap();
    let again = world.call(factory, "try", &[address(template), text("a"), text("second")]).result.unwrap();
    assert!(matches!(&again[1], TxCallArgumentType::String(why) if why.contains("was removed")), "{again:?}");
    let other = world.call(factory, "try", &[address(template), text("b"), text("second")]).result.unwrap();
    assert_eq!(other[0], TxCallArgumentType::Bool(true), "another salt clones as ever");
}

#[test]
fn an_upgrade_shows_check_perms_the_new_codes_hash() {
    const GATED: &str = r#"
local M = {}

function M.allow(hash)
    storage.set("allowed", hash)
end

function M.check_perms(permission, who, detail)
    return permission == "upgrade" and detail == storage.get("allowed")
end

function M.version()
    return 1
end

return M
"#;
    const NEXT: &str = "local M = {}\nfunction M.version() return 2 end\nreturn M\n";
    let mut world = World::new();
    let gated = world.lua_code(GATED);
    let hash = Hasher::digest(world.vm.hash(), NEXT.as_bytes()).to_bytes();

    world.call(gated, "allow", &[TxCallArgumentType::Bytes(hash)]).result.unwrap();
    assert_eq!(world.upgrade(DEPLOYER, gated, Code::Lua(format!("{NEXT}\n"))).unwrap_err(), RegistryError::NotPermitted, "other code, another hash");
    world.upgrade(DEPLOYER, gated, Code::Lua(NEXT.to_string())).unwrap();
    assert_eq!(world.call(gated, "version", &[]).result.unwrap(), [TxCallArgumentType::I64(2)]);
}

#[test]
fn perms_asks_the_contracts_own_check_perms_about_its_own_functions() {
    const MINTER: &str = r#"
local M = {}

function M.init()
    storage.set("owner", ctx.sender:text())
end

function M.check_perms(permission, who, detail)
    return permission == "mint" and who:text() == storage.get("owner") and (detail == nil or detail <= 100)
end

function M.mint(amount)
    perms.require("mint", nil, amount)
    return "minted"
end

function M.may(who)
    return perms.check("mint", who)
end

return M
"#;
    let mut world = World::new();
    let minter = world.lua_code(MINTER);
    let bare = world.lua_code("local M = {}\nfunction M.may() return perms.check(\"mint\") end\nreturn M");
    let stranger = Address::Ed25519([5; 32]);

    assert_eq!(world.call(minter, "mint", &[TxCallArgumentType::I64(5)]).result.unwrap(), [TxCallArgumentType::String("minted".to_string())]);
    let over = world.call(minter, "mint", &[TxCallArgumentType::I64(500)]).result.unwrap_err();
    assert!(over.to_string().contains("not permitted: mint"), "{over}");
    let theirs = world.call_as(stranger, minter, "mint", &[TxCallArgumentType::I64(5)]).result.unwrap_err();
    assert!(theirs.to_string().contains("not permitted: mint"), "{theirs}");
    assert_eq!(world.call(minter, "may", &[address(stranger)]).result.unwrap(), [TxCallArgumentType::Bool(false)]);
    assert_eq!(world.call(minter, "may", &[address(DEPLOYER)]).result.unwrap(), [TxCallArgumentType::Bool(true)]);
    assert_eq!(world.call(bare, "may", &[]).result.unwrap(), [TxCallArgumentType::Bool(false)], "no check_perms permits nothing");
}
