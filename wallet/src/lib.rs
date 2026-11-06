//! A chainv4 light wallet. Its keys come from a seed phrase, an address in every scheme for each
//! account, and are sealed with a password in app storage; everything it
//! shows of the chain it proved itself: it follows the chain from the genesis it trusts with the
//! light client, and checks every balance, token and transaction a node sends against headers it
//! checked. It runs as a desktop app, in the browser built with trunk, and on Android built with
//! xbuild: the binary starts the first two, and Android loads this library itself.

mod airgap;
mod app;
mod biometrics;
mod bitcoin;
mod chain;
mod dapp;
mod format;
mod guard;
mod hardware;
mod history;
mod keys;
#[cfg(all(feature = "bitcoin-light", not(target_arch = "wasm32")))]
mod light;
#[cfg(all(feature = "walletconnect", not(target_arch = "wasm32")))]
mod walletconnect;
mod lock;
mod metamask;
#[cfg(feature = "mining")]
mod miner;
mod node;
mod reading;
mod steps;
#[cfg(not(target_arch = "wasm32"))]
mod store;
mod theme;
mod updates;
mod vault;
mod views;

/// The wallet, for eframe to run. It talks to this node, or else to the one it talked to last, or
/// else to the fallback. Its wallets are kept under this folder of the app's, or, in a browser, which
/// has none, in eframe's storage.
pub fn wallet(creation: &eframe::CreationContext, node: Option<String>, fallback: &str, folder: Option<std::path::PathBuf>) -> Box<dyn eframe::App> {
    Box::new(app::Wallet::new(creation, node, fallback, folder))
}

/// Where Android starts the app, in the NativeActivity's own thread. Its storage is in the app's
/// private data folder.
#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
fn android_main(app: chainv4_ui::android::AndroidApp) {
    let folder = app.internal_data_path();
    chainv4_ui::android::run(app, "chainv4 wallet", "chainv4-wallet", "wallet.ron", move |creation| wallet(creation, None, chainv4_ui::PUBLIC_NODE, folder));
}
