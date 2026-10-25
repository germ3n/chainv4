//! Running an egui app on Android, in a NativeActivity with no Java of its own. The app draws behind
//! the status bar, the navigation bar and the keyboard, so Android's word on where content can be
//! seen becomes egui's safe area, and the app is drawn inside it. The soft keyboard shows while a
//! text field wants it, and goes once none does. What the app copies goes to Android's clipboard.

/// What Android hands an app's android_main.
pub use winit::platform::android::activity::AndroidApp;

/// Starts an app from its android_main: logging under a tag, app storage in a file of the app's
/// private data folder, and the app kept clear of the system's bars and keyboard.
pub fn run(app: AndroidApp, title: &str, log_tag: &str, storage_file: &str, make: impl FnOnce(&eframe::CreationContext) -> Box<dyn eframe::App> + 'static) {
    android_logger::init_once(android_logger::Config::default().with_max_level(log::LevelFilter::Info).with_tag(log_tag));

    let options = eframe::NativeOptions {
        // The file itself, whatever eframe's docs say of a folder.
        persistence_path: app.internal_data_path().map(|dir| dir.join(storage_file)),
        android_app: Some(app),
        ..Default::default()
    };
    let app = options.android_app.clone().expect("given above");
    let started = eframe::run_native(
        title,
        options,
        Box::new(|creation| {
            Ok(Box::new(OnAndroid {
                inner: make(creation),
                app,
                keyboard: false,
                unwanted_since: None,
            }))
        }),
    );

    if let Err(err) = started {
        log::error!("{title} could not start: {err}");
    }
}

/// An app on Android, kept clear of the status bar, the navigation bar and the keyboard.
struct OnAndroid {
    inner: Box<dyn eframe::App>,
    app: AndroidApp,
    /// Whether the soft keyboard was asked for, and since when no text field has wanted it.
    keyboard: bool,
    unwanted_since: Option<f64>,
}

impl OnAndroid {
    /// Shows the soft keyboard while a text field has focus, and hides it once none does.
    /// egui-winit would, from egui's IME output, but it also hides and shows the keyboard again
    /// whenever egui interrupts a composition, which a text field does on every tap on it and every
    /// frame a finger drags in it. There is no composition to interrupt in a NativeActivity, so on
    /// Android that only makes the keyboard flicker. So the app takes the IME output from
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

impl OnAndroid {
    /// Puts what the app copied this frame on Android's clipboard. egui-winit has no clipboard on
    /// Android, so the app's copy commands are taken from it, and handed to ClipboardManager.
    fn copy(&mut self, ctx: &egui::Context) {
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
            if let Err(err) = clipboard(&self.app, &text) {
                log::warn!("could not copy to the clipboard: {err}");
            }
        }
    }
}

/// Sets Android's clipboard to some text, as Java would:
/// `getSystemService("clipboard").setPrimaryClip(ClipData.newPlainText(label, text))`.
fn clipboard(app: &AndroidApp, text: &str) -> Result<(), jni::errors::Error> {
    use jni::{JValue, jni_sig, jni_str, objects::JObject};

    // SAFETY: both come from android-activity, which keeps the VM and a global reference to the
    // activity alive as long as the app runs; neither is freed here.
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };

    vm.attach_current_thread(|env| {
        let activity = unsafe { JObject::from_raw(env, app.activity_as_ptr().cast()) };
        let service = env.new_string("clipboard")?;
        let manager = env
            .call_method(&activity, jni_str!("getSystemService"), jni_sig!("(Ljava/lang/String;)Ljava/lang/Object;"), &[JValue::Object(&service)])?
            .l()?;
        let label = env.new_string("chainv4")?;
        let text = env.new_string(text)?;
        let clip = env
            .call_static_method(
                jni_str!("android/content/ClipData"),
                jni_str!("newPlainText"),
                jni_sig!("(Ljava/lang/CharSequence;Ljava/lang/CharSequence;)Landroid/content/ClipData;"),
                &[JValue::Object(&label), JValue::Object(&text)],
            )?
            .l()?;
        env.call_method(&manager, jni_str!("setPrimaryClip"), jni_sig!("(Landroid/content/ClipData;)V"), &[JValue::Object(&clip)])?;

        Ok(())
    })
}

impl eframe::App for OnAndroid {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        // The root Ui covers the whole window, safe area or not, so the app gets the safe part.
        let seen = ui.ctx().content_rect();
        ui.scope_builder(egui::UiBuilder::new().max_rect(seen), |ui| self.inner.ui(ui, frame));
        self.keep_keyboard(ui.ctx());
        self.copy(ui.ctx());
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        self.inner.save(storage);
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        self.inner.clear_color(visuals)
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
