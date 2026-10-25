//! The transactions waiting for a block, by sender and nonce, as the node lists them.

use {
    super::{Ctx, TxLine},
    crate::api::Ask,
    crate::format,
    egui::Ui,
};

pub fn show(ui: &mut Ui, ctx: &mut Ctx) {
    let look = ctx.look;
    super::heading(ui, look, "Mempool", "Transactions waiting for a block, by sender and nonce.", |_| {});

    let store = ctx.store;
    let Some(mempool) = &store.mempool else {
        super::waiting(ui, ctx, &Ask::Mempool);

        return;
    };

    super::section(ui, look, "Waiting", |ui| {
        super::meter(
            ui,
            look,
            mempool.count as f32 / mempool.capacity.max(1) as f32,
            &format!("{} of {}", format::thousands(&mempool.count.to_string()), format::thousands(&mempool.capacity.to_string())),
        );

        if mempool.txs.len() < mempool.count {
            super::note(ui, look, format!("Showing the first {} by sender and nonce.", mempool.txs.len()));
        }

        ui.add_space(4.0);

        if mempool.txs.is_empty() {
            super::note(ui, look, "Nothing waits: every transaction the node took is in a block.");

            return;
        }

        let lines: Vec<TxLine> = mempool.txs.iter().map(|tx| TxLine { tx, index: None, block: None }).collect();
        super::tx_list(ui, ctx, "mempool", &lines);
    });
}
