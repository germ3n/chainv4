//! On a phone, the pages the tabs have no room for.

use {
    super::{Ctx, Page, Role},
    chainv4_ui::widgets,
    egui::Ui,
};

/// A page More lists: it, its name and what it is for.
pub type Listed = (Page, &'static str, &'static str);

/// The pages More lists, with what each is for, shown under the sidebar's headings (grouped). The
/// sidebar lists these too: a test keeps the two the same.
pub const PAGES: &[Listed] = &[
    (Page::Batch, "Batch", "Several steps as one transaction, all or nothing"),
    (Page::Authorize, "Authorize", "Sign a token allowance or transfer for someone to send"),
    #[cfg(feature = "nft")]
    (Page::Nfts, "NFTs", "What this account holds of collections"),
    (Page::Names, "Names", "Register and manage names like alice.dev"),
    #[cfg(feature = "mining")]
    (Page::Mining, super::MINING_NAME, "Mine here, and claim what a pool owes you"),
    #[cfg(not(feature = "mining"))]
    (Page::Mining, super::MINING_NAME, "Claim what a pool owes you"),
    (Page::Multisig, "Multisig", "Accounts that take several signatures to spend"),
    (Page::Governance, "Governance", "The chain's settings, and changes to approve"),
    (Page::Contracts, "Contracts", "Keep, call and deploy contracts"),
    #[cfg(feature = "swap")]
    (Page::Swaps, "Swaps", "Atomic swaps with Bitcoin and EVM chains"),
    #[cfg(feature = "dex")]
    (Page::Trade, "Trade", "Swap tokens on the DEXes, and add to pools"),
    #[cfg(feature = "bridge")]
    (Page::Bridge, "Bridge", "To and from EVM chains, and bitcoin as wBTC"),
    (Page::Accounts, "Accounts", "The wallet's accounts, addresses and keys"),
    (Page::Settings, "Settings", "The node, the theme, the lock and the words"),
];

/// What More lists, under the sidebar's headings and in its order: each group's pages the tabs
/// have no room for, a group with none left out.
pub fn grouped() -> Vec<(&'static str, Vec<&'static Listed>)> {
    let tabs = super::tabs(Role::Online);

    super::groups(Role::Online)
        .iter()
        .map(|(heading, pages)| {
            let listed = pages
                .iter()
                .filter(|(page, _)| !tabs.iter().any(|(tab, _)| tab == page))
                .filter_map(|(page, _)| PAGES.iter().find(|(listed, _, _)| listed == page))
                .collect::<Vec<_>>();

            (*heading, listed)
        })
        .filter(|(_, listed)| !listed.is_empty())
        .collect()
}

pub fn show(ui: &mut Ui, ctx: &mut Ctx) {
    let palette = *ctx.palette;
    crate::theme::title(ui, &palette, "More", "");

    for (heading, pages) in grouped() {
        ui.add_space(10.0);
        crate::theme::caps(ui, &palette, heading);
        ui.add_space(4.0);

        for (page, name, about) in pages {
            if widgets::page_link(ui, &palette, name, about).clicked() {
                *ctx.page = *page;
            }
        }
    }
}
