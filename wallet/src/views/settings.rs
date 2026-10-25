//! Which node to talk to, which chain to trust there, the light client's saved progress, and
//! how the wallet keeps its keys: when it locks itself, and the password that seals them.

use {
    super::Ctx,
    crate::format,
    crate::theme,
    crate::vault::Vault,
    chainv4::structs::hex::{from_hex, to_hex},
    egui::{RichText, Ui},
};

#[derive(Default)]
pub struct SettingsState {
    /// The node being typed, and a genesis id to trust instead.
    node: Option<String>,
    genesis: String,
    current: String,
    new: String,
    again: String,
    /// What came of changing the password.
    changed: Option<Result<String, String>>,
}

pub fn show(ui: &mut Ui, ctx: &mut Ctx, state: &mut SettingsState) {
    let palette = *ctx.palette;
    let node = ctx.settings.node.clone();
    let typed = state.node.get_or_insert_with(|| node.clone());

    theme::title(ui, &palette, "Settings", "The node, the chain this wallet trusts on it, and how it keeps your keys.");

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Node");
        ui.label(RichText::new("Where the wallet asks. It is never trusted: what it sends shows only once it checks.").small().color(palette.muted));
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(typed).font(egui::TextStyle::Monospace).desired_width(ui.available_width() - 100.0));

            if theme::primary(ui, &palette, "Connect", typed.trim() != node).clicked() {
                *ctx.connect = Some(typed.trim().to_string());
            }
        });
    });

    ui.add_space(14.0);
    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Trusted chain");
        let pinned = ctx.settings.pinned.get(&node).cloned();

        match &pinned {
            Some(genesis) => {
                ui.label(RichText::new("The genesis this wallet trusts at this node: the one it showed first, unless you gave another.").small().color(palette.muted));
                super::address(ui, &palette, genesis);
            }
            None => {
                ui.label(RichText::new("None yet: the wallet trusts the genesis the node shows first. Paste one below to trust that instead.").small().color(palette.muted));
            }
        }

        if let Some(trouble) = &ctx.chain.trouble {
            ui.add_space(4.0);
            ui.colored_label(palette.bad, trouble);
        }

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut state.genesis).hint_text("a genesis id, 0x…, as someone you trust shows it").font(egui::TextStyle::Monospace).desired_width(ui.available_width() - 90.0));

            if theme::quiet(ui, &palette, "Trust").clicked() {
                match from_hex(state.genesis.trim()) {
                    Some(bytes) if bytes.len() >= 16 => {
                        ctx.settings.pinned.insert(node.clone(), to_hex(&bytes));
                        state.genesis.clear();
                        *ctx.connect = Some(node.clone());
                    }
                    _ => state.genesis = "not a genesis id".to_string(),
                }
            }
        });

        if pinned.is_some() && theme::quiet(ui, &palette, "Forget it").on_hover_text("Trust whatever genesis the node shows next, and start the light client over").clicked() {
            if let Some(genesis) = ctx.settings.pinned.remove(&node) {
                ctx.settings.light.remove(&genesis);
            }

            *ctx.connect = Some(node.clone());
        }
    });

    ui.add_space(14.0);
    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Light client");

        match ctx.chain.following() {
            Some((tip, final_height)) => {
                super::row(ui, &palette, "Followed to", |ui| {
                    ui.label(format::thousands(&tip.to_string()));
                });
                super::row(ui, &palette, "Final to", |ui| {
                    ui.label(format::thousands(&final_height.to_string()));
                });
            }
            None => super::waiting(ui, &palette, "Starting…"),
        }

        let saved = ctx.chain.genesis.and_then(|genesis| ctx.settings.light.get(&genesis.to_string())).map_or(0, String::len) / 2;
        super::row(ui, &palette, "Saved", |ui| {
            ui.label(format!("{} KiB, to resume from", format::thousands(&(saved / 1024).to_string())));
        });
        ui.add_space(6.0);

        if theme::quiet(ui, &palette, "Start over").on_hover_text("Forget what it checked and follow the chain again from genesis").clicked() {
            if let Some(genesis) = ctx.chain.genesis {
                ctx.settings.light.remove(&genesis.to_string());
            }

            *ctx.connect = Some(node.clone());
        }
    });

    ui.add_space(14.0);
    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Locking");
        ui.horizontal(|ui| {
            ui.label(RichText::new("Lock after").color(palette.soft));
            ui.add(egui::DragValue::new(&mut ctx.settings.lock_after).range(0..=240).suffix(" min"));
            ui.label(RichText::new(if ctx.settings.lock_after == 0 { "never, until you lock it" } else { "without input" }).small().color(palette.muted));
        });

        ui.add_space(10.0);
        theme::caps(ui, &palette, "Password");

        for (field, hint) in [(&mut state.current, "current password"), (&mut state.new, "new password"), (&mut state.again, "the new one again")] {
            ui.add(egui::TextEdit::singleline(field).password(true).hint_text(hint).desired_width(f32::INFINITY));
        }

        if theme::quiet(ui, &palette, "Change password").clicked() {
            state.changed = Some(change_password(ctx, state));
        }

        match &state.changed {
            Some(Ok(done)) => {
                ui.colored_label(palette.good, done);
            }
            Some(Err(why)) => {
                ui.colored_label(palette.bad, why);
            }
            None => {}
        }
    });
}

/// Seals the accounts with a new password, once the current one opens them.
fn change_password(ctx: &mut Ctx, state: &mut SettingsState) -> Result<String, String> {
    let sealed = ctx.settings.vault.as_ref().ok_or("there is no vault")?;
    Vault::open(sealed, &state.current).map_err(|_| "the current password is not right".to_string())?;

    if state.new.len() < 8 {
        return Err("use at least 8 characters".to_string());
    }

    if state.new != state.again {
        return Err("the two new ones do not match".to_string());
    }

    *ctx.vault = Some(Vault::create(&state.new)?);
    *ctx.changed = true;
    state.current.clear();
    state.new.clear();
    state.again.clear();

    Ok("Sealed with the new password.".to_string())
}
