//! Atomic swaps between any two chains: coins or a token on this chain, Bitcoin, Ethereum or any
//! EVM chain, for coins or a token on another, with no one in between. Whoever starts makes an
//! offer, a code holding the two legs, the secret's hash and their parts, and keeps the secret in
//! the vault; the other reads it and shows back an answer with theirs. Then the starter locks
//! first, the other second, once the first stands, each under the same hash, and claims go the
//! other way: the starter's claim shows the secret, which the other claims with in turn.
//!
//! This chain's legs go through the swap contract the terms name, as calls the Send page reviews
//! and signs, on the offline signer too. An EVM chain's go through Swap.sol there, by a JSON-RPC
//! URL Settings gives ([evm]). Bitcoin's is an HTLC: paid from any Bitcoin wallet, and claimed or
//! refunded here, signed with this account's bitcoin key, or by the offline signer as a PSBT. The
//! wallet does not follow Bitcoin: what it needs of it is pasted, or, with an Esplora API set in
//! Settings, looked up and broadcast there.

mod evm;

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
    chainv4::swap::evm::{CHAINS, chain_name, deployed_address, parse_address, show_address, symbol},
    chainv4::swap::htlc::{Coins, Spend, revealed},
    chainv4::swap::{BitcoinLeg, ChainLeg, EvmLeg, FIRST_DEADLINE_MS, Item, Leg, Lock, LockState, Parties, SECOND_DEADLINE_MS, Settled, Side, Terms, bitcoin_settled, chain_settled, evm_settled, secret},
    egui::{RichText, Ui},
    serde_json::{Value, json},
    std::collections::HashMap,
    std::sync::{Arc, Mutex},
};

/// How often an open swap asks the contracts where its legs stand.
const ASK_EVERY: f64 = 10.0;

pub struct SwapsState {
    open: Option<usize>,
    form: Form,
    scan: Scan,
    /// An offer read, to answer.
    offered: Option<Box<Terms>>,
    /// What this chain's swap contract said of each swap, by its hash, and when it was last asked.
    status: HashMap<String, Result<Vec<Value>, String>>,
    /// What the contract's item said each NFT swap holds, by its hash.
    items: HashMap<String, Result<Vec<Value>, String>>,
    asked: HashMap<String, f64>,
    /// What Swap.sol said of each swap's EVM leg, by its hash and chain, and when it was asked.
    evm_locks: HashMap<(String, u64), Result<Lock, String>>,
    evm_asked: HashMap<String, f64>,
    /// Each EVM chain's height and finalized block, by chain id, as its RPC last said.
    evm_heads: HashMap<u64, (u64, Option<u64>)>,
    /// Acting on the open swap before a lock is final, asked for after a warning.
    anyway: bool,
    /// EVM chains: which URL serves which, the transaction on its way, and what they answered.
    evm: crate::views::evm::Link<evm::Read>,
    bitcoin: BitcoinForm,
    trouble: Option<String>,
    /// What came back from Esplora, for the next frame to take.
    looked: Arc<Mutex<Vec<Looked>>>,
    /// A Bitcoin spend asked of the offline signer.
    signer: Option<Box<Awaiting>>,
    /// Forgetting the open swap, asked once and waiting for the second tap.
    forgetting: bool,
    /// A spend sent to Bitcoin's peers by the light client, by the number it was sent under.
    #[cfg_attr(not(all(feature = "bitcoin-light", not(target_arch = "wasm32"))), allow(dead_code))]
    relayed: Option<u64>,
    /// A followed multisig to offer or answer as, rather than the account.
    as_multisig: Option<String>,
    /// What the node said of the multisigs swapped as, by address: their Bitcoin side is read
    /// from their members' xpubs.
    multisigs: HashMap<String, Value>,
    /// A multisig's Bitcoin spend of the HTLC, collecting its members' signatures.
    shared: Option<Box<Shared>>,
}

/// A multisig's HTLC spend on its way round its members: the PSBT, what it spends, and the
/// signer's code and a pasted PSBT to take signatures from.
struct Shared {
    psbt: chainv4::crypto::bitcoin::psbt::Psbt,
    spend: Spend,
    value: u64,
    scan: Scan,
    pasted: String,
}

impl Default for SwapsState {
    fn default() -> SwapsState {
        SwapsState {
            open: None,
            form: Form::default(),
            scan: Scan::default(),
            offered: None,
            status: HashMap::new(),
            items: HashMap::new(),
            asked: HashMap::new(),
            evm_locks: HashMap::new(),
            evm_asked: HashMap::new(),
            evm_heads: HashMap::new(),
            anyway: false,
            evm: Default::default(),
            bitcoin: BitcoinForm { fee_rate: "2".to_string(), ..BitcoinForm::default() },
            trouble: None,
            looked: Arc::default(),
            signer: None,
            forgetting: false,
            relayed: None,
            as_multisig: None,
            multisigs: HashMap::new(),
            shared: None,
        }
    }
}

/// Which chain a leg of a new offer is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Chain,
    Bitcoin,
    BitcoinTestnet,
    Evm,
}

/// What a leg on this chain or an EVM chain locks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Asset {
    /// The coin, or a fungible token.
    Fungible,
    /// One NFT of a collection.
    Nft,
    /// An amount of one id of a multi-token collection.
    Multi,
}

/// A leg of a new offer, as typed.
struct LegForm {
    kind: Kind,
    asset: Asset,
    amount: String,
    /// The token, or an NFT's collection.
    token: String,
    /// An NFT's id.
    id: String,
    chain_id: String,
    contract: String,
}

impl LegForm {
    fn new(kind: Kind) -> LegForm {
        LegForm { kind, asset: Asset::Fungible, amount: String::new(), token: String::new(), id: String::new(), chain_id: "11155111".to_string(), contract: show_address(&deployed_address()) }
    }

    /// The NFT it locks, from its id, for a leg that locks one.
    fn item(&self) -> Result<Option<Item>, String> {
        let id = || u256::from_str_radix(self.id.trim(), 10).map_err(|_| "an NFT's id, a whole number".to_string());

        match self.asset {
            Asset::Fungible => Ok(None),
            Asset::Nft => Ok(Some(Item { id: id()?, multi: false })),
            Asset::Multi => Ok(Some(Item { id: id()?, multi: true })),
        }
    }
}

/// The asset picker and an NFT's collection and id, for a leg on this chain or an EVM chain.
fn asset_form(ui: &mut Ui, palette: &theme::Palette, form: &mut LegForm, coin: &str, collection_hint: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.selectable_value(&mut form.asset, Asset::Fungible, "Coin or token");
        ui.selectable_value(&mut form.asset, Asset::Nft, "An NFT");
        ui.selectable_value(&mut form.asset, Asset::Multi, "Multi-token");
    });

    match form.asset {
        Asset::Fungible => {
            super::row(ui, palette, if form.token.trim().is_empty() { coin } else { "Token units" }, |ui| {
                ui.add(egui::TextEdit::singleline(&mut form.amount).hint_text("50").desired_width(160.0));
            });
        }
        Asset::Nft | Asset::Multi => {
            super::row(ui, palette, "Collection", |ui| {
                ui.add(egui::TextEdit::singleline(&mut form.token).hint_text(collection_hint).desired_width(f32::INFINITY));
            });
            super::row(ui, palette, "Id", |ui| {
                ui.add(egui::TextEdit::singleline(&mut form.id).hint_text("7").desired_width(160.0));
            });

            if form.asset == Asset::Multi {
                super::row(ui, palette, "How many", |ui| {
                    ui.add(egui::TextEdit::singleline(&mut form.amount).hint_text("1").desired_width(160.0));
                });
            }
        }
    }
}

struct Form {
    /// What the starter pays, locked first, and what they get.
    pay: LegForm,
    get: LegForm,
    first: String,
    second: String,
}

impl Default for Form {
    fn default() -> Form {
        Form {
            pay: LegForm::new(Kind::BitcoinTestnet),
            get: LegForm::new(Kind::Chain),
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
    /// Keeps what the node said of a multisig.
    pub fn multisig_answered(&mut self, ask: &Ask, result: &Result<Value, crate::node::Failure>) {
        if let (Ask::Multisig(address), Ok(value)) = (ask, result) {
            self.multisigs.insert(address.clone(), value.clone());
        }
    }

    /// Takes the contract's answer about a swap; false when the answer is for someone else.
    pub fn answered(&mut self, ask: &Ask, result: &Result<Value, crate::node::Failure>) -> bool {
        let Ask::Call(body) = ask else {
            return false;
        };
        let Ok(body) = serde_json::from_str::<Value>(body) else {
            return false;
        };
        let hash = body["args"][0]["value"].as_str().unwrap_or_default().to_string();

        let function = body["function"].as_str().unwrap_or_default();

        if !matches!(function, "get" | "item") || !self.asked.contains_key(&hash) {
            return false;
        }

        let answer = match result {
            Ok(value) if value["status"] == "returned" => Ok(value["values"].as_array().cloned().unwrap_or_default()),
            Ok(value) => Err(value["error"].as_str().unwrap_or("the contract would not say").to_string()),
            Err(failure) => Err(failure.message.clone()),
        };

        match function {
            "get" => self.status.insert(hash, answer),
            _ => self.items.insert(hash, answer),
        };

        true
    }
}

pub fn show(ui: &mut Ui, ctx: &mut Ctx, state: &mut SwapsState, send: &mut SendState, multisig: &mut super::multisig::MultisigState) {
    let palette = *ctx.palette;
    theme::title(ui, &palette, "Swaps", "Coins or a token on one chain for coins or a token on another, with no one in between: each side locks under one secret's hash.");

    take_looked(ctx, state);
    evm::take(ctx, state);
    state.evm.learn_chains(ctx);
    list(ui, ctx, state);

    if let Some(index) = state.open.filter(|index| *index < ctx.book.swaps.len()) {
        ui.add_space(14.0);

        match detail(ui, ctx, state, index) {
            // A multisig's step is its own transaction: proposed for its members to sign.
            Some(prepared) if matches!(prepared.from, Address::Multisig(_)) => {
                multisig.propose(prepared);
                *ctx.page = Page::Multisig;
            }
            Some(prepared) => {
                send.prepare(prepared);
                *ctx.page = Page::Send;
            }
            None => {}
        }

        if state.evm.pending.is_some() {
            ui.add_space(10.0);
            state.evm.show(ui, ctx, "a swap's transaction", "chainv4 · swap");
        }
    }

    ui.add_space(14.0);
    start(ui, ctx, state);
    ui.add_space(14.0);
    read(ui, ctx, state);
}

/// The swap contract a new offer's leg on this chain uses: Settings', or the one the chain's
/// directory names, proven, or else the one the node names. A swap made uses the one its terms
/// name.
fn contract(ctx: &Ctx) -> Option<Address> {
    if let Ok(given) = ctx.settings.swap_contract.trim().parse() {
        return Some(given);
    }

    let announced = ctx.chain.announced().and_then(|directory| directory.swap);

    announced.or_else(|| ctx.swap_contract.and_then(|text| text.parse().ok()))
}

/// A swap's hash, as text, from what the wallet keeps of it.
fn hash_of(record: &SwapRecord) -> Option<String> {
    Terms::from_text(&record.terms).ok().map(|terms| to_hex(&terms.hash))
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
                    "{} · {} for {} · {}",
                    if record.started { "started" } else { "answered" },
                    shown(ctx, &terms.first),
                    shown(ctx, &terms.second),
                    if terms.agreed() { "agreed" } else { "waiting for an answer" }
                ),
                Err(why) => why,
            };

            if ui.selectable_label(state.open == Some(index), RichText::new(line).color(palette.ink)).clicked() {
                state.open = Some(index);
                state.bitcoin = BitcoinForm { fee_rate: "2".to_string(), ..BitcoinForm::default() };
                state.trouble = None;
                state.forgetting = false;
                state.evm.pending = None;
                state.anyway = false;
            }
        }
    });
}

/// What a leg locks, as people read it: 0.001 tBTC, 50 DEV, 0.01 ETH on Sepolia.
fn shown(ctx: &Ctx, leg: &Leg) -> String {
    match leg {
        Leg::Chain(leg) => match leg.token {
            None => format::amount(&leg.amount.to_string(), ctx.coin.decimals, &ctx.coin.symbol),
            Some(token) => format!("{} units of {}", leg.amount, format::short(&token.to_string())),
        },
        Leg::Bitcoin(leg) => format!("{} {}", format::units(&leg.sats.to_string(), 8), leg.network().symbol()),
        Leg::Evm(leg) => match leg.token {
            None => format!("{} on {}", format::amount(&leg.amount.to_string(), 18, symbol(leg.chain_id)), chain_name(leg.chain_id)),
            Some(token) => format!("{} units of {} on {}", leg.amount, format::short(&show_address(&token)), chain_name(leg.chain_id)),
        },
    }
}

/// The account in use's bitcoin key on a network.
fn bitcoin_key(ctx: &Ctx, holder: &Holder, network: Network) -> Result<[u8; 33], String> {
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
        Some(key) => Ok(key),
        None if network == Network::Test => Err("Testnet swaps take an account of this wallet's words: its testnet key is theirs alone.".to_string()),
        None => Err("This account has no bitcoin key: pair it again from the signer, or pick another.".to_string()),
    }
}

/// Who an account is on a swap's chains: its first address here, its first Ethereum key's
/// address on EVM chains, and, when a leg is on Bitcoin, its bitcoin key on that network.
fn parties(ctx: &Ctx, holder: &Holder, testnet: Option<bool>) -> Result<Parties, String> {
    let network = |testnet: bool| if testnet { Network::Test } else { Network::Main };

    Ok(Parties {
        chain: holder.addresses().first().copied(),
        bitcoin: testnet.map(|testnet| bitcoin_key(ctx, holder, network(testnet))).transpose()?.map(chainv4::swap::BitcoinParty::Key),
        evm: holder.publics.iter().find_map(|public| match public.address() {
            Address::Secp256k1(bytes) => Some(bytes),
            _ => None,
        }),
    })
}

/// Who a swap is made as: the account, or a followed multisig, its address here and, on a Bitcoin
/// leg, its members' keys at the swap's own index, from what the node said of it. A multisig
/// swaps on this chain and Bitcoin; its EVM side is to come.
fn parties_as(ctx: &Ctx, known: &HashMap<String, Value>, holder: &Holder, multisig: Option<&str>, testnet: Option<bool>, hash: &[u8; 32]) -> Result<Parties, String> {
    let Some(multisig) = multisig else {
        return parties(ctx, holder, testnet);
    };
    let address: Address = multisig.parse().map_err(|_| "Not a multisig's address.".to_string())?;
    let value = known.get(multisig).ok_or("Asking the node about the multisig…")?;
    let bitcoin = match testnet {
        None => None,
        Some(testnet) => {
            let wallet = super::multisig::bitcoin::wallet_of(value).ok_or("The multisig's members name no Bitcoin xpubs: it has no side on Bitcoin.")?;

            if (wallet.network == Network::Test) != testnet {
                return Err(format!("The multisig is on {}, not this swap's network.", wallet.network.name()));
            }

            Some(chainv4::swap::BitcoinParty::of_multisig(&wallet, chainv4::swap::multisig_index(hash)).ok_or("A member's key that does not derive.")?)
        }
    };

    Ok(Parties { chain: Some(address), bitcoin, evm: None })
}

/// Picks who to swap as: the account, or one of the multisigs the Multisig page follows.
fn as_picker(ui: &mut Ui, ctx: &mut Ctx, state: &mut SwapsState, salt: &str) {
    if ctx.settings.multisigs.is_empty() {
        return;
    }

    let palette = *ctx.palette;
    super::row(ui, &palette, "Swap as", |ui| {
        egui::ComboBox::from_id_salt(salt)
            .selected_text(state.as_multisig.as_deref().map_or("this account".to_string(), |address| format!("the multisig {}", format::short(address))))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut state.as_multisig, None, "this account");

                for address in ctx.settings.multisigs.clone() {
                    ui.selectable_value(&mut state.as_multisig, Some(address.clone()), format!("the multisig {}", format::short(&address)));
                }
            });
    });

    if let Some(address) = &state.as_multisig
        && !state.multisigs.contains_key(address)
    {
        ctx.node.ask(Ask::Multisig(address.clone()));
    }
}

/// Whether a swap's Bitcoin leg, if it has one, is on a test network.
fn testnet(terms: &Terms) -> Option<bool> {
    terms.bitcoin().map(|(_, leg)| leg.testnet)
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

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Start a swap");
        ui.label(RichText::new("You make the secret and lock first, with the longer deadline. Send the offer's code to the other side, and read their answer below.").color(palette.soft));
        ui.add_space(8.0);

        leg_form(ui, ctx, &mut state.form.pay, "You pay", "pay");
        ui.add_space(6.0);
        leg_form(ui, ctx, &mut state.form.get, "You get", "get");
        ui.add_space(6.0);

        let form = &mut state.form;
        super::row(ui, &palette, "Deadlines", |ui| {
            ui.add(egui::TextEdit::singleline(&mut form.first).desired_width(40.0));
            ui.label("h for yours,");
            ui.add(egui::TextEdit::singleline(&mut form.second).desired_width(40.0));
            ui.label("h for theirs");
        });
        ui.add_space(6.0);

        as_picker(ui, ctx, state, "offer-as");
        ui.add_space(6.0);

        if theme::primary(ui, &palette, "Make the offer", true).clicked() {
            match offer(ctx, state) {
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

/// One leg of a new offer: its chain, and what is locked there.
fn leg_form(ui: &mut Ui, ctx: &Ctx, form: &mut LegForm, label: &str, salt: &str) {
    let palette = *ctx.palette;
    theme::caps(ui, &palette, label);
    ui.horizontal_wrapped(|ui| {
        ui.selectable_value(&mut form.kind, Kind::Chain, "This chain");
        ui.selectable_value(&mut form.kind, Kind::Bitcoin, "Bitcoin");
        ui.selectable_value(&mut form.kind, Kind::BitcoinTestnet, "Bitcoin testnet");
        ui.selectable_value(&mut form.kind, Kind::Evm, "An EVM chain");
    });

    match form.kind {
        Kind::Chain => {
            asset_form(ui, &palette, form, &ctx.coin.symbol, "contract:0x…");

            if form.asset == Asset::Fungible {
                super::row(ui, &palette, "Token", |ui| {
                    ui.add(egui::TextEdit::singleline(&mut form.token).hint_text("empty for the coin, or its contract:0x…").desired_width(f32::INFINITY));
                });
            }

            if contract(ctx).is_none() {
                ui.colored_label(palette.warn, "No swap contract found here: no name resolves to one, and the node names none. Set one under Settings, Swaps.");
            }
        }
        Kind::Bitcoin | Kind::BitcoinTestnet => {
            let network = if form.kind == Kind::BitcoinTestnet { Network::Test } else { Network::Main };
            super::row(ui, &palette, network.symbol(), |ui| {
                ui.add(egui::TextEdit::singleline(&mut form.amount).hint_text("0.001").desired_width(160.0));
            });
        }
        Kind::Evm => {
            let chain_id = form.chain_id.trim().parse::<u64>().ok();
            super::row(ui, &palette, "Chain", |ui| {
                egui::ComboBox::from_id_salt(("swap-chain", salt)).selected_text(chain_id.map_or("Pick one".to_string(), chain_name)).show_ui(ui, |ui| {
                    for (id, name, _) in CHAINS {
                        if ui.selectable_label(chain_id == Some(*id), *name).clicked() {
                            form.chain_id = id.to_string();
                        }
                    }
                });
                ui.add(egui::TextEdit::singleline(&mut form.chain_id).hint_text("its id").desired_width(90.0));
            });
            let coin = chain_id.map_or("ETH", symbol);
            asset_form(ui, &palette, form, coin, "an ERC-721's or ERC-1155's 0x…");

            if form.asset == Asset::Fungible {
                super::row(ui, &palette, "Token", |ui| {
                    ui.add(egui::TextEdit::singleline(&mut form.token).hint_text("empty for the coin, or an ERC-20's 0x…").desired_width(f32::INFINITY));
                });
            }
            super::row(ui, &palette, "Swap.sol", |ui| {
                ui.add(egui::TextEdit::singleline(&mut form.contract).font(egui::TextStyle::Monospace).desired_width(f32::INFINITY));
            });
        }
    }
}

/// A leg from the form.
fn leg(ctx: &Ctx, form: &LegForm) -> Result<Leg, String> {
    let units = |text: &str, decimals: u8, what: &str| format::parse_units(text, decimals).and_then(|digits| u256::from_str_radix(&digits, 10).ok()).filter(|amount| *amount != u256::ZERO).ok_or_else(|| format!("an amount {what}"));

    let item = form.item()?;
    // One NFT is one; a multi-token amount, whole units.
    let count = |what: &str| match item {
        Some(Item { multi: false, .. }) => Ok(u256::from(1u64)),
        _ => units(&form.amount, 0, what),
    };

    match form.kind {
        Kind::Chain => {
            let token = match form.token.trim() {
                "" if item.is_some() => return Err("an NFT's collection, its contract:0x…".to_string()),
                "" => None,
                text => Some(text.parse::<Address>().map_err(|_| "the token is a contract's address".to_string())?),
            };
            let amount = match item {
                Some(_) => count("to lock on this chain")?,
                None => units(&form.amount, if token.is_some() { 0 } else { ctx.coin.decimals }, "to lock on this chain")?,
            };
            let contract = contract(ctx).ok_or("no swap contract on this chain: set one under Settings, Swaps, or register the swap name for it")?;

            Ok(Leg::Chain(ChainLeg { item, contract, token, amount, payer: None, receiver: None }))
        }
        Kind::Bitcoin | Kind::BitcoinTestnet => {
            let sats = units(&form.amount, 8, "of bitcoin, such as 0.001")?;

            Ok(Leg::Bitcoin(BitcoinLeg { testnet: form.kind == Kind::BitcoinTestnet, sats: u64::try_from(sats).map_err(|_| "more bitcoin than there is")?, payer: None, receiver: None }))
        }
        Kind::Evm => {
            let chain_id = form.chain_id.trim().parse::<u64>().map_err(|_| "an EVM chain, by its id")?;
            let token = match form.token.trim() {
                "" if item.is_some() => return Err("an NFT's collection, its 0x…".to_string()),
                "" => None,
                text => Some(parse_address(text)?),
            };
            let what = format!("to lock on {}", chain_name(chain_id));
            let amount = match item {
                Some(_) => count(&what)?,
                None => units(&form.amount, if token.is_some() { 0 } else { 18 }, &what)?,
            };

            Ok(Leg::Evm(EvmLeg { item, chain_id, contract: parse_address(&form.contract)?, token, amount, payer: None, receiver: None }))
        }
    }
}

/// An offer from the form, with a new secret kept beside it.
fn offer(ctx: &Ctx, state: &SwapsState) -> Result<SwapRecord, String> {
    let form = &state.form;
    let holder = ctx.holder().ok_or("pick an account first")?;
    let (first, second) = (leg(ctx, &form.pay)?, leg(ctx, &form.get)?);
    let hours = |text: &str| text.trim().parse::<u64>().map(|hours| hours * 3_600_000).map_err(|_| "deadlines are whole hours".to_string());
    let (first_deadline, second_deadline) = (hours(&form.first)?, hours(&form.second)?);
    let bitcoin = [&first, &second].into_iter().find_map(|leg| match leg {
        Leg::Bitcoin(leg) => Some(leg.testnet),
        _ => None,
    });
    let (secret, hash) = secret()?;
    let me = parties_as(ctx, &state.multisigs, holder, state.as_multisig.as_deref(), bitcoin, &hash)?;
    let terms = Terms::offer(hash, first, second, ctx.now + first_deadline, ctx.now + second_deadline, &me)?;
    terms.check(ctx.now)?;

    Ok(SwapRecord { terms: terms.to_text(), secret: Some(to_hex(&secret)), account: holder.id(), made: ctx.now, started: true, funding: None, multisig: state.as_multisig.clone() })
}

fn read(ui: &mut Ui, ctx: &mut Ctx, state: &mut SwapsState) {
    let palette = *ctx.palette;

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Read an offer or an answer");
        ui.label(RichText::new("An offer someone sent you, to answer; or the answer to one of yours, which completes its terms.").color(palette.soft));
        ui.add_space(8.0);

        match state.scan.show(ui, &palette) {
            Some(Message::Swap(terms)) => match ctx.book.swaps.iter().position(|record| hash_of(record) == Some(to_hex(&terms.hash))) {
                // The answer to one of ours: the same terms, with their parts in.
                Some(index) if terms.agreed() => {
                    let kept = Terms::from_text(&ctx.book.swaps[index].terms).expect("kept terms read");

                    match terms.answers(&kept) {
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
        as_picker(ui, ctx, state, "answer-as");
        ui.horizontal(|ui| {
            if theme::primary(ui, &palette, "Answer it", true).clicked() {
                match answer(ctx, state, &terms) {
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

/// The answer to an offer: its terms with this account's parts in.
fn answer(ctx: &Ctx, state: &SwapsState, offered: &Terms) -> Result<SwapRecord, String> {
    let holder = ctx.holder().ok_or("pick an account first")?;
    let me = parties_as(ctx, &state.multisigs, holder, state.as_multisig.as_deref(), testnet(offered), &offered.hash)?;
    let terms = offered.answer(&me)?;
    terms.check(ctx.now)?;

    Ok(SwapRecord { terms: terms.to_text(), secret: None, account: holder.id(), made: ctx.now, started: false, funding: None, multisig: state.as_multisig.clone() })
}

/// The terms, laid out.
fn summary(ui: &mut Ui, ctx: &Ctx, terms: &Terms) {
    let palette = *ctx.palette;
    theme::fitted(ui, &format!("{} ⇄ {}", shown(ctx, &terms.first), shown(ctx, &terms.second)), 26.0, 14.0, egui::FontFamily::Name(theme::SERIF.into()), palette.ink);
    super::row(ui, &palette, "Starter pays", |ui| {
        ui.label(format!("on {}, locked first", terms.first.chain_name()));
    });

    for (side, leg) in terms.legs() {
        super::row(ui, &palette, &format!("{} lock", leg.chain_name()), |ui| {
            ui.label(format!("ends {}", when(terms.deadline(side), ctx.now)));
        });
    }

    super::row(ui, &palette, "Hash", |ui| {
        ui.label(theme::mono(format::short(&to_hex(&terms.hash))));
    });

    for (_, leg) in terms.legs() {
        match leg {
            Leg::Chain(leg) => super::row(ui, &palette, "Contract", |ui| super::address(ui, &palette, &leg.contract.to_string())),
            Leg::Evm(leg) => super::row(ui, &palette, "Swap.sol", |ui| super::address(ui, &palette, &show_address(&leg.contract))),
            Leg::Bitcoin(_) => {}
        }
    }
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

/// Asks a leg's contract where it stands, when it was not asked lately.
fn ask_leg(ctx: &mut Ctx, state: &mut SwapsState, terms: &Terms, leg: &Leg, now: f64) {
    let hash = to_hex(&terms.hash);
    let stale = |asked: Option<&f64>| asked.is_none_or(|asked| now - asked > ASK_EVERY);

    match leg {
        Leg::Chain(leg) if stale(state.asked.get(&hash)) => {
            state.asked.insert(hash.clone(), now);
            ctx.node.ask(Ask::Call(json!({ "contract": leg.contract.to_string(), "function": "get", "args": [{ "type": "string", "value": hash }] }).to_string()));

            // Which NFT it holds, asked only of legs that lock one: contracts from before NFTs have
            // no item.
            if leg.item.is_some() {
                ctx.node.ask(Ask::Call(json!({ "contract": leg.contract.to_string(), "function": "item", "args": [{ "type": "string", "value": hash }] }).to_string()));
            }
        }
        Leg::Evm(leg) if stale(state.evm_asked.get(&hash)) => {
            if let Some(url) = state.evm.url_for(ctx, leg.chain_id) {
                state.evm_asked.insert(hash, now);
                evm::ask_lock(ctx, state, &url, leg, &terms.hash);
                evm::ask_heads(ctx, state, &url, leg.chain_id);
            }
        }
        _ => {}
    }
}

/// A leg's lock, as its contract last said; None for Bitcoin's, which is no contract's.
fn lock_of(ctx: &Ctx, state: &SwapsState, terms: &Terms, leg: &Leg) -> Option<Result<Lock, String>> {
    let hash = to_hex(&terms.hash);
    let text = |value: &Value| value["value"].as_str().map_or_else(|| value["value"].to_string(), str::to_string);

    match leg {
        Leg::Chain(chain) => Some(match (state.status.get(&hash), chain.item, state.items.get(&hash)) {
            (None, _, _) | (_, Some(_), None) => Err("asking".to_string()),
            (Some(answer), None, _) => answer.clone().and_then(|values| Lock::from_chain(&values.iter().map(text).collect::<Vec<_>>())),
            (Some(answer), Some(_), Some(item)) => answer.clone().and_then(|values| Lock::from_chain(&values.iter().map(text).collect::<Vec<_>>())).and_then(|lock| {
                item.clone().and_then(|values| lock.with_chain_item(&values.iter().map(text).collect::<Vec<_>>()))
            }),
        }),
        Leg::Evm(leg) => Some(match state.evm.url_for(ctx, leg.chain_id) {
            None => Err(format!("no JSON-RPC URL for {} under Settings, Swaps", chain_name(leg.chain_id))),
            Some(_) => state.evm_locks.get(&(hash, leg.chain_id)).cloned().unwrap_or_else(|| Err("asking".to_string())),
        }),
        Leg::Bitcoin(_) => None,
    }
}

/// Where a leg stands, in words.
fn describe(leg: &Leg, lock: Option<&Result<Lock, String>>, deadline: u64) -> String {
    match lock {
        None => "an HTLC: look its address up on a Bitcoin explorer, or with Esplora".to_string(),
        Some(Err(why)) => format!("not known yet: {why}"),
        Some(Ok(lock)) => match lock.state {
            LockState::None => "nothing locked yet".to_string(),
            LockState::Locked => match leg.stands(lock, deadline) {
                Ok(()) => "locked as the terms say, waiting for its claim".to_string(),
                Err(why) => format!("locked, but not as the terms say: {why}"),
            },
            LockState::Claimed => "claimed: the secret is out".to_string(),
            LockState::Refunded => "refunded".to_string(),
        },
    }
}

/// The open swap: what to share, where each leg stands, and what to do next.
fn detail(ui: &mut Ui, ctx: &mut Ctx, state: &mut SwapsState, index: usize) -> Option<Prepared> {
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
        if let Some(address) = &record.multisig
            && !state.multisigs.contains_key(address)
        {
            ctx.node.ask(Ask::Multisig(address.clone()));
        }

        let me = match parties_as(ctx, &state.multisigs, &ctx.holders[holder], record.multisig.as_deref(), testnet(&terms), &terms.hash) {
            Ok(me) => me,
            Err(why) => {
                ui.colored_label(palette.bad, why);

                return;
            }
        };
        let (pays, gets) = terms.mine(&me);

        if pays.is_none() && gets.is_none() {
            ui.colored_label(palette.bad, "This account's keys are not this swap's.");

            return;
        }

        // Where each leg stands, asked now and then.
        let time = ui.input(|input| input.time);
        for (_, leg) in terms.legs() {
            ask_leg(ctx, state, &terms, leg, time);
        }

        let locks = terms.legs().map(|(_, leg)| lock_of(ctx, state, &terms, leg));
        let lock = |side: Side| locks[match side { Side::First => 0, Side::Second => 1 }].as_ref();
        let paid = htlc_seen(ctx, &terms);
        let settles = [Side::First, Side::Second].map(|side| settled_of(ctx, state, &terms, side, lock(side), paid.as_deref()));
        let settled = |side: Side| &settles[match side { Side::First => 0, Side::Second => 1 }];

        for (side, leg) in terms.legs() {
            super::row(ui, &palette, &leg.chain_name(), |ui| {
                let shown = match (lock(side), leg) {
                    (Some(Ok(Lock { state: LockState::Locked | LockState::Claimed, .. })), _) | (None, Leg::Bitcoin(_)) => format!("{} · {}", describe(leg, lock(side), terms.deadline(side)), settled(side).describe()),
                    _ => describe(leg, lock(side), terms.deadline(side)),
                };
                ui.label(shown);
            });
        }

        // A secret the other side showed, by their claim on a contract.
        if record.secret.is_none()
            && let Some(secret) = locks.iter().flatten().find_map(|lock| lock.as_ref().ok().and_then(|lock| lock.secret)).filter(|secret| sha256(secret) == terms.hash)
        {
            ctx.book.swaps[index].secret = Some(to_hex(&secret));
            *ctx.changed = true;
        }

        // Or by their claim of the HTLC, as Bitcoin's peers show it to the light client.
        if ctx.book.swaps[index].secret.is_none()
            && let Some(secret) = htlc_secret(ctx, &terms)
        {
            ctx.book.swaps[index].secret = Some(to_hex(&secret));
            *ctx.changed = true;
        }

        let secret = ctx.book.swaps[index].secret.as_deref().and_then(from_hex).and_then(|bytes| <[u8; 32]>::try_from(bytes).ok());

        if let (Some(htlc), Some((_, leg))) = (terms.htlc(), terms.bitcoin()) {
            super::row(ui, &palette, "Bitcoin HTLC", |ui| {
                ui.vertical(|ui| {
                    super::address(ui, &palette, &htlc.address(leg.network()));
                    ui.label(RichText::new(format!("{} {}", format::units(&leg.sats.to_string(), 8), leg.network().symbol())).small().color(palette.muted));

                    if let Some(paid) = &paid {
                        let line = match paid.iter().max_by_key(|coin| coin.sats) {
                            None => "nothing paid to it yet, as Bitcoin's peers show it".to_string(),
                            Some(coin) => format!(
                                "paid {} sats, {} confirmation{}{}",
                                coin.sats,
                                coin.depth,
                                if coin.depth == 1 { "" } else { "s" },
                                if coin.spent { ", and spent" } else { "" }
                            ),
                        };
                        ui.label(RichText::new(line).small().color(palette.soft));
                    }
                });
            });
        }
        ui.add_space(6.0);

        // The first lock, which must stand before the second goes in: a contract's is checked
        // here; Bitcoin's is the second side's to check.
        let first_stands = match (&terms.first, lock(Side::First), &paid) {
            // Bitcoin's, when the light client follows it: paid in full, in a block.
            (Leg::Bitcoin(leg), None, Some(paid)) => match paid.iter().any(|coin| coin.sats >= leg.sats && coin.depth >= 1) {
                true => Ok(()),
                false => Err("the HTLC holds nothing yet, as Bitcoin's peers show it".to_string()),
            },
            (_, None, _) => Ok(()),
            (leg, Some(Ok(lock)), _) => leg.stands(lock, terms.first_deadline),
            (_, Some(Err(why)), _) => Err(why.clone()),
        };

        // Paying: the starter first; the other only once the first lock stands.
        if let Some(side) = pays {
            let (leg, deadline, first) = (terms.leg(side), terms.deadline(side), side == Side::First);
            let nothing_yet = matches!(lock(side), Some(Ok(Lock { state: LockState::None, .. })));
            let locked = matches!(lock(side), Some(Ok(Lock { state: LockState::Locked, .. })));
            // Theirs must stand as the terms say, and be final: one a reorganization could take
            // away would leave yours to them for a secret they keep.
            let unsettled = (!first && first_stands.is_ok() && !settled(Side::First).is_final()).then(|| settled(Side::First).describe());

            if let Some(why) = &unsettled
                && (nothing_yet || matches!(leg, Leg::Bitcoin(_)))
            {
                ui.colored_label(palette.warn, format!("Their lock on {} is {why}. Lock yours once it is final.", terms.first.chain_name()));
                ui.checkbox(&mut state.anyway, "I checked it myself: lock anyway");
            }

            let waiting = (!first).then(|| first_stands.clone().err().or_else(|| unsettled.clone().filter(|_| !state.anyway))).flatten();

            if let Some(why) = &waiting
                && (nothing_yet || matches!(leg, Leg::Bitcoin(_)))
            {
                ui.colored_label(palette.warn, format!("Lock yours once theirs stands, on {}: {why}.", terms.first.chain_name()));
            }

            match leg {
                Leg::Bitcoin(bitcoin) if waiting.is_none() => {
                    let address = terms.htlc().expect("agreed terms have their HTLC").address(bitcoin.network());
                    ui.label(RichText::new(match (first, &terms.first) {
                        (true, _) => "Pay the HTLC exactly this from your Bitcoin wallet: you lock first.".to_string(),
                        (false, other) => format!("Their lock on {} stands: pay the HTLC exactly this from your Bitcoin wallet.", other.chain_name()),
                    }).color(palette.ink));
                    let uri = format!("bitcoin:{address}?amount={}", format::units(&bitcoin.sats.to_string(), 8));
                    let side = ui.available_width().clamp(180.0, 300.0);
                    ui.vertical_centered(|ui| super::receive::qr(ui, &uri, side));
                }
                Leg::Chain(chain) if nothing_yet => {
                    ui.label(RichText::new(match (first, &terms.first) {
                        (true, _) => "Lock what you pay on this chain: you lock first.".to_string(),
                        (false, Leg::Bitcoin(_)) => "Lock on this chain only once the HTLC above holds the bitcoin, confirmed: check it on a Bitcoin explorer, or look it up below.".to_string(),
                        (false, other) => format!("Their lock on {} stands: lock yours.", other.chain_name()),
                    }).color(palette.ink));
                    let ready = waiting.is_none();
                    ui.horizontal(|ui| {
                        let recipient = TxCallArgumentType::Address(chain.receiver.expect("agreed"));
                        let deadline = TxCallArgumentType::I64(deadline as i64);
                        let text = TxCallArgumentType::String(hash.clone());
                        let from = me.chain.expect("a payer here has an address here");

                        match (chain.token, chain.item) {
                            (None, _) => {
                                if theme::primary(ui, &palette, "Lock", ready).clicked() {
                                    prepared = Some(call(from, chain.contract, "lock", vec![text, recipient, deadline], chain.amount, "Lock a swap".to_string()));
                                }
                            }
                            // An NFT moves as a token does: the contract let to, then the lock.
                            (Some(collection), Some(item)) => {
                                let let_move = match item.multi {
                                    false => call(from, collection, "approve", vec![TxCallArgumentType::Address(chain.contract), TxCallArgumentType::U256(item.id)], u256::ZERO, "Let the swap contract move the NFT".to_string()),
                                    true => call(from, collection, "set_approval_for_all", vec![TxCallArgumentType::Address(chain.contract), TxCallArgumentType::Bool(true)], u256::ZERO, "Make the swap contract an operator".to_string()),
                                };
                                let lock = match item.multi {
                                    false => call(from, chain.contract, "lock_nft", vec![text, recipient, deadline, TxCallArgumentType::Address(collection), TxCallArgumentType::U256(item.id)], u256::ZERO, "Lock a swap's NFT".to_string()),
                                    true => call(
                                        from,
                                        chain.contract,
                                        "lock_multi",
                                        vec![text, recipient, deadline, TxCallArgumentType::Address(collection), TxCallArgumentType::U256(item.id), TxCallArgumentType::U256(chain.amount)],
                                        u256::ZERO,
                                        "Lock a swap's tokens".to_string(),
                                    ),
                                };

                                if theme::quiet(ui, &palette, "1. Let the contract move it").clicked() {
                                    prepared = Some(let_move);
                                }

                                if theme::primary(ui, &palette, "2. Lock", ready).clicked() {
                                    prepared = Some(lock);
                                }
                            }
                            (Some(token), None) => {
                                if theme::quiet(ui, &palette, "1. Let the contract move the token").clicked() {
                                    prepared = Some(call(from, token, "approve", vec![TxCallArgumentType::Address(chain.contract), TxCallArgumentType::U256(chain.amount)], u256::ZERO, "Approve the swap contract".to_string()));
                                }

                                if theme::primary(ui, &palette, "2. Lock", ready).clicked() {
                                    prepared = Some(call(from, chain.contract, "lock_token", vec![text, recipient, deadline, TxCallArgumentType::Address(token), TxCallArgumentType::U256(chain.amount)], u256::ZERO, "Lock a swap's token".to_string()));
                                }
                            }
                        }
                    });
                }
                Leg::Evm(on) if nothing_yet => {
                    ui.label(RichText::new(match (first, &terms.first) {
                        (true, _) => format!("Lock what you pay on {}: you lock first.", chain_name(on.chain_id)),
                        (false, Leg::Bitcoin(_)) => format!("Lock on {} only once the HTLC above holds the bitcoin, confirmed: check it on a Bitcoin explorer, or look it up below.", chain_name(on.chain_id)),
                        (false, other) => format!("Their lock on {} stands: lock yours on {}.", other.chain_name(), chain_name(on.chain_id)),
                    }).color(palette.ink));

                    match on.lock_calls(&terms.hash, deadline) {
                        Ok(calls) => {
                            let steps = calls.len();
                            ui.horizontal(|ui| {
                                for (step, call) in calls.into_iter().enumerate() {
                                    let label = match steps {
                                        1 => format!("Lock on {}", chain_name(on.chain_id)),
                                        _ => format!("{}. {}", step + 1, call.what),
                                    };
                                    // A token's approval may go first; the lock waits for the first leg.
                                    let ready = state.evm.pending.is_none() && (step + 1 < steps || waiting.is_none());

                                    if theme::primary(ui, &palette, &label, ready).clicked() {
                                        send_evm(ctx, state, &terms, on, call, me.evm);
                                    }
                                }
                            });
                        }
                        Err(why) => {
                            ui.colored_label(palette.bad, why);
                        }
                    }
                }
                _ => {}
            }

            // Taking back what was locked, after its deadline.
            if locked && ctx.now >= deadline {
                match leg {
                    Leg::Chain(chain) => {
                        if theme::quiet(ui, &palette, "Take back your lock on this chain").clicked() {
                            prepared = Some(call(me.chain.expect("a payer here"), chain.contract, "refund", vec![TxCallArgumentType::String(hash.clone())], u256::ZERO, "Refund a swap".to_string()));
                        }
                    }
                    Leg::Evm(on) => {
                        if theme::quiet(ui, &palette, &format!("Take back your lock on {}", chain_name(on.chain_id))).clicked() && state.evm.pending.is_none() {
                            send_evm(ctx, state, &terms, on, on.refund_call(&terms.hash), me.evm);
                        }
                    }
                    Leg::Bitcoin(_) => {}
                }
            }
        }

        // Receiving: claimed with the secret, once it is locked as the terms say.
        if let Some(side) = gets {
            let (leg, deadline) = (terms.leg(side), terms.deadline(side));
            let stands = lock(side).and_then(|lock| lock.as_ref().ok()).filter(|lock| lock.state == LockState::Locked).map(|lock| leg.stands(lock, deadline));

            match (leg, secret, stands) {
                (Leg::Bitcoin(_), _, _) => {}
                (_, _, Some(Err(why))) => {
                    ui.colored_label(palette.bad, format!("Their lock is not as the terms say, {why}: do not claim it, which would show the secret. Take yours back after its deadline."));
                }
                (_, Some(secret), Some(Ok(()))) => {
                    // The starter's claim shows the secret: only once their lock is final.
                    let held = record.started && !settled(side).is_final();

                    if held {
                        ui.colored_label(palette.warn, format!("Their lock on {} is {}. Claim once it is final: claiming shows the secret.", leg.chain_name(), settled(side).describe()));
                        ui.checkbox(&mut state.anyway, "I checked it myself: claim anyway");
                    }

                    ui.label(RichText::new(match record.started {
                        true => format!("Their lock on {} stands: claim it. That shows the secret, which they claim yours with.", leg.chain_name()),
                        false => format!("Claim what they locked for you on {}, with the secret they showed.", leg.chain_name()),
                    }).color(palette.ink));
                    let open = ctx.now < deadline && (!held || state.anyway);

                    match leg {
                        Leg::Chain(chain) => {
                            if theme::primary(ui, &palette, "Claim on this chain", open).clicked() {
                                // Anyone may send the claim; this account sends it from here.
                                prepared = Some(call(me.chain.expect("an address here"), chain.contract, "claim", vec![TxCallArgumentType::String(to_hex(&secret))], u256::ZERO, "Claim a swap".to_string()));
                            }
                        }
                        Leg::Evm(on) => {
                            if theme::primary(ui, &palette, &format!("Claim on {}", chain_name(on.chain_id)), open && state.evm.pending.is_none()).clicked() {
                                send_evm(ctx, state, &terms, on, on.claim_call(&secret), me.evm);
                            }
                        }
                        Leg::Bitcoin(_) => {}
                    }
                }
                (_, None, Some(Ok(()))) => match pays.map(|side| terms.leg(side)) {
                    Some(Leg::Bitcoin(_)) => learn_secret(ui, ctx, state, index, &terms),
                    Some(other) => {
                        ui.label(RichText::new(format!("They claim yours on {} first, which shows the secret: it shows here then.", other.chain_name())).small().color(palette.muted));
                    }
                    None => {}
                },
                _ => {}
            }
        }

        // Bitcoin's leg: a claim with the secret, or a refund after the lock time.
        if let (Some(htlc), Some((side, _))) = (terms.htlc(), terms.bitcoin()) {
            let (pays_bitcoin, gets_bitcoin) = (pays == Some(side), gets == Some(side));
            let spend = match (gets_bitcoin, pays_bitcoin, secret) {
                (true, _, Some(secret)) => Some(Spend::Claim { secret }),
                (_, true, _) if ctx.now / 1_000 >= u64::from(htlc.locktime) => Some(Spend::Refund),
                _ => None,
            };

            if gets_bitcoin && secret.is_none() {
                ui.label(RichText::new("You claim the bitcoin once they show the secret, by claiming theirs: it shows here then.").small().color(palette.muted));
            }

            if let Some(spend) = spend {
                // The starter's claim of the bitcoin shows the secret: only once it is deep enough.
                let held = (matches!(spend, Spend::Claim { .. }) && record.started && !settled(side).is_final()).then(|| settled(side).describe());
                ui.add_space(8.0);
                bitcoin_spend(ui, ctx, state, &terms, holder, spend, index, held);
            }
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
                    state.evm.pending = None;
                }
                false => state.forgetting = true,
            }
        }
    });

    prepared
}

/// Starts a call on an EVM leg on its way, through the chain's RPC, from this account's address
/// there.
fn send_evm(ctx: &Ctx, state: &mut SwapsState, terms: &Terms, leg: &EvmLeg, call: chainv4::swap::evm::Call, from: Option<[u8; 20]>) {
    match (state.evm.url_for(ctx, leg.chain_id), from) {
        (Some(url), Some(from)) => state.evm.start(ctx, to_hex(&terms.hash), call, leg.chain_id, url, from),
        (None, _) => state.trouble = Some(format!("Give a JSON-RPC URL for {} under Settings, Swaps, to send there.", chain_name(leg.chain_id))),
        (_, None) => state.trouble = Some("This account has no Ethereum key to send with.".to_string()),
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
#[allow(clippy::too_many_arguments)]
fn bitcoin_spend(ui: &mut Ui, ctx: &mut Ctx, state: &mut SwapsState, terms: &Terms, holder: usize, spend: Spend, index: usize, held: Option<String>) {
    let palette = *ctx.palette;
    let (Some(htlc), Some((_, leg))) = (terms.htlc(), terms.bitcoin()) else {
        return;
    };
    let network = leg.network();
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

    // The coins the light client saw pay it, still unspent.
    if state.bitcoin.funding.is_empty()
        && let Some(coin) = htlc_seen(ctx, terms).and_then(|paid| paid.into_iter().find(|coin| !coin.spent))
    {
        (state.bitcoin.funding, state.bitcoin.sats) = (coin.outpoint, coin.sats.to_string());
    }

    // A multisig's coins come back to it: its receiving address at the swap's index.
    let shared_wallet = ctx.book.swaps[index].multisig.as_ref().and_then(|address| state.multisigs.get(address)).and_then(super::multisig::bitcoin::wallet_of);

    if state.bitcoin.to.is_empty() {
        let back = match (&ctx.book.swaps[index].multisig, &shared_wallet) {
            (Some(_), Some(wallet)) => chainv4::crypto::bitcoin::multisig::bitcoin_address(wallet.threshold, &wallet.xpubs, network, 0, chainv4::swap::multisig_index(&terms.hash)),
            (Some(_), None) => None,
            (None, _) => bitcoin_key(ctx, &ctx.holders[holder], network).ok().map(|key| bech32::encode_segwit(network.hrp(), &hash160(&key))),
        };
        state.bitcoin.to = back.unwrap_or_default();
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

    // A multisig's spend, collecting its members' signatures.
    if state.shared.is_some() {
        shared_spend(ui, ctx, state, &htlc);

        return;
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

    // The starter's claim shows the secret, which takes their whole lock: never for less than
    // the terms say.
    let short = matches!(spend, Spend::Claim { .. }) && ctx.book.swaps[index].started && state.bitcoin.sats.trim().parse::<u64>().is_ok_and(|sats| sats < leg.sats);

    if short {
        ui.colored_label(palette.bad, "Those coins are less than the terms say: claiming them would show the secret for less. Do not; take yours back after its deadline.");
    }

    if let Some(why) = &held {
        ui.colored_label(palette.warn, format!("The HTLC's coins are {why}. Claim once they are deep enough: claiming shows the secret."));
        ui.checkbox(&mut state.anyway, "I checked it myself: claim anyway");
    }

    if theme::primary(ui, &palette, "Sign it", !short && (held.is_none() || state.anyway)).clicked() {
        let account = &ctx.holders[holder];
        let signed = match (&ctx.book.swaps[index].multisig, &shared_wallet) {
            (Some(_), Some(wallet)) => for_members(terms, spend, &state.bitcoin, wallet),
            (Some(_), None) => Err("Asking the node about the multisig…".to_string()),
            (None, _) => sign(secret_key(ctx, account, network), account.origin(), terms, spend, &state.bitcoin),
        };

        match signed {
            Ok(Signed::Here(tx)) => {
                state.bitcoin.signed = Some(to_hex(&tx.encode())[2..].to_string());
                state.trouble = None;
            }
            Ok(Signed::Signer(awaiting)) => state.signer = Some(awaiting),
            Ok(Signed::Members(shared)) => state.shared = Some(shared),
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

            #[cfg(all(feature = "bitcoin-light", not(target_arch = "wasm32")))]
            if let Some(light) = ctx.lights.on_mut(network == Network::Test)
                && theme::quiet(ui, &palette, "Broadcast to Bitcoin's peers").clicked()
                && let Some(raw) = from_hex(&format!("0x{signed}"))
            {
                state.relayed = Some(light.broadcast(raw));
            }
        });

        // What came of a broadcast through the light client.
        #[cfg(all(feature = "bitcoin-light", not(target_arch = "wasm32")))]
        if let Some(id) = state.relayed
            && let Some(result) = ctx.lights.on(network == Network::Test).and_then(|light| light.broadcasted(id))
        {
            state.trouble = Some(match result {
                Ok(txid) => format!("Sent to Bitcoin's peers: {txid}"),
                Err(why) => format!("Not sent: {why}"),
            });
            state.relayed = None;
        }
    }
}

/// Whether a leg's lock is final enough to act on: chainv4's by its finality, as the light client
/// proves it; an EVM chain's by its finalized block; Bitcoin's by its confirmations, as the light
/// client sees them, against the depth Settings asks.
fn settled_of(ctx: &Ctx, state: &SwapsState, terms: &Terms, side: Side, lock: Option<&Result<Lock, String>>, paid: Option<&[Paid]>) -> Settled {
    match (terms.leg(side), lock) {
        (Leg::Bitcoin(leg), _) => match paid {
            Some(paid) => match paid.iter().filter(|coin| coin.sats >= leg.sats).map(|coin| coin.depth).max() {
                Some(depth) => bitcoin_settled(depth, ctx.settings.bitcoin_depth.max(1)),
                None => Settled::Waiting("nothing paid to it yet".to_string()),
            },
            None => Settled::Unknown("follow Bitcoin under Settings, Bitcoin, or check its confirmations yourself".to_string()),
        },
        (Leg::Chain(_), Some(Ok(lock))) => match ctx.chain.finality() {
            Some((tip, finalized)) => chain_settled(lock, tip, finalized),
            None => Settled::Unknown("the light client is not following the chain yet".to_string()),
        },
        (Leg::Evm(leg), Some(Ok(lock))) => match state.evm_heads.get(&leg.chain_id) {
            Some((tip, finalized)) => evm_settled(lock, *tip, *finalized),
            None => Settled::Unknown(format!("asking {}", chain_name(leg.chain_id))),
        },
        (_, Some(Err(why))) => Settled::Unknown(why.clone()),
        (_, None) => Settled::Unknown("not known yet".to_string()),
    }
}

/// A coin paying a swap's HTLC, as the light client saw it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Paid {
    /// txid:vout, as explorers write it.
    outpoint: String,
    sats: u64,
    depth: u32,
    spent: bool,
}

/// The coins paying a swap's HTLC, when a light client follows its Bitcoin network: none for a
/// swap with no Bitcoin leg, or a network not followed.
#[cfg(all(feature = "bitcoin-light", not(target_arch = "wasm32")))]
fn htlc_seen(ctx: &Ctx, terms: &Terms) -> Option<Vec<Paid>> {
    let (htlc, (_, leg)) = (terms.htlc()?, terms.bitcoin()?);
    let light = ctx.lights.on(leg.testnet)?;

    Some(
        light
            .coins(&htlc.output())
            .into_iter()
            .map(|coin| Paid { outpoint: format!("{}:{}", coin.txid, coin.vout), sats: coin.value, depth: light.confirmations(coin.height), spent: coin.spent.is_some() })
            .collect(),
    )
}

#[cfg(not(all(feature = "bitcoin-light", not(target_arch = "wasm32"))))]
fn htlc_seen(_: &Ctx, _: &Terms) -> Option<Vec<Paid>> {
    None
}

/// The secret a claim of the HTLC showed, as the light client saw it.
#[cfg(all(feature = "bitcoin-light", not(target_arch = "wasm32")))]
fn htlc_secret(ctx: &Ctx, terms: &Terms) -> Option<[u8; 32]> {
    let (htlc, (_, leg)) = (terms.htlc()?, terms.bitcoin()?);

    ctx.lights.on(leg.testnet)?.secret(&htlc.output(), &terms.hash)
}

#[cfg(not(all(feature = "bitcoin-light", not(target_arch = "wasm32"))))]
fn htlc_secret(_: &Ctx, _: &Terms) -> Option<[u8; 32]> {
    None
}

enum Signed {
    Here(Transaction),
    Signer(Box<Awaiting>),
    Members(Box<Shared>),
}

/// The coins, value, fee rate and destination a spend's form names.
fn spend_form(form: &BitcoinForm, network: Network) -> Result<(Coins, u64, Vec<u8>), String> {
    let (txid, vout) = form.funding.trim().split_once(':').ok_or("its coins are txid:vout")?;
    let mut txid: [u8; 32] = from_hex(&format!("0x{txid}")).and_then(|bytes| bytes.try_into().ok()).ok_or("a txid is 64 hex digits")?;
    // Explorers write a txid backwards from how a transaction names it.
    txid.reverse();
    let vout: u32 = vout.parse().map_err(|_| "a vout is a number")?;
    let value: u64 = form.sats.trim().parse().map_err(|_| "what the coins hold, in satoshis")?;
    let fee_rate: u64 = form.fee_rate.trim().parse().map_err(|_| "a fee rate in sat/vB")?;
    let to = chainv4::crypto::bitcoin::tx::script_of(form.to.trim(), network)?;

    Ok((Coins { txid, vout, value }, fee_rate, to))
}

/// A multisig's spend of the HTLC, as a PSBT its members sign: its keys at the swap's index.
fn for_members(terms: &Terms, spend: Spend, form: &BitcoinForm, wallet: &chainv4::crypto::bitcoin::psbt::MultisigWallet) -> Result<Signed, String> {
    let htlc = terms.htlc().ok_or("no HTLC yet")?;
    let index = chainv4::swap::multisig_index(&terms.hash);

    if chainv4::swap::BitcoinParty::of_multisig(wallet, index).as_ref() != Some(htlc.party(spend)) {
        return Err("that branch is not the multisig's: its keys at this swap's index are not in it".to_string());
    }

    let (coins, fee_rate, to) = spend_form(form, wallet.network)?;
    let tx = htlc.unsigned(coins, to, fee_rate, spend)?;
    let psbt = chainv4::crypto::bitcoin::psbt::Psbt::for_htlc_multisig(&tx, coins.value, &htlc, wallet, index)?;

    Ok(Signed::Members(Box::new(Shared { psbt, spend, value: coins.value, scan: Scan::default(), pasted: String::new() })))
}

/// A multisig's spend on its way round its members: signed with these words, on the offline
/// signer, or pasted from a member's wallet, and finished once the branch holds enough.
fn shared_spend(ui: &mut Ui, ctx: &mut Ctx, state: &mut SwapsState, htlc: &chainv4::swap::htlc::Htlc) {
    let palette = *ctx.palette;
    let Some(shared) = state.shared.as_mut() else {
        return;
    };
    let (needed, keys) = htlc.party(shared.spend).keys();
    let held = keys.iter().filter(|key| shared.psbt.inputs[0].iter().any(|pair| pair.key == [&[0x02][..], &key[..]].concat())).count();
    ui.label(RichText::new(format!("The multisig's members sign it: {held} of the {needed} it takes, so far.")).color(palette.soft));
    super::code(ui, &palette, &Message::Bitcoin(Box::new(Request { psbt: shared.psbt.clone(), kind: crate::bitcoin::PSBT_TYPE })));

    match shared.scan.show(ui, &palette) {
        Some(Message::Bitcoin(signed)) => {
            if let Err(why) = shared.psbt.combine(&signed.psbt) {
                shared.scan.fail(why);
            }
        }
        Some(other) => shared.scan.refuse(&other, "a member's signed PSBT"),
        None => {}
    }

    ui.add(egui::TextEdit::multiline(&mut shared.pasted).hint_text("Paste a member's signed PSBT, in base64").font(egui::TextStyle::Monospace).desired_rows(2).desired_width(f32::INFINITY));

    if !shared.pasted.trim().is_empty() {
        match chainv4::crypto::bitcoin::psbt::Psbt::from_base64(&shared.pasted).and_then(|theirs| shared.psbt.combine(&theirs)) {
            Ok(()) => shared.pasted.clear(),
            Err(why) => {
                ui.colored_label(palette.bad, why);
            }
        }
    }

    let mut finished = None;
    ui.horizontal_wrapped(|ui| {
        if let Some(seed) = crate::keys::seed(ctx.book)
            && theme::primary(ui, &palette, "Sign with these words", held < needed as usize).clicked()
        {
            match crate::bitcoin::Signs::sign(&shared.psbt, &seed) {
                Ok(signed) => shared.psbt = signed,
                Err(why) => state.trouble = Some(why),
            }
        }

        if theme::quiet(ui, &palette, "Copy as base64").clicked() {
            ui.ctx().copy_text(shared.psbt.to_base64());
        }

        if held >= needed as usize && theme::primary(ui, &palette, "Finish", true).clicked() {
            finished = Some(shared.psbt.finish_htlc(htlc, shared.value, shared.spend));
        }

        if theme::quiet(ui, &palette, "Put away").clicked() {
            finished = Some(Err("put away".to_string()));
        }
    });

    match finished {
        Some(Ok(tx)) => {
            state.bitcoin.signed = Some(to_hex(&tx.encode())[2..].to_string());
            state.shared = None;
        }
        Some(Err(why)) if why == "put away" => state.shared = None,
        Some(Err(why)) => state.trouble = Some(why),
        None => {}
    }
}

/// Signs a spend: with the account's key, when this wallet holds it, or else as a PSBT for the
/// offline signer, which the pairing said where it derives the key.
fn sign(secret: Option<[u8; 32]>, origin: Option<crate::airgap::Origin>, terms: &Terms, spend: Spend, form: &BitcoinForm) -> Result<Signed, String> {
    let (htlc, network) = match (terms.htlc(), terms.bitcoin()) {
        (Some(htlc), Some((_, leg))) => (htlc, leg.network()),
        _ => return Err("no HTLC yet".to_string()),
    };
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
    let chainv4::swap::BitcoinParty::Key(key) = *htlc.party(spend) else {
        return Err("that branch is a multisig's: its members sign it".to_string());
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
    let (Some(htlc), Some((_, leg))) = (terms.htlc(), terms.bitcoin()) else {
        return;
    };
    let network = leg.network();
    let (looked, wake, hash, wanted) = (state.looked.clone(), ctx.wake.clone(), to_hex(&terms.hash), terms.hash);
    let url = format!("{}/address/{}/txs", ctx.settings.esplora.trim().trim_end_matches('/'), htlc.address(network));

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
                if let Some(record) = ctx.book.swaps.iter_mut().find(|record| hash_of(record).as_deref() == Some(hash.as_str())) {
                    record.funding = Some(format!("{outpoint}:{sats}"));
                    *ctx.changed = true;
                }
                (state.bitcoin.funding, state.bitcoin.sats) = (outpoint, sats.to_string());
            }
            Looked::Secret { hash, secret } => {
                if let Some(record) = ctx.book.swaps.iter_mut().find(|record| hash_of(record).as_deref() == Some(hash.as_str())) {
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
