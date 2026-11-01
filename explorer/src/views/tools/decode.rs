//! Decode: a raw signed transaction as the chain encodes it, with whether its signature checks on
//! this chain; the wallet's codes and bundles, a swap's terms (ur:chainv4-swap), a multisig's
//! transaction on its way round (chainv4-multisig:…) and an EVM multisig's
//! (chainv4-multisig-evm:…); and EVM calldata and logs, by the ABIs chainv4::abi::known builds in,
//! or one pasted.

use {
    crate::format,
    crate::views::{self, Ctx, View},
    chainv4::abi::Abi,
    chainv4::abi::known::{KnownAbi, read_call, read_log},
    chainv4::abi::multisig::EvmBundle,
    chainv4::registry::multisig::MultisigBundle,
    chainv4::structs::address::Address,
    chainv4::structs::hash::{Hash, HashType},
    chainv4::structs::hex::{from_hex, to_hex},
    chainv4::structs::tx::{FeeOffer, SignedTx, Tx},
    chainv4::swap::{BitcoinParty, Leg, Party, Role, Side, Terms, evm},
    chainv4_ui::widgets,
    egui::{RichText, Ui},
    wincode::{SchemaRead, SchemaWrite, config::DefaultConfig},
};

#[derive(Default)]
pub struct DecodeState {
    pub input: String,
    pub calldata: String,
    /// A log's topics, one to a line, and its data.
    pub topics: String,
    pub data: String,
    /// Whether calldata and logs are read by the ABI pasted rather than the ones built in.
    pub own_abi: bool,
    pub abi: String,
}

/// What a paste was.
#[derive(Clone, Debug, PartialEq)]
pub enum Decoded {
    Tx(Box<SignedTx>),
    Swap(Box<Terms>),
    Bundle(Box<MultisigBundle>),
    EvmBundle(Box<EvmBundle>),
}

/// Decodes a value that is all of these bytes: anything past its encoding is refused.
fn whole<T>(bytes: &[u8], what: &str) -> Result<T, String>
where
    T: for<'de> SchemaRead<'de, DefaultConfig, Dst = T> + SchemaWrite<DefaultConfig, Src = T>,
{
    let value: T = wincode::deserialize(bytes).map_err(|_| format!("not {what}: the bytes do not decode as one"))?;

    match wincode::serialize(&value) {
        Ok(again) if again == bytes => Ok(value),
        _ => Err(format!("{what} with more bytes after it than it holds")),
    }
}

/// Reads whatever was pasted: a raw signed transaction in hex, or one of the wallet's codes.
pub fn decode(text: &str) -> Result<Decoded, String> {
    let text = text.trim();
    let lower = text.to_ascii_lowercase();

    if lower.starts_with("ur:chainv4-swap/") {
        return Terms::from_text(text).map(|terms| Decoded::Swap(Box::new(terms)));
    }

    // The wallet's other codes: a multisig's bundle and a signed transaction, whole, as its
    // airgap.rs writes them.
    if let Some(rest) = lower.strip_prefix("ur:") {
        let kind = rest.split('/').next().unwrap_or_default().to_string();

        if !matches!(kind.as_str(), "chainv4-multisig" | "chainv4-signed") {
            return Err(format!("a {kind} code, which the explorer does not read"));
        }

        let bytes = match ur::decode(&lower).map_err(|err| format!("a code that does not read: {err:?}"))? {
            (ur::ur::Kind::SinglePart, bytes) => bytes,
            (ur::ur::Kind::MultiPart, _) => return Err("one part of an animated code: copy the whole code as text from the wallet".to_string()),
        };

        return match kind.as_str() {
            "chainv4-multisig" => whole::<MultisigBundle>(&bytes, "a multisig bundle").map(|bundle| Decoded::Bundle(Box::new(bundle))),
            "chainv4-signed" => whole::<SignedTx>(&bytes, "a signed transaction").map(|signed| Decoded::Tx(Box::new(signed))),
            other => Err(format!("a {other} code, which the explorer does not read")),
        };
    }

    if lower.starts_with("chainv4-multisig-evm:") {
        return EvmBundle::from_text(text).map(|bundle| Decoded::EvmBundle(Box::new(bundle))).ok_or_else(|| "an EVM multisig bundle that does not decode".to_string());
    }

    if lower.starts_with("chainv4-multisig:") {
        return MultisigBundle::from_text(text).map(|bundle| Decoded::Bundle(Box::new(bundle))).ok_or_else(|| "a multisig bundle that does not decode".to_string());
    }

    match from_hex(text) {
        Some(bytes) if !bytes.is_empty() => whole::<SignedTx>(&bytes, "a signed transaction")
            .map(|signed| Decoded::Tx(Box::new(signed)))
            .map_err(|why| format!("{why}. EVM calldata goes in the box below")),
        _ => Err("paste a raw signed transaction, 0x…; a swap's code, ur:chainv4-swap/…; or a multisig bundle, chainv4-multisig:… or chainv4-multisig-evm:…".to_string()),
    }
}

/// What kind of transaction it is, in the words the node's JSON uses: transfer, contract_call,
/// evm_call and so on.
pub fn kind(tx: &Tx) -> String {
    let debug = format!("{tx:?}");
    let name: String = debug.chars().take_while(char::is_ascii_alphanumeric).collect();
    let mut snake = String::new();

    for (at, letter) in name.chars().enumerate() {
        if letter.is_ascii_uppercase() && at > 0 {
            snake.push('_');
        }

        snake.push(letter.to_ascii_lowercase());
    }

    snake
}

/// Whether a signed transaction's signature checks on this chain, as nodes check it. A signature
/// names its chain, so with none known yet it cannot be said to.
pub fn signature_checks(signed: &SignedTx, genesis: Option<&Hash>) -> Result<(), String> {
    match genesis {
        Some(genesis) => signed.verify_on(genesis).map_err(|why| why.to_string()),
        None => Err("the chain is not known yet, and a signature holds on one chain alone".to_string()),
    }
}

/// A multisig bundle's signatures: each member's key, the account it controls, and whether its
/// signature checks over the transaction on the bundle's chain.
pub fn bundle_signatures(bundle: &MultisigBundle) -> Vec<(String, String, bool)> {
    let bytes = bundle.body.signing_bytes(&bundle.genesis);

    bundle
        .signatures
        .iter()
        .map(|signed| (signed.key.to_string(), signed.key.address().to_string(), signed.key.verify_signature(&bytes, &signed.signature).is_ok()))
        .collect()
}

/// A function's call or an event's log, read by one of the ABIs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Read {
    /// Which ABI read it.
    pub contract: String,
    /// The function's or event's signature.
    pub signature: String,
    /// Its values beside their names: transfer(to = 0x…, value = 5).
    pub shown: String,
}

/// Calldata read by the ABI given, if any, and otherwise the first of those built in that reads it.
pub fn decode_call(kept: Option<&Abi>, known: &[KnownAbi], input: &[u8]) -> Result<Read, String> {
    let selector = input.get(..4).ok_or("calldata starts with a function's 4-byte selector")?;

    read_call(kept, known, input)
        .map(|read| Read { contract: read.by.label(), signature: read.signature.clone(), shown: read.shown() })
        .ok_or_else(|| format!("no ABI here has a function of selector {} that reads this calldata", to_hex(selector)))
}

/// A log read by the ABI given, if any, and otherwise the first of those built in that reads it: an
/// ERC-20's and an ERC-721's Transfer share a topic, and differ in which values are topics.
pub fn decode_log(kept: Option<&Abi>, known: &[KnownAbi], topics: &[[u8; 32]], data: &[u8]) -> Result<Read, String> {
    let first = topics.first().ok_or("a log's first topic names its event")?;

    read_log(kept, known, topics, data)
        .map(|read| Read { contract: read.by.label(), signature: read.signature.clone(), shown: read.shown() })
        .ok_or_else(|| format!("no ABI here has an event of topic {} that reads this log", to_hex(first)))
}

/// A log's topics, 32 bytes of hex each, one to a line or between commas or spaces.
pub fn topics(text: &str) -> Result<Vec<[u8; 32]>, String> {
    text.split(|letter: char| letter.is_whitespace() || letter == ',')
        .filter(|topic| !topic.is_empty())
        .map(|topic| from_hex(topic).and_then(|bytes| bytes.try_into().ok()).ok_or_else(|| format!("{topic} is not a topic: 0x and 64 hex digits")))
        .collect()
}

/// The chain's hash, from its status, else the light client's, else a dev chain's.
fn chain_hash(ctx: &Ctx) -> HashType {
    ctx.store
        .status
        .as_ref()
        .and_then(|status| status.hash.parse().ok())
        .or_else(|| ctx.light.hash())
        .unwrap_or_else(HashType::default_chain)
}

/// The chain's genesis id, once the node said it.
fn genesis(ctx: &Ctx) -> Option<Hash> {
    let status = ctx.store.status.as_ref()?;

    Hash::from_bytes(chain_hash(ctx), &from_hex(&status.genesis)?)
}

/// A fee offer, in words.
fn offer(fee: &FeeOffer) -> String {
    format!(
        "up to {} gas at up to {} units, tip {}",
        format::thousands(&fee.gas_limit.to_string()),
        format::thousands(&fee.max_price.to_string()),
        format::thousands(&fee.tip.to_string())
    )
}

/// What is inside a transaction, every field, as the chain holds it.
fn fields(ui: &mut Ui, tx: &Tx) {
    let mut text = format!("{tx:#?}");

    // A deploy's code is no read: the first of it is enough.
    if text.len() > 6_000 {
        let cut = (0..=6_000).rev().find(|at| text.is_char_boundary(*at)).unwrap_or_default();
        text.truncate(cut);
        text.push_str("\n…");
    }

    egui::CollapsingHeader::new("every field").id_salt(ui.next_auto_id()).show(ui, |ui| {
        let mut shown = text.as_str();
        ui.add(egui::TextEdit::multiline(&mut shown).code_editor().desired_width(f32::INFINITY));
    });
}

pub fn show(ui: &mut Ui, ctx: &mut Ctx, state: &mut DecodeState) {
    let look = ctx.look;
    let width = super::width(ctx);

    views::section(ui, look, "Transaction or code", |ui| {
        ui.add(
            egui::TextEdit::multiline(&mut state.input)
                .hint_text("A raw signed transaction, 0x…; a swap's code, ur:chainv4-swap/…; a multisig bundle, chainv4-multisig:… or chainv4-multisig-evm:…")
                .font(egui::TextStyle::Monospace)
                .desired_rows(3)
                .desired_width(width),
        );

        if state.input.trim().is_empty() {
            return;
        }

        ui.add_space(6.0);

        match decode(&state.input) {
            Ok(Decoded::Tx(signed)) => signed_tx(ui, ctx, &signed),
            Ok(Decoded::Swap(terms)) => swap(ui, ctx, &terms),
            Ok(Decoded::Bundle(bundle)) => multisig_bundle(ui, ctx, &bundle),
            Ok(Decoded::EvmBundle(bundle)) => evm_bundle(ui, ctx, &bundle),
            Err(why) => super::outcome(ui, ctx, &Err(why)),
        }
    });

    views::section(ui, look, "EVM calldata and logs", |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut state.own_abi, false, "chainv4's ABIs");
            ui.selectable_value(&mut state.own_abi, true, "An ABI pasted");
        });

        if state.own_abi {
            ui.add(egui::TextEdit::multiline(&mut state.abi).hint_text("The contract's ABI JSON, as solc or Foundry writes it").font(egui::TextStyle::Monospace).desired_rows(3).desired_width(width));
        } else {
            views::note(ui, look, format!("Built in: {}.", ctx.reader.known().iter().map(|abi| abi.name).collect::<Vec<_>>().join(", ")));
        }

        // The one pasted alone, parsed once, or those built in, which only fit.
        let known = ctx.reader.known();
        let pasted: Result<Option<std::rc::Rc<Abi>>, String> = match state.own_abi {
            true if state.abi.trim().is_empty() => Err("Paste an ABI to read by it.".to_string()),
            true => ctx.reader.abi(&state.abi).map(Some).ok_or_else(|| Abi::parse(&state.abi).err().unwrap_or_default()),
            false => Ok(None),
        };
        let abis = pasted.map(|pasted| {
            let known: &[KnownAbi] = if pasted.is_some() { &[] } else { known };

            (pasted, known)
        });

        ui.add_space(6.0);
        views::facts(ui, look, "evm decode", |facts| {
            facts.row("calldata", |ui| {
                ui.add(egui::TextEdit::singleline(&mut state.calldata).hint_text("0xa9059cbb…").font(egui::TextStyle::Monospace).desired_width(width));
            });

            if !state.calldata.trim().is_empty() {
                let read = abis.clone().and_then(|(pasted, known)| decode_call(pasted.as_deref(), known, &from_hex(state.calldata.trim()).ok_or("calldata is hex: 0x…")?));
                facts.row("calls", |ui| read_row(ui, ctx, &read));
            }

            facts.row("log topics", |ui| {
                ui.add(egui::TextEdit::multiline(&mut state.topics).hint_text("One topic to a line, the event's first").font(egui::TextStyle::Monospace).desired_rows(2).desired_width(width));
            });
            facts.row("log data", |ui| {
                ui.add(egui::TextEdit::singleline(&mut state.data).hint_text("0x…").font(egui::TextStyle::Monospace).desired_width(width));
            });

            if !state.topics.trim().is_empty() {
                let read = abis.clone().and_then(|(pasted, known)| {
                    let data = match state.data.trim() {
                        "" => Vec::new(),
                        data => from_hex(data).ok_or("a log's data is hex: 0x…")?,
                    };

                    decode_log(pasted.as_deref(), known, &topics(&state.topics)?, &data)
                });
                facts.row("emitted", |ui| read_row(ui, ctx, &read));
            }
        });
    });
}

/// A call or a log read, and by which ABI; or why it was not.
fn read_row(ui: &mut Ui, ctx: &Ctx, read: &Result<Read, String>) {
    match read {
        Ok(read) => {
            ui.vertical(|ui| {
                views::copyable(ui, &read.shown);
                ui.label(RichText::new(format!("{} · {}", read.signature, read.contract)).small().color(ctx.look.palette.muted));
            });
        }
        Err(why) => super::outcome(ui, ctx, &Err(why.clone())),
    }
}

/// A signed transaction: its envelope, whether its signature checks, and what it does.
fn signed_tx(ui: &mut Ui, ctx: &mut Ctx, signed: &SignedTx) {
    let look = ctx.look;
    let id = signed.id(chain_hash(ctx)).to_string();
    let chain = genesis(ctx);
    let checks = signature_checks(signed, chain.as_ref());

    ui.horizontal_wrapped(|ui| {
        ui.label(widgets::strong(format!("Transaction · {}", kind(&signed.tx)), 15.0).color(look.palette.ink));

        match &checks {
            Ok(()) => widgets::pill(ui, "signature checks", look.palette.good),
            Err(_) if chain.is_none() => widgets::pill(ui, "chain not known yet", look.palette.muted),
            Err(_) => widgets::pill(ui, "signature does not check", look.palette.bad),
        };
    });

    views::facts(ui, look, "decoded tx", |facts| {
        facts.row("id", |ui| {
            views::copyable(ui, &id);

            if ui.link(RichText::new("its page").color(look.palette.accent)).clicked() {
                ctx.go(View::Tx(id.clone()));
            }
        });
        facts.row("sender", |ui| views::address(ui, ctx, &signed.sender().to_string()));
        facts.text("nonce", signed.nonce.to_string());
        facts.text("offer", offer(&signed.fee));
        facts.row("signer", |ui| views::copyable(ui, &signed.signer.to_string()));
        facts.row("signature", |ui| views::copyable(ui, &signed.signature.to_string()));

        if let Err(why) = &checks {
            facts.row("why", |ui| {
                ui.colored_label(look.palette.bad, why);
            });
        }

        // The calldata, read as the tx page reads it.
        if let Tx::EvmCall { to, input, .. } = &signed.tx {
            let read = decode_call(ctx.abi(&to.to_string()).as_deref(), ctx.reader.known(), input);
            facts.row("calls", |ui| read_row(ui, ctx, &read));
        }
    });

    // Every field, as the tx page shows one: the node's own JSON for it, written here.
    match node_view(signed, chain_hash(ctx)) {
        Some(view) => views::tx::asked(ui, ctx, &view, None),
        None => fields(ui, &signed.tx),
    }
}

/// A signed transaction as the node's JSON writes it, read as the tx page reads one.
pub fn node_view(signed: &SignedTx, hash: chainv4::structs::hash::HashType) -> Option<crate::model::TxView> {
    serde_json::from_value(chainv4::structs::json::signed_tx(signed, hash)).ok()
}

/// Who a leg's party is, as their chain writes them.
fn party(party: Option<Party>) -> String {
    match party {
        None => "not in yet".to_string(),
        Some(Party::Chain(address)) => address.to_string(),
        Some(Party::Evm(address)) => evm::show_address(&address),
        Some(Party::Bitcoin(BitcoinParty::Key(key))) => format!("key {}", to_hex(&key)),
        Some(Party::Bitcoin(BitcoinParty::Multisig { threshold, keys })) => format!("{threshold} of {} keys, a multisig", keys.len()),
    }
}

/// A swap's terms: the hash, and each leg's chain, what it locks, who pays and who claims, until
/// when.
fn swap(ui: &mut Ui, ctx: &mut Ctx, terms: &Terms) {
    let look = ctx.look;

    ui.horizontal_wrapped(|ui| {
        ui.label(widgets::strong("A swap's terms", 15.0).color(look.palette.ink));

        match terms.agreed() {
            true => widgets::pill(ui, "answered", look.palette.good),
            false => widgets::pill(ui, "an offer", look.palette.warn),
        };
    });
    views::facts(ui, look, "swap", |facts| {
        facts.row("hash", |ui| views::copyable(ui, &to_hex(&terms.hash)));

        if let Err(why) = terms.check(ctx.now) {
            facts.row("holds", |ui| {
                ui.colored_label(look.palette.warn, why);
            });
        }
    });

    for (side, leg) in terms.legs() {
        let title = match side {
            Side::First => format!("The starter's lock, on {}", leg.chain_name()),
            Side::Second => format!("The other's lock, on {}", leg.chain_name()),
        };
        let deadline = terms.deadline(side);
        ui.add_space(6.0);
        ui.label(widgets::strong(&title, 14.0).color(look.palette.ink));
        views::facts(ui, look, &format!("decoded leg {side:?}"), |facts| {
            facts.text("locks", leg.what());

            match leg {
                Leg::Chain(chain) => facts.row("in", |ui| views::address(ui, ctx, &chain.contract.to_string())),
                Leg::Evm(on) => facts.text("in", format!("Swap.sol {} on {}", evm::show_address(&on.contract), evm::chain_name(on.chain_id))),
                Leg::Bitcoin(bitcoin) => facts.text("in", terms.htlc().map_or("an HTLC, once both keys are in".to_string(), |htlc| format!("HTLC {}", htlc.address(bitcoin.network())))),
            }

            for (role, name) in [(Role::Payer, "paid by"), (Role::Receiver, "claimed by")] {
                let who = party(leg.party(role));

                facts.row(name, |ui| match views::is_address(&who) {
                    true => views::address(ui, ctx, &who),
                    false => views::copyable(ui, &who),
                });
            }

            facts.text("ends", format!("{} UTC ({})", format::utc(deadline), format::ago(deadline, ctx.now)));
        });
    }
}

/// A multisig's transaction on its way round: the chain, the multisig, what it does, and each
/// signature so far, checked.
fn multisig_bundle(ui: &mut Ui, ctx: &mut Ctx, bundle: &MultisigBundle) {
    let look = ctx.look;
    let here = ctx.store.status.as_ref().map(|status| status.genesis.eq_ignore_ascii_case(&bundle.genesis.to_string()));
    let signatures = bundle_signatures(bundle);

    ui.label(widgets::strong(format!("A multisig's transaction · {}", kind(&bundle.body.tx)), 15.0).color(look.palette.ink));
    views::facts(ui, look, "bundle", |facts| {
        facts.row("chain", |ui| {
            match here {
                Some(true) => widgets::pill(ui, "this chain", look.palette.good),
                Some(false) => widgets::pill(ui, "another chain", look.palette.bad),
                None => widgets::pill(ui, "chain not known yet", look.palette.muted),
            };
            views::copyable(ui, &bundle.genesis.to_string());
        });
        facts.row("multisig", |ui| {
            let account = bundle.body.account.to_string();
            views::address(ui, ctx, &account);

            if matches!(bundle.body.account, Address::Multisig(_)) && ui.link(RichText::new("its members").color(look.palette.accent)).clicked() {
                ctx.go(View::Multisig(account));
            }
        });
        facts.row("id", |ui| views::copyable(ui, &bundle.body.id(&bundle.genesis).to_string()));
        facts.text("nonce", bundle.body.nonce.to_string());
        facts.text("offer", offer(&bundle.body.fee));
        facts.row(&format!("signatures · {}", signatures.len()), |ui| {
            ui.vertical(|ui| {
                if signatures.is_empty() {
                    ui.label(RichText::new("none yet").color(look.palette.muted));
                }

                for (key, account, checks) in &signatures {
                    ui.horizontal_wrapped(|ui| {
                        match checks {
                            true => widgets::pill(ui, "checks", look.palette.good),
                            false => widgets::pill(ui, "does not check", look.palette.bad),
                        }
                        .on_hover_text(key);
                        views::address(ui, ctx, account);
                    });
                }
            });
        });
    });
    fields(ui, &bundle.body.tx);
}

/// An EVM multisig's call on its way round: the chain and contract, the call read by the ABIs, and
/// who signed so far.
fn evm_bundle(ui: &mut Ui, ctx: &mut Ctx, bundle: &EvmBundle) {
    let look = ctx.look;
    let signers = bundle.signers();
    let read = decode_call(None, ctx.reader.known(), &bundle.data);

    ui.label(widgets::strong("An EVM multisig's call", 15.0).color(look.palette.ink));
    views::facts(ui, look, "evm bundle", |facts| {
        facts.text("chain", evm::chain_name(bundle.chain_id));
        facts.row("contract", |ui| views::copyable(ui, &evm::show_address(&bundle.contract)));
        facts.row("calls", |ui| views::copyable(ui, &evm::show_address(&bundle.to)));
        facts.text("value", format!("{} wei", bundle.value));

        if !bundle.data.is_empty() {
            facts.row("data", |ui| read_row(ui, ctx, &read));
        }

        facts.text("nonce", bundle.nonce.to_string());
        facts.row("digest", |ui| views::copyable(ui, &to_hex(&bundle.digest())));
        facts.row(&format!("signed by · {}", signers.len()), |ui| {
            ui.vertical(|ui| {
                for (signer, _) in &signers {
                    views::copyable(ui, &evm::show_address(signer));
                }
            });
        });
    });
}
