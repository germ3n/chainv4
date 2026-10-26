//! An offline signer's page: a request read off the sender's screen, all it asks laid out to check,
//! and once signed, the signed transaction as a code for the sender to read back. Nothing here
//! talks to a node: the request says the nonce and fee, and the sender proves the rest. With
//! biometrics on, signing waits for one more check.

use {
    super::Ctx,
    super::scan::Scan,
    crate::airgap::{self, Message, Request},
    crate::biometrics::{self, Check},
    crate::theme,
    chainv4_ui::biometric::Failure,
    egui::{RichText, Ui},
};

#[derive(Default)]
pub struct SignState {
    scan: Scan,
    step: Step,
    /// The biometric check before signing, while it is under way.
    confirming: Option<Check>,
}

#[derive(Default)]
enum Step {
    #[default]
    Reading,
    /// Read, to check before signing; why it cannot be signed, if it cannot.
    Review { request: Box<Request>, trouble: Option<String> },
    /// Signed: the code for the sender.
    Signed { text: String },
}

pub fn show(ui: &mut Ui, ctx: &mut Ctx, state: &mut SignState) {
    let palette = *ctx.palette;
    theme::title(ui, &palette, "Sign", "A request from the watch-only sender: read it, check it, sign it, and show it back.");

    match &state.step {
        Step::Reading => reading(ui, ctx, state),
        Step::Review { .. } => review(ui, ctx, state),
        Step::Signed { text } => {
            let text = text.clone();
            theme::card(ui, &palette, |ui| {
                theme::caps(ui, &palette, "Signed");
                ui.label(RichText::new("Show this to the sender: it checks this is what it asked for, and sends it.").color(palette.soft));
                ui.add_space(10.0);
                super::code(ui, &palette, &text);
                ui.add_space(12.0);

                if theme::primary(ui, &palette, "Sign another", true).clicked() {
                    state.step = Step::Reading;
                }
            });
        }
    }
}

fn reading(ui: &mut Ui, ctx: &mut Ctx, state: &mut SignState) {
    let palette = *ctx.palette;

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Read the request");
        ui.label(RichText::new("On the sender, Send shows a code once it is priced. Read it here.").color(palette.soft));
        ui.add_space(10.0);

        match state.scan.show(ui, &palette) {
            Some(Message::Sign(request)) => {
                let trouble = match ctx.holders.iter().find(|holder| holder.publics.contains(&request.signer)) {
                    Some(holder) if holder.keys.is_empty() => Some("This wallet holds that account's public keys alone.".to_string()),
                    Some(_) => None,
                    None => Some(format!("No account in this wallet signs for {}: pair the sender with this wallet's accounts.", request.signer.address())),
                };
                state.step = Step::Review { request: Box::new(request), trouble };
            }
            Some(other) => state.scan.refuse(&other, "a request to sign"),
            None => {}
        }
    });
}

fn review(ui: &mut Ui, ctx: &mut Ctx, state: &mut SignState) {
    let palette = *ctx.palette;
    let Step::Review { request, trouble } = &mut state.step else {
        return;
    };
    let described = airgap::describe(request);
    let mut next = None;

    // What the check before signing came to: passed, it signs; the fingers or faces enrolled
    // changed since, the wallet locks, for its password.
    let mut confirmed = false;

    if let Some(came) = state.confirming.as_ref().and_then(Check::take) {
        state.confirming = None;

        match came {
            Ok(_) => confirmed = true,
            Err(Failure::Canceled) => {}
            Err(Failure::Invalidated) => {
                biometrics::turn_off(ctx.wake, ctx.settings);
                *ctx.lock = Some(biometrics::CHANGED.to_string());
            }
            Err(Failure::Failed(why)) => *trouble = Some(why),
        }
    }

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, &described.kind);
        ui.add_space(4.0);

        if let Some((shown, raw)) = &described.what {
            theme::fitted(ui, shown, 34.0, 16.0, egui::FontFamily::Name(theme::SERIF.into()), palette.ink);
            ui.label(RichText::new(format!("{raw} units, signed as such")).small().color(palette.muted));
            ui.add_space(8.0);
        }

        super::row(ui, &palette, "From", |ui| super::address(ui, &palette, &described.from));

        if let Some(to) = &described.to {
            super::row(ui, &palette, "To", |ui| super::address(ui, &palette, to));
        }

        if let Some(token) = &described.token {
            super::row(ui, &palette, "Token", |ui| super::address(ui, &palette, token));
        }

        super::row(ui, &palette, "Fee, at most", |ui| {
            ui.label(&described.fee).on_hover_text(&described.gas);
        });
        super::row(ui, &palette, "Nonce", |ui| {
            ui.label(described.nonce.to_string());
        });
        ui.add_space(6.0);
        ui.label(
            RichText::new("Check each against what you meant to send: the chain runs what is signed here. Names and decimals are the sender's word; the units and addresses are what is signed.")
                .small()
                .color(palette.warn),
        );
        egui::CollapsingHeader::new(RichText::new("Everything it holds").color(palette.soft)).id_salt("everything").show(ui, |ui| {
            ui.add(egui::Label::new(theme::mono(&described.everything).color(palette.soft)).wrap());
        });

        if let Some(trouble) = trouble.as_ref() {
            ui.add_space(6.0);
            ui.colored_label(palette.bad, trouble.as_str());
        }

        ui.add_space(12.0);
        ui.horizontal(|ui| {
            let tapped = theme::primary(ui, &palette, "Sign", trouble.is_none() && state.confirming.is_none()).clicked();
            let biometric = biometrics::enrolled(ctx.wake, ctx.settings);

            if tapped && !confirmed && let Some((enrolled, _)) = biometric {
                state.confirming = Some(Check::start(ctx.wake, &enrolled, "Sign this transaction"));
            } else if tapped || confirmed {
                let signed = ctx
                    .holders
                    .iter()
                    .find_map(|holder| holder.keys.iter().find(|key| key.public() == request.signer))
                    .ok_or_else(|| "the key is gone from this wallet".to_string())
                    .and_then(|key| airgap::sign(request, key));

                match signed {
                    Ok(signed) => next = Some(Step::Signed { text: Message::Signed(signed).to_text() }),
                    Err(why) => *trouble = Some(why),
                }
            }

            if theme::quiet(ui, &palette, "Cancel").clicked() {
                next = Some(Step::Reading);
                state.confirming = None;
            }
        });
    });

    if let Some(next) = next {
        state.step = next;
    }
}
