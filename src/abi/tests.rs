use super::*;

fn ty(text: &str) -> AbiType {
    AbiType::parse(text).unwrap()
}

fn hex(text: &str) -> Vec<u8> {
    from_hex(&text.split_whitespace().collect::<String>()).unwrap()
}

#[test]
fn solidity_docs_dynamic_example_encodes_byte_for_byte() {
    // Solidity's documentation, "Use of Dynamic Types": f(uint256,uint32[],bytes10,bytes) with
    // (0x123, [0x456, 0x789], "1234567890", "Hello, world!").
    let function = AbiFunction {
        name: "f".to_string(),
        inputs: ["uint256", "uint32[]", "bytes10", "bytes"].iter().map(|text| AbiParam { name: String::new(), ty: ty(text), indexed: false }).collect(),
        outputs: Vec::new(),
        reads: false,
        payable: false,
    };
    let values = vec![
        AbiValue::Uint(u256::from(0x123u64)),
        AbiValue::Array(vec![AbiValue::Uint(u256::from(0x456u64)), AbiValue::Uint(u256::from(0x789u64))]),
        AbiValue::FixedBytes(b"1234567890".to_vec()),
        AbiValue::Bytes(b"Hello, world!".to_vec()),
    ];
    let expected = hex(
        "0x8be65246
         0000000000000000000000000000000000000000000000000000000000000123
         0000000000000000000000000000000000000000000000000000000000000080
         3132333435363738393000000000000000000000000000000000000000000000
         00000000000000000000000000000000000000000000000000000000000000e0
         0000000000000000000000000000000000000000000000000000000000000002
         0000000000000000000000000000000000000000000000000000000000000456
         0000000000000000000000000000000000000000000000000000000000000789
         000000000000000000000000000000000000000000000000000000000000000d
         48656c6c6f2c20776f726c642100000000000000000000000000000000000000",
    );
    let calldata = function.encode_call(&values).unwrap();

    assert_eq!(to_hex(&calldata), to_hex(&expected));
    assert_eq!(function.decode_input(&calldata).unwrap(), values, "and read back");
}

#[test]
fn types_read_and_write_as_solidity_names_them() {
    for text in ["uint256", "int8", "address", "bool", "bytes32", "bytes", "string", "uint256[]", "address[3]", "(uint256,string)", "(address,(uint8,bytes)[])[2][]"] {
        assert_eq!(ty(text).to_string(), text);
    }

    assert_eq!(ty("uint"), AbiType::Uint(256), "uint alone is 256 bits");
    assert!(AbiType::parse("uint7").is_err());
    assert!(AbiType::parse("bytes33").is_err());
    assert!(AbiType::parse("mapping").is_err());
    assert!(ty("(uint256,string)").is_dynamic());
    assert!(!ty("(uint256,address)[2]").is_dynamic());
    assert_eq!(ty("(uint256,address)[2]").head_size(), 128, "static tuples sit in place");
}

#[test]
fn nested_dynamic_values_round_trip() {
    let types = vec![ty("(address,string)[]"), ty("uint8[2][]"), ty("int256"), ty("bool")];
    let values = vec![
        AbiValue::Array(vec![
            AbiValue::Tuple(vec![AbiValue::Address([1; 20]), AbiValue::String("one".into())]),
            AbiValue::Tuple(vec![AbiValue::Address([2; 20]), AbiValue::String("a longer string than a word holds, by a little".into())]),
        ]),
        AbiValue::Array(vec![AbiValue::Array(vec![AbiValue::Uint(u256::ONE), AbiValue::Uint(u256::from(2u64))])]),
        AbiValue::Int(i256::from(-5)),
        AbiValue::Bool(true),
    ];
    let encoded = encode_values(&types, &values).unwrap();
    // What ethers' AbiCoder writes for the same values, as a second, independent reading.
    let ethers = "0x00000000000000000000000000000000000000000000000000000000000000800000000000000000000000000000000000000000000000000000000000000200fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffb00000000000000000000000000000000000000000000000000000000000000010000000000000000000000000000000000000000000000000000000000000002000000000000000000000000000000000000000000000000000000000000004000000000000000000000000000000000000000000000000000000000000000c00000000000000000000000000101010101010101010101010101010101010101000000000000000000000000000000000000000000000000000000000000004000000000000000000000000000000000000000000000000000000000000000036f6e65000000000000000000000000000000000000000000000000000000000000000000000000000000000002020202020202020202020202020202020202020000000000000000000000000000000000000000000000000000000000000040000000000000000000000000000000000000000000000000000000000000002e61206c6f6e67657220737472696e67207468616e206120776f726420686f6c64732c2062792061206c6974746c65000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000100000000000000000000000000000000000000000000000000000000000000010000000000000000000000000000000000000000000000000000000000000002";

    assert_eq!(to_hex(&encoded), ethers);
    assert_eq!(decode_values(&types, &encoded).unwrap(), values);
    assert_eq!(encoded.len() % 32, 0);
}

#[test]
fn what_does_not_fit_or_read_is_refused() {
    assert!(encode_values(&[ty("uint8")], &[AbiValue::Uint(u256::from(256u64))]).is_err());
    assert!(encode_values(&[ty("int8")], &[AbiValue::Int(i256::from(128))]).is_err());
    assert!(encode_values(&[ty("bytes2")], &[AbiValue::FixedBytes(vec![1])]).is_err());
    assert!(encode_values(&[ty("uint256")], &[AbiValue::Bool(true)]).is_err());
    assert!(decode_values(&[ty("uint256")], &[0; 31]).is_err(), "too short");
    assert!(decode_values(&[ty("bool")], &u256::from(2u64).to_be_bytes()).is_err());
    assert!(decode_values(&[ty("address")], &[0xff; 32]).is_err(), "bytes before an address's 20");

    // A length claiming more than is there is refused before anything is made of it.
    let mut huge = u256::from(32u64).to_be_bytes().to_vec();
    huge.extend(u256::from(u64::MAX).to_be_bytes());

    assert!(decode_values(&[ty("uint256[]")], &huge).is_err());
    assert!(decode_values(&[ty("bytes")], &huge).is_err());
}

#[test]
fn typed_values_read_from_what_a_person_types() {
    assert_eq!(parse_value(&ty("uint256"), "0x10").unwrap(), AbiValue::Uint(u256::from(16u64)));
    assert_eq!(parse_value(&ty("int16"), "-300").unwrap(), AbiValue::Int(i256::from(-300)));
    assert_eq!(parse_value(&ty("bool"), "true").unwrap(), AbiValue::Bool(true));
    assert_eq!(parse_value(&ty("string"), "hi there").unwrap(), AbiValue::String("hi there".into()));
    assert_eq!(parse_value(&ty("address"), &format!("0x{}", "ab".repeat(20))).unwrap(), AbiValue::Address([0xab; 20]));
    assert_eq!(parse_value(&ty("address"), &format!("secp256k1:0x{}", "cd".repeat(20))).unwrap(), AbiValue::Address([0xcd; 20]));
    assert_eq!(
        parse_value(&ty("address"), &format!("ed25519:0x{}", "11".repeat(32))).unwrap(),
        AbiValue::Address(evm_address(&Address::Ed25519([0x11; 32]))),
        "any chainv4 address, at its EVM address"
    );
    assert_eq!(parse_value(&ty("uint8[]"), "[1, \"0x02\"]").unwrap(), AbiValue::Array(vec![AbiValue::Uint(u256::ONE), AbiValue::Uint(u256::from(2u64))]));
    assert_eq!(parse_value(&ty("(bool,string)"), "[false, \"x\"]").unwrap(), AbiValue::Tuple(vec![AbiValue::Bool(false), AbiValue::String("x".into())]));
    assert!(parse_value(&ty("uint8"), "256").is_err());
    assert!(parse_value(&ty("uint8[2]"), "[1]").is_err());
    assert!(parse_value(&ty("bytes4"), "0x01").is_err());
}

#[test]
fn a_json_abi_names_functions_events_errors_and_its_constructor() {
    let abi = Abi::parse(include_str!("../../contracts/evm/Counter.abi")).unwrap();
    let bump = abi.functions.iter().find(|function| function.name == "bump").unwrap();
    let count = abi.functions.iter().find(|function| function.name == "count").unwrap();

    assert_eq!(bump.signature(), "bump(uint256)");
    assert_eq!(bump.selector(), [0xb2, 0x0e, 0xb4, 0xc4]);
    assert!(count.reads && !bump.reads);
    assert!(abi.functions.iter().find(|function| function.name == "deposit").unwrap().payable);
    assert_eq!(abi.constructor.len(), 1);
    assert_eq!(abi.function_for(&bump.encode_call(&[AbiValue::Uint(u256::ONE)]).unwrap()).map(|function| function.name.as_str()), Some("bump"));
    assert_eq!(count.decode_output(&u256::from(8u64).to_be_bytes()).unwrap(), vec![AbiValue::Uint(u256::from(8u64))]);

    // Bumped(address indexed by, uint256 count): by from its topic, count from the data.
    let bumped = &abi.events[0];
    let by = [[0; 12].as_slice(), &[7; 20]].concat().try_into().unwrap();
    let topics = [bumped.topic(), by];

    assert_eq!(bumped.signature(), "Bumped(address,uint256)");
    assert_eq!(abi.event_for(&topics).map(|event| event.name.as_str()), Some("Bumped"));
    assert_eq!(
        bumped.show(&bumped.decode_log(&topics, &u256::from(8u64).to_be_bytes()).unwrap()),
        format!("Bumped(by = 0x{}, count = 8)", "07".repeat(20))
    );

    // Foundry's artifacts hold the list under abi; a tuple's type comes from its components.
    let foundry = r#"{"abi": [
        {"type": "function", "name": "set", "stateMutability": "nonpayable",
         "inputs": [{"name": "p", "type": "tuple[]", "components": [{"name": "a", "type": "address"}, {"name": "n", "type": "uint96"}]}],
         "outputs": []},
        {"type": "error", "name": "TooLow", "inputs": [{"name": "wanted", "type": "uint256"}]}
    ]}"#;
    let abi = Abi::parse(foundry).unwrap();

    assert_eq!(abi.functions[0].signature(), "set((address,uint96)[])");

    // A revert: Error(string), and the ABI's own error.
    let error_string = [vec![0x08, 0xc3, 0x79, 0xa0], encode_values(&[AbiType::String], &[AbiValue::String("nothing to add".into())]).unwrap()].concat();
    let too_low = [keccak(b"TooLow(uint256)")[..4].to_vec(), u256::from(5u64).to_be_bytes().to_vec()].concat();

    assert_eq!(abi.revert_reason(&error_string).as_deref(), Some("nothing to add"));
    assert_eq!(abi.revert_reason(&too_low).as_deref(), Some("TooLow(wanted = 5)"));
    assert!(Abi::parse("{}").is_err());
}

#[test]
fn create_addresses_and_aliases_are_ethereums() {
    // Ethereum's own: 0x6ac7…dbf0 makes 0xcd23…3e2d at nonce 0 and 0x343c…f6a3 at nonce 1.
    let deployer: [u8; 20] = from_hex("0x6ac7ea33f8831ea9dcc53393aaa88b25a785dbf0").unwrap().try_into().unwrap();

    assert_eq!(to_hex(&evm_create_address(deployer, 0)), "0xcd234a471b72ba2f1ccf0a70fcaba648a5eecd8d");
    assert_eq!(to_hex(&evm_create_address(deployer, 1)), "0x343c43a37d37dff08ae8c4a11544c718abb4fcf8");
    assert_eq!(evm_address(&Address::Secp256k1([3; 20])), [3; 20]);
    assert_eq!(evm_address(&Address::Evm([4; 20])), [4; 20]);
    assert_ne!(evm_address(&Address::Ed25519([5; 32])), [5; 20], "anything else goes by its alias");
}
