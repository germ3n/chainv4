use super::*;

fn call(signature: &str, values: &[AbiValue]) -> Vec<u8> {
    let (selector, types) = crate::abi::parse_signature(signature).unwrap();

    [selector.unwrap().to_vec(), encode_values(&types, values).unwrap()].concat()
}

fn pad(address: [u8; 20]) -> [u8; 32] {
    let mut topic = [0; 32];
    topic[12..].copy_from_slice(&address);

    topic
}

fn transfer_topic() -> [u8; 32] {
    crate::abi::signature_topic("Transfer(address,address,uint256)").unwrap()
}

/// An ABI of one function, as someone might paste one.
fn pasted(name: &str, inputs: &str) -> Abi {
    Abi::parse(&format!(r#"[{{"type": "function", "name": {name:?}, "stateMutability": "nonpayable", "inputs": [{inputs}], "outputs": []}}]"#)).unwrap()
}

#[test]
fn every_abi_shipped_is_built_in_and_every_file_of_them_reads() {
    let known = known_abis();

    assert_eq!(known.len(), BUILT_IN.len(), "every built-in ABI reads");
    assert_eq!(known.iter().map(|abi| abi.name).take(2).collect::<Vec<_>>(), ["ERC-20", "ERC-721"], "the standards first");

    // Everything in contracts/evm is here: a contract added there and forgotten here fails this.
    let shipped = std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/contracts/evm")).unwrap().filter(|file| file.as_ref().unwrap().path().extension().is_some_and(|extension| extension == "abi")).count();

    assert_eq!(BUILT_IN.len(), shipped + 2, "each of contracts/evm's ABIs, and the two standards");
}

#[test]
fn an_erc20_call_reads_as_one_and_says_a_match_is_all_it_is() {
    let known = known_abis();
    let input = call("transfer(address,uint256)", &[AbiValue::Address([0xab; 20]), AbiValue::Uint(u256::new(5))]);
    let read = read_call(None, &known, &input).unwrap();

    assert_eq!(read.by, ReadBy::As("ERC-20", None));
    assert_eq!(read.by.label(), "read as ERC-20");
    assert!(read.by.only_fits());
    assert_eq!(read.shown(), format!("transfer(to = {}, value = 5)", to_hex(&[0xab; 20])));
    assert_eq!(read.signature, "transfer(address,uint256)");
    assert!(read.exact);
    assert_eq!(read.args[0].pieces(), [Piece::Address([0xab; 20])], "an address stays one, for a link");
    assert_eq!(read.args[1].pieces(), [Piece::Amount(u256::new(5))], "an amount of the contract's token stays one, for its decimals");
}

#[test]
fn what_both_standards_have_says_so_and_only_the_token_s_own_amounts_are_amounts() {
    let known = known_abis();
    let input = call("transferFrom(address,address,uint256)", &[AbiValue::Address([1; 20]), AbiValue::Address([2; 20]), AbiValue::Uint(u256::new(7))]);

    assert_eq!(read_call(None, &known, &input).unwrap().by.label(), "read as ERC-20 or ERC-721", "the third argument is an amount in one and a token in the other");

    // A router's amounts are of other tokens, so they stay plain numbers.
    let swap = call("swapExactDEVForTokens(uint256,address[],uint256[],address,uint256)", &[AbiValue::Uint(u256::new(9)), AbiValue::Array(vec![AbiValue::Address([3; 20])]), AbiValue::Array(vec![AbiValue::Uint(u256::new(30))]), AbiValue::Address([4; 20]), AbiValue::Uint(u256::new(1))]);
    let read = read_call(None, &known, &swap).unwrap();

    assert_eq!(read.by.label(), "read as DEX router");
    assert_eq!(read.args[0].pieces(), [Piece::Text("9".to_string())]);
}

#[test]
fn an_abi_kept_for_the_contract_goes_first_and_says_so_differently() {
    let known = known_abis();
    let input = call("transfer(address,uint256)", &[AbiValue::Address([0xab; 20]), AbiValue::Uint(u256::new(5))]);
    let kept = pasted("transfer", r#"{"name": "payee", "type": "address"}, {"name": "cents", "type": "uint256"}"#);
    let read = read_call(Some(&kept), &known, &input).unwrap();

    assert_eq!(read.by, ReadBy::Kept);
    assert_eq!(read.by.label(), "read by the ABI kept for this contract");
    assert!(!read.by.only_fits());
    assert!(read.shown().contains("payee = ") && read.shown().contains("cents = 5"), "{}", read.shown());

    // One without that function leaves it to the built-in ones, and the label says which read it.
    let other = pasted("other", "");
    assert_eq!(read_call(Some(&other), &known, &input).unwrap().by, ReadBy::As("ERC-20", None));
}

#[test]
fn logs_read_by_their_shape_as_erc20_and_erc721_share_a_topic() {
    let known = known_abis();
    let value = u256::new(7).to_be_bytes();
    let fungible = [transfer_topic(), pad([1; 20]), pad([2; 20])];
    let read = read_log(None, &known, &fungible, &value).unwrap();

    assert_eq!(read.by, ReadBy::As("ERC-20", None));
    assert_eq!(read.shown(), format!("Transfer(from = {}, to = {}, value = 7)", to_hex(&[1; 20]), to_hex(&[2; 20])));
    assert_eq!(read.moves, [TokenMove { from: Some([1; 20]), to: Some([2; 20]), amount: u256::new(7), id: None, standard: "ERC-20" }]);

    let nft = [transfer_topic(), pad([1; 20]), pad([2; 20]), u256::new(9).to_be_bytes()];
    let read = read_log(None, &known, &nft, &[]).unwrap();

    assert_eq!(read.by, ReadBy::As("ERC-721", None));
    assert_eq!(read.moves, [TokenMove { from: Some([1; 20]), to: Some([2; 20]), amount: u256::ONE, id: Some(u256::new(9)), standard: "ERC-721" }]);

    // Four topics and an amount are neither.
    assert_eq!(read_log(None, &known, &nft, &value), None, "no event has that shape");
    assert!(read_log(None, &known, &[[9; 32]], &[]).is_none(), "nothing knows that topic");
    assert!(read_log(None, &known, &[], &[]).is_none(), "an anonymous log names nothing");
}

#[test]
fn token_moves_see_mints_and_burns_erc1155_and_nothing_else() {
    let value = u256::new(7).to_be_bytes();

    assert_eq!(token_moves(&[transfer_topic(), [0; 32], pad([2; 20])], &value)[0].from, None, "a mint comes from the zero address");
    assert_eq!(token_moves(&[transfer_topic(), pad([1; 20]), [0; 32]], &value)[0].to, None, "a burn goes to it");
    assert!(token_moves(&[transfer_topic(), [0xff; 32], pad([2; 20])], &value).is_empty(), "no address has bytes before its 20");
    assert!(token_moves(&[transfer_topic(), pad([1; 20]), pad([2; 20])], &[]).is_empty(), "no amount");
    assert!(token_moves(&[[1; 32]], &[]).is_empty());

    let single = crate::abi::signature_topic("TransferSingle(address,address,address,uint256,uint256)").unwrap();
    let data = [u256::new(4).to_be_bytes(), u256::new(50).to_be_bytes()].concat();

    assert_eq!(token_moves(&[single, pad([9; 20]), pad([1; 20]), pad([2; 20])], &data), [TokenMove { from: Some([1; 20]), to: Some([2; 20]), amount: u256::new(50), id: Some(u256::new(4)), standard: "ERC-1155" }]);

    let batch = crate::abi::signature_topic("TransferBatch(address,address,address,uint256[],uint256[])").unwrap();
    let numbers = |values: [u64; 2]| AbiValue::Array(values.iter().map(|value| AbiValue::Uint(u256::new(u128::from(*value)))).collect());
    let list = AbiType::Array(Box::new(AbiType::Uint(256)));
    let data = encode_values(&[list.clone(), list], &[numbers([1, 2]), numbers([10, 20])]).unwrap();
    let moved = token_moves(&[batch, pad([9; 20]), pad([1; 20]), pad([2; 20])], &data);

    assert_eq!(moved.iter().map(|moved| (moved.id.unwrap(), moved.amount)).collect::<Vec<_>>(), [(u256::new(1), u256::new(10)), (u256::new(2), u256::new(20))]);
}

#[test]
fn a_call_held_in_a_multisig_s_bytes_reads_too() {
    let known = known_abis();
    let inner = call("transfer(address,uint256)", &[AbiValue::Address([0xab; 20]), AbiValue::Uint(u256::new(5))]);
    let outer = call("execute(address,uint256,bytes,address[],bytes[])", &[AbiValue::Address([1; 20]), AbiValue::Uint(u256::ZERO), AbiValue::Bytes(inner), AbiValue::Array(vec![AbiValue::Address([2; 20])]), AbiValue::Array(vec![AbiValue::Bytes(vec![0xee; 65])])]);
    let read = read_call(None, &known, &outer).unwrap();

    assert_eq!(read.by.label(), "read as MultiSig.sol");

    let held = read.args.iter().find(|arg| arg.name == "data").and_then(|arg| arg.inner.as_ref()).expect("the data reads as a call");

    assert_eq!(held.name, "transfer");
    assert_eq!(held.by.label(), "read as ERC-20");
    assert!(read.args.iter().find(|arg| arg.name == "signatures").unwrap().inner.is_none(), "signatures are no call");
}

#[test]
fn bytes_past_what_a_call_reads_are_said_to_be_there() {
    let known = known_abis();
    let mut input = call("transfer(address,uint256)", &[AbiValue::Address([0xab; 20]), AbiValue::Uint(u256::new(5))]);

    assert!(read_call(None, &known, &input).unwrap().exact);
    input.extend([1, 2, 3]);
    assert!(!read_call(None, &known, &input).unwrap().exact, "something this does not show is in there");
}

#[test]
fn offsets_all_pointing_at_one_long_value_cannot_blow_up_what_reads_them() {
    // bytes[] of 2,000 elements, every offset at one 50 KB value: 100 MB, were it decoded.
    let kept = pasted("f", r#"{"name": "items", "type": "bytes[]"}"#);
    let selector = kept.functions[0].selector();
    let count = 2_000usize;
    let tail_at = (count * 32) as u64;
    let mut input = selector.to_vec();

    input.extend(u256::new(32).to_be_bytes());
    input.extend(u256::new(count as u128).to_be_bytes());

    for _ in 0..count {
        input.extend(u256::new(u128::from(tail_at)).to_be_bytes());
    }

    let long = 50_000usize;
    input.extend(u256::new(long as u128).to_be_bytes());
    input.extend(vec![0xcc; long]);

    assert!(input.len() < MAX_INPUT);
    assert!(read_call(Some(&kept), &[], &input).is_none(), "refused, not decoded");

    // Honest bytes of the same type read: a budget is not a limit on them.
    let honest = call("f(bytes[])", &[AbiValue::Array(vec![AbiValue::Bytes(vec![1; 40]); 300])]);
    assert_eq!(read_call(Some(&kept), &[], &honest).unwrap().args[0].pieces().len(), 1);

    let wrapped = vec![AbiValue::Tuple(vec![AbiValue::Uint(u256::ONE)]); 3_000];
    let tuples = pasted("g", r#"{"name": "items", "type": "tuple[]", "components": [{"name": "n", "type": "uint256"}]}"#);
    let input = [tuples.functions[0].selector().to_vec(), encode_values(&[AbiType::parse("(uint256)[]").unwrap()], &[AbiValue::Array(wrapped)]).unwrap()].concat();
    assert!(read_call(Some(&tuples), &[], &input).is_some(), "nor on many small tuples");

    // Too much to read at all is left to the raw facts.
    let too_long = [selector.to_vec(), vec![0; MAX_INPUT]].concat();
    assert!(read_call(Some(&kept), &[], &too_long).is_none());
}

#[test]
fn a_pasted_abi_with_a_fixed_array_too_big_to_measure_is_refused_at_parse() {
    // Once this type's element count times a word overflowed a pointer while it was measured for
    // the head advance, wrapping on a release node and panicking on an overflow-checked one; now
    // the length is capped at parse, so the ABI that names it does not read at all, on any build.
    let json = format!(r#"[{{"type": "function", "name": "h", "stateMutability": "nonpayable", "inputs": [{{"name": "x", "type": "uint256[{}]"}}], "outputs": []}}]"#, usize::MAX);

    assert!(Abi::parse(&json).is_err());
}

#[test]
fn what_is_read_cannot_break_a_line_or_disguise_itself() {
    let known = known_abis();
    let hostile = "To: 0xdead\nSends: 0\r\t\u{202e}evil\u{200b}\u{0301}";
    let input = call("pay(string)", &[AbiValue::String(hostile.to_string())]);
    let shown = read_call(None, &known, &input).unwrap().shown();

    assert!(!shown.chars().any(|c| c.is_control() || matches!(c, '\u{202e}' | '\u{200b}' | '\u{0301}')), "{shown}");
    assert!(shown.contains("To: 0xdead\\nSends: 0"), "kept, escaped: {shown}");
    assert!(shown.contains("\\u{202e}"), "{shown}");
    assert_eq!(clean_text("it's", 10), "it's", "a plain apostrophe stays");

    // Long strings, bytes and arrays say how much more there is.
    assert_eq!(clean_text(&"x".repeat(200), 5), "xxxxx…");

    let arg = |value: AbiValue| ReadArg { name: String::new(), ty: String::new(), value, amount: false, inner: None };
    let [Piece::Text(text)] = &arg(AbiValue::String("y".repeat(100_000))).pieces()[..] else { panic!() };
    assert!(text.chars().count() < MAX_TEXT + 4);

    let [Piece::Text(text)] = &arg(AbiValue::Bytes(vec![0xab; 500])).pieces()[..] else { panic!() };
    assert_eq!(text, &format!("0x{}… 500 bytes", "ab".repeat(LONG_BYTES_START)));

    let [Piece::Text(text)] = &arg(AbiValue::Array(vec![AbiValue::Uint(u256::ONE); 100_000])).pieces()[..] else { panic!() };
    assert_eq!(text, "[1, 1, 1, 1, 1, 1, 1, 1, … 99992 more]");

    let nested = AbiValue::Array(vec![AbiValue::Array(vec![AbiValue::Array(vec![AbiValue::Address([1; 20]); 8]); 8]); 8]);
    assert!(arg(nested).pieces().len() <= MAX_PIECES * 2 + 1, "however it nests");

    // A name in a pasted ABI is a name, whatever it says.
    let kept = pasted("Sends 0\nTo: 0xbeef", r#"{"name": "To: 0xbeef\n", "type": "uint256"}"#);
    let input = [kept.functions[0].selector().to_vec(), u256::new(1).to_be_bytes().to_vec()].concat();
    let read = read_call(Some(&kept), &[], &input).unwrap();

    assert_eq!(read.name, "Sends?0?To??0xbeef");
    assert_eq!(read.args[0].name, "To??0xbeef?");
    assert!(!read.signature.contains('\n'));
}

#[test]
fn a_reader_reads_each_thing_once_and_keeps_a_kept_abi_s_reading_apart() {
    let reader = AbiReader::default();
    let input = call("transfer(address,uint256)", &[AbiValue::Address([0xab; 20]), AbiValue::Uint(u256::new(5))]);
    let hex = to_hex(&input);
    let kept = r#"[{"type": "function", "name": "transfer", "stateMutability": "nonpayable", "inputs": [{"name": "payee", "type": "address"}, {"name": "cents", "type": "uint256"}], "outputs": []}]"#;

    let first = reader.call_hex(None, &hex).unwrap();
    let again = reader.call_hex(None, &hex).unwrap();

    assert!(Rc::ptr_eq(&first, &again), "read once");
    assert_eq!(reader.call(None, &input).unwrap().shown(), first.shown(), "bytes and their hex read alike");

    // With an ABI kept since, what it reads wins, under its own key.
    let by_kept = reader.call_hex(Some(kept), &hex).unwrap();

    assert_eq!(by_kept.by, ReadBy::Kept);
    assert_eq!(reader.call_hex(None, &hex).unwrap().by, ReadBy::As("ERC-20", None), "and without it, as before");
    assert!(Rc::ptr_eq(&reader.abi(kept).unwrap(), &reader.abi(kept).unwrap()), "an ABI is parsed once");
    assert!(reader.call_hex(Some("not an abi"), &hex).is_some(), "one that does not parse reads as none");
    assert_eq!(reader.call_hex(Some("   "), &hex).unwrap().by, ReadBy::As("ERC-20", None));
    assert!(reader.call_hex(None, "not hex").is_none());

    // Nothing to read is remembered too, as is a log.
    assert!(reader.call(None, &[0xde, 0xad, 0xbe, 0xef]).is_none());

    let topics = [to_hex(&transfer_topic()), to_hex(&pad([1; 20])), to_hex(&pad([2; 20]))];
    let topics: Vec<&str> = topics.iter().map(String::as_str).collect();
    let data = to_hex(&u256::new(7).to_be_bytes());
    let log = reader.log(None, &topics, &data).unwrap();

    assert!(Rc::ptr_eq(&log, &reader.log(None, &topics, &data).unwrap()));
    assert!(reader.log(None, &["0x01"], &data).is_none(), "a topic is 32 bytes");
}

#[test]
fn a_cache_that_holds_too_much_starts_over() {
    let mut cache = Cache::<u8>::default();
    let long = vec![1; MAX_HELD / 2];

    assert_eq!(cache.get_or(0, &long, || Some(1)).as_deref(), Some(&1));
    assert_eq!(cache.get_or(0, &long, || Some(2)).as_deref(), Some(&1), "held");

    let other = vec![2; MAX_HELD / 2 + 1];
    cache.get_or(0, &other, || Some(3));

    assert_eq!(cache.get_or(0, &long, || Some(4)).as_deref(), Some(&4), "gone, and read again");
}

#[test]
fn a_signature_gives_its_topic_however_it_is_written() {
    assert_eq!(to_hex(&transfer_topic()), "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef");
    assert_eq!(crate::abi::signature_topic(" Transfer(address, address, uint) ").unwrap(), transfer_topic());
    assert!(crate::abi::signature_topic("(address)").is_err(), "no name");
    assert!(crate::abi::signature_topic("Transfer").is_err());
}

#[test]
fn the_event_names_built_in_are_found() {
    let known = known_abis();

    assert_eq!(known_event(&known, "Transfer").unwrap().signature(), "Transfer(address,address,uint256)");
    assert_eq!(known_event(&known, "Sync").unwrap().signature(), "Sync(uint112,uint112)");
    assert!(known_event(&known, "Nothing").is_none());
}
