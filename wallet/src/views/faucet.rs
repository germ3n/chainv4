//! On a node that runs a faucet, as a dev or test chain's may: a card on the Overview that asks it
//! for coins for one of the account's addresses. The faucet wants a moment's proof of work with
//! each request, which the wallet works out a slice a frame, so the page keeps moving, and then
//! sends; the node pays it once per address and connection each period.

use {
    super::Ctx,
    crate::node::{Ask, Failure},
    crate::theme,
    chainv4::faucet::{faucet_window, solve_faucet},
    chainv4::structs::address::Address,
    egui::{RichText, Ui},
    serde_json::{Value, json},
};

/// How long a frame spends hashing before it lets the page draw.
const SLICE: std::time::Duration = std::time::Duration::from_millis(8);
/// How many nonces a slice tries between looks at the clock.
const TRIES: u64 = 2_048;

#[derive(Default)]
pub struct FaucetState {
    /// Which of the account's addresses to pay, by its place among them.
    chosen: usize,
    /// The work under way: the address, the window, the bits wanted, and the next nonce to try.
    working: Option<(Address, u64, u32, u64)>,
    /// A request out, and what it came to.
    asked: Option<String>,
    said: Option<Result<String, String>>,
}

impl FaucetState {
    /// The node's answer to a request this card sent.
    pub fn answered(&mut self, ask: &Ask, result: &Result<Value, Failure>) {
        let Ask::Faucet(body) = ask else {
            return;
        };

        if self.asked.as_ref() != Some(body) {
            return;
        }

        self.asked = None;
        self.said = Some(match result {
            Ok(answer) => Ok(answer["id"].as_str().unwrap_or("?").to_string()),
            Err(failure) => Err(failure.message.clone()),
        });
    }
}

pub fn card(ui: &mut Ui, ctx: &mut Ctx, state: &mut FaucetState) {
    let palette = *ctx.palette;
    let Some(faucet) = ctx.faucet.cloned() else {
        return;
    };
    let Some(holder) = ctx.holder() else {
        return;
    };
    let addresses = holder.addresses();
    state.chosen = state.chosen.min(addresses.len().saturating_sub(1));
    let amount = ctx.amount(None, faucet["amount"].as_str().unwrap_or("0"));
    let every = faucet["every_ms"].as_u64().unwrap_or(0) / 3_600_000;

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Faucet");
        ui.label(
            RichText::new(format!("This node gives {amount} to each address that asks, once every {every} hours: coins for a dev or test chain, worth nothing elsewhere."))
                .color(palette.soft),
        );
        ui.add_space(8.0);

        if addresses.len() > 1 {
            egui::ComboBox::from_id_salt("faucet address")
                .selected_text(super::scheme(&addresses[state.chosen]))
                .show_ui(ui, |ui| {
                    for (index, address) in addresses.iter().enumerate() {
                        ui.selectable_value(&mut state.chosen, index, format!("{} · {}", super::scheme(address), crate::format::short(&address.to_string())));
                    }
                });
        }

        let busy = state.working.is_some() || state.asked.is_some();
        let Some(genesis) = ctx.chain.genesis else {
            super::waiting(ui, &palette, "Waiting for the light client to meet the chain…");

            return;
        };

        if theme::primary(ui, &palette, &format!("Get {amount}"), !busy).clicked() {
            let bits = u32::try_from(faucet["work_bits"].as_u64().unwrap_or(0)).unwrap_or(u32::MAX);
            state.working = Some((addresses[state.chosen], faucet_window(ctx.now), bits, 0));
            state.said = None;
        }

        // A slice of the work this frame, and the request once it holds.
        if let Some((to, window, bits, next)) = state.working {
            let started = web_time::Instant::now();
            let mut from = next;
            let mut found = None;

            while found.is_none() && started.elapsed() < SLICE {
                found = solve_faucet(&genesis, &to, window, bits, from, TRIES);
                from += TRIES;
            }

            match found {
                Some(nonce) => {
                    let body = json!({ "to": to.to_string(), "window": window, "nonce": nonce }).to_string();
                    state.working = None;
                    state.asked = Some(body.clone());
                    ctx.node.ask(Ask::Faucet(body));
                }
                None => {
                    state.working = Some((to, window, bits, from));
                    super::waiting(ui, &palette, &format!("Working out the faucet's proof of work: {} of about {} hashes", from, 1u64 << bits.min(63)));
                    ui.ctx().request_repaint();
                }
            }
        }

        if state.asked.is_some() {
            super::waiting(ui, &palette, "Asking the faucet…");
        }

        match &state.said {
            Some(Ok(id)) => {
                ui.colored_label(palette.good, format!("On its way: {}", crate::format::short(id)));
            }
            Some(Err(why)) => {
                ui.colored_label(palette.bad, why.as_str());
            }
            None => {}
        }
    });
}
