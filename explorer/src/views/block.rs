//! One block: its header, its seal and proofs, and every transaction in it with what it did.

use {
    super::{Ctx, TxLine, View, pool::percent},
    crate::api::Ask,
    crate::format,
    crate::model::{self, PoolShare},
    chainv4::structs::u256::u256,
    chainv4_ui::widgets,
    egui::Ui,
};

pub fn show(ui: &mut Ui, ctx: &mut Ctx, block: &str) {
    let look = ctx.look;
    let store = ctx.store;
    let Some(detail) = store.detail_for(block) else {
        ctx.api.ask(Ask::Block(block.to_string()));
        super::heading(ui, look, "Block", block, |_| {});
        super::waiting(ui, ctx, &Ask::Block(block.to_string()));

        return;
    };
    let summary = &detail.summary;
    let main = store.on_main(summary) || summary.main == Some(true);
    let child = store.main.get(&(summary.height + 1)).filter(|_| main).cloned();

    super::heading(
        ui,
        look,
        &format!("Block {}", format::thousands(&summary.height.to_string())),
        &format!("{} · {}", format::utc(summary.timestamp), format::ago(summary.timestamp, ctx.now)),
        |ui| match main {
            true => {
                widgets::pill(ui, "main chain", look.roles.main);
            }
            false => {
                widgets::pill(ui, "side branch", look.roles.side);
            }
        },
    );
    ui.horizontal(|ui| {
        if summary.height > 0 && widgets::quiet(ui, &look.palette, "‹ Parent").clicked() {
            ctx.go(View::Block(summary.parent.clone()));
        }

        if let Some(child) = child
            && widgets::quiet(ui, &look.palette, "Child ›").clicked()
        {
            ctx.go(View::Block(child));
        }
    });
    ui.add_space(8.0);

    super::section(ui, look, "Header", |ui| {
        super::facts(ui, look, "block", |facts| {
            facts.row("id", |ui| super::copyable(ui, &summary.id));
            facts.row("parent", |ui| super::block(ui, ctx, &summary.parent, &format::short(&summary.parent)));
            facts.row("producer", |ui| super::address(ui, ctx, &summary.producer));

            if summary.pruned {
                facts.row("body", |ui| super::note(ui, look, "not kept: this node synced from a snapshot after this block, and holds its header alone"));
            }

            if summary.difficulty != "0" {
                facts.text("difficulty", format::thousands(&summary.difficulty));
            }

            if !summary.reward.is_empty() && summary.reward != "0" {
                facts.text("reward", ctx.coins(&summary.reward));
            }

            facts.text("gas used", format::thousands(&summary.gas_used.to_string()));
            facts.text("base price", format!("{} units per gas", format::thousands(&summary.base_price)));
            facts.row("transaction root", |ui| super::copyable(ui, &summary.tx_root));

            if !summary.state_root.is_empty() {
                facts.row("receipts root", |ui| super::copyable(ui, &summary.receipts_root));
                facts.row("state root", |ui| super::copyable(ui, &summary.state_root));
            }

            if summary.pool.is_some() {
                facts.row("pool root", |ui| super::copyable(ui, &summary.pool_root));
            }
        });
    });

    if let Some(pool) = &summary.pool {
        super::section(ui, look, "Mining pool", |ui| self::pool(ui, ctx, pool, &summary.producer));
    }

    super::section_with(
        ui,
        look,
        "Transactions",
        |ui| {
            ui.label(egui::RichText::new(detail.txs.len().to_string()).monospace().color(look.palette.muted));
        },
        |ui| {
            if !main {
                super::note(ui, look, "This block is not on the main chain, so its transactions have no receipts here.");
            }

            if detail.txs.is_empty() {
                super::note(ui, look, "None.");

                return;
            }

            let lines: Vec<TxLine> = detail.txs.iter().enumerate().map(|(index, tx)| TxLine { tx, index: Some(index), block: None }).collect();
            super::tx_list(ui, ctx, "block_txs", &lines);
        },
    );

    super::section(ui, look, "Seal and proofs", |ui| {
        super::json_tree(ui, ctx, "seal", &detail.seal);

        if !detail.proofs.is_empty() {
            egui::CollapsingHeader::new(format!("Finality proofs ({})", detail.proofs.len())).show(ui, |ui| {
                for (index, proof) in detail.proofs.iter().enumerate() {
                    super::json_tree(ui, ctx, &format!("proof {index}"), proof);
                }
            });
        }
    });
}

/// What a pool's block sent its miners: the escrow's part of its reward and fees, what the
/// producer kept as the pool's fee, and the root the miners claim against.
fn pool(ui: &mut Ui, ctx: &mut Ctx, pool: &PoolShare, producer: &str) {
    let look = ctx.look;
    let muted = |text: String| egui::RichText::new(text).color(look.palette.muted);
    let escrowed = model::percent(pool.parts);
    let amount = |raw: &Option<String>| raw.as_deref().and_then(|raw| u256::from_str_radix(raw, 10).ok());
    // The producer kept what the escrow did not get: the pool's fee.
    let kept = amount(&pool.shared).zip(amount(&pool.escrowed)).map(|(shared, escrowed)| shared.saturating_sub(escrowed));

    super::facts(ui, look, "block_pool", |facts| {
        facts.row("to the escrow", |ui| {
            if let Some(raw) = &pool.escrowed {
                ui.label(ctx.coins(raw));
            }

            ui.label(muted(match &pool.shared {
                Some(shared) => format!("{} of {}, the reward and the producer's fees", percent(escrowed), ctx.coins(shared)),
                None => format!("{} of the reward and the producer's fees", percent(escrowed)),
            }));
        });

        if let Some(escrow) = &pool.escrow {
            facts.row("escrow", |ui| {
                super::address(ui, ctx, escrow);
                ui.label(muted("no key controls it: the pool's miners claim from it".to_string()));
            });
        }

        facts.row("the producer kept", |ui| {
            if let Some(kept) = kept {
                ui.label(ctx.coins(&kept.to_string()));
            }

            ui.label(muted(format!("{}, the pool's fee", percent(100.0 - escrowed))));
        });
        facts.row("claims root", |ui| super::copyable(ui, &pool.root));
        facts.text(
            "claims",
            match pool.count {
                1 => "1 miner's earnings, all told".to_string(),
                count => format!("{} miners' earnings, all told", format::thousands(&count.to_string())),
            },
        );
    });

    // The pool's own page, when it is the one this node runs, or may be.
    let runs = ctx.store.node.as_ref().and_then(|node| node.mining.as_ref()).is_some_and(|mining| mining.pool.is_some());
    let theirs = ctx.store.pool.as_ref().is_none_or(|known| known.producer == producer);

    if runs && theirs && widgets::quiet(ui, &look.palette, "The pool's page ›").clicked() {
        ctx.go(View::Pool);
    }
}
