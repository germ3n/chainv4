//! A multisig, looked up by its address: its members, each with its keys on this chain and the
//! others it reaches, how many of them a transaction needs, its next nonce and balance, and the
//! transactions proposed for that nonce with the members that approved each. As the node says it.

use {
    super::{Ctx, Look, View},
    crate::api::Ask,
    crate::format,
    crate::model::{MultisigProposalView, MultisigView},
    chainv4::crypto::keypair::PublicKey,
    chainv4::structs::address::Address,
    chainv4_ui::widgets,
    egui::{RichText, Ui},
    serde_json::Value,
};

const ABOUT: &str = "An account that M of its N members' keys control: its members, how many must sign, and what is proposed for it.";

#[derive(Default)]
pub struct MultisigState {
    /// The address typed into the lookup.
    typed: String,
    trouble: Option<String>,
}

/// The multisig an address typed names, as the node writes it; why not for anything else.
pub fn multisig_address(typed: &str) -> Result<String, String> {
    match typed.trim().parse::<Address>() {
        Ok(address @ Address::Multisig(_)) => Ok(address.to_string()),
        Ok(other) => Err(format!("{other} is a {} address, not a multisig's: those are multisig:0x…", other.kind())),
        Err(_) => Err("Not an address: a multisig's is multisig:0x and 64 hex digits".to_string()),
    }
}

/// The account a member's key controls, as the node writes addresses; None for a key that does
/// not read.
pub fn member_address(key: &str) -> Option<String> {
    key.parse::<PublicKey>().ok().map(|key| key.address().to_string())
}

/// How many of the members a proposal's approvals come to: approvals by keys that are members,
/// each once.
pub fn approving(multisig: &MultisigView, proposal: &MultisigProposalView) -> usize {
    let mut counted: Vec<&String> = proposal.approvals.iter().filter(|key| multisig.members.iter().any(|member| member.key == **key)).collect();
    counted.sort();
    counted.dedup();

    counted.len()
}

pub fn show(ui: &mut Ui, ctx: &mut Ctx, state: &mut MultisigState, address: &str) {
    let look = ctx.look;
    super::heading(ui, look, "Multisig", ABOUT, |_| {});
    lookup(ui, ctx, state, address);

    if address.is_empty() {
        super::note(ui, look, "Look a multisig up by its address, multisig:0x…: an account page of one links here too.");

        return;
    }

    let store = ctx.store;
    let ask = Ask::Multisig(address.to_string());
    let Some(multisig) = store.multisigs.get(address) else {
        match store.failures.get(&ask) {
            Some(failure) if failure.status == 404 => super::note(ui, look, format!("{address} is not a multisig on this chain.")),
            Some(_) => super::waiting(ui, ctx, &ask),
            None => {
                ctx.api.ask(ask.clone());
                super::waiting(ui, ctx, &ask);
            }
        }

        return;
    };

    ui.horizontal_wrapped(|ui| {
        super::address(ui, ctx, address);
        widgets::pill(ui, &format!("{} of {}", multisig.threshold, multisig.members.len()), look.roles.main);

        if multisig.testnet {
            widgets::pill(ui, "Bitcoin testnet", look.palette.soft);
        }
    });
    ui.add_space(8.0);

    super::stats(
        ui,
        look,
        &[
            ("threshold".to_string(), multisig.threshold.to_string(), "signatures a transaction needs".to_string()),
            ("members".to_string(), multisig.members.len().to_string(), "keys that may sign".to_string()),
            ("nonce".to_string(), format::thousands(&multisig.nonce.to_string()), "the next transaction's".to_string()),
            ("balance".to_string(), ctx.coins(&multisig.balance), "held by the multisig".to_string()),
        ],
    );

    super::section(ui, look, "Members", |ui| members(ui, ctx, multisig));
    super::section_with(
        ui,
        look,
        "Open proposals",
        |ui| {
            ui.label(RichText::new(multisig.proposals.len().to_string()).monospace().color(look.palette.muted));
        },
        |ui| proposals(ui, ctx, multisig),
    );

    if !multisig.bitcoin.is_null() || !multisig.evm.is_null() {
        super::section(ui, look, "On other chains", |ui| elsewhere(ui, ctx, multisig));
    }
}

/// The lookup: an address, and Look up.
fn lookup(ui: &mut Ui, ctx: &mut Ctx, state: &mut MultisigState, address: &str) {
    let palette = ctx.look.palette;

    if state.typed.is_empty() && !address.is_empty() {
        state.typed = address.to_string();
    }

    let typed = !state.typed.trim().is_empty();
    let pressed = widgets::beside(
        ui,
        |ui| widgets::primary(ui, &palette, "Look up", typed).clicked(),
        |ui, width| {
            ui.add(egui::TextEdit::singleline(&mut state.typed).hint_text("A multisig's address: multisig:0x…").desired_width(width));
        },
    );

    if pressed {
        match multisig_address(&state.typed) {
            Ok(found) => {
                state.trouble = None;
                ctx.go(View::Multisig(found));
            }
            Err(why) => state.trouble = Some(why),
        }
    }

    if let Some(trouble) = &state.trouble {
        ui.colored_label(palette.bad, trouble.as_str());
    }

    ui.add_space(8.0);
}

/// Each member: its key, the account it controls, and its keys on Bitcoin and EVM chains.
fn members(ui: &mut Ui, ctx: &mut Ctx, multisig: &MultisigView) {
    let look = ctx.look;

    for (index, member) in multisig.members.iter().enumerate() {
        super::list_row(ui, look, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(format!("#{}", index + 1)).monospace().color(look.palette.muted));

                match member_address(&member.key) {
                    Some(account) => super::address(ui, ctx, &account),
                    None => {
                        ui.label(&member.key);
                    }
                }
            });
            super::facts(ui, look, &format!("member {index}"), |facts| {
                facts.row("key", |ui| super::copyable(ui, &member.key));

                if let Some(bitcoin) = &member.bitcoin {
                    facts.row("Bitcoin xpub", |ui| super::copyable(ui, bitcoin));
                }

                if let Some(evm) = &member.evm {
                    facts.row("EVM", |ui| super::copyable(ui, evm));
                }
            });
        });
    }
}

/// The proposals open for the next nonce, each with its approvals against the threshold.
fn proposals(ui: &mut Ui, ctx: &mut Ctx, multisig: &MultisigView) {
    let look = ctx.look;

    if multisig.proposals.is_empty() {
        super::note(ui, look, format!("Nothing proposed for nonce {}: members may still sign a transaction off chain and submit it.", multisig.nonce));

        return;
    }

    super::note(ui, look, format!("All for nonce {}: once any of the multisig's transactions runs, they are void.", multisig.nonce));

    for proposal in &multisig.proposals {
        let approved = approving(multisig, proposal);
        let kind = proposal.tx.get("type").and_then(Value::as_str).unwrap_or("?").to_string();

        super::list_row(ui, look, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(widgets::strong(&kind, 13.5).color(look.palette.ink));
                super::copyable(ui, &proposal.id);
            });
            super::meter(
                ui,
                look,
                approved as f32 / multisig.threshold.max(1) as f32,
                &format!("{approved} of {} approvals needed", multisig.threshold),
            );
            super::facts(ui, look, &format!("proposal {}", proposal.id), |facts| {
                facts.text(
                    "offer",
                    format!(
                        "up to {} gas at up to {} units, tip {}",
                        format::thousands(&proposal.fee.gas_limit.to_string()),
                        format::thousands(&proposal.fee.max_price),
                        format::thousands(&proposal.fee.tip)
                    ),
                );
                facts.row("approved by", |ui| {
                    ui.vertical(|ui| {
                        if proposal.approvals.is_empty() {
                            ui.label(RichText::new("nobody yet").color(look.palette.muted));
                        }

                        for key in &proposal.approvals {
                            match member_address(key) {
                                Some(account) => super::address(ui, ctx, &account),
                                None => {
                                    ui.label(key);
                                }
                            }
                        }
                    });
                });
            });
            super::json_tree(ui, ctx, "what it does", &proposal.tx);
        });
    }
}

/// Its matching multisigs on Bitcoin and EVM chains, where its members name keys there.
fn elsewhere(ui: &mut Ui, ctx: &mut Ctx, multisig: &MultisigView) {
    let look: Look = ctx.look;

    super::facts(ui, look, "elsewhere", |facts| {
        let bitcoin = &multisig.bitcoin;

        if let Some(address) = bitcoin["first_address"].as_str() {
            facts.row("Bitcoin", |ui| {
                super::copyable(ui, address);
                ui.label(RichText::new(format!("first receiving address, {}", bitcoin["network"].as_str().unwrap_or("Bitcoin"))).small().color(look.palette.muted));
            });
        }

        if let Some(receiving) = bitcoin["receiving"].as_str() {
            facts.row("descriptor", |ui| super::copyable(ui, receiving));
        }

        let evm = &multisig.evm;

        if let Some(contract) = evm["contract"].as_str() {
            facts.row("EVM contract", |ui| super::copyable(ui, contract));
        }

        if let Some(owners) = evm["now"]["owners"].as_array().or_else(|| evm["start"]["owners"].as_array()) {
            let threshold = evm["now"]["threshold"].as_u64().or_else(|| evm["start"]["threshold"].as_u64()).unwrap_or_default();
            facts.text("EVM owners", format!("{threshold} of {}", owners.len()));
        }
    });
}

#[cfg(test)]
mod tests {
    use {super::*, crate::api::Failure, crate::light::Light, crate::store::Store, chainv4::crypto::keypair::Keypair, serde_json::json};

    fn multisig(address: &str) -> MultisigView {
        let member = Keypair::development().public().to_string();

        serde_json::from_value(json!({
            "address": address, "threshold": 2, "testnet": false, "nonce": 4, "balance": "900000000000000000000",
            "bitcoin": null,
            "evm": { "contract": "0x0505050505050505050505050505050505050505", "start": { "owners": ["0x01", "0x02"], "threshold": 2 }, "now": null },
            "members": [{ "key": member, "bitcoin": null, "evm": null }, { "key": "ed25519:0x0202020202020202020202020202020202020202020202020202020202020202", "bitcoin": null, "evm": "0x02" }],
            "proposals": [{
                "id": "0x0606", "tx": { "type": "transfer", "from": address, "to": "ed25519:0x0303030303030303030303030303030303030303030303030303030303030303", "amount": "10" },
                "encoded": "0x00", "fee": { "gas_limit": 21000, "max_price": "1", "tip": "0" }, "approvals": [member, member, "ed25519:0x09"],
            }],
        }))
        .unwrap()
    }

    const ADDRESS: &str = "multisig:0x0101010101010101010101010101010101010101010101010101010101010101";

    #[test]
    fn a_lookup_takes_a_multisig_s_address_alone() {
        assert_eq!(multisig_address(&format!("  {ADDRESS} ")), Ok(ADDRESS.to_string()));
        assert!(multisig_address("ed25519:0x0101010101010101010101010101010101010101010101010101010101010101").unwrap_err().contains("not a multisig's"));
        assert!(multisig_address("hello").unwrap_err().starts_with("Not an address"));
    }

    #[test]
    fn approvals_count_members_once_each() {
        let found = multisig(ADDRESS);

        assert_eq!(approving(&found, &found.proposals[0]), 1, "the same member twice, and a stranger");
        assert_eq!(member_address(&found.members[0].key), Some(Keypair::development().public().address().to_string()));
    }

    #[test]
    fn a_multisig_shows_its_members_threshold_and_proposals_on_any_screen() {
        let mut store = Store::default();
        store.multisigs.insert(ADDRESS.to_string(), multisig(ADDRESS));

        for width in [1_200.0, 380.0] {
            let (text, _) = crate::views::render(&store, &mut Light::default(), width, |ui, ctx| show(ui, ctx, &mut MultisigState::default(), ADDRESS));

            for wanted in ["Multisig", "2 of 2", "MEMBERS", "OPEN PROPOSALS", "transfer", "1 of 2 approvals needed", "All for nonce 4", "EVM owners", "2 of 2"] {
                assert!(text.contains(wanted), "{wanted} at {width} wide, in {text}");
            }
        }
    }

    #[test]
    fn an_address_that_is_no_multisig_says_so_and_the_lookup_alone_asks_nothing() {
        let mut store = Store::default();
        store.failures.insert(Ask::Multisig(ADDRESS.to_string()), Failure { status: 404, message: "not a multisig".to_string() });
        let (text, asked) = crate::views::render(&store, &mut Light::default(), 380.0, |ui, ctx| show(ui, ctx, &mut MultisigState::default(), ADDRESS));

        assert!(text.contains("is not a multisig on this chain"), "{text}");
        assert!(asked.is_empty());

        let (text, asked) = crate::views::render(&Store::default(), &mut Light::default(), 1_200.0, |ui, ctx| show(ui, ctx, &mut MultisigState::default(), ""));
        assert!(text.contains("Look a multisig up"), "{text}");
        assert!(asked.is_empty());
    }
}
