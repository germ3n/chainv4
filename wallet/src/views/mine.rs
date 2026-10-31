//! The Mine card: mining from this device for the address picked, alone or in a pool, on the node's
//! mining port as GET /mining describes it, through crate::miner, a worker on this device's GPU or
//! CPU. A phone mines only while the wallet is in front, and off its charger only when told to;
//! locking the wallet stops it everywhere, as the page's state goes with the keys. Below the rest of
//! the page, what to run to mine from other machines. A build with no `mining` feature, as the app
//! stores' are, has none of this.

use {
    super::Ctx,
    crate::app::Settings,
    crate::format,
    crate::miner::{self, Devices, MineConfig, Miner, Port},
    crate::node::Failure,
    crate::theme,
    chainv4::mining::status::Mode,
    chainv4::structs::address::Address,
    egui::{RichText, Ui},
    serde_json::Value,
};

/// What the API answers for a path it has nothing at.
const NOT_FOUND: u16 = 404;

/// How often GET /mining is asked again, in milliseconds: a node's port changes only as it restarts.
pub(super) const PORT_REFRESH: u64 = 60_000;
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

/// The miner, and what it is mined by.
#[derive(Default)]
pub struct Rig {
    /// GET /mining's answer, and the node asked and when.
    pub port: Option<Result<Value, Failure>>,
    pub port_asked: Option<(String, u64)>,
    /// Whether Retry was pressed on what GET /mining failed with.
    pub port_retry: bool,
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

impl Rig {
    /// Keeps the miner as it should be, each frame the wallet is open, on any page: paying the
    /// address picked, the account's first until another is, and restarted when that changes;
    /// stopped once a phone was in the background; paused while a phone is off its charger, unless
    /// the person lets it mine on battery, and started again once it is on one.
    pub fn keep(&mut self, ctx: &egui::Context, settings: &Settings, payout: Option<Address>, now: u64) {
        let since = now.saturating_sub(std::mem::replace(&mut self.drawn, now));

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

        let Some(payout) = payout else {
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

/// Where the mining port is reached: on a node reached at its onion, the onion's port if GET /mining
/// names one, as a wallet that came through Tor should not be sent to the node's public address.
/// Otherwise the address it says the node publishes the port at, else the node's own host with the
/// port; None for an answer without a port or a URL without a host.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(super) fn port_address(mining: &Value, node: &str) -> Option<String> {
    let said = |name: &str| mining[name].as_str().map(str::trim).filter(|said| !said.is_empty()).map(str::to_string);
    let host = node_host(node);

    if let Some(onion) = said("onion").filter(|_| host.as_deref().is_some_and(|host| host.to_ascii_lowercase().ends_with(".onion"))) {
        return Some(onion);
    }

    if let Some(public) = said("public") {
        return Some(public);
    }

    let port = mining["port"].as_u64().filter(|port| (1..=u64::from(u16::MAX)).contains(port))?;

    host.map(|host| format!("{host}:{port}"))
}

/// The host of a node's URL; None for one without.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn node_host(node: &str) -> Option<String> {
    let rest = node.trim().split_once("://").map_or(node.trim(), |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    let host = match authority.strip_prefix('[') {
        // An IPv6 address keeps its brackets, as host:port wants them.
        Some(inside) => format!("[{}]", inside.split_once(']')?.0),
        None => authority.split(':').next()?.to_string(),
    };

    (!host.is_empty() && host != "[]").then_some(host)
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
pub(super) fn mining_node(settings: &Settings) -> String {
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
        true => match settings.gpus_picked() {
            Some(picked) if picked.is_empty() => return Err("tick a GPU to mine on".to_string()),
            // Those gone since they were picked go; with all of them gone, every one there is.
            Some(picked) => Devices::Gpu(Some(picked.into_iter().filter(|which| *which < gpus).collect::<Vec<_>>()).filter(|known| !known.is_empty())),
            None => Devices::Gpu(None),
        },
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

/// A tick for each GPU listed, all of them ticked until one is not. A GPU a machine gains later is
/// mined on too, unless some were picked by hand.
fn gpu_list(ui: &mut Ui, settings: &mut Settings, gpus: &[String]) {
    let picked = settings.gpus_picked();

    for (which, name) in gpus.iter().enumerate() {
        let mut ticked = picked.as_ref().is_none_or(|picked| picked.contains(&which));

        if ui.checkbox(&mut ticked, name).changed() {
            settings.mine_gpu_list = toggled(picked.as_deref(), which, gpus.len());
            settings.mine_gpus = None;
        }
    }
}

/// What is picked once this GPU is ticked or unticked: None again once every one is.
fn toggled(picked: Option<&[usize]>, which: usize, count: usize) -> Option<Vec<usize>> {
    let mut now: Vec<usize> = (0..count).filter(|place| picked.is_none_or(|picked| picked.contains(place))).collect();

    match now.iter().position(|place| *place == which) {
        Some(at) => {
            now.remove(at);
        }
        None => now.push(which),
    }

    now.sort_unstable();

    (now.len() < count).then_some(now)
}

/// The Mine card: where the mining port is, alone or in its pool, on what, and starting and
/// stopping; while mining, how it is doing.
pub(super) fn card(ui: &mut Ui, ctx: &mut Ctx, state: &mut Rig, from: &Address, node: &str) {
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
                ui.add(egui::Label::new(chainv4_ui::widgets::flagged(ui, "This node takes no miners: it was not started with --mining.", egui::TextStyle::Body, palette.ink)).wrap());
            }
            // The wallet's own node in trouble: said at the top of every page, with Retry.
            Some(Err(failure)) if failure.is_trouble() && ctx.settings.pool_node.trim().is_empty() => super::waiting(ui, &palette, "Waiting for the node to answer…"),
            Some(Err(failure)) => {
                if theme::retry(ui, &palette, &format!("The node did not say where it takes miners. {}", failure.sentence()), false) {
                    state.port_retry = true;
                }
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
                super::row(ui, &palette, "GPUs", |ui| {
                    ui.vertical(|ui| gpu_list(ui, ctx.settings, &gpus));
                });

                if ctx.settings.gpus_picked().is_some_and(|picked| picked.is_empty()) {
                    note(ui, "Tick at least one GPU to mine on.");
                }
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

/// How to mine for the address from other machines, such as a rig: a worker pointed at the
/// mining port, paying it.
pub(super) fn how_to_mine(ui: &mut Ui, ctx: &mut Ctx, from: &Address, port: Option<&str>) {
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
        theme::note(ui, &palette, "Add --solo to mine alone. A private port also wants --token FILE, the token its operator gave you. The pool credits your part of each block it finds once the block is final, so earnings show a while after the blocks; one claim takes everything credited so far.");
    });
}

#[cfg(test)]
mod tests {
    use {super::*, serde_json::json};

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

        // Over Tor: the onion's port on a node reached at its onion, and only there.
        let both = json!({ "port": 4002, "public": "203.0.113.7:4002", "onion": "abcdef.onion:4100" });
        assert_eq!(port_address(&both, "http://abcdef.onion").as_deref(), Some("abcdef.onion:4100"));
        assert_eq!(port_address(&both, "http://ABCDEF.ONION:8080/").as_deref(), Some("abcdef.onion:4100"));
        assert_eq!(port_address(&both, "https://node.example").as_deref(), Some("203.0.113.7:4002"), "not over Tor");
        assert_eq!(port_address(&json!({ "port": 4002, "onion": "abcdef.onion:4100" }), "http://203.0.113.7:8080").as_deref(), Some("203.0.113.7:4002"));
        // An older node says no onion: as before.
        assert_eq!(port_address(&said(json!("203.0.113.7:4002")), "http://abcdef.onion").as_deref(), Some("203.0.113.7:4002"));
        assert_eq!(port_address(&json!({ "port": 4002, "public": null, "onion": "" }), "http://abcdef.onion").as_deref(), Some("abcdef.onion:4002"));
    }

    #[test]
    fn gpus_are_ticked_one_by_one_and_every_one_ticked_means_every_one() {
        assert_eq!(toggled(None, 1, 3), Some(vec![0, 2]), "all ticked until one is not");
        assert_eq!(toggled(Some(&[0, 2]), 2, 3), Some(vec![0]));
        assert_eq!(toggled(Some(&[0]), 0, 3), Some(Vec::new()), "none is a choice, and starts nothing");
        assert_eq!(toggled(Some(&[]), 2, 3), Some(vec![2]));
        assert_eq!(toggled(Some(&[2]), 0, 3), Some(vec![0, 2]), "in the list's order");
        assert_eq!(toggled(Some(&[0, 2]), 1, 3), None, "the last ticked: every one, a later GPU too");
        assert_eq!(toggled(Some(&[0, 9]), 1, 2), None, "one gone since it was picked counts for nothing");
    }

    #[test]
    fn a_gpu_list_ticks_each_by_name_and_a_tap_replaces_the_one_a_build_before_picked() {
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0));
        let gpus = ["Mali-G715 (Vulkan)".to_string(), "Mali-G715 (Gl)".to_string()];
        let mut settings = Settings::default();
        settings.mine_gpus = Some(0);
        let mut frame = |events: Vec<egui::Event>| {
            let mut spot = egui::Rect::NOTHING;
            let raw = egui::RawInput { screen_rect: Some(screen), events, ..Default::default() };
            let mut output = ctx.run_ui(raw, |ui| {
                gpu_list(ui, &mut settings, &gpus);
                spot = ui.min_rect();
            });
            output.textures_delta.clear();

            (spot, output)
        };
        let (_, _) = frame(Vec::new());
        let (spot, output) = frame(Vec::new());
        let says: Vec<String> = output.shapes.iter().filter_map(|clipped| match &clipped.shape {
            egui::epaint::Shape::Text(text) => Some(text.galley.text().to_string()),
            _ => None,
        }).collect();
        assert_eq!(says, gpus);

        // The second row, tapped: it was not picked, so now both are, which is every one.
        let second = egui::pos2(spot.left() + 8.0, spot.bottom() - 8.0);
        let press = |pressed| egui::Event::PointerButton { pos: second, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        frame(vec![egui::Event::PointerMoved(second)]);
        frame(vec![press(true)]);
        frame(vec![press(false)]);
        frame(Vec::new());
        assert_eq!((settings.gpus_picked(), settings.mine_gpus), (None, None));
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
        assert_eq!(plan(&with(&settings, &|settings| (settings.mine_gpu, settings.mine_gpu_list) = (true, Some(vec![0, 2, 9]))), Some(&open), payout, 3).unwrap().devices, Devices::Gpu(Some(vec![0, 2])), "several, less any gone");
        assert!(plan(&with(&settings, &|settings| (settings.mine_gpu, settings.mine_gpu_list) = (true, Some(Vec::new()))), Some(&open), payout, 3).is_err_and(|why| why.contains("tick a GPU")), "none ticked starts nothing");

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
}
