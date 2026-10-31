//! A chainv4 explorer: blocks on a map, transactions and the hops their contracts made, the
//! network, the mempool and finality, live. It talks to a node's HTTP API, and runs as a desktop
//! app, in the browser built with trunk, and on Android built with xbuild: the binary starts the
//! first two, and Android loads this library itself.

mod api;
mod app;
mod format;
mod light;
mod model;
mod proof;
mod release;
mod store;
mod theme;
mod views;

/// The explorer, for eframe to run. It talks to this node, or else to the one it talked to last,
/// or else to the fallback.
pub fn explorer(creation: &eframe::CreationContext, node: Option<String>, fallback: &str) -> Box<dyn eframe::App> {
    Box::new(app::Explorer::new(creation, node, fallback))
}

/// Where Android starts the app, in the NativeActivity's own thread.
#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
fn android_main(app: chainv4_ui::android::AndroidApp) {
    chainv4_ui::android::run(app, "chainv4 explorer", "chainv4-explorer", "explorer.ron", |creation| explorer(creation, None, chainv4_ui::PUBLIC_NODE));
}
