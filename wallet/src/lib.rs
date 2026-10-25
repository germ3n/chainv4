//! A chainv4 light wallet. Its keys come from a seed phrase, an address in every scheme for each
//! account, and are sealed with a password in app storage; everything it
//! shows of the chain it proved itself: it follows the chain from the genesis it trusts with the
//! light client, and checks every balance, token and transaction a node sends against headers it
//! checked. It runs as a desktop app, in the browser built with trunk, and on Android built with
//! xbuild: the binary starts the first two, and Android loads this library itself.

mod app;
mod chain;
mod format;
mod history;
mod keys;
mod lock;
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
fn android_main(app: winit::platform::android::activity::AndroidApp) {
    android_logger::init_once(android_logger::Config::default().with_max_level(log::LevelFilter::Info).with_tag("chainv4-wallet"));

    let options = eframe::NativeOptions {
        // The file itself, whatever eframe's docs say of a folder.
        persistence_path: app.internal_data_path().map(|dir| dir.join("wallet.ron")),
        android_app: Some(app),
        ..Default::default()
    };
    let app = options.android_app.clone().expect("given above");
    let started = eframe::run_native(
        "chainv4 wallet",
        options,
        Box::new(|creation| {
            Ok(Box::new(OnAndroid {
                wallet: wallet(creation, None, ANDROID_NODE),
                app,
                keyboard: false,
                unwanted_since: None,
            }))
        }),
    );

    if let Err(err) = started {
        log::error!("the wallet could not start: {err}");
    }
}

/// The wallet on Android, kept clear of the status bar, the navigation bar and the keyboard: the
/// app draws behind them all, so Android's word on where content can be seen becomes egui's safe
/// area, and the wallet is drawn inside it.
#[cfg(target_os = "android")]
struct OnAndroid {
    wallet: Box<dyn eframe::App>,
    app: winit::platform::android::activity::AndroidApp,
    /// Whether the soft keyboard was asked for, and since when no text field has wanted it.
    keyboard: bool,
    unwanted_since: Option<f64>,
}

#[cfg(target_os = "android")]
impl OnAndroid {
    /// Shows the soft keyboard while a text field has focus, and hides it once none does.
    /// egui-winit would, from egui's IME output, but it also hides and shows the keyboard again
    /// whenever egui interrupts a composition, which a text field does on every tap on it and every
    /// frame a finger drags in it. There is no composition to interrupt in a NativeActivity, so on
    /// Android that only makes the keyboard flicker. So the wallet takes the IME output from
    /// egui-winit, and keeps the keyboard itself.
    fn keep_keyboard(&mut self, ctx: &egui::Context) {
        let wanted = ctx.output_mut(|output| output.ime.take()).is_some();
        let (time, tapped) = ctx.input(|input| (input.time, input.pointer.any_pressed()));

        if wanted {
            self.unwanted_since = None;

            // A tap on the field brings back a keyboard the person put away.
            if !self.keyboard || tapped {
                self.app.show_soft_input(true);
                self.keyboard = true;
            }
        } else if self.keyboard {
            // A field lets go of focus for a frame as a tap lands on it: the keyboard goes only
            // once no field has wanted it for a moment.
            let since = *self.unwanted_since.get_or_insert(time);

            if time - since >= 0.2 {
                self.app.hide_soft_input(true);
                self.keyboard = false;
                self.unwanted_since = None;
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(250));
            }
        }
    }
}

#[cfg(target_os = "android")]
impl eframe::App for OnAndroid {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        // The root Ui covers the whole window, safe area or not, so the wallet gets the safe part.
        let seen = ui.ctx().content_rect();
        ui.scope_builder(egui::UiBuilder::new().max_rect(seen), |ui| self.wallet.ui(ui, frame));
        self.keep_keyboard(ui.ctx());
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        self.wallet.save(storage);
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        self.wallet.clear_color(visuals)
    }

    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        // In pixels, and empty until Android has laid the window out.
        let seen = self.app.content_rect();

        if seen.right <= seen.left || seen.bottom <= seen.top {
            return;
        }

        let points = ctx.pixels_per_point();
        let screen = raw_input.screen_rect.unwrap_or_else(|| ctx.viewport_rect());
        raw_input.safe_area_insets = Some(egui::SafeAreaInsets(egui::epaint::MarginF32 {
            left: (seen.left as f32 / points).max(0.0),
            top: (seen.top as f32 / points).max(0.0),
            right: (screen.width() - seen.right as f32 / points).max(0.0),
            bottom: (screen.height() - seen.bottom as f32 / points).max(0.0),
        }));
    }
}
