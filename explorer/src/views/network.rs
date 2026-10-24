//! The node and its peers: where it listens and what it does, what it heard and did since it
//! started, who it is connected to and how far their chains got.

use {
    super::{BAD, Ctx, GOOD, MAIN, SIDE},
    crate::api::Ask,
    crate::format,
    crate::model::{NodeView, Peer},
    egui::{Align2, Color32, FontId, Pos2, Sense, Stroke, Ui, pos2, vec2},
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

pub fn show(ui: &mut Ui, ctx: &mut Ctx, streaming: &Streaming) {
    let store = ctx.store;
    let Some(node) = &store.node else {
        super::waiting(ui, ctx, &Ask::Node);

        return;
    };
    let height = store.status.as_ref().map_or(0, |status| status.height);

    ui.columns(2, |columns| {
        about(&mut columns[0], ctx, node, streaming);
        graph(&mut columns[1], node, height);
    });

    ui.add_space(6.0);
    ui.strong(format!("Peers ({})", node.peers.len()));
    peers(ui, ctx, &node.peers, height);
}

fn about(ui: &mut Ui, ctx: &mut Ctx, node: &NodeView, streaming: &Streaming) {
    super::facts(ui, "node", |ui| {
        super::fact(ui, "API");
        super::copyable(ui, ctx.api.base());
        ui.end_row();
        super::fact(ui, "up for");
        ui.label(format::span(node.uptime_ms));
        ui.end_row();
        super::fact(ui, "role");
        ui.horizontal(|ui| {
            super::badge(ui, "follows", MAIN);

            if node.producing {
                super::badge(ui, "produces", GOOD);
            }

            if node.validating {
                super::badge(ui, "validates", super::JUSTIFIED);
            }
        });
        ui.end_row();
        super::fact(ui, "updates");

        match streaming {
            Streaming::Live => ui.colored_label(GOOD, "streamed as they happen"),
            Streaming::Connecting => ui.weak("opening the stream…"),
            Streaming::Unsupported => ui.weak("polled: the node does not stream (start it with --stream)"),
            Streaming::Lost(why) => ui.colored_label(SIDE, format!("polled: the stream dropped ({why})")),
            Streaming::Off => ui.weak("polled"),
        };

        ui.end_row();
        super::fact(ui, "syncing");

        match &node.syncing {
            Some(syncing) => ui.label(format!("from height {} off {}", syncing.from, format::short(&syncing.peer))),
            None => ui.weak("no"),
        };

        ui.end_row();
        super::fact(ui, "mempool");
        ui.add(
            egui::ProgressBar::new(node.mempool.count as f32 / node.mempool.capacity.max(1) as f32)
                .text(format!("{} of {}", format::thousands(&node.mempool.count.to_string()), format::thousands(&node.mempool.capacity.to_string())))
                .desired_width(220.0),
        );
        ui.end_row();

        if let Some(backlog) = &node.backlog {
            super::fact(ui, "backlog");
            ui.add(
                egui::ProgressBar::new(backlog.waiting as f32 / backlog.total.max(1) as f32)
                    .text(format!(
                        "{} of {} waiting, {} per peer",
                        format::thousands(&backlog.waiting.to_string()),
                        format::thousands(&backlog.total.to_string()),
                        format::thousands(&backlog.per_peer.to_string())
                    ))
                    .desired_width(220.0),
            )
            .on_hover_text("Gossiped transactions and votes, and submissions, on their way to the node loop. Past either limit they are dropped, and a peer that keeps going over its share is disconnected.");
            ui.end_row();
        }

        super::fact(ui, "listening");
        ui.vertical(|ui| {
            for address in &node.listening {
                super::copyable(ui, address);
            }
        });
        ui.end_row();
    });

    ui.add_space(6.0);
    ui.strong("Since it started");
    egui::Grid::new("counters").num_columns(4).striped(true).show(ui, |ui| {
        for (index, (name, count)) in node.counters.iter().enumerate() {
            ui.weak(name.replace('_', " "));
            ui.monospace(format::thousands(&count.to_string()));

            if index % 2 == 1 {
                ui.end_row();
            }
        }
    });
}

/// This node in the middle and its peers around it, joined more thickly the more they gossiped.
fn graph(ui: &mut Ui, node: &NodeView, height: u64) {
    let size = vec2(ui.available_width(), 320.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter_at(rect);
    let center = rect.center();
    let radius = (rect.height().min(rect.width()) / 2.0 - 40.0).max(40.0);
    let loudest = node.peers.iter().map(heard).max().unwrap_or(0).max(1) as f32;

    for (index, peer) in node.peers.iter().enumerate() {
        let angle = std::f32::consts::TAU * index as f32 / node.peers.len() as f32 - std::f32::consts::FRAC_PI_2;
        let at = center + vec2(angle.cos(), angle.sin()) * radius;
        let width = 1.0 + 5.0 * heard(peer) as f32 / loudest;
        let color = match peer.chain.as_ref().map(|chain| chain.height) {
            Some(theirs) if theirs > height => SIDE,
            Some(_) => GOOD,
            None => Color32::GRAY,
        };

        painter.line_segment([center, at], Stroke::new(width, color.gamma_multiply(0.6)));
        dot(&painter, at, 9.0, color, &format::short(&peer.id));
    }

    dot(&painter, center, 14.0, MAIN, "this node");

    if node.peers.is_empty() {
        painter.text(center + vec2(0.0, 52.0), Align2::CENTER_CENTER, "no peers connected", FontId::proportional(12.0), BAD);
    }
}

fn dot(painter: &egui::Painter, at: Pos2, radius: f32, color: Color32, label: &str) {
    painter.circle_filled(at, radius, color);
    painter.text(pos2(at.x, at.y + radius + 8.0), Align2::CENTER_CENTER, label, FontId::monospace(10.0), Color32::LIGHT_GRAY);
}

fn heard(peer: &Peer) -> u64 {
    peer.heard.blocks + peer.heard.txs + peer.heard.votes
}

fn peers(ui: &mut Ui, ctx: &mut Ctx, peers: &[Peer], height: u64) {
    egui::Grid::new("peers").num_columns(6).striped(true).show(ui, |ui| {
        for heading in ["peer", "address", "connected", "heard", "their height", "justified"] {
            ui.strong(heading);
        }

        ui.end_row();

        for peer in peers {
            super::copyable(ui, &format::short(&peer.id));
            ui.monospace(&peer.address);
            ui.weak(format::ago(peer.since, ctx.now));
            ui.horizontal(|ui| {
                ui.label(format!("{} blocks · {} txs · {} votes", peer.heard.blocks, peer.heard.txs, peer.heard.votes));

                if peer.dropped > 0 {
                    ui.colored_label(BAD, format!("· {} dropped", format::thousands(&peer.dropped.to_string())));
                }
            });

            match &peer.chain {
                Some(chain) => {
                    let ahead = chain.height as i64 - height as i64;
                    let text = match ahead {
                        0 => format!("{} (level)", chain.height),
                        ahead if ahead > 0 => format!("{} ({ahead} ahead)", chain.height),
                        behind => format!("{} ({} behind)", chain.height, -behind),
                    };
                    ui.label(text).on_hover_text(format!("total work {}", format::thousands(&chain.work)));
                    ui.label(format!("epoch {}", chain.justified));
                }
                None => {
                    ui.weak("not told yet");
                    ui.weak("—");
                }
            }

            ui.end_row();
        }
    });
}
