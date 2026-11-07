//! EVM collections in the collection registry: every contract the EVM creates, a transaction's own
//! or a factory's CREATE and CREATE2, asked by ERC-165 what it is once the transaction stands, and
//! listed with its name and symbol as native collections are.

use {
    super::*,
    crate::abi::{AbiType, AbiValue, encode_values},
    crate::registry::collections::{Collection, CollectionFunctions, CollectionKind, Collections},
};

const COLLECTIBLE_BIN: &str = include_str!("../../../../contracts/evm/Collectible.bin");
const ITEMS_BIN: &str = include_str!("../../../../contracts/evm/Items.bin");
const FACTORY_BIN: &str = include_str!("../../../../contracts/evm/CollectibleFactory.bin");

fn with_collections(db: &Arc<MemoryDb>) -> Chain {
    Chain::open(db.clone(), &Genesis { collections: Some(CollectionFunctions::standard()), ..genesis() }).unwrap()
}

fn create(chain: &mut Chain, init: Vec<u8>) -> Address {
    let deployer = authority();
    let at = evm::create_address(&deployer.public().address(), nonce(chain, &deployer));
    let deploy = sign(chain, &deployer, Tx::Deploy { code: Code::Evm(init) });
    assert_eq!(advance(chain, vec![deploy])[0].result, Ok(Effect::Deployed(at)));

    at
}

fn collectible(name: &str, symbol: &str) -> Vec<u8> {
    let names = encode_values(&[AbiType::String, AbiType::String], &[AbiValue::String(name.into()), AbiValue::String(symbol.into())]).unwrap();

    [crate::structs::hex::from_hex(COLLECTIBLE_BIN.trim()).unwrap(), names].concat()
}

fn listed(chain: &Chain, address: &Address) -> Option<Collection> {
    Collections::new(&chain.state()).get(address).unwrap()
}

/// What a call returned, as the address in its one word.
fn made(receipt: &Receipt) -> Address {
    let bytes = returned(receipt);

    Address::Evm(bytes[12..32].try_into().unwrap())
}

#[test]
fn an_erc_721_or_1155_the_evm_creates_is_listed_wherever_it_was_made() {
    let db = Arc::new(MemoryDb::default());
    let mut chain = with_collections(&db);

    // A transaction's own deploys: an ERC-721 with its name and symbol, an ERC-1155 without.
    let art = create(&mut chain, collectible("Art", "ART"));
    let items = create(&mut chain, crate::structs::hex::from_hex(ITEMS_BIN.trim()).unwrap());

    assert_eq!(listed(&chain, &art), Some(Collection { address: art, kind: CollectionKind::Nft, name: Some("Art".to_string()), symbol: Some("ART".to_string()) }));
    assert_eq!(listed(&chain, &items), Some(Collection { address: items, kind: CollectionKind::Multi, name: None, symbol: None }));

    // What says it is no collection is not one: the counter answers no ERC-165 at all.
    let counter = deploy(&mut chain, 1);

    assert_eq!(listed(&chain, &counter), None);

    // A factory's CREATE and CREATE2, inside a call.
    let factory = create(&mut chain, crate::structs::hex::from_hex(FACTORY_BIN.trim()).unwrap());
    let names = encode_values(&[AbiType::String, AbiType::String], &[AbiValue::String("Made".into()), AbiValue::String("MADE".into())]).unwrap();
    let make = call(&chain, &authority(), factory, 0, calldata("make(string,string)", &[names]));
    let by_create = made(&advance(&mut chain, vec![make])[0]);
    let make = call(&chain, &authority(), factory, 0, calldata("makeItems(bytes32)", &[word(7)]));
    let by_create2 = made(&advance(&mut chain, vec![make])[0]);

    assert_eq!(listed(&chain, &factory), None, "the factory is no collection itself");
    assert_eq!(listed(&chain, &by_create).map(|found| (found.kind, found.name, found.symbol)), Some((CollectionKind::Nft, Some("Made".to_string()), Some("MADE".to_string()))));
    assert_eq!(listed(&chain, &by_create2).map(|found| found.kind), Some(CollectionKind::Multi));

    // A name too long for the registry is left out; the collection is listed all the same.
    let long = create(&mut chain, collectible(&"x".repeat(65), "LONG"));

    assert_eq!(listed(&chain, &long).map(|found| (found.name, found.symbol)), Some((None, Some("LONG".to_string()))));
}

#[test]
fn a_chain_without_the_collection_registry_lists_no_evm_collection() {
    let db = Arc::new(MemoryDb::default());
    let mut chain = Chain::open(db.clone(), &Genesis { collections: None, ..genesis() }).unwrap();
    let art = create(&mut chain, collectible("Art", "ART"));

    assert_eq!(listed(&chain, &art), None);
}
