//! On a phone, the pages the tabs have no room for.

use {
    super::{Ctx, View},
    chainv4_ui::widgets,
    egui::Ui,
};

pub fn show(ui: &mut Ui, ctx: &mut Ctx) {
    let look = ctx.look;
    super::heading(ui, look, "More", "", |_| {});

    for (view, name, about) in View::MORE {
        let link = widgets::page_link(ui, &look.palette, name, about);
        ui.add_space(8.0);

        if link.clicked() {
            ctx.go(view);
        }
    }
}
