//! A chainv4 light wallet. Its keys come from a seed phrase, an address in every scheme for each
//! account, and are sealed with a password in app storage; everything it
//! shows of the chain it proved itself: it follows the chain from the genesis it trusts with the
//! light client, and checks every balance, token and transaction a node sends against headers it
//! checked. It runs as a desktop app, in the browser built with trunk, and on Android built with
//! xbuild: the binary starts the first two, and Android loads this library itself.

mod airgap;
mod app;
mod biometrics;
mod chain;
mod format;
mod history;
mod keys;
mod lock;
mod metamask;
mod node;
mod theme;
mod vault;
mod views;

/// The wallet, for eframe to run. It talks to this node, or else to the one it talked to last, or
/// else to the fallback.
pub fn wallet(creation: &eframe::CreationContext, node: Option<String>, fallback: &str) -> Box<dyn eframe::App> {
    Box::new(app::Wallet::new(creation, node, fallback))
}

/// The node an Android wallet talks to until told otherwise: the machine running the emulator, as
/// the emulator sees it. On a phone, Settings names the node.
#[cfg(target_os = "android")]
const ANDROID_NODE: &str = "http://10.0.2.2:8080";

/// Where Android starts the app, in the NativeActivity's own thread. App storage, the vault in it,
/// is a file in the app's private data folder.
#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
fn android_main(app: chainv4_ui::android::AndroidApp) {
    chainv4_ui::android::run(app, "chainv4 wallet", "chainv4-wallet", "wallet.ron", |creation| wallet(creation, None, ANDROID_NODE));
}
