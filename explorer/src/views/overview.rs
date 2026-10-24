//! Where the chain stands at a glance: the tip, how fast blocks and transactions come, the
//! network and finality, charts of the last blocks, and what just happened.

use {
    super::{Ctx, GOOD, MAIN, View},
    crate::format,
    crate::model::Summary,
    egui::{RichText, Ui},
    egui_plot::{Bar, BarChart, HLine, Line, Plot, PlotPoints},
};

/// How many blocks the numbers and charts look back over.
const WINDOW: usize = 120;

pub fn show(ui: &mut Ui, ctx: &mut Ctx) {
    let Some(status) = ctx.store.status.clone() else {
        super::waiting(ui, ctx, &crate::api::Ask::Status);

        return;
    };
    let recent = ctx.store.recent(WINDOW);
    let rates = Rates::of(&recent);
    let (finalized, justified) = ctx.store.finality_heights();

    let mut cards = vec![
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
            ctx.store.node.as_ref().map_or("—".to_string(), |node| node.peers.len().to_string()),
            ctx.store
                .node
                .as_ref()
                .map_or("no node report".to_string(), |node| format!("up {}", format::span(node.uptime_ms))),
        ),
        (
            "Mempool".to_string(),
            ctx.store.node.as_ref().map_or("—".to_string(), |node| format::thousands(&node.mempool.count.to_string())),
            "transactions waiting".to_string(),
        ),
    ];

    if let Some(finality) = &status.finality {
        cards.push((
            "Finalized".to_string(),
            format::thousands(&finalized.to_string()),
            format!("{} · justified {justified} · epoch {}", finality.kind, finality.epoch),
        ));
    }

    if status.difficulty != "0" {
        cards.push((
            "Difficulty".to_string(),
            format::thousands(&status.difficulty),
            format!("total work {}", format::thousands(&status.work)),
        ));
    }

    if let Some(base) = status.base_price() {
        cards.push(("Base price".to_string(), format::thousands(base), "units per gas".to_string()));
    }

    cards.push((
        "Chain".to_string(),
        status.hash.clone(),
        format!("{} · {} decimals", status.coin.symbol, status.coin.decimals),
    ));

    // As many to a row as fit, so none runs off the edge.
    let per_row = ((ui.available_width() + 8.0) / (CARD + 8.0)).floor().max(1.0) as usize;

    for row in cards.chunks(per_row) {
        ui.horizontal(|ui| {
            for (name, value, note) in row {
                card(ui, name, value, note);
            }
        });
    }

    ui.add_space(8.0);

    if recent.len() > 1 {
        charts(ui, &recent, status.block_gas_limit());
    }

    ui.add_space(8.0);
    ui.columns(2, |columns| {
        latest(&mut columns[0], ctx, &recent);
        feed(&mut columns[1], ctx);
    });
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

/// How wide a card is.
const CARD: f32 = 190.0;

fn card(ui: &mut Ui, name: &str, value: &str, note: &str) {
    egui::Frame::group(ui.style()).inner_margin(10.0).show(ui, |ui| {
        ui.set_width(CARD - 22.0);
        ui.vertical(|ui| {
            ui.weak(name);
            ui.label(RichText::new(value).size(22.0).strong());
            ui.small(note);
        });
    });
}

fn charts(ui: &mut Ui, recent: &[&Summary], gas_limit: Option<u64>) {
    let heights = |value: &dyn Fn(&Summary) -> f64| -> PlotPoints {
        recent.iter().map(|block| [block.height as f64, value(block)]).collect()
    };
    let times: PlotPoints = recent
        .windows(2)
        .map(|pair| [pair[1].height as f64, (pair[1].timestamp.saturating_sub(pair[0].timestamp)) as f64 / 1_000.0])
        .collect();
    let bars: Vec<Bar> = recent.iter().map(|block| Bar::new(block.height as f64, block.tx_count as f64)).collect();
    let pow = recent.iter().any(|block| block.difficulty != "0");

    ui.columns(2, |columns| {
        chart(&mut columns[0], "Seconds between blocks", |plot| plot.line(Line::new("block time", times).color(MAIN)));
        chart(&mut columns[1], "Transactions per block", |plot| {
            plot.bar_chart(BarChart::new("transactions", bars).color(GOOD));
        });
    });
    ui.columns(2, |columns| {
        chart(&mut columns[0], "Gas used", |plot| {
            plot.line(Line::new("gas used", heights(&|block| block.gas_used as f64)));

            if let Some(limit) = gas_limit {
                plot.hline(HLine::new("block gas limit", limit as f64));
            }
        });

        match pow {
            true => chart(&mut columns[1], "Difficulty", |plot| {
                plot.line(Line::new("difficulty", heights(&|block| block.difficulty.parse().unwrap_or(0.0))));
            }),
            false => chart(&mut columns[1], "Base price (units per gas)", |plot| {
                plot.line(Line::new("base price", heights(&|block| block.base_price.parse().unwrap_or(0.0))));
            }),
        }
    });
}

fn chart<'a>(ui: &mut Ui, name: &str, build: impl FnOnce(&mut egui_plot::PlotUi<'a>) + 'a) {
    ui.strong(name);
    Plot::new(name)
        .height(150.0)
        .allow_scroll(false)
        .allow_zoom(false)
        .allow_drag(false)
        .show_background(false)
        .include_y(0.0)
        .show(ui, build);
}

fn latest(ui: &mut Ui, ctx: &mut Ctx, recent: &[&Summary]) {
    ui.strong("Latest blocks");
    egui::Grid::new("latest").num_columns(5).striped(true).show(ui, |ui| {
        for block in recent.iter().rev().take(15) {
            super::block(ui, ctx, &block.id, &format::thousands(&block.height.to_string()));
            ui.monospace(format::short(&block.id));
            ui.label(format!("{} txs", block.tx_count));
            super::address(ui, ctx, &block.producer);
            ui.weak(format::ago(block.timestamp, ctx.now));
            ui.end_row();
        }
    });

    if ui.link("Open the block map").clicked() {
        ctx.go(View::Map);
    }
}

fn feed(ui: &mut Ui, ctx: &mut Ctx) {
    ui.strong("Live");

    if ctx.store.feed.is_empty() {
        ui.weak("Nothing streamed yet. A node started with --stream pushes blocks, reorgs, finality, transactions and peers here as they happen.");

        return;
    }

    egui::ScrollArea::vertical().id_salt("feed").max_height(320.0).show(ui, |ui| {
        for live in &ctx.store.feed {
            ui.horizontal(|ui| {
                ui.weak(format::ago(live.at, ctx.now));
                ui.strong(&live.name);
                ui.label(&live.text);
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
