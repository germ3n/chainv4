//! The node and its peers: where it listens and what it does, what it heard and did since it
//! started, who it is connected to and how far their chains got.

use {
    super::{Ctx, Look},
    crate::api::Ask,
    crate::format,
    crate::model::{Device, MiningView, NodeView, Peer},
    chainv4_ui::{Palette, widgets},
    egui::{Align2, Color32, FontId, Pos2, RichText, Sense, Stroke, Ui, pos2, vec2},
};

/// Where the stream stands, for the page to say.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Streaming {
    Off,
    Connecting,
    Live,
    /// The node does not stream: the explorer polls.
    Unsupported,
    Lost(String),
}

/// How the explorer hears of what changes: a color and a few words.
pub fn updates(palette: &Palette, streaming: &Streaming) -> (Color32, String) {
    match streaming {
        Streaming::Live => (palette.good, "streamed as they happen".to_string()),
        Streaming::Connecting => (palette.muted, "opening the stream…".to_string()),
        Streaming::Unsupported => (palette.muted, "polled: the node does not stream (start it with --stream)".to_string()),
        Streaming::Lost(why) => (palette.warn, format!("polled: the stream dropped ({why})")),
        Streaming::Off => (palette.muted, "polled".to_string()),
    }
}

pub fn show(ui: &mut Ui, ctx: &mut Ctx, streaming: &Streaming) {
    let look = ctx.look;
    super::heading(ui, look, "Network", "The node this explorer reads, and the peers it talks to.", |_| {});

    let store = ctx.store;
    let Some(node) = &store.node else {
        super::waiting(ui, ctx, &Ask::Node);

        return;
    };
    let height = store.status.as_ref().map_or(0, |status| status.height);

    match look.narrow {
        true => {
            about(ui, ctx, node, streaming);
            super::section(ui, look, "Peers at a glance", |ui| graph(ui, look, node, height));
        }
        false => {
            ui.columns(2, |columns| {
                about(&mut columns[0], ctx, node, streaming);
                super::section(&mut columns[1], look, "Peers at a glance", |ui| graph(ui, look, node, height));
            });
        }
    }

    if let Some(mining) = &node.mining {
        super::section_with(
            ui,
            look,
            "Mining",
            |ui| {
                ui.label(RichText::new(format::hashrate(mining.rate)).monospace().color(look.palette.good));
            },
            |ui| self::mining(ui, ctx, mining),
        );
    }

    super::section_with(
        ui,
        look,
        "Peers",
        |ui| {
            ui.label(RichText::new(node.peers.len().to_string()).monospace().color(look.palette.muted));
        },
        |ui| peers(ui, ctx, &node.peers, height),
    );
    super::section(ui, look, "Since it started", |ui| counters(ui, look, node));
}

fn about(ui: &mut Ui, ctx: &mut Ctx, node: &NodeView, streaming: &Streaming) {
    let look = ctx.look;
    let palette = look.palette;

    super::section(ui, look, "This node", |ui| {
        super::facts(ui, look, "node", |facts| {
            facts.row("API", |ui| super::copyable(ui, ctx.api.base()));
            facts.text("up for", format::span(node.uptime_ms));
            facts.row("role", |ui| {
                widgets::pill(ui, "follows", look.roles.main);

                if node.producing {
                    widgets::pill(ui, "produces", palette.good);
                }

                if node.validating {
                    widgets::pill(ui, "validates", look.roles.justified);
                }
            });
            facts.row("updates", |ui| {
                let (color, text) = updates(&palette, streaming);
                ui.label(RichText::new(text).color(color));
            });
            facts.row("syncing", |ui| match &node.syncing {
                Some(syncing) => {
                    ui.label(format!("from height {} off {}", syncing.from, format::short(&syncing.peer)));
                }
                None => {
                    ui.label(RichText::new("no").color(palette.muted));
                }
            });
            facts.row("mempool", |ui| {
                super::meter(
                    ui,
                    look,
                    node.mempool.count as f32 / node.mempool.capacity.max(1) as f32,
                    &format!("{} of {}", format::thousands(&node.mempool.count.to_string()), format::thousands(&node.mempool.capacity.to_string())),
                );
            });

            if let Some(backlog) = &node.backlog {
                facts.row("backlog", |ui| {
                    super::meter(
                        ui,
                        look,
                        backlog.waiting as f32 / backlog.total.max(1) as f32,
                        &format!(
                            "{} of {} waiting, {} per peer",
                            format::thousands(&backlog.waiting.to_string()),
                            format::thousands(&backlog.total.to_string()),
                            format::thousands(&backlog.per_peer.to_string())
                        ),
                    );
                });
            }

            facts.row("listening", |ui| {
                ui.vertical(|ui| {
                    for address in &node.listening {
                        super::copyable(ui, address);
                    }
                });
            });

            let bootnodes = ctx.store.status.as_ref().map(|status| status.bootnodes.as_slice()).unwrap_or_default();
            facts.row("chain's bootnodes", |ui| {
                ui.vertical(|ui| {
                    if bootnodes.is_empty() {
                        ui.label(RichText::new("none: nodes find each other through --peer, --bootnode and mDNS").color(palette.muted));
                    }

                    for address in bootnodes {
                        super::copyable(ui, address);
                    }
                });
            });
        });
    });
}

/// The node's mining: the hashrate over the last minutes, its own devices, the pool it runs, if
/// it does, and its workers, with whom a pool's pay.
fn mining(ui: &mut Ui, ctx: &mut Ctx, mining: &MiningView) {
    let look = ctx.look;
    let palette = look.palette;
    let history = &ctx.store.hashrates;

    if history.len() > 1 {
        let first = history.front().map_or(0, |(at, _)| *at);
        let series = super::chart::Series {
            name: "Hashrate, every device and worker".to_string(),
            points: history.iter().map(|(at, rate)| [(at - first) as f64, *rate]).collect(),
            kind: super::chart::Kind::Line,
            color: look.roles.side,
            limit: None,
            unit: format::hashrate,
            across: |since| format!("{} in", format::span(since as u64)),
        };
        super::chart::chart(ui, look, &series, if look.narrow { 84.0 } else { 110.0 });
        ui.add_space(8.0);
    }

    let fastest = mining.devices.iter().chain(mining.workers.iter().flat_map(|worker| &worker.devices)).map(|device| device.rate).fold(0.0, f64::max);

    super::facts(ui, look, "mining", |facts| {
        facts.text("hashrate", format::hashrate(mining.rate));

        match &mining.listening {
            Some(listening) => facts.row("workers on", |ui| super::copyable(ui, listening)),
            None => facts.row("workers on", |ui| {
                ui.label(RichText::new("no mining port: start the node with --mining to take workers").color(palette.muted));
            }),
        }

        if let Some(pool) = &mining.pool {
            facts.row("pool", |ui| {
                ui.label(format!(
                    "{} fee · {} blocks' window · {} miners · {}",
                    super::pool::percent(crate::model::percent(pool.fee)),
                    pool.window,
                    format::thousands(&pool.miners.to_string()),
                    if pool.open { "open to anyone" } else { "members with the token" },
                ));

                if ui.link(RichText::new("Mining pool ›").color(palette.accent)).clicked() {
                    ctx.go(super::View::Pool);
                }
            });
        }

        facts.row("this node", |ui| {
            ui.vertical(|ui| {
                if mining.devices.is_empty() {
                    ui.label(RichText::new("no devices of its own: its workers mine").color(palette.muted));
                }

                for device in &mining.devices {
                    self::device(ui, look, device, fastest);
                }
            });
        });
    });

    ui.add_space(6.0);

    if mining.workers.is_empty() {
        if mining.listening.is_some() {
            super::note(ui, look, "No workers connected. Run chainv4 worker --node HOST:PORT --token FILE on a rig to add its GPUs or threads.");
        }

        return;
    }

    let found = |worker: &crate::model::MiningWorker| match worker.refused {
        0 => format!("{} found", worker.found),
        refused => format!("{} found · {refused} refused", worker.found),
    };
    // A pool's worker: the shares it sent, at the difficulty the pool set it.
    let shares = |worker: &crate::model::MiningWorker| match worker.share_difficulty.as_str() {
        "" | "0" => format!("{} shares", format::thousands(&worker.shares.to_string())),
        difficulty => format!("{} shares at {}", format::thousands(&worker.shares.to_string()), format::thousands(difficulty)),
    };
    let pooled = mining.pool.is_some() || mining.workers.iter().any(|worker| worker.payout.is_some());

    if look.narrow {
        for worker in &mining.workers {
            super::list_row(ui, look, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&worker.name).strong().color(palette.ink));
                    ui.label(RichText::new(format::hashrate(worker.rate)).monospace().color(palette.good));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(format::ago(worker.since, ctx.now)).small().color(palette.muted));
                    });
                });
                ui.label(RichText::new(&worker.address).monospace().small().color(palette.soft));

                if let Some(payout) = &worker.payout {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("pays to").small().color(palette.muted));
                        super::address(ui, ctx, payout);
                        ui.label(RichText::new(shares(worker)).small().color(palette.muted));
                    });
                }

                ui.label(RichText::new(found(worker)).small().color(palette.muted));

                for device in &worker.devices {
                    self::device(ui, look, device, fastest);
                }
            });
        }

        return;
    }

    let headings: &[&str] = match pooled {
        true => &["worker", "address", "payout", "connected", "hashrate", "devices", "seals"],
        false => &["worker", "address", "connected", "hashrate", "devices", "seals"],
    };

    super::table(ui, look, "workers", headings, |ui| {
        for worker in &mining.workers {
            ui.label(RichText::new(&worker.name).strong());
            ui.monospace(&worker.address);

            if pooled {
                ui.vertical(|ui| match &worker.payout {
                    Some(payout) => {
                        super::address(ui, ctx, payout);
                        ui.label(RichText::new(shares(worker)).small().color(palette.muted));
                    }
                    None => {
                        ui.label(RichText::new("—").color(palette.muted));
                    }
                });
            }

            ui.label(RichText::new(format::ago(worker.since, ctx.now)).color(palette.muted));
            ui.monospace(format::hashrate(worker.rate));
            ui.vertical(|ui| {
                if worker.devices.is_empty() {
                    ui.label(RichText::new("not told yet").color(palette.muted));
                }

                for device in &worker.devices {
                    self::device(ui, look, device, fastest);
                }
            });
            ui.label(found(worker));
            ui.end_row();
        }
    });
}

/// A device and its hashrate on one line, and under it a bar as long as its share of the fastest
/// device's: a fixed width, so a narrow table cell squeezes neither.
fn device(ui: &mut Ui, look: Look, device: &Device, fastest: f64) {
    ui.vertical(|ui| {
        ui.add(
            egui::Label::new(
                RichText::new(format!("{} · {}", device.name, format::hashrate(device.rate)))
                    .small()
                    .color(look.palette.ink),
            )
            .extend(),
        );
        let (rect, _) = ui.allocate_exact_size(vec2(160.0, 4.0), Sense::hover());
        ui.painter().rect_filled(rect, 2.0, look.palette.row);
        let share = (device.rate / fastest.max(1.0)).clamp(0.0, 1.0) as f32;
        let filled = egui::Rect::from_min_size(rect.min, vec2(rect.width() * share, rect.height()));
        ui.painter().rect_filled(filled, 2.0, look.palette.good);
    });
}

/// What the node counted since it started, two to a row.
fn counters(ui: &mut Ui, look: Look, node: &NodeView) {
    let per_row = if look.narrow { 1 } else { 2 };

    egui::Grid::new("counters").num_columns(2 * per_row).striped(true).spacing([18.0, 4.0]).show(ui, |ui| {
        for (index, (name, count)) in node.counters.iter().enumerate() {
            ui.label(RichText::new(name.replace('_', " ")).color(look.palette.muted));
            ui.monospace(format::thousands(&count.to_string()));

            if index % per_row == per_row - 1 {
                ui.end_row();
            }
        }
    });
}

/// This node in the middle and its peers around it, joined more thickly the more they gossiped.
fn graph(ui: &mut Ui, look: Look, node: &NodeView, height: u64) {
    let side = if look.narrow { ui.available_width().min(300.0) } else { 300.0 };
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), side), Sense::hover());
    let painter = ui.painter_at(rect);
    let center = rect.center();
    let radius = (rect.height().min(rect.width()) / 2.0 - 40.0).max(40.0);
    let loudest = node.peers.iter().map(heard).max().unwrap_or(0).max(1) as f32;
    let palette = look.palette;

    for (index, peer) in node.peers.iter().enumerate() {
        let angle = std::f32::consts::TAU * index as f32 / node.peers.len() as f32 - std::f32::consts::FRAC_PI_2;
        let at = center + vec2(angle.cos(), angle.sin()) * radius;
        let width = 1.0 + 5.0 * heard(peer) as f32 / loudest;
        let color = match peer.chain.as_ref().map(|chain| chain.height) {
            Some(theirs) if theirs > height => look.roles.side,
            Some(_) => palette.good,
            None => palette.muted,
        };

        painter.line_segment([center, at], Stroke::new(width, color.gamma_multiply(0.6)));
        dot(&painter, palette, at, 8.0, color, &format::short(&peer.id));
    }

    dot(&painter, palette, center, 13.0, look.roles.main, "this node");

    if node.peers.is_empty() {
        painter.text(center + vec2(0.0, 48.0), Align2::CENTER_CENTER, "no peers connected", FontId::proportional(12.0), palette.bad);
    }
}

fn dot(painter: &egui::Painter, palette: Palette, at: Pos2, radius: f32, color: Color32, label: &str) {
    painter.circle_filled(at, radius, color);
    painter.text(pos2(at.x, at.y + radius + 8.0), Align2::CENTER_CENTER, label, FontId::monospace(10.0), palette.soft);
}

fn heard(peer: &Peer) -> u64 {
    peer.heard.blocks + peer.heard.txs + peer.heard.votes
}

/// How far a peer's chain got against this node's: its height, and how many ahead or behind.
fn their_height(peer: &Peer, height: u64) -> Option<String> {
    let chain = peer.chain.as_ref()?;
    let ahead = chain.height as i64 - height as i64;

    Some(match ahead {
        0 => format!("{} (level)", format::thousands(&chain.height.to_string())),
        ahead if ahead > 0 => format!("{} ({ahead} ahead)", format::thousands(&chain.height.to_string())),
        behind => format!("{} ({} behind)", format::thousands(&chain.height.to_string()), -behind),
    })
}

fn peers(ui: &mut Ui, ctx: &mut Ctx, peers: &[Peer], height: u64) {
    let look = ctx.look;
    let palette = look.palette;

    if peers.is_empty() {
        super::note(ui, look, "No peers connected. Start another node with --peer pointing at this one.");

        return;
    }

    let heard_text = |peer: &Peer| format!("{} blocks · {} txs · {} votes", peer.heard.blocks, peer.heard.txs, peer.heard.votes);

    if look.narrow {
        for peer in peers {
            super::list_row(ui, look, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format::short(&peer.id)).monospace().color(palette.ink));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(format::ago(peer.since, ctx.now)).small().color(palette.muted));
                    });
                });
                ui.add(egui::Label::new(RichText::new(&peer.address).monospace().small().color(palette.soft)).wrap());
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(their_height(peer, height).unwrap_or_else(|| "height not told yet".to_string())).small());
                    ui.label(RichText::new(heard_text(peer)).small().color(palette.muted));

                    if peer.dropped > 0 {
                        ui.colored_label(palette.bad, RichText::new(format!("{} dropped", format::thousands(&peer.dropped.to_string()))).small());
                    }
                });
            });
        }

        return;
    }

    super::table(ui, look, "peers", &["peer", "address", "connected", "heard", "their height", "justified"], |ui| {
        for peer in peers {
            super::copyable(ui, &format::short(&peer.id));
            ui.monospace(&peer.address);
            ui.label(RichText::new(format::ago(peer.since, ctx.now)).color(palette.muted));
            ui.horizontal(|ui| {
                ui.label(heard_text(peer));

                if peer.dropped > 0 {
                    ui.colored_label(palette.bad, format!("· {} dropped", format::thousands(&peer.dropped.to_string())));
                }
            });

            match (&peer.chain, their_height(peer, height)) {
                (Some(chain), Some(text)) => {
                    ui.label(text).on_hover_text(format!("total work {}", format::thousands(&chain.work)));
                    ui.label(format!("epoch {}", chain.justified));
                }
                _ => {
                    ui.label(RichText::new("not told yet").color(palette.muted));
                    ui.label(RichText::new("—").color(palette.muted));
                }
            }

            ui.end_row();
        }
    });
}
