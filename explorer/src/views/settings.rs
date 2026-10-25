//! Which node the explorer reads, the proxy every request goes through, how it looks, and what its
//! light client trusts there.

use {
    super::{Ctx, network::Streaming},
    crate::api::Ask,
    crate::format,
    chainv4_ui::widgets,
    egui::{RichText, Ui},
};

/// The proxy being typed, and why the one chosen would not do.
#[derive(Default)]
pub struct SettingsState {
    proxy: Option<String>,
    proxy_trouble: Option<String>,
}

/// The settings page. The node being typed lives with the app, which connects to what is asked
/// once the page is drawn, through the proxy then set.
pub fn show(ui: &mut Ui, ctx: &mut Ctx, address: &mut String, state: &mut SettingsState, streaming: &Streaming, connect: &mut Option<String>) {
    let look = ctx.look;
    let palette = look.palette;
    super::heading(ui, look, "Settings", "The node the explorer reads and the way to it, how it looks, and what it trusts.", |_| {});

    super::section(ui, look, "Node", |ui| {
        super::note(ui, look, "Its HTTP API. The explorer reads everything from it, and checks what it can with its light client.");
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
        let answers = ctx.store.status.is_some() && !ctx.store.failures.contains_key(&Ask::Status);
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

            if let Some(status) = &status {
                facts.text("chain", format!("{} · {} hashes · {} decimals", status.coin.symbol, status.hash, status.coin.decimals));
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

    super::section(ui, look, "Appearance", |ui| {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut ctx.settings.light_theme, false, "Dark");
            ui.selectable_value(&mut ctx.settings.light_theme, true, "Light");
        });
    });

    super::section(ui, look, "Light client", |ui| {
        let (color, words, story) = match (&ctx.light.trouble, ctx.light.following()) {
            (Some(trouble), _) => (palette.bad, "stopped", trouble.clone()),
            (None, Some((tip, final_height))) if ctx.light.caught_up() => (
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
        ui.add_space(6.0);

        let node = ctx.settings.node.clone();

        match ctx.settings.pinned.get(&node).cloned() {
            Some(genesis) => {
                super::note(ui, look, "The genesis it trusts at this node: the one it saw there first.");
                super::copyable(ui, &genesis);

                if widgets::quiet(ui, &palette, "Forget it").on_hover_text("Trust whatever genesis the node shows next, and start over").clicked() {
                    ctx.settings.pinned.remove(&node);
                    *connect = Some(node);
                }
            }
            None => super::note(ui, look, "It trusts nothing at this node yet: it pins the genesis it sees there first."),
        }
    });
}
