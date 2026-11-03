//! A batch's steps in words, for every screen someone signs one on: the Send page's review, the
//! offline signer, and a multisig's bundle and proposals. All of them lay out the same thing, so a
//! step reads the same wherever it is checked.
//!
//! What a batch says comes from whoever made it, so nothing of it is trusted to behave: every piece
//! of text is cut short and has anything that could lay out or hide text written as an escape, and
//! it only ever fills a field of a row it cannot leave. The count of steps and the coin they send
//! in all come first, worked out here, whatever the steps go on to say.

use {
    crate::airgap::Shown,
    crate::format,
    crate::history::function,
    chainv4::abi::{evm_address, evm_create_address},
    chainv4::crypto::hasher::Hasher,
    chainv4::registry::collections::CollectionKind,
    chainv4::structs::address::{Address, contract_address},
    chainv4::structs::code::Code,
    chainv4::structs::hash::{Hash, HashType},
    chainv4::structs::hex::to_hex,
    chainv4::structs::tx::{Tx, TxCallArgumentType, batch_deploy_nonce},
    chainv4::structs::u256::u256,
};

/// The most of any one piece of text a row shows, in characters. What is signed is whole, under
/// Everything it holds.
const MAX_TEXT: usize = 160;

/// Text from a transaction as a row may show it: on one line, with controls, invisible marks and
/// anything that reorders text written as escapes, and cut short.
pub fn plain(text: &str) -> String {
    let mut chars = text.chars();
    let shown: String = chars.by_ref().take(MAX_TEXT).flat_map(char::escape_debug).collect();

    // escape_debug writes a quote as an escape too, which says nothing wrong and reads badly.
    let shown = shown.replace("\\\"", "\"").replace("\\'", "'");

    match chars.next() {
        Some(_) => format!("{shown}…"),
        None => shown,
    }
}

/// What a row of a step says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Said {
    /// An address, to show as addresses show: cut short, and copied whole.
    Address(String),
    /// Anything else, already plain.
    Text(String),
}

/// One row of a step: what it is, and what it says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub label: String,
    pub said: Said,
}

impl Row {
    fn address(label: &str, address: &Address) -> Row {
        Row { label: label.to_string(), said: Said::Address(address.to_string()) }
    }

    fn text(label: &str, text: impl Into<String>) -> Row {
        Row { label: label.to_string(), said: Said::Text(text.into()) }
    }
}

/// An EVM step's calldata, for what reads it by name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvmData {
    pub to: String,
    pub input: Vec<u8>,
}

/// One step, laid out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    /// What it does, in a few words.
    pub kind: String,
    /// What it sends of the coin, in the coin's units and in the raw units signed.
    pub coin: Option<(String, String)>,
    pub rows: Vec<Row>,
    pub evm: Option<EvmData>,
    /// Whether the chain takes it in a batch at all: a batch holding one that it does not is
    /// refused whole, and no screen signs it.
    pub allowed: bool,
}

/// A batch, laid out: how many steps, what they send, and each of them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Steps {
    pub count: usize,
    /// All the steps send of the coin, in its units, and in the raw units signed. Not its fee.
    pub total: (String, String),
    /// All they send of the coin, as a number: what a balance is checked for.
    pub coin: u256,
    /// Whether a step's name burns coin the chain prices, which the steps do not say.
    pub burns_names: bool,
    /// Whether a step runs a contract, which may move more than the steps say.
    pub runs_code: bool,
    pub steps: Vec<Step>,
}

/// What the wallet knows a contract to be, from the chain's registries as the light client proved
/// them: a token, a collection, or neither. A call's numbers mean what the contract makes of them,
/// so a transfer's is an amount only of a token, an id of an NFT collection, and nothing the
/// wallet can name of a contract it does not know.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Known {
    Token(Shown),
    /// A collection of either standard, by its name, symbol or address, short.
    Collection { kind: CollectionKind, title: String },
    Unknown,
}

/// What a request or a page knows of where it will land, which says where its deploys go: its
/// sender and nonce, and the chain's hash. A signer offline knows the first two and no hash.
#[derive(Clone, Copy, Debug)]
pub struct Landing {
    pub sender: Address,
    pub nonce: u64,
    pub hash: HashType,
}

/// Where a deploy lands, by how many deploys came before it in its batch.
pub fn lands_at(code: &Code, landing: &Landing, before: u32) -> Address {
    let nonce = batch_deploy_nonce(landing.nonce, before);

    match code {
        Code::Evm(_) => Address::Evm(evm_create_address(evm_address(&landing.sender), nonce)),
        _ => contract_address(&landing.sender, nonce, landing.hash),
    }
}

/// A batch's steps in words. Contracts are read by what known says of them: a token's transfer in
/// its units, a collection's as the NFT or the amount of an id it moves, and the same call to a
/// contract known as neither as just that, its numbers unnamed.
pub fn of(steps: &[Tx], coin: &Shown, known: &dyn Fn(&Address) -> Known, landing: Option<&Landing>) -> Steps {
    // On the offline signer the coin's name is the sender's word, like anything else it says.
    let coin = &Shown { symbol: plain(&coin.symbol), decimals: coin.decimals };
    let mut total = u256::ZERO;
    let mut deploys = 0;
    let mut laid = Vec::with_capacity(steps.len());

    for tx in steps {
        let lands = match (tx, landing) {
            (Tx::Deploy { code }, Some(landing)) => Some(lands_at(code, landing, deploys)),
            _ => None,
        };
        let sends = sent(tx);
        deploys += u32::from(matches!(tx, Tx::Deploy { .. }));
        total = total.saturating_add(sends);
        laid.push(step(tx, coin, known, lands));
    }

    Steps {
        count: steps.len(),
        total: (format::amount(&total.to_string(), coin.decimals, &coin.symbol), total.to_string()),
        coin: total,
        burns_names: steps.iter().any(|tx| matches!(tx, Tx::Name { action: chainv4::registry::names::NameAction::Register { .. } | chainv4::registry::names::NameAction::Renew { .. }, .. })),
        runs_code: steps.iter().any(|tx| matches!(tx, Tx::ContractCall { .. } | Tx::EvmCall { .. } | Tx::Deploy { .. } | Tx::Upgrade { .. })),
        steps: laid,
    }
}

/// What a step sends of the coin, as the transaction itself says it.
pub fn sent(tx: &Tx) -> u256 {
    match tx {
        Tx::Transfer { amount, .. } | Tx::Burn { amount, .. } | Tx::ContractCall { amount, .. } => amount.to_u256(),
        Tx::EvmCall { value, .. } => value.to_u256(),
        _ => u256::ZERO,
    }
}

fn step(tx: &Tx, coin: &Shown, known: &dyn Fn(&Address) -> Known, lands: Option<Address>) -> Step {
    let shown = |raw: u256| (format::amount(&raw.to_string(), coin.decimals, &coin.symbol), raw.to_string());
    let sends = |raw: u256| (raw != u256::ZERO).then(|| shown(raw));
    let mut laid = Step { kind: String::new(), coin: None, rows: Vec::new(), evm: None, allowed: true };

    match tx {
        Tx::Transfer { to, amount, .. } => {
            laid.kind = format!("Send {}", coin.symbol);
            laid.coin = Some(shown(amount.to_u256()));
            laid.rows.push(Row::address("To", to));
        }
        Tx::Burn { amount, .. } => {
            laid.kind = format!("Burn {}", coin.symbol);
            laid.coin = Some(shown(amount.to_u256()));
        }
        Tx::ContractCall { to, func, amount, data } => {
            laid.coin = sends(amount.to_u256());

            match moved(func, amount.to_u256(), data, known(to)) {
                Some(Moved::Token { asset, to: recipient, units }) => {
                    laid.kind = format!("Send {}", plain(&asset.symbol));
                    laid.rows.push(Row::address("Token", to));
                    laid.rows.push(Row::address("To", recipient));
                    laid.rows.push(Row::text("Amount", format!("{} · {units} units", format::amount(&units.to_string(), asset.decimals, &plain(&asset.symbol)))));
                }
                Some(Moved::Nft { title, to: recipient, id }) => {
                    laid.kind = format!("Send NFT #{id}");
                    laid.rows.push(Row::text("Collection", plain(&title)));
                    laid.rows.push(Row::address("Contract", to));
                    laid.rows.push(Row::text("Id", id.to_string()));
                    laid.rows.push(Row::address("To", recipient));
                }
                Some(Moved::Multi { title, to: recipient, id, count }) => {
                    laid.kind = format!("Send {count} of #{id}");
                    laid.rows.push(Row::text("Collection", plain(&title)));
                    laid.rows.push(Row::address("Contract", to));
                    laid.rows.push(Row::text("Id", id.to_string()));
                    laid.rows.push(Row::text("How many", count.to_string()));
                    laid.rows.push(Row::address("To", recipient));
                }
                None => {
                    laid.kind = match function(func) {
                        Some(name) => format!("Call {name}"),
                        None => "Call a contract".to_string(),
                    };
                    laid.rows.push(Row::address("Contract", to));

                    if function(func).is_none() {
                        laid.rows.push(Row::text("Function", func.to_string()));
                    }

                    // A transfer's shape to a contract the wallet cannot name: what its numbers are
                    // is the contract's to say, so none of them is called an amount.
                    if known(to) == Known::Unknown && sends_like_a_standard(func, data) {
                        laid.rows.push(Row::text("Known as", UNKNOWN_CONTRACT));
                    }

                    for (index, argument) in data.iter().enumerate() {
                        laid.rows.push(argument_row(index, argument));
                    }
                }
            }
        }
        Tx::EvmCall { to, value, input } => {
            laid.coin = sends(value.to_u256());
            laid.kind = match input.is_empty() {
                true => "Send through the EVM".to_string(),
                false => "Call an EVM contract".to_string(),
            };
            laid.rows.push(Row::address("To", to));

            if !input.is_empty() {
                laid.rows.push(Row::text("Data", format!("{} · {} bytes", to_hex(&input[..input.len().min(4)]), input.len())));
                laid.evm = Some(EvmData { to: to.to_string(), input: input.clone() });
            }
        }
        Tx::Deploy { code } => {
            laid.kind = format!("Deploy a {} contract", code.engine().name());
            laid.rows.push(Row::text("Code", format!("{} bytes", code.bytes().len())));

            if let Some(lands) = lands {
                laid.rows.push(Row::address("Lands at", &lands));
            }
        }
        Tx::Upgrade { contract, code } => {
            laid.kind = "Upgrade a contract".to_string();
            laid.rows.push(Row::address("Contract", contract));
            laid.rows.push(Row::text("New code", format!("{} bytes of {}", code.bytes().len(), code.engine().name())));
        }
        Tx::Name { name, action } => {
            laid.kind = crate::airgap::name_kind(action);
            laid.rows.push(Row::text("Name", plain(&crate::airgap::name_detail(name, action))));
        }
        Tx::PrimaryName { name: Some(name) } => {
            laid.kind = "Choose a primary name".to_string();
            laid.rows.push(Row::text("Name", plain(name)));
        }
        Tx::PrimaryName { name: None } => laid.kind = "Clear the primary name".to_string(),
        Tx::Claim { to, height, earned, .. } => {
            laid.kind = "Claim mining pool earnings".to_string();
            laid.rows.push(Row::address("For", to));
            laid.rows.push(Row::text("Of the block", height.to_string()));
            laid.rows.push(Row::text("Earned in all", format::amount(&earned.to_string(), coin.decimals, &coin.symbol)));
        }
        other => {
            laid.kind = format!("Not allowed in a batch: {}", crate::airgap::variant(other));
            laid.allowed = false;
        }
    }

    laid
}

/// A call argument's row: an address as addresses show, and anything else in plain text.
fn argument_row(index: usize, argument: &TxCallArgumentType) -> Row {
    let label = format!("Argument {}", index + 1);

    match argument {
        TxCallArgumentType::Address(address) => Row::address(&label, address),
        TxCallArgumentType::String(text) => Row::text(&label, format!("\"{}\"", plain(text))),
        TxCallArgumentType::Bytes(bytes) => Row::text(&label, format!("{} · {} bytes", to_hex(&bytes[..bytes.len().min(16)]), bytes.len())),
        TxCallArgumentType::Amount(amount) => Row::text(&label, amount.to_u256().to_string()),
        TxCallArgumentType::U256(value) => Row::text(&label, value.to_string()),
        TxCallArgumentType::I256(value) => Row::text(&label, value.to_string()),
        TxCallArgumentType::U128(value) => Row::text(&label, value.to_string()),
        TxCallArgumentType::I128(value) => Row::text(&label, value.to_string()),
        TxCallArgumentType::U64(value) => Row::text(&label, value.to_string()),
        TxCallArgumentType::I64(value) => Row::text(&label, value.to_string()),
        TxCallArgumentType::Number(value) => Row::text(&label, value.to_string()),
        TxCallArgumentType::Bool(value) => Row::text(&label, value.to_string()),
        TxCallArgumentType::Nil => Row::text(&label, "nil"),
    }
}

/// What a step says of a contract the wallet knows as neither a token nor a collection, where it
/// takes the shape of their transfers.
pub const UNKNOWN_CONTRACT: &str = "neither a token nor a collection the chain lists, as far as the wallet has proven: what its numbers are, an amount or an id, is the contract's to say";

/// What a call moves by a standard the wallet knows its contract follows.
enum Moved<'a> {
    Token { asset: Shown, to: &'a Address, units: u256 },
    Nft { title: String, to: &'a Address, id: u256 },
    Multi { title: String, to: &'a Address, id: u256, count: u256 },
}

/// The functions that move a token, or a collection's tokens, to whom the call names.
const MOVES: [&str; 2] = ["transfer", "safe_transfer"];

fn named(func: &Hash, name: &str) -> bool {
    *func == Hasher::digest(HashType::Xxh3_64, name.as_bytes())
}

/// Whether a call takes the shape of a standard's transfer: to an address, then numbers.
fn sends_like_a_standard(func: &Hash, data: &[TxCallArgumentType]) -> bool {
    MOVES.iter().any(|name| named(func, name))
        && matches!(data, [TxCallArgumentType::Address(_), TxCallArgumentType::U256(_)] | [TxCallArgumentType::Address(_), TxCallArgumentType::U256(_), TxCallArgumentType::U256(_)])
}

/// What a call moves, read by the standard its contract is known to follow: a token's transfer, an
/// NFT collection's transfer or safe_transfer of an id, or a multi-token one's of an amount of an
/// id. None for anything else, a contract known as neither among them.
fn moved<'a>(func: &Hash, value: u256, data: &'a [TxCallArgumentType], known: Known) -> Option<Moved<'a>> {
    if value != u256::ZERO {
        return None;
    }

    match (known, data) {
        (Known::Token(asset), [TxCallArgumentType::Address(to), TxCallArgumentType::U256(units)]) if named(func, "transfer") => Some(Moved::Token { asset, to, units: *units }),
        (Known::Collection { kind: CollectionKind::Nft, title }, [TxCallArgumentType::Address(to), TxCallArgumentType::U256(id)]) if MOVES.iter().any(|name| named(func, name)) => {
            Some(Moved::Nft { title, to, id: *id })
        }
        (Known::Collection { kind: CollectionKind::Multi, title }, [TxCallArgumentType::Address(to), TxCallArgumentType::U256(id), TxCallArgumentType::U256(count)]) if MOVES.iter().any(|name| named(func, name)) => {
            Some(Moved::Multi { title, to, id: *id, count: *count })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests;
