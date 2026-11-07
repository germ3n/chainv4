//! EVM calls and logs as what they say rather than their hex: names and typed values, addresses
//! that open their pages, amounts in the token's decimals where those are known and plain whole
//! numbers where not, and always which ABI read them, since a match does not say what the contract
//! is. What is read here comes from chainv4::abi::known, which reads each thing once, however many
//! rows and frames show it.

use {
    super::Ctx,
    crate::format,
    crate::model::Argument,
    chainv4::abi::known::{Piece, ReadArg, ReadBy, ReadCall, ReadLog, TokenMove, clean_text},
    chainv4::abi::{AbiType, decode_values},
    chainv4::structs::hex::{from_hex, to_hex},
    egui::{RichText, Ui},
    serde_json::json,
    std::rc::Rc,
};

/// A token's symbol as long as it may show: what a contract says of itself is untrusted text.
const MAX_SYMBOL: usize = 16;

/// ERC-20's decimals() and symbol(), which an EVM contract answers about itself.
const DECIMALS: [u8; 4] = [0x31, 0x3c, 0xe5, 0x67];
const SYMBOL: [u8; 4] = [0x95, 0xd8, 0x9b, 0x41];

/// An EVM call's calldata, read: by the ABI kept for whom it calls, and otherwise those built in.
pub fn call_of(ctx: &Ctx, to: &str, input: &str) -> Option<Rc<ReadCall>> {
    ctx.reader.call_hex(ctx.abi_text(to), input)
}

/// An event, read, if it is an EVM log something reads: topics first and its data last, as the node
/// writes them.
pub fn log_of(ctx: &Ctx, contract: &str, name: &str, data: &[Argument]) -> Option<Rc<ReadLog>> {
    if name != chainv4::structs::event::EVM_LOG || !contract.starts_with("evm:") {
        return None;
    }

    let values: Vec<&str> = data.iter().map(|value| value.value.as_str()).collect::<Option<_>>()?;
    let (log_data, topics) = values.split_last()?;

    ctx.reader.log(ctx.abi_text(contract), topics, log_data)
}

/// What an EVM contract answers through eth_call, once: asks the first time, and waits.
fn answered(ctx: &mut Ctx, contract: &str, selector: [u8; 4]) -> Option<Vec<u8>> {
    let to = contract.strip_prefix("evm:")?;
    let body = json!({ "jsonrpc": "2.0", "id": 1, "method": "eth_call", "params": [{ "to": to, "data": to_hex(&selector) }, "latest"] });
    let ask = crate::api::Ask::Rpc(body.to_string());

    if let Some(answer) = ctx.store.calls.get(&ask) {
        return answer["result"].as_str().and_then(from_hex);
    }

    if !ctx.store.failures.contains_key(&ask) {
        ctx.api.ask(ask);
    }

    None
}

/// An EVM token's decimals and symbol: as the chain's registry holds them or as set by hand, and
/// otherwise as the contract says of itself. None until one of them does, as for a contract that
/// answers neither: its amounts are then plain whole numbers.
pub fn token_of(ctx: &mut Ctx, contract: &str) -> Option<(u8, String)> {
    if let Some(token) = ctx.token(contract) {
        return Some(token);
    }

    let symbol = answered(ctx, contract, SYMBOL)
        .and_then(|output| decode_values(&[AbiType::String], &output).ok())
        .and_then(|values| match values.first() {
            Some(chainv4::abi::AbiValue::String(symbol)) => Some(clean_text(symbol, MAX_SYMBOL)),
            _ => None,
        })
        .unwrap_or_default();

    if let Some(decimals) = ctx.decimals_set(contract) {
        return Some((decimals, symbol));
    }

    let decimals = answered(ctx, contract, DECIMALS).and_then(|output| decode_values(&[AbiType::Uint(8)], &output).ok()).and_then(|values| match values.first() {
        Some(chainv4::abi::AbiValue::Uint(decimals)) => u8::try_from(*decimals).ok(),
        _ => None,
    })?;

    Some((decimals, symbol))
}

/// Which ABI read something, small, under it: a person always sees whether it is a match or what
/// someone kept for the contract.
fn read_by(ui: &mut Ui, ctx: &Ctx, by: ReadBy) {
    ui.label(RichText::new(by.label()).small().color(ctx.look.palette.muted));
}

/// A whole amount of a contract's own token: in its decimals where they are known, and otherwise as
/// the plain whole number it is.
fn amount(ui: &mut Ui, ctx: &mut Ctx, contract: Option<&str>, raw: &str) {
    match contract.and_then(|contract| token_of(ctx, contract)) {
        Some((decimals, symbol)) => {
            ui.label(RichText::new(format::amount(raw, decimals, &symbol)).strong()).on_hover_text(format!("{raw} whole units"));
        }
        None => {
            ui.label(RichText::new(raw).monospace().strong()).on_hover_text("A whole number: no decimals are known for this contract");
        }
    }
}

/// An address the EVM knows by its 20 bytes, opening its page.
fn address(ui: &mut Ui, ctx: &mut Ctx, bytes: &[u8; 20]) {
    super::address(ui, ctx, &format!("evm:{}", to_hex(bytes)));
}

fn arguments(ui: &mut Ui, ctx: &mut Ctx, contract: Option<&str>, args: &[ReadArg]) {
    let muted = ctx.look.palette.muted;

    for arg in args {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;

            if !arg.name.is_empty() {
                ui.label(RichText::new(format!("{} =", arg.name)).monospace().color(muted)).on_hover_text(&arg.ty);
            }

            for piece in arg.pieces() {
                match piece {
                    Piece::Text(text) => {
                        ui.add(egui::Label::new(RichText::new(text).monospace()).wrap());
                    }
                    Piece::Address(bytes) => address(ui, ctx, &bytes),
                    Piece::Amount(raw) => amount(ui, ctx, contract, &raw.to_string()),
                }
            }
        });

        // A call its bytes hold: its contract is no longer this one, so its amounts stay plain.
        if let Some(inner) = &arg.inner {
            ui.indent(ui.next_auto_id(), |ui| call(ui, ctx, None, inner));
        }
    }
}

/// A call, read: its function, its arguments, and which ABI read it.
pub fn call(ui: &mut Ui, ctx: &mut Ctx, contract: Option<&str>, read: &ReadCall) {
    ui.vertical(|ui| {
        ui.label(RichText::new(&read.name).monospace().strong()).on_hover_text(&read.signature);
        arguments(ui, ctx, contract, &read.args);
        read_by(ui, ctx, read.by);

        if !read.exact {
            ui.label(RichText::new("Its calldata holds more than this reads.").small().color(ctx.look.palette.warn));
        }
    });
}

/// A token moving, as a native Transfer reads: who, to whom, and how much.
fn moved(ui: &mut Ui, ctx: &mut Ctx, contract: &str, token: &TokenMove) {
    let muted = ctx.look.palette.muted;

    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;

        match &token.from {
            Some(from) => address(ui, ctx, from),
            None => {
                ui.label(RichText::new("minted").color(muted));
            }
        }

        ui.label(RichText::new("to").color(muted));

        match &token.to {
            Some(to) => address(ui, ctx, to),
            None => {
                ui.label(RichText::new("burned").color(muted));
            }
        }

        match &token.id {
            // An ERC-721's id says which, and an ERC-1155's says which and how many.
            Some(id) if token.amount == chainv4::structs::u256::u256::ONE => {
                ui.label(RichText::new(format!("token {id}")).monospace().strong());
            }
            Some(id) => {
                ui.label(RichText::new(format!("{} of token {id}", token.amount)).monospace().strong());
            }
            None => amount(ui, ctx, Some(contract), &token.amount.to_string()),
        }
    });
}

/// A log's raw topics and data, a click away, as the node wrote them.
fn raw(ui: &mut Ui, data: &[Argument]) {
    egui::CollapsingHeader::new(RichText::new("raw").small()).id_salt(ui.next_auto_id()).show(ui, |ui| {
        for value in data {
            super::copyable(ui, &value.text());
        }
    });
}

/// A log, read: the tokens it says moved, or its event's values; which ABI read it; and its bytes. A
/// token moving is only what the log claims unless the chain's registry lists the contract.
pub fn log(ui: &mut Ui, ctx: &mut Ctx, contract: &str, read: &ReadLog, data: &[Argument]) {
    ui.vertical(|ui| {
        if read.moves.is_empty() {
            arguments(ui, ctx, Some(contract), &read.args);
        }

        for token in &read.moves {
            moved(ui, ctx, contract, token);
        }

        ui.horizontal_wrapped(|ui| {
            read_by(ui, ctx, read.by);

            if !read.moves.is_empty() && !ctx.store.tokens.contains_key(contract) {
                ui.label(RichText::new("· no listed token, so only what its log claims").small().color(ctx.look.palette.muted));
            }
        });
        raw(ui, data);
    });
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::store::Store,
        chainv4::abi::known::{known_abis, read_call},
        chainv4::abi::{AbiValue, signature_topic},
        chainv4::structs::u256::u256,
        serde_json::Value,
    };

    const TOKEN: &str = "evm:0x0505050505050505050505050505050505050505";

    fn argument(value: Vec<u8>) -> Argument {
        Argument { kind: "bytes".to_string(), value: Value::String(to_hex(&value)) }
    }

    fn pad(address: [u8; 20]) -> Vec<u8> {
        [vec![0; 12], address.to_vec()].concat()
    }

    /// An ERC-20 Transfer, as the node writes an EVM log.
    fn transfer() -> Vec<Argument> {
        vec![
            argument(signature_topic("Transfer(address,address,uint256)").unwrap().to_vec()),
            argument(pad([1; 20])),
            argument(pad([2; 20])),
            argument(u256::new(1_500_000).to_be_bytes().to_vec()),
        ]
    }

    fn drawn(store: &Store, draw: impl FnMut(&mut Ui, &mut Ctx)) -> (String, Vec<crate::api::Ask>) {
        crate::views::render(store, &mut crate::light::Light::default(), 1_200.0, draw)
    }

    #[test]
    fn a_transfer_of_a_contract_with_no_decimals_known_is_a_plain_whole_number_and_a_claim() {
        let (text, asked) = drawn(&Store::default(), |ui, ctx| {
            let read = log_of(ctx, TOKEN, "log", &transfer()).unwrap();
            log(ui, ctx, TOKEN, &read, &transfer());
        });

        assert!(text.contains("1500000"), "{text}");
        assert!(text.contains("read as ERC-20"), "{text}");
        assert!(text.contains("only what its log claims"), "{text}");
        assert!(text.contains("evm:0x0101"), "the sender, by the address that opens its page: {text}");
        assert!(!text.contains("1.5"), "no decimals pretended: {text}");
        assert!(asked.iter().any(|ask| matches!(ask, crate::api::Ask::Rpc(_))), "the contract is asked what it says of itself");
    }

    #[test]
    fn a_contract_that_says_its_decimals_and_symbol_shows_its_amounts_in_them() {
        let mut store = Store::default();
        let eth_call = |selector: [u8; 4]| crate::api::Ask::Rpc(json!({ "jsonrpc": "2.0", "id": 1, "method": "eth_call", "params": [{ "to": TOKEN.strip_prefix("evm:"), "data": to_hex(&selector) }, "latest"] }).to_string());

        store.calls.insert(eth_call(DECIMALS), json!({ "result": to_hex(&u256::new(6).to_be_bytes()) }));
        store.calls.insert(eth_call(SYMBOL), json!({ "result": to_hex(&chainv4::abi::encode_values(&[AbiType::String], &[AbiValue::String("U\nSD".to_string())]).unwrap()) }));

        let (text, _) = drawn(&store, |ui, ctx| {
            let read = log_of(ctx, TOKEN, "log", &transfer()).unwrap();
            log(ui, ctx, TOKEN, &read, &transfer());
        });

        assert!(text.contains("1.5 U\\nSD"), "in its decimals, its symbol escaped: {text}");
    }

    #[test]
    fn an_abi_kept_for_the_contract_reads_first_and_says_so() {
        let store = Store { status: serde_json::from_value(json!({
            "genesis": "0x01", "tip": "0x00", "height": 1, "timestamp": 0, "difficulty": "0", "work": "1",
            "hash": "blake3", "coin": { "symbol": "DEV", "decimals": 18 }, "finality": null, "fees": null,
        })).ok(), ..Store::default() };
        let kept = r#"[{"type": "event", "name": "Transfer", "inputs": [{"name": "payer", "type": "address", "indexed": true}, {"name": "payee", "type": "address", "indexed": true}, {"name": "cents", "type": "uint256"}]}]"#;
        let (text, _) = drawn(&store, |ui, ctx| {
            ctx.set_abi(TOKEN, Some(kept.to_string()));
            let read = log_of(ctx, TOKEN, "log", &transfer()).unwrap();

            assert_eq!(read.by, ReadBy::Kept);
            assert!(read.shown().contains("payer = "));
            log(ui, ctx, TOKEN, &read, &transfer());
        });

        assert!(text.contains("read by the ABI kept for this contract"), "{text}");
    }

    #[test]
    fn a_call_shows_its_arguments_and_nested_call_and_what_is_not_a_log_stays_as_it_is() {
        let known = known_abis();
        let token = chainv4::abi::parse_signature("transfer(address,uint256)").unwrap();
        let held = [token.0.unwrap().to_vec(), chainv4::abi::encode_values(&token.1, &[AbiValue::Address([0xab; 20]), AbiValue::Uint(u256::new(5))]).unwrap()].concat();
        let execute = chainv4::abi::parse_signature("execute(address,uint256,bytes,address[],bytes[])").unwrap();
        let input = [execute.0.unwrap().to_vec(), chainv4::abi::encode_values(&execute.1, &[AbiValue::Address([1; 20]), AbiValue::Uint(u256::ZERO), AbiValue::Bytes(held), AbiValue::Array(vec![AbiValue::Address([2; 20])]), AbiValue::Array(vec![AbiValue::Bytes(vec![0xee; 65])])]).unwrap()].concat();
        let read = read_call(None, &known, &input).unwrap();
        let (text, _) = drawn(&Store::default(), |ui, ctx| call(ui, ctx, Some(TOKEN), &read));

        for said in ["execute", "read as MultiSig.sol", "transfer", "read as ERC-20", "value ="] {
            assert!(text.contains(said), "{said} in {text}");
        }

        // A native event called log is not one, whatever its data.
        let (_, _) = drawn(&Store::default(), |_, ctx| {
            assert!(log_of(ctx, "contract:0x01", "log", &transfer()).is_none());
            assert!(log_of(ctx, TOKEN, "Transfer", &transfer()).is_none());
            assert!(log_of(ctx, TOKEN, "log", &[argument(vec![1; 32]), argument(Vec::new())]).is_none(), "nothing knows this topic");
        });
    }
}
