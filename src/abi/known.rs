//! EVM calldata and logs, read for a person. The ABIs of every contract chainv4 ships are built in,
//! with the standard ERC-20's and ERC-721's, so common things read with no setup; an ABI someone
//! pasted or saved for the contract always goes first.
//!
//! A match only says the bytes fit a signature, not that the contract is what it fits: any contract
//! can have a transfer(address,uint256). So what is read says which ABI read it, and what it read is
//! untrusted text. A string or a long array is cut short here, in one place, and control characters
//! come out escaped, so nothing read can break a line or look like what surrounds it.

use {
    super::{Abi, AbiEvent, AbiFunction, AbiParam, AbiType, AbiValue, decode_within, encode_values, keccak},
    crate::structs::hex::{from_hex, to_hex},
    crate::structs::u256::u256,
    r#macro::document,
    std::cell::{Cell, OnceCell, RefCell},
    std::collections::HashMap,
    std::rc::Rc,
};

/// What to say beside anything a built-in ABI read, where someone is about to sign it.
pub const NOT_PROOF: &str = "A match, not proof: the bytes fit this signature, whatever the contract really does.";

/// The most calldata, or log data, that is read: past this, the raw facts are all there is.
const MAX_INPUT: usize = 128 * 1024;
/// How many of an array's elements show, and how many pieces all of a value's parts come to.
const MAX_ELEMENTS: usize = 8;
const MAX_PIECES: usize = 64;
/// How many characters of a string show.
const MAX_TEXT: usize = 160;
/// How many bytes show whole, and of longer ones, how many at the start.
const MAX_BYTES: usize = 32;
const LONG_BYTES_START: usize = 16;
/// How long a name from an ABI may be.
const MAX_NAME: usize = 64;
/// How deep calls held in a bytes argument are read.
const MAX_NESTED: usize = 3;
/// The most a token's batch log counts as moves.
const MAX_BATCH: usize = 64;
/// How many ABIs pasted or saved are held, read.
const MAX_KEPT: usize = 256;
/// What the cache holds before it starts over, in bytes of what it is keyed by.
const MAX_HELD: usize = 16 << 20;
/// What the cache counts for an entry besides its key: what was read of it.
const ENTRY_COST: usize = 1 << 10;

/// Every ABI built in, standards first, by what it reads as. Where two fit, the first one says.
const BUILT_IN: [(&str, &str, bool); 20] = [
    ("ERC-20", include_str!("erc20.json"), true),
    ("ERC-721", include_str!("erc721.json"), true),
    ("ERC-1155 (Items)", include_str!("../../contracts/evm/Items.abi"), false),
    ("Token.sol", include_str!("../../contracts/evm/Token.abi"), false),
    ("Collectible", include_str!("../../contracts/evm/Collectible.abi"), false),
    ("CollectibleFactory", include_str!("../../contracts/evm/CollectibleFactory.abi"), false),
    ("WDEV", include_str!("../../contracts/evm/WDEV.abi"), false),
    ("WrappedToken", include_str!("../../contracts/evm/WrappedToken.abi"), false),
    ("WrappedNft", include_str!("../../contracts/evm/WrappedNft.abi"), false),
    ("WrappedMulti", include_str!("../../contracts/evm/WrappedMulti.abi"), false),
    ("Swap.sol", include_str!("../../contracts/evm/Swap.abi"), false),
    ("ChainV4Finality.sol", include_str!("../../contracts/evm/ChainV4Finality.abi"), false),
    ("DEX router", include_str!("../../contracts/evm/DexRouter.abi"), false),
    ("DEX factory", include_str!("../../contracts/evm/DexFactory.abi"), false),
    ("DEX pair", include_str!("../../contracts/evm/DexPair.abi"), false),
    ("Gateway.sol", include_str!("../../contracts/evm/Gateway.abi"), false),
    ("MultiSig.sol", include_str!("../../contracts/evm/MultiSig.abi"), false),
    ("Bridge", include_str!("../../contracts/evm/Bridge.abi"), false),
    ("NamePayer", include_str!("../../contracts/evm/NamePayer.abi"), false),
    ("Counter", include_str!("../../contracts/evm/Counter.abi"), false),
];

/// The functions and events whose amount is of the contract's own token, by a standard's name for
/// it, so it can go in the token's decimals.
const OWN_AMOUNT: [&str; 7] = ["transfer", "transferFrom", "approve", "mint", "burn", "Transfer", "Approval"];
const AMOUNT_NAMES: [&str; 5] = ["value", "amount", "wad", "_value", "_amount"];

#[document(summary = "An ABI built in: what a call or log it reads is said to read as, and the ABI.", category = "ABI", see_also = "Abi, known_abis")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnownAbi {
    #[document(summary = "What it reads as, such as ERC-20.")]
    pub name: &'static str,
    #[document(summary = "The ABI.")]
    pub abi: Abi,
    standard: bool,
}

#[document(
    summary = "Every ABI built in, read: the standard ERC-20 and ERC-721, then each contract chainv4 ships. Whoever holds this should keep it: reading them all takes a moment.",
    category = "ABI",
    see_also = "KnownAbi, AbiReader",
)]
pub fn known_abis() -> Vec<KnownAbi> {
    BUILT_IN
        .iter()
        .filter_map(|(name, json, standard)| Abi::parse(json).ok().map(|abi| KnownAbi { name, abi, standard: *standard }))
        .collect()
}

#[document(summary = "The event of this name among the ABIs built in, the first one's where several have it.", category = "ABI", see_also = "known_abis")]
pub fn known_event<'a>(known: &'a [KnownAbi], name: &str) -> Option<&'a AbiEvent> {
    known.iter().flat_map(|abi| &abi.abi.events).find(|event| event.name == name)
}

#[document(summary = "Which ABI read a call or a log: what is shown beside it.", category = "ABI", see_also = "ReadCall, ReadLog")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadBy {
    #[document(summary = "One pasted or saved for this contract, by someone who says it is its.")]
    Kept,
    #[document(summary = "One built in, which only fits: and the other standard it fits as well, as approve and transferFrom fit both ERC-20 and ERC-721.")]
    As(&'static str, Option<&'static str>),
}

impl ReadBy {
    #[document(parent = "ReadBy", summary = "Which ABI, in words: read as ERC-20, or read by the ABI kept for this contract.")]
    pub fn label(&self) -> String {
        match self {
            ReadBy::Kept => "read by the ABI kept for this contract".to_string(),
            ReadBy::As(name, None) => format!("read as {name}"),
            ReadBy::As(name, Some(other)) => format!("read as {name} or {other}"),
        }
    }

    #[document(parent = "ReadBy", summary = "Whether a built-in ABI read it, which is a match and no more.")]
    pub fn only_fits(&self) -> bool {
        matches!(self, ReadBy::As(..))
    }
}

#[document(summary = "A piece of a value to show: text to print as it is, or what an app shows its own way.", category = "ABI", see_also = "ReadArg")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Piece {
    #[document(summary = "Text, already cut short and escaped.")]
    Text(String),
    #[document(summary = "An address's 20 bytes.")]
    Address([u8; 20]),
    #[document(summary = "A whole amount of the contract's own token, for its decimals where they are known.")]
    Amount(u256),
}

#[document(summary = "An argument read from calldata or a log, typed.", category = "ABI", see_also = "ReadCall, ReadLog, Piece")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadArg {
    #[document(summary = "Its name, as letters and digits alone; empty when the ABI gives none.")]
    pub name: String,
    #[document(summary = "Its type.")]
    pub ty: String,
    #[document(summary = "Its value.")]
    pub value: AbiValue,
    #[document(summary = "Whether it is a whole amount of the contract's own token.")]
    pub amount: bool,
    #[document(summary = "The call its bytes hold, where they read as one.")]
    pub inner: Option<Box<ReadCall>>,
}

impl ReadArg {
    #[document(parent = "ReadArg", summary = "Its value in pieces, however long it is cut short: strings and bytes to their first part, arrays to their first few elements, each saying how much more there is.")]
    pub fn pieces(&self) -> Vec<Piece> {
        let (mut out, left) = (Vec::new(), Cell::new(MAX_PIECES));
        pieces_of(&self.value, self.amount, &left, &mut out);

        out
    }

    /// In a line: name = value.
    fn line(&self) -> String {
        let value = line_of(&self.pieces());

        match self.name.is_empty() {
            true => value,
            false => format!("{} = {value}", self.name),
        }
    }
}

/// What pieces say, in a line.
fn line_of(pieces: &[Piece]) -> String {
    pieces
        .iter()
        .map(|piece| match piece {
            Piece::Text(text) => text.clone(),
            Piece::Address(bytes) => to_hex(bytes),
            Piece::Amount(amount) => amount.to_string(),
        })
        .collect()
}

fn push_text(out: &mut Vec<Piece>, text: &str) {
    match out.last_mut() {
        Some(Piece::Text(last)) => last.push_str(text),
        _ => out.push(Piece::Text(text.to_string())),
    }
}

fn pieces_of(value: &AbiValue, amount: bool, left: &Cell<usize>, out: &mut Vec<Piece>) {
    if left.get() == 0 {
        return push_text(out, "…");
    }

    left.set(left.get() - 1);

    match value {
        AbiValue::Uint(number) if amount => out.push(Piece::Amount(*number)),
        AbiValue::Uint(number) => push_text(out, &number.to_string()),
        AbiValue::Int(number) => push_text(out, &number.to_string()),
        AbiValue::Bool(flag) => push_text(out, &flag.to_string()),
        AbiValue::Address(bytes) => out.push(Piece::Address(*bytes)),
        AbiValue::FixedBytes(bytes) | AbiValue::Bytes(bytes) => push_text(out, &clipped_bytes(bytes)),
        AbiValue::String(text) => push_text(out, &format!("\"{}\"", clean_text(text, MAX_TEXT))),
        AbiValue::Array(values) => list(values, "[", "]", left, out),
        AbiValue::Tuple(values) => list(values, "(", ")", left, out),
    }
}

fn list(values: &[AbiValue], open: &str, close: &str, left: &Cell<usize>, out: &mut Vec<Piece>) {
    push_text(out, open);

    for (at, value) in values.iter().take(MAX_ELEMENTS).enumerate() {
        if at > 0 {
            push_text(out, ", ");
        }

        pieces_of(value, false, left, out);
    }

    if values.len() > MAX_ELEMENTS {
        push_text(out, &format!(", … {} more", values.len() - MAX_ELEMENTS));
    }

    push_text(out, close);
}

/// Bytes, whole if they are few, and otherwise their start and how many there are.
fn clipped_bytes(bytes: &[u8]) -> String {
    match bytes.len() <= MAX_BYTES {
        true => to_hex(bytes),
        false => format!("{}… {} bytes", to_hex(&bytes[..LONG_BYTES_START]), bytes.len()),
    }
}

#[document(
    summary = "Untrusted text made safe to show beside anything else: its first characters, with control characters, line breaks, direction overrides, invisible ones and combining marks written out as escapes, so it cannot break a line or disguise itself, and an ellipsis if it was cut.",
    category = "ABI",
    params = { text = "The text.", limit = "How many of its characters to keep." },
)]
pub fn clean_text(text: &str, limit: usize) -> String {
    let mut shown = String::new();

    for character in text.chars().take(limit) {
        match character {
            '\'' => shown.push(character),
            character => shown.extend(character.escape_debug()),
        }
    }

    if text.chars().nth(limit).is_some() {
        shown.push('…');
    }

    shown
}

/// A name from an ABI, which anyone may have written: letters, digits, _ and $, and no more of it
/// than a name needs, so it cannot pass for anything but a name.
fn clean_name(name: &str) -> String {
    name.chars()
        .take(MAX_NAME)
        .map(|letter| match letter.is_ascii_alphanumeric() || letter == '_' || letter == '$' {
            true => letter,
            false => '?',
        })
        .collect()
}

#[document(summary = "A function's call, read from its calldata.", category = "ABI", see_also = "read_call, ReadBy")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadCall {
    #[document(summary = "Which ABI read it.")]
    pub by: ReadBy,
    #[document(summary = "The function's name, as letters and digits alone.")]
    pub name: String,
    #[document(summary = "Its signature, cut short and escaped.")]
    pub signature: String,
    #[document(summary = "Its arguments.")]
    pub args: Vec<ReadArg>,
    #[document(summary = "Whether the arguments, encoded again, are the calldata, byte for byte: false when bytes sit there that this does not show.")]
    pub exact: bool,
}

impl ReadCall {
    #[document(parent = "ReadCall", summary = "In a line, cut short: transfer(to = 0x…, value = 5).")]
    pub fn shown(&self) -> String {
        format!("{}({})", self.name, self.args.iter().map(ReadArg::line).collect::<Vec<_>>().join(", "))
    }
}

#[document(summary = "A token moving, as a log claims: who sent what to whom. A claim is all it is: anything can emit a Transfer.", category = "ABI", see_also = "token_moves")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TokenMove {
    #[document(summary = "Who sent it, or None for a mint, from the zero address.")]
    pub from: Option<[u8; 20]>,
    #[document(summary = "Who got it, or None for a burn, to the zero address.")]
    pub to: Option<[u8; 20]>,
    #[document(summary = "How much, whole: 1 for an ERC-721.")]
    pub amount: u256,
    #[document(summary = "Which token, for an ERC-721's or ERC-1155's.")]
    pub id: Option<u256>,
    #[document(summary = "Which standard its shape fits: ERC-20, ERC-721 or ERC-1155.")]
    pub standard: &'static str,
}

#[document(summary = "A log, read: its event's name and values, and any token it says moved.", category = "ABI", see_also = "read_log, ReadBy")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadLog {
    #[document(summary = "Which ABI read it.")]
    pub by: ReadBy,
    #[document(summary = "The event's name, as letters and digits alone.")]
    pub name: String,
    #[document(summary = "Its signature, cut short and escaped.")]
    pub signature: String,
    #[document(summary = "Its values.")]
    pub args: Vec<ReadArg>,
    #[document(summary = "The tokens it says moved.")]
    pub moves: Vec<TokenMove>,
}

impl ReadLog {
    #[document(parent = "ReadLog", summary = "In a line, cut short: Transfer(from = 0x…, to = 0x…, value = 5).")]
    pub fn shown(&self) -> String {
        format!("{}({})", self.name, self.args.iter().map(ReadArg::line).collect::<Vec<_>>().join(", "))
    }
}

/// What a static type takes in place, unless a pasted ABI makes that too big to count.
fn head_fits(ty: &AbiType) -> Option<usize> {
    match ty {
        _ if ty.is_dynamic() => Some(32),
        AbiType::FixedArray(inner, len) => head_fits(inner)?.checked_mul(*len),
        AbiType::Tuple(types) => types.iter().try_fold(0usize, |sum, ty| sum.checked_add(head_fits(ty)?)),
        _ => Some(32),
    }
}

/// What to spend on decoding these bytes: several times what honest bytes take, however they nest.
fn budget_for(data: &[u8]) -> Cell<usize> {
    Cell::new(data.len().saturating_mul(8).saturating_add(4096))
}

/// Whether a value goes in the decimals of the contract it was sent to or logged by.
fn own_amount(function: &str, param: &AbiParam) -> bool {
    param.ty == AbiType::Uint(256) && OWN_AMOUNT.contains(&function) && AMOUNT_NAMES.contains(&param.name.as_str())
}

fn read_args(function: &str, params: &[AbiParam], values: Vec<AbiValue>, known: &[KnownAbi], depth: usize) -> Vec<ReadArg> {
    params
        .iter()
        .zip(values)
        .map(|(param, value)| {
            let inner = match &value {
                AbiValue::Bytes(bytes) if depth < MAX_NESTED => read_call_at(None, known, bytes, depth + 1).filter(|call| call.exact).map(Box::new),
                _ => None,
            };

            ReadArg { name: clean_name(&param.name), ty: clean_text(&param.ty.to_string(), MAX_NAME), amount: own_amount(function, param), value, inner }
        })
        .collect()
}

fn read_function(function: &AbiFunction, input: &[u8], by: ReadBy, known: &[KnownAbi], depth: usize) -> Option<ReadCall> {
    let types: Vec<AbiType> = function.inputs.iter().map(|param| param.ty.clone()).collect();

    types.iter().try_fold(0usize, |sum, ty| sum.checked_add(head_fits(ty)?))?;

    let args = input.get(4..)?;
    let values = decode_within(&types, args, &budget_for(args)).ok()?;
    let exact = encode_values(&types, &values).is_ok_and(|again| again == args);

    Some(ReadCall {
        by,
        name: clean_name(&function.name),
        signature: clean_text(&function.signature(), MAX_TEXT),
        args: read_args(&function.name, &function.inputs, values, known, depth),
        exact,
    })
}

fn read_call_at(kept: Option<&Abi>, known: &[KnownAbi], input: &[u8], depth: usize) -> Option<ReadCall> {
    if !(4..=MAX_INPUT).contains(&input.len()) {
        return None;
    }

    if let Some(read) = kept.and_then(|abi| abi.function_for(input)).and_then(|function| read_function(function, input, ReadBy::Kept, known, depth)) {
        return Some(read);
    }

    for (at, entry) in known.iter().enumerate() {
        let Some(function) = entry.abi.function_for(input) else {
            continue;
        };

        // The other standard with this selector, whose arguments mean something else: an ERC-20's
        // transferFrom names an amount where an ERC-721's names a token.
        let other = known[at + 1..].iter().find(|later| entry.standard && later.standard && later.abi.function_for(input).is_some()).map(|later| later.name);

        if let Some(read) = read_function(function, input, ReadBy::As(entry.name, other), known, depth) {
            return Some(read);
        }
    }

    None
}

#[document(
    summary = "Reads calldata: by the ABI kept for the contract if it has the function and fits, and otherwise by the first built-in ABI that does. A bytes argument that is itself a call reads too, a few levels down, as a multisig's execute holds one.",
    category = "ABI",
    params = { kept = "The ABI pasted or saved for the contract called, if any.", known = "The ABIs built in.", input = "The calldata." },
    returns = "What it says, or None when nothing reads it, or it is too long to read.",
    see_also = "ReadCall, AbiReader",
)]
pub fn read_call(kept: Option<&Abi>, known: &[KnownAbi], input: &[u8]) -> Option<ReadCall> {
    read_call_at(kept, known, input, 0)
}

fn read_event(event: &AbiEvent, topics: &[[u8; 32]], data: &[u8], by: ReadBy) -> Option<ReadLog> {
    // Exactly as many topics as it has indexed values, as a contract emitting it writes: an ERC-20's
    // and an ERC-721's Transfer share a first topic, and differ in how many follow.
    if topics.len() != 1 + event.inputs.iter().filter(|param| param.indexed).count() {
        return None;
    }

    let types: Vec<AbiType> = event.inputs.iter().map(|param| param.ty.clone()).collect();

    types.iter().try_fold(0usize, |sum, ty| sum.checked_add(head_fits(ty)?))?;

    let values = event.decode_log_within(topics, data, &budget_for(data)).ok()?;

    // And exactly its data: what a contract emitting it writes has nothing else in it, so a log with
    // more is some other event's, whatever its first topic says.
    let (unindexed_types, unindexed): (Vec<AbiType>, Vec<AbiValue>) = event.inputs.iter().zip(&values).filter(|(param, _)| !param.indexed).map(|(param, value)| (param.ty.clone(), value.clone())).unzip();

    if encode_values(&unindexed_types, &unindexed).ok()? != data {
        return None;
    }

    Some(ReadLog {
        by,
        name: clean_name(&event.name),
        signature: clean_text(&event.signature(), MAX_TEXT),
        args: read_args(&event.name, &event.inputs, values, &[], MAX_NESTED),
        moves: token_moves(topics, data),
    })
}

#[document(
    summary = "Reads a log: by the ABI kept for the contract if it has the event and fits, and otherwise by the first built-in ABI that does. A log that leaves out its event's topic reads by nothing.",
    category = "ABI",
    params = { kept = "The ABI pasted or saved for the contract that emitted it, if any.", known = "The ABIs built in.", topics = "Its topics.", data = "Its data." },
    returns = "What it says, or None when nothing reads it, or it is too long to read.",
    see_also = "ReadLog, AbiReader",
)]
pub fn read_log(kept: Option<&Abi>, known: &[KnownAbi], topics: &[[u8; 32]], data: &[u8]) -> Option<ReadLog> {
    let first = topics.first()?;

    if data.len() > MAX_INPUT {
        return None;
    }

    let events = |abi: &'_ Abi, by: ReadBy| -> Option<ReadLog> {
        abi.events.iter().filter(|event| !event.anonymous && event.topic() == *first).find_map(|event| read_event(event, topics, data, by))
    };

    kept.and_then(|abi| events(abi, ReadBy::Kept)).or_else(|| known.iter().find_map(|entry| events(&entry.abi, ReadBy::As(entry.name, None))))
}

/// An address in a topic's last 20 bytes, or None for the zero address; and refused if anything
/// sits before it.
fn party(word: &[u8; 32]) -> Result<Option<[u8; 20]>, ()> {
    let (zeros, address) = word.split_at(12);

    if zeros.iter().any(|byte| *byte != 0) {
        return Err(());
    }

    let address: [u8; 20] = address.try_into().map_err(|_| ())?;

    Ok(address.iter().any(|byte| *byte != 0).then_some(address))
}

#[document(
    summary = "The tokens a log says moved, by its shape alone: the Transfer of an ERC-20, of an ERC-721, and an ERC-1155's TransferSingle and TransferBatch. What it says is a claim: anything can emit one, so it never goes into a balance, which only the chain's own state can say.",
    category = "ABI",
    returns = "The moves; none for any other log, or one with bytes where a standard has none.",
    see_also = "TokenMove",
)]
pub fn token_moves(topics: &[[u8; 32]], data: &[u8]) -> Vec<TokenMove> {
    let word = |at: usize| data.get(at..at + 32).and_then(|word| <[u8; 32]>::try_from(word).ok()).map(u256::from_be_bytes);
    let Some(first) = topics.first() else {
        return Vec::new();
    };
    let is = |signature: &[u8]| *first == keccak(signature);

    let moved = || -> Option<Vec<TokenMove>> {
        if is(b"Transfer(address,address,uint256)") {
            let (from, to) = (party(topics.get(1)?).ok()?, party(topics.get(2)?).ok()?);

            return match (topics.len(), data.len()) {
                (3, 32) => Some(vec![TokenMove { from, to, amount: word(0)?, id: None, standard: "ERC-20" }]),
                (4, 0) => Some(vec![TokenMove { from, to, amount: u256::ONE, id: Some(u256::from_be_bytes(topics[3])), standard: "ERC-721" }]),
                _ => None,
            };
        }

        if topics.len() != 4 {
            return None;
        }

        let (from, to) = (party(&topics[2]).ok()?, party(&topics[3]).ok()?);

        if is(b"TransferSingle(address,address,address,uint256,uint256)") && data.len() == 64 {
            return Some(vec![TokenMove { from, to, amount: word(32)?, id: Some(word(0)?), standard: "ERC-1155" }]);
        }

        if is(b"TransferBatch(address,address,address,uint256[],uint256[])") {
            let types = [AbiType::Array(Box::new(AbiType::Uint(256))), AbiType::Array(Box::new(AbiType::Uint(256)))];
            let values = decode_within(&types, data, &budget_for(data)).ok()?;
            let ([AbiValue::Array(ids), AbiValue::Array(amounts)], true) = (values.as_slice(), data.len() <= MAX_INPUT) else {
                return None;
            };

            if ids.len() != amounts.len() {
                return None;
            }

            return ids
                .iter()
                .zip(amounts)
                .take(MAX_BATCH)
                .map(|pair| match pair {
                    (AbiValue::Uint(id), AbiValue::Uint(amount)) => Some(TokenMove { from, to, amount: *amount, id: Some(*id), standard: "ERC-1155" }),
                    _ => None,
                })
                .collect();
        }

        None
    };

    moved().unwrap_or_default()
}

/// What was read, by what it was read by and the bytes it was read from. It starts over once it
/// holds too much, as nothing here is more than a moment's work to read again.
struct Cache<T> {
    entries: HashMap<u64, HashMap<Vec<u8>, Option<Rc<T>>>>,
    held: usize,
}

impl<T> Default for Cache<T> {
    fn default() -> Cache<T> {
        Cache { entries: HashMap::new(), held: 0 }
    }
}

impl<T> Cache<T> {
    fn get_or(&mut self, scope: u64, key: &[u8], read: impl FnOnce() -> Option<T>) -> Option<Rc<T>> {
        if let Some(found) = self.entries.get(&scope).and_then(|entries| entries.get(key)) {
            return found.clone();
        }

        let made = read().map(Rc::new);

        if self.held + key.len() + ENTRY_COST > MAX_HELD {
            self.entries.clear();
            self.held = 0;
        }

        self.held += key.len() + ENTRY_COST;
        self.entries.entry(scope).or_default().insert(key.to_vec(), made.clone());

        made
    }
}

#[document(
    summary = "Reads calldata and logs for a page that draws every frame, reading each only once: the ABIs built in on first need, those pasted or saved when first seen, and everything they read. An app holds one for as long as it runs. Takes hex as the node's JSON gives it, or bytes.",
    category = "ABI",
    see_also = "read_call, read_log, known_abis",
)]
#[derive(Default)]
pub struct AbiReader {
    known: OnceCell<Vec<KnownAbi>>,
    /// The ABIs seen, by their text, each with a number the cache knows it by.
    kept: RefCell<HashMap<String, KeptAbi>>,
    last_kept: Cell<u64>,
    calls: RefCell<Cache<ReadCall>>,
    logs: RefCell<Cache<ReadLog>>,
}

/// An ABI seen: the number the cache knows it by, and the ABI, if its text was one.
type KeptAbi = (u64, Option<Rc<Abi>>);

/// Cache scopes: what is keyed by bytes and what by their hex stay apart.
const BY_BYTES: u64 = 0;
const BY_HEX: u64 = 1;

impl AbiReader {
    #[document(parent = "AbiReader", summary = "The ABIs built in.")]
    pub fn known(&self) -> &[KnownAbi] {
        self.known.get_or_init(known_abis)
    }

    #[document(parent = "AbiReader", summary = "An ABI's JSON, read, once; None for text that is not one.")]
    pub fn abi(&self, text: &str) -> Option<Rc<Abi>> {
        self.kept_abi(Some(text)).1
    }

    /// The number the cache knows an ABI's text by, 0 for none, and the ABI.
    fn kept_abi(&self, text: Option<&str>) -> KeptAbi {
        let Some(text) = text.filter(|text| !text.trim().is_empty()) else {
            return (0, None);
        };
        let mut kept = self.kept.borrow_mut();

        if let Some(found) = kept.get(text) {
            return found.clone();
        }

        // An ABI being typed into a field is a new one at every key: only the latest few are kept.
        // Numbers are never used twice, so nothing read by one forgotten is taken for another's.
        if kept.len() >= MAX_KEPT {
            kept.clear();
        }

        self.last_kept.set(self.last_kept.get() + 1);

        let found = (self.last_kept.get(), Abi::parse(text).ok().map(Rc::new));
        kept.insert(text.to_string(), found.clone());

        found
    }

    #[document(parent = "AbiReader", summary = "Calldata, read, as read_call does; the same bytes are only read once.")]
    pub fn call(&self, kept: Option<&str>, input: &[u8]) -> Option<Rc<ReadCall>> {
        let (id, abi) = self.kept_abi(kept);

        self.calls.borrow_mut().get_or(id * 2 + BY_BYTES, input, || read_call(abi.as_deref(), self.known(), input))
    }

    #[document(parent = "AbiReader", summary = "Calldata in hex, read, as read_call does; the same text is only read once.")]
    pub fn call_hex(&self, kept: Option<&str>, input: &str) -> Option<Rc<ReadCall>> {
        let (id, abi) = self.kept_abi(kept);

        self.calls.borrow_mut().get_or(id * 2 + BY_HEX, input.as_bytes(), || read_call(abi.as_deref(), self.known(), &from_hex(input)?))
    }

    #[document(parent = "AbiReader", summary = "A log in hex, read, as read_log does; the same text is only read once.")]
    pub fn log(&self, kept: Option<&str>, topics: &[&str], data: &str) -> Option<Rc<ReadLog>> {
        let (id, abi) = self.kept_abi(kept);
        let mut key = String::with_capacity(topics.iter().map(|topic| topic.len() + 1).sum::<usize>() + data.len());

        for topic in topics {
            key.push_str(topic);
            key.push(',');
        }

        key.push_str(data);

        self.logs.borrow_mut().get_or(id, key.as_bytes(), || {
            let topics: Vec<[u8; 32]> = topics.iter().map(|topic| from_hex(topic)?.try_into().ok()).collect::<Option<_>>()?;

            read_log(abi.as_deref(), self.known(), &topics, &from_hex(data)?)
        })
    }
}

#[cfg(test)]
mod tests;
