//! The crypto library and host calls, from inside contracts of each native engine.

#[cfg(all(feature = "ed25519", feature = "bitcoin"))]
use {
    crate::crypto::keypair::{Keypair, PrivateKey},
    crate::db::address_key,
};
use super::*;

const LUA_CRYPTO: &str = r#"
local M = {}

function M.verify(scheme, key, message, signature) return crypto.verify(scheme, key, message, signature) end
function M.recover(digest, signature) return crypto.recover(digest, signature) end
function M.address(scheme, key) return crypto.address(scheme, key) end
function M.sha256d(data) return crypto.sha256d(data) end
function M.hash160(data) return crypto.hash160(data) end
function M.merkle(root, data, index, count, path) return crypto.check_merkle(root, data, index, count, path) end
function M.state(root, column, key, value, proof) return crypto.check_state(root, column, key, value, proof) end

return M
"#;

fn bytes(value: &[u8]) -> TxCallArgumentType {
    TxCallArgumentType::Bytes(value.to_vec())
}

fn text(value: &str) -> TxCallArgumentType {
    TxCallArgumentType::String(value.to_string())
}

#[cfg(all(feature = "ed25519", feature = "secp256k1", feature = "bitcoin"))]
#[test]
fn lua_checks_signatures_hashes_and_proofs() {
    let mut world = World::new();
    let lua = world.lua_code(LUA_CRYPTO);
    let call = |function: &str, args: &[TxCallArgumentType]| world.call(lua, function, args).result;

    // An Ed25519 signature holds over its message alone; the key's account comes back.
    let keypair = Keypair::from_private(PrivateKey::Ed25519([5; 32]));
    let crate::crypto::keypair::PublicKey::Ed25519(key) = keypair.public() else { unreachable!() };
    let crate::crypto::keypair::Signature::Ed25519(signature) = keypair.sign(b"hello") else { unreachable!() };

    assert_eq!(call("verify", &[text("ed25519"), bytes(&key), text("hello"), bytes(&signature)]).unwrap(), [TxCallArgumentType::Bool(true)]);
    assert_eq!(call("verify", &[text("ed25519"), bytes(&key), text("hullo"), bytes(&signature)]).unwrap(), [TxCallArgumentType::Bool(false)]);
    assert!(call("verify", &[text("rsa"), bytes(&key), text("hello"), bytes(&signature)]).unwrap_err().to_string().contains("not a scheme"));
    assert_eq!(call("address", &[text("ed25519"), bytes(&key)]).unwrap(), [address(keypair.public().address())]);

    // ecrecover's way: the key that signed a digest.
    let digest = [9; 32];
    let (rs, v) = crate::crypto::secp256k1::sign_recoverable(&[7; 32], &digest);
    let signer = crate::crypto::secp256k1::public_key(&[7; 32]).unwrap();

    assert_eq!(call("recover", &[bytes(&digest), bytes(&[rs.as_slice(), &[v + 27]].concat())]).unwrap(), [bytes(&signer)]);
    assert_eq!(call("recover", &[bytes(&digest), bytes(&rs)]).unwrap(), [TxCallArgumentType::Nil]);

    // Bitcoin's hashes.
    assert_eq!(call("sha256d", &[text("abc")]).unwrap(), [bytes(&crate::crypto::bitcoin::sha256d(b"abc"))]);
    assert_eq!(call("hash160", &[text("abc")]).unwrap(), [bytes(&crate::crypto::bitcoin::hash160(b"abc"))]);

    // A Merkle path, and a state proof, against roots the contract is handed.
    let hash = HashType::default_chain();
    let leaves: Vec<_> = [b"one".as_slice(), b"two", b"three"].iter().map(|data| crate::crypto::merkle::leaf(hash, data)).collect();
    let root = crate::crypto::merkle::root(hash, &leaves).to_bytes();
    let path: Vec<u8> = crate::crypto::merkle::audit_path(hash, &leaves, 2).iter().flat_map(|sibling| sibling.to_bytes()).collect();
    let merkle = |data: &str, index: i64| call("merkle", &[bytes(&root), text(data), TxCallArgumentType::I64(index), TxCallArgumentType::I64(3), bytes(&path)]).unwrap();

    assert_eq!(merkle("three", 2), [TxCallArgumentType::Bool(true)]);
    assert_eq!(merkle("three", 1), [TxCallArgumentType::Bool(false)]);

    let tree = crate::db::StateTree::new(&world.state, hash);
    let state_root = tree.follow().unwrap().to_bytes();
    let held = address_key(&lua);
    let record = world.state.get(Column::Contracts, &held).unwrap().unwrap();
    let proof = wincode::serialize(&tree.prove(Column::Contracts, &held).unwrap()).unwrap();

    assert_eq!(call("state", &[bytes(&state_root), text("contracts"), bytes(&held), bytes(&record), bytes(&proof)]).unwrap(), [TxCallArgumentType::Bool(true)]);
    assert_eq!(call("state", &[bytes(&state_root), text("contracts"), bytes(&held), TxCallArgumentType::Nil, bytes(&proof)]).unwrap(), [TxCallArgumentType::Bool(false)]);
}

/// Hashes its input with sha256d through crypto_hash, and recovers the key that signed a digest
/// through recover: the WebAssembly side of the same calls.
const WASM_CRYPTO: &str = r#"
(module
  (import "chain" "input_len" (func $input_len (result i32)))
  (import "chain" "input_read" (func $input_read (param i32)))
  (import "chain" "crypto_hash" (func $crypto_hash (param i32 i32 i32 i32 i32) (result i32)))
  (import "chain" "recover" (func $recover (param i32 i32 i32 i32) (result i32)))
  (import "chain" "output" (func $output (param i32 i32)))
  (memory (export "memory") 1)
  (data (i32.const 0) "sha256d")
  (data (i32.const 16) "abc")
  ;; The encoded list of one bytes value: count 1, tag 2, length 32, then the digest at 65.
  (data (i32.const 48) "\01\00\00\00\00\00\00\00\02\20\00\00\00\00\00\00\00")
  (func (export "double")
    (drop (call $crypto_hash (i32.const 0) (i32.const 7) (i32.const 16) (i32.const 3) (i32.const 65)))
    (call $output (i32.const 48) (i32.const 49)))
  ;; Reads 32 bytes of digest and 65 of signature from the input's one bytes value, at 256 + 17.
  (func (export "who")
    (call $input_read (i32.const 256))
    (if (i32.eq (call $recover (i32.const 273) (i32.const 305) (i32.const 65) (i32.const 65)) (i32.const 1))
      (then
        (i32.store8 (i32.const 57) (i32.const 33))
        (call $output (i32.const 48) (i32.const 50))))))
"#;

#[cfg(all(feature = "secp256k1", feature = "bitcoin"))]
#[test]
fn webassembly_reaches_the_same_crypto() {
    let mut world = World::new();
    let wasm = world.wasm(WASM_CRYPTO);

    assert_eq!(world.call(wasm, "double", &[]).result.unwrap(), [bytes(&crate::crypto::bitcoin::sha256d(b"abc"))]);

    let digest = [9; 32];
    let (rs, v) = crate::crypto::secp256k1::sign_recoverable(&[7; 32], &digest);
    let signer = crate::crypto::secp256k1::public_key(&[7; 32]).unwrap();
    let input = [digest.as_slice(), &rs, &[v]].concat();

    assert_eq!(world.call(wasm, "who", &[bytes(&input)]).result.unwrap(), [bytes(&signer)]);
}

fn federation_of(threshold: u32, signers: &[u8]) -> crate::registry::federation::Federation {
    crate::registry::federation::Federation {
        signers: signers.iter().map(|byte| crate::crypto::secp256k1::public_key(&[*byte; 32]).unwrap()).collect(),
        threshold,
        chain_depth: 20,
        evm_depth: 12,
        bitcoin_depth: 6,
        btc: None,
    }
}

const LUA_FEDERATION: &str = r#"
local M = {}

function M.read()
    local fed = federation.get()
    if fed == nil then return "none" end
    return fed.version, fed.threshold, #fed.signers, fed.signers[1]
end

return M
"#;

/// Returns the length of what the federation host call put in the result buffer, as an i64:
/// encoded, a count of 1, the i64 tag 6, then 8 bytes.
const WASM_FEDERATION: &str = r#"
(module
  (import "chain" "federation" (func $federation (result i32)))
  (import "chain" "output" (func $output (param i32 i32)))
  (memory (export "memory") 1)
  (data (i32.const 0) "\01\00\00\00\00\00\00\00\06")
  (func (export "read")
    (i64.store (i32.const 9) (i64.extend_i32_s (call $federation)))
    (call $output (i32.const 0) (i32.const 17))))
"#;

#[cfg(feature = "secp256k1")]
#[test]
fn contracts_read_the_federation_in_force() {
    let mut bare = World::new();
    let lua = bare.lua_code(LUA_FEDERATION);
    let wasm = bare.wasm(WASM_FEDERATION);

    assert_eq!(bare.call(lua, "read", &[]).result.unwrap(), [text("none")], "a chain with none");
    assert_eq!(bare.call(wasm, "read", &[]).result.unwrap(), [TxCallArgumentType::I64(-1)]);

    // Genesis's, version 0, until governance decides another.
    let genesis = federation_of(2, &[1, 2, 3]);
    let mut world = World {
        vm: Arc::new(Vm::new(&[Engine::Lua, Engine::Wasm], HashType::default_chain()).unwrap().with_federation(Some(genesis.clone()))),
        ..World::new()
    };
    let lua = world.lua_code(LUA_FEDERATION);
    let wasm = world.wasm(WASM_FEDERATION);

    assert_eq!(
        world.call(lua, "read", &[]).result.unwrap(),
        [TxCallArgumentType::I64(0), TxCallArgumentType::I64(2), TxCallArgumentType::I64(3), bytes(&genesis.signers[0])]
    );
    assert_eq!(world.call(wasm, "read", &[]).result.unwrap(), [TxCallArgumentType::I64(16 + 3 * 33)]);

    // What governance decided stands over genesis's.
    let decided = crate::registry::federation::DecidedFederation { version: 4, federation: federation_of(1, &[9]) };
    world.state.put_value(Column::Validators, b"change:federation".to_vec(), &decided);

    assert_eq!(world.call(lua, "read", &[]).result.unwrap()[..3], [TxCallArgumentType::I64(4), TxCallArgumentType::I64(1), TxCallArgumentType::I64(1)]);
}

const LUA_FEDERATION_AT: &str = r#"
local M = {}

function M.read(version)
    local fed = federation.at(version)
    if fed == nil then return "none" end
    return fed.version, fed.threshold, fed.signers[1], fed.decided_height, fed.decided_time, fed.replaced_height, fed.replaced_time
end

function M.once()
    return federation.at(0).version
end

function M.again()
    return federation.at(0).version, federation.at(0).version
end

return M
"#;

#[cfg(feature = "secp256k1")]
#[test]
fn contracts_read_a_federation_of_any_version_with_when_it_was_decided_and_replaced() {
    use crate::registry::federation::{DecidedFederation, DecidedWhen};

    let read = |world: &World, lua: Address, version: u64| world.call(lua, "read", &[TxCallArgumentType::U64(version)]).result.unwrap();
    let mut bare = World::new();
    let lua = bare.lua_code(LUA_FEDERATION_AT);

    assert_eq!(read(&bare, lua, 0), [text("none")], "a chain with none");

    // Genesis's, given to the VM: version 0, with no block kept for it, and in force.
    let genesis = federation_of(2, &[1, 2, 3]);
    let mut world = World {
        vm: Arc::new(Vm::new(&[Engine::Lua, Engine::Wasm], HashType::default_chain()).unwrap().with_federation(Some(genesis.clone()))),
        ..World::new()
    };
    let lua = world.lua_code(LUA_FEDERATION_AT);
    let none = TxCallArgumentType::Nil;

    assert_eq!(read(&world, lua, 0), [TxCallArgumentType::I64(0), TxCallArgumentType::I64(2), bytes(&genesis.signers[0]), none.clone(), none.clone(), none.clone(), none.clone()]);
    assert_eq!(read(&world, lua, 1), [text("none")], "a version never decided");

    // Governance decides two more: each past one says the block that replaced it.
    let changes = crate::registry::changes::Changes::new(&world.state);
    changes.write_federation(&DecidedFederation { version: 1, federation: federation_of(1, &[9]) }, DecidedWhen { height: 7, time: 70_000 });
    changes.write_federation(&DecidedFederation { version: 2, federation: federation_of(2, &[4, 5]) }, DecidedWhen { height: 9, time: 90_000 });

    assert_eq!(read(&world, lua, 0)[5..], [TxCallArgumentType::I64(7), TxCallArgumentType::I64(70_000)], "genesis's, replaced by version 1");
    assert_eq!(
        read(&world, lua, 1),
        [TxCallArgumentType::I64(1), TxCallArgumentType::I64(1), bytes(&federation_of(1, &[9]).signers[0]), TxCallArgumentType::I64(7), TxCallArgumentType::I64(70_000), TxCallArgumentType::I64(9), TxCallArgumentType::I64(90_000)]
    );
    assert_eq!(read(&world, lua, 2)[3..], [TxCallArgumentType::I64(9), TxCallArgumentType::I64(90_000), none.clone(), none], "in force");

    // A version is a whole number, and reading a past one costs what reading the one in force does.
    assert!(world.call(lua, "read", &[TxCallArgumentType::I64(-1)]).result.unwrap_err().to_string().contains("whole number"));
    assert!(world.call(lua, "read", &[TxCallArgumentType::String("1.5".into())]).result.is_err());
    let charged = world.call(lua, "again", &[]).fuel_used - world.call(lua, "once", &[]).fuel_used;
    let costs = crate::params::vm::HostCosts::default();
    assert!((costs.federation..costs.federation + 20).contains(&charged), "{charged}");
}

const LUA_BLS: &str = r#"
local M = {}

function M.check(keys, message, signature) return crypto.fast_aggregate_verify(keys, message, signature) end
function M.pair(a, b, message, signature) return crypto.fast_aggregate_verify({a, b}, message, signature) end

return M
"#;

#[cfg(feature = "bls12-381")]
#[test]
fn lua_checks_ethereum_bls_aggregates_and_pays_for_each_key() {
    use bls12_381::Scalar;

    let mut world = World::new();
    let lua = world.lua_code(LUA_BLS);
    let secrets = [Scalar::from(11u64), Scalar::from(13u64), Scalar::from(17u64)];
    let keys: Vec<[u8; 48]> = secrets.iter().map(ethlight::testing::public_key).collect();
    let message = [0x42; 32];
    let both = ethlight::testing::sign(&(secrets[0] + secrets[1]), &message);
    let all = ethlight::testing::sign(&(secrets[0] + secrets[1] + secrets[2]), &message);
    let check = |keys: &[u8], message: &[u8], signature: &[u8]| world.call(lua, "check", &[bytes(keys), bytes(message), bytes(signature)]);
    let yes = [TxCallArgumentType::Bool(true)];
    let no = [TxCallArgumentType::Bool(false)];

    // Keys back to back, or a table of them.
    assert_eq!(check(&keys.concat(), &message, &all).result.unwrap(), yes);
    assert_eq!(world.call(lua, "pair", &[bytes(&keys[0]), bytes(&keys[1]), bytes(&message), bytes(&both)]).result.unwrap(), yes);
    assert_eq!(check(&keys[..2].concat(), &message, &all).result.unwrap(), no, "a key short");
    assert_eq!(check(&keys.concat(), &[0x43; 32], &all).result.unwrap(), no, "another message");
    assert_eq!(check(&keys.concat()[1..], &message, &all).result.unwrap(), no, "not whole keys");
    assert_eq!(check(&keys.concat(), &message, &all[..95]).result.unwrap(), no, "a signature short");

    let mut infinity = [0u8; 96];
    infinity[0] = 0xc0;
    assert_eq!(check(&keys.concat(), &message, &infinity).result.unwrap(), no, "the signature at infinity");

    // Each key costs bls_key more, whether the check holds or not.
    let costs = crate::params::vm::HostCosts::DEFAULT;
    let three = check(&keys.concat(), &message, &all).fuel_used;
    let two = check(&keys[..2].concat(), &message, &all).fuel_used;
    assert!((costs.bls_key..costs.bls_key + 20).contains(&(three - two)), "{three} - {two}");
    assert!(two > costs.bls_verify + 2 * costs.bls_key);
}

#[cfg(not(feature = "bls12-381"))]
#[test]
fn without_bls12_381_the_check_fails_the_call() {
    let mut world = World::new();
    let lua = world.lua_code(LUA_BLS);
    let failed = world.call(lua, "check", &[bytes(&[0; 48]), bytes(&[0; 32]), bytes(&[0; 96])]).result.unwrap_err();

    assert!(failed.to_string().contains("bls12-381 is not in this build"), "{failed}");
}
