//! Atomic swaps with Bitcoin: coins or a token here for bitcoin there, with no one in between.
//! Whoever starts makes an offer, a code holding the terms, the secret's hash and their keys, and
//! keeps the secret in the vault; the other reads it and shows back an answer with their keys.
//! Then the starter locks first, the other second, each under the same hash, and claims go the
//! other way: the starter's claim shows the secret, which the other claims with in turn.
//!
//! chainv4's side goes through the swap contract the node names, as calls the Send page reviews
//! and signs, on the offline signer too. Bitcoin's side is an HTLC: paid from any Bitcoin wallet,
//! and claimed or refunded here, signed with this account's bitcoin key, or by the offline signer
//! as a PSBT. The wallet does not follow Bitcoin: what it needs of it is pasted, or, with an
//! Esplora API set in Settings, looked up and broadcast there.

use {
    super::scan::Scan,
    super::send::{Prepared, SendState},
    super::{Ctx, Page},
    crate::airgap::Message,
    crate::bitcoin::{Network, Psbt, Request},
    crate::format,
    crate::keys::Holder,
    crate::node::Ask,
    crate::theme,
    crate::vault::SwapRecord,
    chainv4::crypto::bitcoin::tx::Transaction,
    chainv4::crypto::bitcoin::{hash160, sha256},
    chainv4::crypto::hasher::Hasher,
    chainv4::crypto::keypair::{PrivateKey, PublicKey},
    chainv4::structs::address::Address,
    chainv4::structs::amount::Amount,
    chainv4::structs::bech32,
    chainv4::structs::hash::HashType,
    chainv4::structs::hex::{from_hex, to_hex},
    chainv4::structs::tx::{Tx, TxCallArgumentType},
    chainv4::structs::u256::u256,
    chainv4::swap::htlc::{Coins, Spend, revealed},
    chainv4::swap::{Counterparty, FIRST_DEADLINE_MS, Pays, SECOND_DEADLINE_MS, Terms, secret},
    egui::{RichText, Ui},
    serde_json::{Value, json},
    std::collections::HashMap,
    std::sync::{Arc, Mutex},
};

/// How often an open swap asks the contract where it stands.
const ASK_EVERY: f64 = 10.0;

pub struct SwapsState {
    open: Option<usize>,
    form: Form,
    scan: Scan,
    /// An offer read, to answer.
    offered: Option<Box<Terms>>,
    /// What the contract said of each swap, by its hash, and when it was last asked.
    status: HashMap<String, Result<Vec<Value>, String>>,
    asked: HashMap<String, f64>,
    bitcoin: BitcoinForm,
    trouble: Option<String>,
    /// What came back from Esplora, for the next frame to take.
    looked: Arc<Mutex<Vec<Looked>>>,
    /// A Bitcoin spend asked of the offline signer.
    signer: Option<Box<Awaiting>>,
    /// Forgetting the open swap, asked once and waiting for the second tap.
    forgetting: bool,
}

impl Default for SwapsState {
    fn default() -> SwapsState {
        SwapsState {
            open: None,
            form: Form::default(),
            scan: Scan::default(),
            offered: None,
            status: HashMap::new(),
            asked: HashMap::new(),
            bitcoin: BitcoinForm { fee_rate: "2".to_string(), ..BitcoinForm::default() },
            trouble: None,
            looked: Arc::default(),
            signer: None,
            forgetting: false,
        }
    }
}

struct Form {
    pay_bitcoin: bool,
    testnet: bool,
    btc: String,
    amount: String,
    token: String,
    first: String,
    second: String,
}

impl Default for Form {
    fn default() -> Form {
        Form {
            pay_bitcoin: true,
            testnet: true,
            btc: String::new(),
            amount: String::new(),
            token: String::new(),
            first: (FIRST_DEADLINE_MS / 3_600_000).to_string(),
            second: (SECOND_DEADLINE_MS / 3_600_000).to_string(),
        }
    }
}

#[derive(Default)]
struct BitcoinForm {
    funding: String,
    sats: String,
    to: String,
    fee_rate: String,
    claim_tx: String,
    /// The signed spend, in hex, to broadcast.
    signed: Option<String>,
}

/// A spend the offline signer was asked to sign: what to finish once its PSBT comes back.
struct Awaiting {
    request: Box<Request>,
    spend: Spend,
    value: u64,
    scan: Scan,
}

enum Looked {
    Funding { hash: String, outpoint: String, sats: u64 },
    Secret { hash: String, secret: [u8; 32] },
    Broadcast(Result<String, String>),
    Failed(String),
}

impl SwapsState {
    /// Takes the contract's answer about a swap; false when the answer is for someone else.
    pub fn answered(&mut self, ask: &Ask, result: &Result<Value, crate::node::Failure>) -> bool {
        let Ask::Call(body) = ask else {
            return false;
        };
        let Ok(body) = serde_json::from_str::<Value>(body) else {
            return false;
        };
        let hash = body["args"][0]["value"].as_str().unwrap_or_default().to_string();

        if body["function"] != "get" || !self.asked.contains_key(&hash) {
            return false;
        }

        let answer = match result {
            Ok(value) if value["status"] == "returned" => Ok(value["values"].as_array().cloned().unwrap_or_default()),
            Ok(value) => Err(value["error"].as_str().unwrap_or("the contract would not say").to_string()),
            Err(failure) => Err(failure.message.clone()),
        };
        self.status.insert(hash, answer);

        true
    }
}

pub fn show(ui: &mut Ui, ctx: &mut Ctx, state: &mut SwapsState, send: &mut SendState) {
    let palette = *ctx.palette;
    theme::title(ui, &palette, "Swaps", "Coins or a token here for bitcoin there, with no one in between: each side locks under one secret's hash.");

    take_looked(ctx, state);
    let contract = contract(ctx);

    if contract.is_none() {
        theme::card(ui, &palette, |ui| {
            ui.colored_label(palette.warn, "This node names no swap contract. Set one under Settings, Swaps, to lock and claim on this chain.");
        });
        ui.add_space(14.0);
    }

    list(ui, ctx, state);

    if let Some(index) = state.open.filter(|index| *index < ctx.book.swaps.len()) {
        ui.add_space(14.0);

        if let Some(prepared) = detail(ui, ctx, state, index, contract) {
            send.prepare(prepared);
            *ctx.page = Page::Send;
        }
    }

    ui.add_space(14.0);
    start(ui, ctx, state);
    ui.add_space(14.0);
    read(ui, ctx, state);
}

/// The swap contract: Settings', or the node's.
fn contract(ctx: &Ctx) -> Option<Address> {
    match ctx.settings.swap_contract.trim() {
        "" => ctx.swap_contract.and_then(|text| text.parse().ok()),
        text => text.parse().ok(),
    }
}

fn list(ui: &mut Ui, ctx: &mut Ctx, state: &mut SwapsState) {
    let palette = *ctx.palette;

    if ctx.book.swaps.is_empty() {
        return;
    }

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Your swaps");

        for (index, record) in ctx.book.swaps.iter().enumerate() {
            let line = match Terms::from_text(&record.terms) {
                Ok(terms) => format!(
                    "{} · {} {} for {} · {}",
                    if record.started { "started" } else { "answered" },
                    format::units(&terms.sats.to_string(), 8),
                    terms.network().symbol(),
                    terms.amount,
                    if terms.agreed() { "agreed" } else { "waiting for an answer" }
                ),
                Err(why) => why,
            };

            if ui.selectable_label(state.open == Some(index), RichText::new(line).color(palette.ink)).clicked() {
                state.open = Some(index);
                state.bitcoin = BitcoinForm { fee_rate: "2".to_string(), ..BitcoinForm::default() };
                state.trouble = None;
                state.forgetting = false;
            }
        }
    });
}

/// The account in use's keys for a swap: its bitcoin key on the network, and its chainv4 address.
fn keys(ctx: &Ctx, holder: &Holder, network: Network) -> Result<([u8; 33], Address), String> {
    let address = *holder.addresses().first().ok_or("an account with no address")?;
    let key = match network {
        Network::Main => holder.publics.iter().find_map(|public| match public {
            PublicKey::Bitcoin(bytes) => Some(*bytes),
            _ => None,
        }),
        Network::Test => match (crate::keys::seed(ctx.book), holder.number()) {
            (Some(seed), Some(number)) => match seed.bitcoin_key(network.coin(), 0, number).public() {
                PublicKey::Bitcoin(bytes) => Some(bytes),
                _ => None,
            },
            _ => None,
        },
    };

    match key {
        Some(key) => Ok((key, address)),
        None if network == Network::Test => Err("Testnet swaps take an account of this wallet's words: its testnet key is theirs alone.".to_string()),
        None => Err("This account has no bitcoin key: pair it again from the signer, or pick another.".to_string()),
    }
}

/// The account's bitcoin key's secret on the network, when this wallet holds it.
fn secret_key(ctx: &Ctx, holder: &Holder, network: Network) -> Option<[u8; 32]> {
    match network {
        Network::Main => holder.keys.iter().find_map(|key| match key.private() {
            PrivateKey::Bitcoin(secret) => Some(*secret),
            _ => None,
        }),
        Network::Test => {
            let (seed, number) = (crate::keys::seed(ctx.book)?, holder.number()?);
            let key = seed.bitcoin_key(network.coin(), 0, number);

            match key.private() {
                PrivateKey::Bitcoin(secret) => Some(*secret),
                _ => None,
            }
        }
    }
}

fn start(ui: &mut Ui, ctx: &mut Ctx, state: &mut SwapsState) {
    let palette = *ctx.palette;
    let coin = ctx.coin.clone();

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Start a swap");
        ui.label(RichText::new("You make the secret and lock first, with the longer deadline. Send the offer's code to the other side, and read their answer below.").color(palette.soft));
        ui.add_space(8.0);
        let form = &mut state.form;

        ui.horizontal(|ui| {
            ui.selectable_value(&mut form.pay_bitcoin, true, "I pay bitcoin");
            ui.selectable_value(&mut form.pay_bitcoin, false, "I pay on this chain");
        });
        ui.horizontal(|ui| {
            ui.selectable_value(&mut form.testnet, false, "Mainnet");
            ui.selectable_value(&mut form.testnet, true, "Testnet");
        });
        let network = if form.testnet { Network::Test } else { Network::Main };
        super::row(ui, &palette, network.symbol(), |ui| {
            ui.add(egui::TextEdit::singleline(&mut form.btc).hint_text("0.001").desired_width(160.0));
        });
        super::row(ui, &palette, if form.token.trim().is_empty() { &coin.symbol } else { "Token units" }, |ui| {
            ui.add(egui::TextEdit::singleline(&mut form.amount).hint_text("50").desired_width(160.0));
        });
        super::row(ui, &palette, "Token", |ui| {
            ui.add(egui::TextEdit::singleline(&mut form.token).hint_text("empty for the coin, or its contract:0x…").desired_width(f32::INFINITY));
        });
        super::row(ui, &palette, "Deadlines", |ui| {
            ui.add(egui::TextEdit::singleline(&mut form.first).desired_width(40.0));
            ui.label("h for yours,");
            ui.add(egui::TextEdit::singleline(&mut form.second).desired_width(40.0));
            ui.label("h for theirs");
        });
        ui.add_space(6.0);

        if theme::primary(ui, &palette, "Make the offer", true).clicked() {
            match offer(ctx, &state.form) {
                Ok(record) => {
                    ctx.book.swaps.push(record);
                    *ctx.changed = true;
                    state.open = Some(ctx.book.swaps.len() - 1);
                    state.trouble = None;
                }
                Err(why) => state.trouble = Some(why),
            }
        }

        if let Some(trouble) = &state.trouble
            && state.open.is_none()
        {
            ui.colored_label(palette.bad, trouble);
        }
    });
}

/// An offer from the form, with a new secret kept beside it.
fn offer(ctx: &Ctx, form: &Form) -> Result<SwapRecord, String> {
    let holder = ctx.holder().ok_or("pick an account first")?;
    let network = if form.testnet { Network::Test } else { Network::Main };
    let (key, address) = keys(ctx, holder, network)?;
    let sats: u64 = format::parse_units(&form.btc, 8).and_then(|digits| digits.parse().ok()).ok_or("an amount of bitcoin, such as 0.001")?;
    let token = match form.token.trim() {
        "" => None,
        text => Some(text.parse::<Address>().map_err(|_| "the token is a contract's address".to_string())?),
    };
    let decimals = if token.is_some() { 0 } else { ctx.coin.decimals };
    let amount = format::parse_units(&form.amount, decimals).and_then(|digits| u256::from_str_radix(&digits, 10).ok()).ok_or("an amount to lock on this chain")?;
    let hours = |text: &str| text.trim().parse::<u64>().map(|hours| hours * 3_600_000).map_err(|_| "deadlines are whole hours".to_string());
    let (first, second) = (hours(&form.first)?, hours(&form.second)?);
    let (secret, hash) = secret()?;
    let starter = if form.pay_bitcoin { Pays::Bitcoin } else { Pays::Chain };
    let terms = Terms {
        chain: if form.testnet { Counterparty::BitcoinTestnet } else { Counterparty::Bitcoin },
        hash,
        sats,
        token,
        amount,
        starter,
        bitcoin_payer: (starter == Pays::Bitcoin).then_some(key),
        bitcoin_receiver: (starter == Pays::Chain).then_some(key),
        chain_payer: (starter == Pays::Chain).then_some(address),
        chain_receiver: (starter == Pays::Bitcoin).then_some(address),
        first_deadline: ctx.now + first,
        second_deadline: ctx.now + second,
    };
    terms.check(ctx.now)?;

    Ok(SwapRecord { terms: terms.to_text(), secret: Some(to_hex(&secret)), account: holder.id(), made: ctx.now, started: true, funding: None })
}

fn read(ui: &mut Ui, ctx: &mut Ctx, state: &mut SwapsState) {
    let palette = *ctx.palette;

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Read an offer or an answer");
        ui.label(RichText::new("An offer someone sent you, to answer; or the answer to one of yours, which completes its terms.").color(palette.soft));
        ui.add_space(8.0);

        match state.scan.show(ui, &palette) {
            Some(Message::Swap(terms)) => match ctx.book.swaps.iter().position(|record| Terms::from_text(&record.terms).is_ok_and(|kept| kept.hash == terms.hash)) {
                // The answer to one of ours: the same terms, with their keys in.
                Some(index) if terms.agreed() => {
                    let kept = Terms::from_text(&ctx.book.swaps[index].terms).expect("kept terms read");

                    match answers(&kept, &terms) {
                        true => {
                            ctx.book.swaps[index].terms = terms.to_text();
                            *ctx.changed = true;
                            state.open = Some(index);
                        }
                        false => state.scan.fail("That answer changes the terms you offered: it is refused.".to_string()),
                    }
                }
                Some(index) => state.open = Some(index),
                None if terms.agreed() => state.scan.fail("An answer to an offer this wallet did not make.".to_string()),
                None => state.offered = Some(terms),
            },
            Some(other) => state.scan.refuse(&other, "a swap's offer or answer"),
            None => {}
        }

        let Some(terms) = state.offered.clone() else {
            return;
        };

        ui.add_space(8.0);
        summary(ui, ctx, &terms);
        ui.horizontal(|ui| {
            if theme::primary(ui, &palette, "Answer it with this account", true).clicked() {
                match answer(ctx, &terms) {
                    Ok(record) => {
                        ctx.book.swaps.push(record);
                        *ctx.changed = true;
                        state.open = Some(ctx.book.swaps.len() - 1);
                        state.offered = None;
                    }
                    Err(why) => state.scan.fail(why),
                }
            }

            if theme::quiet(ui, &palette, "Not now").clicked() {
                state.offered = None;
            }
        });
    });
}

/// Whether terms answer an offer: everything the offer said, as it said it, the other side's keys
/// added and nothing else.
fn answers(offer: &Terms, answer: &Terms) -> bool {
    let kept = |given: Option<[u8; 33]>, now: Option<[u8; 33]>| given.is_none() || given == now;
    let kept_address = |given: Option<Address>, now: Option<Address>| given.is_none() || given == now;
    let same = (offer.chain, offer.hash, offer.sats, offer.token, offer.amount, offer.starter, offer.first_deadline, offer.second_deadline)
        == (answer.chain, answer.hash, answer.sats, answer.token, answer.amount, answer.starter, answer.first_deadline, answer.second_deadline);

    same && answer.agreed()
        && kept(offer.bitcoin_payer, answer.bitcoin_payer)
        && kept(offer.bitcoin_receiver, answer.bitcoin_receiver)
        && kept_address(offer.chain_payer, answer.chain_payer)
        && kept_address(offer.chain_receiver, answer.chain_receiver)
}

/// The answer to an offer: its terms with this account's keys in.
fn answer(ctx: &Ctx, offered: &Terms) -> Result<SwapRecord, String> {
    let holder = ctx.holder().ok_or("pick an account first")?;
    let (key, address) = keys(ctx, holder, offered.network())?;
    let mut terms = offered.clone();

    match terms.starter {
        Pays::Bitcoin => (terms.bitcoin_receiver, terms.chain_payer) = (Some(key), Some(address)),
        Pays::Chain => (terms.bitcoin_payer, terms.chain_receiver) = (Some(key), Some(address)),
    }

    terms.check(ctx.now)?;

    Ok(SwapRecord { terms: terms.to_text(), secret: None, account: holder.id(), made: ctx.now, started: false, funding: None })
}

/// The terms, laid out.
fn summary(ui: &mut Ui, ctx: &Ctx, terms: &Terms) {
    let palette = *ctx.palette;
    let network = terms.network();
    let asset = match terms.token {
        None => format::amount(&terms.amount.to_string(), ctx.coin.decimals, &ctx.coin.symbol),
        Some(token) => format!("{} units of {}", terms.amount, format::short(&token.to_string())),
    };
    theme::fitted(ui, &format!("{} {} ⇄ {asset}", format::units(&terms.sats.to_string(), 8), network.symbol()), 26.0, 14.0, egui::FontFamily::Name(theme::SERIF.into()), palette.ink);
    super::row(ui, &palette, "Starter pays", |ui| {
        ui.label(match terms.starter {
            Pays::Bitcoin => format!("{}, locked first", network.name()),
            Pays::Chain => "on this chain, locked first".to_string(),
        });
    });
    super::row(ui, &palette, "Bitcoin lock", |ui| {
        ui.label(format!("ends {}", when(terms.bitcoin_deadline(), ctx.now)));
    });
    super::row(ui, &palette, "This chain's", |ui| {
        ui.label(format!("ends {}", when(terms.chain_deadline(), ctx.now)));
    });
    super::row(ui, &palette, "Hash", |ui| {
        ui.label(theme::mono(format::short(&to_hex(&terms.hash))));
    });
}

/// A time, from now: in 11 h 58 min, or 2 h ago.
fn when(time: u64, now: u64) -> String {
    let (gap, ahead) = (time.abs_diff(now) / 60_000, time > now);
    let text = match gap {
        0..60 => format!("{gap} min"),
        _ => format!("{} h {} min", gap / 60, gap % 60),
    };

    match ahead {
        true => format!("in {text}"),
        false => format!("{text} ago"),
    }
}

/// The open swap: what to share, where each side stands, and what to do next.
fn detail(ui: &mut Ui, ctx: &mut Ctx, state: &mut SwapsState, index: usize, contract: Option<Address>) -> Option<Prepared> {
    let palette = *ctx.palette;
    let record = ctx.book.swaps[index].clone();
    let terms = Terms::from_text(&record.terms).ok()?;
    let hash = to_hex(&terms.hash);
    let mut prepared = None;

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, if record.started { "A swap you started" } else { "A swap you answered" });
        summary(ui, ctx, &terms);
        ui.add_space(8.0);

        // What the other side needs to see.
        if !terms.agreed() || !record.started {
            ui.label(RichText::new(match terms.agreed() {
                false => "Send this offer to the other side. Once they answer, read their answer here.",
                true => "Show this answer to who offered: their wallet completes the terms with it.",
            }).color(palette.soft));
            super::code(ui, &palette, &Message::Swap(Box::new(terms.clone())));
            ui.add_space(8.0);
        }

        if !terms.agreed() {
            return;
        }

        let Some(holder) = ctx.holders.iter().position(|holder| holder.id() == record.account) else {
            ui.colored_label(palette.bad, "The account this swap uses is gone from this wallet.");

            return;
        };
        let Ok((my_bitcoin, my_address)) = keys(ctx, &ctx.holders[holder], terms.network()) else {
            ui.colored_label(palette.bad, "This account's keys are not this swap's.");

            return;
        };
        let (pays_bitcoin, gets_bitcoin) = (terms.bitcoin_payer == Some(my_bitcoin), terms.bitcoin_receiver == Some(my_bitcoin));
        let (pays_chain, gets_chain) = (terms.chain_payer == Some(my_address), terms.chain_receiver == Some(my_address));
        let first = record.started;

        // Where chainv4's side stands, asked now and then.
        if let Some(contract) = contract
            && state.asked.get(&hash).is_none_or(|asked| ui.input(|input| input.time) - asked > ASK_EVERY)
        {
            state.asked.insert(hash.clone(), ui.input(|input| input.time));
            ctx.node.ask(Ask::Call(json!({ "contract": contract.to_string(), "function": "get", "args": [{ "type": "string", "value": hash }] }).to_string()));
        }

        let chain = chain_state(state.status.get(&hash));
        super::row(ui, &palette, "This chain", |ui| {
            ui.label(chain.describe());
        });

        // A secret the other side showed: on chainv4 by its claim.
        if record.secret.is_none()
            && let ChainState::Claimed { secret } = &chain
            && let Some(secret) = from_hex(secret).filter(|bytes| sha256(bytes) == terms.hash)
        {
            ctx.book.swaps[index].secret = Some(to_hex(&secret));
            *ctx.changed = true;
        }

        let secret = record.secret.as_deref().and_then(from_hex).and_then(|bytes| <[u8; 32]>::try_from(bytes).ok());
        let htlc = terms.htlc().expect("agreed terms have their HTLC");
        let address = htlc.address(terms.network());
        super::row(ui, &palette, "Bitcoin HTLC", |ui| {
            ui.vertical(|ui| {
                super::address(ui, &palette, &address);
                ui.label(RichText::new(format!("{} {}", format::units(&terms.sats.to_string(), 8), terms.network().symbol())).small().color(palette.muted));
            });
        });
        ui.add_space(6.0);

        // Locking: the starter first; the other only once the first lock stands.
        if pays_bitcoin {
            ui.label(RichText::new(match first {
                true => "Pay the HTLC exactly this from your Bitcoin wallet: you lock first.",
                false => "Once this chain shows their lock, for you, with this amount and deadline, pay the HTLC this from your Bitcoin wallet.",
            }).color(palette.ink));
            let uri = format!("bitcoin:{address}?amount={}", format::units(&terms.sats.to_string(), 8));
            let side = ui.available_width().clamp(180.0, 300.0);
            ui.vertical_centered(|ui| super::receive::qr(ui, &uri, side));
        }

        if pays_chain
            && matches!(chain, ChainState::None)
            && let Some(contract) = contract
        {
            {
                ui.label(RichText::new(match first {
                    true => "Lock what you pay on this chain: you lock first.",
                    false => "Lock on this chain only once the HTLC above holds the bitcoin, confirmed: check it on a Bitcoin explorer, or look it up below.",
                }).color(palette.ink));
                ui.horizontal(|ui| {
                    let recipient = TxCallArgumentType::Address(terms.chain_receiver.expect("agreed"));
                    let deadline = TxCallArgumentType::I64(terms.chain_deadline() as i64);
                    let text = TxCallArgumentType::String(hash.clone());

                    match terms.token {
                        None => {
                            if theme::primary(ui, &palette, "Lock", true).clicked() {
                                prepared = Some(call(my_address, contract, "lock", vec![text, recipient, deadline], terms.amount, "Lock a swap".to_string()));
                            }
                        }
                        Some(token) => {
                            if theme::quiet(ui, &palette, "1. Let the contract move the token").clicked() {
                                prepared = Some(call(my_address, token, "approve", vec![TxCallArgumentType::Address(contract), TxCallArgumentType::U256(terms.amount)], u256::ZERO, "Approve the swap contract".to_string()));
                            }

                            if theme::primary(ui, &palette, "2. Lock", true).clicked() {
                                prepared = Some(call(my_address, contract, "lock_token", vec![text, recipient, deadline, TxCallArgumentType::Address(token), TxCallArgumentType::U256(terms.amount)], u256::ZERO, "Lock a swap's token".to_string()));
                            }
                        }
                    }
                });
            }
        }

        // Claiming on this chain, with the secret.
        if gets_chain && matches!(chain, ChainState::Locked) {
            match (secret, contract) {
                (Some(secret), Some(contract)) => {
                    ui.label(RichText::new(match first {
                        true => "Their lock stands: claim it. That shows the secret, which they claim your bitcoin with.",
                        false => "Claim what they locked for you, with the secret they showed.",
                    }).color(palette.ink));

                    if theme::primary(ui, &palette, "Claim on this chain", ctx.now < terms.chain_deadline()).clicked() {
                        prepared = Some(call(my_address, contract, "claim", vec![TxCallArgumentType::String(to_hex(&secret))], u256::ZERO, "Claim a swap".to_string()));
                    }
                }
                (None, _) => learn_secret(ui, ctx, state, index, &terms),
                (_, None) => {}
            }
        }

        // Taking back what was locked on this chain, after its deadline.
        if pays_chain
            && matches!(chain, ChainState::Locked)
            && ctx.now >= terms.chain_deadline()
            && let Some(contract) = contract
            && theme::quiet(ui, &palette, "Take back your lock on this chain").clicked()
        {
            prepared = Some(call(my_address, contract, "refund", vec![TxCallArgumentType::String(hash.clone())], u256::ZERO, "Refund a swap".to_string()));
        }

        // Bitcoin's side: a claim with the secret, or a refund after the lock time.
        let spend = match (gets_bitcoin, pays_bitcoin, secret) {
            (true, _, Some(secret)) => Some(Spend::Claim { secret }),
            (_, true, _) if ctx.now / 1_000 >= u64::from(htlc.locktime) => Some(Spend::Refund),
            _ => None,
        };

        if gets_bitcoin && secret.is_none() {
            ui.label(RichText::new("You claim the bitcoin once they show the secret, by claiming on this chain: it shows here then.").small().color(palette.muted));
        }

        if let Some(spend) = spend {
            ui.add_space(8.0);
            bitcoin_spend(ui, ctx, state, &terms, holder, spend, index);
        }

        if let Some(trouble) = &state.trouble {
            ui.colored_label(palette.bad, trouble);
        }

        ui.add_space(10.0);
        let label = if state.forgetting { "Forget it: its secret goes too" } else { "Forget this swap" };

        if theme::quiet(ui, &palette, label).clicked() {
            match state.forgetting {
                true => {
                    ctx.book.swaps.remove(index);
                    *ctx.changed = true;
                    state.open = None;
                    state.forgetting = false;
                }
                false => state.forgetting = true,
            }
        }
    });

    prepared
}

/// What the contract said of a swap.
enum ChainState {
    Unknown(String),
    None,
    Locked,
    Claimed { secret: String },
    Refunded,
}

impl ChainState {
    fn describe(&self) -> String {
        match self {
            ChainState::Unknown(why) => format!("not known yet: {why}"),
            ChainState::None => "nothing locked yet".to_string(),
            ChainState::Locked => "locked, waiting for its claim".to_string(),
            ChainState::Claimed { .. } => "claimed: the secret is out".to_string(),
            ChainState::Refunded => "refunded".to_string(),
        }
    }
}

fn chain_state(answer: Option<&Result<Vec<Value>, String>>) -> ChainState {
    let text = |value: &Value| value["value"].as_str().map_or_else(|| value["value"].to_string(), str::to_string);

    match answer {
        None => ChainState::Unknown("asking".to_string()),
        Some(Err(why)) => ChainState::Unknown(why.clone()),
        Some(Ok(values)) => match values.first().map(text).as_deref() {
            Some("none") => ChainState::None,
            Some("locked") => ChainState::Locked,
            Some("claimed") => ChainState::Claimed { secret: values.get(6).map(text).unwrap_or_default() },
            Some("refunded") => ChainState::Refunded,
            _ => ChainState::Unknown("an answer this wallet does not read".to_string()),
        },
    }
}

/// The secret, from the other side's claim of the bitcoin: pasted, or looked up on Esplora.
fn learn_secret(ui: &mut Ui, ctx: &mut Ctx, state: &mut SwapsState, index: usize, terms: &Terms) {
    let palette = *ctx.palette;
    ui.label(RichText::new("They claim your bitcoin first, which shows the secret on Bitcoin. Paste their claim's transaction, in hex, to read it.").color(palette.soft));
    ui.add(egui::TextEdit::multiline(&mut state.bitcoin.claim_tx).hint_text("02000000000101…").desired_rows(2).desired_width(f32::INFINITY));
    ui.horizontal(|ui| {
        if theme::quiet(ui, &palette, "Read the secret").clicked() {
            let read = from_hex(&format!("0x{}", state.bitcoin.claim_tx.trim().trim_start_matches("0x")))
                .ok_or("not hex".to_string())
                .and_then(|bytes| Transaction::read(&bytes))
                .and_then(|(tx, _)| revealed(&tx, &terms.hash).ok_or("that transaction shows no secret of this swap's".to_string()));

            match read {
                Ok(secret) => {
                    ctx.book.swaps[index].secret = Some(to_hex(&secret));
                    *ctx.changed = true;
                    state.trouble = None;
                }
                Err(why) => state.trouble = Some(why),
            }
        }

        if !ctx.settings.esplora.trim().is_empty() && theme::quiet(ui, &palette, "Look it up on Esplora").clicked() {
            look_for_secret(ctx, state, terms);
        }
    });
}

/// Claiming or refunding the HTLC: the coins it holds, where they go, and the fee; signed here,
/// or by the offline signer.
fn bitcoin_spend(ui: &mut Ui, ctx: &mut Ctx, state: &mut SwapsState, terms: &Terms, holder: usize, spend: Spend, index: usize) {
    let palette = *ctx.palette;
    let network = terms.network();
    let htlc = terms.htlc().expect("agreed terms have their HTLC");
    let hash = to_hex(&terms.hash);
    theme::caps(ui, &palette, match spend {
        Spend::Claim { .. } => "Claim the bitcoin",
        Spend::Refund => "Take back your bitcoin",
    });

    if state.bitcoin.funding.is_empty()
        && let Some(funding) = &ctx.book.swaps[index].funding
    {
        let mut parts = funding.rsplitn(2, ':');
        let (sats, outpoint) = (parts.next().unwrap_or_default(), parts.next().unwrap_or_default());
        (state.bitcoin.funding, state.bitcoin.sats) = (outpoint.to_string(), sats.to_string());
    }

    if state.bitcoin.to.is_empty()
        && let Ok((key, _)) = keys(ctx, &ctx.holders[holder], network)
    {
        state.bitcoin.to = bech32::encode_segwit(network.hrp(), &hash160(&key));
    }

    super::row(ui, &palette, "Its coins", |ui| {
        ui.add(egui::TextEdit::singleline(&mut state.bitcoin.funding).hint_text("txid:vout").desired_width(260.0));
        ui.add(egui::TextEdit::singleline(&mut state.bitcoin.sats).hint_text("sats").desired_width(90.0));
    });
    super::row(ui, &palette, "To", |ui| {
        ui.add(egui::TextEdit::singleline(&mut state.bitcoin.to).desired_width(f32::INFINITY));
    });
    super::row(ui, &palette, "Fee", |ui| {
        ui.add(egui::TextEdit::singleline(&mut state.bitcoin.fee_rate).desired_width(50.0));
        ui.label("sat/vB");
    });

    if !ctx.settings.esplora.trim().is_empty() && theme::quiet(ui, &palette, "Look its coins up on Esplora").clicked() {
        look_for_funding(ctx, state, &htlc.address(network), &hash);
    }

    // The offline signer's answer, when it was asked.
    if let Some(awaiting) = state.signer.as_mut() {
        ui.label(RichText::new("On the offline signer, Sign, and read this; then read its answer here.").color(palette.soft));
        super::code(ui, &palette, &Message::Bitcoin(awaiting.request.clone()));

        match awaiting.scan.show(ui, &palette) {
            Some(Message::Bitcoin(signed)) => {
                match signed.psbt.finish_htlc(&htlc, awaiting.value, awaiting.spend) {
                    Ok(tx) => state.bitcoin.signed = Some(to_hex(&tx.encode())[2..].to_string()),
                    Err(why) => state.trouble = Some(why),
                }
                state.signer = None;
            }
            Some(other) => awaiting.scan.refuse(&other, "the signer's PSBT"),
            None => {}
        }

        return;
    }

    if theme::primary(ui, &palette, "Sign it", true).clicked() {
        let account = &ctx.holders[holder];

        match sign(secret_key(ctx, account, network), account.origin(), terms, spend, &state.bitcoin) {
            Ok(Signed::Here(tx)) => {
                state.bitcoin.signed = Some(to_hex(&tx.encode())[2..].to_string());
                state.trouble = None;
            }
            Ok(Signed::Signer(awaiting)) => state.signer = Some(awaiting),
            Err(why) => state.trouble = Some(why),
        }

        // What was typed is kept with the swap, for its refund or a second look.
        let funding = format!("{}:{}", state.bitcoin.funding.trim(), state.bitcoin.sats.trim());
        if ctx.book.swaps[index].funding.as_deref() != Some(funding.as_str()) {
            ctx.book.swaps[index].funding = Some(funding);
            *ctx.changed = true;
        }
    }

    if let Some(signed) = state.bitcoin.signed.clone() {
        ui.label(RichText::new(match spend {
            Spend::Claim { .. } => "Signed. Broadcast it with any Bitcoin node or explorer.",
            Spend::Refund => "Signed. Bitcoin mines it once its median time is past the lock time.",
        }).color(palette.soft));
        super::text_block(ui, &palette, &signed);
        ui.horizontal(|ui| {
            if theme::quiet(ui, &palette, "Copy").clicked() {
                ui.ctx().copy_text(signed.clone());
            }

            if !ctx.settings.esplora.trim().is_empty() && theme::quiet(ui, &palette, "Broadcast on Esplora").clicked() {
                broadcast(ctx, state, signed.clone());
            }
        });
    }
}

enum Signed {
    Here(Transaction),
    Signer(Box<Awaiting>),
}

/// Signs a spend: with the account's key, when this wallet holds it, or else as a PSBT for the
/// offline signer, which the pairing said where it derives the key.
fn sign(secret: Option<[u8; 32]>, origin: Option<crate::airgap::Origin>, terms: &Terms, spend: Spend, form: &BitcoinForm) -> Result<Signed, String> {
    let network = terms.network();
    let htlc = terms.htlc().ok_or("no HTLC yet")?;
    let (txid, vout) = form.funding.trim().split_once(':').ok_or("its coins are txid:vout")?;
    let mut txid: [u8; 32] = from_hex(&format!("0x{txid}")).and_then(|bytes| bytes.try_into().ok()).ok_or("a txid is 64 hex digits")?;
    // Explorers write a txid backwards from how a transaction names it.
    txid.reverse();
    let vout: u32 = vout.parse().map_err(|_| "a vout is a number")?;
    let value: u64 = form.sats.trim().parse().map_err(|_| "what the coins hold, in satoshis")?;
    let fee_rate: u64 = form.fee_rate.trim().parse().map_err(|_| "a fee rate in sat/vB")?;
    let (hrp, program) = bech32::decode_segwit(form.to.trim()).ok_or("send to a native segwit address, bc1q… or tb1q…")?;

    if hrp != network.hrp() {
        return Err(format!("that address is not on {}", network.name()));
    }

    let to = [&[0x00, program.len() as u8][..], &program].concat();

    if let Some(secret) = secret {
        return htlc.spend(Coins { txid, vout, value }, to, fee_rate, spend, &secret).map(Signed::Here);
    }

    // Watched from the signer: it signs by the account's place under its words.
    let origin = origin.ok_or("this account's keys are on a signer that did not say where: pair it again")?;
    let key = match spend {
        Spend::Claim { .. } => htlc.claim,
        Spend::Refund => htlc.refund,
    };
    let tx = htlc.unsigned(Coins { txid, vout, value }, to, fee_rate, spend)?;
    let hardened = 1 << 31;
    let path = [84 | hardened, network.coin() | hardened, hardened, 0, origin.account];
    let psbt = Psbt::for_htlc(&tx, value, &htlc, key, origin.fingerprint, &path);

    Ok(Signed::Signer(Box::new(Awaiting { request: Box::new(Request { psbt, kind: crate::bitcoin::PSBT_TYPE }), spend, value, scan: Scan::default() })))
}

/// A call to a contract, for the Send page to review and sign.
fn call(from: Address, to: Address, function: &str, args: Vec<TxCallArgumentType>, coin: u256, what: String) -> Prepared {
    let amount = match u128::try_from(coin) {
        Ok(narrow) => Amount::U128(narrow),
        Err(_) => Amount::U256(coin),
    };

    Prepared {
        from,
        tx: Tx::ContractCall { to, func: Hasher::digest(HashType::Xxh3_64, function.as_bytes()), amount, data: args },
        what,
        about: to.to_string(),
        coin,
    }
}

/// Asks Esplora for what the HTLC holds.
fn look_for_funding(ctx: &Ctx, state: &SwapsState, address: &str, hash: &str) {
    let Ok(client) = crate::app::client(ctx.settings) else {
        return;
    };
    let (looked, wake, hash) = (state.looked.clone(), ctx.wake.clone(), hash.to_string());
    let url = format!("{}/address/{address}/utxo", ctx.settings.esplora.trim().trim_end_matches('/'));

    client.fetch(url, None, move |answer| {
        let found = answer.and_then(|answer| serde_json::from_slice::<Value>(&answer.bytes).map_err(|err| err.to_string())).map(|utxos| {
            utxos.as_array().and_then(|utxos| utxos.first()).map(|utxo| Looked::Funding {
                hash,
                outpoint: format!("{}:{}", utxo["txid"].as_str().unwrap_or_default(), utxo["vout"]),
                sats: utxo["value"].as_u64().unwrap_or(0),
            })
        });
        let looked_now = match found {
            Ok(Some(found)) => found,
            Ok(None) => Looked::Failed("Esplora shows nothing at the HTLC yet".to_string()),
            Err(why) => Looked::Failed(why),
        };
        looked.lock().expect("never poisoned").push(looked_now);
        wake.request_repaint();
    });
}

/// Asks Esplora for the HTLC's transactions, and reads the secret from a claim among them.
fn look_for_secret(ctx: &Ctx, state: &SwapsState, terms: &Terms) {
    let Ok(client) = crate::app::client(ctx.settings) else {
        return;
    };
    let Some(htlc) = terms.htlc() else {
        return;
    };
    let (looked, wake, hash, wanted) = (state.looked.clone(), ctx.wake.clone(), to_hex(&terms.hash), terms.hash);
    let url = format!("{}/address/{}/txs", ctx.settings.esplora.trim().trim_end_matches('/'), htlc.address(terms.network()));

    client.fetch(url, None, move |answer| {
        let secret = answer.ok().and_then(|answer| serde_json::from_slice::<Value>(&answer.bytes).ok()).and_then(|txs| {
            txs.as_array()?.iter().flat_map(|tx| tx["vin"].as_array().into_iter().flatten()).flat_map(|vin| vin["witness"].as_array().into_iter().flatten()).find_map(|item| {
                let bytes: [u8; 32] = from_hex(&format!("0x{}", item.as_str()?)).and_then(|bytes| bytes.try_into().ok())?;

                (sha256(&bytes) == wanted).then_some(bytes)
            })
        });
        let looked_now = match secret {
            Some(secret) => Looked::Secret { hash, secret },
            None => Looked::Failed("Esplora shows no claim of the HTLC yet".to_string()),
        };
        looked.lock().expect("never poisoned").push(looked_now);
        wake.request_repaint();
    });
}

/// Broadcasts a signed spend through Esplora.
fn broadcast(ctx: &Ctx, state: &SwapsState, hex: String) {
    let Ok(client) = crate::app::client(ctx.settings) else {
        return;
    };
    let (looked, wake) = (state.looked.clone(), ctx.wake.clone());
    let url = format!("{}/tx", ctx.settings.esplora.trim().trim_end_matches('/'));

    client.fetch(url, Some(hex.into_bytes()), move |answer| {
        let result = answer.and_then(|answer| match answer.ok {
            true => Ok(String::from_utf8_lossy(&answer.bytes).trim().to_string()),
            false => Err(format!("{}: {}", answer.status, String::from_utf8_lossy(&answer.bytes).trim())),
        });
        looked.lock().expect("never poisoned").push(Looked::Broadcast(result));
        wake.request_repaint();
    });
}

/// Takes what Esplora answered since the last frame.
fn take_looked(ctx: &mut Ctx, state: &mut SwapsState) {
    let looked: Vec<Looked> = std::mem::take(&mut *state.looked.lock().expect("never poisoned"));

    for looked in looked {
        match looked {
            Looked::Funding { hash, outpoint, sats } => {
                if let Some(record) = ctx.book.swaps.iter_mut().find(|record| Terms::from_text(&record.terms).is_ok_and(|terms| to_hex(&terms.hash) == hash)) {
                    record.funding = Some(format!("{outpoint}:{sats}"));
                    *ctx.changed = true;
                }
                (state.bitcoin.funding, state.bitcoin.sats) = (outpoint, sats.to_string());
            }
            Looked::Secret { hash, secret } => {
                if let Some(record) = ctx.book.swaps.iter_mut().find(|record| Terms::from_text(&record.terms).is_ok_and(|terms| to_hex(&terms.hash) == hash)) {
                    record.secret = Some(to_hex(&secret));
                    *ctx.changed = true;
                }
            }
            Looked::Broadcast(Ok(txid)) => state.trouble = Some(format!("Broadcast: {txid}")),
            Looked::Broadcast(Err(why)) | Looked::Failed(why) => state.trouble = Some(why),
        }
    }
}

#[cfg(test)]
mod tests;
