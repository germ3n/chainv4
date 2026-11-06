//! Which node the explorer reads, the proxy every request goes through, how it looks, and what its
//! light client trusts there.

use {
    super::{Ctx, network::Streaming},
    crate::api::Ask,
    crate::format,
    chainv4_ui::widgets,
    egui::{RichText, Ui},
};

/// The proxy being typed, and why the one chosen would not do; and a checkpoint to start from.
#[derive(Default)]
pub struct SettingsState {
    reading: super::reading::ReadingState,
    proxy: Option<String>,
    proxy_trouble: Option<String>,
    checkpoint: String,
    /// A genesis id typed, to trust at this node before meeting it.
    genesis: String,
}

/// How the explorer uses the network: Low data, and how long it waits for an answer. A new wait
/// takes a new way to the node, as a new proxy does.
pub(crate) fn network_use(ui: &mut Ui, ctx: &mut Ctx, connect: &mut Option<String>) {
    let look = ctx.look;

    super::section(ui, look, "Network use", |ui| {
        let proxied = !cfg!(target_arch = "wasm32") && !ctx.settings.proxy.is_empty();
        let (_, waits) = widgets::network_use(ui, &look.palette, &mut ctx.settings.low_data, &mut ctx.settings.patience, proxied);

        if waits {
            *connect = Some(ctx.settings.node.clone());
        }
    });
}

/// The settings page. The node being typed lives with the app, which connects to what is asked
/// once the page is drawn, through the proxy then set.
pub fn show(ui: &mut Ui, ctx: &mut Ctx, address: &mut String, state: &mut SettingsState, streaming: &Streaming, connect: &mut Option<String>) {
    let look = ctx.look;
    let palette = look.palette;
    let reading = ctx.settings.read_blocks;
    super::heading(ui, look, "Settings", "The node the explorer reads and the way to it, how it looks, and what it trusts.", |_| {});
    super::reading::card(ui, ctx, &mut state.reading, connect);

    if !reading {
        super::section(ui, look, "Node's report token", |ui| {
            let node = ctx.api.base().to_string();
            let carries = crate::api::carries_token(&node);
            super::note(ui, look, "A node you run keeps its peers' and miners' addresses from whoever asks. Its --report-token shows them to you here. It goes only to GET /node, and only over https, an onion or this machine.");
            ui.add_space(4.0);
            let token = ctx.settings.report_tokens.entry(node.clone()).or_default();
            let response = chainv4_ui::secret::secret(ui, egui::TextEdit::singleline(token).hint_text("the token, if this node is yours").desired_width(ui.available_width()));

            if response.changed() {
                let token = token.clone();
                ctx.api.bear_report_token(Some(&token));
            }

            if !carries {
                super::note(ui, look, "This node is reached over plain http: the token stays here until it is https or an onion.");
            }
        });
    }

    super::section(ui, look, "Node", |ui| {
        super::note(ui, look, match reading {
            true => "Its HTTP API, which the explorer reads headers and final blocks from, and sends transactions through.",
            false => "Its HTTP API. The explorer reads everything from it, and checks what it can with its light client.",
        });
        ui.add_space(4.0);
        let pressed = widgets::beside(
            ui,
            |ui| widgets::primary(ui, &palette, "Connect", true).clicked(),
            |ui, width| {
                ui.add(egui::TextEdit::singleline(address).hint_text("http://127.0.0.1:8080").desired_width(width));
            },
        );

        if pressed {
            *connect = Some(address.trim().to_string());
        }

        ui.add_space(6.0);
        let answers = match reading {
            true => ctx.api.down().is_none(),
            false => ctx.store.status.is_some() && !ctx.store.failures.contains_key(&Ask::Status),
        };
        let status = ctx.store.status.clone();
        super::facts(ui, look, "settings_node", |facts| {
            facts.row("answers", |ui| match answers {
                true => {
                    widgets::pill(ui, "yes", palette.good);
                }
                false => {
                    widgets::pill(ui, "no", palette.bad);
                }
            });
            facts.row("updates", |ui| {
                let (color, text) = super::network::updates(&palette, streaming);
                ui.label(RichText::new(text).color(color));
            });

            // The node's report: what it hides from this explorer, and whether it took the token.
            if let Some(node) = &ctx.store.node {
                facts.row("shares", |ui| match (node.token_refused, node.hidden.is_empty()) {
                    (true, _) => {
                        widgets::pill(ui, "refused the report token", palette.bad);
                    }
                    (false, true) => {
                        widgets::pill(ui, "everything", palette.good);
                    }
                    (false, false) => {
                        ui.label(RichText::new(format!("all but {}", node.hidden.join(", "))).color(palette.muted));
                    }
                });
            }

            if let Some(status) = &status {
                facts.text("chain", format!("{} · {} · {} hashes · {} decimals", status.chain_name(), status.coin.symbol, status.hash, status.coin.decimals));
                facts.row("genesis", |ui| super::copyable(ui, &status.genesis));
                facts.text("height", format::thousands(&status.height.to_string()));
            }
        });
    });

    super::section(ui, look, "Proxy", |ui| {
        let set = ctx.settings.proxy.clone();
        let typed = state.proxy.get_or_insert_with(|| set.clone());

        if let Some(chosen) = widgets::proxy(ui, &palette, typed, &mut state.proxy_trouble, &set) {
            ctx.settings.proxy = chosen;
            *connect = Some(ctx.settings.node.clone());
        }
    });

    network_use(ui, ctx, connect);

    super::section(ui, look, "Appearance", |ui| {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut ctx.settings.light_theme, false, "Dark");
            ui.selectable_value(&mut ctx.settings.light_theme, true, "Light");
        });
    });

    super::section(ui, look, "Light client", |ui| {
        let (color, words, story) = match (&ctx.light.trouble, ctx.light.following()) {
            (Some(trouble), _) => (palette.bad, "stopped", trouble.clone()),
            (None, Some((tip, final_height))) if ctx.light.has_followed() && ctx.light.fast_resumed.is_some() => (
                palette.good,
                "following",
                widgets::fast_resumed_story(&ctx.light.fast_resumed_words().unwrap_or_default(), tip, final_height),
            ),
            (None, Some((tip, final_height))) if ctx.light.has_followed() => (
                palette.good,
                "following",
                format!("Every header to {tip} checked, and finality to {final_height}, trusting only the genesis below."),
            ),
            (None, Some((tip, _))) => (palette.warn, "catching up", format!("{tip} headers checked so far.")),
            (None, None) => (palette.muted, "starting", "Fetching the chain's light parameters.".to_string()),
        };
        ui.horizontal(|ui| {
            widgets::dot(ui, color);
            ui.label(widgets::strong(words, 13.5).color(color));
        });
        super::note(ui, look, story);

        if let Some(said) = ctx.light.fast_resumed_words() {
            ui.add(egui::Label::new(RichText::new(said).color(palette.warn)).wrap());
        }

        ui.add_space(6.0);

        let node = ctx.settings.node.clone();
        let mut trusted = ctx.settings.fast_resume.get(&node).copied().unwrap_or(false);

        if reading {
            super::note(ui, look, "Fast resume is off while Read from checked blocks is on: the light client walks every header.");
        } else if widgets::fast_resume_switch(ui, &palette, &mut trusted, "Opening the explorer skips the headers from genesis", "the genesis it trusts") {
            match trusted {
                true => {
                    ctx.settings.fast_resume.insert(node.clone(), true);
                    *connect = Some(node.clone());
                }
                false => {
                    ctx.settings.fast_resume.remove(&node);
                }
            }
        }

        ui.add_space(8.0);

        match ctx.settings.pinned.get(&node).cloned() {
            Some(genesis) => {
                super::note(ui, look, "The genesis it trusts at this node: the one it saw there first.");
                super::copyable(ui, &genesis);

                if widgets::quiet(ui, &palette, "Forget it").on_hover_text("Trust whatever genesis the node shows next, and start over").clicked() {
                    ctx.settings.pinned.remove(&node);
                    *connect = Some(node.clone());
                }
            }
            None => {
                super::note(ui, look, "It trusts nothing at this node yet: it pins the genesis it sees there first. Or give the genesis id to trust, as someone you trust shows it, and it refuses a node on any other chain from the start.");
                let trust = widgets::beside(
                    ui,
                    |ui| widgets::quiet(ui, &palette, "Trust it").clicked(),
                    |ui, width| {
                        ui.add(egui::TextEdit::singleline(&mut state.genesis).hint_text("a genesis id, 0x…").desired_width(width));
                    },
                );

                if trust {
                    match trusted_genesis(&state.genesis) {
                        Some(genesis) => {
                            ctx.settings.pinned.insert(node.clone(), genesis);
                            state.genesis.clear();
                            *connect = Some(node.clone());
                        }
                        None => state.genesis = "not a genesis id".to_string(),
                    }
                }
            }
        }

        ui.add_space(8.0);

        match ctx.settings.checkpoints.get(&node).cloned() {
            Some(checkpoint) => {
                super::note(ui, look, "It starts at this checkpoint, trusted as the genesis is, rather than following from genesis:");
                super::copyable(ui, &checkpoint);

                if widgets::quiet(ui, &palette, "Forget it").on_hover_text("Follow from genesis, checking every block").clicked() {
                    ctx.settings.checkpoints.remove(&node);
                    *connect = Some(node.clone());
                }
            }
            None => super::note(
                ui,
                look,
                "Unless trusted as above, it follows from genesis, or from the node's own finalized checkpoint the first time it meets one. A checkpoint's block id, as someone you trust shows it, starts it there each time instead.",
            ),
        }

        let start = widgets::beside(
            ui,
            |ui| widgets::quiet(ui, &palette, "Start there").clicked(),
            |ui, width| {
                ui.add(egui::TextEdit::singleline(&mut state.checkpoint).hint_text("a checkpoint's block id, 0x…").desired_width(width));
            },
        );

        if start {
            match chainv4::structs::hex::from_hex(state.checkpoint.trim()) {
                Some(bytes) if bytes.len() >= 16 => {
                    ctx.settings.checkpoints.insert(node.clone(), chainv4::structs::hex::to_hex(&bytes));
                    state.checkpoint.clear();
                    *connect = Some(node);
                }
                _ => state.checkpoint = "not a block id".to_string(),
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use {super::*, crate::reading::tests::{SCREENS, drawn}};

    /// The whole page, as Settings draws it, with Read from checked blocks on or off.
    fn page(size: (f32, f32), on: bool) -> (f32, String) {
        let mut state = SettingsState::default();
        let (mut address, mut connect) = ("http://here".to_string(), None);

        drawn(size, on, |ui, ctx| show(ui, ctx, &mut address, &mut state, &Streaming::Off, &mut connect))
    }

    #[test]
    fn network_use_and_the_reading_cap_fit_every_screen_and_say_what_they_do() {
        for size in SCREENS {
            for on in [false, true] {
                let (width, _) = page(size, on);
                let (_, said) = page((size.0, 6_000.0), on);

                assert!(width <= size.0 + 0.5, "{width} on a screen {} wide", size.0);

                for words in ["NETWORK USE", "Low data", "Wait for answers", "Usual", "10×", "Keeps at most", "16 MB", "2 GB"] {
                    assert!(said.contains(words), "{words} on {size:?}: {said}");
                }

                assert!(said.contains("gives up after 12 seconds without getting connected, and 30 seconds in all"), "today's waits: {said}");
            }
        }
    }
}

/// A genesis id as typed, written as the explorer pins one: None for text that is no id.
fn trusted_genesis(text: &str) -> Option<String> {
    chainv4::structs::hex::from_hex(text.trim()).filter(|bytes| bytes.len() >= 16).map(|bytes| chainv4::structs::hex::to_hex(&bytes))
}

#[cfg(test)]
mod genesis_tests {
    use super::trusted_genesis;

    #[test]
    fn a_genesis_id_typed_is_pinned_as_the_light_client_writes_one() {
        let id = chainv4::structs::hash::Hash::zero(chainv4::structs::hash::HashType::default_chain()).to_string();

        assert_eq!(trusted_genesis(&format!("  {}  ", id.to_uppercase().replacen("0X", "0x", 1))), Some(id));
        assert_eq!(trusted_genesis("0x1234"), None, "too short for an id");
        assert_eq!(trusted_genesis("not hex"), None);
    }
}
