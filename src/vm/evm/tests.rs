use {super::*, crate::structs::hash::HashType};

#[test]
fn create_addresses_are_ethereums() {
    // Ethereum's own: the first contracts 0x6ac7…dbf0 creates.
    let sender = Address::Secp256k1(crate::structs::hex::from_hex("0x6ac7ea33f8831ea9dcc53393aaa88b25a785dbf0").unwrap().try_into().unwrap());

    assert_eq!(create_address(&sender, 0).to_string(), "evm:0xcd234a471b72ba2f1ccf0a70fcaba648a5eecd8d");
    assert_eq!(create_address(&sender, 1).to_string(), "evm:0x343c43a37d37dff08ae8c4a11544c718abb4fcf8");
}

#[test]
fn every_account_has_20_bytes_in_the_evm() {
    let ethereum = Address::Secp256k1([7; 20]);
    let contract = Address::Evm([8; 20]);
    let ed25519 = Address::Ed25519([9; 32]);
    let edblake = Address::EdBlake([9; 32]);

    assert_eq!(evm_alias(&ethereum), [7; 20], "secp256k1 accounts are themselves");
    assert_eq!(evm_alias(&contract), [8; 20]);
    assert_ne!(evm_alias(&ed25519), evm_alias(&edblake), "the kind is part of what is hashed");
    assert_eq!(evm_alias(&ed25519), revm::primitives::keccak256(address_key(&ed25519)).0[12..]);
}

#[test]
fn an_alias_reads_back_once_written_and_unknown_bytes_are_secp256k1() {
    let state = State::new(Arc::new(crate::db::MemoryDb::default()));
    let ed25519 = Address::Ed25519([9; 32]);
    let alias = evm_alias(&ed25519);

    assert_eq!(account_of(&state, alias).unwrap(), Address::Secp256k1(alias), "not yet written down");

    register_alias(&state, &ed25519);
    register_alias(&state, &Address::Secp256k1([7; 20]));

    assert_eq!(account_of(&state, alias).unwrap(), ed25519);
    assert_eq!(account_of(&state, [7; 20]).unwrap(), Address::Secp256k1([7; 20]));
}

#[test]
fn every_chain_has_an_id_metamask_takes() {
    let one = Hash::from_bytes(HashType::default_chain(), &[0xff; 32]).unwrap();
    let other = Hash::from_bytes(HashType::default_chain(), &[0x01; 32]).unwrap();

    assert!(chain_id(&one) < 1 << 48);
    assert_ne!(chain_id(&one), chain_id(&other));
    assert_eq!(chain_id(&Hash::zero(HashType::default_chain())), 1, "never 0, which EIP-155 leaves unprotected");
}

#[test]
fn reverts_say_why_as_solidity_wrote_it() {
    // Error("nothing to add"), as solc encodes require's message.
    let mut output = vec![0x08, 0xc3, 0x79, 0xa0];
    output.extend(U256::from(32).to_be_bytes::<32>());
    output.extend(U256::from(14).to_be_bytes::<32>());
    output.extend(b"nothing to add");
    output.extend([0; 18]);

    assert_eq!(revert_reason(&output), "nothing to add");
    assert_eq!(revert_reason(&[]), "without a reason");
    assert_eq!(revert_reason(&[0xde, 0xad]), "0xdead", "a custom error stays as its bytes");
}
