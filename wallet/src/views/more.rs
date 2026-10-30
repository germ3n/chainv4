//! On a phone, the pages the tabs have no room for.

use {
    super::{Ctx, Page},
    chainv4_ui::widgets,
    egui::Ui,
};

/// The pages More lists, with what each is for. The sidebar lists these too: a test keeps the two
/// the same.
pub const PAGES: &[(Page, &str, &str)] = &[
    #[cfg(feature = "nft")]
    (Page::Nfts, "NFTs", "What this account holds of collections"),
    (Page::Names, "Names", "Register and manage names like alice.dev"),
    (Page::Mining, "Mining", "Mine here, and claim what a pool owes you"),
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

pub fn show(ui: &mut Ui, ctx: &mut Ctx) {
    let palette = *ctx.palette;
    crate::theme::title(ui, &palette, "More", "");

    for (page, name, about) in PAGES {
        if widgets::page_link(ui, &palette, name, about).clicked() {
            *ctx.page = *page;
        }
    }
}
