//! Picking files to read, alike wherever an app runs: rfd's dialog on the desktop and in the
//! browser, and on Android the system's document picker, or its camera for a photo. Android hands a pick back only to Java,
//! so there the APK carries a small activity of its own, `ui/android/Picker.java`, which
//! `ui/build-android.sh` compiles in when it finds a JDK and Android's build-tools. An APK built
//! without one cannot pick files, and [`can_pick`] says so.

/// A file picked, read whole.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct File {
    pub name: String,
    pub bytes: Vec<u8>,
}

/// What to ask for: several files or one, and on the desktop and in the browser, which kind, by a
/// name and its extensions. Android's picker offers every file.
#[derive(Clone, Copy, Debug, Default)]
pub struct Pick {
    pub many: bool,
    pub kind: Option<(&'static str, &'static [&'static str])>,
    /// A photo to take rather than a file to pick. On Android the system's camera opens, and the
    /// photo comes back as a JPEG, or on Android 9 and older as the camera's small preview, a PNG.
    /// The desktop and the browser reach no camera, so they pick an image, which a phone's
    /// browser offers to take with its camera.
    pub camera: bool,
}

/// The images a photo picked on the desktop, in the browser or on iOS without a camera may be.
#[cfg(not(target_os = "android"))]
const IMAGES: (&str, &[&str]) = ("Images", &["png", "jpg", "jpeg"]);

/// What a pick comes to: the files, none when nothing was picked, or why they could not be read.
pub type Picked = Result<Vec<File>, String>;

/// What a pick is handed to once it is read.
pub type Done = Box<dyn FnOnce(Picked) + Send>;

/// Whether files can be picked here: always on the desktop and in the browser, and on Android when
/// the APK carries the picker.
pub fn can_pick(ctx: &egui::Context) -> bool {
    #[cfg(target_os = "android")]
    return android::can_pick(ctx);

    #[cfg(not(target_os = "android"))]
    {
        let _ = ctx;

        true
    }
}

/// Opens a picker. What is picked comes to done, read: at once on the desktop, whose dialog holds
/// the window while it is open, and later in the browser and on Android, which repaint when it
/// comes. A pick asked for while another is open on Android takes its place.
pub fn pick(ctx: &egui::Context, pick: Pick, done: impl FnOnce(Picked) + Send + 'static) {
    let wake = ctx.clone();
    let done = move |picked: Picked| {
        done(picked);
        wake.request_repaint();
    };

    #[cfg(not(any(target_arch = "wasm32", target_os = "android", target_os = "ios")))]
    {
        let mut dialog = rfd::FileDialog::new();

        if let Some((name, extensions)) = pick.kind.or(pick.camera.then_some(IMAGES)) {
            dialog = dialog.add_filter(name, extensions);
        }

        let paths = match pick.many {
            true => dialog.pick_files().unwrap_or_default(),
            false => dialog.pick_file().into_iter().collect(),
        };
        done(
            paths
                .iter()
                .map(|path| {
                    let name = path.file_name().map_or_else(String::new, |name| name.to_string_lossy().into_owned());

                    match std::fs::read(path) {
                        Ok(bytes) => Ok(File { name, bytes }),
                        Err(err) => Err(format!("{name}: {err}")),
                    }
                })
                .collect(),
        );
    }

    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(async move {
        let mut dialog = rfd::AsyncFileDialog::new();

        if let Some((name, extensions)) = pick.kind.or(pick.camera.then_some(IMAGES)) {
            dialog = dialog.add_filter(name, extensions);
        }

        let handles = match pick.many {
            true => dialog.pick_files().await.unwrap_or_default(),
            false => dialog.pick_file().await.into_iter().collect(),
        };
        let mut files = Vec::new();

        for handle in handles {
            files.push(File {
                name: handle.file_name(),
                bytes: handle.read().await,
            });
        }

        done(Ok(files));
    });

    #[cfg(target_os = "android")]
    android::ask(ctx, pick, Box::new(done));

    #[cfg(target_os = "ios")]
    ios::pick(pick, Box::new(done));
}

/// On iOS, UIKit's document picker, a copy of each file picked read from where it lands; or for a
/// photo, the system's camera, the photo as a JPEG. Each is presented over the app's window and
/// answers its delegate, kept until it does.
#[cfg(target_os = "ios")]
mod ios {
    use {
        super::{Done, File, Pick, Picked},
        objc2::rc::Retained,
        objc2::runtime::{AnyObject, ProtocolObject},
        objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send},
        objc2_foundation::{NSArray, NSDictionary, NSObject, NSObjectProtocol, NSString, NSURL},
        objc2_ui_kit::{
            UIApplication, UIDocumentPickerDelegate, UIDocumentPickerViewController, UIImage, UIImagePickerController, UIImagePickerControllerDelegate,
            UIImagePickerControllerInfoKey, UIImagePickerControllerOriginalImage, UIImagePickerControllerSourceType, UINavigationControllerDelegate,
            UIViewController,
        },
        objc2_uniform_type_identifiers::{UTType, UTTypeItem},
        std::cell::RefCell,
    };

    std::thread_local! {
        /// The delegate of the picker shown, kept until it answers.
        static SHOWN: RefCell<Option<Retained<Delegate>>> = const { RefCell::new(None) };
    }

    pub(super) struct Ivars {
        done: RefCell<Option<Done>>,
    }

    define_class!(
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "Chainv4PickDelegate"]
        #[ivars = Ivars]
        pub(super) struct Delegate;

        unsafe impl NSObjectProtocol for Delegate {}

        unsafe impl UIDocumentPickerDelegate for Delegate {
            #[unsafe(method(documentPicker:didPickDocumentsAtURLs:))]
            fn picked(&self, _controller: &UIDocumentPickerViewController, urls: &NSArray<NSURL>) {
                let files = urls
                    .iter()
                    .filter_map(|url| url.path().map(|path| path.to_string()))
                    .map(|path| {
                        let name = std::path::Path::new(&path).file_name().map_or_else(String::new, |name| name.to_string_lossy().into_owned());

                        std::fs::read(&path).map(|bytes| File { name: name.clone(), bytes }).map_err(|err| format!("{name}: {err}"))
                    })
                    .collect();
                self.finish(files);
            }

            #[unsafe(method(documentPickerWasCancelled:))]
            fn canceled(&self, _controller: &UIDocumentPickerViewController) {
                self.finish(Ok(Vec::new()));
            }
        }

        unsafe impl UINavigationControllerDelegate for Delegate {}

        unsafe impl UIImagePickerControllerDelegate for Delegate {
            #[unsafe(method(imagePickerController:didFinishPickingMediaWithInfo:))]
            unsafe fn photographed(&self, picker: &UIImagePickerController, info: &NSDictionary<UIImagePickerControllerInfoKey, AnyObject>) {
                // SAFETY: the key is UIKit's own, and what it names is a UIImage.
                let image = unsafe { info.objectForKey(UIImagePickerControllerOriginalImage) }.and_then(|image| image.downcast::<UIImage>().ok());
                let jpeg = image.and_then(|image| image.jpeg_representation(0.9));
                picker.dismissViewControllerAnimated_completion(true, None);
                self.finish(match jpeg {
                    Some(jpeg) => Ok(vec![File { name: "photo.jpg".to_string(), bytes: jpeg.to_vec() }]),
                    None => Err("the camera gave no photo".to_string()),
                });
            }

            #[unsafe(method(imagePickerControllerDidCancel:))]
            fn photo_canceled(&self, picker: &UIImagePickerController) {
                picker.dismissViewControllerAnimated_completion(true, None);
                self.finish(Ok(Vec::new()));
            }
        }
    );

    impl Delegate {
        fn new(done: Done, mtm: MainThreadMarker) -> Retained<Delegate> {
            let this = Delegate::alloc(mtm).set_ivars(Ivars { done: RefCell::new(Some(done)) });

            // SAFETY: NSObject's init, on a fresh instance.
            unsafe { msg_send![super(this), init] }
        }

        fn finish(&self, picked: Picked) {
            if let Some(done) = self.ivars().done.borrow_mut().take() {
                done(picked);
            }

            SHOWN.with(|shown| shown.borrow_mut().take());
        }
    }

    /// What presents the picker: the app window's root view controller.
    fn presenter(mtm: MainThreadMarker) -> Option<Retained<UIViewController>> {
        #[allow(deprecated)]
        let windows = UIApplication::sharedApplication(mtm).windows();

        windows.iter().find_map(|window| window.rootViewController())
    }

    pub(super) fn pick(pick: Pick, done: Done) {
        let Some(mtm) = MainThreadMarker::new() else {
            return done(Err("a pick is asked for from the app's own thread".to_string()));
        };
        let Some(presenter) = presenter(mtm) else {
            return done(Err("the app has no window to show the picker over".to_string()));
        };
        let delegate = Delegate::new(done, mtm);

        let shown: Retained<UIViewController> = match pick.camera && UIImagePickerController::isSourceTypeAvailable(UIImagePickerControllerSourceType::Camera, mtm) {
            true => {
                let camera = UIImagePickerController::new(mtm);
                camera.setSourceType(UIImagePickerControllerSourceType::Camera);
                // SAFETY: the delegate is both a navigation controller's and an image picker's.
                unsafe { camera.setDelegate(Some(&delegate)) };

                Retained::into_super(Retained::into_super(camera))
            }
            false => {
                // Files of the kinds named, by their extensions; otherwise any.
                let kinds: Vec<Retained<UTType>> = pick
                    .kind
                    .or(pick.camera.then_some(super::IMAGES))
                    .map(|(_, extensions)| extensions.iter().filter_map(|extension| UTType::typeWithFilenameExtension(&NSString::from_str(extension))).collect())
                    .unwrap_or_default();
                // SAFETY: UTTypeItem is UniformTypeIdentifiers' own constant.
                let kinds = if kinds.is_empty() { vec![unsafe { UTTypeItem }.retain()] } else { kinds };
                let documents = UIDocumentPickerViewController::initForOpeningContentTypes_asCopy(UIDocumentPickerViewController::alloc(mtm), &NSArray::from_retained_slice(&kinds), true);
                documents.setAllowsMultipleSelection(pick.many);
                documents.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));

                Retained::into_super(documents)
            }
        };

        presenter.presentViewController_animated_completion(&shown, true, None);
        SHOWN.with(|kept| *kept.borrow_mut() = Some(delegate));
    }
}

/// On Android, a pick is asked for through the egui context, which the app's Android glue owns
/// and checks every frame: it starts the picker, waits for it, and hands over what it read.
#[cfg(any(target_os = "android", test))]
pub(crate) mod android {
    use {
        super::{Done, Pick},
        std::sync::{Arc, Mutex, PoisonError},
    };

    /// A pick asked for, for the glue to start.
    pub(crate) struct Asked {
        pub many: bool,
        pub camera: bool,
        pub done: Done,
    }

    /// The pick asked for and not yet started: one at a time, the newest.
    #[derive(Clone, Default)]
    struct Asks(Arc<Mutex<Option<Asked>>>);

    fn asks(ctx: &egui::Context) -> Asks {
        ctx.data_mut(|data| data.get_temp_mut_or_default::<Asks>(egui::Id::unique("chainv4-ui pick")).clone())
    }

    pub(crate) fn ask(ctx: &egui::Context, pick: Pick, done: Done) {
        *asks(ctx).0.lock().unwrap_or_else(PoisonError::into_inner) = Some(Asked {
            many: pick.many,
            camera: pick.camera,
            done,
        });
        ctx.request_repaint();
    }

    /// The pick asked for since last time, taken.
    pub(crate) fn asked(ctx: &egui::Context) -> Option<Asked> {
        asks(ctx).0.lock().unwrap_or_else(PoisonError::into_inner).take()
    }

    /// Says whether the APK carries the picker, as the glue found when it started.
    pub(crate) fn set_can_pick(ctx: &egui::Context, can: bool) {
        ctx.data_mut(|data| data.insert_temp(egui::Id::unique("chainv4-ui can pick"), can));
    }

    pub(crate) fn can_pick(ctx: &egui::Context) -> bool {
        ctx.data(|data| data.get_temp::<bool>(egui::Id::unique("chainv4-ui can pick"))).unwrap_or(false)
    }

    /// A color as Android packs one, in an int: alpha, red, green and blue, a byte each. The picker's
    /// window takes the app's background, so it does not flash as it shows.
    pub(crate) fn argb([red, green, blue, alpha]: [f32; 4]) -> i32 {
        let byte = |channel: f32| (channel.clamp(0.0, 1.0) * 255.0).round() as u32;

        ((byte(alpha) << 24) | (byte(red) << 16) | (byte(green) << 8) | byte(blue)) as i32
    }
}

#[cfg(test)]
mod tests {
    use {
        super::android::{argb, ask, asked, set_can_pick},
        super::*,
        std::sync::mpsc,
    };

    #[test]
    fn on_android_a_pick_waits_in_the_context_for_the_glue_to_take() {
        let ctx = egui::Context::default();

        assert!(asked(&ctx).is_none(), "nothing asked yet");
        assert!(!android::can_pick(&ctx), "no picker until the glue finds one");

        let (sent, got) = mpsc::channel();
        let first = sent.clone();
        ask(&ctx, Pick::default(), Box::new(move |_| first.send("first").unwrap()));
        ask(&ctx, Pick { many: true, camera: true, ..Pick::default() }, Box::new(move |_| sent.send("second").unwrap()));
        let taken = asked(&ctx).expect("asked");

        assert!(taken.many && taken.camera, "the newest pick takes the place of one not started");
        assert!(asked(&ctx).is_none(), "taken once");

        (taken.done)(Ok(Vec::new()));

        assert_eq!(got.try_recv(), Ok("second"));

        set_can_pick(&ctx, true);

        assert!(android::can_pick(&ctx));
    }

    #[test]
    fn colors_pack_as_android_wants_them() {
        assert_eq!(argb([1.0, 0.0, 0.0, 1.0]), 0xffff0000_u32 as i32);
        assert_eq!(argb([0.0, 0.5, 1.0, 1.0]), 0xff0080ff_u32 as i32);
        assert_eq!(argb([0.0, 0.0, 0.0, 0.0]), 0);
    }
}
