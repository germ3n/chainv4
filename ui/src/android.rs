//! Running an egui app on Android, in a NativeActivity. The app draws behind the status bar, the
//! navigation bar and the keyboard, so Android's word on where content can be seen becomes egui's
//! safe area, and the app is drawn inside it. The soft keyboard shows while a text field wants it,
//! and goes once none does. egui reaches no clipboard on Android, so what the app copies is put on
//! Android's here, and pasting is offered by [`crate::edit`]. Files are picked through the Java
//! activity the APK carries when it was built with one (see [`crate::files`]).

use {
    crate::{
        edit::{Clipboard, Editing},
        files::{self, File, Picked},
    },
    jni::{
        Env, JValue, jni_sig, jni_str,
        objects::{Global, JByteArray, JClass, JClassLoader, JObject, JObjectArray, JString, LoaderContext},
    },
};

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
            let picker = picker(&app);
            files::android::set_can_pick(&creation.egui_ctx, picker.is_some());

            Ok(Box::new(OnAndroid {
                inner: make(creation),
                app,
                keyboard: false,
                unwanted_since: None,
                editing: Editing::default(),
                picker,
                waiting: None,
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
    /// The bar a held finger brings up over a text field, and the Paste button over the keyboard.
    editing: Editing,
    /// The picker's Java class, when the APK carries it.
    picker: Option<Global<JClass<'static>>>,
    /// What a pick is handed to once the picker is done.
    waiting: Option<files::Done>,
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
            if let Err(err) = AndroidClipboard(&self.app).set(&text) {
                log::warn!("could not copy to the clipboard: {err}");
            }
        }
    }

    /// Starts a pick the app asked for, and hands over what a pick under way read once it is done.
    /// The picker is an activity of its own, so the app goes to the background while it is open,
    /// and finds what was picked when it comes back.
    fn pick(&mut self, ctx: &egui::Context, background: [f32; 4]) {
        if let Some(asked) = files::android::asked(ctx) {
            match self.start(asked.many, asked.camera, files::android::argb(background)) {
                Ok(()) => self.waiting = Some(asked.done),
                Err(err) => (asked.done)(Err(format!("could not open the picker: {err}"))),
            }
        }

        if self.waiting.is_none() {
            return;
        }

        match self.take() {
            Ok(Some(picked)) => {
                if let Some(done) = self.waiting.take() {
                    done(picked);
                }
            }
            Ok(None) => ctx.request_repaint_after(std::time::Duration::from_millis(150)),
            Err(err) => {
                if let Some(done) = self.waiting.take() {
                    done(Err(format!("could not read what was picked: {err}")));
                }
            }
        }
    }

    /// `Picker.start(activity, many, camera, background)`.
    fn start(&self, many: bool, camera: bool, background: i32) -> jni::errors::Result<()> {
        let class = self.picker.as_ref().ok_or(jni::errors::Error::NullPtr("this APK carries no picker"))?;

        java(&self.app, |env, activity| {
            env.call_static_method(
                &**class,
                jni_str!("start"),
                jni_sig!("(Landroid/app/Activity;ZZI)V"),
                &[JValue::Object(activity), JValue::Bool(many), JValue::Bool(camera), JValue::Int(background)],
            )?;

            Ok(())
        })
    }

    /// `Picker.take()`: the files picked, or why they could not be read, once the picker is done.
    fn take(&self) -> jni::errors::Result<Option<Picked>> {
        let class = self.picker.as_ref().ok_or(jni::errors::Error::NullPtr("this APK carries no picker"))?;

        java(&self.app, |env, _| {
            let taken = env.call_static_method(&**class, jni_str!("take"), jni_sig!("()[Ljava/lang/Object;"), &[])?.l()?;

            if taken.is_null() {
                return Ok(None);
            }

            let taken = env.cast_local::<JObjectArray<JObject>>(taken)?;
            let failure = taken.get_element(env, 2)?;

            if !failure.is_null() {
                let failure = env.cast_local::<JString>(failure)?;

                return Ok(Some(Err(failure.try_to_string(env)?)));
            }

            let names = taken.get_element(env, 0)?;
            let names = env.cast_local::<JObjectArray<JString>>(names)?;
            let contents = taken.get_element(env, 1)?;
            let contents = env.cast_local::<JObjectArray<JByteArray>>(contents)?;
            let mut files = Vec::new();

            for index in 0..names.len(env)? {
                let name = names.get_element(env, index)?.try_to_string(env)?;
                let bytes = contents.get_element(env, index)?;
                let bytes = env.convert_byte_array(&bytes)?;
                files.push(File { name, bytes });
            }

            Ok(Some(Ok(files)))
        })
    }
}

/// The picker's class, when the APK carries it. It comes from the app's class loader: one a
/// native thread finds by name comes from the system's, which holds Android's classes alone.
fn picker(app: &AndroidApp) -> Option<Global<JClass<'static>>> {
    let found = java(app, |env, activity| {
        let loader = env.call_method(activity, jni_str!("getClassLoader"), jni_sig!("()Ljava/lang/ClassLoader;"), &[])?.l()?;
        let loader = env.cast_local::<JClassLoader>(loader)?;
        let class = LoaderContext::Loader(&loader).load_class(env, jni_str!("org.chainv4.ui.Picker"), true)?;

        env.new_global_ref(class)
    });

    match found {
        Ok(class) => Some(class),
        Err(err) => {
            log::info!("this APK carries no file picker: {err}");

            None
        }
    }
}

/// Runs some Java through JNI, with the app's activity at hand. An exception Java throws is written
/// to the log and cleared, so the next call starts clean.
fn java<T>(app: &AndroidApp, call: impl FnOnce(&mut Env, &JObject) -> jni::errors::Result<T>) -> jni::errors::Result<T> {
    // SAFETY: both come from android-activity, which keeps the VM and a global reference to the
    // activity alive as long as the app runs; neither is freed here.
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };

    vm.attach_current_thread(|env| {
        let activity = unsafe { JObject::from_raw(env, app.activity_as_ptr().cast()) };
        let result = call(env, &activity);

        if env.exception_check() {
            env.exception_describe();
            env.exception_clear();
        }

        result
    })
}

/// Android's clipboard, through its ClipboardManager.
struct AndroidClipboard<'a>(&'a AndroidApp);

impl AndroidClipboard<'_> {
    /// `getSystemService("clipboard")`.
    fn manager<'local>(env: &mut Env<'local>, activity: &JObject) -> jni::errors::Result<JObject<'local>> {
        let service = env.new_string("clipboard")?;

        env.call_method(activity, jni_str!("getSystemService"), jni_sig!("(Ljava/lang/String;)Ljava/lang/Object;"), &[JValue::Object(&service)])?
            .l()
    }

    /// `setPrimaryClip(ClipData.newPlainText(label, text))`.
    fn set(&self, text: &str) -> jni::errors::Result<()> {
        java(self.0, |env, activity| {
            let manager = Self::manager(env, activity)?;
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
}

impl Clipboard for AndroidClipboard<'_> {
    /// `hasPrimaryClip()`, which Android answers without the app reading the clip, and so without
    /// telling the person it did.
    fn has_text(&mut self) -> bool {
        java(self.0, |env, activity| {
            let manager = Self::manager(env, activity)?;

            env.call_method(&manager, jni_str!("hasPrimaryClip"), jni_sig!("()Z"), &[])?.z()
        })
        .unwrap_or(false)
    }

    /// `getPrimaryClip().getItemAt(0).coerceToText(activity).toString()`.
    fn text(&mut self) -> Option<String> {
        let read = java(self.0, |env, activity| {
            let manager = Self::manager(env, activity)?;
            let clip = env.call_method(&manager, jni_str!("getPrimaryClip"), jni_sig!("()Landroid/content/ClipData;"), &[])?.l()?;

            if clip.is_null() || env.call_method(&clip, jni_str!("getItemCount"), jni_sig!("()I"), &[])?.i()? == 0 {
                return Ok(None);
            }

            let item = env
                .call_method(&clip, jni_str!("getItemAt"), jni_sig!("(I)Landroid/content/ClipData$Item;"), &[JValue::Int(0)])?
                .l()?;
            let text = env
                .call_method(&item, jni_str!("coerceToText"), jni_sig!("(Landroid/content/Context;)Ljava/lang/CharSequence;"), &[JValue::Object(activity)])?
                .l()?;
            let text = env.call_method(&text, jni_str!("toString"), jni_sig!("()Ljava/lang/String;"), &[])?.l()?;
            let text = env.cast_local::<JString>(text)?;

            Ok(Some(text.try_to_string(env)?))
        });

        read.unwrap_or_else(|err| {
            log::warn!("could not read the clipboard: {err}");

            None
        })
    }
}

impl eframe::App for OnAndroid {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        // The root Ui covers the whole window, safe area or not, so the app gets the safe part.
        let seen = ui.ctx().content_rect();
        ui.scope_builder(egui::UiBuilder::new().max_rect(seen), |ui| self.inner.ui(ui, frame));

        let ctx = ui.ctx().clone();
        self.editing.show(&ctx, self.keyboard, &mut AndroidClipboard(&self.app));
        self.keep_keyboard(&ctx);
        self.copy(&ctx);
        self.pick(&ctx, self.inner.clear_color(&ctx.global_style().visuals));
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        self.inner.save(storage);
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        self.inner.clear_color(visuals)
    }

    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        self.editing.feed(raw_input);

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
