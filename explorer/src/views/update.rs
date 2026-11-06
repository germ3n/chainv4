//! The explorer's own updates: a notice over the page when the release the node relays names a
//! newer explorer, and a screen in place of every page once this one no longer works with the
//! network. On the desktop, Update downloads the release's binary for this target, off the UI
//! thread and through the API's own client (Api::download_client), with its proxy, and puts it in
//! place of this one.

use {
    super::Look,
    crate::release::{self, Update},
    chainv4::structs::version::Version,
    chainv4_ui::{net::Client, widgets},
    egui::{RichText, Ui},
    std::sync::{Arc, Mutex},
};

/// Whether this build can put a new binary in place of itself: on the desktop, not in the
/// browser, nor on a phone, whose store or sideloaded package updates it.
pub const DESKTOP: bool = cfg!(not(any(target_arch = "wasm32", target_os = "android", target_os = "ios")));

/// Where putting a new binary in place stands. Off the desktop it stays Idle.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(any(target_arch = "wasm32", target_os = "android", target_os = "ios"), allow(dead_code))]
pub enum Installing {
    #[default]
    Idle,
    Downloading { got: u64, size: u64 },
    /// In place: it runs once the explorer restarts.
    Installed(Version),
    Failed(String),
}

#[derive(Default)]
pub struct UpdateState {
    /// What the release the node last relayed says of this explorer: a newer one, or None.
    pub found: Option<Update>,
    /// Whether the newest release seen is past its expires: its host is behind, or this device's
    /// clock is wrong, and no notice can be trusted until one or the other is put right.
    pub lapsed: bool,
    /// The newest version whose notice was dismissed: a newer one shows again.
    dismissed: Option<Version>,
    /// Shared with the thread downloading.
    installing: Arc<Mutex<Installing>>,
}

impl UpdateState {
    /// Whether this explorer no longer works with the network, so the update screen stands in
    /// for every page.
    pub fn blocks(&self) -> bool {
        self.found.as_ref().is_some_and(Update::blocks)
    }

    fn installing(&self) -> Installing {
        lock(&self.installing).clone()
    }

    /// Downloads the binary for this target through client, and puts it in place of this one,
    /// on a thread of its own. Only the desktop does; elsewhere it does nothing.
    pub fn start(&mut self, client: Result<Client, String>, wake: egui::Context) {
        #[cfg(not(any(target_arch = "wasm32", target_os = "android", target_os = "ios")))]
        if let Some(update) = &self.found
            && let Some(file) = update.file.clone()
        {
            match client {
                Ok(client) => download(&client, file, update.newest(), self.installing.clone(), wake),
                Err(why) => *lock(&self.installing) = Installing::Failed(format!("the proxy set does not work: {why}")),
            }
        }

        #[cfg(any(target_arch = "wasm32", target_os = "android", target_os = "ios"))]
        let _ = (client, wake);
    }
}

/// Downloads a release's file and installs it over the running binary, telling installing how
/// far it got. The node's API client's own requests time out; a download's stream does not.
#[cfg(not(any(target_arch = "wasm32", target_os = "android", target_os = "ios")))]
fn download(client: &Client, file: chainv4::release::ReleaseFile, newest: Version, installing: Arc<Mutex<Installing>>, wake: egui::Context) {
    use {chainv4_ui::net::Piece, std::ops::ControlFlow};

    *lock(&installing) = Installing::Downloading { got: 0, size: file.size };
    let mut bytes = Vec::new();
    let url = file.url.clone();

    client.stream(url, move |piece| {
        let tell = |state: Installing| {
            *lock(&installing) = state;
            wake.request_repaint();
        };

        match piece {
            Ok(Piece::Head { ok: true, .. }) => {}
            Ok(Piece::Head { status, status_text, .. }) => {
                tell(Installing::Failed(format!("{} answered {status} {status_text}", file.url)));

                return ControlFlow::Break(());
            }
            Ok(Piece::Chunk(chunk)) if chunk.is_empty() => {
                let installed = std::env::current_exe()
                    .map_err(|err| format!("could not find the explorer's own binary: {err}"))
                    .and_then(|path| chainv4::release::install(&file, &bytes, &path));
                tell(match installed {
                    Ok(()) => Installing::Installed(newest),
                    Err(why) => Installing::Failed(why),
                });

                return ControlFlow::Break(());
            }
            Ok(Piece::Chunk(chunk)) => {
                bytes.extend_from_slice(&chunk);

                if bytes.len() as u64 > file.size {
                    tell(Installing::Failed(format!("the download from {} is bigger than the release's binary", file.url)));

                    return ControlFlow::Break(());
                }

                tell(Installing::Downloading { got: bytes.len() as u64, size: file.size });
            }
            Err(why) => {
                tell(Installing::Failed(why));

                return ControlFlow::Break(());
            }
        }

        ControlFlow::Continue(())
    });
}

/// The notice over the page, while a newer explorer is out that this one still works beside, or
/// one is downloading or in place. Returns whether Update was pressed.
pub fn notice(ui: &mut Ui, look: Look, state: &mut UpdateState) -> bool {
    let installing = state.installing();
    if state.lapsed {
        ui.label(RichText::new("The newest release this explorer knows of has expired, so it cannot say whether it is up to date: its node's host is behind, or this device's clock is wrong.").small().color(look.palette.warn));
    }

    let Some(update) = state.found.clone().filter(|update| !update.blocks()) else {
        return false;
    };

    if installing == Installing::Idle && state.dismissed == Some(update.newest()) {
        return false;
    }

    let palette = look.palette;
    let mut pressed = false;

    egui::Frame::new()
        .fill(palette.accent.gamma_multiply(0.12))
        .stroke(egui::Stroke::new(1.0, palette.accent.gamma_multiply(0.5)))
        .corner_radius(ui.visuals().widgets.inactive.corner_radius)
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            widgets::end_row(
                ui,
                |ui| {
                    ui.add(egui::Label::new(RichText::new(format!("Explorer {} is out (this is {})", update.newest(), release::ours())).strong().color(palette.ink)).wrap());
                },
                |ui| {
                    if !matches!(installing, Installing::Downloading { .. }) && ui.small_button("Dismiss").clicked() {
                        state.dismissed = Some(update.newest());
                    }
                },
            );

            if !update.notes.is_empty() {
                super::note(ui, look, &update.notes);
            }

            ui.add_space(4.0);
            pressed = install(ui, look, &update, &installing);
        });
    ui.add_space(8.0);

    pressed
}

/// The screen in place of every page once this explorer no longer works with the network: what
/// to update to, Update on the desktop, and Quit. Returns whether Update was pressed.
pub fn blocking(ui: &mut Ui, look: Look, state: &UpdateState) -> bool {
    let Some(update) = state.found.clone().filter(Update::blocks) else {
        return false;
    };
    let installing = state.installing();
    let palette = look.palette;
    let least = match update.standing {
        chainv4::release::Standing::Unsupported { least, .. } => least,
        _ => update.newest(),
    };
    let mut pressed = false;

    ui.vertical_centered(|ui| {
        ui.set_max_width(ui.available_width().min(520.0));
        ui.add_space(ui.available_height() * 0.2);
        widgets::card(ui, &palette, |ui| {
            ui.label(widgets::strong("This explorer no longer works with the network", 19.0).color(palette.ink));
            ui.add_space(6.0);
            super::note(
                ui,
                look,
                format!("It is {}, and the network now takes {least} or newer. Explorer {} is out.", release::ours(), update.newest()),
            );

            if !update.notes.is_empty() {
                ui.add_space(4.0);
                ui.add(egui::Label::new(RichText::new(&update.notes).color(palette.soft)).wrap());
            }

            ui.add_space(10.0);
            pressed = install(ui, look, &update, &installing);

            // A browser tab is the browser's to close.
            if !cfg!(target_arch = "wasm32") {
                ui.add_space(8.0);

                if widgets::quiet(ui, &palette, "Quit").clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        });
    });

    pressed
}

/// Update and how it is going on the desktop, when the release has a binary for this target; how
/// to update anywhere else. Returns whether Update was pressed.
fn install(ui: &mut Ui, look: Look, update: &Update, installing: &Installing) -> bool {
    let palette = look.palette;

    if !DESKTOP || update.file.is_none() {
        super::note(ui, look, "Update it from where you installed it.");

        return false;
    }

    match installing {
        Installing::Idle => widgets::primary(ui, &palette, "Update", true).clicked(),
        Installing::Downloading { got, size } => {
            super::meter(
                ui,
                look,
                *got as f32 / (*size).max(1) as f32,
                &format!("Downloading {:.1} of {:.1} MB", *got as f64 / 1e6, *size as f64 / 1e6),
            );

            false
        }
        Installing::Installed(version) => {
            ui.label(RichText::new(format!("Restart the explorer to run {version}")).strong().color(palette.good));

            false
        }
        Installing::Failed(why) => {
            ui.add(egui::Label::new(RichText::new(why).color(palette.bad)).wrap());

            widgets::quiet(ui, &palette, "Try again").clicked()
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}
