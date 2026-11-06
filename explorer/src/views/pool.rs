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
    /// The miner and the tip its claim's entries were last asked for at.
    proving: Option<(String, String)>,
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
                let claimable = ctx.coins(&miner.claimable);
                widgets::end_row(ui, |ui| super::address(ui, ctx, &miner.to), |ui| ui.label(RichText::new(claimable).strong().color(palette.good)));
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
            super::row_end(ui);
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

            super::row_end(ui);
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
                ctx.api.ask(Ask::PoolClaim(address.clone()));
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
            widgets::flagged_label(ui, "with chainv4 claim --key FILE, or once the pool pushes it", egui::TextStyle::Body, palette.muted);
        });
    });

    // Its newest claim, checked against the chain: the root it is under and what was claimed.
    if let Some(proof) = claim_proof(ctx, page, &miner) {
        ui.add_space(4.0);
        let (text, color) = match proof {
            ClaimProof::Waiting => ("The node's figures until the light client proves its claim.".to_string(), palette.muted),
            ClaimProof::Pays { earned, claimed, pays, height, final_ } => {
                let agrees = earned.to_string() == standing.earned && claimed.to_string() == standing.claimed && pays.to_string() == standing.claimable;
                let said = format!(
                    "Its newest claim, proven at block {}{}: earned {}, claimed {}, so {} to claim. Only which claim is the pool's newest is the pool's word.",
                    format::thousands(&height.to_string()),
                    if final_ { ", final" } else { "" },
                    ctx.coins(&earned.to_string()),
                    ctx.coins(&claimed.to_string()),
                    ctx.coins(&pays.to_string())
                );

                match agrees {
                    true => (said, palette.good),
                    false => (format!("{said} The node's figures above say otherwise."), palette.warn),
                }
            }
            ClaimProof::Refused(why) => (format!("Its claim does not hold on chain: {why}. It pays nothing, whatever the node says."), palette.bad),
        };
        ui.add(egui::Label::new(RichText::new(text).small().color(color)).wrap());
    }

    ui.add_space(8.0);
    widgets::caps(ui, &palette, "Its workers");
    workers(ui, look, &books.workers);
    ui.add_space(8.0);
    widgets::caps(ui, &palette, "Blocks that paid it");
    blocks_list(ui, ctx, &books.blocks, Some(standing.to.as_str()));
}

/// What the chain says a miner's newest claim pays.
enum ClaimProof {
    Waiting,
    Pays { earned: chainv4::structs::u256::u256, claimed: chainv4::structs::u256::u256, pays: chainv4::structs::u256::u256, height: u64, final_: bool },
    Refused(String),
}

/// A claim as GET /pool/claim/{address} gives it.
fn claim_of(value: &serde_json::Value, hash: chainv4::structs::hash::HashType) -> Option<chainv4::registry::pools::PoolClaim> {
    use chainv4::structs::{address::Address, hash::Hash, hex::from_hex, u256::u256};

    let address = |name: &str| value[name].as_str()?.parse::<Address>().ok();
    let hash_of = |text: &str| Hash::from_bytes(hash, &from_hex(text)?);

    Some(chainv4::registry::pools::PoolClaim {
        pool: address("pool")?,
        height: value["height"].as_u64()?,
        block: hash_of(value["block"].as_str()?)?,
        to: address("to")?,
        earned: u256::from_str_radix(value["earned"].as_str()?, 10).ok()?,
        index: value["index"].as_u64()?,
        count: value["count"].as_u64()?,
        path: value["path"].as_array()?.iter().map(|step| hash_of(step.as_str()?)).collect::<Option<Vec<_>>>()?,
    })
}

/// Asks for a claim's two entries at the light client's tip, once per tip, and checks what came:
/// None while the node gave no claim, or the light client follows nothing yet.
fn claim_proof(ctx: &mut Ctx, page: &mut PoolPage, miner: &str) -> Option<ClaimProof> {
    use chainv4::db::{Column, StateProof};
    use chainv4::registry::pools::Pools;
    use chainv4::structs::hex::{from_hex, to_hex};

    let hash = ctx.light.hash()?;
    let claim = match claim_of(ctx.store.pool_claims.get(miner)?, hash) {
        Some(claim) => claim,
        None => return Some(ClaimProof::Refused("the node's claim does not read".to_string())),
    };
    let tip = ctx.light.tip_block()?;
    let keys = [Pools::root_key(claim.height), Pools::claimed_key(&claim.pool, &claim.to)];

    if page.proving.as_ref() != Some(&(miner.to_string(), tip.clone())) {
        for key in &keys {
            let ask = Ask::Proof { column: Column::Pools.name().to_string(), key: to_hex(key), block: Some(tip.clone()) };
            ctx.light.fetch(&ask);
            ctx.api.ask(ask);
        }

        page.proving = Some((miner.to_string(), tip));
    }

    let client = ctx.light.client()?;
    let mut entries = Vec::new();

    for key in &keys {
        let Some(Ok(answer)) = ctx.light.fetched(Column::Pools.name(), &to_hex(key)) else {
            return Some(ClaimProof::Waiting);
        };
        let checked = (|| {
            let block = chainv4::structs::hash::Hash::from_bytes(hash, &from_hex(answer["block"].as_str()?)?)?;
            let held = match answer["value"].as_str() {
                Some(text) => Some(from_hex(text)?),
                None => None,
            };
            let proof: StateProof = wincode::deserialize(&from_hex(answer["proof"].as_str()?)?).ok()?;

            Some((client.check_state(&block, Column::Pools, key, held.as_deref(), &proof), held))
        })();

        match checked {
            Some((Ok(assurance), held)) => entries.push((assurance, held)),
            Some((Err(err), _)) => return Some(ClaimProof::Refused(format!("the node's proof does not check: {err}"))),
            None => return Some(ClaimProof::Refused("the node's proof does not read".to_string())),
        }
    }

    let root = entries[0].1.as_deref().map(|bytes| Pools::decode_root(bytes).ok_or("a root that does not decode")).transpose();
    let claimed = entries[1].1.as_deref().map_or(Some(chainv4::structs::u256::u256::ZERO), Pools::decode_claimed);
    let (Ok(root), Some(claimed)) = (root, claimed) else {
        return Some(ClaimProof::Refused("an entry that does not decode".to_string()));
    };

    Some(match claim.pays(root.as_ref(), claimed, hash) {
        Ok((earned, claimed, pays)) => ClaimProof::Pays { earned, claimed, pays, height: entries[0].0.height.min(entries[1].0.height), final_: entries[0].0.final_ && entries[1].0.final_ },
        Err(why) => ClaimProof::Refused(why.to_string()),
    })
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
            super::row_end(ui);
        }
    });
}

/// The address typed, as the node writes it, an Ethereum one as Ethereum writes it included.
pub(super) fn payout_address(text: &str) -> Result<String, String> {
    let text = match text.len() == 42 && text.starts_with("0x") {
        true => format!("secp256k1:{text}"),
        false => text.to_string(),
    };

    text.parse::<chainv4::structs::address::Address>().map(|address| address.to_string()).map_err(|err| format!("{text}: {err}"))
}

/// One decimal amount's fraction of another, for a share of work: 0 when the whole is.
pub(super) fn fraction(part: &str, whole: &str) -> f64 {
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

    #[test]
    fn a_claim_reads_as_the_node_writes_it_and_holds_under_its_root() {
        use chainv4::crypto::merkle;
        use chainv4::registry::pools::{PoolRoot, claim_leaf};
        use chainv4::structs::{address::Address, hash::HashType, u256::u256};

        let hash = HashType::default_chain();
        let (pool, miner) = (Address::Ed25519([1; 32]), Address::Ed25519([2; 32]));
        let leaves = [claim_leaf(hash, &miner, u256::new(70)), claim_leaf(hash, &pool, u256::new(1))];
        let path = merkle::audit_path(hash, &leaves, 0);
        let value = serde_json::json!({
            "pool": pool.to_string(), "height": 9, "block": chainv4::structs::hash::Hash::zero(hash).to_string(), "to": miner.to_string(),
            "earned": "70", "index": 0, "count": 2, "path": path.iter().map(ToString::to_string).collect::<Vec<_>>(), "hash": hash.name(),
        });
        let claim = claim_of(&value, hash).unwrap();
        let root = PoolRoot { pool, root: merkle::root(hash, &leaves), count: 2 };

        assert_eq!((claim.height, claim.earned, claim.path.len()), (9, u256::new(70), 1));
        assert_eq!(claim.pays(Some(&root), u256::new(20), hash).unwrap(), (u256::new(70), u256::new(20), u256::new(50)));
        assert!(claim_of(&serde_json::json!({ "pool": pool.to_string() }), hash).is_none(), "half a claim");
    }
}
