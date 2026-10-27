//! Reading what the other device shows: its QR codes live, with the camera, as fast as it shows
//! them; or a photo of its code, picked as an image, a file holding the code or its text, or the
//! text pasted. Every code is a Uniform Resource, this wallet's, MetaMask's and Bitcoin wallets'
//! alike; one too long for a code animates through fountain-coded parts, and any enough of them,
//! in any order, make the message.
//!
//! The camera's frames are searched off the UI thread, where there are threads, the newest at a
//! time; in the browser, between frames.

use {
    crate::airgap::{self, Message},
    crate::hardware,
    crate::theme::{self, Palette},
    chainv4_ui::camera::{Camera, Frame, State},
    chainv4_ui::files::{self, Pick},
    egui::{RichText, Ui},
    std::sync::{Arc, Mutex, PoisonError},
};

/// The widest the camera's view is drawn.
const VIEW_WIDTH: f32 = 420.0;

/// What a photo or file came to.
enum Read {
    /// The code's text.
    Got(String),
    Failed(String),
    /// Nothing was taken or picked.
    Nothing,
}

#[derive(Default)]
pub struct Scan {
    inbox: Arc<Mutex<Option<Read>>>,
    /// The parts of an animated code read so far.
    assembly: hardware::Assembly,
    pasted: String,
    reading: bool,
    error: Option<String>,
    /// The camera, while it is open.
    live: Option<Live>,
}

/// The camera open, what shows of it, and what searches its frames.
struct Live {
    camera: Camera,
    view: Option<egui::TextureHandle>,
    search: Search,
    /// The last code read, so one held in view is taken once.
    last: String,
}

/// Searches frames for codes: on a thread of its own, the newest frame at a time, where there are
/// threads.
#[cfg(not(target_arch = "wasm32"))]
struct Search {
    frames: std::sync::mpsc::SyncSender<Frame>,
    found: Arc<Mutex<Vec<String>>>,
}

#[cfg(not(target_arch = "wasm32"))]
impl Search {
    fn start(wake: egui::Context) -> Search {
        let (frames, taken) = std::sync::mpsc::sync_channel::<Frame>(1);
        let found: Arc<Mutex<Vec<String>>> = Arc::default();
        let into = found.clone();

        std::thread::spawn(move || {
            // Ends once the camera, and so the sender, is dropped.
            for frame in taken {
                if let Some(text) = airgap::code_in_frame(frame.width, frame.height, &frame.luma) {
                    into.lock().unwrap_or_else(PoisonError::into_inner).push(text);
                    wake.request_repaint();
                }
            }
        });

        Search { frames, found }
    }

    /// Hands a frame on, unless the last is still being searched: then it is dropped.
    fn give(&mut self, frame: Frame) {
        let _ = self.frames.try_send(frame);
    }

    fn found(&mut self) -> Vec<String> {
        std::mem::take(&mut *self.found.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

/// In the browser, on the page's own thread, between frames, as often as it can spare.
#[cfg(target_arch = "wasm32")]
struct Search {
    found: Vec<String>,
    searched_at: f64,
}

#[cfg(target_arch = "wasm32")]
impl Search {
    /// The least time between frames searched, in milliseconds.
    const EVERY_MS: f64 = 180.0;

    fn start(_: egui::Context) -> Search {
        Search { found: Vec::new(), searched_at: 0.0 }
    }

    fn give(&mut self, frame: Frame) {
        let now = js_sys::Date::now();

        if now - self.searched_at < Self::EVERY_MS {
            return;
        }

        self.searched_at = now;
        self.found.extend(airgap::code_in_frame(frame.width, frame.height, &frame.luma));
    }

    fn found(&mut self) -> Vec<String> {
        std::mem::take(&mut self.found)
    }
}

impl Scan {
    /// Draws the ways in, and gives the message once one is read.
    pub fn show(&mut self, ui: &mut Ui, palette: &Palette) -> Option<Message> {
        let arrived = self.inbox.lock().unwrap_or_else(PoisonError::into_inner).take();

        if let Some(read) = arrived {
            self.reading = false;

            match read {
                Read::Got(text) => {
                    if let Some(message) = self.read_text(&text) {
                        return Some(message);
                    }
                }
                Read::Failed(why) => self.error = Some(why),
                Read::Nothing => {}
            }
        }

        if self.live.is_some() {
            if let Some(message) = self.film(ui, palette) {
                return Some(message);
            }
        } else {
            ui.horizontal_wrapped(|ui| {
                if theme::primary(ui, palette, "Scan with the camera", true).clicked() {
                    self.error = None;
                    self.live = Some(Live {
                        camera: Camera::open(ui.ctx()),
                        view: None,
                        search: Search::start(ui.ctx().clone()),
                        last: String::new(),
                    });
                }
            });
            ui.label(RichText::new("Hold the other device's code in view; a message in parts is read as it shows each.").small().color(palette.muted));
            ui.add_space(8.0);
        }

        self.pickers(ui, palette);
        self.progress(ui, palette);

        ui.label(RichText::new("Or paste its text, as Copy as text gave it:").small().color(palette.muted));
        let read = super::beside(
            ui,
            |ui| theme::quiet(ui, palette, "Read").clicked(),
            |ui, width| {
                ui.add(egui::TextEdit::singleline(&mut self.pasted).hint_text("ur:…").font(egui::TextStyle::Monospace).desired_width(width));
            },
        );

        if read {
            let pasted = std::mem::take(&mut self.pasted);

            if let Some(message) = self.read_text(&pasted) {
                return Some(message);
            }

            // Kept to fix, when it did not read.
            if self.error.is_some() {
                self.pasted = pasted;
            }
        }

        if let Some(error) = &self.error {
            ui.colored_label(palette.bad, error.as_str());
        }

        None
    }

    /// The camera's view, and what it read: a message, once every part of it is in.
    fn film(&mut self, ui: &mut Ui, palette: &Palette) -> Option<Message> {
        let live = self.live.as_mut()?;
        let state = live.camera.state();

        if let Some(frame) = live.camera.frame() {
            let image = frame.image();

            match &mut live.view {
                Some(view) => view.set(image, egui::TextureOptions::LINEAR),
                None => live.view = Some(ui.ctx().load_texture("camera", image, egui::TextureOptions::LINEAR)),
            }

            live.search.give(frame);
        }

        let mut stop = false;

        theme::card(ui, palette, |ui| {
            match (&state, &live.view) {
                (State::Failed(why), _) => {
                    ui.colored_label(palette.bad, why.as_str());
                }
                (_, Some(view)) => {
                    let size = view.size_vec2();
                    let width = ui.available_width().min(VIEW_WIDTH);
                    ui.vertical_centered(|ui| ui.add(egui::Image::new(view).fit_to_exact_size(egui::vec2(width, width * size.y / size.x.max(1.0))).corner_radius(10.0)));
                }
                (State::Starting, None) => super::waiting(ui, palette, "Opening the camera… allow it if asked."),
                (State::Running, None) => super::waiting(ui, palette, "Waiting for the camera…"),
            }

            ui.add_space(6.0);

            if theme::quiet(ui, palette, "Stop the camera").clicked() {
                stop = true;
            }
        });

        let found = live.search.found();
        let mut message = None;

        for text in found {
            let live = self.live.as_mut()?;

            if text == live.last {
                continue;
            }

            live.last = text.clone();

            // A code of something else in view: said, and the camera keeps looking.
            if !airgap::is_ur(&text) {
                self.error = Some("That QR code is not one of this wallet's, MetaMask's or a Bitcoin wallet's.".to_string());

                continue;
            }

            if let Some(read) = self.read_text(&text) {
                message = Some(read);

                break;
            }
        }

        if stop || message.is_some() {
            self.live = None;
        }

        message
    }

    fn pickers(&mut self, ui: &mut Ui, palette: &Palette) {
        if !files::can_pick(ui.ctx()) {
            return;
        }

        // On Android the camera is the live one: the system camera's photo would want the
        // permission the app now asks for itself.
        let photo = match () {
            _ if cfg!(target_os = "android") => None,
            _ if cfg!(target_arch = "wasm32") => Some("A photo"),
            _ => Some("Open a photo"),
        };

        ui.horizontal_wrapped(|ui| {
            if let Some(photo) = photo
                && theme::quiet(ui, palette, photo).on_hover_text("A photo or a screenshot of the code").clicked()
            {
                self.pick(ui.ctx(), true);
            }

            if theme::quiet(ui, palette, "Open a file").on_hover_text("An image of the code, or a text file holding it").clicked() {
                self.pick(ui.ctx(), false);
            }

            if self.reading {
                ui.spinner();
                ui.label(RichText::new("Reading…").color(palette.muted));
            }
        });
        ui.add_space(8.0);
    }

    /// How far a message in parts got.
    fn progress(&self, ui: &mut Ui, palette: &Palette) {
        if let Some((read, count)) = self.assembly.progress() {
            ui.colored_label(palette.accent, format!("Read {read} of {count} parts. Keep it in view: any parts will do."));
            ui.add(egui::ProgressBar::new(read as f32 / count.max(1) as f32).desired_height(6.0));
            ui.add_space(6.0);
        }
    }

    /// Says why what was read will not do.
    pub fn fail(&mut self, why: String) {
        self.error = Some(why);
    }

    /// Says a message of the wrong kind came.
    pub fn refuse(&mut self, got: &Message, wanted: &str) {
        self.error = Some(format!("That is {}, not {wanted}.", got.name()));
    }

    /// Takes a code's text, whole or a part of an animated one: this wallet's, MetaMask's or a
    /// Bitcoin wallet's. Gives the message once it is all in.
    fn read_text(&mut self, text: &str) -> Option<Message> {
        self.error = None;
        let read = match airgap::is_ur(text) {
            true => self.assembly.add(text).and_then(|done| done.map(|(kind, bytes)| Message::from_ur(&kind, &bytes)).transpose()),
            false => Err("not a code of this wallet's, MetaMask's or a Bitcoin wallet's: those start ur:".to_string()),
        };

        match read {
            Ok(message) => message,
            Err(why) => {
                self.error = Some(why);

                None
            }
        }
    }

    /// Opens a picker. What comes back is read on a thread of its own, where there are threads.
    fn pick(&mut self, ctx: &egui::Context, camera: bool) {
        self.reading = true;
        self.error = None;
        let (inbox, wake) = (self.inbox.clone(), ctx.clone());

        files::pick(ctx, Pick { camera, ..Pick::default() }, move |picked| {
            let read = move || {
                let read = match picked {
                    Ok(files) => match files.into_iter().next() {
                        Some(file) => match airgap::text_in(&file.bytes) {
                            Ok(text) => Read::Got(text),
                            Err(why) => Read::Failed(why),
                        },
                        None => Read::Nothing,
                    },
                    Err(why) => Read::Failed(why),
                };
                *inbox.lock().unwrap_or_else(PoisonError::into_inner) = Some(read);
                wake.request_repaint();
            };

            #[cfg(not(target_arch = "wasm32"))]
            std::thread::spawn(read);

            #[cfg(target_arch = "wasm32")]
            read();
        });
    }
}

impl std::fmt::Debug for Scan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Scan").field("reading", &self.reading).field("error", &self.error).field("live", &self.live.is_some()).finish_non_exhaustive()
    }
}
