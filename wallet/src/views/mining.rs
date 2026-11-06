//! What a pool owes one of the account's addresses, how it stands in the pool's window, and claiming
//! what it earned; and, in a build with the `mining` feature, mining from this device, on the Mine
//! card at the top (`mine`). In a build without, as the app stores' are, the page is called Pool
//! earnings, and nothing on it mines: claiming is not mining.
//!
//! A pool credits what it owes once its blocks can no longer be undone, and its escrow pays by a
//! claim alone: Tx::Claim, with the Merkle path from the address's leaf to a root the pool's block
//! committed to, which the pool's node serves at GET /pool/claim/{address}.
//!
//! What the pool's node says is shown as its word. A claim rests on nothing it says but the fee:
//! the chain checks the path against the root on chain and pays what the leaf says less what was
//! claimed before, so a path that does not hold fails and moves nothing. The claim is signed and
//! sent on the Send page, as any other transaction, through the wallet's own node.
//!
//! Each claim the pool's node gives is kept on this device, as SavedClaims keeps them: the newest
//! few, the next to settle, and the newest settled. Should the pool's node be gone, the page
//! checks the kept ones against the wallet's own node, GET /claims/{pool}/{address}, newest
//! first, and claims by the first whose root is still on chain.

use {
    super::send::{Prepared, SendState},
    super::{Ctx, Page},
    crate::format,
    crate::node::{Ask, Failure, Node},
    crate::theme,
    chainv4::registry::pools::{PoolClaim, PoolRoot, SavedClaims},
    chainv4::structs::address::Address,
    chainv4::structs::hash::{Hash, HashType},
    chainv4::structs::hex::{from_hex, to_hex},
    std::collections::HashMap,
    chainv4::structs::tx::MAX_CLAIM_PATH,
    chainv4::structs::u256::u256,
    egui::{RichText, Ui},
    serde_json::Value,
};
#[cfg(feature = "mining")]
use {super::mine, crate::app::Settings};

/// How often the pool is asked again, in milliseconds: what it owes moves at most once a block.
const REFRESH: u64 = 15_000;

/// What the API answers for a path it has nothing at.
const NOT_FOUND: u16 = 404;

#[derive(Default)]
pub struct MiningState {
    /// The address whose earnings show: one of the account's, the first until another is picked.
    from: Option<Address>,
    /// The pool's node as typed, before it is used.
    typed: Option<String>,
    trouble: Option<String>,
    /// The pool's own node, for a pool on another node than the wallet's: the URL and the proxy
    /// it was made for, and the connection.
    remote: Option<(String, String, Node)>,
    /// The pool's answers, each with the address it was asked about.
    claim: Option<(String, Result<Value, Failure>)>,
    standing: Option<(String, Result<Value, Failure>)>,
    /// The address last asked about, and when.
    asked: Option<(String, u64)>,
    /// Whether the pool's claim came since it was last kept.
    fresh: bool,
    /// The claims kept on this device, with the genesis of the chain they are on: read from the
    /// settings once, written back as they change.
    kept: Option<(String, SavedClaims)>,
    /// What the wallet's node said of kept claims, by pool, miner and height: None while asked.
    checks: HashMap<(String, String, u64), Option<Result<Value, Failure>>>,
    /// The miner, and what it is mined by.
    #[cfg(feature = "mining")]
    pub rig: mine::Rig,
}

impl MiningState {
    /// The claim the page shows, for the light client to prove the two entries it rests on: its
    /// pool, its miner and the height of its root.
    pub fn wanted_claims(&self) -> Vec<(String, String, u64)> {
        let Some((_, Ok(value))) = &self.claim else {
            return Vec::new();
        };

        match (value["pool"].as_str(), value["to"].as_str(), value["height"].as_u64()) {
            (Some(pool), Some(to), Some(height)) => vec![(pool.to_string(), to.to_string(), height)],
            _ => Vec::new(),
        }
    }

    /// The wallet's node's answer to something this page asked: it counts only while the pool is
    /// asked there, not at a node of its own.
    pub fn answered(&mut self, ask: &Ask, result: &Result<Value, Failure>) {
        // A kept claim is always checked at the wallet's own node, the pool's no matter.
        if let Ask::Claims { pool, to, height } = ask {
            self.checks.insert((pool.clone(), to.clone(), *height), Some(result.clone()));

            return;
        }

        if self.remote.is_none() {
            self.take(ask, result);
        }
    }

    fn take(&mut self, ask: &Ask, result: &Result<Value, Failure>) {
        match ask {
            Ask::PoolClaim(address) => {
                self.claim = Some((address.clone(), result.clone()));
                self.fresh = true;
            }
            Ask::Pool(address) => self.standing = Some((address.clone(), result.clone())),
            #[cfg(feature = "mining")]
            Ask::Mining => self.rig.port = Some(result.clone()),
            _ => {}
        }
    }

    /// Talks to the pool's node set, made again whenever it or the proxy changes; to the wallet's
    /// own when none is. What one node said goes once another is asked.
    fn connect(&mut self, ctx: &Ctx) {
        let url = ctx.settings.pool_node.trim().trim_end_matches('/');
        let proxy = ctx.settings.proxy.as_str();
        let current = self.remote.as_ref().map(|(url, proxy, _)| (url.as_str(), proxy.as_str()));
        let wanted = (!url.is_empty()).then_some((url, proxy));

        if current == wanted {
            return;
        }

        self.remote = wanted.map(|(url, proxy)| (url.to_string(), proxy.to_string(), Node::new(url, crate::app::client(ctx.settings), ctx.wake.clone())));
        self.claim = None;
        self.standing = None;
        self.asked = None;

        #[cfg(feature = "mining")]
        {
            self.rig.port = None;
            self.rig.port_asked = None;
        }
    }

    /// Asks again for all the page shows, at whichever node it asks, for a reload.
    pub fn reload(&mut self) {
        self.asked = None;
        self.checks.clear();

        #[cfg(feature = "mining")]
        {
            self.rig.port_asked = None;
        }
    }

    /// Keeps the miner as it should be, each frame the wallet is open, on any page: paying the
    /// address picked, the account's first until another is.
    #[cfg(feature = "mining")]
    pub fn keep_mining(&mut self, ctx: &egui::Context, settings: &Settings, addresses: &[Address], now: u64) {
        if !self.from.is_some_and(|from| addresses.contains(&from)) {
            self.from = addresses.first().copied();
        }

        self.rig.keep(ctx, settings, self.from, now);
    }
}

pub fn show(ui: &mut Ui, ctx: &mut Ctx, state: &mut MiningState, send: &mut SendState) {
    let palette = *ctx.palette;
    let lead = match cfg!(feature = "mining") {
        true => "Mining from this device, alone or in a pool, and claiming what a pool owes the account's addresses.",
        false => "What a pool owes the account's addresses, and claiming it.",
    };
    theme::title(ui, &palette, super::MINING_NAME, lead);

    let addresses = ctx.holder().map(|holder| holder.addresses()).unwrap_or_default();

    if !state.from.is_some_and(|from| addresses.contains(&from)) {
        state.from = addresses.first().copied();
    }

    let Some(from) = state.from else {
        return;
    };

    if addresses.len() > 1 {
        theme::card(ui, &palette, |ui| {
            theme::caps(ui, &palette, "Address");
            egui::ComboBox::from_id_salt("mining-from")
                .selected_text(super::from::label(ctx, &from))
                .width(ui.available_width())
                .show_ui(ui, |ui| {
                    for address in &addresses {
                        ui.selectable_value(&mut state.from, Some(*address), super::from::label(ctx, address));
                    }
                });
        });
    }

    state.connect(ctx);

    // A pool's node of its own answers here; the wallet's, through the app.
    for (ask, result) in state.remote.as_mut().map(|(_, _, node)| node.answers()).unwrap_or_default() {
        state.take(&ask, &result);
    }

    let address = from.to_string();

    #[cfg(feature = "mining")]
    let node = mine::mining_node(ctx.settings);

    #[cfg(feature = "mining")]
    {
        if std::mem::take(&mut state.rig.port_retry) {
            match &mut state.remote {
                Some((_, _, remote)) => remote.retry(Ask::Mining),
                None => ctx.node.retry(Ask::Mining),
            }
        }

        if state.rig.port_asked.as_ref().is_none_or(|(asked, at)| *asked != node || ctx.now.saturating_sub(*at) > ctx.settings.pace().every_ms(mine::PORT_REFRESH)) {
            match &mut state.remote {
                Some((_, _, remote)) => remote.ask(Ask::Mining),
                None => ctx.node.ask(Ask::Mining),
            }

            state.rig.port_asked = Some((node.clone(), ctx.now));
        }

        mine::card(ui, ctx, &mut state.rig, &from, &node);
    }

    if state.asked.as_ref().is_none_or(|(asked, at)| *asked != address || ctx.now.saturating_sub(*at) > ctx.settings.pace().every_ms(REFRESH)) {
        let node = match &mut state.remote {
            Some((_, _, node)) => node,
            None => &mut *ctx.node,
        };
        node.ask(Ask::PoolClaim(address.clone()));
        node.ask(Ask::Pool(address.clone()));
        state.asked = Some((address.clone(), ctx.now));
        // What was claimed moves too: kept claims are checked again as they show.
        state.checks.clear();
    }

    let claim = state.claim.as_ref().filter(|(asked, _)| *asked == address).map(|(_, result)| result.clone());
    let standing = state.standing.as_ref().filter(|(asked, _)| *asked == address).map(|(_, result)| result.clone());
    let mut prepared = None;

    keep(ctx, state, &from, claim.as_ref());
    let kept = state.kept.as_ref().map_or(0, |(_, saved)| saved.of(None, &from).count());

    if earnings(ui, ctx, &from, claim.as_ref(), standing.as_ref(), kept, &mut prepared) {
        let node = match &mut state.remote {
            Some((_, _, node)) => node,
            None => &mut *ctx.node,
        };
        node.retry(Ask::PoolClaim(address.clone()));
        node.retry(Ask::Pool(address.clone()));
    }

    if !matches!(claim, None | Some(Ok(_))) {
        kept_claims(ui, ctx, state, &from, &mut prepared);
    }

    if let Some(Ok(value)) = &standing {
        in_the_pool(ui, ctx, value, claim.as_ref().and_then(|claim| claim.as_ref().ok()));
    }

    pool_node(ui, ctx, state);

    #[cfg(feature = "mining")]
    {
        let port = state.rig.port.as_ref().and_then(|port| port.as_ref().ok()).and_then(|mining| mine::worker_node(mining, &node));
        mine::how_to_mine(ui, ctx, &from, port.as_deref());
    }

    if let Some(prepared) = prepared {
        send.prepare(prepared);
        *ctx.page = Page::Send;
    }
}

/// What the pool owes the address: all it earned, what it claimed, and what a claim pays now.
/// Whether Retry was pressed on what its node failed with.
fn earnings(ui: &mut Ui, ctx: &mut Ctx, from: &Address, claim: Option<&Result<Value, Failure>>, standing: Option<&Result<Value, Failure>>, kept: usize, prepared: &mut Option<Prepared>) -> bool {
    let palette = *ctx.palette;
    let note = |ui: &mut Ui, text: &str| {
        ui.label(RichText::new(text).small().color(palette.muted));
    };
    let mut retry = false;

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Earnings");

        // A node with no pool says so at both: said once, with where to look instead.
        if let Some(Err(failure)) = standing
            && failure.status == NOT_FOUND
        {
            ui.label(RichText::new("This node runs no pool.").color(palette.ink));
            note(ui, "If your pool runs on another node, set its node below.");

            return;
        }

        let value = match claim {
            None => {
                super::waiting(ui, &palette, "Asking the pool…");

                return;
            }
            Some(Err(failure)) if failure.status == NOT_FOUND => {
                ui.label(RichText::new("The pool owes this address nothing yet.").color(palette.ink));
                note(ui, "The pool credits what it owes once its blocks are final, so earnings show a while after the blocks are found.");

                return;
            }
            // The wallet's own node in trouble: said at the top of every page, with Retry.
            Some(Err(failure)) if failure.is_trouble() && ctx.settings.pool_node.trim().is_empty() => {
                super::waiting(ui, &palette, "Waiting for the node to answer…");

                return;
            }
            Some(Err(failure)) => {
                retry = theme::retry(ui, &palette, &format!("The pool's node did not answer. {}", failure.sentence()), false);

                return;
            }
            Some(Ok(value)) => value,
        };
        let number = |name: &str| value[name].as_str().unwrap_or("0").to_string();
        let hash = ctx.chain.client.as_ref().map(|client| client.params().hash);
        let proven = hash.and_then(|hash| claim_of(value, from, hash).ok()).map(|(claim, _)| proven_claim(ctx.chain, &claim, hash.expect("there is a client")));

        // Whether the pool's node said other figures than the chain holds.
        let differs = matches!(&proven, Some(ProvenClaim::Pays { claimed, pays, .. }) if number("claimed") != claimed.to_string() || number("claimable") != pays.to_string());

        // What the chain says, once proven: the figures, in place of the node's.
        let (earned, claimed, claimable) = match &proven {
            Some(ProvenClaim::Pays { earned, claimed, pays, .. }) => (earned.to_string(), claimed.to_string(), pays.to_string()),
            _ => (number("earned"), number("claimed"), number("claimable")),
        };

        super::row(ui, &palette, "Earned", |ui| {
            ui.label(RichText::new(ctx.amount(None, &earned)).color(palette.ink));
        });
        super::row(ui, &palette, "Claimed", |ui| {
            ui.label(ctx.amount(None, &claimed));
        });
        super::row(ui, &palette, "Claimable", |ui| {
            ui.label(RichText::new(ctx.amount(None, &claimable)).color(palette.good).size(16.0));
        });
        super::row(ui, &palette, "Pool", |ui| super::address(ui, &palette, value["pool"].as_str().unwrap_or("?")));
        let block = value["height"].as_u64().map_or_else(|| "?".to_string(), |height| format::thousands(&height.to_string()));

        match &proven {
            Some(ProvenClaim::Pays { at, final_, .. }) => {
                ui.label(RichText::new(format!(
                    "Proven at block {}{}: the claim's path leads to the root the pool's block {block} committed to, and what was claimed is the chain's. Only which of the pool's claims is its newest is its word.",
                    format::thousands(&at.to_string()),
                    if *final_ { ", final" } else { "" }
                )).small().color(palette.good));

                if differs {
                    ui.colored_label(palette.warn, "The pool's node said other figures than the chain holds: these are the chain's.");
                }
            }
            Some(ProvenClaim::Refused(why)) => {
                ui.colored_label(palette.bad, format!("The pool's claim does not hold on chain: {why}. It would pay nothing; trust none of these figures."));
            }
            Some(ProvenClaim::Waiting) | None => note(
                ui,
                &format!("As the pool's node says, from the claims its block {block} committed to, until the light client proves them. Earned is all the pool has credited this address; a claim pays it less what was claimed before, from the pool's escrow."),
            ),
        }
        ui.add_space(8.0);

        let Some(hash) = ctx.chain.client.as_ref().map(|client| client.params().hash) else {
            super::waiting(ui, &palette, "Waiting for the light client to meet the chain…");

            return;
        };

        match claim_of(value, from, hash) {
            Ok((claim, claimable)) => {
                if theme::primary(ui, &palette, "Claim", claimable > u256::ZERO).clicked() {
                    *prepared = Some(Prepared {
                        from: *from,
                        tx: claim.tx(),
                        what: "Claim pool earnings".to_string(),
                        about: format!("{} from the pool's escrow", ctx.amount(None, &claimable.to_string())),
                        coin: u256::ZERO,
                    });
                }

                if claimable == u256::ZERO {
                    note(ui, "Nothing to claim: all it credited is claimed. The pool may send claims itself.");
                }

                if kept > 0 {
                    note(ui, &format!("This device keeps {kept} of the pool's claims for this address, so it can claim from any node should the pool's be gone."));
                }
            }
            Err(why) => {
                ui.colored_label(palette.bad, format!("The pool's claim does not read: {why}"));
            }
        }
    });

    retry
}

/// How the address stands in the pool's window: its part of the next block, and in a build that
/// mines, its shares and the workers mining for it.
fn in_the_pool(ui: &mut Ui, ctx: &mut Ctx, value: &Value, claim: Option<&Value>) {
    let palette = *ctx.palette;
    let miner = value["miners"].as_array().and_then(|miners| miners.first());

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "In the pool");

        match miner {
            Some(miner) => {
                super::row(ui, &palette, "Next block", |ui| {
                    ui.label(RichText::new(format!("{:.2}%", miner["percent"].as_f64().unwrap_or(0.0))).color(palette.ink));
                });

                #[cfg(feature = "mining")]
                super::row(ui, &palette, "Shares", |ui| {
                    ui.label(format!("{} in the last {} blocks' work", format::thousands(&miner["shares"].as_u64().unwrap_or(0).to_string()), value["window"].as_u64().unwrap_or(0)));
                });

                // Credited lately, and not in a root on chain yet: the pool's next block commits
                // to it, and it is claimable from then on.
                let credited = miner["earned"].as_str().and_then(|text| u256::from_str_radix(text, 10).ok());
                let committed = claim.and_then(|claim| claim["earned"].as_str()).and_then(|text| u256::from_str_radix(text, 10).ok()).unwrap_or(u256::ZERO);

                if let Some(waiting) = credited.and_then(|credited| credited.checked_sub(committed)).filter(|waiting| *waiting > u256::ZERO) {
                    super::row(ui, &palette, "Credited", |ui| {
                        ui.label(format!("{} more, claimable once the pool's next block commits to it", ctx.amount(None, &waiting.to_string())));
                    });
                }
            }
            None => {
                ui.label(RichText::new("Nothing from this address in the pool's window: its next block gives it nothing.").color(palette.muted));
            }
        }

        super::row(ui, &palette, "Pool's fee", |ui| {
            ui.label(format!("{:.2}% of each block", value["fee_percent"].as_f64().unwrap_or(0.0)));
        });

        #[cfg(feature = "mining")]
        workers(ui, ctx, value);
    });
}

/// The workers mining for the address, as the pool says.
#[cfg(feature = "mining")]
fn workers(ui: &mut Ui, ctx: &Ctx, value: &Value) {
    let palette = *ctx.palette;
    let workers = value["workers"].as_array().cloned().unwrap_or_default();

    if workers.is_empty() {
        ui.label(RichText::new("No worker mining for this address is connected to the pool.").small().color(palette.muted));
    }

    for worker in &workers {
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(worker["name"].as_str().unwrap_or("worker")).color(palette.ink));
            ui.label(RichText::new(format!("{} · {} shares", format::hashrate(worker["rate"].as_f64().unwrap_or(0.0)), worker["shares"].as_u64().unwrap_or(0))).small().color(palette.soft));
        });
    }
}

/// Where the pool runs, for one on another node than the wallet's.
fn pool_node(ui: &mut Ui, ctx: &mut Ctx, state: &mut MiningState) {
    let palette = *ctx.palette;
    let set = ctx.settings.pool_node.clone();
    let typed = state.typed.get_or_insert_with(|| set.clone());

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Pool's node");
        ui.label(
            RichText::new("The HTTP API of the node that runs your pool, such as http://203.0.113.7:8080, for a pool on another node than the wallet's. Empty asks the wallet's own node. The claim is still sent through the wallet's node, so the pool must be on the same chain.")
                .small()
                .color(palette.muted),
        );
        let other = typed.trim().trim_end_matches('/') != set;
        let used = super::beside(
            ui,
            |ui| theme::primary(ui, &palette, "Use", other).clicked(),
            |ui, width| {
                ui.add(egui::TextEdit::singleline(typed).hint_text("empty: the wallet's node").font(egui::TextStyle::Monospace).desired_width(width));
            },
        );

        if used {
            let url = typed.trim().trim_end_matches('/').to_string();

            match url.is_empty() || url.starts_with("http://") || url.starts_with("https://") {
                true => {
                    ctx.settings.pool_node = url.clone();
                    *typed = url;
                    state.trouble = None;
                }
                false => state.trouble = Some("A node's API starts with http:// or https://.".to_string()),
            }
        }

        if let Some(why) = &state.trouble {
            ui.colored_label(palette.bad, why.as_str());
        }
    });
}

/// The claim a pool's answer describes, to send or keep, with what it would pay now: none for an
/// answer about another address than the one asked about.
/// What a pool's claim pays by the chain: the root at its height and what its miner claimed, both
/// proven at the light client's tip.
enum ProvenClaim {
    Waiting,
    Pays { earned: u256, claimed: u256, pays: u256, at: u64, final_: bool },
    Refused(String),
}

fn proven_claim(chain: &crate::chain::Chain, claim: &PoolClaim, hash: HashType) -> ProvenClaim {
    let (Some(root), Some(claimed)) = (chain.pool_roots.get(&claim.height), chain.pool_claimed.get(&(claim.pool.to_string(), claim.to.to_string()))) else {
        return ProvenClaim::Waiting;
    };

    match claim.pays(root.value.as_ref(), claimed.value, hash) {
        Ok((earned, claimed_total, pays)) => ProvenClaim::Pays {
            earned,
            claimed: claimed_total,
            pays,
            at: root.assurance.height.min(claimed.assurance.height),
            final_: root.assurance.final_ && claimed.assurance.final_,
        },
        Err(why) => ProvenClaim::Refused(why.to_string()),
    }
}

fn claim_of(value: &Value, miner: &Address, hash: HashType) -> Result<(PoolClaim, u256), String> {
    let to: Address = value["to"].as_str().and_then(|text| text.parse().ok()).ok_or("it names no miner")?;

    if to != *miner {
        return Err(format!("it is for {}, not this address", format::short(&to.to_string())));
    }

    let number = |name: &str| value[name].as_str().and_then(|text| u256::from_str_radix(text, 10).ok()).ok_or(format!("it says no {name}"));
    let whole = |name: &str| value[name].as_u64().ok_or(format!("it says no {name}"));
    let path = value["path"]
        .as_array()
        .ok_or("it has no path")?
        .iter()
        .map(|step| step.as_str().and_then(from_hex).and_then(|bytes| Hash::from_bytes(hash, &bytes)).ok_or("its path is not the chain's hashes".to_string()))
        .collect::<Result<Vec<Hash>, String>>()?;

    if path.len() > MAX_CLAIM_PATH {
        return Err(format!("its path is longer than the {MAX_CLAIM_PATH} hashes a claim may carry"));
    }

    let pool: Address = value["pool"].as_str().and_then(|text| text.parse().ok()).ok_or("it names no pool")?;
    let block = value["block"].as_str().and_then(from_hex).and_then(|bytes| Hash::from_bytes(hash, &bytes)).ok_or("it names no block")?;
    let claim = PoolClaim { pool, height: whole("height")?, block, to, earned: number("earned")?, index: whole("index")?, count: whole("count")?, path };

    Ok((claim, number("claimable")?))
}

/// Claims kept, from the hex the settings keep them as.
fn read_kept(hex: &str) -> Option<SavedClaims> {
    from_hex(hex).and_then(|bytes| wincode::deserialize(&bytes).ok())
}

/// Keeps the pool's newest claim, once, as it comes: in the settings, under the chain's genesis.
fn keep(ctx: &mut Ctx, state: &mut MiningState, from: &Address, claim: Option<&Result<Value, Failure>>) {
    let (Some(genesis), Some(hash)) = (ctx.chain.genesis.map(|genesis| genesis.to_string()), ctx.chain.client.as_ref().map(|client| client.params().hash)) else {
        return;
    };

    if state.kept.as_ref().is_none_or(|(kept, _)| *kept != genesis) {
        let saved = ctx.settings.pool_claims.get(&genesis).and_then(|hex| read_kept(hex)).unwrap_or_default();
        state.kept = Some((genesis.clone(), saved));
        state.fresh = true;
    }

    let Some(Ok(value)) = claim.filter(|_| std::mem::take(&mut state.fresh)) else {
        return;
    };
    let Some((_, saved)) = state.kept.as_mut() else {
        return;
    };

    if let Ok((claim, _)) = claim_of(value, from, hash)
        && saved.keep(claim, value["settled"].as_u64())
        && let Ok(bytes) = wincode::serialize(saved)
    {
        ctx.settings.pool_claims.insert(genesis, to_hex(&bytes));
    }
}

/// With the pool's node not answering: the newest kept claim that still holds on the wallet's
/// node's chain, checked there one at a time, and claiming by it.
fn kept_claims(ui: &mut Ui, ctx: &mut Ctx, state: &mut MiningState, from: &Address, prepared: &mut Option<Prepared>) {
    let palette = *ctx.palette;
    let mut claims: Vec<PoolClaim> = state.kept.as_ref().map(|(_, saved)| saved.of(None, from).cloned().collect()).unwrap_or_default();
    claims.sort_by_key(|claim| std::cmp::Reverse(claim.height));

    if claims.is_empty() {
        return;
    }

    let Some(hash) = ctx.chain.client.as_ref().map(|client| client.params().hash) else {
        return;
    };

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Kept claims");
        ui.label(
            RichText::new(format!("The pool's node did not give a claim, but this device keeps {} for this address. A kept claim is checked against the chain at the wallet's own node and pays from the pool's escrow as any other, the pool's node no matter.", claims.len()))
                .small()
                .color(palette.muted),
        );
        ui.add_space(4.0);

        for claim in &claims {
            let key = (claim.pool.to_string(), claim.to.to_string(), claim.height);
            let answer = match state.checks.get(&key) {
                None => {
                    ctx.node.ask(Ask::Claims { pool: key.0.clone(), to: key.1.clone(), height: claim.height });
                    state.checks.insert(key, None);
                    None
                }
                Some(answer) => answer.clone(),
            };
            let value = match answer {
                None => return super::waiting(ui, &palette, &format!("Checking the claim kept at block {} against the chain…", format::thousands(&claim.height.to_string()))),
                Some(Err(failure)) => {
                    if theme::retry(ui, &palette, &failure.sentence(), false) {
                        state.checks.clear();
                    }

                    return;
                }
                Some(Ok(value)) => value,
            };
            let root = value["root"]["root"].as_str().and_then(from_hex).and_then(|bytes| Hash::from_bytes(hash, &bytes));

            let (Some(root), Some(count)) = (root, value["root"]["count"].as_u64()) else {
                continue;
            };

            if !claim.holds(&PoolRoot { pool: claim.pool, root, count }, hash) {
                continue;
            }

            let claimed = value["claimed"].as_str().and_then(|text| u256::from_str_radix(text, 10).ok()).unwrap_or(u256::ZERO);
            let claimable = claim.earned.saturating_sub(claimed);

            super::row(ui, &palette, "Earned", |ui| {
                ui.label(RichText::new(ctx.amount(None, &claim.earned.to_string())).color(palette.ink));
            });
            super::row(ui, &palette, "Claimed", |ui| {
                ui.label(ctx.amount(None, &claimed.to_string()));
            });
            super::row(ui, &palette, "Claimable", |ui| {
                ui.label(RichText::new(ctx.amount(None, &claimable.to_string())).color(palette.good).size(16.0));
            });
            super::row(ui, &palette, "Pool", |ui| super::address(ui, &palette, &claim.pool.to_string()));
            ui.label(
                RichText::new(format!("By the claim kept at block {}, whose root is on chain. The pool may have credited more since, which only its node can give.", format::thousands(&claim.height.to_string())))
                    .small()
                    .color(palette.muted),
            );
            ui.add_space(8.0);

            if theme::primary(ui, &palette, "Claim", claimable > u256::ZERO).clicked() {
                *prepared = Some(Prepared {
                    from: *from,
                    tx: claim.tx(),
                    what: "Claim pool earnings".to_string(),
                    about: format!("{} from the pool's escrow, by a claim kept on this device", ctx.amount(None, &claimable.to_string())),
                    coin: u256::ZERO,
                });
            }

            return;
        }

        ui.label(RichText::new("None of the kept claims holds on this chain: the blocks they name were undone.").color(palette.muted));
    });
}

#[cfg(test)]
mod proven_tests {
    use {
        super::*,
        chainv4::crypto::merkle,
        chainv4::light::Assurance,
        chainv4::registry::pools::{PoolRoot, claim_leaf},
    };

    fn at(height: u64, final_: bool) -> Assurance {
        Assurance { block: Hash::zero(HashType::default_chain()), height, final_ }
    }

    #[test]
    fn a_claim_pays_by_the_root_and_the_claimed_total_proven_and_is_refused_by_a_root_it_is_not_under() {
        let hash = HashType::default_chain();
        let (pool, miner) = (Address::Ed25519([1; 32]), Address::Ed25519([2; 32]));
        let leaves = [claim_leaf(hash, &miner, u256::new(900)), claim_leaf(hash, &pool, u256::new(5))];
        let claim = PoolClaim { pool, height: 12, block: Hash::zero(hash), to: miner, earned: u256::new(900), index: 0, count: 2, path: merkle::audit_path(hash, &leaves, 0) };
        let mut chain = crate::chain::Chain::default();

        assert!(matches!(proven_claim(&chain, &claim, hash), ProvenClaim::Waiting), "nothing proven yet");

        chain.pool_roots.insert(12, crate::chain::Proven { value: Some(PoolRoot { pool, root: merkle::root(hash, &leaves), count: 2 }), assurance: at(20, true) });
        chain.pool_claimed.insert((pool.to_string(), miner.to_string()), crate::chain::Proven { value: u256::new(300), assurance: at(21, false) });

        match proven_claim(&chain, &claim, hash) {
            ProvenClaim::Pays { earned, claimed, pays, at, final_ } => assert_eq!((earned, claimed, pays, at, final_), (u256::new(900), u256::new(300), u256::new(600), 20, false)),
            _ => panic!("it pays"),
        }

        // The block undone for another, whose root the claim is not under: it pays nothing.
        chain.pool_roots.insert(12, crate::chain::Proven { value: Some(PoolRoot { pool, root: leaves[1], count: 2 }), assurance: at(22, true) });
        assert!(matches!(proven_claim(&chain, &claim, hash), ProvenClaim::Refused(why) if why.contains("path")));
        chain.pool_roots.insert(12, crate::chain::Proven { value: None, assurance: at(22, true) });
        assert!(matches!(proven_claim(&chain, &claim, hash), ProvenClaim::Refused(why) if why.contains("no mining pool's root")));
    }
}

#[cfg(test)]
mod tests {
    use {super::*, chainv4::structs::tx::Tx, serde_json::json};

    fn answer(to: &Address, path: Vec<String>) -> Value {
        json!({
            "pool": Address::Ed25519([1; 32]).to_string(),
            "escrow": Address::Contract([2; 32]).to_string(),
            "height": 120,
            "block": Hash::zero(HashType::default_chain()).to_string(),
            "to": to.to_string(),
            "earned": "5000000000000000000",
            "claimed": "2000000000000000000",
            "claimable": "3000000000000000000",
            "index": 3,
            "count": 7,
            "path": path,
        })
    }

    #[test]
    fn a_pools_answer_becomes_the_claim_it_describes() {
        let hash = HashType::default_chain();
        let miner = Address::Ed25519([3; 32]);
        let steps = [Hash::zero(hash), chainv4::crypto::hasher::Hasher::digest(hash, b"leaf")];
        let (claim, claimable) = claim_of(&answer(&miner, steps.iter().map(ToString::to_string).collect()), &miner, hash).expect("reads");

        assert_eq!(claim.tx(), Tx::Claim { height: 120, to: miner, earned: u256::new(5_000_000_000_000_000_000), index: 3, path: steps.to_vec() });
        assert_eq!((claim.pool, claim.count, claim.block), (Address::Ed25519([1; 32]), 7, Hash::zero(hash)));
        assert_eq!(claimable, u256::new(3_000_000_000_000_000_000));
    }

    #[test]
    fn a_kept_claim_reads_back_from_the_settings_hex() {
        let hash = HashType::default_chain();
        let miner = Address::Ed25519([3; 32]);
        let (claim, _) = claim_of(&answer(&miner, vec![Hash::zero(hash).to_string()]), &miner, hash).unwrap();
        let mut saved = SavedClaims::default();
        assert!(saved.keep(claim.clone(), Some(100)));

        let hex = to_hex(&wincode::serialize(&saved).unwrap());
        assert_eq!(read_kept(&hex).unwrap().newest(&claim.pool, &miner), Some(&claim));
        assert!(read_kept("0x12").is_none());
    }

    #[test]
    fn a_claim_for_another_address_or_with_a_path_that_does_not_read_is_refused() {
        let hash = HashType::default_chain();
        let (miner, other) = (Address::Ed25519([3; 32]), Address::Ed25519([4; 32]));

        assert!(claim_of(&answer(&other, Vec::new()), &miner, hash).is_err_and(|why| why.contains("not this address")));
        assert!(claim_of(&answer(&miner, vec!["0x12".to_string()]), &miner, hash).is_err_and(|why| why.contains("path")));
        assert!(claim_of(&answer(&miner, vec![Hash::zero(hash).to_string(); MAX_CLAIM_PATH + 1]), &miner, hash).is_err_and(|why| why.contains("longer")));
    }
}
