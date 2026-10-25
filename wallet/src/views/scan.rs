//! Reading what the other device shows: a photo of its QR code, taken with the camera on Android or
//! picked as an image elsewhere, a file holding the code or its text, or the text pasted. A photo
//! is read off the UI thread, since a camera's takes a moment to search. A message shown as parts
//! is read a part at a time, in any order, until every one is in.

use {
    crate::airgap::{self, Assembly, Code, Message},
    crate::theme::{self, Palette},
    chainv4_ui::files::{self, Pick},
    egui::{RichText, Ui},
    std::sync::{Arc, Mutex, PoisonError},
};

/// What a photo or file came to.
enum Read {
    Got(Code),
    Failed(String),
    /// Nothing was taken or picked.
    Nothing,
}

#[derive(Default)]
pub struct Scan {
    inbox: Arc<Mutex<Option<Read>>>,
    /// The parts of a message read so far.
    assembly: Assembly,
    pasted: String,
    reading: bool,
    error: Option<String>,
}

impl Scan {
    /// Draws the ways in, and gives the message once one is read.
    pub fn show(&mut self, ui: &mut Ui, palette: &Palette) -> Option<Message> {
        let arrived = self.inbox.lock().unwrap_or_else(PoisonError::into_inner).take();

        if let Some(read) = arrived {
            self.reading = false;

            match read {
                Read::Got(code) => {
                    if let Some(message) = self.took(code) {
                        return Some(message);
                    }
                }
                Read::Failed(why) => self.error = Some(why),
                Read::Nothing => {}
            }
        }

        let can_pick = files::can_pick(ui.ctx());
        let photo = match () {
            _ if cfg!(target_os = "android") => "Take a photo",
            _ if cfg!(target_arch = "wasm32") => "Photo",
            _ => "Open a photo",
        };

        if can_pick {
            ui.horizontal_wrapped(|ui| {
                if theme::primary(ui, palette, photo, !self.reading).clicked() {
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
            ui.label(RichText::new("Hold the camera close enough that the code fills most of the photo, and steady.").small().color(palette.muted));
            ui.add_space(8.0);
        }

        if let (count, missing) = self.assembly.missing()
            && count > 0
        {
            let read = count - missing.len();
            let next = missing.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ");
            ui.colored_label(palette.accent, format!("Read {read} of {count} parts. Still to read: {next}."));
            ui.add_space(6.0);
        }

        ui.label(RichText::new("Or paste its text, as Copy as text gave it:").small().color(palette.muted));
        let read = super::beside(
            ui,
            |ui| theme::quiet(ui, palette, "Read").clicked(),
            |ui, width| {
                ui.add(egui::TextEdit::singleline(&mut self.pasted).hint_text("CV4:1:…").font(egui::TextStyle::Monospace).desired_width(width));
            },
        );

        if read {
            match Code::from_text(&self.pasted) {
                Ok(code) => {
                    self.pasted.clear();

                    if let Some(message) = self.took(code) {
                        return Some(message);
                    }
                }
                Err(why) => self.error = Some(why),
            }
        }

        if let Some(error) = &self.error {
            ui.colored_label(palette.bad, error.as_str());
        }

        None
    }

    /// Says why what was read will not do.
    pub fn fail(&mut self, why: String) {
        self.error = Some(why);
    }

    /// Says a message of the wrong kind came.
    pub fn refuse(&mut self, got: &Message, wanted: &str) {
        self.error = Some(format!("That is {}, not {wanted}.", got.name()));
    }

    /// Takes a code read: a whole message, or a part, which gives the message once it is the last.
    fn took(&mut self, code: Code) -> Option<Message> {
        self.error = None;

        match code {
            Code::Whole(message) => {
                self.assembly = Assembly::default();

                Some(*message)
            }
            Code::Part(part) => match self.assembly.add(part) {
                Ok(message) => message,
                Err(why) => {
                    self.error = Some(why);

                    None
                }
            },
        }
    }

    /// Opens the camera or a picker. What comes back is read on a thread of its own, where there
    /// are threads.
    fn pick(&mut self, ctx: &egui::Context, camera: bool) {
        self.reading = true;
        self.error = None;
        let (inbox, wake) = (self.inbox.clone(), ctx.clone());

        files::pick(ctx, Pick { camera, ..Pick::default() }, move |picked| {
            let read = move || {
                let read = match picked {
                    Ok(files) => match files.into_iter().next() {
                        Some(file) => match airgap::read(&file.bytes) {
                            Ok(code) => Read::Got(code),
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
        f.debug_struct("Scan").field("reading", &self.reading).field("error", &self.error).finish_non_exhaustive()
    }
}
