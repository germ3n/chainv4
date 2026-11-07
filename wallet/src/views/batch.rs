//! Building a batch: any steps a batch may hold, added one at a time, put in order, looked over and
//! sent as one transaction, such as an approval and the swap that spends it, or several people
//! paid at once. Nothing is signed here: it goes to the Send page, prepared, for the same review
//! and signing as anything else, on to the offline signer for a watch-only account; or, from a
//! multisig, to the Multisig page, as a bundle for its members.
//!
//! The steps and the form that adds them are the Multisig page's too, for proposing anything a
//! multisig sends: there one step is sent as it is, and several as a batch.

use {
    super::send::{Destination, Prepared, SendState, destination, next_nonce},
    super::{Ctx, Page, steps},
    crate::format,
    crate::theme,
    chainv4::abi::{encode_values, parse_signature, parse_value},
    chainv4::crypto::hasher::Hasher,
    chainv4::crypto::verifiable::Verifiable,
    chainv4::registry::names::NameAction,
    chainv4::structs::address::Address,
    chainv4::structs::amount::Amount,
    chainv4::structs::code::Code,
    chainv4::structs::hash::{Hash, HashType},
    chainv4::structs::hex::from_hex,
    chainv4::structs::tx::{MAX_BATCH_STEPS, Tx, TxCallArgumentType},
    chainv4::structs::u256::u256,
    chainv4_ui::files::{self, Pick},
    egui::{RichText, Ui},
    std::sync::{Arc, Mutex},
};

/// What a step does, as the form asks it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    #[default]
    Coin,
    Token,
    Call,
    Evm,
    Deploy,
    Upgrade,
    Remove,
    Name,
    Primary,
    Burn,
    Claim,
}

/// What each kind is called in the form.
const KINDS: [(Kind, &str); 11] = [
    (Kind::Coin, "Send the coin"),
    (Kind::Token, "Send a token"),
    (Kind::Call, "Call a contract"),
    (Kind::Evm, "Call an EVM contract"),
    (Kind::Deploy, "Deploy a contract"),
    (Kind::Upgrade, "Upgrade a contract"),
    (Kind::Remove, "Remove a contract"),
    (Kind::Name, "A name"),
    (Kind::Primary, "The primary name"),
    (Kind::Burn, "Burn the coin"),
    (Kind::Claim, "Claim pool earnings"),
];

/// What a name step does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NameKind {
    #[default]
    Register,
    Renew,
    Give,
    Point,
    Text,
    Subname,
}

/// The engine a deploy or an upgrade's code is for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Engine {
    #[default]
    Lua,
    Wasm,
    Evm,
}

/// The fields of the step being added: each kind reads those it asks for.
#[derive(Clone, Debug, Default)]
pub struct Form {
    pub kind: Kind,
    /// Whom it pays: an address or a name. A call's contract, too.
    pub to: String,
    pub amount: String,
    pub token: String,
    pub function: String,
    /// A native call's arguments as type:value, or an EVM call's in its signature's order: a line each.
    pub args: String,
    /// An EVM call's calldata in hex, when it names no signature.
    pub data: String,
    pub engine: Engine,
    /// A Lua source, or EVM init code in hex.
    pub code: String,
    /// A WebAssembly module picked.
    pub wasm: Option<(String, Vec<u8>)>,
    pub name: String,
    pub name_kind: NameKind,
    pub years: String,
    pub key: String,
    pub value: String,
    pub height: String,
    pub index: String,
    pub path: String,
}

/// What a form's step is read with: whose it is, and what this wallet knows of the coin and names.
pub struct Reading<'a> {
    pub from: Address,
    pub decimals: u8,
    /// A token's decimals, where they are known.
    pub token_decimals: &'a dyn Fn(&str) -> Option<u8>,
    /// Whom a recipient as typed names: an address, or what a name was proven to resolve to.
    pub recipient: &'a dyn Fn(&str) -> Result<Address, String>,
    /// A name without the chain's suffix.
    pub bare_name: &'a dyn Fn(&str) -> String,
}

/// The narrowest width an amount fits, as the chain's own builders write one.
fn amount(units: u256) -> Amount {
    match u128::try_from(units) {
        Ok(narrow) => Amount::U128(narrow),
        Err(_) => Amount::U256(units),
    }
}

/// An amount as typed, in units of so many decimals: nothing for none.
fn units(text: &str, decimals: u8, of: &str) -> Result<u256, String> {
    match text.trim() {
        "" => Ok(u256::ZERO),
        text => format::parse_units(text, decimals).and_then(|units| u256::from_str_radix(&units, 10).ok()).ok_or_else(|| format!("{text} is not an amount of {of}")),
    }
}

/// A contract's address as typed: with its kind, or an EVM one as 0x and 40 hex digits.
fn contract(text: &str) -> Result<Address, String> {
    let text = text.trim();
    let read = match text.len() == 42 && text.starts_with("0x") {
        true => format!("evm:{text}").parse(),
        false => text.parse::<Address>(),
    };

    read.map_err(|_| format!("{text} is not a contract's address: contract:0x…, evm:0x…, or an EVM 0x…"))
}

impl Form {
    /// The code a deploy or upgrade's fields hold.
    fn code(&self) -> Result<Code, String> {
        match self.engine {
            Engine::Lua if self.code.trim().is_empty() => Err("give the contract's source".to_string()),
            Engine::Lua => Ok(Code::Lua(self.code.clone())),
            Engine::Wasm => Ok(Code::Wasm(self.wasm.as_ref().ok_or("pick the module")?.1.clone())),
            Engine::Evm => Ok(Code::Evm(from_hex(self.code.trim()).filter(|code| !code.is_empty()).ok_or("the init code is not hex")?)),
        }
    }

    /// The transaction the fields make, or why they make none.
    pub fn step(&self, reading: &Reading) -> Result<Tx, String> {
        let from = reading.from;
        let lines = |text: &str| text.lines().map(str::trim).filter(|line| !line.is_empty()).map(str::to_string).collect::<Vec<_>>();
        let whole = |text: &str, what: &str| text.trim().parse::<u64>().map_err(|_| format!("{what} must be a whole number"));

        let step = match self.kind {
            Kind::Coin => Tx::Transfer { from, to: (reading.recipient)(&self.to)?, amount: amount(units(&self.amount, reading.decimals, "the coin")?) },
            Kind::Burn => Tx::Burn { from, amount: amount(units(&self.amount, reading.decimals, "the coin")?) },
            Kind::Token => {
                let token = contract(&self.token)?;
                let decimals = (reading.token_decimals)(&token.to_string()).unwrap_or(0);
                let sent = units(&self.amount, decimals, "the token")?;

                if sent == u256::ZERO {
                    return Err("send more than nothing".to_string());
                }

                Tx::ContractCall {
                    to: token,
                    func: Hasher::digest(HashType::Xxh3_64, b"transfer"),
                    amount: Amount::U128(0),
                    data: vec![TxCallArgumentType::Address((reading.recipient)(&self.to)?), TxCallArgumentType::U256(sent)],
                }
            }
            Kind::Call => {
                if self.function.trim().is_empty() {
                    return Err("name the function to call".to_string());
                }

                Tx::ContractCall {
                    to: contract(&self.to)?,
                    func: Hasher::digest(HashType::Xxh3_64, self.function.trim().as_bytes()),
                    amount: amount(units(&self.amount, reading.decimals, "the coin")?),
                    data: lines(&self.args).iter().map(|line| TxCallArgumentType::parse_typed(line)).collect::<Result<_, _>>()?,
                }
            }
            Kind::Evm => {
                let input = match self.function.trim() {
                    "" => match self.data.trim() {
                        "" => Vec::new(),
                        data => from_hex(data).ok_or("the calldata is not hex")?,
                    },
                    signature => {
                        let (selector, types) = parse_signature(signature)?;
                        let given = lines(&self.args);

                        if given.len() != types.len() {
                            return Err(format!("{signature} takes {} arguments, a line each; this has {}", types.len(), given.len()));
                        }

                        let values = types.iter().zip(&given).map(|(ty, line)| parse_value(ty, line)).collect::<Result<Vec<_>, _>>()?;

                        [selector.ok_or("the signature names no function")?.to_vec(), encode_values(&types, &values)?].concat()
                    }
                };

                Tx::EvmCall { to: contract(&self.to).or_else(|_| (reading.recipient)(&self.to))?, value: amount(units(&self.amount, reading.decimals, "the coin")?), input }
            }
            Kind::Deploy => Tx::Deploy { code: self.code()? },
            Kind::Upgrade => match self.engine {
                Engine::Evm => return Err("EVM contracts are upgraded through a proxy, with a call".to_string()),
                _ => Tx::Upgrade { contract: contract(&self.to)?, code: self.code()? },
            },
            // For good: typed out, as on the Contracts page.
            Kind::Remove => {
                let removed = contract(&self.to)?;

                if matches!(removed, Address::Evm(_)) {
                    return Err("an EVM contract is removed by its own code, as Ethereum's are".to_string());
                }

                if self.value.trim() != super::contracts::REMOVE_WORD {
                    return Err(format!("type {} to remove it: it cannot be undone", super::contracts::REMOVE_WORD));
                }

                let beneficiary = match self.key.trim() {
                    "" => from,
                    to => (reading.recipient)(to)?,
                };

                Tx::Remove { contract: removed, beneficiary }
            }
            Kind::Name => {
                let name = (reading.bare_name)(&self.name);
                let years = || match self.years.trim() {
                    "" => Ok(1),
                    years => years.parse::<u32>().map_err(|_| "years must be a whole number".to_string()),
                };
                let action = match self.name_kind {
                    NameKind::Register => NameAction::Register { years: years()? },
                    NameKind::Renew => NameAction::Renew { years: years()? },
                    NameKind::Give => NameAction::Transfer { to: (reading.recipient)(&self.to)? },
                    NameKind::Point => NameAction::Target { target: match self.to.trim() {
                        "" => None,
                        to => Some((reading.recipient)(to)?),
                    } },
                    NameKind::Text => NameAction::Text { key: self.key.trim().to_string(), value: (!self.value.is_empty()).then(|| self.value.clone()) },
                    NameKind::Subname => NameAction::Subname { label: self.key.trim().to_string(), owner: match self.to.trim() {
                        "" => from,
                        to => (reading.recipient)(to)?,
                    } },
                };

                Tx::Name { name, action }
            }
            Kind::Primary => Tx::PrimaryName { name: match self.name.trim() {
                "" => None,
                name => Some((reading.bare_name)(name)),
            } },
            Kind::Claim => Tx::Claim {
                height: whole(&self.height, "the block")?,
                to: (reading.recipient)(&self.to)?,
                earned: u256::from_str_radix(self.amount.trim(), 10).map_err(|_| "what it earned must be whole units".to_string())?,
                index: whole(&self.index, "its place")?,
                path: self
                    .path
                    .split([',', '\n', ' '])
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .map(|text| from_hex(text).and_then(|bytes| Hash::from_bytes(HashType::default_chain(), &bytes)).ok_or_else(|| format!("{text} is not a hash of the claim's path")))
                    .collect::<Result<_, _>>()?,
            },
        };

        step.verify().map_err(|err| err.to_string())?;

        Ok(step)
    }
}

/// A file picked for a deploy, once it arrives.
type Picked = Arc<Mutex<Option<Result<(String, Vec<u8>), String>>>>;

#[derive(Default)]
pub struct BatchState {
    /// Whom it is sent as: one of the account's addresses, or a multisig it follows.
    from: Option<Address>,
    steps: Vec<Tx>,
    /// The functions its calls were added by, which MetaMask shows them as.
    pub(super) known: Vec<String>,
    form: Form,
    picked: Picked,
    trouble: Option<String>,
}

impl BatchState {
    /// Adds a step, as far as a batch holds them.
    pub fn add(&mut self, step: Tx) -> Result<(), String> {
        if self.steps.len() >= MAX_BATCH_STEPS {
            return Err(format!("a batch holds at most {MAX_BATCH_STEPS} steps"));
        }

        self.steps.push(step);

        Ok(())
    }

    /// Takes a step out.
    pub fn remove(&mut self, index: usize) {
        if index < self.steps.len() {
            self.steps.remove(index);
        }
    }

    /// Moves a step one place earlier; the first stays.
    pub fn earlier(&mut self, index: usize) {
        if index > 0 && index < self.steps.len() {
            self.steps.swap(index, index - 1);
        }
    }

    /// Moves a step one place later; the last stays.
    pub fn later(&mut self, index: usize) {
        if index + 1 < self.steps.len() {
            self.steps.swap(index, index + 1);
        }
    }

    /// What its steps make as a proposal: the one step as it is, or several as a batch, as the
    /// chain will check them from this sender.
    pub fn proposal(&self, from: &Address) -> Result<Tx, String> {
        match self.steps.as_slice() {
            [one] => {
                self.batch(from)?;

                Ok(one.clone())
            }
            _ => self.batch(from),
        }
    }

    /// Drops the steps, as once they went out.
    pub fn clear(&mut self) {
        self.steps.clear();
        self.trouble = None;
    }

    /// The transaction its steps make, as the chain will check it from this sender.
    pub fn batch(&self, from: &Address) -> Result<Tx, String> {
        let tx = Tx::Batch { steps: self.steps.clone() };

        tx.verify().map_err(|err| err.to_string())?;

        match self.steps.iter().all(|step| !matches!(step, Tx::Transfer { from: taken, .. } | Tx::Burn { from: taken, .. } if taken != from)) {
            true => Ok(tx),
            false => Err("every step takes from whom it is sent as: pick the sender first, and add its steps again".to_string()),
        }
    }

    /// The name the form's To field holds, if it holds one rather than an address: what to prove.
    pub fn wanted_name(&self) -> Option<String> {
        let to = self.form.to.trim();

        (!to.is_empty() && !to.contains(':') && !(to.len() == 42 && to.starts_with("0x"))).then(|| to.to_string())
    }
}

/// Who may send a batch: the account's addresses, and the multisigs this wallet's accounts are
/// members of or that it follows.
fn senders(ctx: &Ctx, multisig: &super::multisig::MultisigState) -> Vec<Address> {
    let mine = ctx.holder().map(|holder| holder.addresses()).unwrap_or_default();

    mine.into_iter().chain(multisig.known(ctx).iter().filter_map(|known| known.address.parse().ok())).collect()
}

/// What a sender goes by in the picker: a multisig by its address, one of the account's as the
/// other pages' From picker lists it, with the coin it holds.
fn sender_name(ctx: &Ctx, from: &Address) -> String {
    match from {
        Address::Multisig(_) => format!("Multisig · {}", format::short(&from.to_string())),
        address => super::from::label(ctx, address),
    }
}

/// What a name step costs, where the chain's prices are known: what the review checks the balance for.
fn name_cost(ctx: &Ctx, steps: &[Tx]) -> u256 {
    let Some(names) = &ctx.chain.names else {
        return u256::ZERO;
    };

    steps
        .iter()
        .filter_map(|step| match step {
            Tx::Name { name, action: NameAction::Register { years } | NameAction::Renew { years } } => chainv4::registry::names::price(&names.prices, name, *years),
            _ => None,
        })
        .fold(u256::ZERO, u256::saturating_add)
}

pub fn show(ui: &mut Ui, ctx: &mut Ctx, state: &mut BatchState, send: &mut SendState, multisig: &mut super::multisig::MultisigState) {
    let palette = *ctx.palette;
    theme::title(ui, &palette, "Batch", "Several steps under one signature and one fee: all of them go through, or none do.");

    // What was sent is done with, once the node took it.
    if send.take_batch_sent() {
        state.steps.clear();
        state.trouble = None;
    }

    multisig.ask_memberships(ctx);
    let senders = senders(ctx, multisig);

    // Started from the address picked on another page; once steps are in, they keep their sender
    // until it is changed here, as changing it clears them.
    if !state.from.is_some_and(|from| senders.contains(&from)) {
        state.from = super::from::remembered(ctx.wake).filter(|picked| senders.contains(picked)).or_else(|| senders.first().copied());
        state.steps.clear();
    }

    let Some(from) = state.from else {
        return;
    };

    if senders.len() > 1 {
        theme::card(ui, &palette, |ui| {
            theme::caps(ui, &palette, "Send as");
            let before = state.from;
            egui::ComboBox::from_id_salt("batch-from").selected_text(sender_name(ctx, &from)).width(ui.available_width()).show_ui(ui, |ui| {
                for sender in &senders {
                    ui.selectable_value(&mut state.from, Some(*sender), sender_name(ctx, sender));
                }
            });

            // Each step that moves coin says whom it takes from.
            if state.from != before {
                state.steps.clear();

                if let Some(picked) = state.from.filter(|picked| !matches!(picked, Address::Multisig(_))) {
                    super::from::remember(ctx.wake, picked);
                }
            }
        });
    }

    list(ui, ctx, state, send, multisig, &from);
    add_form(ui, ctx, state, &from);
}

/// The steps so far, each to move or take out, and the button that goes on to review them.
fn list(ui: &mut Ui, ctx: &mut Ctx, state: &mut BatchState, send: &mut SendState, multisig: &mut super::multisig::MultisigState, from: &Address) {
    let Some((tx, coin, count)) = steps_card(ui, ctx, state, from, "Review", false) else {
        return;
    };
    let prepared = Prepared {
        from: *from,
        tx,
        what: format!("A batch of {count} step{}", if count == 1 { "" } else { "s" }),
        about: "its steps".to_string(),
        coin,
        functions: state.known.clone(),
    };

    match from {
        // The members agree to it as they do to anything their multisig sends.
        Address::Multisig(_) => {
            multisig.propose(prepared);
            *ctx.page = Page::Multisig;
        }
        _ => {
            send.prepare(prepared);
            *ctx.page = Page::Send;
        }
    }
}

/// The steps so far, each to move or take out, and a button that goes on with them: once pressed,
/// the transaction they make, what they send in all, the coin and names together, and how many
/// they are. A batch of them, or, as a proposal, one step alone as it is.
pub(super) fn steps_card(ui: &mut Ui, ctx: &mut Ctx, state: &mut BatchState, from: &Address, button: &str, proposal: bool) -> Option<(Tx, u256, usize)> {
    let palette = *ctx.palette;
    let tx = Tx::Batch { steps: state.steps.clone() };
    let nonce = ctx.chain.account(&from.to_string()).map(|proven| next_nonce(proven.value.nonce, &ctx.settings.sent, &from.to_string(), ctx.now));
    let landing = nonce.and_then(|nonce| match from {
        Address::Multisig(_) => None,
        _ => steps::landing_of(ctx, &tx, *from, nonce),
    });
    let laid = steps::of_tx(ctx, &tx, landing.as_ref())?;
    let mut change = None;
    let mut made = None;

    theme::card(ui, &palette, |ui| {
        match laid.count {
            0 => {
                theme::caps(ui, &palette, "Steps");
                ui.add(egui::Label::new(RichText::new("None yet: add the first below.").color(palette.muted)).wrap());
            }
            _ => steps::heading(ui, ctx, &laid),
        }

        for (index, step) in laid.steps.iter().enumerate() {
            ui.push_id(("builder step", index), |ui| {
                steps::one(ui, ctx, index, laid.count, step);
                ui.horizontal_wrapped(|ui| {
                    if index > 0 && theme::quiet(ui, &palette, "Move up").clicked() {
                        change = Some((index, -1));
                    }

                    if index + 1 < laid.count && theme::quiet(ui, &palette, "Move down").clicked() {
                        change = Some((index, 1));
                    }

                    if theme::quiet(ui, &palette, "Remove").clicked() {
                        change = Some((index, 0));
                    }
                });
                ui.add_space(4.0);
            });
        }

        if let Some(why) = &state.trouble {
            ui.colored_label(palette.bad, why.as_str());
        }

        if theme::primary(ui, &palette, button, laid.count > 0).clicked() {
            let made_up = match proposal {
                true => state.proposal(from),
                false => state.batch(from),
            };

            match made_up {
                Ok(tx) => made = Some((tx, laid.coin.saturating_add(name_cost(ctx, &state.steps)), laid.count)),
                Err(why) => state.trouble = Some(why),
            }
        }
    });

    match change {
        Some((index, -1)) => state.earlier(index),
        Some((index, 1)) => state.later(index),
        Some((index, _)) => state.remove(index),
        None => {}
    }

    made
}

/// The form that adds a step.
pub(super) fn add_form(ui: &mut Ui, ctx: &mut Ctx, state: &mut BatchState, from: &Address) {
    let palette = *ctx.palette;

    if let Some(picked) = state.picked.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take() {
        match picked {
            Ok((name, bytes)) => match state.form.engine {
                Engine::Wasm => state.form.wasm = Some((name, bytes)),
                Engine::Lua => state.form.code = String::from_utf8_lossy(&bytes).into_owned(),
                Engine::Evm => state.form.code = String::from_utf8_lossy(&bytes).trim().to_string(),
            },
            Err(why) => state.trouble = Some(why),
        }
    }

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Add a step");
        let shown = KINDS.iter().find(|(kind, _)| *kind == state.form.kind).map_or("", |(_, name)| name);
        egui::ComboBox::from_id_salt("batch-kind").selected_text(shown).width(ui.available_width()).show_ui(ui, |ui| {
            for (kind, name) in KINDS {
                ui.selectable_value(&mut state.form.kind, kind, name);
            }
        });
        ui.add_space(6.0);
        fields(ui, ctx, state);
        ui.add_space(8.0);

        let room = state.steps.len() < MAX_BATCH_STEPS;

        if !room {
            ui.label(RichText::new(format!("A batch holds at most {MAX_BATCH_STEPS} steps.")).small().color(palette.warn));
        }

        if theme::primary(ui, &palette, "Add step", room).clicked() {
            let token_decimals = |token: &str| ctx.chain.tokens.get(token).and_then(|token| token.value.decimals);
            let recipient = |text: &str| match destination(ctx, text) {
                Destination::Address(address) => Ok(address),
                Destination::Named { shown, resolved: Some(resolved) } => resolved.target().ok_or_else(|| format!("{shown} resolves to nothing")),
                Destination::Named { shown, resolved: None } => Err(format!("{shown} is not proven yet: wait a moment")),
                Destination::Blank | Destination::Bad | Destination::NeedsAsking => Err("the address to send to does not read".to_string()),
            };
            let bare_name = |text: &str| ctx.chain.names.as_ref().map_or_else(|| text.trim().to_string(), |names| names.strip(text.trim()));
            let reading = Reading { from: *from, decimals: ctx.coin.decimals, token_decimals: &token_decimals, recipient: &recipient, bare_name: &bare_name };

            let function = state.form.function.trim().to_string();

            match state.form.step(&reading).and_then(|step| state.add(step)) {
                Ok(()) => {
                    if !function.is_empty() && !state.known.contains(&function) {
                        state.known.push(function);
                    }

                    // What the kind's fields hold goes; which kind it was stays, for the next of them.
                    state.form = Form { kind: state.form.kind, engine: state.form.engine, ..Form::default() };
                    state.trouble = None;
                }
                Err(why) => state.trouble = Some(why),
            }
        }
    });
}

/// One line of the form, across the card.
fn line(ui: &mut Ui, value: &mut String, hint: &str) {
    ui.add(egui::TextEdit::singleline(value).hint_text(hint).desired_width(f32::INFINITY));
}

/// One field of the form holding an address, in the mono face.
fn address_line(ui: &mut Ui, value: &mut String, hint: &str) {
    ui.add(egui::TextEdit::singleline(value).hint_text(hint).font(egui::TextStyle::Monospace).desired_width(f32::INFINITY));
}

/// Several lines of the form.
fn lines(ui: &mut Ui, value: &mut String, hint: &str) {
    ui.add(egui::TextEdit::multiline(value).hint_text(hint).font(egui::TextStyle::Monospace).desired_rows(2).desired_width(f32::INFINITY));
}

/// The code of a deploy or upgrade: its engine, and its source, init code or module.
fn code_fields(ui: &mut Ui, ctx: &Ctx, state: &mut BatchState, engines: &[(Engine, &str)]) {
    let palette = *ctx.palette;

    if !engines.iter().any(|(engine, _)| *engine == state.form.engine) {
        state.form.engine = engines[0].0;
    }

    theme::segmented_rows(ui, &palette, &mut state.form.engine, engines);
    ui.add_space(4.0);
    let kind: (&'static str, &'static [&'static str]) = match state.form.engine {
        Engine::Lua => {
            lines(ui, &mut state.form.code, "the contract's Lua source, one file");

            ("Lua", &["lua", "luau"])
        }
        Engine::Wasm => {
            let text = state.form.wasm.as_ref().map_or("No module picked.".to_string(), |(name, bytes)| format!("{name}, {} bytes", bytes.len()));
            ui.label(RichText::new(text).color(palette.soft));

            ("WebAssembly", &["wasm"])
        }
        Engine::Evm => {
            lines(ui, &mut state.form.code, "its init code in hex, as solc --bin writes it");

            ("EVM bytecode", &["bin", "hex"])
        }
    };

    if files::can_pick(ui.ctx()) && theme::quiet(ui, &palette, "Pick a file…").clicked() {
        let picked = state.picked.clone();
        files::pick(ui.ctx(), Pick { kind: Some(kind), ..Pick::default() }, move |files| {
            let read = match files {
                Ok(mut files) if !files.is_empty() => {
                    let file = files.swap_remove(0);

                    Ok((file.name, file.bytes))
                }
                Ok(_) => return,
                Err(why) => Err(why),
            };
            *picked.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(read);
        });
    }
}

/// What the kind of step asks for.
fn fields(ui: &mut Ui, ctx: &Ctx, state: &mut BatchState) {
    let palette = *ctx.palette;
    let symbol = ctx.coin.symbol.clone();
    let form = &mut state.form;

    match form.kind {
        Kind::Coin => {
            address_line(ui, &mut form.to, "To: an address, or a name such as alice.dev");
            line(ui, &mut form.amount, &format!("Amount in {symbol}"));
        }
        Kind::Burn => line(ui, &mut form.amount, &format!("Amount in {symbol}")),
        Kind::Token => {
            address_line(ui, &mut form.token, "The token: contract:0x…");
            address_line(ui, &mut form.to, "To: an address, or a name");
            line(ui, &mut form.amount, "Amount of the token");
        }
        Kind::Call => {
            address_line(ui, &mut form.to, "The contract: contract:0x…");
            line(ui, &mut form.function, "The function's name");
            lines(ui, &mut form.args, "Arguments as type:value, a line each, such as i64:5");
            line(ui, &mut form.amount, &format!("{symbol} to send with it, optional"));
        }
        Kind::Evm => {
            address_line(ui, &mut form.to, "The contract: 0x… or evm:0x…");
            line(ui, &mut form.function, "A signature, such as approve(address,uint256)");
            lines(ui, &mut form.args, "Its arguments, a line each");
            address_line(ui, &mut form.data, "Or the calldata in hex");
            line(ui, &mut form.amount, &format!("{symbol} to send with it, optional"));
        }
        Kind::Deploy => {
            ui.add(egui::Label::new(RichText::new("Where it lands shows on the step once added, for the steps after it to call.").small().color(palette.muted)).wrap());
            code_fields(ui, ctx, state, &[(Engine::Lua, "Lua"), (Engine::Wasm, "WebAssembly"), (Engine::Evm, "EVM")]);
        }
        Kind::Upgrade => {
            address_line(ui, &mut form.to, "The contract: contract:0x…");
            code_fields(ui, ctx, state, &[(Engine::Lua, "Lua"), (Engine::Wasm, "WebAssembly")]);
        }
        Kind::Remove => {
            ui.add(egui::Label::new(RichText::new("For good: its balance to whom you name, its code and storage deleted, its address never used again. Only if its check_perms lets this sender.").small().color(palette.bad)).wrap());
            address_line(ui, &mut form.to, "The contract: contract:0x…");
            address_line(ui, &mut form.key, "Its balance to: an address or a name, or empty for this sender");
            line(ui, &mut form.value, "Type remove");
        }
        Kind::Name => {
            theme::segmented_rows(ui, &palette, &mut form.name_kind, &[(NameKind::Register, "Register"), (NameKind::Renew, "Renew"), (NameKind::Give, "Give"), (NameKind::Point, "Point"), (NameKind::Text, "Text"), (NameKind::Subname, "Subname")]);
            ui.add_space(4.0);
            line(ui, &mut form.name, "The name, such as alice.dev");

            match form.name_kind {
                NameKind::Register | NameKind::Renew => line(ui, &mut form.years, "Years, 1 if left empty"),
                NameKind::Give => address_line(ui, &mut form.to, "To: an address, or a name"),
                NameKind::Point => address_line(ui, &mut form.to, "Points at: an address, or empty for nothing"),
                NameKind::Text => {
                    line(ui, &mut form.key, "Which record, such as url");
                    line(ui, &mut form.value, "Its text, or empty to clear it");
                }
                NameKind::Subname => {
                    line(ui, &mut form.key, "The label under it");
                    address_line(ui, &mut form.to, "Owned by: an address, or empty for this sender");
                }
            }
        }
        Kind::Primary => line(ui, &mut form.name, "The name, or empty to clear it"),
        Kind::Claim => {
            line(ui, &mut form.height, "The pool's block");
            address_line(ui, &mut form.to, "The miner");
            line(ui, &mut form.amount, "All it earned, in whole units");
            line(ui, &mut form.index, "Its place in the claims");
            lines(ui, &mut form.path, "The path's hashes, a line each");
        }
    }
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::keys,
        crate::views::drawn,
        chainv4::crypto::keypair::Keypair,
    };

    fn me() -> Address {
        Keypair::from_seed([7; 32]).public().address()
    }

    fn someone() -> Address {
        Keypair::from_seed([8; 32]).public().address()
    }

    fn read(form: &Form) -> Result<Tx, String> {
        let token_decimals = |_: &str| Some(2);
        let recipient = |text: &str| text.trim().parse::<Address>().map_err(|_| "does not read".to_string());
        let bare_name = |text: &str| text.trim().trim_end_matches(".dev").to_string();

        form.step(&Reading { from: me(), decimals: 18, token_decimals: &token_decimals, recipient: &recipient, bare_name: &bare_name })
    }

    fn pay(amount: u128) -> Tx {
        Tx::Transfer { from: me(), to: someone(), amount: Amount::U128(amount) }
    }

    #[test]
    fn steps_are_added_put_in_order_and_taken_out_keeping_to_what_a_batch_holds() {
        let mut state = BatchState::default();

        for amount in 1..=3 {
            state.add(pay(amount)).unwrap();
        }

        state.later(0);
        assert_eq!(state.steps, [pay(2), pay(1), pay(3)]);
        state.earlier(2);
        assert_eq!(state.steps, [pay(2), pay(3), pay(1)]);

        // The ends stay; what is past them does nothing.
        state.earlier(0);
        state.later(2);
        state.later(9);
        state.remove(9);
        assert_eq!(state.steps, [pay(2), pay(3), pay(1)]);

        state.remove(1);
        assert_eq!(state.steps, [pay(2), pay(1)]);

        assert!(state.batch(&me()).is_ok());
        assert!(state.batch(&someone()).unwrap_err().contains("takes from whom it is sent as"));
        assert!(BatchState::default().batch(&me()).unwrap_err().contains("1 to 32"));

        for _ in 2..MAX_BATCH_STEPS {
            state.add(pay(1)).unwrap();
        }

        assert!(state.add(pay(1)).unwrap_err().contains("at most 32"));
        assert_eq!(state.steps.len(), MAX_BATCH_STEPS);
        assert!(state.batch(&me()).is_ok());
    }

    #[test]
    fn a_removal_is_typed_out_and_pays_this_sender_unless_it_names_another() {
        let token = format!("contract:0x{}", "11".repeat(32));
        let form = |value: &str, key: &str| Form { kind: Kind::Remove, to: token.clone(), value: value.to_string(), key: key.to_string(), ..Form::default() };

        assert!(read(&form("", "")).unwrap_err().contains("type remove"), "not typed, nothing made");
        assert_eq!(read(&form("remove", "")), Ok(Tx::Remove { contract: token.parse().unwrap(), beneficiary: me() }));
        assert_eq!(read(&form("remove", &someone().to_string())), Ok(Tx::Remove { contract: token.parse().unwrap(), beneficiary: someone() }));

        let evm = Form { to: format!("0x{}", "ab".repeat(20)), ..form("remove", "") };
        assert!(read(&evm).unwrap_err().contains("its own code"));
    }

    #[test]
    fn every_kind_of_step_a_batch_holds_can_be_made_and_each_says_what_is_wrong() {
        let to = someone().to_string();
        let evm = format!("0x{}", "ab".repeat(20));
        let token = format!("contract:0x{}", "11".repeat(32));
        let path = format!("0x{}", "00".repeat(32));
        let forms = [
            Form { kind: Kind::Coin, to: to.clone(), amount: "1.5".to_string(), ..Form::default() },
            Form { kind: Kind::Burn, amount: "1".to_string(), ..Form::default() },
            Form { kind: Kind::Token, token: token.clone(), to: to.clone(), amount: "2.5".to_string(), ..Form::default() },
            Form { kind: Kind::Call, to: token.clone(), function: "bump".to_string(), args: "i64:5\nstring:hi\n".to_string(), ..Form::default() },
            Form { kind: Kind::Evm, to: evm.clone(), function: "approve(address,uint256)".to_string(), args: format!("{evm}\n5"), ..Form::default() },
            Form { kind: Kind::Evm, to: evm.clone(), data: "0x01020304".to_string(), amount: "1".to_string(), ..Form::default() },
            Form { kind: Kind::Deploy, code: "return {}".to_string(), ..Form::default() },
            Form { kind: Kind::Deploy, engine: Engine::Evm, code: "0x60006000".to_string(), ..Form::default() },
            Form { kind: Kind::Deploy, engine: Engine::Wasm, wasm: Some(("m.wasm".to_string(), vec![0, 97])), ..Form::default() },
            Form { kind: Kind::Upgrade, to: token.clone(), code: "return {}".to_string(), ..Form::default() },
            Form { kind: Kind::Name, name: "alice.dev".to_string(), years: "2".to_string(), ..Form::default() },
            Form { kind: Kind::Name, name: "alice".to_string(), name_kind: NameKind::Point, ..Form::default() },
            Form { kind: Kind::Name, name: "alice".to_string(), name_kind: NameKind::Subname, key: "pay".to_string(), ..Form::default() },
            Form { kind: Kind::Primary, name: "alice.dev".to_string(), ..Form::default() },
            Form { kind: Kind::Primary, ..Form::default() },
            Form { kind: Kind::Claim, height: "9".to_string(), to: to.clone(), amount: "100".to_string(), index: "2".to_string(), path, ..Form::default() },
        ];
        let made: Vec<Tx> = forms.iter().map(|form| read(form).unwrap_or_else(|why| panic!("{form:?}: {why}"))).collect();

        assert!(matches!(&made[0], Tx::Transfer { from, amount, .. } if *from == me() && amount.to_u256() == u256::new(1_500_000_000_000_000_000)));
        assert!(matches!(&made[2], Tx::ContractCall { data, .. } if matches!(data[1], TxCallArgumentType::U256(units) if units == u256::new(250))), "in the token's decimals");
        assert!(matches!(&made[3], Tx::ContractCall { data, .. } if data.len() == 2));
        assert!(matches!(&made[4], Tx::EvmCall { input, .. } if input.len() == 4 + 64));
        assert!(matches!(&made[10], Tx::Name { name, action: NameAction::Register { years: 2 } } if name == "alice"), "without its suffix");
        assert!(matches!(&made[12], Tx::Name { action: NameAction::Subname { owner, .. }, .. } if *owner == me()));
        assert_eq!(made[14], Tx::PrimaryName { name: None });

        // All of them together are a batch the chain would take from this sender.
        let batch = Tx::Batch { steps: made };

        assert_eq!(batch.verify(), Ok(()));

        for (form, wrong) in [
            (Form { kind: Kind::Coin, to: "nobody".to_string(), amount: "1".to_string(), ..Form::default() }, "does not read"),
            (Form { kind: Kind::Coin, to: to.clone(), amount: "lots".to_string(), ..Form::default() }, "not an amount"),
            (Form { kind: Kind::Coin, to: to.clone(), ..Form::default() }, "greater than zero"),
            (Form { kind: Kind::Burn, ..Form::default() }, "greater than zero"),
            (Form { kind: Kind::Token, token: token.clone(), to, ..Form::default() }, "more than nothing"),
            (Form { kind: Kind::Call, to: token.clone(), ..Form::default() }, "name the function"),
            (Form { kind: Kind::Call, to: token.clone(), function: "f".to_string(), args: "nonsense".to_string(), ..Form::default() }, "needs a type"),
            (Form { kind: Kind::Evm, to: evm.clone(), function: "f(uint256)".to_string(), ..Form::default() }, "takes 1 arguments"),
            (Form { kind: Kind::Deploy, ..Form::default() }, "source"),
            (Form { kind: Kind::Upgrade, to: token, engine: Engine::Evm, code: "0x00".to_string(), ..Form::default() }, "proxy"),
            (Form { kind: Kind::Name, name: "NOT A NAME".to_string(), ..Form::default() }, "not a name"),
            (Form { kind: Kind::Claim, ..Form::default() }, "whole number"),
        ] {
            let said = read(&form).unwrap_err();

            assert!(said.contains(wrong), "{wrong} in {said} for {form:?}");
        }
    }

    #[test]
    fn the_page_fits_phones_and_a_desktop_with_every_kind_of_form_and_a_full_list() {
        let form_for = |kind| Form { kind, ..Form::default() };

        for size in drawn::SCREENS {
            for (kind, _) in KINDS {
                let mut book = drawn::two_phrases();
                let mut holders = keys::open(&book).unwrap();
                let mut state = BatchState { form: form_for(kind), ..BatchState::default() };
                let from = holders[0].addresses()[0];

                for _ in 0..MAX_BATCH_STEPS {
                    state.add(Tx::Transfer { from, to: someone(), amount: Amount::U128(1) }).unwrap();
                }

                let (mut send, mut multisig) = (SendState::default(), super::super::multisig::MultisigState::default());
                state.from = Some(from);
                // Tall enough that nothing is left undrawn for being off the screen.
                let (width, _, shown) = drawn::page_shown(&mut book, &mut holders, (size.0, 40_000.0), |ui, ctx| show(ui, ctx, &mut state, &mut send, &mut multisig));
                let texts: Vec<&str> = shown.iter().map(|(text, _)| text.as_str()).collect();

                assert!(width <= size.0 + 1.0, "{kind:?} on {size:?}: {width}");
                assert_eq!(state.steps.len(), MAX_BATCH_STEPS);
                assert!(texts.contains(&"Step 32 of 32"), "{kind:?}: {texts:?}");
                assert_eq!(texts.iter().filter(|text| **text == "Remove").count(), 32, "each can be taken out");
                assert_eq!(texts.iter().filter(|text| **text == "Move up").count(), 31, "the first has nowhere to go");
                assert_eq!(texts.iter().filter(|text| **text == "Move down").count(), 31);
                assert!(texts.iter().any(|text| text.contains("at most 32")), "{texts:?}");
            }
        }
    }

    #[test]
    fn with_no_steps_the_page_says_so_and_a_batch_waits_for_one() {
        let mut book = drawn::two_phrases();
        let mut holders = keys::open(&book).unwrap();
        let (mut state, mut send, mut multisig) = (BatchState::default(), SendState::default(), super::super::multisig::MultisigState::default());
        let (_, _, shown) = drawn::page_shown(&mut book, &mut holders, drawn::SCREENS[0], |ui, ctx| show(ui, ctx, &mut state, &mut send, &mut multisig));

        assert!(shown.iter().any(|(text, _)| text.contains("None yet")));
        assert!(shown.iter().any(|(text, _)| text == "Add step"));
    }

    #[test]
    fn a_name_typed_for_a_step_is_asked_to_be_proven_and_an_address_is_not() {
        let mut state = BatchState::default();

        assert_eq!(state.wanted_name(), None);
        state.form.to = "alice.dev".to_string();
        assert_eq!(state.wanted_name().as_deref(), Some("alice.dev"));
        state.form.to = someone().to_string();
        assert_eq!(state.wanted_name(), None);
        state.form.to = format!("0x{}", "ab".repeat(20));
        assert_eq!(state.wanted_name(), None);
    }

    #[test]
    fn a_proposal_of_one_step_is_that_step_and_of_several_a_batch() {
        let token = format!("contract:0x{}", "11".repeat(32));
        let evm = format!("0x{}", "ab".repeat(20));
        let mut state = BatchState::default();
        let token_step = read(&Form { kind: Kind::Token, token, to: someone().to_string(), amount: "2".to_string(), ..Form::default() }).unwrap();

        state.add(token_step.clone()).unwrap();
        assert_eq!(state.proposal(&me()), Ok(token_step), "one step goes as it is, not as a batch of one");

        for form in [
            Form { kind: Kind::Evm, to: evm, data: "0x01020304".to_string(), ..Form::default() },
            Form { kind: Kind::Deploy, code: "return {}".to_string(), ..Form::default() },
            Form { kind: Kind::Name, name: "alice.dev".to_string(), ..Form::default() },
            Form { kind: Kind::Burn, amount: "1".to_string(), ..Form::default() },
        ] {
            state.add(read(&form).unwrap()).unwrap();
        }

        assert!(matches!(state.proposal(&me()), Ok(Tx::Batch { steps }) if steps.len() == 5));
        assert!(state.proposal(&someone()).unwrap_err().contains("takes from whom it is sent as"));

        state.clear();
        assert!(state.proposal(&me()).is_err(), "nothing to propose");
    }
}
