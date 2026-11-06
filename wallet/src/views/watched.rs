//! Tokens sites asked the wallet to show (wallet_watchAsset), on this chain's EVM: an ERC-20's
//! balance, an ERC-721's owner, or an ERC-1155 id's amount, at each of the account's EVM forms.
//! Each is an EVM read run at the block the light client checked last, with every entry it reads
//! proven, and run again here (POST /call with calldata, vm::run_proven_evm): what the card shows is
//! this wallet's own answer, and says when it is only the node's. Asked again once the light
//! client moves on.

use {
    super::Ctx,
    crate::dapp::{AssetKind, Watched},
    crate::format,
    crate::node::Ask,
    crate::theme,
    chainv4::structs::address::Address,
    chainv4::structs::hex::{from_hex, to_hex},
    chainv4::structs::u256::u256,
    egui::{RichText, Ui},
    serde_json::{Value, json},
    std::collections::HashMap,
};

const BALANCE_OF: [u8; 4] = [0x70, 0xa0, 0x82, 0x31];
const OWNER_OF: [u8; 4] = [0x63, 0x52, 0x21, 0x1e];
const BALANCE_OF_ID: [u8; 4] = [0x00, 0xfd, 0xd5, 0x8e];

#[derive(Default)]
pub struct WatchedState {
    /// The height last asked at: once the light client moves past it, everything is asked again.
    asked_at: Option<u64>,
    /// What came back, by token and EVM form: the word read, or why there is none, and whether this
    /// wallet ran it over proven state.
    read: HashMap<(String, String), Read>,
}

#[derive(Clone, Debug)]
struct Read {
    answer: Result<Vec<u8>, String>,
    proven: Option<u64>,
}

/// The token's key: its contract, and an NFT's id.
fn key(token: &Watched) -> String {
    format!("{}#{}", token.address, token.token_id.as_deref().unwrap_or(""))
}

fn word(value: u256) -> Vec<u8> {
    value.to_be_bytes().to_vec()
}

fn address_word(evm: &[u8; 20]) -> Vec<u8> {
    [vec![0; 12], evm.to_vec()].concat()
}

/// The calldata that reads a token for one EVM form.
fn calldata(token: &Watched, evm: &[u8; 20]) -> Vec<u8> {
    let id = token.token_id.as_deref().and_then(|id| id.parse::<u256>().ok()).unwrap_or(u256::ZERO);

    match token.kind {
        AssetKind::Erc20 => [BALANCE_OF.to_vec(), address_word(evm)].concat(),
        AssetKind::Erc721 => [OWNER_OF.to_vec(), word(id)].concat(),
        AssetKind::Erc1155 => [BALANCE_OF_ID.to_vec(), address_word(evm), word(id)].concat(),
    }
}

/// The account's EVM forms: each address's 20 bytes as the EVM sees it, once each.
fn forms(ctx: &Ctx) -> Vec<[u8; 20]> {
    let mut forms: Vec<[u8; 20]> = ctx.holder().map(|holder| holder.addresses().iter().map(chainv4::abi::evm_address).collect()).unwrap_or_default();
    forms.dedup();

    forms
}

/// This chain's watched tokens.
fn on_this_chain(ctx: &Ctx) -> Vec<Watched> {
    let Some(genesis) = ctx.chain.genesis.map(|genesis| genesis.to_string()) else {
        return Vec::new();
    };

    ctx.settings.watched.iter().filter(|(on, _)| *on == genesis).map(|(_, token)| token.clone()).collect()
}

impl WatchedState {
    /// Takes the reads' answers; false for anyone else's.
    pub fn answered(&mut self, ask: &Ask, result: &Result<Value, crate::node::Failure>) -> bool {
        let Ask::Call(body) = ask else {
            return false;
        };
        let Ok(body) = serde_json::from_str::<Value>(body) else {
            return false;
        };

        if body["purpose"] != "watched" {
            return false;
        }

        let token = body["token"].as_str().unwrap_or_default().to_string();
        let form = body["from"].as_str().unwrap_or_default().to_string();
        let read = match result {
            Ok(value) => {
                let proven = (value["checked"]["here"] == true).then(|| value["checked"]["height"].as_u64()).flatten();
                let answer = match (value["status"].as_str(), value["output"].as_str().and_then(from_hex)) {
                    (Some("returned"), Some(output)) if output.len() >= 32 => Ok(output[..32].to_vec()),
                    (Some("returned"), _) => Err("it returned no word: not such a token".to_string()),
                    _ => Err(value["error"].as_str().unwrap_or("it failed").to_string()),
                };

                Read { answer, proven }
            }
            Err(failure) => Read { answer: Err(failure.message.clone()), proven: None },
        };
        self.read.insert((token, form), read);

        true
    }
}

/// The card of watched tokens on Overview, asking for what it lacks.
pub fn card(ui: &mut Ui, ctx: &mut Ctx, state: &mut WatchedState) {
    let palette = *ctx.palette;
    let tokens = on_this_chain(ctx);

    if tokens.is_empty() {
        return;
    }

    let forms = forms(ctx);
    let height = ctx.chain.following().map(|(tip, _)| tip);

    // Once the light client moves on, everything is read again, at the block it checked.
    if height.is_some() && state.asked_at != height {
        state.asked_at = height;

        for token in &tokens {
            // An NFT's owner is one read, whoever asks.
            let askers: &[[u8; 20]] = if token.kind == AssetKind::Erc721 { &forms[..forms.len().min(1)] } else { &forms };

            for form in askers {
                ctx.node.ask(Ask::Call(
                    json!({
                        "contract": format!("evm:{}", token.address),
                        "input": to_hex(&calldata(token, form)),
                        "from": to_hex(form),
                        "purpose": "watched",
                        "token": key(token),
                    })
                    .to_string(),
                ));
            }
        }
    }

    let mut forget = None;
    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Tokens sites added");

        for token in &tokens {
            let reads: Vec<&Read> = forms.iter().filter_map(|form| state.read.get(&(key(token), to_hex(form)))).collect();
            let name = if token.symbol.is_empty() { format::short(&token.address) } else { token.symbol.clone() };
            let shown = match token.kind {
                AssetKind::Erc20 | AssetKind::Erc1155 => {
                    let held = reads.iter().filter_map(|read| read.answer.as_ref().ok()).fold(u256::ZERO, |sum, word| sum.saturating_add(u256::from_be_bytes(word[..].try_into().expect("32 bytes"))));
                    let decimals = if token.kind == AssetKind::Erc20 { token.decimals } else { 0 };

                    format::amount(&held.to_string(), decimals, &name)
                }
                AssetKind::Erc721 => match reads.first().map(|read| &read.answer) {
                    Some(Ok(owner)) => match forms.iter().any(|form| owner[12..] == form[..]) {
                        true => format!("{name} #{} · held by this account", token.token_id.as_deref().unwrap_or("?")),
                        false => format!("{name} #{} · held by {}", token.token_id.as_deref().unwrap_or("?"), format::short(&Address::Evm(owner[12..].try_into().expect("20 bytes")).to_string())),
                    },
                    _ => format!("{name} #{}", token.token_id.as_deref().unwrap_or("?")),
                },
            };
            let trouble = reads.iter().find_map(|read| read.answer.as_ref().err());
            let proven = reads.iter().all(|read| read.proven.is_some()) && !reads.is_empty();

            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(shown).color(palette.ink));
                ui.label(RichText::new(format!("{} · evm:{}", token.kind.name(), format::short(&token.address))).small().color(palette.muted));

                if theme::quiet(ui, &palette, "Stop showing").clicked() {
                    forget = Some(key(token));
                }
            });

            let note = match (reads.is_empty(), trouble, proven) {
                (true, _, _) => "Reading it…".to_string(),
                (_, Some(why), _) => format!("Not read: {why}"),
                (_, None, true) => format!("Run here over state proven at block {}", reads.iter().filter_map(|read| read.proven).max().unwrap_or(0)),
                (_, None, false) => "The node's word: this build or this read could not run it over proven state".to_string(),
            };
            ui.label(RichText::new(note).small().color(if trouble.is_some() { palette.warn } else { palette.muted }));
            ui.add_space(4.0);
        }
    });

    if let Some(forgotten) = forget {
        ctx.settings.watched.retain(|(_, token)| key(token) != forgotten);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_kind_of_token_is_read_by_its_own_calldata() {
        let token = |kind, id: Option<&str>| Watched { kind, address: format!("0x{}", "12".repeat(20)), symbol: "T".to_string(), decimals: 18, token_id: id.map(str::to_string), image: None };
        let me = [0xab; 20];

        assert_eq!(calldata(&token(AssetKind::Erc20, None), &me), [BALANCE_OF.to_vec(), vec![0; 12], me.to_vec()].concat());
        assert_eq!(calldata(&token(AssetKind::Erc721, Some("7")), &me), [OWNER_OF.to_vec(), word(u256::new(7))].concat());
        assert_eq!(calldata(&token(AssetKind::Erc1155, Some("7")), &me), [BALANCE_OF_ID.to_vec(), vec![0; 12], me.to_vec(), word(u256::new(7))].concat());
        assert_ne!(key(&token(AssetKind::Erc721, Some("7"))), key(&token(AssetKind::Erc721, Some("8"))), "two NFTs of one collection are two tokens");

        // The selectors are Solidity's: keccak256 of each signature's first four bytes.
        for (signature, selector) in [("balanceOf(address)", BALANCE_OF), ("ownerOf(uint256)", OWNER_OF), ("balanceOf(address,uint256)", BALANCE_OF_ID)] {
            assert_eq!(chainv4::crypto::secp256k1::keccak(signature.as_bytes())[..4], selector, "{signature}");
        }
    }

    #[test]
    fn an_answer_is_kept_by_its_token_and_form_and_says_whether_it_was_proven() {
        let mut state = WatchedState::default();
        let body = json!({ "contract": "evm:0x00", "input": "0x", "from": "0xab", "purpose": "watched", "token": "0x12#" }).to_string();
        let answer = json!({ "status": "returned", "output": to_hex(&word(u256::new(5))), "checked": { "here": true, "height": 9 } });

        assert!(state.answered(&Ask::Call(body.clone()), &Ok(answer)));
        let read = &state.read[&("0x12#".to_string(), "0xab".to_string())];
        assert_eq!((read.answer.clone(), read.proven), (Ok(word(u256::new(5))), Some(9)));

        let failed = json!({ "status": "failed", "error": "reverted: nope", "checked": { "here": false } });
        assert!(state.answered(&Ask::Call(body), &Ok(failed)));
        assert!(state.read[&("0x12#".to_string(), "0xab".to_string())].answer.as_ref().unwrap_err().contains("nope"));

        let others = json!({ "contract": "c", "function": "f", "purpose": "nfts" }).to_string();
        assert!(!state.answered(&Ask::Call(others), &Ok(Value::Null)), "someone else's");
    }
}
