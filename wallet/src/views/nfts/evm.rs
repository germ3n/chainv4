//! The chain's EVM collections on the NFTs page: ERC-721 and ERC-1155 contracts on its own EVM,
//! which the collection registry lists as it lists native ones. Where to look comes from the
//! node's event index: each collection's logs that name one of the account's EVM forms in a topic,
//! a Transfer's id or a TransferSingle's and TransferBatch's ids. What is so is proven: ownerOf
//! and balanceOf run at the block the light client checked last, over the entries they read, each
//! proven, and run again here (POST /call with calldata, vm::run_proven_evm), as watched tokens
//! are. Sending one is an EVM call, safeTransferFrom or safeBatchTransferFrom, the Send page
//! reviews and signs.

use {
    super::super::Ctx,
    super::{Card, Listed, Proof},
    crate::node::Ask,
    chainv4::abi::{AbiType, AbiValue, decode_values, encode_values, evm_address},
    chainv4::crypto::secp256k1::keccak,
    chainv4::registry::collections::CollectionKind,
    chainv4::structs::address::Address,
    chainv4::structs::hex::{from_hex, to_hex},
    chainv4::structs::u256::u256,
    serde_json::{Value, json},
    std::collections::HashMap,
};

const OWNER_OF: &str = "ownerOf(uint256)";
const BALANCE_OF: &str = "balanceOf(address,uint256)";
const TOKEN_URI: &str = "tokenURI(uint256)";
const URI: &str = "uri(uint256)";
const ROYALTY_INFO: &str = "royaltyInfo(uint256,uint256)";
const TRANSFER: &str = "Transfer(address,address,uint256)";
const TRANSFER_SINGLE: &str = "TransferSingle(address,address,address,uint256,uint256)";
const TRANSFER_BATCH: &str = "TransferBatch(address,address,address,uint256[],uint256[])";
/// The most pages of one collection's logs followed for one form, 200 a page.
const PAGES: usize = 10;

/// What came back of a read: the bytes it returned, or why there are none, and the height this
/// wallet ran it at over proven state, when it did.
#[derive(Clone, Debug, PartialEq)]
pub struct Read {
    pub answer: Result<Vec<u8>, String>,
    pub proven: Option<u64>,
}

#[derive(Default)]
pub struct EvmState {
    /// The ids each collection's logs name for the account's forms: where to look.
    ids: HashMap<String, Vec<String>>,
    /// The pages of logs asked for each collection and form so far.
    pages: HashMap<(String, String), usize>,
    /// The height the owners and amounts were last asked at.
    asked_at: Option<u64>,
    /// Owners and amounts: by collection, id and form, the form empty for an owner.
    held: HashMap<(String, String, String), Read>,
    /// Each token's URI, by collection and id.
    pub uris: HashMap<(String, String), String>,
    /// Each token's royalty on a sale of 10,000, its receiver and share; None when it keeps none.
    pub royalties: HashMap<(String, String), Option<(String, u256)>>,
    /// Why a collection's logs could not be read, until they are.
    pub unlisted: Option<String>,
}

/// The first four bytes of a function's keccak256.
fn selector(signature: &str) -> Vec<u8> {
    keccak(signature.as_bytes())[..4].to_vec()
}

/// A form as the topic a log holds it in: its 20 bytes, padded to 32.
fn topic(form: &[u8; 20]) -> String {
    to_hex(&[[0; 12].as_slice(), form].concat())
}

fn word(value: u256) -> Vec<u8> {
    value.to_be_bytes().to_vec()
}

/// Whether a registry entry is an EVM contract.
pub fn is_evm(listed: &Listed) -> bool {
    matches!(listed.address.parse::<Address>(), Ok(Address::Evm(_)))
}

/// The account's addresses with each one's EVM form.
pub fn forms(ctx: &Ctx) -> Vec<(Address, [u8; 20])> {
    let mut forms: Vec<(Address, [u8; 20])> = Vec::new();

    for address in ctx.holder().map(|holder| holder.addresses()).unwrap_or_default() {
        let form = evm_address(&address);

        if !forms.iter().any(|(_, seen)| *seen == form) {
            forms.push((address, form));
        }
    }

    forms
}

/// The ids a page of a collection's logs names: a Transfer's last topic, a TransferSingle's id and a
/// TransferBatch's ids, each once, in the order they come.
pub fn ids_in(events: &[Value]) -> Vec<String> {
    let (single, batch, transfer) = (keccak(TRANSFER_SINGLE.as_bytes()), keccak(TRANSFER_BATCH.as_bytes()), keccak(TRANSFER.as_bytes()));
    let mut ids: Vec<String> = Vec::new();

    for event in events {
        // An EVM log's values: its topics, then its data.
        let parts: Vec<Vec<u8>> = event["data"].as_array().into_iter().flatten().filter_map(|part| part["value"].as_str().and_then(from_hex)).collect();
        let Some((data, topics)) = parts.split_last() else { continue };
        let first = topics.first().map(Vec::as_slice);
        let found: Vec<u256> = match first {
            Some(first) if first == transfer && topics.len() == 4 => topics[3].as_slice().try_into().map(u256::from_be_bytes).into_iter().collect(),
            Some(first) if first == single && data.len() >= 32 => data[..32].try_into().map(u256::from_be_bytes).into_iter().collect(),
            Some(first) if first == batch => match decode_values(&[AbiType::Array(Box::new(AbiType::Uint(256))), AbiType::Array(Box::new(AbiType::Uint(256)))], data).ok().as_deref() {
                Some([AbiValue::Array(listed), _]) => listed.iter().filter_map(|id| if let AbiValue::Uint(id) = id { Some(*id) } else { None }).collect(),
                _ => Vec::new(),
            },
            _ => Vec::new(),
        };

        for id in found.into_iter().map(|id| id.to_string()) {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
    }

    ids
}

/// A read of a collection, run over proven state: its calldata, what it is for, from whom.
fn read_ask(collection: &str, id: &str, form: &[u8; 20], what: &str, input: Vec<u8>) -> Ask {
    Ask::Call(
        json!({
            "contract": collection,
            "input": to_hex(&input),
            "from": to_hex(form),
            "purpose": "nfts-evm",
            "collection": collection,
            "id": id,
            "what": what,
        })
        .to_string(),
    )
}

/// One ABI string out of what a call returned.
fn string(output: &[u8]) -> Option<String> {
    match decode_values(&[AbiType::String], output).ok()?.pop()? {
        AbiValue::String(text) => Some(text),
        _ => None,
    }
}

impl EvmState {
    /// Asks what the page lacks: each collection's logs for each form, once, then a page further
    /// while they go on; owners and amounts again once the light client moves on; URIs once.
    pub fn refresh(&mut self, ctx: &mut Ctx, collections: &[Listed]) {
        let forms = forms(ctx);

        for listed in collections.iter().filter(|listed| is_evm(listed)) {
            for (_, form) in &forms {
                let at = (listed.address.clone(), topic(form));

                if !self.pages.contains_key(&at) {
                    self.pages.insert(at.clone(), 1);
                    ctx.node.ask(Ask::Logs { contract: listed.address.clone(), topic: at.1, cursor: None });
                }
            }
        }

        let height = ctx.chain.following().map(|(tip, _)| tip);

        if height.is_some() && self.asked_at != height {
            self.asked_at = height;

            for listed in collections.iter().filter(|listed| is_evm(listed)) {
                for id in self.ids.get(&listed.address).into_iter().flatten() {
                    let number = u256::from_str_radix(id, 10).unwrap_or(u256::ZERO);

                    match listed.kind {
                        // An owner is one read, whoever asks.
                        CollectionKind::Nft => {
                            if let Some((_, form)) = forms.first() {
                                ctx.node.ask(read_ask(&listed.address, id, form, "owner", [selector(OWNER_OF), word(number)].concat()));
                            }
                        }
                        CollectionKind::Multi => {
                            for (_, form) in &forms {
                                let input = [selector(BALANCE_OF), vec![0; 12], form.to_vec(), word(number)].concat();
                                ctx.node.ask(read_ask(&listed.address, id, form, "amount", input));
                            }
                        }
                    }
                }
            }
        }

        // Each held token's URI, once.
        let Some((_, form)) = forms.first() else { return };

        for listed in collections.iter().filter(|listed| is_evm(listed)) {
            for id in self.ids.get(&listed.address).into_iter().flatten() {
                let at = (listed.address.clone(), id.clone());

                if !self.uris.contains_key(&at) && self.held.keys().any(|(collection, held, _)| (collection, held) == (&at.0, &at.1)) {
                    // Asked once: what comes back, or nothing, stands.
                    self.uris.insert(at.clone(), String::new());
                    let function = if listed.kind == CollectionKind::Nft { TOKEN_URI } else { URI };
                    let number = u256::from_str_radix(id, 10).unwrap_or(u256::ZERO);
                    ctx.node.ask(read_ask(&listed.address, id, form, "uri", [selector(function), word(number)].concat()));
                }
            }
        }
    }

    /// Asks a token's royalty, once.
    pub fn royalty(&mut self, ctx: &mut Ctx, collection: &str, id: &str) {
        let at = (collection.to_string(), id.to_string());

        if self.royalties.contains_key(&at) {
            return;
        }

        if let Some((_, form)) = forms(ctx).first() {
            self.royalties.insert(at, None);
            let number = u256::from_str_radix(id, 10).unwrap_or(u256::ZERO);
            ctx.node.ask(read_ask(collection, id, form, "royalty", [selector(ROYALTY_INFO), word(number), word(u256::new(10_000))].concat()));
        }
    }

    /// Takes a page of a collection's logs: the ids they name, and the next page while there is one.
    pub fn logs(&mut self, ask: &Ask, result: &Result<Value, crate::node::Failure>) -> Option<Ask> {
        let Ask::Logs { contract, topic, .. } = ask else {
            return None;
        };

        match result {
            Ok(value) => {
                self.unlisted = None;
                let found = ids_in(value["events"].as_array().map(Vec::as_slice).unwrap_or_default());
                let ids = self.ids.entry(contract.clone()).or_default();

                for id in found {
                    if !ids.contains(&id) {
                        ids.push(id);
                        // Asked at once, whatever height was asked at before.
                        self.asked_at = None;
                    }
                }

                let pages = self.pages.entry((contract.clone(), topic.clone())).or_insert(1);

                match value["next"].as_str() {
                    Some(next) if *pages < PAGES => {
                        *pages += 1;

                        Some(Ask::Logs { contract: contract.clone(), topic: topic.clone(), cursor: Some(next.to_string()) })
                    }
                    _ => None,
                }
            }
            Err(failure) => {
                self.unlisted = Some(match failure.status {
                    404 => "the node keeps no event index, so EVM collections' tokens cannot be found".to_string(),
                    _ => failure.message.clone(),
                });
                // Asked again on the next look.
                self.pages.remove(&(contract.clone(), topic.clone()));

                None
            }
        }
    }

    /// Takes a read's answer; false for anyone else's.
    pub fn answered(&mut self, ask: &Ask, result: &Result<Value, crate::node::Failure>) -> bool {
        let Ask::Call(body) = ask else {
            return false;
        };
        let Ok(body) = serde_json::from_str::<Value>(body) else {
            return false;
        };

        if body["purpose"] != "nfts-evm" {
            return false;
        }

        let text = |key: &str| body[key].as_str().unwrap_or_default().to_string();
        let (collection, id, from) = (text("collection"), text("id"), text("from"));
        let read = match result {
            Ok(value) => Read {
                answer: match (value["status"].as_str(), value["output"].as_str().and_then(from_hex)) {
                    (Some("returned"), Some(output)) => Ok(output),
                    _ => Err(value["error"].as_str().unwrap_or("it failed").to_string()),
                },
                proven: (value["checked"]["here"] == true).then(|| value["checked"]["height"].as_u64()).flatten(),
            },
            Err(failure) => Read { answer: Err(failure.message.clone()), proven: None },
        };

        match body["what"].as_str() {
            Some("owner") => {
                self.held.insert((collection, id, String::new()), read);
            }
            Some("amount") => {
                self.held.insert((collection, id, from), read);
            }
            Some("uri") => {
                let uri = read.answer.ok().and_then(|output| string(&output)).unwrap_or_default();
                // ERC-1155's {id}: the id as 64 lowercase hex digits.
                let id_hex = u256::from_str_radix(&id, 10).map(|id| to_hex(&id.to_be_bytes())[2..].to_string()).unwrap_or_default();
                self.uris.insert((collection, id), uri.replace("{id}", &id_hex));
            }
            Some("royalty") => {
                let paid = read.answer.ok().and_then(|output| match decode_values(&[AbiType::Address, AbiType::Uint(256)], &output).ok().as_deref() {
                    Some([AbiValue::Address(receiver), AbiValue::Uint(share)]) if *share > u256::ZERO => Some((Address::Evm(*receiver).to_string(), *share)),
                    _ => None,
                });
                self.royalties.insert((collection, id), paid);
            }
            _ => {}
        }

        true
    }

    /// The cards of what the account's forms are proven, or said, to hold of the EVM collections.
    pub fn cards(&self, ctx: &Ctx, collections: &[Listed]) -> Vec<Card> {
        let forms = forms(ctx);
        let mut cards = Vec::new();

        for listed in collections.iter().filter(|listed| is_evm(listed)) {
            for id in self.ids.get(&listed.address).into_iter().flatten() {
                match listed.kind {
                    CollectionKind::Nft => {
                        let Some(read) = self.held.get(&(listed.address.clone(), id.clone(), String::new())) else { continue };
                        let Ok(owner) = &read.answer else { continue };
                        let mine = forms.iter().find(|(_, form)| owner.len() >= 32 && owner[12..32] == form[..]);

                        if let Some((holder, _)) = mine {
                            cards.push(Card { listed: listed.clone(), id: id.clone(), holder: holder.to_string(), amount: u256::ONE, proof: Proof::Rerun(read.proven) });
                        }
                    }
                    CollectionKind::Multi => {
                        for (holder, form) in &forms {
                            let Some(read) = self.held.get(&(listed.address.clone(), id.clone(), to_hex(form))) else { continue };
                            let amount = read.answer.as_ref().ok().filter(|output| output.len() >= 32).map(|output| u256::from_be_bytes(output[..32].try_into().expect("32 bytes")));

                            if let Some(amount) = amount.filter(|amount| *amount > u256::ZERO) {
                                cards.push(Card { listed: listed.clone(), id: id.clone(), holder: holder.to_string(), amount, proof: Proof::Rerun(read.proven) });
                            }
                        }
                    }
                }
            }
        }

        cards
    }

    /// Whether ids were found that are still being read.
    pub fn reading(&self) -> bool {
        self.ids.values().any(|ids| !ids.is_empty()) && self.held.is_empty()
    }
}

/// The calldata that sends a token of an EVM collection: safeTransferFrom, an ERC-721's with three
/// arguments and an ERC-1155's with five, from the holder's EVM form, data empty.
pub fn transfer_input(kind: CollectionKind, from: &Address, to: &Address, id: u256, amount: u256) -> Vec<u8> {
    let (from, to) = (AbiValue::Address(evm_address(from)), AbiValue::Address(evm_address(to)));

    match kind {
        CollectionKind::Nft => [selector("safeTransferFrom(address,address,uint256)"), encode_values(&[AbiType::Address, AbiType::Address, AbiType::Uint(256)], &[from, to, AbiValue::Uint(id)]).expect("fits")].concat(),
        CollectionKind::Multi => [
            selector("safeTransferFrom(address,address,uint256,uint256,bytes)"),
            encode_values(&[AbiType::Address, AbiType::Address, AbiType::Uint(256), AbiType::Uint(256), AbiType::Bytes], &[from, to, AbiValue::Uint(id), AbiValue::Uint(amount), AbiValue::Bytes(Vec::new())]).expect("fits"),
        ]
        .concat(),
    }
}

/// The calldata that sends several ids of an ERC-1155 collection at once: safeBatchTransferFrom.
pub fn batch_input(from: &Address, to: &Address, items: &[(u256, u256)]) -> Vec<u8> {
    let list = |values: Vec<u256>| AbiValue::Array(values.into_iter().map(AbiValue::Uint).collect());
    let uints = || AbiType::Array(Box::new(AbiType::Uint(256)));

    [
        selector("safeBatchTransferFrom(address,address,uint256[],uint256[],bytes)"),
        encode_values(
            &[AbiType::Address, AbiType::Address, uints(), uints(), AbiType::Bytes],
            &[AbiValue::Address(evm_address(from)), AbiValue::Address(evm_address(to)), list(items.iter().map(|(id, _)| *id).collect()), list(items.iter().map(|(_, amount)| *amount).collect()), AbiValue::Bytes(Vec::new())],
        )
        .expect("fits"),
    ]
    .concat()
}

#[cfg(test)]
mod tests;
