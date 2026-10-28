//! The explorer on the desktop, in the browser and on iOS. The app itself is the library, which
//! Android loads too.

use chainv4_explorer::explorer;

/// The node the desktop app talks to unless told otherwise.
#[cfg(not(target_arch = "wasm32"))]
const LOCAL_NODE: &str = "http://127.0.0.1:8080";

/// Runs the app on iOS, from its own main as UIKit wants: it talks to the node it talked to last,
/// or to one on this machine, which the Simulator reaches; on a phone, Settings names the node.
#[cfg(target_os = "ios")]
fn main() -> eframe::Result {
    chainv4_ui::ios::run("chainv4 explorer", "explorer.ron", |creation| explorer(creation, None, LOCAL_NODE))
}

/// Runs the desktop app: `explorer --node http://host:port`, or CHAINV4_NODE, or the node it
/// talked to last.
#[cfg(not(any(target_arch = "wasm32", target_os = "ios")))]
fn main() -> eframe::Result {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let mut args = std::env::args().skip(1);
    let mut node = std::env::var("CHAINV4_NODE").ok();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--node" => node = args.next(),
            "--help" | "-h" => {
                println!("explorer [--node URL]: explores a chainv4 node's HTTP API, by default {LOCAL_NODE}");

                return Ok(());
            }
            other => eprintln!("ignoring {other}"),
        }
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("chainv4 explorer")
            .with_inner_size([1360.0, 860.0])
            .with_min_inner_size([360.0, 480.0]),
        ..Default::default()
    };

    eframe::run_native(
        "chainv4 explorer",
        options,
        Box::new(move |creation| Ok(explorer(creation, node, LOCAL_NODE))),
    )
}

/// Runs the web app in the page's canvas. It talks to the node in ?node=, or else to the node it
/// talked to last, or else to the node that served the page.
#[cfg(target_arch = "wasm32")]
fn main() {
    use eframe::wasm_bindgen::JsCast;

    eframe::WebLogger::init(log::LevelFilter::Info).ok();

    wasm_bindgen_futures::spawn_local(async {
        let window = web_sys::window().expect("a page has a window");
        let location = window.location();
        let origin = location.origin().unwrap_or_default();
        let node = location
            .search()
            .ok()
            .and_then(|search| web_sys::UrlSearchParams::new_with_str(&search).ok())
            .and_then(|params| params.get("node"));
        let canvas = window
            .document()
            .and_then(|document| document.get_element_by_id("explorer"))
            .and_then(|element| element.dyn_into::<web_sys::HtmlCanvasElement>().ok())
            .expect("the page has a canvas with the id explorer");
        let started = eframe::WebRunner::new()
            .start(
                canvas,
                eframe::WebOptions::default(),
                Box::new(move |creation| Ok(explorer(creation, node, &origin))),
            )
            .await;

        if let Err(err) = started {
            log::error!("the explorer could not start: {err:?}");
        }
    });
}
