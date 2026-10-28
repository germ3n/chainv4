//! Running an egui app on iOS, in winit's UIKit view, drawn through Metal. winit raises the keyboard
//! while a text field wants it. What it does not do, this does: the app is drawn inside the safe
//! area, as on Android, its view moved there and its edges painted the app's own color, as eframe
//! sizes its surface to winit's inner size, which on iOS is the safe area's, while egui-winit would
//! lay the app out over the whole screen; egui reaches no clipboard on iOS, so what the app copies is put on the general
//! pasteboard here, and pasting is offered by [`crate::edit`], as on Android; and iOS has no back
//! button, so a swipe in from the left edge, iOS's own gesture for going back, comes to the app as
//! `Key::BrowserBack`, which it takes where Android's back gesture is. Files are picked through
//! UIKit's document picker (see [`crate::files`]), Face ID or Touch ID checks made through
//! LocalAuthentication (see [`crate::biometric`]), and the camera run through AVFoundation (see
//! [`crate::camera`]).
//!
//! iOS 27 runs an app only through the scene lifecycle, which winit predates: UIKit asks for a
//! scene delegate, so the app's Info.plist names `Chainv4SceneDelegate`, registered here before
//! UIKit starts, which keeps the scene it is given; and winit's window, made outside any scene,
//! shows only once it is that scene's, and draws nothing before: so it is caught as eframe hands
//! the app its creation context, and the two are joined by whichever comes second.

use {
    crate::edit::{Clipboard, Editing},
    objc2::rc::Retained,
    objc2::{ClassType, MainThreadOnly, Message, define_class},
    objc2_foundation::{NSObject, NSObjectProtocol, NSString},
    objc2_core_foundation::{CGPoint, CGRect, CGSize},
    objc2_ui_kit::{UIColor, UIPasteboard, UIScene, UISceneConnectionOptions, UISceneDelegate, UISceneSession, UIView, UIWindow, UIWindowScene, UIWindowSceneDelegate},
    raw_window_handle::{HasWindowHandle, RawWindowHandle},
    std::cell::RefCell,
};

std::thread_local! {
    /// The scene UIKit connected, and winit's window, until both are there and joined.
    static SCENE: RefCell<Option<Retained<UIWindowScene>>> = const { RefCell::new(None) };
    static WINDOW: RefCell<Option<Retained<UIWindow>>> = const { RefCell::new(None) };
    /// winit's view, which is moved into the safe area.
    static VIEW: RefCell<Option<Retained<UIView>>> = const { RefCell::new(None) };
}

/// Moves winit's view into the window's safe area, as it changes with the device turned, and
/// paints the window behind it the app's color. Where the safe area is, in points from the
/// window's top left, and its size: what egui is told the screen is.
fn safe_area(background: [f32; 4]) -> Option<egui::Rect> {
    let window = WINDOW.with(|window| window.borrow().clone())?;
    let view = VIEW.with(|view| view.borrow().clone())?;
    let (bounds, insets) = (window.bounds(), window.safeAreaInsets());
    let safe = CGRect {
        origin: CGPoint { x: insets.left, y: insets.top },
        size: CGSize { width: bounds.size.width - insets.left - insets.right, height: bounds.size.height - insets.top - insets.bottom },
    };

    if view.frame() != safe {
        view.setFrame(safe);
        let [red, green, blue, alpha] = background.map(f64::from);
        window.setBackgroundColor(Some(&UIColor::colorWithRed_green_blue_alpha(red, green, blue, alpha)));
    }

    Some(egui::Rect::from_min_size(egui::pos2(safe.origin.x as f32, safe.origin.y as f32), egui::vec2(safe.size.width as f32, safe.size.height as f32)))
}

/// Puts winit's window in the scene and shows it, once both are there.
fn join() {
    let (Some(scene), Some(window)) = (SCENE.with(|scene| scene.borrow().clone()), WINDOW.with(|window| window.borrow().clone())) else {
        return;
    };

    window.setWindowScene(Some(&scene));
    window.makeKeyAndVisible();
}

/// Keeps winit's window, from what eframe hands the app as it makes it.
fn keep_window(handle: &impl HasWindowHandle) {
    let Ok(handle) = handle.window_handle() else {
        return;
    };
    let RawWindowHandle::UiKit(handle) = handle.as_raw() else {
        return;
    };
    // SAFETY: winit's view, alive as long as its window, which lives as long as the app.
    let view = unsafe { &*handle.ui_view.as_ptr().cast::<UIView>() };

    if let Some(window) = view.window() {
        WINDOW.with(|kept| *kept.borrow_mut() = Some(window));
        VIEW.with(|kept| *kept.borrow_mut() = Some(view.retain()));
        join();
    }
}

define_class!(
    /// The scene delegate the Info.plist names: it keeps the window scene UIKit connects.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "Chainv4SceneDelegate"]
    struct SceneDelegate;

    unsafe impl NSObjectProtocol for SceneDelegate {}

    unsafe impl UISceneDelegate for SceneDelegate {
        #[unsafe(method(scene:willConnectToSession:options:))]
        fn connect(&self, scene: &UIScene, _session: &UISceneSession, _options: &UISceneConnectionOptions) {
            if let Ok(scene) = scene.retain().downcast::<UIWindowScene>() {
                SCENE.with(|kept| *kept.borrow_mut() = Some(scene));
                join();
            }
        }
    }

    unsafe impl UIWindowSceneDelegate for SceneDelegate {}
);

/// How near the left edge a swipe back starts, and how far right it goes, in points.
const EDGE: f32 = 24.0;
const SWIPE: f32 = 80.0;

/// Starts an app from its main: app storage in a file of the app's own, under its Application
/// Support folder.
pub fn run(title: &str, storage_file: &str, make: impl FnOnce(&eframe::CreationContext) -> Box<dyn eframe::App> + 'static) -> eframe::Result {
    let storage = std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join("Library/Application Support").join(storage_file));
    // The device's own limits, rather than WebGPU's defaults, which ask a little more than some
    // iOS GPUs, the Simulator's among them, give: egui needs far less than either.
    let mut setup = eframe::egui_wgpu::WgpuSetupCreateNew::without_display_handle();
    setup.device_descriptor = std::sync::Arc::new(|adapter| eframe::wgpu::DeviceDescriptor {
        label: Some("chainv4 device"),
        required_limits: adapter.limits(),
        ..Default::default()
    });
    let options = eframe::NativeOptions {
        persistence_path: storage,
        wgpu_options: eframe::egui_wgpu::WgpuConfiguration { wgpu_setup: eframe::egui_wgpu::WgpuSetup::CreateNew(setup), ..Default::default() },
        ..Default::default()
    };
    let _ = log::set_logger(&StdErr).map(|()| log::set_max_level(log::LevelFilter::Info));
    // Registered before UIKit looks it up by name, as it makes the app's scene.
    let _ = SceneDelegate::class();

    eframe::run_native(
        title,
        options,
        Box::new(|creation| {
            keep_window(creation);

            Ok(Box::new(OnIos { inner: make(creation), editing: Editing::default(), swipe: None }))
        }),
    )
}

/// The log, to standard error, which Xcode and `simctl launch --console` show.
struct StdErr;

impl log::Log for StdErr {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Info
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            eprintln!("[{}] {}: {}", record.level(), record.target(), record.args());
        }
    }

    fn flush(&self) {}
}

/// An app on iOS: its clipboard, and the swipe back.
struct OnIos {
    inner: Box<dyn eframe::App>,
    /// The bar a held finger brings up over a text field, and the Paste button over the keyboard.
    editing: Editing,
    /// Where a touch began at the left edge, for a swipe back, until it lifts or is one.
    swipe: Option<egui::Pos2>,
}

impl OnIos {
    /// Puts what the app copied this frame on the general pasteboard.
    fn copy(ctx: &egui::Context) {
        let copied: Vec<String> = ctx.output_mut(|output| {
            let mut copied = Vec::new();
            output.commands.retain(|command| match command {
                egui::OutputCommand::CopyText(text) => {
                    copied.push(text.clone());

                    false
                }
                _ => true,
            });

            copied
        });

        for text in copied {
            Pasteboard.set(&text);
        }
    }

    /// A touch that starts at the left edge and moves right far enough, more across than down, is
    /// the swipe back: it comes to the app as `Key::BrowserBack`.
    fn swipe_back(&mut self, raw_input: &mut egui::RawInput) {
        let mut back = false;

        for event in &raw_input.events {
            match event {
                egui::Event::PointerButton { pos, pressed: true, .. } if pos.x <= EDGE => self.swipe = Some(*pos),
                egui::Event::PointerMoved(pos) => {
                    if let Some(start) = self.swipe
                        && pos.x - start.x >= SWIPE
                        && (pos.y - start.y).abs() < pos.x - start.x
                    {
                        back = true;
                        self.swipe = None;
                    }
                }
                egui::Event::PointerButton { pressed: false, .. } | egui::Event::PointerGone => self.swipe = None,
                _ => {}
            }
        }

        if back {
            for pressed in [true, false] {
                raw_input.events.push(egui::Event::Key { key: egui::Key::BrowserBack, physical_key: None, pressed, repeat: false, modifiers: egui::Modifiers::NONE });
            }
        }
    }
}

impl eframe::App for OnIos {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.inner.ui(ui, frame);

        let ctx = ui.ctx().clone();
        let keyboard = ctx.egui_wants_keyboard_input();
        self.editing.show(&ctx, keyboard, &mut Pasteboard);
        Self::copy(&ctx);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        self.inner.save(storage);
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        self.inner.clear_color(visuals)
    }

    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        // The screen is the safe area, where the view is; touches come in the window's points, so
        // they move by its corner.
        if let Some(safe) = safe_area(self.inner.clear_color(&ctx.global_style().visuals)) {
            raw_input.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, safe.size()));
            raw_input.safe_area_insets = None;
            let shift = safe.min.to_vec2();

            for event in &mut raw_input.events {
                match event {
                    egui::Event::PointerMoved(pos) | egui::Event::PointerButton { pos, .. } | egui::Event::Touch { pos, .. } => *pos -= shift,
                    _ => {}
                }
            }
        }

        self.editing.feed(raw_input);
        self.swipe_back(raw_input);
        self.inner.raw_input_hook(ctx, raw_input);
    }
}

/// iOS's general pasteboard, which UIKit lets any thread use.
struct Pasteboard;

impl Pasteboard {
    fn set(&self, text: &str) {
        // SAFETY: the general pasteboard is UIKit's own, alive as long as the app runs.
        unsafe { UIPasteboard::generalPasteboard().setString(Some(&NSString::from_str(text))) };
    }
}

impl Clipboard for Pasteboard {
    /// `hasStrings`, which iOS answers without the app reading the pasteboard, and so without the
    /// banner saying it did.
    fn has_text(&mut self) -> bool {
        // SAFETY: as in set.
        unsafe { UIPasteboard::generalPasteboard().hasStrings() }
    }

    fn text(&mut self) -> Option<String> {
        // SAFETY: as in set.
        unsafe { UIPasteboard::generalPasteboard().string() }.map(|text| text.to_string())
    }
}
