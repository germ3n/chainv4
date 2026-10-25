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
                    ui.horizontal(|ui| {
                        super::address(ui, ctx, &text(member, "account"));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(RichText::new(ctx.coins(&text(member, "weight"))).monospace());
                        });
                    });
                    ui.label(RichText::new(format!("votes as {}", format::short(&text(member, "voter")))).small().color(look.palette.muted));
                });
            }

            return;
        }

        super::table(ui, look, "active", &["voter", "account", "weight"], |ui| {
            for member in &active {
                super::copyable(ui, &format::short(&text(member, "voter")));
                super::address(ui, ctx, &text(member, "account"));
                ui.monospace(ctx.coins(&text(member, "weight")));
                ui.end_row();
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
                    ui.horizontal(|ui| {
                        super::address(ui, ctx, &text(validator, "account"));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(RichText::new(ctx.coins(&text(validator, "bond"))).monospace());
                        });
                    });
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
                super::copyable(ui, &format::short(&text(validator, "voter")));
                super::address(ui, ctx, &text(validator, "account"));
                ui.monospace(ctx.coins(&text(validator, "bond")));
                ui.label(active_text(validator));
                ui.label(slashed(validator));
                ui.end_row();
            }
        });
    });
}
