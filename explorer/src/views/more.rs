//! On a phone, the pages the tabs have no room for.

use {
    super::{Ctx, View},
    chainv4_ui::widgets,
    egui::{RichText, Ui},
};

/// The room kept on a card's right for its arrow.
const ARROW: f32 = 24.0;

pub fn show(ui: &mut Ui, ctx: &mut Ctx) {
    let look = ctx.look;
    super::heading(ui, look, "More", "", |_| {});

    for (view, name, about) in View::MORE {
        // The whole card is what a finger taps.
        let drawn = ui
            .scope(|ui| {
                widgets::card(ui, &look.palette, |ui| {
                    ui.horizontal(|ui| {
                        // What the page is for wraps short of the arrow, rather than under it.
                        let width = ui.available_width() - ARROW;
                        ui.vertical(|ui| {
                            ui.set_max_width(width);
                            ui.label(widgets::strong(name, 15.0).color(look.palette.ink));
                            ui.add(egui::Label::new(RichText::new(about).color(look.palette.muted)).wrap());
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(RichText::new("›").size(20.0).color(look.palette.muted));
                        });
                    });
                });
            })
            .response;
        ui.add_space(8.0);

        if ui.interact(drawn.rect, ui.scope_id().with(("more", name)), egui::Sense::click()).clicked() {
            ctx.go(view);
        }
    }
}
