//! The From picker the pages that send share: which of the account's addresses a page sends from,
//! each shown by its scheme, its address short and the coin it holds, as proven. Each address holds
//! its own coin, tokens and NFTs, so a page that took the first alone would leave the rest out.
//!
//! The pick holds across pages, until another is picked: what was sent from on Send is what Bridge
//! and Trade start from. Where a page cannot use some scheme, its address is still listed, greyed
//! with why, so nothing seems missing.

use {
    super::Ctx,
    crate::format,
    crate::theme,
    chainv4::structs::address::Address,
    egui::{RichText, Ui},
};

/// Where the pick last made, on any page, is kept for the others: in memory, for this run.
fn kept() -> egui::Id {
    egui::Id::unique("the address picked to send from")
}

/// The address last picked on any page, if one was.
pub fn remembered(egui: &egui::Context) -> Option<Address> {
    egui.data(|data| data.get_temp::<Address>(kept()))
}

/// Keeps a pick for the other pages.
pub fn remember(egui: &egui::Context, address: Address) {
    egui.data_mut(|data| data.insert_temp(kept(), address));
}

/// An address as the picker lists it: its scheme, its address short without its kind, and what it
/// holds of the coin, proven, or … while that is on its way.
pub fn label(ctx: &Ctx, from: &Address) -> String {
    let text = from.to_string();
    let held = ctx.chain.account(&text).map_or_else(|| "…".to_string(), |proven| ctx.brief(None, &proven.value.balance.to_string()));

    // The scheme leads, so the address goes without its kind.
    let short = format::short(&text);
    let hex = short.split_once(':').map_or(short.as_str(), |(_, hex)| hex);

    format!("{} · {hex} · {held}", super::scheme(from))
}

/// Settles a page's pick before it is drawn: the pick last made on any page, while it is one of the
/// account's that this page can use, else the page's own, else the first it can use. refused says
/// why the page cannot use an address, None for one it can.
pub fn settle(ctx: &Ctx, picked: &mut Option<Address>, refused: &dyn Fn(&Address) -> Option<&'static str>) {
    let addresses = ctx.holder().map(|holder| holder.addresses()).unwrap_or_default();
    let usable = |address: &Address| addresses.contains(address) && refused(address).is_none();

    *picked = remembered(ctx.wake).filter(usable).or(picked.filter(usable)).or_else(|| addresses.iter().copied().find(|address| refused(address).is_none()));
}

/// The picker: the account's addresses, each with what it holds, those the page cannot use greyed
/// with why. Shown once there is more than one to pick from; true once the pick changed.
pub fn pick(ui: &mut Ui, ctx: &Ctx, salt: &str, picked: &mut Option<Address>, refused: &dyn Fn(&Address) -> Option<&'static str>) -> bool {
    let palette = *ctx.palette;
    settle(ctx, picked, refused);
    let addresses = ctx.holder().map(|holder| holder.addresses()).unwrap_or_default();

    if addresses.len() < 2 {
        return false;
    }

    let before = *picked;
    theme::caps(ui, &palette, "From");
    egui::ComboBox::from_id_salt(("from", salt))
        .selected_text(picked.as_ref().map(|picked| label(ctx, picked)).unwrap_or_else(|| "None of them can".to_string()))
        .width(ui.available_width())
        .show_ui(ui, |ui| {
            for from in &addresses {
                match refused(from) {
                    None => {
                        ui.selectable_value(picked, Some(*from), label(ctx, from));
                    }
                    Some(why) => {
                        ui.add_enabled(false, egui::Button::selectable(false, format!("{} · {why}", label(ctx, from))));
                    }
                }
            }
        });

    // What this page cannot use, said on the page and not only in the list.
    let mut reasons: Vec<(&'static str, &'static str)> = addresses.iter().filter_map(|address| Some((super::scheme(address), refused(address)?))).collect();
    reasons.dedup();

    for (scheme, why) in reasons {
        ui.add(egui::Label::new(RichText::new(format!("{scheme}: {why}.")).small().color(palette.muted)).wrap());
    }

    let changed = *picked != before;

    if changed && let Some(picked) = picked {
        remember(ctx.wake, *picked);
    }

    changed
}

#[cfg(test)]
mod tests {
    use {super::*, crate::views::drawn};

    #[test]
    fn a_pick_made_on_one_page_is_where_the_next_starts_unless_it_cannot_use_it() {
        let mut book = drawn::two_phrases();
        let mut holders = crate::keys::open(&book).unwrap();
        let mine = holders[0].addresses();
        let (mut first, mut second, mut picky) = (None, None, None);
        let not_bitcoin = |address: &Address| matches!(address, Address::Bitcoin(_)).then_some("this page cannot use a Bitcoin key");

        let mut once = true;
        drawn::page(&mut book, &mut holders, (360.0, 740.0), |_, ctx| {
            if !std::mem::take(&mut once) {
                return;
            }

            settle(ctx, &mut first, &|_| None);
            remember(ctx.wake, mine[3]);
            settle(ctx, &mut second, &|_| None);
            settle(ctx, &mut picky, &not_bitcoin);
        });

        assert_eq!(first, Some(mine[0]), "the first, with nothing picked yet");
        assert_eq!(second, Some(mine[3]), "the one picked on another page");
        assert_eq!(picky, Some(mine[0]), "a page that cannot use it starts from the first it can");
    }

    #[test]
    fn the_picker_lists_every_address_with_its_scheme_and_says_why_one_cannot_be_used_on_every_screen() {
        let mut book = drawn::two_phrases();
        let mut holders = crate::keys::open(&book).unwrap();
        let not_bitcoin = |address: &Address| matches!(address, Address::Bitcoin(_)).then_some("this page cannot use a Bitcoin key");

        for size in drawn::SCREENS {
            let mut picked = None;
            let (width, _, shown) = drawn::page_shown(&mut book, &mut holders, size, |ui, ctx| {
                pick(ui, ctx, "test", &mut picked, &not_bitcoin);
            });
            let texts: Vec<&str> = shown.iter().map(|(text, _)| text.as_str()).collect();

            assert!(width <= size.0 + 1.0, "{width} on {size:?}");
            assert!(texts.iter().any(|text| text.starts_with("Ed25519 · ")), "the pick, by its scheme: {texts:?}");
            assert!(texts.contains(&"Bitcoin: this page cannot use a Bitcoin key."), "said on the page: {texts:?}");
        }
    }
}
