//! Where the chain stands at a glance: the tip, how fast blocks and transactions come, the
//! network and finality, charts of the last blocks, and what just happened.

use {
    super::{
        Ctx, Look, View,
        chart::{self, Kind, Series},
    },
    crate::format,
    crate::model::Summary,
    chainv4_ui::widgets,
    egui::{RichText, Ui},
};

/// How many blocks the numbers and charts look back over.
const WINDOW: usize = 120;

pub fn show(ui: &mut Ui, ctx: &mut Ctx) {
    let look = ctx.look;
    let Some(status) = ctx.store.status.clone() else {
        super::heading(ui, look, "Overview", "", |_| {});
        super::waiting(ui, ctx, &crate::api::Ask::Status);

        return;
    };
    // Genesis is left out: its time is when the chain was planned, not when it started, so the gap
    // after it says nothing of how fast blocks come.
    let recent: Vec<&Summary> = ctx.store.recent(WINDOW).into_iter().filter(|block| block.height > 0).collect();
    let rates = Rates::of(&recent);
    let (finalized, justified) = ctx.store.finality_heights();

    super::heading(
        ui,
        look,
        "Overview",
        &format!("{} · {} · {} hashes · genesis {}", status.chain_name(), status.coin.symbol, status.hash, format::short(&status.genesis)),
        |_| {},
    );

    let mut tiles = vec![
        (
            "Height".to_string(),
            format::thousands(&status.height.to_string()),
            format::ago(status.timestamp, ctx.now),
        ),
        (
            "Block time".to_string(),
            rates.block_ms.map_or("—".to_string(), |ms| format::span(ms as u64)),
            format!("over the last {} blocks", recent.len()),
        ),
        ("Throughput".to_string(), format!("{:.2} tx/s", rates.tps), format!("{} transactions", rates.txs)),
        (
            "Peers".to_string(),
            ctx.store.node.as_ref().map_or("—".to_string(), |node| match node.hides("peers") {
                true => "hidden".to_string(),
                false => node.peers.len().to_string(),
            }),
            ctx.store.node.as_ref().map_or("no node report".to_string(), |node| match node.hides("uptime") {
                true => "uptime hidden by the node".to_string(),
                false => format!("up {}", format::span(node.uptime_ms)),
            }),
        ),
        (
            "Mempool".to_string(),
            ctx.store.node.as_ref().map_or("—".to_string(), |node| format::thousands(&node.mempool.count.to_string())),
            "transactions waiting".to_string(),
        ),
    ];

    if let Some(finality) = &status.finality {
        tiles.push((
            "Finalized".to_string(),
            format::thousands(&finalized.to_string()),
            format!("{} · justified {justified} · epoch {}", finality.kind, finality.epoch),
        ));
    }

    if status.difficulty != "0" {
        tiles.push((
            "Difficulty".to_string(),
            format::thousands(&status.difficulty),
            format!("total work {}", format::thousands(&status.work)),
        ));
    }

    if let Some(mining) = ctx.store.node.as_ref().and_then(|node| node.mining.as_ref()) {
        let devices = mining.devices.len() + mining.workers.iter().map(|worker| worker.devices.len()).sum::<usize>();
        tiles.push((
            "Hashrate".to_string(),
            format::hashrate(mining.rate),
            format!("this node · {devices} device{} · {} worker{}", if devices == 1 { "" } else { "s" }, mining.workers.len(), if mining.workers.len() == 1 { "" } else { "s" }),
        ));
    }

    // Difficulty is the hashes a block takes, so over the time blocks take it is the hashrate
    // the whole network mines at.
    if status.difficulty != "0"
        && let Some(block_ms) = rates.block_ms.filter(|ms| *ms > 0.0)
    {
        let network = status.difficulty.parse::<f64>().unwrap_or(0.0) / (block_ms / 1_000.0);
        tiles.push(("Network hashrate".to_string(), format::hashrate(network), "difficulty over block time".to_string()));
    }

    if let Some(base) = status.base_price() {
        tiles.push(("Base price".to_string(), format::thousands(base), "units per gas".to_string()));
    }

    super::stats(ui, look, &tiles);

    if recent.len() > 1 {
        super::section(ui, look, &format!("The last {} blocks", recent.len()), |ui| charts(ui, look, &recent, status.block_gas_limit()));
    }

    match look.narrow {
        true => {
            latest(ui, ctx, &recent);
            feed(ui, ctx);
        }
        false => {
            ui.columns(2, |columns| {
                latest(&mut columns[0], ctx, &recent);
                feed(&mut columns[1], ctx);
            });
        }
    }
}

/// How fast the chain went over some blocks.
struct Rates {
    block_ms: Option<f64>,
    tps: f64,
    txs: usize,
}

impl Rates {
    fn of(blocks: &[&Summary]) -> Rates {
        let txs: usize = blocks.iter().skip(1).map(|block| block.tx_count).sum();
        let elapsed = match (blocks.first(), blocks.last()) {
            (Some(first), Some(last)) if last.timestamp > first.timestamp => Some((last.timestamp - first.timestamp) as f64),
            _ => None,
        };

        Rates {
            block_ms: elapsed.map(|elapsed| elapsed / (blocks.len() - 1) as f64),
            tps: elapsed.map_or(0.0, |elapsed| txs as f64 * 1_000.0 / elapsed),
            txs,
        }
    }
}

fn charts(ui: &mut Ui, look: Look, recent: &[&Summary], gas_limit: Option<u64>) {
    let roles = look.roles;
    let each = |value: &dyn Fn(&Summary) -> f64| -> Vec<[f64; 2]> { recent.iter().map(|block| [block.height as f64, value(block)]).collect() };
    let pow = recent.iter().any(|block| block.difficulty != "0");
    let series = [
        Series {
            name: "Seconds between blocks".to_string(),
            points: recent
                .windows(2)
                .map(|pair| [pair[1].height as f64, pair[1].timestamp.saturating_sub(pair[0].timestamp) as f64 / 1_000.0])
                .collect(),
            kind: Kind::Line,
            color: roles.main,
            limit: None,
            unit: |seconds| format!("{seconds:.1} s"),
            across: chart::block,
        },
        Series {
            name: "Transactions per block".to_string(),
            points: each(&|block| block.tx_count as f64),
            kind: Kind::Bars,
            color: roles.final_,
            limit: None,
            unit: |txs| format!("{txs:.0} transactions"),
            across: chart::block,
        },
        Series {
            name: "Gas used".to_string(),
            points: each(&|block| block.gas_used as f64),
            kind: Kind::Bars,
            color: roles.justified,
            limit: gas_limit.map(|limit| (limit as f64, "limit")),
            unit: |gas| format!("{} gas", format::thousands(&format!("{gas:.0}"))),
            across: chart::block,
        },
        match pow {
            true => Series {
                name: "Difficulty".to_string(),
                points: each(&|block| block.difficulty.parse().unwrap_or(0.0)),
                kind: Kind::Line,
                color: roles.side,
                limit: None,
                unit: |difficulty| format::thousands(&format!("{difficulty:.0}")),
                across: chart::block,
            },
            false => Series {
                name: "Base price, units per gas".to_string(),
                points: each(&|block| block.base_price.parse().unwrap_or(0.0)),
                kind: Kind::Line,
                color: roles.side,
                limit: None,
                unit: |price| format!("{} units", format::thousands(&format!("{price:.0}"))),
                across: chart::block,
            },
        },
    ];
    let height = if look.narrow { 84.0 } else { 110.0 };

    match look.narrow {
        true => {
            for series in &series {
                chart::chart(ui, look, series, height);
                ui.add_space(6.0);
            }
        }
        false => {
            for pair in series.chunks(2) {
                ui.columns(2, |columns| {
                    for (column, series) in columns.iter_mut().zip(pair) {
                        chart::chart(column, look, series, height);
                    }
                });
                ui.add_space(8.0);
            }
        }
    }
}

fn latest(ui: &mut Ui, ctx: &mut Ctx, recent: &[&Summary]) {
    let look = ctx.look;
    let link = |ui: &mut Ui, go: &mut bool| {
        if ui.link("Block map").clicked() {
            *go = true;
        }
    };
    let mut open_map = false;

    super::section_with(ui, look, "Latest blocks", |ui| link(ui, &mut open_map), |ui| {
        let blocks: Vec<&&Summary> = recent.iter().rev().take(if look.narrow { 8 } else { 14 }).collect();

        if look.narrow {
            for block in blocks {
                super::list_row(ui, look, |ui| {
                    ui.horizontal(|ui| {
                        super::block(ui, ctx, &block.id, &format::thousands(&block.height.to_string()));
                        ui.label(RichText::new(format!("{} txs", block.tx_count)).color(look.palette.soft));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(RichText::new(format::ago(block.timestamp, ctx.now)).small().color(look.palette.muted));
                        });
                    });
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format::short(&block.id)).monospace().small().color(look.palette.muted));
                        ui.label(RichText::new("by").small().color(look.palette.muted));
                        super::address(ui, ctx, &block.producer);
                    });
                });
            }

            return;
        }

        super::table(ui, look, "latest", &["height", "id", "txs", "producer", "age"], |ui| {
            for block in blocks {
                super::block(ui, ctx, &block.id, &format::thousands(&block.height.to_string()));
                ui.label(RichText::new(format::short(&block.id)).monospace().color(look.palette.soft));
                ui.monospace(block.tx_count.to_string());
                super::address(ui, ctx, &block.producer);
                ui.label(RichText::new(format::ago(block.timestamp, ctx.now)).color(look.palette.muted));
                super::row_end(ui);
            }
        });
    });

    if open_map {
        ctx.go(View::Map);
    }
}

fn feed(ui: &mut Ui, ctx: &mut Ctx) {
    let look = ctx.look;

    super::section(ui, look, "Live", |ui| {
        if ctx.store.feed.is_empty() {
            super::note(ui, look, "Nothing streamed yet. A node started with --stream pushes blocks, reorgs, finality, transactions and peers here as they happen.");

            return;
        }

        // The newest only, with no scrolling of its own to catch a finger scrolling the page.
        for live in ctx.store.feed.iter().take(if look.narrow { 10 } else { 16 }) {
            let color = match live.name.as_str() {
                "block" => look.roles.main,
                "reorg" | "lagged" | "peer down" => look.palette.bad,
                "finalized" => look.roles.final_,
                "justified" => look.roles.justified,
                _ => look.palette.soft,
            };
            ui.horizontal(|ui| {
                ui.add_sized([54.0, 16.0], egui::Label::new(RichText::new(format::ago(live.at, ctx.now)).small().color(look.palette.muted)));
                widgets::dot(ui, color);
                ui.label(widgets::strong(&live.name, 12.5).color(look.palette.ink));
                ui.add(egui::Label::new(RichText::new(&live.text).monospace().small().color(look.palette.soft)).truncate());
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use {super::*, crate::store::tests::block};

    #[test]
    fn rates_count_what_came_after_the_first_block() {
        let blocks: Vec<Summary> = (0..5)
            .map(|height| Summary {
                tx_count: 10,
                ..block(height, &height.to_string(), "")
            })
            .collect();
        let refs: Vec<&Summary> = blocks.iter().collect();
        let rates = Rates::of(&refs);

        // Four gaps of 2 s, and 40 transactions after the first block: 5 a second.
        assert_eq!(rates.block_ms, Some(2_000.0));
        assert_eq!(rates.txs, 40);
        assert!((rates.tps - 5.0).abs() < 1e-9);
        assert_eq!(Rates::of(&refs[..1]).block_ms, None);
    }
}
