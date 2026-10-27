//! The node's faucet, on a node that runs one, as a dev or test chain's may: coins for an address
//! or a name, once per address and connection each period. The faucet wants a moment's proof of
//! work with each request, which the page works out a slice a frame, then sends.

use {
    super::Ctx,
    crate::api::Ask,
    chainv4::faucet::{faucet_window, solve_faucet},
    chainv4::structs::address::Address,
    chainv4::structs::hash::{Hash, HashType},
    chainv4::structs::hex::from_hex,
    chainv4_ui::widgets,
    egui::{RichText, Ui},
    serde_json::json,
};

/// How long a frame spends hashing before it lets the page draw.
const SLICE: std::time::Duration = std::time::Duration::from_millis(8);
/// How many nonces a slice tries between looks at the clock.
const TRIES: u64 = 2_048;

#[derive(Default)]
pub struct FaucetPage {
    to: String,
    /// The work under way: the address, the window, the bits wanted, and the next nonce to try.
    working: Option<(Address, u64, u32, u64)>,
    /// The request sent, whose answer the store holds.
    asked: Option<Ask>,
    trouble: Option<String>,
}

pub fn show(ui: &mut Ui, ctx: &mut Ctx, page: &mut FaucetPage) {
    let look = ctx.look;
    let palette = look.palette;
    super::heading(ui, look, "Faucet", "Coins for a dev or test chain, from the node's own, once per address and connection each period.", |_| {});

    let Some(status) = ctx.store.status.clone() else {
        super::note(ui, look, "Waiting for the node…");

        return;
    };
    let Some(faucet) = status.faucet.clone().filter(|faucet| !faucet.is_null()) else {
        super::note(ui, look, "This node runs no faucet. A node runs one with --faucet-key; see the node guide.");

        return;
    };
    let amount = ctx.coins(faucet["amount"].as_str().unwrap_or("0"));
    let hours = faucet["every_ms"].as_u64().unwrap_or(0) / 3_600_000;

    super::section(ui, look, "Ask for coins", |ui| {
        super::note(ui, look, format!("{amount} to each address that asks, once every {hours} hours. Worth nothing elsewhere."));
        ui.add_space(6.0);
        let busy = page.working.is_some() || page.asked.as_ref().is_some_and(|ask| !ctx.store.calls.contains_key(ask) && !ctx.store.failures.contains_key(ask));
        let typed = !page.to.trim().is_empty();
        let pressed = widgets::beside(
            ui,
            |ui| widgets::primary(ui, &palette, &format!("Get {amount}"), !busy && typed).clicked(),
            |ui, width| {
                ui.add(egui::TextEdit::singleline(&mut page.to).hint_text("ed25519:0x…, an Ethereum 0x…, a bc1q…, or a name such as alice.dev").desired_width(width));
            },
        );

        if pressed {
            page.trouble = None;
            page.asked = None;

            match recipient(ctx, page.to.trim()) {
                Ok(Some(to)) => {
                    let bits = u32::try_from(faucet["work_bits"].as_u64().unwrap_or(0)).unwrap_or(u32::MAX);
                    page.working = Some((to, faucet_window(ctx.now), bits, 0));
                }
                // A name the node is asked for: pressed again once it answers.
                Ok(None) => page.trouble = Some("Looking the name up: press again in a moment.".to_string()),
                Err(why) => page.trouble = Some(why),
            }
        }

        let genesis = status.hash.parse::<HashType>().ok().and_then(|hash| Hash::from_bytes(hash, &from_hex(&status.genesis)?));

        if let (Some((to, window, bits, next)), Some(genesis)) = (page.working, genesis) {
            let started = web_time::Instant::now();
            let mut from = next;
            let mut found = None;

            while found.is_none() && started.elapsed() < SLICE {
                found = solve_faucet(&genesis, &to, window, bits, from, TRIES);
                from += TRIES;
            }

            match found {
                Some(nonce) => {
                    let ask = Ask::Faucet(json!({ "to": to.to_string(), "window": window, "nonce": nonce }).to_string());
                    ctx.api.ask(ask.clone());
                    page.working = None;
                    page.asked = Some(ask);
                }
                None => {
                    page.working = Some((to, window, bits, from));
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(RichText::new(format!("Working out the faucet's proof of work: {from} of about {} hashes", 1u64 << bits.min(63))).color(palette.muted));
                    });
                    ui.ctx().request_repaint();
                }
            }
        }

        if let Some(ask) = &page.asked {
            match (ctx.store.calls.get(ask), ctx.store.failures.get(ask)) {
                (Some(answer), _) => {
                    ui.colored_label(palette.good, format!("{amount} on its way to {}", answer["to"].as_str().unwrap_or("?")));
                    let id = answer["id"].as_str().unwrap_or("").to_string();

                    if !id.is_empty() && ui.link(RichText::new(format!("transaction {id}")).monospace()).clicked() {
                        ctx.go(super::View::Tx(id));
                    }
                }
                (None, Some(failure)) => {
                    ui.colored_label(palette.bad, failure.message.as_str());
                }
                (None, None) => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(RichText::new("Asking the faucet…").color(palette.muted));
                    });
                }
            }
        }

        if let Some(trouble) = &page.trouble {
            ui.colored_label(palette.bad, trouble.as_str());
        }
    });
}

/// Whom the field names: an address, an Ethereum one as Ethereum writes it, or a name the node
/// resolves; None while the name's lookup is out.
fn recipient(ctx: &mut Ctx, text: &str) -> Result<Option<Address>, String> {
    if let Ok(address) = text.parse() {
        return Ok(Some(address));
    }

    if text.contains(':') {
        return text.parse().map(Some).map_err(|err| format!("{text}: {err}"));
    }

    if text.len() == 42 && text.starts_with("0x") {
        return format!("secp256k1:{text}").parse().map(Some).map_err(|err| format!("{text}: {err}"));
    }

    let name = text.to_ascii_lowercase();

    match ctx.store.names.get(&name) {
        Some(found) => match &found.resolves_to {
            Some(target) => target.parse().map(Some).map_err(|err| format!("{target}: {err}")),
            None => Err(format!("{text} resolves to nothing: it is {}", found.status)),
        },
        None => {
            ctx.api.ask(Ask::Name(name));

            Ok(None)
        }
    }
}
