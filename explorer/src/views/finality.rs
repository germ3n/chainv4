//! How far finality has come, who votes, and any cheating the node caught.

use {
    super::Ctx,
    crate::api::Ask,
    crate::format,
    chainv4_ui::widgets,
    egui::{RichText, Ui},
    serde_json::Value,
};

pub fn show(ui: &mut Ui, ctx: &mut Ctx) {
    let look = ctx.look;
    let store = ctx.store;
    let Some(status) = &store.status else {
        super::heading(ui, look, "Finality", "", |_| {});
        super::waiting(ui, ctx, &Ask::Status);

        return;
    };
    let Some(finality) = &status.finality else {
        super::heading(ui, look, "Finality", "", |_| {});
        super::note(ui, look, "This chain has no finality: the branch with the most work is the main chain, as far back as max_reorg_depth.");

        return;
    };
    let (finalized, justified) = store.finality_heights();

    super::heading(ui, look, "Finality", &format!("{} finality, checkpoints every {} blocks", finality.kind, finality.checkpoint_interval), |ui| {
        widgets::pill(ui, &format!("epoch {}", finality.epoch), look.palette.soft);
    });

    super::stats(ui, look, &[
        (
            "Justified".to_string(),
            format::thousands(&justified.to_string()),
            format!("epoch {}", finality.justified.epoch),
        ),
        (
            "Finalized".to_string(),
            format::thousands(&finalized.to_string()),
            format!("epoch {} · {} behind the tip", finality.finalized.epoch, status.height.saturating_sub(finalized)),
        ),
    ]);

    super::section(ui, look, "Checkpoints", |ui| {
        super::facts(ui, look, "finality", |facts| {
            facts.row("justified", |ui| {
                widgets::pill(ui, &format!("epoch {}", finality.justified.epoch), look.roles.justified);
                super::block(ui, ctx, &finality.justified.block, &format::short(&finality.justified.block));
            });
            facts.row("finalized", |ui| {
                widgets::pill(ui, &format!("epoch {}", finality.finalized.epoch), look.roles.final_);
                super::block(ui, ctx, &finality.finalized.block, &format::short(&finality.finalized.block));
            });
        });
    });

    // What the light client checked is above: who votes, and who was caught, the node lists.
    if ctx.settings.read_blocks {
        super::reading::turned_off_card(ui, ctx, "Who votes, the signers and any cheating caught are listed by the node");

        return;
    }

    match finality.kind.as_str() {
        "ffg" | "authority" => voters(ui, ctx),
        _ => super::section(ui, look, "Voters", |ui| {
            super::note(ui, look, "Depth finality has no voters: a block is final once enough blocks sit on it.");
        }),
    }

    if finality.kind == "authority" {
        super::section(ui, look, "Signers", |ui| match &store.signers {
            Some(signers) => super::json_tree(ui, ctx, "signers", signers),
            None => super::waiting(ui, ctx, &Ask::Signers),
        });
    }

    anchors(ui, ctx);

    super::section(ui, look, "Conflicting votes this node caught", |ui| {
        match store.evidence.as_ref().and_then(|evidence| evidence.get("evidence")).and_then(Value::as_array) {
            Some(evidence) if evidence.is_empty() => super::note(ui, look, "None."),
            Some(evidence) => {
                for (index, item) in evidence.iter().enumerate() {
                    super::json_tree(ui, ctx, &format!("evidence {index}"), item);
                }
            }
            None => super::waiting(ui, ctx, &Ask::Evidence),
        }
    });
}

/// What the chain anchored in Bitcoin and Ethereum: which it takes, whether it enforces anchors,
/// how far each relay reaches, and the newest anchors, each with the block of its chain that holds
/// it.
fn anchors(ui: &mut Ui, ctx: &mut Ctx) {
    let look = ctx.look;
    let store = ctx.store;
    let Some(anchors) = &store.anchors else {
        super::section(ui, look, "Anchored", |ui| super::waiting(ui, ctx, &Ask::Anchors));

        return;
    };
    let settings = &anchors["settings"];
    let on = |name: &str| settings[name].as_bool() == Some(true);
    let entries = anchors["entries"].as_array().cloned().unwrap_or_default();
    let count = anchors["count"].as_u64().unwrap_or(0);

    super::section(ui, look, &format!("Anchored · {}", format::thousands(&count.to_string())), |ui| {
        if !on("bitcoin") && !on("ethereum") && count == 0 {
            super::note(ui, look, "This chain takes no anchors, in Bitcoin or Ethereum: governance turns them on.");

            return;
        }

        super::facts(ui, look, "anchoring", |facts| {
            facts.row("taking anchors", |ui| {
                ui.horizontal(|ui| {
                    if on("bitcoin") {
                        widgets::pill(ui, "Bitcoin", look.palette.soft);
                    }
                    if on("ethereum") {
                        widgets::pill(ui, "Ethereum", look.palette.soft);
                    }
                    if !on("bitcoin") && !on("ethereum") {
                        widgets::pill(ui, "off", look.palette.soft);
                    }
                });
            });
            facts.row("enforced", |ui| {
                ui.label(if on("enforce") { "yes: no block replaces an anchored checkpoint" } else { "no" });
            });
            facts.row("deep", |ui| {
                ui.label(format!("{} Bitcoin blocks", settings["depth"].as_u64().unwrap_or(0)));
            });
            if let Some(height) = anchors["enforced_height"].as_u64() {
                facts.row("held up to", |ui| {
                    ui.label(format!("block {}", format::thousands(&height.to_string())));
                });
            }
            if let Some(tip) = anchors["relay_tip"]["height"].as_u64() {
                facts.row("Bitcoin relay's tip", |ui| {
                    ui.label(format!("Bitcoin block {}", format::thousands(&tip.to_string())));
                });
            }
            if let Some(slot) = anchors["eth_finalized"]["slot"].as_u64() {
                facts.row("Ethereum relay's finalized", |ui| {
                    ui.label(format!("slot {}", format::thousands(&slot.to_string())));
                });
            }
        });

        for entry in entries.iter().rev() {
            super::list_row(ui, look, |ui| {
                let epoch = entry["epoch"].as_u64().unwrap_or(0);
                let block = entry["block"].as_str().unwrap_or_default().to_string();
                let ethereum = entry["chain"].as_str() == Some("ethereum");
                widgets::end_row(
                    ui,
                    |ui| {
                        widgets::pill(ui, &format!("epoch {epoch}"), if entry["enforced"].as_bool() == Some(true) { look.roles.final_ } else { look.palette.soft });
                        super::block(ui, ctx, &block, &format::short(&block));
                    },
                    |ui| {
                        ui.label(RichText::new(match ethereum {
                            true => format!("Ethereum slot {}", format::thousands(&entry["eth_slot"].as_u64().unwrap_or(0).to_string())),
                            false => format!("Bitcoin {}", format::thousands(&entry["btc_height"].as_u64().unwrap_or(0).to_string())),
                        }).monospace());
                    },
                );
                let held = entry[if ethereum { "eth_root" } else { "btc_block" }].as_str().unwrap_or_default();
                super::copyable_as(ui, RichText::new(format::short(held)).small().color(look.palette.muted), held);
            });
        }
    });
}

fn voters(ui: &mut Ui, ctx: &mut Ctx) {
    let look = ctx.look;
    let store = ctx.store;
    let Some(validators) = &store.validators else {
        super::section(ui, look, "Voters", |ui| super::waiting(ui, ctx, &Ask::Validators));

        return;
    };
    let text = |value: &Value, name: &str| value.get(name).map(|value| value.as_str().map_or(value.to_string(), str::to_string)).unwrap_or_default();
    let active: Vec<Value> = validators.get("active").and_then(Value::as_array).cloned().unwrap_or_default();

    super::section(ui, look, &format!("Voting in epoch {}", text(validators, "epoch")), |ui| {
        if look.narrow {
            for member in &active {
                super::list_row(ui, look, |ui| {
                    let weight = ctx.coins(&text(member, "weight"));
                    widgets::end_row(ui, |ui| super::address(ui, ctx, &text(member, "account")), |ui| ui.label(RichText::new(weight).monospace()));
                    ui.label(RichText::new(format!("votes as {}", format::short(&text(member, "voter")))).small().color(look.palette.muted));
                });
            }

            return;
        }

        super::table(ui, look, "active", &["voter", "account", "weight"], |ui| {
            for member in &active {
                let voter = text(member, "voter");
                super::copyable_as(ui, RichText::new(format::short(&voter)).monospace(), &voter);
                super::address(ui, ctx, &text(member, "account"));
                ui.monospace(ctx.coins(&text(member, "weight")));
                super::row_end(ui);
            }
        });
    });

    let bonded = validators.get("validators").and_then(Value::as_array).cloned().unwrap_or_default();

    if bonded.is_empty() {
        return;
    }

    let slashed = |validator: &Value| match text(validator, "slashed").as_str() {
        "false" => "not slashed".to_string(),
        "true" => "slashed".to_string(),
        other => format!("slashed {other}"),
    };
    let active_text = |validator: &Value| match text(validator, "active_until").as_str() {
        "null" => format!("from epoch {}", text(validator, "active_from")),
        until => format!("epochs {} to {until}", text(validator, "active_from")),
    };

    super::section(ui, look, &format!("Bonded validators · {}", bonded.len()), |ui| {
        if look.narrow {
            for validator in &bonded {
                super::list_row(ui, look, |ui| {
                    let bond = ctx.coins(&text(validator, "bond"));
                    widgets::end_row(ui, |ui| super::address(ui, ctx, &text(validator, "account")), |ui| ui.label(RichText::new(bond).monospace()));
                    ui.label(
                        RichText::new(format!("{} · {}", active_text(validator), slashed(validator)))
                            .small()
                            .color(look.palette.muted),
                    );
                });
            }

            return;
        }

        super::table(ui, look, "bonded", &["voter", "account", "bond", "active", "slashed"], |ui| {
            for validator in &bonded {
                let voter = text(validator, "voter");
                super::copyable_as(ui, RichText::new(format::short(&voter)).monospace(), &voter);
                super::address(ui, ctx, &text(validator, "account"));
                ui.monospace(ctx.coins(&text(validator, "bond")));
                ui.label(active_text(validator));
                ui.label(slashed(validator));
                super::row_end(ui);
            }
        });
    });
}
