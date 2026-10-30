//! The richest accounts and the coin's supply. The supply's minted and burned are one state entry,
//! which the light client proves against a header it checked itself, so the page says whether
//! what the node reports is proven or only reported. The rich list and what is staked are the
//! node's word alone, and the page says so.

use {
    super::{Ctx, Look},
    crate::api::Ask,
    crate::format,
    crate::light::Light,
    crate::model::{RichList, SupplyView},
    crate::proof::{self, Proof, Verdict},
    crate::store::Store,
    chainv4::db::Column,
    chainv4::light::LightClient,
    chainv4::registry::supply::{SUPPLY_KEY, Supply},
    chainv4::structs::hex::to_hex,
    chainv4_ui::widgets,
    egui::{RichText, Ui},
    serde_json::Value,
};

const ABOUT: &str = "The accounts holding the most, and the coin's supply: every coin made, every coin destroyed, and what there is.";

/// The supply's key in the validators column, in hex, as this explorer knows it: the entry it
/// proves, whatever the node names.
pub fn supply_key() -> String {
    to_hex(SUPPLY_KEY)
}

/// The proof to ask for the supply, at the light client's tip once it caught up, kept for the page
/// to check; nothing before the node reported a supply, or while the light client is behind.
pub fn supply_proof(store: &Store, light: &mut Light) -> Option<Ask> {
    store.supply.as_ref()?;

    if !light.caught_up() {
        return None;
    }

    let ask = Ask::Proof {
        column: Column::Validators.name().to_string(),
        key: supply_key(),
        block: Some(light.tip_block()?),
    };
    light.fetch(&ask);

    Some(ask)
}

/// The supply from its state entry: two u256, little-endian, minted then burned. Nothing there
/// is nothing made yet, as on a chain from before the supply was kept.
pub fn supply_of(value: Option<&[u8]>) -> Result<Supply, String> {
    match value {
        None => Ok(Supply::default()),
        Some(bytes) if bytes.len() == 64 => wincode::deserialize(bytes).map_err(|_| "a supply that does not decode".to_string()),
        Some(bytes) => Err(format!("a supply of {} bytes, not 64", bytes.len())),
    }
}

/// How the supply stands against the light client.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Standing {
    /// The light client proved minted and burned at a block, final or not, and whether they are
    /// what the node reports, when the node reported them at that block.
    Proven { supply: Supply, height: u64, final_: bool, agrees: Option<bool> },
    /// The node's proof does not check: why.
    Wrong(String),
    /// Not proven: why. What the node reports is all there is.
    Reported(String),
}

/// How the supply the node reports stands, from its proof's answer, if one came.
pub fn standing(reported: &SupplyView, answer: Option<&Result<Value, String>>, client: Option<&LightClient>, trouble: Option<&str>) -> Standing {
    let Some(answer) = answer else {
        return Standing::Reported(match (client, trouble) {
            (_, Some(trouble)) => format!("the light client stopped ({trouble})"),
            (None, None) => "the light client is not following this node yet".to_string(),
            (Some(_), None) => "the light client has not proven it yet".to_string(),
        });
    };
    let value = match answer {
        Ok(value) => value,
        Err(why) => return Standing::Reported(format!("the node did not prove it: {why}")),
    };
    let entry = match proof::read(value) {
        Ok(Proof::State(entry)) if entry.column == Column::Validators && entry.key == SUPPLY_KEY => entry,
        Ok(_) => return Standing::Wrong("the node proved another entry than the supply".to_string()),
        Err(why) => return Standing::Wrong(why),
    };
    let checked = proof::check(&Proof::State(entry.clone()), client, trouble);

    match checked.verdict {
        Verdict::Proven { final_, height } => match supply_of(entry.value.as_deref()) {
            Ok(supply) => {
                let agrees = (height == reported.height).then(|| supply.minted.to_string() == reported.minted && supply.burned.to_string() == reported.burned);

                Standing::Proven { supply, height, final_, agrees }
            }
            Err(why) => Standing::Wrong(why),
        },
        Verdict::Wrong(why) => Standing::Wrong(why),
        Verdict::Waiting(height) => Standing::Reported(format!("the light client has yet to reach block {}", format::thousands(&height.to_string()))),
        Verdict::Unchecked(why) => Standing::Reported(why),
    }
}

/// An account's share of the supply, as a percent; None for a supply of nothing or numbers that
/// do not read.
pub fn share(balance: &str, supply: &str) -> Option<f64> {
    let (balance, supply) = (balance.parse::<f64>().ok()?, supply.parse::<f64>().ok()?);

    (supply > 0.0).then(|| balance / supply * 100.0)
}

/// A share as the page writes it: two decimals, or under 0.01% for a speck.
pub fn percent(share: Option<f64>) -> String {
    match share {
        None => "—".to_string(),
        Some(share) if share > 0.0 && share < 0.01 => "<0.01%".to_string(),
        Some(share) => format!("{share:.2}%"),
    }
}

pub fn show(ui: &mut Ui, ctx: &mut Ctx) {
    let look = ctx.look;
    let store = ctx.store;
    super::heading(ui, look, "Rich list & supply", ABOUT, |_| {});

    if store.supply.is_none() && store.failures.get(&Ask::Supply).is_none_or(|failure| failure.status != 404) {
        ctx.api.ask(Ask::Supply);
    }

    if store.rich.is_none() && store.failures.get(&Ask::Rich).is_none_or(|failure| failure.status != 404) {
        ctx.api.ask(Ask::Rich);
    }

    match &store.supply {
        Some(supply) => {
            super::stats(
                ui,
                look,
                &[
                    ("supply".to_string(), ctx.coins(&supply.supply), "minted less burned".to_string()),
                    ("minted".to_string(), ctx.coins(&supply.minted), "made since genesis".to_string()),
                    ("burned".to_string(), ctx.coins(&supply.burned), "destroyed since genesis".to_string()),
                    ("staked".to_string(), ctx.coins(&supply.staked), "bonded, as the node reports".to_string()),
                ],
            );
            super::section(ui, look, "Supply", |ui| supply_section(ui, ctx, supply));
        }
        None => super::section(ui, look, "Supply", |ui| match store.failures.get(&Ask::Supply) {
            Some(failure) if failure.status == 404 => super::note(ui, look, "This node does not report the supply: it is from before GET /supply."),
            _ => super::waiting(ui, ctx, &Ask::Supply),
        }),
    }

    let holders = store.rich.as_ref().map(|rich| format!("{} holders", format::thousands(&rich.holders.to_string())));
    super::section_with(
        ui,
        look,
        "Richest accounts",
        |ui| {
            if let Some(holders) = holders {
                ui.label(RichText::new(holders).monospace().color(look.palette.muted));
            }
        },
        |ui| match &store.rich {
            Some(rich) => richest(ui, ctx, rich, store.supply.as_ref()),
            None => match store.failures.get(&Ask::Rich) {
                Some(failure) if failure.status == 404 => super::note(ui, look, "This node does not list the richest accounts: it is from before GET /rich."),
                _ => super::waiting(ui, ctx, &Ask::Rich),
            },
        },
    );
}

/// The supply: what the node reports, and whether the light client proved it.
fn supply_section(ui: &mut Ui, ctx: &mut Ctx, supply: &SupplyView) {
    let look = ctx.look;
    let key = supply_key();
    let answer = ctx.light.fetched(Column::Validators.name(), &key).cloned();
    let standing = standing(supply, answer.as_ref(), ctx.light.client(), ctx.light.trouble.as_deref());

    ui.horizontal_wrapped(|ui| {
        standing_pill(ui, look, &standing);
        ui.label(RichText::new(standing_text(ctx, &standing, supply)).color(look.palette.soft));
    });
    ui.add_space(6.0);

    super::facts(ui, look, "supply", |facts| {
        facts.text("at block", format::thousands(&supply.height.to_string()));
        facts.text("minted", ctx.coins(&supply.minted));
        facts.text("burned", ctx.coins(&supply.burned));
        facts.text("supply", ctx.coins(&supply.supply));
        facts.row("staked", |ui| {
            ui.label(ctx.coins(&supply.staked));
            ui.label(RichText::new("reported by the node").small().color(look.palette.muted));
        });
        facts.row("reward pool", |ui| {
            ui.label(ctx.coins(&supply.reward_pool));
            ui.label(RichText::new("reported by the node").small().color(look.palette.muted));
        });
        facts.row("state key", |ui| {
            super::copyable(ui, &supply.key);

            if !supply.key.eq_ignore_ascii_case(&key) {
                ui.colored_label(look.palette.bad, format!("not the supply's key, {key}: the explorer proves its own"));
            }
        });
    });
}

fn standing_pill(ui: &mut Ui, look: Look, standing: &Standing) {
    match standing {
        Standing::Proven { agrees: Some(false), .. } | Standing::Wrong(_) => widgets::pill(ui, "does not check", look.palette.bad),
        Standing::Proven { final_: true, .. } => widgets::pill(ui, "proven, final", look.roles.final_),
        Standing::Proven { .. } => widgets::pill(ui, "proven", look.palette.good),
        Standing::Reported(_) => widgets::pill(ui, "reported by the node", look.palette.muted),
    }
    .on_hover_text("Minted and burned, checked by the light client from the node's proof against a header it checked itself, trusting only the genesis it first saw at this node");
}

/// What the light client made of the supply, in words.
fn standing_text(ctx: &Ctx, standing: &Standing, reported: &SupplyView) -> String {
    match standing {
        Standing::Proven { supply, height, agrees, .. } => {
            let at = format::thousands(&height.to_string());
            let proven = format!("minted {}, burned {}, so {}", ctx.coins(&supply.minted.to_string()), ctx.coins(&supply.burned.to_string()), ctx.coins(&supply.supply().to_string()));

            match agrees {
                Some(true) => format!("The light client proved it at block {at}: {proven}."),
                Some(false) => format!("The node's proof at block {at} says {proven}, not what it reports."),
                None => format!("Proven at block {at}: {proven}. The node reports block {}.", format::thousands(&reported.height.to_string())),
            }
        }
        Standing::Wrong(why) => format!("The node's proof does not check: {why}."),
        Standing::Reported(why) => format!("Not proven: {why}."),
    }
}

/// The richest accounts: a table on a wide screen, rows on a phone, each with its share.
fn richest(ui: &mut Ui, ctx: &mut Ctx, rich: &RichList, supply: Option<&SupplyView>) {
    let look = ctx.look;
    super::note(ui, look, format!("At block {}, as the node reports them: the rich list is not proven.", format::thousands(&rich.height.to_string())));
    ui.add_space(4.0);

    if rich.accounts.is_empty() {
        super::note(ui, look, "Nobody holds anything yet.");

        return;
    }

    let shares: Vec<String> = rich.accounts.iter().map(|holder| percent(supply.and_then(|supply| share(&holder.balance, &supply.supply)))).collect();

    if look.narrow {
        for (rank, (holder, share)) in rich.accounts.iter().zip(&shares).enumerate() {
            super::list_row(ui, look, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("#{}", rank + 1)).monospace().color(look.palette.muted));
                    super::address(ui, ctx, &holder.address);
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(ctx.coins(&holder.balance)).strong());
                    ui.label(RichText::new(format!("{share} of the supply")).small().color(look.palette.muted));
                });
            });
        }

        return;
    }

    super::table(ui, look, "rich", &["#", "account", "balance", "share"], |ui| {
        for (rank, (holder, share)) in rich.accounts.iter().zip(&shares).enumerate() {
            ui.monospace((rank + 1).to_string());
            super::address(ui, ctx, &holder.address);
            ui.label(ctx.coins(&holder.balance)).on_hover_text(format!("{} units", holder.balance));
            ui.monospace(share);
            super::row_end(ui);
        }
    });
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::api::Failure,
        crate::proof::tests::{chain, proof_json},
        chainv4::structs::u256::u256,
        serde_json::json,
    };

    fn reported(height: u64, minted: &str, burned: &str) -> SupplyView {
        serde_json::from_value(json!({
            "height": height, "minted": minted, "burned": burned, "supply": "0", "staked": "200", "reward_pool": "50", "key": supply_key(),
        }))
        .unwrap()
    }

    #[test]
    fn the_supply_decodes_from_its_state_bytes_minted_then_burned_little_endian() {
        let mut bytes = vec![0u8; 64];
        bytes[..2].copy_from_slice(&1500u16.to_le_bytes());
        bytes[32..34].copy_from_slice(&500u16.to_le_bytes());
        bytes[31] = 0x01;
        let supply = supply_of(Some(&bytes)).unwrap();

        assert_eq!(supply.minted, u256::new(1500) + (u256::ONE << 248u32));
        assert_eq!(supply.burned, u256::new(500));
        assert_eq!(supply_of(Some(&wincode::serialize(&supply).unwrap())).unwrap(), supply, "as the chain writes it");
        assert_eq!(supply_of(None).unwrap(), Supply::default(), "a chain from before it was kept made none yet");
        assert_eq!(supply_of(Some(&[0; 63])).unwrap_err(), "a supply of 63 bytes, not 64");
        assert_eq!(supply_key(), "0x636f696e3a737570706c79");
    }

    #[test]
    fn the_supply_is_proven_by_the_light_client_or_says_it_is_reported() {
        let held = Supply { minted: u256::new(1500), burned: u256::new(500) };
        let (client, state) = chain(&[(Column::Validators, SUPPLY_KEY, &wincode::serialize(&held).unwrap())]);
        let answer = Ok(proof_json(&client, &state, Column::Validators, SUPPLY_KEY));

        assert_eq!(standing(&reported(0, "1500", "500"), Some(&answer), Some(&client), None), Standing::Proven { supply: held, height: 0, final_: false, agrees: Some(true) });
        assert_eq!(
            standing(&reported(0, "1600", "500"), Some(&answer), Some(&client), None),
            Standing::Proven { supply: held, height: 0, final_: false, agrees: Some(false) },
            "a node that reports other than it proves"
        );
        assert_eq!(standing(&reported(7, "1600", "500"), Some(&answer), Some(&client), None), Standing::Proven { supply: held, height: 0, final_: false, agrees: None }, "reported at another block");

        let mut lying = proof_json(&client, &state, Column::Validators, SUPPLY_KEY);
        lying["value"] = to_hex(&[9; 64]).into();
        assert!(matches!(standing(&reported(0, "1500", "500"), Some(&Ok(lying)), Some(&client), None), Standing::Wrong(_)));

        let other = proof_json(&client, &state, Column::Validators, b"something else");
        assert_eq!(standing(&reported(0, "1500", "500"), Some(&Ok(other)), Some(&client), None), Standing::Wrong("the node proved another entry than the supply".to_string()));

        assert!(matches!(standing(&reported(0, "1500", "500"), Some(&answer), None, None), Standing::Reported(why) if why.contains("can't be checked")));
        assert_eq!(standing(&reported(0, "1500", "500"), None, None, None), Standing::Reported("the light client is not following this node yet".to_string()));
        assert_eq!(standing(&reported(0, "1500", "500"), Some(&Err("404 Not Found".to_string())), Some(&client), None), Standing::Reported("the node did not prove it: 404 Not Found".to_string()));
    }

    #[test]
    fn shares_of_the_supply_read_in_percent() {
        assert_eq!(percent(share("250", "1000")), "25.00%");
        assert_eq!(percent(share("1", "1000000")), "<0.01%");
        assert_eq!(percent(share("0", "1000")), "0.00%");
        assert_eq!(percent(share("1", "0")), "—", "a supply of nothing has no shares");
        assert_eq!(percent(share("x", "1")), "—");
    }

    fn store() -> Store {
        Store {
            rich: Some(serde_json::from_value(json!({
                "height": 120, "holders": 3,
                "accounts": [{ "address": "ed25519:0x0101010101010101010101010101010101010101010101010101010101010101", "balance": "750000000000000000000" }],
            })).unwrap()),
            supply: Some(serde_json::from_value(json!({
                "height": 120, "minted": "1500000000000000000000", "burned": "500000000000000000000", "supply": "1000000000000000000000",
                "staked": "200000000000000000000", "reward_pool": "0", "key": supply_key(),
            })).unwrap()),
            ..Store::default()
        }
    }

    #[test]
    fn the_page_shows_the_richest_their_shares_and_the_supply_on_any_screen() {
        for width in [1_200.0, 380.0] {
            let (text, _) = super::super::render(&store(), &mut Light::default(), width, show);

            for wanted in ["Rich list & supply", "3 holders", "75.00%", "1,000", "reported by the node", "the rich list is not proven", "Not proven: the light client is not following this node yet."] {
                assert!(text.contains(wanted), "{wanted} at {width} wide, in {text}");
            }
        }

        let mut older = Store::default();
        older.failures.insert(Ask::Supply, Failure { status: 404, message: "404 Not Found".to_string() });
        older.failures.insert(Ask::Rich, Failure { status: 404, message: "404 Not Found".to_string() });
        let (text, asked) = super::super::render(&older, &mut Light::default(), 1_200.0, show);

        assert!(text.contains("from before GET /supply") && text.contains("from before GET /rich"), "{text}");
        assert!(asked.is_empty(), "a 404 stands until the page's own poll asks again");
    }
}
