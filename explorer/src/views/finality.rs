//! How far finality has come, who votes, and any cheating the node caught.

use {
    super::{Ctx, FINAL, JUSTIFIED},
    crate::api::Ask,
    crate::format,
    egui::Ui,
    serde_json::Value,
};

pub fn show(ui: &mut Ui, ctx: &mut Ctx) {
    let store = ctx.store;
    let Some(status) = &store.status else {
        super::waiting(ui, ctx, &Ask::Status);

        return;
    };
    let Some(finality) = &status.finality else {
        super::title(ui, "Finality");
        ui.weak("This chain has no finality: the branch with the most work is the main chain, as far back as max_reorg_depth.");

        return;
    };
    let (finalized, justified) = store.finality_heights();

    super::title(ui, &format!("Finality: {}", finality.kind));
    super::facts(ui, "finality", |ui| {
        super::fact(ui, "epoch");
        ui.label(format!("{} (checkpoints every {} blocks)", finality.epoch, finality.checkpoint_interval));
        ui.end_row();
        super::fact(ui, "justified");
        ui.horizontal(|ui| {
            super::badge(ui, &format!("epoch {}", finality.justified.epoch), JUSTIFIED);
            super::block(ui, ctx, &finality.justified.block, &format!("height {justified}"));
        });
        ui.end_row();
        super::fact(ui, "finalized");
        ui.horizontal(|ui| {
            super::badge(ui, &format!("epoch {}", finality.finalized.epoch), FINAL);
            super::block(ui, ctx, &finality.finalized.block, &format!("height {finalized}"));
            ui.weak(format!("{} blocks behind the tip", status.height.saturating_sub(finalized)));
        });
        ui.end_row();
    });

    match finality.kind.as_str() {
        "ffg" | "authority" => voters(ui, ctx),
        _ => {
            ui.weak("Depth finality has no voters: a block is final once enough blocks sit on it.");
        }
    }

    if finality.kind == "authority" {
        ui.add_space(6.0);
        ui.strong("Signers");

        match &store.signers {
            Some(signers) => super::json_tree(ui, ctx, "signers", signers),
            None => super::waiting(ui, ctx, &Ask::Signers),
        }
    }

    ui.add_space(6.0);
    ui.strong("Conflicting votes this node caught");

    match store.evidence.as_ref().and_then(|evidence| evidence.get("evidence")).and_then(Value::as_array) {
        Some(evidence) if evidence.is_empty() => {
            ui.weak("None.");
        }
        Some(evidence) => {
            for (index, item) in evidence.iter().enumerate() {
                super::json_tree(ui, ctx, &format!("evidence {index}"), item);
            }
        }
        None => super::waiting(ui, ctx, &Ask::Evidence),
    }
}

fn voters(ui: &mut Ui, ctx: &mut Ctx) {
    let store = ctx.store;
    let Some(validators) = &store.validators else {
        super::waiting(ui, ctx, &Ask::Validators);

        return;
    };
    let text = |value: &Value, name: &str| value.get(name).map(|value| value.as_str().map_or(value.to_string(), str::to_string)).unwrap_or_default();

    ui.add_space(6.0);
    ui.strong(format!("Voting in epoch {}", text(validators, "epoch")));
    egui::Grid::new("active").num_columns(3).striped(true).show(ui, |ui| {
        for heading in ["voter", "account", "weight"] {
            ui.strong(heading);
        }

        ui.end_row();

        for member in validators.get("active").and_then(Value::as_array).into_iter().flatten() {
            super::copyable(ui, &format::short(&text(member, "voter")));
            super::address(ui, ctx, &text(member, "account"));
            ui.label(ctx.coins(&text(member, "weight")));
            ui.end_row();
        }
    });

    let bonded = validators.get("validators").and_then(Value::as_array).cloned().unwrap_or_default();

    if bonded.is_empty() {
        return;
    }

    ui.add_space(6.0);
    ui.strong(format!("Bonded validators ({})", bonded.len()));
    egui::Grid::new("bonded").num_columns(5).striped(true).show(ui, |ui| {
        for heading in ["voter", "account", "bond", "active", "slashed"] {
            ui.strong(heading);
        }

        ui.end_row();

        for validator in &bonded {
            super::copyable(ui, &format::short(&text(validator, "voter")));
            super::address(ui, ctx, &text(validator, "account"));
            ui.label(ctx.coins(&text(validator, "bond")));
            ui.label(match text(validator, "active_until").as_str() {
                "null" => format!("from epoch {}", text(validator, "active_from")),
                until => format!("epochs {} to {until}", text(validator, "active_from")),
            });
            ui.label(text(validator, "slashed"));
            ui.end_row();
        }
    });
}
