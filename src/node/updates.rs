//! A node and releases (docs/updates.md): a thread fetches the signed manifest from --updates every
//! hour, keeps the newest that a threshold of the release keys built in signed, in release.json
//! beside the database, and warns while this node is behind. The node stops at an upgrade the
//! manifest announces for its chain, as at one governance decided; GET /release serves the
//! manifest to the apps, and the mining port to workers. With --auto-update the thread downloads
//! the newer binary, puts it in place and restarts the node on it.

use {
    crate::release::{ReleaseError, ReleaseFile, ReleaseKeys, SignedRelease, Standing, TARGET, current_release, kept_release, take_newest, write_kept},
    crate::structs::version::{Upgrade, Version},
    r#macro::document,
    std::path::PathBuf,
    std::sync::atomic::{AtomicBool, Ordering},
    std::sync::{Arc, Mutex, PoisonError},
    std::time::Duration,
};

#[document(summary = "How often a node fetches the manifest, in milliseconds, unless --update-every says otherwise: an hour.", category = "Releases", see_also = "UpdateConfig::every_ms")]
pub const CHECK_EVERY_MS: u64 = 3_600_000;
/// The most a manifest may be.
const MOST_MANIFEST: u64 = 1 << 20;
/// The most a binary may be.
const MOST_BINARY: u64 = 1 << 30;

#[document(
    summary = "Where a node looks for releases, how often, and what it does with one: --updates, its proxy, --update-every and --auto-update.",
    category = "Releases",
    see_also = "Releases, spawn_updates",
)]
#[derive(Clone, Debug)]
pub struct UpdateConfig {
    #[document(summary = "The manifest's URL: --updates, or the one the release keys built in name.")]
    pub url: String,
    #[document(summary = "A SOCKS5 proxy to fetch through, socks5h://host:port: Tor's, on a node that dials through it.")]
    pub proxy: Option<String>,
    #[document(summary = "Whether to download a newer chainv4, put it in place of this binary, and restart on it.")]
    pub auto: bool,
    #[document(summary = "The binary to replace: the running one.")]
    pub binary: PathBuf,
    #[document(summary = "How often to fetch the manifest, in milliseconds: CHECK_EVERY_MS by default, --update-every.")]
    pub every_ms: u64,
}

#[document(
    summary = "What a node knows of releases now: the newest manifest it took, when it last looked, why that failed if it did, and the version --auto-update put in place, which runs once the node restarts.",
    category = "Releases",
    see_also = "Releases",
)]
#[derive(Clone, Debug, Default)]
pub struct KnownRelease {
    #[document(summary = "The newest manifest a threshold of the release keys signed.")]
    pub signed: Option<SignedRelease>,
    #[document(summary = "When the node last fetched one, in milliseconds since the Unix epoch; 0 before.")]
    pub checked: u64,
    #[document(summary = "Why the last fetch failed, or None.")]
    pub error: Option<String>,
    #[document(summary = "The version --auto-update put in place, which runs once the node restarts.")]
    pub installed: Option<Version>,
}

impl KnownRelease {
    #[document(parent = "KnownRelease", summary = "Where this build stands against the newest chainv4 the manifest names, or None without one that still counts.", params = { now = "The time, in seconds since the Unix epoch." })]
    pub fn standing(&self, now: u64) -> Option<Standing> {
        current_release(&self.signed, now)?.release.product("chainv4").map(|product| product.standing(Version::ours()))
    }

    #[document(parent = "KnownRelease", summary = "Whether the newest manifest it took stopped counting: its host fell behind, or this machine's clock is wrong.", params = { now = "The time, in seconds since the Unix epoch." })]
    pub fn expired(&self, now: u64) -> bool {
        self.signed.as_ref().is_some_and(|signed| signed.release.expired(now))
    }
}

#[document(
    summary = "A node's releases, shared between it, the thread that fetches them, the API and the mining port: the release keys built in, the newest manifest they signed, and release.json, which keeps it across restarts so an older one is never taken after it.",
    category = "Releases",
    see_also = "KnownRelease, UpdateConfig, spawn_updates",
)]
#[derive(Debug)]
pub struct Releases {
    keys: ReleaseKeys,
    file: Option<PathBuf>,
    known: Mutex<KnownRelease>,
}

impl Releases {
    #[document(
        parent = "Releases",
        summary = "A node's releases under these keys, starting from the manifest kept in file if it still verifies. A file gone or unreadable is said and goes by the floor built in, as there is nothing else to go by, and the next manifest taken writes it again.",
        params = {
            keys = "The release keys to trust: ReleaseKeys::built_in, but for tests.",
            file = "Where the newest manifest is kept, release.json beside the database; None keeps it in memory alone.",
        },
    )]
    pub fn new(keys: ReleaseKeys, file: Option<PathBuf>) -> Releases {
        let signed = file.as_ref().and_then(|path| match std::fs::read_to_string(path) {
            Ok(text) => {
                let kept = kept_release(&text, &keys);

                if kept.is_none() {
                    log::warn!("[updates] {} holds no release these keys take: going by the oldest this build takes, until a newer one comes", path.display());
                }

                kept
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
            Err(err) => {
                log::warn!("[updates] could not read {}: {err}: going by the oldest this build takes, until a newer one comes", path.display());

                None
            }
        });

        Releases { keys, file, known: Mutex::new(KnownRelease { signed, ..KnownRelease::default() }) }
    }

    #[document(parent = "Releases", summary = "The release keys it trusts.")]
    pub fn keys(&self) -> &ReleaseKeys {
        &self.keys
    }

    #[document(parent = "Releases", summary = "What it knows now.")]
    pub fn known(&self) -> KnownRelease {
        self.known.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    #[document(
        parent = "Releases",
        summary = "Takes a manifest that verifies, at or above the floor, and was issued after the one it has, keeping it in its file; one expired is kept too, and said to be.",
        params = { now = "The time, in milliseconds since the Unix epoch." },
        returns = "Whether it was newer, or why it does not verify.",
    )]
    pub fn offer(&self, signed: SignedRelease, now: u64) -> Result<bool, ReleaseError> {
        let mut known = self.known.lock().unwrap_or_else(PoisonError::into_inner);
        known.checked = now;
        let had = known.signed.as_ref().map(|had| had.release.issued);
        let taken = take_newest(&self.keys, &mut known.signed, signed, now / 1_000);
        known.error = taken.as_ref().err().map(ToString::to_string);

        if let (Some(path), Some(kept)) = (&self.file, known.signed.as_ref().filter(|kept| Some(kept.release.issued) != had))
            && let Err(err) = write_kept(path, kept)
        {
            log::warn!("[updates] could not keep the release: {err}");
        }

        taken
    }

    #[document(parent = "Releases", summary = "Notes a fetch that failed.")]
    pub fn failed(&self, why: String, now: u64) {
        let mut known = self.known.lock().unwrap_or_else(PoisonError::into_inner);
        (known.checked, known.error) = (now, Some(why));
    }

    // Even expired: it was signed and taken in time, and the chain's upgrade does not lapse with the
    // host that stopped serving it. Running past it would fork the node off on its own.
    #[document(parent = "Releases", summary = "The upgrade the manifest announces for a chain that stops this build first, if any: of the newest it took, even past its expires.", params = { chain = "The chain's genesis id, as bytes." })]
    pub fn stopping(&self, chain: &[u8]) -> Option<Upgrade> {
        self.known.lock().unwrap_or_else(PoisonError::into_inner).signed.as_ref()?.release.stopping(chain, Version::ours())
    }

    fn installed(&self, version: Version) {
        self.known.lock().unwrap_or_else(PoisonError::into_inner).installed = Some(version);
    }
}

#[document(
    summary = "Fetches a manifest. Its signatures are not checked here: Releases::offer and SignedRelease::verify do that.",
    category = "Releases",
    params = {
        url = "The manifest's URL, or a node's GET /release.",
        proxy = "A SOCKS5 proxy to fetch through, or None.",
    },
)]
pub fn fetch_release(url: &str, proxy: Option<&str>) -> Result<SignedRelease, String> {
    let bytes = get(url, proxy, MOST_MANIFEST)?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|err| format!("{url} is not JSON: {err}"))?;
    // A node's GET /release carries the manifest under "manifest", beside its own standing.
    let manifest = match value.get("signatures") {
        Some(_) => &value,
        None => &value["manifest"],
    };

    match manifest.is_null() {
        true => Err(format!("{url} has no release")),
        false => SignedRelease::from_json(manifest),
    }
}

#[document(
    summary = "Downloads a release's binary, checking its size and SHA-256 are the ones signed.",
    category = "Releases",
    params = {
        file = "The binary.",
        proxy = "A SOCKS5 proxy to fetch through, or None.",
    },
    see_also = "install",
)]
pub fn download(file: &ReleaseFile, proxy: Option<&str>) -> Result<Vec<u8>, String> {
    // One byte past its size, so a download that is longer reads as one, not as cut short.
    let bytes = get(&file.url, proxy, file.size.min(MOST_BINARY) + 1)?;

    match bytes.len() as u64 == file.size && crate::release::sha256(&bytes) == file.sha256 {
        true => Ok(bytes),
        false => Err(format!("the download from {} is not the binary the release signed", file.url)),
    }
}

fn get(url: &str, proxy: Option<&str>, most: u64) -> Result<Vec<u8>, String> {
    let mut config = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(600))).http_status_as_error(true);

    if let Some(proxy) = proxy {
        config = config.proxy(Some(ureq::Proxy::new(proxy).map_err(|err| format!("the proxy {proxy}: {err}"))?));
    }

    let agent: ureq::Agent = config.build().into();
    let mut answer = agent.get(url).call().map_err(|err| format!("{url}: {err}"))?;

    answer.body_mut().with_config().limit(most).read_to_vec().map_err(|err| format!("{url}: {err}"))
}

#[document(
    summary = "Starts the thread that fetches releases every UpdateConfig::every_ms, CHECK_EVERY_MS by default, and warns while this build is behind; with --auto-update it puts a newer chainv4 in place and sets restart and stop, so the node stops and main starts it again on the new binary.",
    category = "Releases",
    params = {
        config = "Where to look, and whether to update.",
        releases = "What the node knows of releases, which the thread keeps up.",
        stop = "Set when the node stops, which ends the thread; the thread sets it to restart.",
        restart = "Set by the thread once a new binary is in place.",
    },
)]
pub fn spawn_updates(config: UpdateConfig, releases: Arc<Releases>, stop: Arc<AtomicBool>, restart: Arc<AtomicBool>) -> std::io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new().name("updates".to_string()).spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            check(&config, &releases, &stop, &restart);

            // A second at a time, so a stop is seen within one however long the wait.
            for _ in 0..config.every_ms.div_ceil(1_000).max(1) {
                if stop.load(Ordering::Relaxed) {
                    return;
                }

                std::thread::sleep(Duration::from_secs(1));
            }
        }
    })
}

#[document(
    summary = "One look for releases: fetches the manifest, takes it if it is newer, warns while this build is behind, and with --auto-update puts a newer chainv4 in place.",
    category = "Releases",
    params = {
        config = "Where to look, and whether to update.",
        releases = "What the node knows of releases.",
        stop = "Set, with restart, once a new binary is in place.",
        restart = "Set once a new binary is in place.",
    },
    see_also = "spawn_updates",
)]
pub fn check(config: &UpdateConfig, releases: &Releases, stop: &AtomicBool, restart: &AtomicBool) {
    let now = crate::node::now_ms();

    match fetch_release(&config.url, config.proxy.as_deref()) {
        Ok(signed) => {
            if let Err(err) = releases.offer(signed, now) {
                log::warn!("[updates] the release at {} is refused: {err}", config.url);
            }
        }
        Err(why) => {
            log::debug!("[updates] {why}");
            releases.failed(why, now);
        }
    }

    let known = releases.known();

    if known.expired(now / 1_000) {
        log::warn!("[updates] the newest release this node has expired: the host at {} is behind, or this machine's clock is wrong. Nothing is updated from it", config.url);

        return;
    }

    let Some(product) = known.signed.as_ref().and_then(|signed| signed.release.product("chainv4")) else {
        return;
    };

    match product.standing(Version::ours()) {
        Standing::Current => return,
        _ if known.installed.is_some() => return,
        Standing::Behind { newest } => log::warn!("[updates] chainv4 {newest} is out, and this node runs {}: update it with chainv4 update, or start it with --auto-update. {}", Version::ours(), product.notes),
        Standing::Unsupported { newest, least } => log::warn!("[updates] chainv4 {newest} is out, and versions older than {least} are no longer supported; this node runs {}: update it with chainv4 update. {}", Version::ours(), product.notes),
    }

    if !config.auto {
        return;
    }

    let Some(file) = product.file(TARGET) else {
        return log::warn!("[updates] chainv4 {} has no binary for {TARGET}: update it by hand", product.version);
    };

    match download(file, config.proxy.as_deref()).and_then(|bytes| crate::release::install(file, &bytes, &config.binary)) {
        Ok(()) => {
            log::info!("[updates] chainv4 {} is in place of {}: restarting on it", product.version, config.binary.display());
            releases.installed(product.version);
            restart.store(true, Ordering::Relaxed);
            stop.store(true, Ordering::Relaxed);
        }
        Err(why) => log::warn!("[updates] could not update to chainv4 {}: {why}", product.version),
    }
}
