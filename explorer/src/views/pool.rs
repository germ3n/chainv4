//! The mining pool the node runs, on a node that runs one: its fee and window, every miner with
//! its share of the work and what it earned, claimed and may still claim from the escrow, and the
//! blocks the pool sealed lately with whether they were credited. One miner's books, its workers
//! included, are looked up by its payout address.

use {
    super::{Ctx, Look},
    crate::api::Ask,
    crate::format,
    crate::model::{PoolBlock, PoolMiner, PoolView, PoolWorker},
    chainv4_ui::widgets,
    egui::{RichText, Ui},
};

const ABOUT: &str = "The pool this node runs: its miners' part of every block it finds goes to an escrow no key controls, which each miner claims from.";

#[derive(Default)]
pub struct PoolPage {
    /// The address typed into the lookup.
    typed: String,
    /// The miner looked up, its address as the node writes it.
    miner: Option<String>,
    trouble: Option<String>,
}

impl PoolPage {
    /// The miner looked up, whose books the page asks for too.
    pub fn miner(&self) -> Option<&str> {
        self.miner.as_deref()
    }
}

pub fn show(ui: &mut Ui, ctx: &mut Ctx, page: &mut PoolPage) {
    let look = ctx.look;
    let store = ctx.store;
    let Some(pool) = &store.pool else {
        super::heading(ui, look, "Mining pool", ABOUT, |_| {});

        match store.failures.get(&Ask::Pool) {
            Some(failure) if failure.status == 404 => none(ui, look),
            Some(_) => super::waiting(ui, ctx, &Ask::Pool),
            None => {
                ctx.api.ask(Ask::Pool);
                super::waiting(ui, ctx, &Ask::Pool);
            }
        }

        return;
    };

    super::heading(ui, look, "Mining pool", ABOUT, |ui| match pool.open {
        true => {
            widgets::pill(ui, "open", look.palette.good).on_hover_text("Anyone joins with a payout address alone");
        }
        false => {
            widgets::pill(ui, "members", look.roles.main).on_hover_text("Workers join with the mining token");
        }
    });

    super::stats(
        ui,
        look,
        &[
            ("fee".to_string(), percent(pool.fee_percent), "the producer keeps of each block".to_string()),
            ("window".to_string(), blocks(pool.window), "of shares each block splits by".to_string()),
            ("miners".to_string(), format::thousands(&pool.miners.len().to_string()), "in the pool's books".to_string()),
            ("escrow".to_string(), ctx.coins(&pool.escrow_balance), "held for miners to claim".to_string()),
            ("earned".to_string(), ctx.coins(&pool.earned), "credited to miners, all told".to_string()),
        ],
    );

    super::section(ui, look, "The pool", |ui| about(ui, ctx, pool));
    super::section_with(
        ui,
        look,
        "Miners",
        |ui| {
            ui.label(RichText::new(pool.miners.len().to_string()).monospace().color(look.palette.muted));
        },
        |ui| miners(ui, ctx, pool),
    );
    super::section_with(
        ui,
        look,
        "Blocks it sealed",
        |ui| {
            ui.label(RichText::new(pool.blocks.len().to_string()).monospace().color(look.palette.muted));
        },
        |ui| self::blocks_list(ui, ctx, &pool.blocks, None),
    );
    super::section(ui, look, "Look up a miner", |ui| lookup(ui, ctx, page));
}

/// What a node without a pool shows: how a node runs one.
fn none(ui: &mut Ui, look: Look) {
    super::section(ui, look, "No pool here", |ui| {
        super::note(
            ui,
            look,
            "This node runs no mining pool. A node runs one with --mining ADDR --pool, its workers each naming a --payout address; --pool-fee sets what its producer keeps, and --pool-window how many blocks' worth of shares each block splits by. See the consensus guide.",
        );
    });
}

/// The pool's producer and escrow, who may join, and the work in its window.
fn about(ui: &mut Ui, ctx: &mut Ctx, pool: &PoolView) {
    let look = ctx.look;
    let palette = look.palette;

    super::facts(ui, look, "pool", |facts| {
        facts.row("producer", |ui| {
            super::address(ui, ctx, &pool.producer);
            ui.label(RichText::new(format!("keeps {} of each block", percent(pool.fee_percent))).color(palette.muted));
        });
        facts.row("escrow", |ui| {
            super::address(ui, ctx, &pool.escrow);
            ui.label(RichText::new("no key controls it: miners claim from it").color(palette.muted));
        });
        facts.text(
            "who may join",
            match pool.open {
                true => "anyone, with a payout address",
                false => "workers with the mining token",
            },
        );
        facts.text("easiest share", format!("difficulty {}, set higher for faster workers", format::thousands(&pool.min_share)));
        facts.text("work in the window", format::thousands(&pool.work));
    });
}

/// Every miner: its part of the window's work, of the next block, and what it earned and claimed.
fn miners(ui: &mut Ui, ctx: &mut Ctx, pool: &PoolView) {
    let look = ctx.look;
    let palette = look.palette;

    if pool.miners.is_empty() {
        super::note(ui, look, "No miners yet. A worker joins with chainv4 worker --node HOST:PORT --payout ADDRESS.");

        return;
    }

    let work = |miner: &PoolMiner| format!("{} · {} shares", percent(fraction(&miner.work, &pool.work) * 100.0), format::thousands(&miner.shares.to_string()));

    if look.narrow {
        for miner in &pool.miners {
            super::list_row(ui, look, |ui| {
                ui.horizontal(|ui| {
                    super::address(ui, ctx, &miner.to);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(ctx.coins(&miner.claimable)).strong().color(palette.good));
                    });
                });
                ui.add(egui::Label::new(RichText::new(format!("work {} · next block {}", work(miner), percent(miner.percent))).small().color(palette.soft)).wrap());
                ui.add(egui::Label::new(RichText::new(format!("earned {} · claimed {}", ctx.coins(&miner.earned), ctx.coins(&miner.claimed))).small().color(palette.muted)).wrap());
            });
        }

        return;
    }

    super::table(ui, look, "pool_miners", &["miner", "work", "next block", "earned", "claimed", "claimable"], |ui| {
        for miner in &pool.miners {
            super::address(ui, ctx, &miner.to);
            ui.monospace(work(miner));
            ui.monospace(percent(miner.percent));
            ui.label(ctx.coins(&miner.earned));
            ui.label(RichText::new(ctx.coins(&miner.claimed)).color(palette.muted));
            ui.label(RichText::new(ctx.coins(&miner.claimable)).strong());
            ui.end_row();
        }
    });
}

/// The blocks the pool sealed, newest first: when, whether they were credited, what they shared
/// out, and among how many miners. For one miner, what its part came to instead.
fn blocks_list(ui: &mut Ui, ctx: &mut Ctx, blocks: &[PoolBlock], miner: Option<&str>) {
    let look = ctx.look;
    let palette = look.palette;

    if blocks.is_empty() {
        super::note(ui, look, "None lately.");

        return;
    }

    let mut blocks: Vec<&PoolBlock> = blocks.iter().collect();
    blocks.sort_by_key(|block| std::cmp::Reverse(block.height));
    let shared = |ctx: &Ctx, block: &PoolBlock| block.shared.as_deref().map_or_else(|| "—".to_string(), |shared| ctx.coins(shared));
    // A miner's part of a block: its percent, and what that came to once the block's total is known.
    let part = |ctx: &Ctx, block: &PoolBlock| {
        let payout = block.payouts.iter().find(|payout| Some(payout.to.as_str()) == miner)?;

        Some(match &payout.amount {
            Some(amount) => format!("{} · {}", ctx.coins(amount), percent(payout.percent)),
            None => percent(payout.percent),
        })
    };
    let miners = |block: &PoolBlock| match block.payouts.len() {
        1 => "1 miner".to_string(),
        count => format!("{count} miners"),
    };

    if look.narrow {
        for block in blocks {
            super::list_row(ui, look, |ui| {
                ui.horizontal(|ui| {
                    super::block_link(ui, ctx, &block.id, block.height);
                    ui.label(RichText::new(format::ago(block.at, ctx.now)).small().color(palette.muted));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| credited(ui, look, block));
                });

                let text = match miner {
                    Some(_) => format!("its part {} of {}", part(ctx, block).unwrap_or_default(), shared(ctx, block)),
                    None => format!("shared {} among {}", shared(ctx, block), miners(block)),
                };
                ui.add(egui::Label::new(RichText::new(text).small().color(palette.soft)).wrap());
            });
        }

        return;
    }

    let last = if miner.is_some() { "its part" } else { "miners" };

    super::table(ui, look, ("pool_blocks", miner), &["block", "sealed", "state", "shared", last], |ui| {
        for block in blocks {
            super::block_link(ui, ctx, &block.id, block.height);
            ui.label(RichText::new(format::ago(block.at, ctx.now)).color(palette.muted)).on_hover_text(format!("{} UTC", format::utc(block.at)));
            credited(ui, look, block);
            ui.label(shared(ctx, block));

            match miner {
                Some(_) => {
                    ui.label(part(ctx, block).unwrap_or_default());
                }
                None => {
                    let who: Vec<String> = block.payouts.iter().map(|payout| format!("{} · {}", format::short(&payout.to), percent(payout.percent))).collect();
                    ui.label(miners(block)).on_hover_text(who.join("\n"));
                }
            }

            ui.end_row();
        }
    });
}

/// Whether a block's payouts were credited: waiting while it is not final, dropped when a
/// reorganization took it off the main chain.
fn credited(ui: &mut Ui, look: Look, block: &PoolBlock) {
    let (text, color, about) = match block.credited {
        None => ("waiting", look.palette.warn, "Credited once the block is final"),
        Some(true) => ("credited", look.palette.good, "Its miners' parts are in their earnings"),
        Some(false) => ("dropped", look.palette.bad, "A reorganization took it off the main chain: it paid nobody"),
    };

    widgets::pill(ui, text, color).on_hover_text(about);
}

/// One miner's books, by its payout address: what it earned and may claim, its workers, and the
/// blocks that paid it.
fn lookup(ui: &mut Ui, ctx: &mut Ctx, page: &mut PoolPage) {
    let look = ctx.look;
    let palette = look.palette;
    let typed = !page.typed.trim().is_empty();
    let pressed = widgets::beside(
        ui,
        |ui| widgets::primary(ui, &palette, "Look up", typed).clicked(),
        |ui, width| {
            ui.add(egui::TextEdit::singleline(&mut page.typed).hint_text("A miner's payout address: ed25519:0x…, or an Ethereum 0x…").desired_width(width));
        },
    );

    if pressed {
        match payout_address(page.typed.trim()) {
            Ok(address) => {
                ctx.api.ask(Ask::PoolMiner(address.clone()));
                page.miner = Some(address);
                page.trouble = None;
            }
            Err(why) => page.trouble = Some(why),
        }
    }

    if let Some(trouble) = &page.trouble {
        ui.colored_label(palette.bad, trouble.as_str());
    }

    let Some(miner) = page.miner.clone() else {
        return;
    };
    let ask = Ask::PoolMiner(miner.clone());
    let Some(books) = ctx.store.pool_miners.get(&miner) else {
        ui.add_space(6.0);
        super::waiting(ui, ctx, &ask);

        return;
    };

    ui.add_space(8.0);
    let Some(standing) = books.miners.first() else {
        super::note(ui, look, format!("The pool has no shares or earnings for {miner} yet."));

        return;
    };

    super::facts(ui, look, "pool_miner", |facts| {
        facts.row("miner", |ui| super::address(ui, ctx, &standing.to));
        facts.text("work", format!("{} of the window's · {} shares", percent(fraction(&standing.work, &books.work) * 100.0), format::thousands(&standing.shares.to_string())));
        facts.text("next block", format!("{} of what goes to the escrow", percent(standing.percent)));
        facts.text("earned", ctx.coins(&standing.earned));
        facts.text("claimed", ctx.coins(&standing.claimed));
        facts.row("claimable", |ui| {
            ui.label(RichText::new(ctx.coins(&standing.claimable)).strong().color(palette.good));
            ui.label(RichText::new("with chainv4 claim --key FILE, or once the pool pushes it").color(palette.muted));
        });
    });

    ui.add_space(8.0);
    widgets::caps(ui, &palette, "Its workers");
    workers(ui, look, &books.workers);
    ui.add_space(8.0);
    widgets::caps(ui, &palette, "Blocks that paid it");
    blocks_list(ui, ctx, &books.blocks, Some(standing.to.as_str()));
}

/// A miner's workers on the pool: their hashrate, shares, and the difficulty each is set.
fn workers(ui: &mut Ui, look: Look, workers: &[PoolWorker]) {
    let palette = look.palette;

    if workers.is_empty() {
        super::note(ui, look, "None connected now.");

        return;
    }

    if look.narrow {
        for worker in workers {
            super::list_row(ui, look, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&worker.name).strong().color(palette.ink));
                    ui.label(RichText::new(format::hashrate(worker.rate)).monospace().color(palette.good));
                });
                ui.label(
                    RichText::new(format!("{} shares at difficulty {}", format::thousands(&worker.shares.to_string()), format::thousands(&worker.share_difficulty)))
                        .small()
                        .color(palette.muted),
                );
            });
        }

        return;
    }

    super::table(ui, look, "pool_workers", &["worker", "hashrate", "shares", "share difficulty"], |ui| {
        for worker in workers {
            ui.label(RichText::new(&worker.name).strong());
            ui.monospace(format::hashrate(worker.rate));
            ui.monospace(format::thousands(&worker.shares.to_string()));
            ui.monospace(format::thousands(&worker.share_difficulty));
            ui.end_row();
        }
    });
}

/// The address typed, as the node writes it, an Ethereum one as Ethereum writes it included.
fn payout_address(text: &str) -> Result<String, String> {
    let text = match text.len() == 42 && text.starts_with("0x") {
        true => format!("secp256k1:{text}"),
        false => text.to_string(),
    };

    text.parse::<chainv4::structs::address::Address>().map(|address| address.to_string()).map_err(|err| format!("{text}: {err}"))
}

/// One decimal amount's fraction of another, for a share of work: 0 when the whole is.
fn fraction(part: &str, whole: &str) -> f64 {
    match (part.parse::<f64>(), whole.parse::<f64>()) {
        (Ok(part), Ok(whole)) if whole > 0.0 => part / whole,
        _ => 0.0,
    }
}

/// A percent to two places, without the zeros that say nothing: 1.5%, 12%.
pub fn percent(value: f64) -> String {
    let text = format!("{value:.2}");

    format!("{}%", text.trim_end_matches('0').trim_end_matches('.'))
}

/// A count of blocks, one of them or several.
fn blocks(count: u64) -> String {
    match count {
        1 => "1 block".to_string(),
        count => format!("{} blocks", format::thousands(&count.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percents_lose_the_zeros_that_say_nothing() {
        assert_eq!(percent(1.5), "1.5%");
        assert_eq!(percent(12.0), "12%");
        assert_eq!(percent(0.0), "0%");
        assert_eq!(percent(33.3333), "33.33%");
    }

    #[test]
    fn a_share_of_work_is_its_fraction_and_none_of_nothing() {
        assert_eq!(fraction("25", "100"), 0.25);
        assert_eq!(fraction("5", "0"), 0.0);
        assert_eq!(fraction("x", "100"), 0.0);
        assert!((fraction("340282366920938463463374607431768211455", "680564733841876926926749214863536422910") - 0.5).abs() < 1e-9, "wider than any integer f64 holds exactly");
    }

    #[test]
    fn payout_addresses_read_as_the_node_writes_them() {
        assert!(payout_address("not an address").is_err());
        assert!(payout_address("0x52908400098527886E0F7030069857D2E4169EE7").unwrap().starts_with("secp256k1:"));
    }
}
