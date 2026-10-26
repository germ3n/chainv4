//! Starts the wallet on the desktop and in the browser. Android loads the library itself, from
//! `android_main`.

/// The node the desktop app, and the browser extension, talk to unless told otherwise.
#[cfg(not(target_os = "android"))]
const LOCAL_NODE: &str = "http://127.0.0.1:8080";

/// Runs the desktop app: `wallet --node http://host:port`, or CHAINV4_NODE, or the node it talked
/// to last.
#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
fn main() -> eframe::Result {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let mut args = std::env::args().skip(1);
    let mut node = std::env::var("CHAINV4_NODE").ok();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--node" => node = args.next(),
            "--help" | "-h" => {
                println!("wallet [--node URL]: a light wallet for a chainv4 node's HTTP API, by default {LOCAL_NODE}");

                return Ok(());
            }
            other => eprintln!("ignoring {other}"),
        }
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("chainv4 wallet")
            .with_inner_size([1080.0, 760.0])
            .with_min_inner_size([360.0, 520.0]),
        ..Default::default()
    };

    eframe::run_native(
        "chainv4 wallet",
        options,
        Box::new(move |creation| Ok(chainv4_wallet::wallet(creation, node, LOCAL_NODE))),
    )
}

/// Runs the web app in the page's canvas. It talks to the node in ?node=, or else to the node it
/// talked to last, or else to the node that served the page; in the browser extension, which no
/// node serves, to one on this machine.
#[cfg(target_arch = "wasm32")]
fn main() {
    use eframe::wasm_bindgen::JsCast;

    eframe::WebLogger::init(log::LevelFilter::Info).ok();

    wasm_bindgen_futures::spawn_local(async {
        let window = web_sys::window().expect("a page has a window");
        let location = window.location();
        let origin = location.origin().unwrap_or_default();
        let served = location.protocol().unwrap_or_default();
        let fallback = match served.as_str() {
            "chrome-extension:" | "moz-extension:" => LOCAL_NODE.to_string(),
            _ => origin,
        };
        let node = location
            .search()
            .ok()
            .and_then(|search| web_sys::UrlSearchParams::new_with_str(&search).ok())
            .and_then(|params| params.get("node"));
        let canvas = window
            .document()
            .and_then(|document| document.get_element_by_id("wallet"))
            .and_then(|element| element.dyn_into::<web_sys::HtmlCanvasElement>().ok())
            .expect("the page has a canvas with the id wallet");
        let started = eframe::WebRunner::new()
            .start(
                canvas,
                eframe::WebOptions::default(),
                Box::new(move |creation| Ok(chainv4_wallet::wallet(creation, node, &fallback))),
            )
            .await;

        if let Err(err) = started {
            log::error!("the wallet could not start: {err:?}");
        }
    });
}

/// Android starts the library, never this.
#[cfg(target_os = "android")]
fn main() {}
