//! The transactions waiting for a block, by sender and nonce, as the node lists them.

use {
    super::Ctx,
    crate::api::Ask,
    crate::format,
    egui::{RichText, Ui},
};

pub fn show(ui: &mut Ui, ctx: &mut Ctx) {
    let store = ctx.store;
    let Some(mempool) = &store.mempool else {
        super::waiting(ui, ctx, &Ask::Mempool);

        return;
    };

    super::title(ui, "Mempool");
    ui.add(
        egui::ProgressBar::new(mempool.count as f32 / mempool.capacity.max(1) as f32)
            .text(format!("{} waiting of {}", format::thousands(&mempool.count.to_string()), format::thousands(&mempool.capacity.to_string()))),
    );

    if mempool.txs.len() < mempool.count {
        ui.weak(format!("Showing the first {} by sender and nonce.", mempool.txs.len()));
    }

    egui::ScrollArea::vertical().id_salt("mempool").show(ui, |ui| {
        egui::Grid::new("mempool").num_columns(7).striped(true).show(ui, |ui| {
            for heading in ["id", "sender", "nonce", "type", "to", "gas limit", "max price · tip"] {
                ui.strong(heading);
            }

            ui.end_row();

            for tx in &mempool.txs {
                super::tx(ui, ctx, &tx.id);
                super::address(ui, ctx, &tx.sender);
                ui.label(tx.nonce.to_string());
                ui.label(RichText::new(tx.kind()).monospace());

                match tx.target() {
                    Some(target) => super::address(ui, ctx, target),
                    None => {
                        ui.weak("—");
                    }
                }

                ui.label(format::thousands(&tx.fee.gas_limit.to_string()));
                ui.label(format!("{} · {}", format::thousands(&tx.fee.max_price), format::thousands(&tx.fee.tip)));
                ui.end_row();
            }
        });
    });
}
