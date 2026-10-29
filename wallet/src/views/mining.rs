//! Mining from this device, and what a pool owes for it. The Mine card mines for the address
//! picked, alone or in a pool, on the node's mining port as GET /mining describes it: through
//! crate::miner, a worker on this device's GPU or CPU. A phone mines only while the wallet is in
//! front, and off its charger only when told to; locking the wallet stops it everywhere, as the
//! page's state goes with the keys.
//!
//! Below it, mining in a pool: what a pool owes one of the account's addresses, how it stands in
//! the pool's window, and claiming what it earned. A pool credits its miners once its blocks can no longer be
//! undone, and its escrow pays them only by a claim: Tx::Claim, with the Merkle path from the
//! miner's leaf to a root the pool's block committed to, which the pool's node serves at
//! GET /pool/claim/{address}.
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
    crate::app::Settings,
    crate::format,
    crate::miner::{self, Devices, MineConfig, Miner, Port},
    crate::node::{Ask, Failure, Node},
    crate::theme,
    chainv4::mining::status::Mode,
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

/// How often the pool is asked again, in milliseconds: what it owes moves at most once a block.
const REFRESH: u64 = 15_000;

/// What the API answers for a path it has nothing at.
const NOT_FOUND: u16 = 404;

/// How often GET /mining is asked again, in milliseconds: a node's port changes only as it restarts.
const PORT_REFRESH: u64 = 60_000;
// The token every worker of an open port uses: the node's own, in the protocol.
use chainv4::mining::protocol::OPEN_TOKEN;
/// The protocol a node speaks unless started with another, as chainv4::net::DEFAULT_PROTOCOL.
const DEFAULT_PROTOCOL: &str = "chainv4";
/// Whether this is a phone, which mines only in front and, unless told, on its charger.
const PHONE: bool = cfg!(any(target_os = "android", target_os = "ios"));
/// How long a phone may go without drawing the wallet before it is taken to have been in the
/// background, in milliseconds: the native miner stops itself after as long.
const UNSEEN: u64 = 5_000;
/// How often a phone's charger is looked at while mining, in milliseconds.
const POWER_EVERY: u64 = 3_000;

/// The Mine card's miner: mining, or paused while a phone is off its charger, to start again as it
/// was once it is on one.
enum Mine {
    Running(Miner),
    Paused(MineConfig),
}

impl Mine {
    fn config(&self) -> &MineConfig {
        match self {
            Mine::Running(miner) => miner.config(),
            Mine::Paused(config) => config,
        }
    }
}

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
    /// GET /mining's answer, and the node asked and when.
    port: Option<Result<Value, Failure>>,
    port_asked: Option<(String, u64)>,
    /// The miner, while the person has it mine.
    mine: Option<Mine>,
    /// Why mining stopped by itself, or did not start.
    mine_note: Option<String>,
    /// When the wallet last drew a frame, and last looked at the charger, in milliseconds.
    drawn: u64,
    powered: u64,
    /// The GPUs this device has, listed once, as listing them asks the graphics driver.
    gpus: Option<Vec<String>>,
}

impl MiningState {
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
            Ask::Mining => self.port = Some(result.clone()),
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
        self.port = None;
        self.port_asked = None;
    }

    /// Keeps the miner as it should be, each frame the wallet is open, on any page: paying the
    /// address picked, the account's first until another is, and restarted when that changes;
    /// stopped once a phone was in the background; paused while a phone is off its charger, unless
    /// the person lets it mine on battery, and started again once it is on one.
    pub fn keep_mining(&mut self, ctx: &egui::Context, settings: &Settings, addresses: &[Address], now: u64) {
        let since = now.saturating_sub(std::mem::replace(&mut self.drawn, now));

        if !self.from.is_some_and(|from| addresses.contains(&from)) {
            self.from = addresses.first().copied();
        }

        let Some(mine) = &self.mine else {
            return;
        };

        // Not drawn for a while: the wallet was in the background, and stays stopped until the
        // person starts it again.
        if PHONE && since > UNSEEN {
            self.mine = None;
            self.mine_note = Some("Stopped as the wallet went to the background: a phone mines only while the wallet is in front. Start it again to mine.".to_string());

            return;
        }

        let Some(payout) = self.from else {
            self.mine = None;

            return;
        };

        if mine.config().payout != payout {
            let config = MineConfig { payout, ..mine.config().clone() };
            // The old worker lets go of the devices before the new one takes them.
            self.mine = None;
            self.mine = self.begin(config, ctx, settings);
        }

        if PHONE && now.saturating_sub(self.powered) >= POWER_EVERY {
            self.powered = now;
            let pause = pauses(PHONE, settings.mine_on_battery, chainv4_ui::power::charging(ctx));

            self.mine = match self.mine.take() {
                Some(Mine::Running(miner)) if pause => Some(Mine::Paused(miner.config().clone())),
                Some(Mine::Paused(config)) if !pause => self.begin(config, ctx, settings),
                kept => kept,
            };
        } else if PHONE {
            // Asked each frame, so the phone keeps looking while it mines.
            let _ = chainv4_ui::power::charging(ctx);
        }
    }

    /// Mining as configured, or paused at once on a phone off its charger; None, with the note
    /// saying why, for a miner that does not start.
    fn begin(&mut self, config: MineConfig, ctx: &egui::Context, settings: &Settings) -> Option<Mine> {
        if pauses(PHONE, settings.mine_on_battery, chainv4_ui::power::charging(ctx)) {
            return Some(Mine::Paused(config));
        }

        match Miner::start(config, ctx) {
            Ok(miner) => {
                self.mine_note = None;

                Some(Mine::Running(miner))
            }
            Err(why) => {
                self.mine_note = Some(format!("Mining did not start: {why}."));

                None
            }
        }
    }
}

/// Whether mining pauses now: on a phone off its charger, unless it may mine on battery. A charger
/// the phone does not say anything of does not pause it.
fn pauses(phone: bool, on_battery: bool, charging: Option<bool>) -> bool {
    phone && !on_battery && charging == Some(false)
}

/// Where the mining port is reached: the address GET /mining says the node publishes it at, else
/// the node's own host with the port; None for an answer without a port or a URL without a host.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn port_address(mining: &Value, node: &str) -> Option<String> {
    if let Some(public) = mining["public"].as_str().map(str::trim).filter(|public| !public.is_empty()) {
        return Some(public.to_string());
    }

    let port = mining["port"].as_u64().filter(|port| (1..=u64::from(u16::MAX)).contains(port))?;
    let rest = node.trim().split_once("://").map_or(node.trim(), |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    let host = match authority.strip_prefix('[') {
        // An IPv6 address keeps its brackets, as host:port wants them.
        Some(inside) => format!("[{}]", inside.split_once(']')?.0),
        None => authority.split(':').next()?.to_string(),
    };

    (!host.is_empty() && host != "[]").then(|| format!("{host}:{port}"))
}

/// The WebSocket a browser mines over: the node's URL with http:// made ws:// and https:// made
/// wss://, and the path GET /mining names; None for a URL that is neither.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn socket_url(node: &str, path: &str) -> Option<String> {
    let node = node.trim().trim_end_matches('/');
    let path = if path.starts_with('/') { path.to_string() } else { format!("/{path}") };

    if let Some(rest) = node.strip_prefix("https://") {
        return Some(format!("wss://{rest}{path}"));
    }

    node.strip_prefix("http://").map(|rest| format!("ws://{rest}{path}"))
}

/// How long a block takes to find, on average, at a hashrate: as many hashes as the difficulty,
/// as the work hash meets it once in that many. None at no hashrate.
fn block_every(difficulty: u128, rate: f64) -> Option<String> {
    if rate.is_nan() || rate <= 0.0 {
        return None;
    }

    let seconds = difficulty as f64 / rate;
    const YEAR: f64 = 365.25 * 86_400.0;

    Some(match seconds {
        seconds if seconds >= 1_000.0 * YEAR => "more than a thousand years".to_string(),
        seconds if seconds >= YEAR => format!("about {:.1} years", seconds / YEAR),
        seconds => format!("about {}", format::span((seconds * 1_000.0) as u64)),
    })
}

/// The address the node's API is at, where GET /mining is asked: the pool's node when one is set,
/// else the wallet's.
fn mining_node(settings: &Settings) -> String {
    match settings.pool_node.trim().trim_end_matches('/') {
        "" => settings.node.clone(),
        pool => pool.to_string(),
    }
}

/// The proxy the worker goes through: the SOCKS5 one set, or None for none. An HTTP proxy does
/// not carry a mining port's protocol, and the wallet never goes around a proxy set, so with one
/// it does not mine.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn socks(proxy: &str) -> Result<Option<String>, String> {
    match proxy.trim() {
        "" => Ok(None),
        proxy if proxy.starts_with("socks5h://") || proxy.starts_with("socks5://") => Ok(Some(proxy.to_string())),
        _ => Err("the proxy set is an HTTP one, which does not carry a mining port: set a socks5h:// proxy, or none, in Settings".to_string()),
    }
}

/// How to mine, from the settings and GET /mining's answer: where, with which token, alone or in
/// the pool, and on what; why not, for a port that cannot be mined on as asked.
fn plan(settings: &Settings, mining: Option<&Value>, payout: Address, gpus: usize) -> Result<MineConfig, String> {
    let node = mining_node(settings);
    let typed = settings.mine_port.trim();

    #[cfg(not(target_arch = "wasm32"))]
    let port = {
        let address = match typed {
            "" => mining.and_then(|mining| port_address(mining, &node)).ok_or("the node said no mining port: give its host:port")?,
            typed => typed.to_string(),
        };

        Port::Tcp { address, proxy: socks(&settings.proxy)? }
    };
    #[cfg(target_arch = "wasm32")]
    let port = {
        let _ = typed;
        let path = mining.and_then(|mining| mining["ws"].as_str()).unwrap_or("/mining/ws");

        Port::WebSocket(socket_url(&node, path).ok_or("the node's URL is neither http:// nor https://")?)
    };

    // The node's word on its own port, which a port typed in place of it may not be.
    let said = mining.filter(|_| typed.is_empty() || cfg!(target_arch = "wasm32"));
    let open = said.is_some_and(|mining| mining["open"] == true);
    let token = match settings.mine_token.trim() {
        _ if open => OPEN_TOKEN.to_vec(),
        "" if said.is_some() => return Err("this port is private: give the token its operator gave you".to_string()),
        "" => OPEN_TOKEN.to_vec(),
        typed => typed.as_bytes().to_vec(),
    };

    if let Some(mining) = said {
        match (settings.mine_solo, mining["solo"] == true, mining["pool"] == true) {
            (true, false, _) => return Err("this port takes nobody mining alone".to_string()),
            (false, _, false) => return Err("this port runs no pool".to_string()),
            _ => {}
        }
    }

    let devices = match settings.mine_gpu {
        true if gpus == 0 => return Err("this device has no GPU to mine on".to_string()),
        true => Devices::Gpu(settings.mine_gpus.filter(|which| *which < gpus).map(|which| vec![which])),
        false => Devices::Cpu(settings.mine_threads.clamp(1, miner::cpus())),
    };

    Ok(MineConfig {
        port,
        token,
        protocol: said.and_then(|mining| mining["protocol"].as_str()).unwrap_or(DEFAULT_PROTOCOL).to_string(),
        name: format!("wallet-{}", if cfg!(target_arch = "wasm32") { "web" } else { std::env::consts::OS }),
        payout,
        solo: settings.mine_solo,
        devices,
    })
}

pub fn show(ui: &mut Ui, ctx: &mut Ctx, state: &mut MiningState, send: &mut SendState) {
    let palette = *ctx.palette;
    theme::title(ui, &palette, "Mining", "Mining from this device, alone or in a pool, and claiming what a pool owes the account's addresses.");

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
                .selected_text(format!("{} · {}", super::scheme(&from), format::short(&from.to_string())))
                .width(ui.available_width())
                .show_ui(ui, |ui| {
                    for address in &addresses {
                        ui.selectable_value(&mut state.from, Some(*address), format!("{} · {}", super::scheme(address), format::short(&address.to_string())));
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
    let node = mining_node(ctx.settings);

    if state.port_asked.as_ref().is_none_or(|(asked, at)| *asked != node || ctx.now.saturating_sub(*at) > PORT_REFRESH) {
        match &mut state.remote {
            Some((_, _, remote)) => remote.ask(Ask::Mining),
            None => ctx.node.ask(Ask::Mining),
        }

        state.port_asked = Some((node.clone(), ctx.now));
    }

    mine(ui, ctx, state, &from, &node);

    if state.asked.as_ref().is_none_or(|(asked, at)| *asked != address || ctx.now.saturating_sub(*at) > REFRESH) {
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
    earnings(ui, ctx, &from, claim.as_ref(), standing.as_ref(), kept, &mut prepared);

    if !matches!(claim, None | Some(Ok(_))) {
        kept_claims(ui, ctx, state, &from, &mut prepared);
    }

    if let Some(Ok(value)) = &standing {
        in_the_pool(ui, ctx, value, claim.as_ref().and_then(|claim| claim.as_ref().ok()));
    }

    pool_node(ui, ctx, state);
    let port = state.port.as_ref().and_then(|port| port.as_ref().ok()).and_then(|mining| port_address(mining, &node));
    how_to_mine(ui, ctx, &from, port.as_deref());

    if let Some(prepared) = prepared {
        send.prepare(prepared);
        *ctx.page = Page::Send;
    }
}

/// The Mine card: where the mining port is, alone or in its pool, on what, and starting and
/// stopping; while mining, how it is doing.
fn mine(ui: &mut Ui, ctx: &mut Ctx, state: &mut MiningState, from: &Address, node: &str) {
    let palette = *ctx.palette;
    let note = |ui: &mut Ui, text: &str| {
        ui.label(RichText::new(text).small().color(palette.muted));
    };
    let gpus = state.gpus.get_or_insert_with(miner::gpus).clone();
    let mining = state.port.as_ref().and_then(|port| port.as_ref().ok()).cloned();
    let running = state.mine.is_some();

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Mine");

        match &state.port {
            None => super::waiting(ui, &palette, "Asking the node where it takes miners…"),
            Some(Ok(mining)) => {
                #[cfg(not(target_arch = "wasm32"))]
                let at = port_address(mining, node);
                #[cfg(target_arch = "wasm32")]
                let at = socket_url(node, mining["ws"].as_str().unwrap_or("/mining/ws"));

                super::row(ui, &palette, "Port", |ui| {
                    ui.label(theme::mono(at.unwrap_or_else(|| "?".to_string())).color(palette.ink));
                    ui.label(RichText::new(if mining["open"] == true { "open to anyone" } else { "private: it wants a token" }).small().color(palette.soft));
                });
            }
            Some(Err(failure)) if failure.status == NOT_FOUND => {
                ui.label(RichText::new("This node takes no miners: it was not started with --mining.").color(palette.ink));
            }
            Some(Err(failure)) => {
                ui.colored_label(palette.bad, format!("The node did not say where it takes miners: {}", failure.message));
            }
        }

        ui.add_enabled_ui(!running, |ui| {
            // A browser reaches the port only through its node's WebSocket.
            #[cfg(not(target_arch = "wasm32"))]
            super::row(ui, &palette, "Other port", |ui| {
                ui.add(egui::TextEdit::singleline(&mut ctx.settings.mine_port).hint_text("host:port, empty for the node's").font(egui::TextStyle::Monospace).desired_width(260.0));
            });

            let said = mining.as_ref().filter(|_| ctx.settings.mine_port.trim().is_empty() || cfg!(target_arch = "wasm32"));

            if !said.is_some_and(|mining| mining["open"] == true) {
                super::row(ui, &palette, "Token", |ui| {
                    ui.add(egui::TextEdit::singleline(&mut ctx.settings.mine_token).password(true).hint_text("the port's token").desired_width(260.0));
                });
            }

            let (solo, pool) = said.map_or((true, true), |mining| (mining["solo"] == true, mining["pool"] == true));

            // What the port does not take is not chosen.
            if ctx.settings.mine_solo && !solo && pool {
                ctx.settings.mine_solo = false;
            } else if !ctx.settings.mine_solo && !pool && solo {
                ctx.settings.mine_solo = true;
            }

            super::row(ui, &palette, "Mine", |ui| {
                ui.add_enabled_ui(pool, |ui| ui.selectable_value(&mut ctx.settings.mine_solo, false, "In the pool"));
                ui.add_enabled_ui(solo, |ui| ui.selectable_value(&mut ctx.settings.mine_solo, true, "Alone"));
            });

            match (solo, pool, ctx.settings.mine_solo) {
                (false, false, _) => note(ui, "This port mines only for its node: the blocks found pay the node's own address, not yours."),
                (_, _, true) => note(ui, "Each block this device finds pays the address in full. Between blocks it earns nothing: on a busy chain a block may be a long way off."),
                (_, _, false) => {
                    let fee = said.and_then(|mining| mining["fee"].as_u64()).map_or_else(String::new, |fee| format!(", less its fee of {:.2}%", fee as f64 / 1e7));
                    note(ui, &format!("The pool pays the address its part of each block the pool finds, by the shares this device sends{fee}. Claim it below once it is credited."));
                }
            }

            super::row(ui, &palette, "On", |ui| {
                ui.selectable_value(&mut ctx.settings.mine_gpu, false, "CPU");
                ui.add_enabled_ui(!gpus.is_empty(), |ui| ui.selectable_value(&mut ctx.settings.mine_gpu, true, "GPU"))
                    .response
                    .on_disabled_hover_text("No GPU found on this device");
            });

            if ctx.settings.mine_gpu && !gpus.is_empty() {
                let chosen = ctx.settings.mine_gpus.and_then(|which| gpus.get(which)).map_or("Every GPU", String::as_str).to_string();

                super::row(ui, &palette, "GPU", |ui| {
                    egui::ComboBox::from_id_salt("mine-gpu").selected_text(chosen).show_ui(ui, |ui| {
                        ui.selectable_value(&mut ctx.settings.mine_gpus, None, "Every GPU");

                        for (which, name) in gpus.iter().enumerate() {
                            ui.selectable_value(&mut ctx.settings.mine_gpus, Some(which), name);
                        }
                    });
                });
            } else {
                let cpus = miner::cpus();
                ctx.settings.mine_threads = ctx.settings.mine_threads.clamp(1, cpus);

                super::row(ui, &palette, "Threads", |ui| {
                    ui.add(egui::Slider::new(&mut ctx.settings.mine_threads, 1..=cpus).text(format!("of {cpus}")));
                });
            }

            if PHONE {
                ui.checkbox(&mut ctx.settings.mine_on_battery, "Mine on battery");
                note(ui, "A phone mines only while the wallet is in front, and pauses off its charger unless this is ticked. Mining warms a phone and drains it fast.");
            }
        });

        ui.add_space(6.0);

        if running {
            if theme::primary(ui, &palette, "Stop", true).clicked() {
                state.mine = None;
                state.mine_note = None;
            }
        } else {
            let planned = plan(ctx.settings, mining.as_ref(), *from, gpus.len());

            if theme::primary(ui, &palette, "Start", planned.is_ok()).clicked()
                && let Ok(config) = planned.clone()
            {
                state.mine = state.begin(config, ctx.wake, ctx.settings);
            }

            if let Err(why) = planned {
                note(ui, &format!("Not yet: {why}."));
            }
        }

        if let Some(why) = &state.mine_note {
            ui.colored_label(palette.warn, why.as_str());
        }

        if let Some(mine) = &state.mine {
            ui.add_space(6.0);
            mining_now(ui, ctx, mine);
        }

        note(ui, "Payouts go to the address picked: another account, or another of its addresses, restarts mining for it. Locking the wallet stops mining.");
    });
}

/// How the miner is doing: connected or not and why, what for, its hashrate, the work it had and
/// what it found, and a pool's word on its standing, or how long a block takes alone.
fn mining_now(ui: &mut Ui, ctx: &Ctx, mine: &Mine) {
    let palette = *ctx.palette;
    let note = |ui: &mut Ui, text: &str| {
        ui.label(RichText::new(text).small().color(palette.muted));
    };

    let miner = match mine {
        Mine::Paused(_) => {
            super::row(ui, &palette, "Status", |ui| {
                ui.colored_label(palette.warn, "Paused: on battery");
            });
            note(ui, "It starts again by itself once the phone is on its charger, or once Mine on battery is ticked.");

            return;
        }
        Mine::Running(miner) => miner,
    };
    let status = miner.status();
    let devices = miner.devices();
    let rate: f64 = devices.iter().map(|device| device.rate).sum();
    let count = |number: u64| format::thousands(&number.to_string());

    super::row(ui, &palette, "Status", |ui| match (&status.connected, &status.trouble) {
        (true, _) => {
            ui.colored_label(palette.good, "Connected");
        }
        (false, Some(why)) => {
            ui.colored_label(palette.bad, format!("Not connected: {why}"));
        }
        (false, None) => super::waiting(ui, &palette, "Connecting…"),
    });

    if !status.connected && status.trouble.is_some() {
        note(ui, "It tries again by itself every few seconds.");
    }

    super::row(ui, &palette, "Mode", |ui| {
        ui.label(match status.mode.clone() {
            Some(Mode::Pool { fee, window }) => format!("In the pool: a fee of {:.2}%, each block split by the last {window} blocks' worth of shares", fee as f64 / 1e7),
            Some(Mode::Solo) => "Alone: each block found pays the address in full".to_string(),
            Some(Mode::Node) => "For the node itself: its blocks pay its own address".to_string(),
            None => "Not said yet".to_string(),
        });
    });
    super::row(ui, &palette, "Hashrate", |ui| {
        ui.label(RichText::new(format::hashrate(rate)).color(palette.ink));
    });

    for device in &devices {
        note(ui, &format!("{} · {}", device.name, format::hashrate(device.rate)));
    }

    super::row(ui, &palette, "Jobs", |ui| {
        ui.label(count(status.jobs));
    });
    super::row(ui, &palette, "Blocks found", |ui| {
        ui.label(RichText::new(count(status.blocks)).color(if status.blocks > 0 { palette.good } else { palette.soft }));
    });
    super::row(ui, &palette, "Refused", |ui| {
        ui.label(count(status.refused));
    });

    match &status.mode {
        Some(Mode::Pool { .. }) => {
            super::row(ui, &palette, "Shares", |ui| {
                let at = status.share_difficulty.map_or_else(String::new, |difficulty| format!(", at difficulty {}", format::thousands(&difficulty.to_string())));
                ui.label(format!("{}{at}", count(status.shares)));
            });

            if let Some(standing) = &status.standing {
                super::row(ui, &palette, "Next block", |ui| {
                    ui.label(format!("{:.4}% of it", standing["parts"].as_u64().unwrap_or(0) as f64 / 1e7));
                });
                super::row(ui, &palette, "Earned", |ui| {
                    ui.label(RichText::new(ctx.amount(None, standing["earned"].as_str().unwrap_or("0"))).color(palette.good));
                });
                note(ui, "As the pool says: the address's part of the pool's next block, by its shares in the window, and all the pool has credited it.");
            }
        }
        _ => {
            if let Some(difficulty) = status.difficulty {
                super::row(ui, &palette, "A block every", |ui| {
                    ui.label(block_every(difficulty, rate).unwrap_or_else(|| "once the hashrate is known".to_string()));
                });
                note(ui, &format!("On average, at this hashrate and the block difficulty of {}. Finding one is luck: it may come much sooner, or much later.", format::thousands(&difficulty.to_string())));
            }
        }
    }
}

/// What the pool owes the address: all it earned, what it claimed, and what a claim pays now.
fn earnings(ui: &mut Ui, ctx: &mut Ctx, from: &Address, claim: Option<&Result<Value, Failure>>, standing: Option<&Result<Value, Failure>>, kept: usize, prepared: &mut Option<Prepared>) {
    let palette = *ctx.palette;
    let note = |ui: &mut Ui, text: &str| {
        ui.label(RichText::new(text).small().color(palette.muted));
    };

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Earnings");

        // A node with no pool says so at both; the pool's own answer is the plainer.
        if let Some(Err(failure)) = standing
            && failure.status == NOT_FOUND
        {
            ui.label(RichText::new("This node runs no mining pool.").color(palette.ink));
            note(ui, &format!("{}. If your pool runs on another node, set its node below.", capital(&failure.message)));

            return;
        }

        let value = match claim {
            None => {
                super::waiting(ui, &palette, "Asking the pool…");

                return;
            }
            Some(Err(failure)) if failure.status == NOT_FOUND => {
                ui.label(RichText::new("The pool owes this address nothing yet.").color(palette.ink));
                note(ui, &format!("{}. The pool credits its miners once its blocks are final, so earnings show a while after the blocks are found.", capital(&failure.message)));

                return;
            }
            Some(Err(failure)) => {
                ui.colored_label(palette.bad, format!("The pool's node did not answer: {}", failure.message));

                return;
            }
            Some(Ok(value)) => value,
        };
        let number = |name: &str| value[name].as_str().unwrap_or("0").to_string();

        super::row(ui, &palette, "Earned", |ui| {
            ui.label(RichText::new(ctx.amount(None, &number("earned"))).color(palette.ink));
        });
        super::row(ui, &palette, "Claimed", |ui| {
            ui.label(ctx.amount(None, &number("claimed")));
        });
        super::row(ui, &palette, "Claimable", |ui| {
            ui.label(RichText::new(ctx.amount(None, &number("claimable"))).color(palette.good).size(16.0));
        });
        super::row(ui, &palette, "Pool", |ui| super::address(ui, &palette, value["pool"].as_str().unwrap_or("?")));
        note(
            ui,
            &format!(
                "As the pool's node says, from the claims its block {} committed to. Earned is all the pool has credited this address; a claim pays it less what was claimed before, from the pool's escrow.",
                value["height"].as_u64().map_or_else(|| "?".to_string(), |height| format::thousands(&height.to_string()))
            ),
        );
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
                        what: "Claim mining earnings".to_string(),
                        about: format!("{} from the pool's escrow", ctx.amount(None, &claimable.to_string())),
                        coin: u256::ZERO,
                    });
                }

                if claimable == u256::ZERO {
                    note(ui, "Nothing to claim: all it credited is claimed. The pool may send claims for its miners itself.");
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
}

/// How the address stands in the pool's window: its part of the next block, its shares and the
/// workers mining for it.
fn in_the_pool(ui: &mut Ui, ctx: &mut Ctx, value: &Value, claim: Option<&Value>) {
    let palette = *ctx.palette;
    let miner = value["miners"].as_array().and_then(|miners| miners.first());
    let workers = value["workers"].as_array().cloned().unwrap_or_default();

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "In the pool");

        match miner {
            Some(miner) => {
                super::row(ui, &palette, "Next block", |ui| {
                    ui.label(RichText::new(format!("{:.2}%", miner["percent"].as_f64().unwrap_or(0.0))).color(palette.ink));
                });
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
                ui.label(RichText::new("No shares from this address in the pool's window: its next block gives it nothing.").color(palette.muted));
            }
        }

        super::row(ui, &palette, "Pool's fee", |ui| {
            ui.label(format!("{:.2}% of each block", value["fee_percent"].as_f64().unwrap_or(0.0)));
        });

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
    });
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

/// How to mine for the address from other machines, such as a rig: a worker pointed at the
/// mining port, paying it.
fn how_to_mine(ui: &mut Ui, ctx: &mut Ctx, from: &Address, port: Option<&str>) {
    let palette = *ctx.palette;
    let command = format!("chainv4 worker --node {} --payout {from}", port.unwrap_or("HOST:PORT"));

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Mining from other machines");
        ui.label(RichText::new("Run a worker on each machine, pointed at the mining port, which is not the node's API, and paying this address:").color(palette.soft));
        ui.add_space(4.0);
        super::text_block(ui, &palette, &command);
        ui.horizontal_wrapped(|ui| {
            super::copy(ui, &palette, &command);
        });
        ui.label(
            RichText::new("Add --solo to mine alone. A private port also wants --token FILE, the token its operator gave you. The pool credits your part of each block it finds once the block is final, so earnings show a while after the blocks; one claim takes everything credited so far.")
                .small()
                .color(palette.muted),
        );
    });
}

/// The claim a pool's answer describes, to send or keep, with what it would pay now: none for an
/// answer about another address than the one asked about.
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
                    ui.colored_label(palette.bad, format!("The wallet's node did not answer: {}", failure.message));

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
                    what: "Claim mining earnings".to_string(),
                    about: format!("{} from the pool's escrow, by a claim kept on this device", ctx.amount(None, &claimable.to_string())),
                    coin: u256::ZERO,
                });
            }

            return;
        }

        ui.label(RichText::new("None of the kept claims holds on this chain: the blocks they name were undone.").color(palette.muted));
    });
}

/// The API's lowercase message as a sentence's start.
fn capital(text: &str) -> String {
    let mut chars = text.chars();

    chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
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

    #[test]
    fn the_port_is_where_the_node_publishes_it_else_the_nodes_own_host() {
        let said = |public: Value| json!({ "port": 4002, "public": public, "protocol": "chainv4", "pool": true, "solo": true, "open": true, "ws": "/mining/ws" });

        assert_eq!(port_address(&said(json!("miner.example:4100")), "http://203.0.113.7:8080").as_deref(), Some("miner.example:4100"));
        assert_eq!(port_address(&said(Value::Null), "http://203.0.113.7:8080").as_deref(), Some("203.0.113.7:4002"));
        assert_eq!(port_address(&said(json!("")), "https://user@node.example/api/").as_deref(), Some("node.example:4002"));
        assert_eq!(port_address(&said(Value::Null), "http://[2001:db8::1]:8080").as_deref(), Some("[2001:db8::1]:4002"));
        assert_eq!(port_address(&said(Value::Null), "abcdef.onion:8080").as_deref(), Some("abcdef.onion:4002"));
        assert_eq!(port_address(&json!({ "public": null }), "http://node"), None, "no port said");
        assert_eq!(port_address(&said(Value::Null), "http://"), None, "no host");
    }

    #[test]
    fn a_browser_mines_over_the_nodes_websocket_secure_as_its_api() {
        assert_eq!(socket_url("http://127.0.0.1:8080/", "/mining/ws").as_deref(), Some("ws://127.0.0.1:8080/mining/ws"));
        assert_eq!(socket_url("https://node.example/api", "mining/ws").as_deref(), Some("wss://node.example/api/mining/ws"));
        assert_eq!(socket_url("node.example", "/mining/ws"), None);
    }

    #[test]
    fn a_block_alone_takes_as_many_hashes_as_its_difficulty() {
        assert_eq!(block_every(500, 100.0).as_deref(), Some("about 5.0s"));
        assert_eq!(block_every(7_200_000_000, 1_000_000.0).as_deref(), Some("about 2h 0m"));
        assert_eq!(block_every(u128::from(u64::MAX), 1_000.0).as_deref(), Some("more than a thousand years"));
        assert_eq!(block_every(63_115_200_000, 1_000.0).as_deref(), Some("about 2.0 years"));
        assert_eq!(block_every(1_000, 0.0), None);
        assert_eq!(block_every(1_000, f64::NAN), None);
    }

    #[test]
    fn only_a_phone_off_its_charger_pauses_and_only_unless_it_may_mine_on_battery() {
        assert!(pauses(true, false, Some(false)));
        assert!(!pauses(true, false, Some(true)), "on its charger");
        assert!(!pauses(true, false, None), "a charger it says nothing of");
        assert!(!pauses(true, true, Some(false)), "on battery, as allowed");
        assert!(!pauses(false, false, Some(false)), "a desktop never pauses");
    }

    #[test]
    fn a_plan_takes_the_port_the_node_names_its_token_and_what_it_allows() {
        let payout = Address::Ed25519([3; 32]);
        let with = |settings: &Settings, change: &dyn Fn(&mut Settings)| {
            let mut settings = settings.clone();
            change(&mut settings);
            settings
        };
        let settings = with(&Settings::default(), &|settings| {
            settings.node = "http://203.0.113.7:8080".to_string();
            settings.proxy = "socks5h://127.0.0.1:9050".to_string();
        });
        let open = json!({ "port": 4002, "public": null, "protocol": "mainnet", "pool": true, "solo": false, "open": true, "fee": 10_000_000, "ws": "/mining/ws" });
        let config = plan(&settings, Some(&open), payout, 0).unwrap();

        assert_eq!(config.port, Port::Tcp { address: "203.0.113.7:4002".to_string(), proxy: Some("socks5h://127.0.0.1:9050".to_string()) });
        assert_eq!((config.token.as_slice(), config.protocol.as_str(), config.solo), (OPEN_TOKEN, "mainnet", false));
        assert_eq!((config.payout, config.devices), (payout, Devices::Cpu(1)));

        assert!(plan(&with(&settings, &|settings| settings.mine_solo = true), Some(&open), payout, 0).is_err_and(|why| why.contains("alone")));
        assert!(plan(&with(&settings, &|settings| settings.mine_gpu = true), Some(&open), payout, 0).is_err_and(|why| why.contains("no GPU")));
        assert_eq!(plan(&with(&settings, &|settings| (settings.mine_gpu, settings.mine_gpus) = (true, Some(1))), Some(&open), payout, 2).unwrap().devices, Devices::Gpu(Some(vec![1])));
        assert_eq!(plan(&with(&settings, &|settings| (settings.mine_gpu, settings.mine_gpus) = (true, Some(5))), Some(&open), payout, 2).unwrap().devices, Devices::Gpu(None), "one gone: every one");

        let private = json!({ "port": 4002, "public": "pool.example:4002", "protocol": "chainv4", "pool": false, "solo": true, "open": false });
        let solo = with(&settings, &|settings| (settings.mine_solo, settings.proxy) = (true, String::new()));

        assert!(plan(&solo, Some(&private), payout, 0).is_err_and(|why| why.contains("token")));
        let config = plan(&with(&solo, &|settings| settings.mine_token = "secret".to_string()), Some(&private), payout, 0).unwrap();
        assert_eq!(config.port, Port::Tcp { address: "pool.example:4002".to_string(), proxy: None });
        assert_eq!(config.token, b"secret");

        let http = with(&solo, &|settings| (settings.mine_token, settings.proxy) = ("secret".to_string(), "http://127.0.0.1:8118".to_string()));
        assert!(plan(&http, Some(&private), payout, 0).is_err_and(|why| why.contains("HTTP")), "never around the proxy set");

        // A port typed in place of the node's: the node's word on its own does not hold for it.
        let typed = with(&settings, &|settings| settings.mine_port = "rig.example:5000".to_string());
        let config = plan(&typed, Some(&private), payout, 0).unwrap();
        assert_eq!((config.token.as_slice(), config.protocol.as_str()), (OPEN_TOKEN, DEFAULT_PROTOCOL));
        assert!(plan(&settings, None, payout, 0).is_err_and(|why| why.contains("host:port")), "no port known");
    }

    #[test]
    fn the_pools_node_when_set_is_where_the_port_is_asked() {
        let mut settings = Settings::default();
        settings.node = "http://wallet-node".to_string();

        assert_eq!(mining_node(&settings), "http://wallet-node");
        settings.pool_node = "http://pool-node/".to_string();
        assert_eq!(mining_node(&settings), "http://pool-node");
    }

    #[test]
    fn the_apis_messages_start_a_sentence() {
        assert_eq!(capital("this node runs no mining pool"), "This node runs no mining pool");
        assert_eq!(capital(""), "");
    }
}
