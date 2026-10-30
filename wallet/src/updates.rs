//! Updates: the newest wallet, as the release the node relays says, checked here against the
//! release keys built in, as the node only passes it on. A wallet behind it says so, once, until
//! put away; one older than the oldest that still works with the network stops until it is
//! updated. On the desktop the wallet downloads its new binary through the proxy set, off the UI
//! thread, and puts it in place of the one running once its size and SHA-256 are what the keys
//! signed, for the next start; elsewhere it says to update it from where it was installed. A build
//! without release keys, as one from the checked-in release/keys.json, shows nothing.
//!
//! The node's own chainv4 is another matter: an upgrade that stops it at a height shows as a
//! warning where the node is set.

use {
    crate::app::Settings,
    crate::node::Failure,
    crate::theme::{self, Palette},
    chainv4::release::{ReleaseFile, ReleaseKeys, SignedRelease, Standing},
    chainv4::structs::version::{Upgrade, Version},
    egui::{RichText, Ui},
    serde_json::Value,
    std::sync::{Arc, Mutex},
};

/// Seconds between asking the node for the release again, and again after it did not answer.
const CHECK_EVERY: f64 = 3_600.0;
const RETRY: f64 = 60.0;

/// What a node from before releases answers: the same as no release.
const NOT_FOUND: u16 = 404;

/// The release's name for this app.
const PRODUCT: &str = "wallet";

/// Whether this build is the desktop's: one file the wallet may put a new one in place of. A
/// phone's app updates through its store, and the browser's page through whoever serves it.
const DESKTOP: bool = cfg!(not(any(target_arch = "wasm32", target_os = "android", target_os = "ios")));

/// What a release says of this wallet, once checked: a newer one is out, or this one no longer
/// works with the network.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    /// Behind or unsupported; a current wallet has no notice.
    pub standing: Standing,
    /// This wallet's version.
    pub ours: Version,
    pub notes: String,
    /// The newest one's binary for the target this build is for, if the release has one.
    pub file: Option<ReleaseFile>,
}

impl Notice {
    pub fn newest(&self) -> Version {
        match self.standing {
            Standing::Behind { newest } | Standing::Unsupported { newest, .. } => newest,
            Standing::Current => self.ours,
        }
    }

    /// Whether this wallet is older than the oldest that still works.
    pub fn unsupported(&self) -> bool {
        matches!(self.standing, Standing::Unsupported { .. })
    }
}

/// This wallet's version, from its own Cargo.toml: not chainv4's, which Version::ours is.
pub fn ours() -> Version {
    env!("CARGO_PKG_VERSION").parse().expect("Cargo.toml's version is major.minor.patch")
}

/// What to show of a release the node relayed, as GET /release's manifest: nothing without keys
/// to check it by, for no release, for one fewer of the keys signed than it takes, for one that
/// names no wallet, and for a wallet as new as its newest.
pub fn notice(manifest: &Value, keys: &ReleaseKeys, ours: Version) -> Option<Notice> {
    if !keys.on() || manifest.is_null() {
        return None;
    }

    let signed = SignedRelease::from_json(manifest).inspect_err(|why| log::warn!("the node's release does not read: {why}")).ok()?;
    signed.verify(keys).inspect_err(|why| log::warn!("the node's release is not taken: {why}")).ok()?;
    let product = signed.release.product(PRODUCT)?;

    match product.standing(ours) {
        Standing::Current => None,
        standing => Some(Notice {
            standing,
            ours,
            notes: product.notes.clone(),
            file: product.file(chainv4::release::TARGET).cloned(),
        }),
    }
}

/// The upgrade that stops the node's own chainv4, as GET /status or GET /release says: the one it
/// names, if the version it names beside it, its own, is older than the upgrade's least.
pub fn stopping(value: &Value) -> Option<Upgrade> {
    let version: Version = value["version"].as_str()?.parse().ok()?;
    let upgrade = Upgrade {
        least: value["upgrade"]["least"].as_str()?.parse().ok()?,
        from: value["upgrade"]["from"].as_u64()?,
    };

    upgrade.stops(version).then_some(upgrade)
}

/// The binary to put in place of this one: on the desktop alone.
fn installable(notice: &Notice) -> Option<&ReleaseFile> {
    notice.file.as_ref().filter(|_| DESKTOP)
}

/// Getting the new binary and putting it in place, on another thread.
#[derive(Default)]
struct Download {
    /// What came of it, once done: the version put in place, or why not.
    done: Arc<Mutex<Option<Result<Version, String>>>>,
    running: bool,
    outcome: Option<Result<Version, String>>,
}

impl Download {
    /// Downloads the file through the client, and puts it in place of the wallet's own binary.
    fn start(&mut self, file: ReleaseFile, newest: Version, client: Result<chainv4_ui::net::Client, String>, wake: &egui::Context) {
        if self.running {
            return;
        }

        self.running = true;
        self.outcome = None;
        let (done, wake, url) = (self.done.clone(), wake.clone(), file.url.clone());
        let finish = move |result: Result<(), String>| {
            *done.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(result.map(|()| newest));
            wake.request_repaint();
        };

        // A stream, not a fetch: the node's requests time out after 30 seconds, and a binary over
        // a slow link or Tor takes longer.
        match client {
            Ok(client) => {
                use {chainv4_ui::net::Piece, std::ops::ControlFlow};

                let mut bytes = Vec::new();
                let mut finish = Some(finish);

                client.stream(url, move |piece| {
                    let result = match piece {
                        Ok(Piece::Head { ok: true, .. }) => return ControlFlow::Continue(()),
                        Ok(Piece::Head { status, status_text, .. }) => Err(format!("{} answered {status} {status_text}", file.url)),
                        Ok(Piece::Chunk(chunk)) if chunk.is_empty() => put_in_place(&file, &bytes),
                        Ok(Piece::Chunk(chunk)) => {
                            bytes.extend_from_slice(&chunk);

                            if bytes.len() as u64 <= file.size {
                                return ControlFlow::Continue(());
                            }

                            Err(format!("the download from {} is bigger than the release's binary", file.url))
                        }
                        Err(message) => Err(message),
                    };

                    if let Some(finish) = finish.take() {
                        finish(result);
                    }

                    ControlFlow::Break(())
                });
            }
            Err(why) => finish(Err(format!("the proxy set does not work: {why}"))),
        }
    }

    /// Takes what came of it, once done.
    fn take(&mut self) {
        if let Some(outcome) = self.done.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take() {
            self.running = false;
            self.outcome = Some(outcome);
        }
    }
}

/// Puts a downloaded binary in place of the wallet's own, once it is the one the release signed.
#[cfg(not(target_arch = "wasm32"))]
fn put_in_place(file: &ReleaseFile, bytes: &[u8]) -> Result<(), String> {
    let path = std::env::current_exe().map_err(|err| format!("could not find the wallet's own binary: {err}"))?;

    chainv4::release::install(file, bytes, &path)
}

#[cfg(target_arch = "wasm32")]
fn put_in_place(_file: &ReleaseFile, _bytes: &[u8]) -> Result<(), String> {
    Err("the browser keeps no binary to put it in place of".to_string())
}

/// What the wallet knows of releases: its own standing, and whether its node stops at a height.
#[derive(Default)]
pub struct Updates {
    /// When to ask the node for the release next: None to ask now, as on connecting.
    next: Option<f64>,
    notice: Option<Notice>,
    /// The newest version whose notice was put away, for this run.
    dismissed: Option<Version>,
    /// The upgrades that stop the node, as its status and its release each say.
    status_stops: Option<Upgrade>,
    release_stops: Option<Upgrade>,
    download: Download,
}

impl Updates {
    /// Starts over with another node: what it relays is asked for at once.
    pub fn connected(&mut self) {
        self.next = None;
        self.notice = None;
        self.status_stops = None;
        self.release_stops = None;
    }

    /// Whether the release is due to be asked for, noting it asked.
    pub fn due(&mut self, time: f64) -> bool {
        if self.next.is_some_and(|next| time < next) {
            return false;
        }

        self.next = Some(time + CHECK_EVERY);

        true
    }

    /// Takes the node's GET /release, checked with these keys. A node from before releases has
    /// none to give; one that did not answer is asked again in a minute, and what it said before
    /// stands until then.
    pub fn answered(&mut self, result: &Result<Value, Failure>, keys: &ReleaseKeys, time: f64) {
        match result {
            Ok(value) => {
                self.notice = notice(&value["manifest"], keys, ours());
                self.release_stops = stopping(value);
            }
            Err(failure) if failure.status == NOT_FOUND => {
                self.notice = None;
                self.release_stops = None;
            }
            Err(failure) => {
                log::warn!("could not ask the node for its release: {}", failure.message);
                self.next = Some(time + RETRY);
            }
        }
    }

    /// Takes the node's GET /status, for the upgrade governance decided.
    pub fn status(&mut self, value: &Value) {
        self.status_stops = stopping(value);
    }

    /// Takes what came of a download.
    pub fn take(&mut self) {
        self.download.take();
    }

    /// The upgrade that stops the node first, if one does.
    pub fn stops(&self) -> Option<Upgrade> {
        self.status_stops.into_iter().chain(self.release_stops).min_by_key(|upgrade| upgrade.from)
    }

    /// Whether the wallet is too old to go on: the screen that says so takes the place of all
    /// else.
    pub fn blocks(&self) -> bool {
        self.notice.as_ref().is_some_and(Notice::unsupported)
    }

    /// Above every page, while a newer wallet is out and its notice was not put away.
    pub fn banner(&mut self, ui: &mut Ui, palette: &Palette, settings: &Settings, wake: &egui::Context) {
        let Some(notice) = self.notice.clone().filter(|notice| !notice.unsupported() && self.dismissed != Some(notice.newest())) else {
            return;
        };

        theme::card(ui, palette, |ui| {
            theme::caps(ui, palette, "Update");
            ui.label(theme::strong(format!("Wallet {} is out (this is {})", notice.newest(), notice.ours), 15.0).color(palette.ink));
            notes(ui, palette, &notice);
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                self.update(ui, palette, &notice, settings, wake);

                if theme::quiet(ui, palette, "Not now").on_hover_text("Put this away until the wallet starts again").clicked() {
                    self.dismissed = Some(notice.newest());
                }
            });
            self.outcome(ui, palette, &notice);
        });
    }

    /// In place of everything, while this wallet is older than the oldest that still works.
    pub fn blocking(&mut self, ui: &mut Ui, palette: &Palette, settings: &Settings, wake: &egui::Context) {
        let Some(notice) = self.notice.clone().filter(Notice::unsupported) else {
            return;
        };
        let Standing::Unsupported { least, .. } = notice.standing else {
            return;
        };
        let height = ui.available_height();

        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space((height * 0.16).max(24.0));
                ui.set_max_width(ui.available_width().min(420.0));
                ui.label(RichText::new("chainv4").font(egui::FontId::new(44.0, egui::FontFamily::Name(theme::SERIF.into()))).color(palette.ink));
                theme::caps(ui, palette, "Update needed");
                ui.add_space(26.0);
                ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                    theme::card(ui, palette, |ui| {
                        ui.label(theme::strong("This version of the wallet no longer works with the network.", 15.0).color(palette.ink));
                        ui.label(
                            RichText::new(format!("This is {}, and the oldest that still works is {least}. Wallet {} is the newest.", notice.ours, notice.newest()))
                                .color(palette.soft),
                        );
                        notes(ui, palette, &notice);
                        ui.add_space(8.0);
                        ui.horizontal_wrapped(|ui| {
                            self.update(ui, palette, &notice, settings, wake);

                            // A page in the browser is closed with its tab.
                            if !cfg!(target_arch = "wasm32") && theme::quiet(ui, palette, "Quit").clicked() {
                                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                        });
                        self.outcome(ui, palette, &notice);
                    });
                });
                ui.add_space(24.0);
            });
        });
    }

    /// The Update button, on the desktop with a binary for it, until the new one is in place.
    fn update(&mut self, ui: &mut Ui, palette: &Palette, notice: &Notice, settings: &Settings, wake: &egui::Context) {
        let Some(file) = installable(notice) else {
            return;
        };

        if matches!(self.download.outcome, Some(Ok(_))) {
            return;
        }

        if self.download.running {
            crate::views::waiting(ui, palette, &format!("Downloading wallet {}…", notice.newest()));

            return;
        }

        if theme::primary(ui, palette, "Update", true).on_hover_text(format!("Download it from {} and put it in place of this one", file.url)).clicked() {
            self.download.start(file.clone(), notice.newest(), crate::app::client(settings), wake);
        }
    }

    /// What came of updating, or where to update from.
    fn outcome(&self, ui: &mut Ui, palette: &Palette, notice: &Notice) {
        match (&self.download.outcome, installable(notice)) {
            (Some(Ok(version)), _) => {
                ui.colored_label(palette.good, format!("Restart the wallet to run {version}."));
            }
            (Some(Err(why)), _) => {
                ui.colored_label(palette.bad, format!("Could not update it: {why}"));
            }
            (None, Some(_)) => {}
            (None, None) if DESKTOP => {
                ui.label(RichText::new(format!("There is no build of it for {}: update it from where you installed it.", chainv4::release::TARGET)).small().color(palette.muted));
            }
            (None, None) => {
                ui.label(RichText::new("Update it from where you installed it.").small().color(palette.muted));
            }
        }
    }
}

/// What changed, as the release says.
fn notes(ui: &mut Ui, palette: &Palette, notice: &Notice) {
    if !notice.notes.trim().is_empty() {
        ui.add_space(4.0);
        ui.label(RichText::new(notice.notes.trim()).small().color(palette.soft));
    }
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        chainv4::crypto::keypair::Keypair,
        chainv4::release::{Product, Release, TARGET, sha256},
        serde_json::json,
    };

    fn key(seed: u8) -> Keypair {
        Keypair::from_seed([seed; 32])
    }

    fn keys(threshold: usize, seeds: &[u8]) -> ReleaseKeys {
        ReleaseKeys { threshold, keys: seeds.iter().map(|seed| key(*seed).public()).collect(), updates: None }
    }

    /// A release of wallet 0.2.0 that works back to 0.1.5, with a binary for these targets,
    /// signed by these keys, as GET /release's manifest carries it.
    fn manifest(targets: &[&str], signers: &[u8]) -> Value {
        let binary = b"the new wallet";
        let mut targets = targets.to_vec();
        targets.sort_unstable();
        let release = Release {
            issued: 1_800_000_000,
            products: vec![Product {
                name: "wallet".to_string(),
                version: Version::new(0, 2, 0),
                least: Version::new(0, 1, 5),
                notes: "Mining pools".to_string(),
                files: targets
                    .iter()
                    .map(|target| ReleaseFile { target: target.to_string(), url: format!("https://example.org/{target}/wallet"), sha256: sha256(binary), size: binary.len() as u64 })
                    .collect(),
            }],
            breaking: Vec::new(),
        };
        let mut signed = SignedRelease { release, signatures: Vec::new() };

        for seed in signers {
            signed.sign(&key(*seed));
        }

        signed.to_json()
    }

    #[test]
    fn without_release_keys_built_in_nothing_shows() {
        let signed = manifest(&[TARGET], &[1, 2]);

        assert_eq!(notice(&signed, &ReleaseKeys::default(), Version::new(0, 1, 0)), None, "not even an unsupported wallet");
        assert_eq!(notice(&Value::Null, &keys(1, &[1]), Version::new(0, 1, 0)), None, "a node that took no release");
    }

    #[test]
    fn a_release_fewer_of_the_keys_signed_than_it_takes_shows_nothing() {
        let trusted = keys(2, &[1, 2, 3]);

        assert_eq!(notice(&manifest(&[TARGET], &[1, 9]), &trusted, Version::new(0, 1, 0)), None, "a key not trusted counts for nothing");
        assert!(notice(&manifest(&[TARGET], &[1, 3]), &trusted, Version::new(0, 1, 0)).is_some());

        let mut changed = manifest(&[TARGET], &[1, 3]);
        changed["release"]["products"][0]["files"][0]["url"] = json!("https://elsewhere.example/wallet");

        assert_eq!(notice(&changed, &trusted, Version::new(0, 1, 0)), None, "the node changed where the binary is");
    }

    #[test]
    fn a_wallet_behind_the_newest_says_so_with_its_binary_and_notes() {
        let shown = notice(&manifest(&["riscv64gc-unknown-none-elf", TARGET], &[1]), &keys(1, &[1]), Version::new(0, 1, 7)).expect("a notice");

        assert_eq!(shown.standing, Standing::Behind { newest: Version::new(0, 2, 0) });
        assert_eq!((shown.newest(), shown.ours, shown.notes.as_str(), shown.unsupported()), (Version::new(0, 2, 0), Version::new(0, 1, 7), "Mining pools", false));
        assert_eq!(shown.file.map(|file| file.url), Some(format!("https://example.org/{TARGET}/wallet")));
    }

    #[test]
    fn a_wallet_older_than_the_least_is_unsupported() {
        let shown = notice(&manifest(&[TARGET], &[1]), &keys(1, &[1]), Version::new(0, 1, 4)).expect("a notice");

        assert_eq!(shown.standing, Standing::Unsupported { newest: Version::new(0, 2, 0), least: Version::new(0, 1, 5) });
        assert!(shown.unsupported());
        assert!(Updates { notice: Some(shown), ..Updates::default() }.blocks());
    }

    #[test]
    fn a_wallet_as_new_as_the_newest_or_newer_shows_nothing() {
        let signed = manifest(&[TARGET], &[1]);

        assert_eq!(notice(&signed, &keys(1, &[1]), Version::new(0, 2, 0)), None);
        assert_eq!(notice(&signed, &keys(1, &[1]), Version::new(0, 3, 1)), None);

        let mut explorer_only = signed;
        explorer_only["release"]["products"][0]["name"] = json!("explorer");
        let mut resigned = SignedRelease::from_json(&explorer_only).unwrap();
        resigned.sign(&key(1));

        assert_eq!(notice(&resigned.to_json(), &keys(1, &[1]), Version::new(0, 1, 0)), None, "a release naming no wallet");
    }

    #[test]
    fn without_a_binary_for_this_target_there_is_nothing_to_install() {
        let shown = notice(&manifest(&["riscv64gc-unknown-none-elf"], &[1]), &keys(1, &[1]), Version::new(0, 1, 7)).expect("still a notice");

        assert_eq!(shown.file, None);
        assert_eq!(installable(&shown), None);

        let with = notice(&manifest(&[TARGET], &[1]), &keys(1, &[1]), Version::new(0, 1, 7)).unwrap();

        assert_eq!(installable(&with).is_some(), DESKTOP, "the desktop alone puts a binary in place");
    }

    #[test]
    fn the_node_asked_for_its_release_takes_it_as_it_relays_it() {
        let mut updates = Updates::default();

        assert!(updates.due(0.0));
        assert!(!updates.due(CHECK_EVERY - 1.0), "an hour apart");

        let answer = json!({ "version": "0.1.0", "manifest": manifest(&[TARGET], &[1]), "upgrade": { "least": "0.2.0", "from": 150_000, "source": "release" } });
        updates.answered(&Ok(answer), &keys(1, &[1]), 10.0);

        assert_eq!(updates.notice.as_ref().map(Notice::newest), Some(Version::new(0, 2, 0)));
        assert_eq!(updates.stops(), Some(Upgrade { least: Version::new(0, 2, 0), from: 150_000 }));

        // A failure keeps what was said, and asks again in a minute; a node from before releases
        // has none.
        updates.answered(&Err(Failure { status: 0, message: "refused".to_string() }), &keys(1, &[1]), 20.0);

        assert!(updates.notice.is_some());
        assert!(!updates.due(20.0 + RETRY - 1.0) && updates.due(20.0 + RETRY));

        updates.answered(&Err(Failure { status: NOT_FOUND, message: "not found".to_string() }), &keys(1, &[1]), 30.0);

        assert_eq!((updates.notice.is_none(), updates.stops()), (true, None));

        updates.connected();

        assert!(updates.due(31.0), "asked at once of another node");
    }

    #[test]
    fn the_nodes_upgrade_warns_only_when_it_stops_the_node() {
        let upgrade = json!({ "least": "0.2.0", "from": 150_000 });
        let mut updates = Updates::default();

        assert_eq!(stopping(&json!({ "version": "0.2.0", "upgrade": upgrade })), None, "the node is new enough");
        assert_eq!(stopping(&json!({ "version": "0.1.9", "upgrade": null })), None);
        assert_eq!(stopping(&json!({ "upgrade": upgrade })), None, "a node from before versions says none");

        updates.status(&json!({ "version": "0.1.9", "upgrade": upgrade }));
        updates.release_stops = Some(Upgrade { least: Version::new(0, 3, 0), from: 120_000 });

        assert_eq!(updates.stops(), Some(Upgrade { least: Version::new(0, 3, 0), from: 120_000 }), "the first to stop it");
    }

    #[test]
    fn the_wallets_version_is_its_own() {
        assert_eq!(ours().to_string(), env!("CARGO_PKG_VERSION"));
    }
}
