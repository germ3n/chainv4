//! Running an egui app on iOS, in winit's UIKit view, drawn through Metal. egui-winit keeps the app
//! inside the safe area and raises the keyboard while a text field wants it. What it does not do,
//! this does: egui reaches no clipboard on iOS, so what the app copies is put on the general
//! pasteboard here, and pasting is offered by [`crate::edit`], as on Android; and iOS has no back
//! button, so a swipe in from the left edge, iOS's own gesture for going back, comes to the app as
//! `Key::BrowserBack`, which it takes where Android's back gesture is. Files are picked through
//! UIKit's document picker (see [`crate::files`]), Face ID or Touch ID checks made through
//! LocalAuthentication (see [`crate::biometric`]), and the camera run through AVFoundation (see
//! [`crate::camera`]).

use {
    crate::edit::{Clipboard, Editing},
    objc2_foundation::NSString,
    objc2_ui_kit::UIPasteboard,
};

/// How near the left edge a swipe back starts, and how far right it goes, in points.
const EDGE: f32 = 24.0;
const SWIPE: f32 = 80.0;

/// Starts an app from its main: app storage in a file of the app's own, under its Application
/// Support folder.
pub fn run(title: &str, storage_file: &str, make: impl FnOnce(&eframe::CreationContext) -> Box<dyn eframe::App> + 'static) -> eframe::Result {
    let storage = std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join("Library/Application Support").join(storage_file));
    let options = eframe::NativeOptions { persistence_path: storage, ..Default::default() };

    eframe::run_native(title, options, Box::new(|creation| Ok(Box::new(OnIos { inner: make(creation), editing: Editing::default(), swipe: None }))))
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
