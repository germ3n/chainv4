//! A live camera, for reading QR codes as another screen shows them: its frames in grey, upright,
//! the newest at a time, for the app to search and to draw. In the browser, getUserMedia, the back
//! camera where there is one. On the desktop, the first webcam, through nokhwa. On Android, the back
//! camera through Camera2, by the APK's own Java class, which the app's Android glue drives, as it
//! drives the picker. On iOS, the back camera through AVFoundation. Behind the camera feature: only
//! an app that reads codes needs it.
//!
//! A camera runs from open until it is dropped. Frames come as fast as the camera gives them; an
//! app that is slower to read them than that only ever sees the newest.

use std::sync::{Arc, Mutex, PoisonError};

/// One frame: its luma, a byte a pixel, row after row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub width: usize,
    pub height: usize,
    pub luma: Vec<u8>,
}

impl Frame {
    /// The frame turned clockwise by a quarter turn this many times: what Android's sensor, which
    /// lies sideways in the phone, gives upright.
    pub fn turned(self, quarters: u32) -> Frame {
        let (width, height) = (self.width, self.height);
        let at = |x: usize, y: usize| self.luma[y * width + x];

        match quarters % 4 {
            1 => Frame {
                width: height,
                height: width,
                luma: (0..width).flat_map(|y| (0..height).map(move |x| (x, y))).map(|(x, y)| at(y, height - 1 - x)).collect(),
            },
            2 => Frame {
                width,
                height,
                luma: self.luma.iter().rev().copied().collect(),
            },
            3 => Frame {
                width: height,
                height: width,
                luma: (0..width).flat_map(|y| (0..height).map(move |x| (x, y))).map(|(x, y)| at(width - 1 - y, x)).collect(),
            },
            _ => self,
        }
    }

    /// As an image for egui to draw.
    pub fn image(&self) -> egui::ColorImage {
        egui::ColorImage::from_gray([self.width, self.height], &self.luma)
    }
}

/// How the camera is doing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    /// Asked for, and not giving frames yet: the browser or Android may be asking for leave.
    Starting,
    Running,
    /// It stopped, or never started, and why.
    Failed(String),
}

/// What the camera's side and the app's share: how it is doing, and the newest frame.
#[derive(Debug)]
pub(crate) struct Shared {
    state: State,
    frame: Option<Frame>,
    /// Set as the app drops the camera: whatever runs it stops.
    stopped: bool,
}

/// Where frames are handed to: the app's camera and whatever runs it share it.
pub(crate) type Slot = Arc<Mutex<Shared>>;

fn lock(slot: &Slot) -> std::sync::MutexGuard<'_, Shared> {
    slot.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A camera, running until it is dropped.
pub struct Camera {
    slot: Slot,
    #[cfg(target_arch = "wasm32")]
    web: std::rc::Rc<std::cell::RefCell<Option<web::Live>>>,
    #[cfg(target_os = "android")]
    ctx: egui::Context,
}

impl Camera {
    /// Opens the camera, for this app's frames: it starts in the background, and each frame wakes
    /// the app.
    pub fn open(ctx: &egui::Context) -> Camera {
        let slot: Slot = Arc::new(Mutex::new(Shared {
            state: State::Starting,
            frame: None,
            stopped: false,
        }));

        #[cfg(target_arch = "wasm32")]
        {
            let web = std::rc::Rc::default();
            web::open(ctx, slot.clone(), std::rc::Rc::clone(&web));

            Camera { slot, web }
        }

        #[cfg(target_os = "android")]
        {
            android::want(ctx, Some(slot.clone()));

            Camera { slot, ctx: ctx.clone() }
        }

        #[cfg(not(any(target_arch = "wasm32", target_os = "android", target_os = "ios")))]
        {
            desktop::open(ctx.clone(), slot.clone());

            Camera { slot }
        }

        #[cfg(target_os = "ios")]
        {
            ios::open(ctx.clone(), slot.clone());

            Camera { slot }
        }
    }

    pub fn state(&self) -> State {
        lock(&self.slot).state.clone()
    }

    /// The newest frame since the last one taken.
    pub fn frame(&mut self) -> Option<Frame> {
        #[cfg(target_arch = "wasm32")]
        if let Some(live) = self.web.borrow_mut().as_mut() {
            live.grab(&self.slot);
        }

        lock(&self.slot).frame.take()
    }
}

impl Drop for Camera {
    fn drop(&mut self) {
        lock(&self.slot).stopped = true;

        #[cfg(target_arch = "wasm32")]
        if let Some(live) = self.web.borrow_mut().take() {
            live.stop();
        }

        #[cfg(target_os = "android")]
        android::want(&self.ctx, None);

        #[cfg(target_os = "ios")]
        ios::stop();
    }
}

impl std::fmt::Debug for Camera {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Camera").field("state", &self.state()).finish_non_exhaustive()
    }
}

/// Hands a frame over, unless the app dropped the camera; false once it did.
fn hand(slot: &Slot, ctx: &egui::Context, frame: Frame) -> bool {
    let mut shared = lock(slot);

    if shared.stopped {
        return false;
    }

    shared.state = State::Running;
    shared.frame = Some(frame);
    drop(shared);
    ctx.request_repaint();

    true
}

fn fail(slot: &Slot, ctx: &egui::Context, why: String) {
    lock(slot).state = State::Failed(why);
    ctx.request_repaint();
}

/// The first webcam, on a thread of its own, through nokhwa: AVFoundation on macOS, V4L2 on Linux,
/// Media Foundation on Windows.
#[cfg(not(any(target_arch = "wasm32", target_os = "android", target_os = "ios")))]
mod desktop {
    use {
        super::{Frame, Slot, fail, hand, lock},
        nokhwa::pixel_format::LumaFormat,
        nokhwa::utils::{CameraIndex, RequestedFormat, RequestedFormatType},
    };

    pub(super) fn open(ctx: egui::Context, slot: Slot) {
        std::thread::spawn(move || {
            if let Err(why) = run(&ctx, &slot) {
                fail(&slot, &ctx, why);
            }
        });
    }

    fn run(ctx: &egui::Context, slot: &Slot) -> Result<(), String> {
        allowed()?;
        let format = RequestedFormat::new::<LumaFormat>(RequestedFormatType::AbsoluteHighestFrameRate);
        let mut camera = nokhwa::Camera::new(CameraIndex::Index(0), format).map_err(|err| format!("no camera to open: {err}"))?;
        camera.open_stream().map_err(|err| format!("the camera would not start: {err}"))?;

        loop {
            if lock(slot).stopped {
                break;
            }

            let buffer = camera.frame().map_err(|err| format!("the camera stopped: {err}"))?;
            let image = buffer.decode_image::<LumaFormat>().map_err(|err| format!("a frame that does not read: {err}"))?;
            let (width, height) = (image.width() as usize, image.height() as usize);

            if !hand(slot, ctx, Frame { width, height, luma: image.into_raw() }) {
                break;
            }
        }

        camera.stop_stream().map_err(|err| err.to_string())
    }

    /// macOS asks whether the app may use the camera the first time; the answer comes back later.
    #[cfg(target_os = "macos")]
    fn allowed() -> Result<(), String> {
        let (sent, answer) = std::sync::mpsc::channel();
        nokhwa::nokhwa_initialize(move |granted| {
            let _ = sent.send(granted);
        });

        match answer.recv_timeout(std::time::Duration::from_secs(60)) {
            Ok(true) => Ok(()),
            Ok(false) => Err("the camera is not allowed: let this app use it in System Settings, Privacy & Security, Camera".to_string()),
            Err(_) => Err("no answer about the camera".to_string()),
        }
    }

    #[cfg(not(target_os = "macos"))]
    fn allowed() -> Result<(), String> {
        Ok(())
    }
}

/// getUserMedia: the back camera where there is one, into a video element the page does not show,
/// read through a canvas as the app asks for frames.
#[cfg(target_arch = "wasm32")]
mod web {
    use {
        super::{Frame, Slot, fail, hand},
        std::cell::RefCell,
        std::rc::Rc,
        wasm_bindgen::{JsCast, JsValue},
        web_sys::{CanvasRenderingContext2d, HtmlCanvasElement, HtmlVideoElement, MediaStream, MediaStreamConstraints},
    };

    /// The longest side a frame is read at: enough for a code that fills a third of the view.
    const LONGEST: f64 = 960.0;
    /// The least time between frames read, in milliseconds: reading one takes the page's thread.
    const EVERY_MS: f64 = 90.0;

    pub(super) struct Live {
        stream: MediaStream,
        video: HtmlVideoElement,
        canvas: HtmlCanvasElement,
        context: CanvasRenderingContext2d,
        ctx: egui::Context,
        read_at: f64,
    }

    pub(super) fn open(ctx: &egui::Context, slot: Slot, live: Rc<RefCell<Option<Live>>>) {
        let ctx = ctx.clone();

        wasm_bindgen_futures::spawn_local(async move {
            match start(&ctx).await {
                Ok(started) => *live.borrow_mut() = Some(started),
                Err(why) => fail(&slot, &ctx, why),
            }
        });
    }

    async fn start(ctx: &egui::Context) -> Result<Live, String> {
        let window = web_sys::window().ok_or("no window")?;
        let devices = window.navigator().media_devices().map_err(|_| "this browser reaches no camera here: it takes https or localhost".to_string())?;
        let constraints = MediaStreamConstraints::new();
        let video = js_sys::Object::new();
        let facing = js_sys::Object::new();
        js_sys::Reflect::set(&facing, &"ideal".into(), &"environment".into()).map_err(shown)?;
        js_sys::Reflect::set(&video, &"facingMode".into(), &facing).map_err(shown)?;
        constraints.set_video(&video);
        let asked = devices.get_user_media_with_constraints(&constraints).map_err(shown)?;
        let stream: MediaStream = wasm_bindgen_futures::JsFuture::from(asked).await.map_err(refused)?.unchecked_into();
        let document = window.document().ok_or("no document")?;
        let element: HtmlVideoElement = document.create_element("video").map_err(shown)?.unchecked_into();
        element.set_muted(true);
        element.set_attribute("playsinline", "").map_err(shown)?;
        // In the page, so it plays, but out of sight: the app draws the frames itself.
        element.set_attribute("style", "position: fixed; width: 1px; height: 1px; opacity: 0; pointer-events: none;").map_err(shown)?;
        element.set_src_object(Some(&stream));
        document.body().ok_or("no body")?.append_child(&element).map_err(shown)?;
        let _ = element.play().map(wasm_bindgen_futures::JsFuture::from);
        let canvas: HtmlCanvasElement = document.create_element("canvas").map_err(shown)?.unchecked_into();
        let context: CanvasRenderingContext2d = canvas
            .get_context_with_context_options("2d", &{
                let options = js_sys::Object::new();
                js_sys::Reflect::set(&options, &"willReadFrequently".into(), &true.into()).map_err(shown)?;
                options.into()
            })
            .map_err(shown)?
            .ok_or("no canvas to read frames with")?
            .unchecked_into();
        ctx.request_repaint();

        Ok(Live {
            stream,
            video: element,
            canvas,
            context,
            ctx: ctx.clone(),
            read_at: 0.0,
        })
    }

    impl Live {
        /// Reads the frame the video shows now, if it shows one and enough time went by.
        pub(super) fn grab(&mut self, slot: &Slot) {
            let now = js_sys::Date::now();

            if now - self.read_at < EVERY_MS || self.video.ready_state() < 2 || self.video.video_width() == 0 {
                self.ctx.request_repaint_after(std::time::Duration::from_millis(EVERY_MS as u64));

                return;
            }

            self.read_at = now;
            let (shot_width, shot_height) = (f64::from(self.video.video_width()), f64::from(self.video.video_height()));
            let scale = (LONGEST / shot_width.max(shot_height)).min(1.0);
            let (width, height) = ((shot_width * scale).round() as u32, (shot_height * scale).round() as u32);
            self.canvas.set_width(width);
            self.canvas.set_height(height);

            if self.context.draw_image_with_html_video_element_and_dw_and_dh(&self.video, 0.0, 0.0, f64::from(width), f64::from(height)).is_err() {
                return;
            }

            let Ok(data) = self.context.get_image_data(0.0, 0.0, f64::from(width), f64::from(height)) else {
                return;
            };
            let luma = data
                .data()
                .as_chunks::<4>()
                .0
                .iter()
                .map(|pixel| ((u32::from(pixel[0]) * 77 + u32::from(pixel[1]) * 150 + u32::from(pixel[2]) * 29) >> 8) as u8)
                .collect();
            hand(slot, &self.ctx, Frame { width: width as usize, height: height as usize, luma });
            self.ctx.request_repaint_after(std::time::Duration::from_millis(EVERY_MS as u64));
        }

        pub(super) fn stop(self) {
            for track in self.stream.get_tracks().iter() {
                track.unchecked_into::<web_sys::MediaStreamTrack>().stop();
            }

            self.video.remove();
        }
    }

    fn shown(err: JsValue) -> String {
        err.as_string().or_else(|| js_sys::Reflect::get(&err, &"message".into()).ok().and_then(|message| message.as_string())).unwrap_or_else(|| "the camera failed".to_string())
    }

    /// Why getUserMedia gave no stream, as people read it.
    fn refused(err: JsValue) -> String {
        let name = js_sys::Reflect::get(&err, &"name".into()).ok().and_then(|name| name.as_string()).unwrap_or_default();

        match name.as_str() {
            "NotAllowedError" => "the camera is not allowed: let this page use it, in the browser's site settings. A browser extension's side panel cannot ask: open the wallet in a tab once, and allow it there".to_string(),
            "NotFoundError" | "OverconstrainedError" => "this device has no camera".to_string(),
            "NotReadableError" => "another app is using the camera".to_string(),
            _ => shown(err),
        }
    }
}

/// On Android the camera is the APK's Java class, which only the app's Android glue can reach: a
/// camera asked for here is picked up there each frame, which starts it, asks for leave first if it
/// must, and hands each frame back.
#[cfg(any(target_os = "android", test))]
pub(crate) mod android {
    use super::{Frame, Slot, State, fail, hand, lock};

    #[derive(Clone, Default)]
    struct Wanted(std::sync::Arc<std::sync::Mutex<Option<Slot>>>);

    fn wanted(ctx: &egui::Context) -> Wanted {
        ctx.data_mut(|data| data.get_temp_mut_or_default::<Wanted>(egui::Id::unique("chainv4-ui camera")).clone())
    }

    /// Asks for the camera, for this slot's frames, or with None stops it.
    pub(crate) fn want(ctx: &egui::Context, slot: Option<Slot>) {
        *wanted(ctx).0.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = slot;
        ctx.request_repaint();
    }

    /// The slot a camera is wanted for, if one is.
    pub(crate) fn wanted_slot(ctx: &egui::Context) -> Option<Slot> {
        wanted(ctx).0.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    /// Says the camera waits for Android's leave, which the person gives in its prompt.
    pub(crate) fn asking(slot: &Slot, ctx: &egui::Context) {
        let mut shared = lock(slot);

        if shared.state != State::Running {
            shared.state = State::Starting;
        }

        drop(shared);
        ctx.request_repaint_after(std::time::Duration::from_millis(300));
    }

    /// Hands a frame from Java over: its luma, its size, and how far to turn it upright.
    pub(crate) fn frame(slot: &Slot, ctx: &egui::Context, luma: Vec<u8>, width: usize, height: usize, rotation: i32) {
        if width == 0 || luma.len() != width * height {
            return;
        }

        let frame = Frame { width, height, luma }.turned(rotation.rem_euclid(360) as u32 / 90);
        hand(slot, ctx, frame);
    }

    pub(crate) fn failed(slot: &Slot, ctx: &egui::Context, why: String) {
        fail(slot, ctx, why);
    }
}

#[cfg(test)]
mod tests;

/// The back camera through AVFoundation: a capture session on the app's own thread, its frames
/// delivered on a queue of their own, each one's luma copied out, the first plane of the
/// bi-planar YUV iOS gives, or worked out from BGRA, and turned upright, as the sensor lies
/// sideways in a phone held up.
#[cfg(target_os = "ios")]
mod ios {
    use {
        super::{Frame, Slot, fail, hand},
        dispatch2::{DispatchQueue, DispatchRetained},
        objc2::rc::Retained,
        objc2::runtime::{Bool, ProtocolObject},
        objc2::{AllocAnyThread, DefinedClass, define_class, msg_send},
        objc2_av_foundation::{
            AVAuthorizationStatus, AVCaptureConnection, AVCaptureDevice, AVCaptureDeviceInput, AVCaptureOutput, AVCaptureSession, AVCaptureSessionPreset640x480,
            AVCaptureVideoDataOutput, AVCaptureVideoDataOutputSampleBufferDelegate, AVMediaTypeVideo,
        },
        objc2_core_media::CMSampleBuffer,
        objc2_core_video::{
            CVPixelBufferGetBaseAddress, CVPixelBufferGetBaseAddressOfPlane, CVPixelBufferGetBytesPerRow, CVPixelBufferGetBytesPerRowOfPlane, CVPixelBufferGetHeight,
            CVPixelBufferGetHeightOfPlane, CVPixelBufferGetWidth, CVPixelBufferGetWidthOfPlane, CVPixelBufferIsPlanar, CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags,
            CVPixelBufferUnlockBaseAddress,
        },
        objc2_foundation::{NSObject, NSObjectProtocol},
        std::cell::RefCell,
    };

    /// The session running, its delegate and its queue, kept on the app's thread until stopped.
    struct Live {
        session: Retained<AVCaptureSession>,
        _delegate: Retained<Delegate>,
        queue: DispatchRetained<DispatchQueue>,
    }

    std::thread_local! {
        static LIVE: RefCell<Option<Live>> = const { RefCell::new(None) };
    }

    /// The session, for its own queue to start and stop: AVFoundation lets any thread do that.
    struct Session(Retained<AVCaptureSession>);

    // SAFETY: startRunning and stopRunning may be called from any thread, as Apple documents; they
    // are only ever called from the camera's own serial queue.
    unsafe impl Send for Session {}

    impl Session {
        // Taking self whole, so a closure moves the Session, not its field.
        fn start(self) {
            // SAFETY: as above.
            unsafe { self.0.startRunning() };
        }

        fn stop(self) {
            // SAFETY: as above.
            unsafe { self.0.stopRunning() };
        }
    }

    pub(super) struct Ivars {
        slot: Slot,
        ctx: egui::Context,
    }

    define_class!(
        #[unsafe(super(NSObject))]
        #[name = "Chainv4CameraDelegate"]
        #[ivars = Ivars]
        struct Delegate;

        unsafe impl NSObjectProtocol for Delegate {}

        unsafe impl AVCaptureVideoDataOutputSampleBufferDelegate for Delegate {
            #[unsafe(method(captureOutput:didOutputSampleBuffer:fromConnection:))]
            fn captured(&self, _output: &AVCaptureOutput, buffer: &CMSampleBuffer, _connection: &AVCaptureConnection) {
                if let Some(frame) = luma(buffer) {
                    hand(&self.ivars().slot, &self.ivars().ctx, frame.turned(1));
                }
            }
        }
    );

    impl Delegate {
        fn new(slot: Slot, ctx: egui::Context) -> Retained<Delegate> {
            let this = Delegate::alloc().set_ivars(Ivars { slot, ctx });

            // SAFETY: NSObject's init, on a fresh instance.
            unsafe { msg_send![super(this), init] }
        }
    }

    /// A frame's grey, row after row: the luma plane of the bi-planar YUV iOS gives by default, or
    /// worked out from BGRA should it give that.
    fn luma(buffer: &CMSampleBuffer) -> Option<Frame> {
        // SAFETY: a video sample buffer, as the delegate gets it, holds an image buffer.
        let image = unsafe { buffer.image_buffer() }?;
        // SAFETY: the image buffer is alive while it is locked, read and unlocked here.
        unsafe { CVPixelBufferLockBaseAddress(&image, CVPixelBufferLockFlags::ReadOnly) };
        let planar = CVPixelBufferIsPlanar(&image);
        let (width, height, row, base) = match planar {
            true => (CVPixelBufferGetWidthOfPlane(&image, 0), CVPixelBufferGetHeightOfPlane(&image, 0), CVPixelBufferGetBytesPerRowOfPlane(&image, 0), CVPixelBufferGetBaseAddressOfPlane(&image, 0)),
            false => (CVPixelBufferGetWidth(&image), CVPixelBufferGetHeight(&image), CVPixelBufferGetBytesPerRow(&image), CVPixelBufferGetBaseAddress(&image)),
        };
        let mut luma = Vec::with_capacity(width * height);

        if !base.is_null() {
            for y in 0..height {
                // SAFETY: the base address is locked, and each row holds bytes_per_row bytes, at
                // least a byte a pixel in the luma plane and four in BGRA.
                let line = unsafe { std::slice::from_raw_parts(base.cast::<u8>().add(y * row), row) };

                match planar {
                    true => luma.extend_from_slice(&line[..width]),
                    false => luma.extend(line.chunks_exact(4).take(width).map(|bgra| ((u32::from(bgra[2]) * 77 + u32::from(bgra[1]) * 150 + u32::from(bgra[0]) * 29) >> 8) as u8)),
                }
            }
        }

        // SAFETY: as it was locked above.
        unsafe { CVPixelBufferUnlockBaseAddress(&image, CVPixelBufferLockFlags::ReadOnly) };

        (luma.len() == width * height && width > 0).then_some(Frame { width, height, luma })
    }

    pub(super) fn open(ctx: egui::Context, slot: Slot) {
        // SAFETY: AVFoundation's own constant.
        let Some(video) = (unsafe { AVMediaTypeVideo }) else {
            return fail(&slot, &ctx, "this system has no video".to_string());
        };

        // SAFETY: asking, and asking for, the camera's permission takes no other state.
        match unsafe { AVCaptureDevice::authorizationStatusForMediaType(video) } {
            AVAuthorizationStatus::Authorized => start(ctx, slot),
            AVAuthorizationStatus::NotDetermined => {
                let asked = block2::RcBlock::new(move |granted: Bool| {
                    let (ctx, slot) = (ctx.clone(), slot.clone());

                    // Started on the app's thread, which keeps the session.
                    DispatchQueue::main().exec_async(move || match granted.as_bool() {
                        true => start(ctx, slot),
                        false => fail(&slot, &ctx, denied()),
                    });
                });
                unsafe { AVCaptureDevice::requestAccessForMediaType_completionHandler(video, &asked) };
            }
            _ => fail(&slot, &ctx, denied()),
        }
    }

    fn denied() -> String {
        "the camera is not allowed: let this app use it in Settings, Privacy & Security, Camera".to_string()
    }

    fn start(ctx: egui::Context, slot: Slot) {
        if let Err(why) = started(&ctx, &slot) {
            fail(&slot, &ctx, why);
        }
    }

    fn started(ctx: &egui::Context, slot: &Slot) -> Result<(), String> {
        stop();
        // SAFETY: a fresh session and output, set up before they run, on the app's thread.
        unsafe {
            let video = AVMediaTypeVideo.ok_or("this system has no video")?;
            let session = AVCaptureSession::new();
            session.setSessionPreset(AVCaptureSessionPreset640x480);
            let device = AVCaptureDevice::defaultDeviceWithMediaType(video).ok_or("no camera to open")?;
            let input = AVCaptureDeviceInput::deviceInputWithDevice_error(&device).map_err(|err| format!("the camera would not open: {}", err.localizedDescription()))?;

            if !session.canAddInput(&input) {
                return Err("the camera would not start".to_string());
            }

            session.addInput(&input);
            let output = AVCaptureVideoDataOutput::new();
            output.setAlwaysDiscardsLateVideoFrames(true);
            let queue = DispatchQueue::new("org.chainv4.ui.camera", None);
            let delegate = Delegate::new(slot.clone(), ctx.clone());
            output.setSampleBufferDelegate_queue(Some(ProtocolObject::from_ref(&*delegate)), Some(&queue));

            if !session.canAddOutput(&output) {
                return Err("the camera would not give frames".to_string());
            }

            session.addOutput(&output);
            let running = Session(session.clone());
            // startRunning blocks until the camera runs: off the app's thread.
            queue.exec_async(move || running.start());
            LIVE.with(|live| *live.borrow_mut() = Some(Live { session, _delegate: delegate, queue }));
        }

        Ok(())
    }

    /// Stops the session running, if one is.
    pub(super) fn stop() {
        if let Some(live) = LIVE.with(|live| live.borrow_mut().take()) {
            let stopping = Session(live.session);
            live.queue.exec_async(move || stopping.stop());
        }
    }
}

