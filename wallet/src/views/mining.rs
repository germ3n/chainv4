//! Mining in a pool: what a pool owes one of the account's addresses, how it stands in the pool's
//! window, and claiming what it earned. A pool credits its miners once its blocks can no longer be
//! undone, and its escrow pays them only by a claim: Tx::Claim, with the Merkle path from the
//! miner's leaf to a root the pool's block committed to, which the pool's node serves at
//! GET /pool/claim/{address}.
//!
//! What the pool's node says is shown as its word. A claim rests on nothing it says but the fee:
//! the chain checks the path against the root on chain and pays what the leaf says less what was
//! claimed before, so a path that does not hold fails and moves nothing. The claim is signed and
//! sent on the Send page, as any other transaction, through the wallet's own node.

use {
    super::send::{Prepared, SendState},
    super::{Ctx, Page},
    crate::format,
    crate::node::{Ask, Failure, Node},
    crate::theme,
    chainv4::structs::address::Address,
    chainv4::structs::hash::{Hash, HashType},
    chainv4::structs::hex::from_hex,
    chainv4::structs::tx::{MAX_CLAIM_PATH, Tx},
    chainv4::structs::u256::u256,
    egui::{RichText, Ui},
    serde_json::Value,
};

/// How often the pool is asked again, in milliseconds: what it owes moves at most once a block.
const REFRESH: u64 = 15_000;

/// What the API answers for a path it has nothing at.
const NOT_FOUND: u16 = 404;

#[derive(Default)]
pub struct MiningState {
    /// The address whose earnings show: one of the account's, the first until another is picked.
    from: Option<Address>,
    /// The pool's node as typed, before it is used.
    typed: Option<String>,
    trouble: Option<String>,
    /// The pool's own node, for a pool on another node than the wallet's: the URL and the proxy
    /// it was made for, and the connection.
    remote: Option<(String, String, Node)>,
    /// The pool's answers, each with the address it was asked about.
    claim: Option<(String, Result<Value, Failure>)>,
    standing: Option<(String, Result<Value, Failure>)>,
    /// The address last asked about, and when.
    asked: Option<(String, u64)>,
}

impl MiningState {
    /// The wallet's node's answer to something this page asked: it counts only while the pool is
    /// asked there, not at a node of its own.
    pub fn answered(&mut self, ask: &Ask, result: &Result<Value, Failure>) {
        if self.remote.is_none() {
            self.take(ask, result);
        }
    }

    fn take(&mut self, ask: &Ask, result: &Result<Value, Failure>) {
        match ask {
            Ask::PoolClaim(address) => self.claim = Some((address.clone(), result.clone())),
            Ask::Pool(address) => self.standing = Some((address.clone(), result.clone())),
            _ => {}
        }
    }

    /// Talks to the pool's node set, made again whenever it or the proxy changes; to the wallet's
    /// own when none is. What one node said goes once another is asked.
    fn connect(&mut self, ctx: &Ctx) {
        let url = ctx.settings.pool_node.trim().trim_end_matches('/');
        let proxy = ctx.settings.proxy.as_str();
        let current = self.remote.as_ref().map(|(url, proxy, _)| (url.as_str(), proxy.as_str()));
        let wanted = (!url.is_empty()).then_some((url, proxy));

        if current == wanted {
            return;
        }

        self.remote = wanted.map(|(url, proxy)| (url.to_string(), proxy.to_string(), Node::new(url, crate::app::client(ctx.settings), ctx.wake.clone())));
        self.claim = None;
        self.standing = None;
        self.asked = None;
    }
}

pub fn show(ui: &mut Ui, ctx: &mut Ctx, state: &mut MiningState, send: &mut SendState) {
    let palette = *ctx.palette;
    theme::title(ui, &palette, "Mining", "What a mining pool owes the account's addresses, and claiming it.");

    let addresses = ctx.holder().map(|holder| holder.addresses()).unwrap_or_default();

    if !state.from.is_some_and(|from| addresses.contains(&from)) {
        state.from = addresses.first().copied();
    }

    let Some(from) = state.from else {
        return;
    };

    if addresses.len() > 1 {
        theme::card(ui, &palette, |ui| {
            theme::caps(ui, &palette, "Address");
            egui::ComboBox::from_id_salt("mining-from")
                .selected_text(format!("{} · {}", super::scheme(&from), format::short(&from.to_string())))
                .width(ui.available_width())
                .show_ui(ui, |ui| {
                    for address in &addresses {
                        ui.selectable_value(&mut state.from, Some(*address), format!("{} · {}", super::scheme(address), format::short(&address.to_string())));
                    }
                });
        });
    }

    state.connect(ctx);

    // A pool's node of its own answers here; the wallet's, through the app.
    for (ask, result) in state.remote.as_mut().map(|(_, _, node)| node.answers()).unwrap_or_default() {
        state.take(&ask, &result);
    }

    let address = from.to_string();

    if state.asked.as_ref().is_none_or(|(asked, at)| *asked != address || ctx.now.saturating_sub(*at) > REFRESH) {
        let node = match &mut state.remote {
            Some((_, _, node)) => node,
            None => &mut *ctx.node,
        };
        node.ask(Ask::PoolClaim(address.clone()));
        node.ask(Ask::Pool(address.clone()));
        state.asked = Some((address.clone(), ctx.now));
    }

    let claim = state.claim.as_ref().filter(|(asked, _)| *asked == address).map(|(_, result)| result.clone());
    let standing = state.standing.as_ref().filter(|(asked, _)| *asked == address).map(|(_, result)| result.clone());
    let mut prepared = None;

    earnings(ui, ctx, &from, claim.as_ref(), standing.as_ref(), &mut prepared);

    if let Some(Ok(value)) = &standing {
        in_the_pool(ui, ctx, value, claim.as_ref().and_then(|claim| claim.as_ref().ok()));
    }

    pool_node(ui, ctx, state);
    how_to_mine(ui, ctx, &from);

    if let Some(prepared) = prepared {
        send.prepare(prepared);
        *ctx.page = Page::Send;
    }
}

/// What the pool owes the address: all it earned, what it claimed, and what a claim pays now.
fn earnings(ui: &mut Ui, ctx: &mut Ctx, from: &Address, claim: Option<&Result<Value, Failure>>, standing: Option<&Result<Value, Failure>>, prepared: &mut Option<Prepared>) {
    let palette = *ctx.palette;
    let note = |ui: &mut Ui, text: &str| {
        ui.label(RichText::new(text).small().color(palette.muted));
    };

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Earnings");

        // A node with no pool says so at both; the pool's own answer is the plainer.
        if let Some(Err(failure)) = standing
            && failure.status == NOT_FOUND
        {
            ui.label(RichText::new("This node runs no mining pool.").color(palette.ink));
            note(ui, &format!("{}. If your pool runs on another node, set its node below.", capital(&failure.message)));

            return;
        }

        let value = match claim {
            None => {
                super::waiting(ui, &palette, "Asking the pool…");

                return;
            }
            Some(Err(failure)) if failure.status == NOT_FOUND => {
                ui.label(RichText::new("The pool owes this address nothing yet.").color(palette.ink));
                note(ui, &format!("{}. The pool credits its miners once its blocks are final, so earnings show a while after the blocks are found.", capital(&failure.message)));

                return;
            }
            Some(Err(failure)) => {
                ui.colored_label(palette.bad, format!("The pool's node did not answer: {}", failure.message));

                return;
            }
            Some(Ok(value)) => value,
        };
        let number = |name: &str| value[name].as_str().unwrap_or("0").to_string();

        super::row(ui, &palette, "Earned", |ui| {
            ui.label(RichText::new(ctx.amount(None, &number("earned"))).color(palette.ink));
        });
        super::row(ui, &palette, "Claimed", |ui| {
            ui.label(ctx.amount(None, &number("claimed")));
        });
        super::row(ui, &palette, "Claimable", |ui| {
            ui.label(RichText::new(ctx.amount(None, &number("claimable"))).color(palette.good).size(16.0));
        });
        super::row(ui, &palette, "Pool", |ui| super::address(ui, &palette, value["pool"].as_str().unwrap_or("?")));
        note(
            ui,
            &format!(
                "As the pool's node says, from the claims its block {} committed to. Earned is all the pool has credited this address; a claim pays it less what was claimed before, from the pool's escrow.",
                value["height"].as_u64().map_or_else(|| "?".to_string(), |height| format::thousands(&height.to_string()))
            ),
        );
        ui.add_space(8.0);

        let Some(hash) = ctx.chain.client.as_ref().map(|client| client.params().hash) else {
            super::waiting(ui, &palette, "Waiting for the light client to meet the chain…");

            return;
        };

        match claim_of(value, from, hash) {
            Ok((tx, claimable)) => {
                if theme::primary(ui, &palette, "Claim", claimable > u256::ZERO).clicked() {
                    *prepared = Some(Prepared {
                        from: *from,
                        tx,
                        what: "Claim mining earnings".to_string(),
                        about: format!("{} from the pool's escrow", ctx.amount(None, &claimable.to_string())),
                        coin: u256::ZERO,
                    });
                }

                if claimable == u256::ZERO {
                    note(ui, "Nothing to claim: all it credited is claimed. The pool may send claims for its miners itself.");
                }
            }
            Err(why) => {
                ui.colored_label(palette.bad, format!("The pool's claim does not read: {why}"));
            }
        }
    });
}

/// How the address stands in the pool's window: its part of the next block, its shares and the
/// workers mining for it.
fn in_the_pool(ui: &mut Ui, ctx: &mut Ctx, value: &Value, claim: Option<&Value>) {
    let palette = *ctx.palette;
    let miner = value["miners"].as_array().and_then(|miners| miners.first());
    let workers = value["workers"].as_array().cloned().unwrap_or_default();

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "In the pool");

        match miner {
            Some(miner) => {
                super::row(ui, &palette, "Next block", |ui| {
                    ui.label(RichText::new(format!("{:.2}%", miner["percent"].as_f64().unwrap_or(0.0))).color(palette.ink));
                });
                super::row(ui, &palette, "Shares", |ui| {
                    ui.label(format!("{} in the last {} blocks' work", format::thousands(&miner["shares"].as_u64().unwrap_or(0).to_string()), value["window"].as_u64().unwrap_or(0)));
                });

                // Credited lately, and not in a root on chain yet: the pool's next block commits
                // to it, and it is claimable from then on.
                let credited = miner["earned"].as_str().and_then(|text| u256::from_str_radix(text, 10).ok());
                let committed = claim.and_then(|claim| claim["earned"].as_str()).and_then(|text| u256::from_str_radix(text, 10).ok()).unwrap_or(u256::ZERO);

                if let Some(waiting) = credited.and_then(|credited| credited.checked_sub(committed)).filter(|waiting| *waiting > u256::ZERO) {
                    super::row(ui, &palette, "Credited", |ui| {
                        ui.label(format!("{} more, claimable once the pool's next block commits to it", ctx.amount(None, &waiting.to_string())));
                    });
                }
            }
            None => {
                ui.label(RichText::new("No shares from this address in the pool's window: its next block gives it nothing.").color(palette.muted));
            }
        }

        super::row(ui, &palette, "Pool's fee", |ui| {
            ui.label(format!("{:.2}% of each block", value["fee_percent"].as_f64().unwrap_or(0.0)));
        });

        if workers.is_empty() {
            ui.label(RichText::new("No worker mining for this address is connected to the pool.").small().color(palette.muted));
        }

        for worker in &workers {
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(worker["name"].as_str().unwrap_or("worker")).color(palette.ink));
                ui.label(RichText::new(format!("{} · {} shares", format::hashrate(worker["rate"].as_f64().unwrap_or(0.0)), worker["shares"].as_u64().unwrap_or(0))).small().color(palette.soft));
            });
        }
    });
}

/// Where the pool runs, for one on another node than the wallet's.
fn pool_node(ui: &mut Ui, ctx: &mut Ctx, state: &mut MiningState) {
    let palette = *ctx.palette;
    let set = ctx.settings.pool_node.clone();
    let typed = state.typed.get_or_insert_with(|| set.clone());

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Pool's node");
        ui.label(
            RichText::new("The HTTP API of the node that runs your pool, such as http://203.0.113.7:8080, for a pool on another node than the wallet's. Empty asks the wallet's own node. The claim is still sent through the wallet's node, so the pool must be on the same chain.")
                .small()
                .color(palette.muted),
        );
        let other = typed.trim().trim_end_matches('/') != set;
        let used = super::beside(
            ui,
            |ui| theme::primary(ui, &palette, "Use", other).clicked(),
            |ui, width| {
                ui.add(egui::TextEdit::singleline(typed).hint_text("empty: the wallet's node").font(egui::TextStyle::Monospace).desired_width(width));
            },
        );

        if used {
            let url = typed.trim().trim_end_matches('/').to_string();

            match url.is_empty() || url.starts_with("http://") || url.starts_with("https://") {
                true => {
                    ctx.settings.pool_node = url.clone();
                    *typed = url;
                    state.trouble = None;
                }
                false => state.trouble = Some("A node's API starts with http:// or https://.".to_string()),
            }
        }

        if let Some(why) = &state.trouble {
            ui.colored_label(palette.bad, why.as_str());
        }
    });
}

/// How to start mining for the address: a worker pointed at the pool, paying it.
fn how_to_mine(ui: &mut Ui, ctx: &mut Ctx, from: &Address) {
    let palette = *ctx.palette;
    let command = format!("chainv4 worker --node HOST:PORT --payout {from}");

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Mining for this address");
        ui.label(RichText::new("Run a worker on each machine, pointed at the pool's mining port, which is not its API, and paying this address:").color(palette.soft));
        ui.add_space(4.0);
        super::text_block(ui, &palette, &command);
        ui.horizontal_wrapped(|ui| {
            super::copy(ui, &palette, &command);
        });
        ui.label(
            RichText::new("A private pool also wants --token FILE, the token its operator gave you. The pool credits your part of each block it finds once the block is final, so earnings show a while after the blocks; one claim takes everything credited so far.")
                .small()
                .color(palette.muted),
        );
    });
}

/// The claim a pool's answer describes, as Tx::Claim takes it, with what it would pay now: none
/// for an answer about another address than the one asked about.
fn claim_of(value: &Value, miner: &Address, hash: HashType) -> Result<(Tx, u256), String> {
    let to: Address = value["to"].as_str().and_then(|text| text.parse().ok()).ok_or("it names no miner")?;

    if to != *miner {
        return Err(format!("it is for {}, not this address", format::short(&to.to_string())));
    }

    let number = |name: &str| value[name].as_str().and_then(|text| u256::from_str_radix(text, 10).ok()).ok_or(format!("it says no {name}"));
    let whole = |name: &str| value[name].as_u64().ok_or(format!("it says no {name}"));
    let path = value["path"]
        .as_array()
        .ok_or("it has no path")?
        .iter()
        .map(|step| step.as_str().and_then(from_hex).and_then(|bytes| Hash::from_bytes(hash, &bytes)).ok_or("its path is not the chain's hashes".to_string()))
        .collect::<Result<Vec<Hash>, String>>()?;

    if path.len() > MAX_CLAIM_PATH {
        return Err(format!("its path is longer than the {MAX_CLAIM_PATH} hashes a claim may carry"));
    }

    let tx = Tx::Claim { height: whole("height")?, to, earned: number("earned")?, index: whole("index")?, path };

    Ok((tx, number("claimable")?))
}

/// The API's lowercase message as a sentence's start.
fn capital(text: &str) -> String {
    let mut chars = text.chars();

    chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use {super::*, serde_json::json};

    fn answer(to: &Address, path: Vec<String>) -> Value {
        json!({
            "pool": Address::Ed25519([1; 32]).to_string(),
            "escrow": Address::Contract([2; 32]).to_string(),
            "height": 120,
            "block": Hash::zero(HashType::default_chain()).to_string(),
            "to": to.to_string(),
            "earned": "5000000000000000000",
            "claimed": "2000000000000000000",
            "claimable": "3000000000000000000",
            "index": 3,
            "count": 7,
            "path": path,
        })
    }

    #[test]
    fn a_pools_answer_becomes_the_claim_it_describes() {
        let hash = HashType::default_chain();
        let miner = Address::Ed25519([3; 32]);
        let steps = [Hash::zero(hash), chainv4::crypto::hasher::Hasher::digest(hash, b"leaf")];
        let (tx, claimable) = claim_of(&answer(&miner, steps.iter().map(ToString::to_string).collect()), &miner, hash).expect("reads");

        assert_eq!(tx, Tx::Claim { height: 120, to: miner, earned: u256::new(5_000_000_000_000_000_000), index: 3, path: steps.to_vec() });
        assert_eq!(claimable, u256::new(3_000_000_000_000_000_000));
    }

    #[test]
    fn a_claim_for_another_address_or_with_a_path_that_does_not_read_is_refused() {
        let hash = HashType::default_chain();
        let (miner, other) = (Address::Ed25519([3; 32]), Address::Ed25519([4; 32]));

        assert!(claim_of(&answer(&other, Vec::new()), &miner, hash).is_err_and(|why| why.contains("not this address")));
        assert!(claim_of(&answer(&miner, vec!["0x12".to_string()]), &miner, hash).is_err_and(|why| why.contains("path")));
        assert!(claim_of(&answer(&miner, vec![Hash::zero(hash).to_string(); MAX_CLAIM_PATH + 1]), &miner, hash).is_err_and(|why| why.contains("longer")));
    }

    #[test]
    fn the_apis_messages_start_a_sentence() {
        assert_eq!(capital("this node runs no mining pool"), "This node runs no mining pool");
        assert_eq!(capital(""), "");
    }
}
