//! Settings, WalletConnect: the project sites are reached under and the relay, pairing with a
//! site by its code, scanned or pasted, and the sessions there are, each with Disconnect.

use {
    super::Ctx,
    crate::theme,
    egui::{RichText, Ui},
};

#[derive(Default)]
pub struct WalletConnectForm {
    scan: super::scan::Scan,
    pairing: bool,
    trouble: Option<String>,
    /// Said once a code was taken, until the site's proposal shows.
    paired: bool,
}

pub fn card(ui: &mut Ui, ctx: &mut Ctx, form: &mut WalletConnectForm) {
    let palette = *ctx.palette;

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "WalletConnect");
        ui.label(
            RichText::new("Sites on other devices, or in a browser without the extension, connect by WalletConnect: the site shows a code, the wallet reads it, and the two talk through a relay that sees nothing of what they say. What a site asks waits here, as the extension's does.")
                .color(palette.soft),
        );
        ui.add_space(8.0);
        ui.label(RichText::new("Project id, from cloud.reown.com: empty keeps WalletConnect off").small().color(palette.muted));
        ui.add(egui::TextEdit::singleline(&mut ctx.settings.wc_project).hint_text("a1b2c3…").font(egui::TextStyle::Monospace).desired_width(f32::INFINITY));
        ui.label(RichText::new("Relay: empty for Reown's, wss://relay.walletconnect.org. It learns which topics this wallet listens to, and when, but nothing said in them.").small().color(palette.muted));
        ui.add(egui::TextEdit::singleline(&mut ctx.settings.wc_relay).hint_text(crate::walletconnect::RELAY).font(egui::TextStyle::Monospace).desired_width(f32::INFINITY));

        let Some(running) = ctx.walletconnect else {
            if !ctx.settings.wc_project.trim().is_empty() {
                ui.label(RichText::new("It starts once the wallet follows a chain.").small().color(palette.muted));
            }

            return;
        };
        let status = running.status();
        ui.add_space(6.0);
        ui.label(RichText::new(match (&status.connected, &status.trouble) {
            (true, None) => "Connected to the relay".to_string(),
            (_, Some(why)) => format!("The relay: {why}"),
            (false, None) => "Connecting to the relay…".to_string(),
        }).small().color(if status.trouble.is_some() { palette.warn } else { palette.muted }));

        ui.add_space(8.0);

        if !form.pairing {
            if theme::primary(ui, &palette, "Connect a site", true).clicked() {
                (form.pairing, form.trouble, form.paired) = (true, None, false);
            }
        } else {
            ui.label(RichText::new("Read the site's WalletConnect code, or paste it: wc:…").color(palette.soft));
            form.scan.show(ui, &palette);

            if let Some(code) = form.scan.take_plain() {
                match running.pair(&code) {
                    Ok(()) => (form.pairing, form.paired) = (false, true),
                    Err(why) => form.trouble = Some(why),
                }
            }

            if theme::quiet(ui, &palette, "Cancel").clicked() {
                form.pairing = false;
            }
        }

        if form.paired {
            ui.label(RichText::new("Paired: the site's request to connect shows once the relay brings it.").small().color(palette.muted));
        }

        if let Some(why) = &form.trouble {
            ui.colored_label(palette.bad, why.as_str());
        }

        let sessions = running.sessions();

        if sessions.is_empty() {
            return;
        }

        ui.add_space(8.0);
        theme::caps(ui, &palette, "Connected by WalletConnect");

        for session in sessions {
            theme::end_row(
                ui,
                |ui| {
                    ui.vertical(|ui| {
                        ui.label(RichText::new(if session.peer.name.is_empty() { session.peer.url.as_str() } else { session.peer.name.as_str() }).color(palette.ink));
                        ui.label(RichText::new(format!("chain {} · {} account{} · says it is {}", session.chain, session.accounts.len(), if session.accounts.len() == 1 { "" } else { "s" }, session.peer.url)).small().color(palette.muted));
                    });
                },
                |ui| {
                    if theme::quiet(ui, &palette, "Disconnect").clicked() {
                        running.disconnect(&session.topic);
                    }
                },
            );
        }
    });
}
