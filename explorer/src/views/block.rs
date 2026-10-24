//! One block: its header, its seal and proofs, and every transaction in it with what it did.

use {
    super::{Ctx, MAIN, SIDE, View},
    crate::api::Ask,
    crate::format,
    egui::{RichText, Ui},
};

pub fn show(ui: &mut Ui, ctx: &mut Ctx, block: &str) {
    let store = ctx.store;
    let Some(detail) = store.detail_for(block) else {
        ctx.api.ask(Ask::Block(block.to_string()));
        super::waiting(ui, ctx, &Ask::Block(block.to_string()));

        return;
    };
    let summary = &detail.summary;
    let main = store.on_main(summary) || summary.main == Some(true);

    ui.horizontal(|ui| {
        super::title(ui, &format!("Block {}", format::thousands(&summary.height.to_string())));

        match main {
            true => super::badge(ui, "main chain", MAIN),
            false => super::badge(ui, "side branch", SIDE),
        }

        if summary.height > 0 && ui.button("< Parent").clicked() {
            ctx.go(View::Block(summary.parent.clone()));
        }

        if let Some(child) = store.main.get(&(summary.height + 1)).filter(|_| main).cloned()
            && ui.button("Child >").clicked()
        {
            ctx.go(View::Block(child));
        }
    });

    super::facts(ui, "block", |ui| {
        super::fact(ui, "id");
        super::copyable(ui, &summary.id);
        ui.end_row();
        super::fact(ui, "parent");
        super::block(ui, ctx, &summary.parent, &summary.parent);
        ui.end_row();
        super::fact(ui, "time");
        ui.label(format!("{} UTC ({})", format::utc(summary.timestamp), format::ago(summary.timestamp, ctx.now)));
        ui.end_row();
        super::fact(ui, "producer");
        super::address(ui, ctx, &summary.producer);
        ui.end_row();

        if summary.difficulty != "0" {
            super::fact(ui, "difficulty");
            ui.label(format::thousands(&summary.difficulty));
            ui.end_row();
        }

        super::fact(ui, "gas used");
        ui.label(format::thousands(&summary.gas_used.to_string()));
        ui.end_row();
        super::fact(ui, "base price");
        ui.label(format!("{} units per gas", format::thousands(&summary.base_price)));
        ui.end_row();
        super::fact(ui, "transaction root");
        super::copyable(ui, &summary.tx_root);
        ui.end_row();

        if !summary.state_root.is_empty() {
            super::fact(ui, "receipts root");
            super::copyable(ui, &summary.receipts_root);
            ui.end_row();
            super::fact(ui, "state root");
            super::copyable(ui, &summary.state_root);
            ui.end_row();
        }
    });

    ui.add_space(6.0);
    super::json_tree(ui, ctx, "seal", &detail.seal);

    if !detail.proofs.is_empty() {
        egui::CollapsingHeader::new(format!("Finality proofs ({})", detail.proofs.len())).show(ui, |ui| {
            for (index, proof) in detail.proofs.iter().enumerate() {
                super::json_tree(ui, ctx, &format!("proof {index}"), proof);
            }
        });
    }

    ui.add_space(6.0);
    ui.strong(format!("Transactions ({})", detail.txs.len()));

    if !main {
        ui.weak("This block is not on the main chain, so its transactions have no receipts here.");
    }

    egui::ScrollArea::vertical().id_salt("block_txs").show(ui, |ui| {
        egui::Grid::new("block_txs").num_columns(7).striped(true).show(ui, |ui| {
            for heading in ["#", "id", "type", "from", "to", "status", "fee"] {
                ui.strong(heading);
            }

            ui.end_row();

            for (index, tx) in detail.txs.iter().enumerate() {
                ui.label(index.to_string());
                super::tx(ui, ctx, &tx.id);
                ui.label(RichText::new(tx.kind()).monospace());
                super::address(ui, ctx, &tx.sender);

                match tx.target() {
                    Some(target) => super::address(ui, ctx, target),
                    None => {
                        ui.weak("—");
                    }
                }

                match &tx.receipt {
                    Some(receipt) => {
                        super::status(ui, &receipt.status);
                        ui.label(ctx.coins(&receipt.fee));
                    }
                    None => {
                        ui.weak("—");
                        ui.weak("—");
                    }
                }

                ui.end_row();
            }
        });
    });
}
