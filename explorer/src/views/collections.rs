//! The chain's collection registry (docs/nft-standard.md): every contract that exports the NFT
//! standard, one owner a token, or the multi-token one, amounts of each id. A collection's page
//! looks a token up, lists what an address holds, and shows what moved, from the event index:
//! Transfer events for an NFT collection, TransferSingle for a multi-token one. Token URIs show as
//! text: the explorer fetches nothing from where they point.
//!
//! An ERC-721 or ERC-1155 contract on the chain's EVM, which the registry lists too, is read by
//! eth_call, ownerOf, balanceOf, tokenURI and uri, the node's word; what a holder holds is found
//! by its logs naming the holder's EVM form, then checked by ownerOf or balanceOf; what moved is
//! its Transfer or TransferSingle logs.

use {
    super::{Ctx, View},
    crate::api::{Ask, EventQuery},
    crate::model::{Called, FoundEvent, RegisteredCollection},
    chainv4_ui::widgets,
    egui::{RichText, Ui},
    serde_json::{Value, json},
};

/// How long the registry, a read or a page of events stands before it is asked again, in
/// milliseconds.
const REFRESH: u64 = 15_000;
/// The most transfers a collection's page lists.
const EVENTS: u32 = 200;
/// The most ids asked of a collection for one holder: tokens_of's page.
const PAGE: u64 = 100;

#[derive(Default)]
pub struct CollectionsState {
    /// The id typed to look up.
    id: String,
    /// The holder typed, whose tokens to list.
    holder: String,
}

/// The registry, asking again once it is old.
pub fn registry(ctx: &mut Ctx) -> Option<Vec<RegisteredCollection>> {
    let stale = ctx.store.collections.as_ref().is_none_or(|(_, when)| ctx.now.saturating_sub(*when) > ctx.settings.pace().every_ms(REFRESH));

    if stale {
        ctx.api.ask(Ask::Collections(None));
    }

    ctx.store.collections.as_ref().map(|(collections, _)| collections.clone())
}

/// What a read-only call returned, as text, once it answered: Err with the contract's refusal.
fn called(ctx: &mut Ctx, contract: &str, function: &str, args: Value) -> Option<Result<Vec<String>, String>> {
    let asked = Ask::Call { contract: contract.to_string(), function: function.to_string(), args: args.to_string() };

    if !ctx.store.calls.contains_key(&asked) && !ctx.api.asking(&asked) && !ctx.store.failures.contains_key(&asked) {
        ctx.api.ask(asked.clone());
    }

    let called: Called = serde_json::from_value(ctx.store.calls.get(&asked)?.clone()).ok()?;

    Some(match called.status.as_str() {
        "returned" => Ok(called.values.iter().map(|value| value.text()).collect()),
        _ => Err(called.error.unwrap_or_else(|| "the call failed".to_string())),
    })
}

/// Whether a registry entry is a contract on the chain's EVM.
fn is_evm(collection: &RegisteredCollection) -> bool {
    collection.address.starts_with("evm:")
}

/// What an EVM read returned, through eth_call, once it answered: Err with why it did not.
fn evm_read(ctx: &mut Ctx, contract: &str, signature: &str, words: &[Vec<u8>]) -> Option<Result<Vec<u8>, String>> {
    let to = contract.strip_prefix("evm:")?;
    let input = [chainv4::crypto::secp256k1::keccak(signature.as_bytes())[..4].to_vec(), words.concat()].concat();
    let ask = Ask::Rpc(json!({ "jsonrpc": "2.0", "id": 1, "method": "eth_call", "params": [{ "to": to, "data": chainv4::structs::hex::to_hex(&input) }, "latest"] }).to_string());

    if let Some(failure) = ctx.store.failures.get(&ask) {
        return Some(Err(failure.message.clone()));
    }

    let Some(answer) = ctx.store.calls.get(&ask) else {
        if !ctx.api.asking(&ask) {
            ctx.api.ask(ask);
        }

        return None;
    };

    Some(match answer["result"].as_str().and_then(chainv4::structs::hex::from_hex) {
        Some(output) => Ok(output),
        None => Err(answer["error"]["message"].as_str().unwrap_or("it reverted").to_string()),
    })
}

/// An id as one ABI word.
fn id_word(id: &str) -> Vec<u8> {
    chainv4::structs::u256::u256::from_str_radix(id, 10).unwrap_or_default().to_be_bytes().to_vec()
}

/// The EVM form of an address, as one ABI word: an EVM address's own bytes, any other's alias.
fn form_word(address: &str) -> Option<Vec<u8>> {
    let address: chainv4::structs::address::Address = address.parse().ok()?;

    Some([vec![0; 12], chainv4::abi::evm_address(&address).to_vec()].concat())
}

/// An address out of an ABI word, as the chain writes an EVM one.
fn address_of(word: &[u8]) -> Option<String> {
    Some(chainv4::structs::address::Address::Evm(word.get(12..32)?.try_into().ok()?).to_string())
}

/// One ABI string, as tokenURI and uri return it; ERC-1155's {id} filled in with the id's 64 hex
/// digits.
fn uri_of(output: &[u8], id: &str) -> Option<String> {
    use chainv4::abi::{AbiType, AbiValue, decode_values};

    let Some(AbiValue::String(uri)) = decode_values(&[AbiType::String], output).ok()?.pop() else {
        return None;
    };
    let hex = chainv4::structs::hex::to_hex(&id_word(id));

    Some(uri.replace("{id}", &hex[2..]))
}

/// The ids a holder holds of a collection, as tokens_of says, once it answered.
pub fn held(ctx: &mut Ctx, collection: &str, holder: &str) -> Option<Vec<String>> {
    let args = json!([
        { "type": "address", "value": holder },
        { "type": "u256", "value": "0" },
        { "type": "u256", "value": PAGE.to_string() },
    ]);

    called(ctx, collection, "tokens_of", args)?.ok()
}

fn id_arg(id: &str) -> Value {
    json!({ "type": "u256", "value": id })
}

pub fn list(ui: &mut Ui, ctx: &mut Ctx) {
    let look = ctx.look;
    super::heading(ui, look, "NFTs", "The chain's collections: NFTs, one owner a token, and multi-token ones, amounts of each id.", |_| {});

    if ctx.store.status.as_ref().is_some_and(|status| status.collections.is_none()) {
        super::note(ui, look, "This chain keeps no collection registry: neither its genesis nor its governance names what makes one.");

        return;
    }

    let Some(collections) = registry(ctx) else {
        super::waiting(ui, ctx, &Ask::Collections(None));

        return;
    };

    super::note(ui, look, "Names and symbols are each collection's own, as it answered when deployed or last upgraded; symbols are not unique.");
    ui.add_space(8.0);
    super::section(ui, look, "Registered", |ui| {
        if collections.is_empty() {
            super::note(ui, look, "No collections yet.");

            return;
        }

        let none = |text: &str| RichText::new(text.to_string()).color(look.palette.muted);

        match look.narrow {
            true => {
                for collection in &collections {
                    super::list_row(ui, look, |ui| {
                        ui.horizontal(|ui| {
                            open_link(ui, ctx, collection);
                            ui.label(none(kind(collection)));
                        });
                        super::address(ui, ctx, &collection.address);
                    });
                }
            }
            false => {
                super::table(ui, look, "collections", &["collection", "kind", "symbol", "contract"], |ui| {
                    for collection in &collections {
                        open_link(ui, ctx, collection);
                        ui.label(none(kind(collection)));
                        ui.label(collection.symbol.as_deref().unwrap_or("—"));
                        super::address(ui, ctx, &collection.address);
                        super::row_end(ui);
                    }
                });
            }
        }
    });
}

fn kind(collection: &RegisteredCollection) -> &'static str {
    match collection.multi() {
        true => "multi-token",
        false => "NFT",
    }
}

/// A collection's name, opening its page.
pub fn open_link(ui: &mut Ui, ctx: &mut Ctx, collection: &RegisteredCollection) {
    let title = collection.title().unwrap_or_else(|| crate::format::short(&collection.address));

    if ui.link(widgets::strong(&title, 13.5)).on_hover_text(&collection.address).clicked() {
        ctx.go(View::Collection(collection.address.clone()));
    }
}

pub fn show(ui: &mut Ui, ctx: &mut Ctx, state: &mut CollectionsState, address: &str) {
    let look = ctx.look;
    let found = registry(ctx).and_then(|collections| collections.into_iter().find(|collection| collection.address == address));
    let Some(collection) = found else {
        super::heading(ui, look, "Collection", address, |_| {});

        match ctx.store.collections.is_some() {
            true => super::note(ui, look, "The collection registry does not list this contract."),
            false => super::waiting(ui, ctx, &Ask::Collections(None)),
        }

        return;
    };
    let title = collection.title().unwrap_or_else(|| "Collection".to_string());
    super::heading(ui, look, &title, kind(&collection), |_| {});

    super::section(ui, look, "Collection", |ui| {
        super::facts(ui, look, "collection facts", |facts| {
            facts.text("Kind", match collection.multi() {
                true => "Multi-token: holders hold amounts of each id",
                false => "NFT: each token has one owner",
            });
            facts.text("Name", collection.name.clone().unwrap_or_else(|| "—".to_string()));
            facts.text("Symbol", collection.symbol.clone().unwrap_or_else(|| "—".to_string()));
            facts.row("Contract", |ui| super::address(ui, ctx, &collection.address));

            if is_evm(&collection) {
                facts.text("Engine", "EVM: ERC-721 or ERC-1155, read through eth_call");
            } else if !collection.multi()
                && let Some(Ok(supply)) = called(ctx, &collection.address, "total_supply", json!([]))
                && let Some(supply) = supply.first()
            {
                facts.text("Tokens", crate::format::thousands(supply));
            }
        });
    });

    ui.add_space(10.0);
    super::section(ui, look, "Look up a token", |ui| {
        ui.add(egui::TextEdit::singleline(&mut state.id).hint_text("its id, such as 7").desired_width(if look.narrow { f32::INFINITY } else { 200.0 }));
        let id = state.id.trim().to_string();

        if id.is_empty() {
            return;
        }

        if id.parse::<u128>().is_err() && chainv4::structs::u256::u256::from_str_radix(&id, 10).is_err() {
            super::note(ui, look, "An id is a whole number.");

            return;
        }

        token(ui, ctx, &collection, &id);
    });

    ui.add_space(10.0);
    super::section(ui, look, "Held by", |ui| {
        ui.add(egui::TextEdit::singleline(&mut state.holder).hint_text("an address, such as ed25519:0x…").desired_width(f32::INFINITY));
        let holder = state.holder.trim().to_string();

        if holder.is_empty() {
            return;
        }

        if !super::is_address(&holder) {
            super::note(ui, look, "That is not an address.");

            return;
        }

        holdings(ui, ctx, &collection, &holder);
    });

    ui.add_space(10.0);
    super::section(ui, look, "Transfers", |ui| transfers(ui, ctx, &collection));
}

/// An EVM collection's token: an ERC-721's owner, and its URI.
fn evm_token(ui: &mut Ui, ctx: &mut Ctx, collection: &RegisteredCollection, id: &str) {
    let look = ctx.look;
    let owner = (!collection.multi()).then(|| evm_read(ctx, &collection.address, "ownerOf(uint256)", &[id_word(id)]));
    let signature = if collection.multi() { "uri(uint256)" } else { "tokenURI(uint256)" };
    let uri = evm_read(ctx, &collection.address, signature, &[id_word(id)]).and_then(Result::ok).and_then(|output| uri_of(&output, id));

    super::facts(ui, look, "token facts", |facts| {
        match owner {
            Some(None) => facts.row("Owner", |ui| {
                ui.spinner();
            }),
            Some(Some(Err(why))) => facts.text("Owner", format!("none: {why}")),
            Some(Some(Ok(output))) => match address_of(&output) {
                Some(owner) => facts.row("Owner", |ui| super::address(ui, ctx, &owner)),
                None => facts.text("Owner", "it returned no address"),
            },
            None => facts.text("Kind", "ERC-1155: holders hold amounts of it"),
        }

        if let Some(uri) = &uri {
            facts.row("Token URI", |ui| {
                super::copyable(ui, uri);
            });
        }
    });
}

/// One token: its owner, or how many exist, and its URI.
fn token(ui: &mut Ui, ctx: &mut Ctx, collection: &RegisteredCollection, id: &str) {
    if is_evm(collection) {
        return evm_token(ui, ctx, collection, id);
    }

    let look = ctx.look;
    let first = |answer: Option<Result<Vec<String>, String>>| answer.map(|answer| answer.map(|values| values.into_iter().next().unwrap_or_default()));
    let held = match collection.multi() {
        false => first(called(ctx, &collection.address, "owner_of", json!([id_arg(id)]))),
        true => first(called(ctx, &collection.address, "total_supply", json!([id_arg(id)]))),
    };
    let uri = first(called(ctx, &collection.address, "token_uri", json!([id_arg(id)])));

    super::facts(ui, look, "token facts", |facts| match held {
        None => facts.row("Token", |ui| {
            ui.spinner();
        }),
        Some(Err(why)) => facts.text("Token", why),
        Some(Ok(held)) => {
            match collection.multi() {
                false => facts.row("Owner", |ui| super::address(ui, ctx, &held)),
                true => facts.text("In existence", crate::format::thousands(&held)),
            }

            if let Some(Ok(uri)) = &uri {
                facts.row("Token URI", |ui| {
                    super::copyable(ui, uri);
                });
            }
        }
    });
}

/// What a holder holds of an EVM collection: the ids its logs name the holder's EVM form in, each
/// checked by ownerOf or balanceOf.
fn evm_holdings(ui: &mut Ui, ctx: &mut Ctx, collection: &RegisteredCollection, holder: &str) {
    let look = ctx.look;
    let Some(form) = form_word(holder) else {
        super::note(ui, look, "That is not an address.");

        return;
    };
    let query = EventQuery::topic(&chainv4::structs::hex::to_hex(&form), Some(&collection.address)).limited(EVENTS);
    let asked = Ask::Events { query: query.clone(), cursor: None };

    if !ctx.store.events.contains_key(&query) && !ctx.api.asking(&asked) && !ctx.store.failures.contains_key(&asked) {
        ctx.api.ask(asked.clone());
    }

    let Some(events) = ctx.store.events.get(&query).map(|found| found.items.clone()) else {
        match ctx.store.failures.get(&asked) {
            Some(failure) if failure.status == 404 => super::note(ui, look, "The node keeps no event index: start it with --index-events to find what an address holds of an EVM collection."),
            _ => super::waiting(ui, ctx, &asked),
        }

        return;
    };
    let mut ids: Vec<String> = Vec::new();

    for moved in events.iter().filter_map(|event| evm_moved(event, collection.multi())) {
        if !ids.contains(&moved.id) {
            ids.push(moved.id);
        }
    }

    let mut held = Vec::new();

    for id in &ids {
        let answer = match collection.multi() {
            false => evm_read(ctx, &collection.address, "ownerOf(uint256)", &[id_word(id)]).map(|answer| answer.map(|output| (output.get(..32) == Some(&form[..])).then(|| "1".to_string()))),
            true => evm_read(ctx, &collection.address, "balanceOf(address,uint256)", &[form.clone(), id_word(id)])
                .map(|answer| answer.map(|output| output.get(..32).map(|word| chainv4::structs::u256::u256::from_be_bytes(word.try_into().expect("32 bytes"))).filter(|amount| *amount > chainv4::structs::u256::u256::ZERO).map(|amount| amount.to_string()))),
        };

        match answer {
            None => {
                ui.spinner();

                return;
            }
            Some(Ok(Some(amount))) => held.push((id.clone(), amount)),
            Some(_) => {}
        }
    }

    if held.is_empty() {
        super::note(ui, look, "Nothing of this collection, as its logs and its own answers say.");

        return;
    }

    super::table(ui, look, ("held", holder.to_string()), if collection.multi() { &["id", "amount"] } else { &["id"] }, |ui| {
        for (id, amount) in &held {
            ui.monospace(id);

            if collection.multi() {
                ui.monospace(amount);
            }

            super::row_end(ui);
        }
    });
}

/// What a holder holds of a collection: its ids, and of a multi-token one, how many of each.
fn holdings(ui: &mut Ui, ctx: &mut Ctx, collection: &RegisteredCollection, holder: &str) {
    if is_evm(collection) {
        return evm_holdings(ui, ctx, collection, holder);
    }

    let look = ctx.look;
    let Some(ids) = held(ctx, &collection.address, holder) else {
        ui.spinner();

        return;
    };

    if ids.is_empty() {
        super::note(ui, look, "Nothing of this collection.");

        return;
    }

    super::table(ui, look, ("held", holder.to_string()), if collection.multi() { &["id", "amount"] } else { &["id"] }, |ui| {
        for id in &ids {
            ui.monospace(id);

            if collection.multi() {
                let args = json!([{ "type": "address", "value": holder }, id_arg(id)]);

                match called(ctx, &collection.address, "balance_of", args) {
                    Some(Ok(amount)) => ui.monospace(amount.first().cloned().unwrap_or_default()),
                    Some(Err(why)) => ui.label(why),
                    None => ui.spinner(),
                };
            }

            super::row_end(ui);
        }
    });

    if ids.len() as u64 >= PAGE {
        super::note(ui, look, format!("The first {PAGE} shown."));
    }
}

/// A transfer, as a collection's events say: who from and to, which id, how many.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Moved {
    pub from: String,
    pub to: String,
    pub id: String,
    pub amount: String,
    pub height: u64,
    pub block: String,
    pub tx: String,
}

/// A collection's event as a transfer: Transfer(from, to, id) for an NFT collection,
/// TransferSingle(operator, from, to, id, amount) for a multi-token one.
pub fn moved(event: &FoundEvent, multi: bool) -> Option<Moved> {
    let text = |index: usize| event.data.get(index).map(|argument| argument.text());
    let (from, to, id, amount) = match (multi, event.name.as_str()) {
        (false, "Transfer") => (text(0)?, text(1)?, text(2)?, "1".to_string()),
        (true, "TransferSingle") => (text(1)?, text(2)?, text(3)?, text(4)?),
        _ => return None,
    };

    Some(Moved { from, to, id, amount, height: event.height, block: event.block.clone(), tx: event.tx.clone() })
}

/// An EVM collection's log as a transfer: an ERC-721's Transfer, with four topics, or an ERC-1155's
/// TransferSingle; addresses as the chain writes EVM ones.
pub fn evm_moved(event: &FoundEvent, multi: bool) -> Option<Moved> {
    use chainv4::crypto::secp256k1::keccak;

    let parts: Vec<Vec<u8>> = event.data.iter().filter_map(|part| chainv4::structs::hex::from_hex(&part.text())).collect();
    let (data, topics) = parts.split_last()?;
    let number = |word: &[u8]| Some(chainv4::structs::u256::u256::from_be_bytes(word.get(..32)?.try_into().ok()?).to_string());
    let (from, to, id, amount) = match (multi, topics.first().map(Vec::as_slice)) {
        (false, Some(first)) if first == keccak(b"Transfer(address,address,uint256)") && topics.len() == 4 => (address_of(&topics[1])?, address_of(&topics[2])?, number(&topics[3])?, "1".to_string()),
        (true, Some(first)) if first == keccak(b"TransferSingle(address,address,address,uint256,uint256)") && topics.len() == 4 => {
            (address_of(&topics[2])?, address_of(&topics[3])?, number(data)?, number(data.get(32..)?)?)
        }
        _ => return None,
    };

    Some(Moved { from, to, id, amount, height: event.height, block: event.block.clone(), tx: event.tx.clone() })
}

fn transfers(ui: &mut Ui, ctx: &mut Ctx, collection: &RegisteredCollection) {
    let look = ctx.look;
    let name = if collection.multi() { "TransferSingle" } else { "Transfer" };
    let query = match is_evm(collection) {
        true => {
            let signature = if collection.multi() { "TransferSingle(address,address,address,uint256,uint256)" } else { "Transfer(address,address,uint256)" };
            let topic = chainv4::structs::hex::to_hex(&chainv4::crypto::secp256k1::keccak(signature.as_bytes()));

            EventQuery::topic(&topic, Some(&collection.address)).limited(EVENTS)
        }
        false => EventQuery::contract(&collection.address, Some(name)).limited(EVENTS),
    };
    let newest = Ask::Events { query: query.clone(), cursor: None };
    let found = ctx.store.events.get(&query);

    if found.is_none_or(|found| ctx.now.saturating_sub(found.refreshed) > ctx.settings.pace().every_ms(REFRESH)) {
        ctx.api.ask(newest.clone());
    }

    let Some(events) = ctx.store.events.get(&query).map(|found| found.items.clone()) else {
        match ctx.store.failures.get(&newest) {
            Some(failure) if failure.status == 404 => super::note(ui, look, "The node keeps no event index: start it with --index-events to see what moved."),
            _ => super::waiting(ui, ctx, &newest),
        }

        return;
    };
    let moves: Vec<Moved> = events.iter().filter_map(|event| if is_evm(collection) { evm_moved(event, collection.multi()) } else { moved(event, collection.multi()) }).collect();

    if moves.is_empty() {
        super::note(ui, look, "Nothing moved yet.");

        return;
    }

    let zero = chainv4::structs::address::Address::Ed25519([0; 32]).to_string();
    let evm_zero = chainv4::structs::address::Address::Evm([0; 20]).to_string();
    let party = |ui: &mut Ui, ctx: &mut Ctx, text: &str, what: &str| match text == zero || text == evm_zero {
        true => {
            ui.label(RichText::new(what).color(look.palette.muted));
        }
        false => super::address(ui, ctx, text),
    };

    if look.narrow {
        for moved in &moves {
            super::list_row(ui, look, |ui| {
                ui.horizontal(|ui| {
                    ui.label(widgets::strong(format!("#{}", moved.id), 13.5).color(look.palette.ink));

                    if collection.multi() {
                        ui.label(RichText::new(format!("× {}", moved.amount)).monospace());
                    }

                    super::tx(ui, ctx, &moved.tx);
                });
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    party(ui, ctx, &moved.from, "minted");
                    ui.label(RichText::new("→").color(look.palette.muted));
                    party(ui, ctx, &moved.to, "burned");
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("block").small().color(look.palette.muted));
                    super::block_link(ui, ctx, &moved.block, moved.height);
                });
            });
        }

        return;
    }

    super::table(ui, look, ("transfers", &collection.address), &["block", "from", "to", "id", "amount", "transaction"], |ui| {
        for moved in &moves {
            super::block_link(ui, ctx, &moved.block, moved.height);
            party(ui, ctx, &moved.from, "minted");
            party(ui, ctx, &moved.to, "burned");
            ui.monospace(&moved.id);
            ui.monospace(&moved.amount);
            super::tx(ui, ctx, &moved.tx);
            super::row_end(ui);
        }
    });
}

#[cfg(test)]
mod tests;
