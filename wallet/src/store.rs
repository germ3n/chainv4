//! Where a wallet app on a device or a desktop keeps what it must not lose: a file for each wallet
//! and one for its settings, in a folder of their own. eframe's file holds everything else, and
//! nothing here depends on it: eframe empties its file and then writes it, so a kill between the
//! two leaves a broken file, which reads as empty, and the next save makes that for good. The
//! browser keeps the old way, as localStorage writes each value whole.
//!
//! A kill leaves a file as it was or as it became, never between: each is written to a file beside
//! it, flushed, read back, and only then renamed over it. And each has a copy, written the same way
//! just after, for when the first will not read. The copy holds what the first does and not what it
//! held before: otherwise a password changed, or an account removed, would stay in the copy until the
//! wallet next changed. A kill between the two leaves the copy old for a moment, until the next save.
//!
//! What will not read is set aside under a name of its own, never written over. A wallet's file is
//! written before settings naming it, so a kill between leaves a wallet settings do not name, which
//! is taken in rather than lost. Removing one goes the other way: its files go first, so that it
//! cannot come back once settings say what it held is to be forgotten.

use {
    crate::app::{Settings, WalletEntry},
    serde::{Deserialize, Serialize, de::DeserializeOwned},
    std::{
        collections::{HashMap, HashSet},
        fs,
        io::Write,
        path::{Path, PathBuf},
        thread::JoinHandle,
    },
};

/// The folder's name, inside the one the app keeps its files in.
const FOLDER: &str = "wallet-store";
/// What settings go by. No wallet's name can be it: those are hex.
const SETTINGS: &str = "settings";
/// What a file being written goes by until it is whole.
const WRITING: &str = "writing";

/// Settings on disk, with the wallets' files in the order the list has them.
#[derive(Serialize, Deserialize)]
struct SettingsFile<S> {
    order: Vec<String>,
    settings: S,
}

/// What was read at start: all that could be.
#[derive(Default)]
pub struct Loaded {
    /// Settings, with `wallets` left empty: None when there were none, or none that read.
    pub settings: Option<Settings>,
    /// The wallets, in the order settings name them, and then any they do not.
    pub wallets: Vec<WalletEntry>,
    /// What to tell the person of anything that could not be read.
    pub trouble: Vec<String>,
    /// Whether a save is needed soon, as a file or a copy was not what it should be.
    pub heal: bool,
}

/// What one save writes and removes, in order.
struct Job {
    gone: Vec<String>,
    wallets: Vec<(String, String)>,
    settings: Option<String>,
}

/// What came of a save, once it is done: what the files now hold.
struct Done {
    gone: Vec<String>,
    wallets: Vec<(String, String)>,
    settings: Option<String>,
}

pub struct Store {
    dir: PathBuf,
    /// What each wallet's files hold, by name, so that only what changed is written.
    written: HashMap<String, String>,
    settings_written: Option<String>,
    /// The save under way. Only one is, and each waits for the last, so they land in order.
    saving: Option<JoinHandle<Result<Done, String>>>,
    /// Why the last save failed, if it did. What failed is tried again by the next.
    pub failed: Option<String>,
}

impl Store {
    /// A store in this folder of the app's, made if it is not there.
    pub fn new(app_dir: &Path) -> Result<Store, String> {
        let dir = app_dir.join(FOLDER);
        fs::create_dir_all(&dir).map_err(|err| format!("could not make {}: {err}", dir.display()))?;
        private_folder(&dir);

        Ok(Store {
            dir,
            written: HashMap::new(),
            settings_written: None,
            saving: None,
            failed: None,
        })
    }

    /// Reads all there is, setting aside what will not read. It writes nothing else.
    pub fn load(&mut self) -> Loaded {
        let mut loaded = Loaded::default();
        let mut names = Vec::new();

        for entry in fs::read_dir(&self.dir).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();

            // A file not yet whole when the app was killed: what it was to replace still stands.
            if name.ends_with(&format!(".{WRITING}")) {
                fs::remove_file(entry.path()).ok();
            } else if let Some(stem) = name.strip_suffix(".ron").filter(|stem| !stem.contains('.') && *stem != SETTINGS) {
                names.push(stem.to_string());
            } else if let Some(stem) = name.strip_suffix(".copy.ron").filter(|stem| *stem != SETTINGS) {
                // Its first gone, as when a kill came after nothing else could have: the copy is it.
                names.push(stem.to_string());
            }
        }

        names.sort();
        names.dedup();

        let (mut order, mut settings) = (Vec::new(), None);

        match self.read::<SettingsFile<Settings>>(SETTINGS, &mut loaded) {
            Some((file, text)) => {
                self.settings_written = text;
                order = file.order;
                settings = Some(file.settings);
            }
            None => self.settings_written = None,
        }

        let mut wallets: HashMap<String, WalletEntry> = HashMap::new();

        for name in &names {
            if let Some((mut entry, text)) = self.read::<WalletEntry>(name, &mut loaded) {
                if let Some(text) = text {
                    self.written.insert(name.clone(), text);
                }

                entry.id = name.clone();
                wallets.insert(name.clone(), entry);
            }
        }

        for name in &order {
            match wallets.remove(name) {
                Some(entry) => loaded.wallets.push(entry),
                // Removing a wallet takes its files first, so a kill can leave this, but so can a
                // file lost to something else, which someone should hear of.
                None if !names.contains(name) => {
                    loaded.trouble.push("A wallet this device listed has no file any more, so it is not in the list.".to_string());
                    loaded.heal = true;
                }
                None => {}
            }
        }

        // Written, and killed before settings could name them.
        for name in &names {
            if let Some(entry) = wallets.remove(name) {
                loaded.wallets.push(entry);
                loaded.heal = true;
            }
        }

        loaded.settings = settings;

        loaded
    }

    /// Reads a file or else its copy. The text comes back only when the two hold the same; without
    /// it, the next save writes them both. Neither reading, each that exists is set aside.
    fn read<T: DeserializeOwned>(&self, name: &str, loaded: &mut Loaded) -> Option<(T, Option<String>)> {
        let (first, copy) = (self.first(name), self.copy(name));
        let (first_text, copy_text) = (fs::read_to_string(&first).ok(), fs::read_to_string(&copy).ok());

        if first_text.is_none() && copy_text.is_none() && !first.exists() && !copy.exists() {
            return None;
        }

        let read = first_text.as_deref().and_then(|text| ron::from_str::<T>(text).ok()).or_else(|| copy_text.as_deref().and_then(|text| ron::from_str::<T>(text).ok()));

        match read {
            Some(value) => {
                let same = first_text.is_some() && first_text == copy_text;
                loaded.heal |= !same;

                Some((value, first_text.filter(|_| same)))
            }
            None => {
                let kept = self.set_aside(&[first, copy]);
                let what = if name == SETTINGS { "This device's settings" } else { "A wallet's file" };
                loaded.trouble.push(format!("{what} could not be read, so it is left out. It was kept as {kept}: restoring from your words brings a wallet back."));
                loaded.heal = true;

                None
            }
        }
    }

    /// Moves files out of the way under names nothing reads, saying what the first became.
    fn set_aside(&self, paths: &[PathBuf]) -> String {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_millis());
        let mut kept = String::new();

        for path in paths.iter().filter(|path| path.exists()) {
            let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
            let aside = format!("{name}.unreadable-{stamp}");

            if fs::rename(path, self.dir.join(&aside)).is_ok() && kept.is_empty() {
                kept = aside;
            }
        }

        kept
    }

    fn first(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{name}.ron"))
    }

    fn copy(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{name}.copy.ron"))
    }

    /// Saves settings and the wallets in them, on a thread of its own: only what changed. A wallet
    /// with no name for its files yet gets one, kept in `settings`.
    pub fn save(&mut self, settings: &mut Settings) {
        self.wait();

        let mut names = HashSet::new();

        for entry in &mut settings.wallets {
            while entry.id.is_empty() || !names.insert(entry.id.clone()) {
                match fresh_name() {
                    Ok(name) => entry.id = name,
                    Err(why) => {
                        self.failed = Some(why);

                        return;
                    }
                }
            }
        }

        let job = match self.job(settings, &names) {
            Ok(job) => job,
            Err(why) => {
                self.failed = Some(why);

                return;
            }
        };

        if job.gone.is_empty() && job.wallets.is_empty() && job.settings.is_none() {
            return;
        }

        let dir = self.dir.clone();
        let saving = std::thread::Builder::new().name("wallet store".to_string()).spawn(move || run(&dir, job));

        match saving {
            Ok(saving) => self.saving = Some(saving),
            Err(why) => self.failed = Some(format!("could not start saving: {why}")),
        }
    }

    /// What a save has to do, worked out from settings as they stand.
    fn job(&self, settings: &mut Settings, names: &HashSet<String>) -> Result<Job, String> {
        let form = |why: ron::Error| format!("could not write this down: {why}");
        let mut wallets = Vec::new();

        for entry in &settings.wallets {
            let text = ron::to_string(entry).map_err(form)?;

            if self.written.get(&entry.id) != Some(&text) {
                wallets.push((entry.id.clone(), text));
            }
        }

        // Settings hold every wallet in memory, and each wallet has a file of its own.
        let held = std::mem::take(&mut settings.wallets);
        let file = SettingsFile { order: held.iter().map(|entry| entry.id.clone()).collect(), settings: &*settings };
        let text = ron::to_string(&file).map_err(form);
        settings.wallets = held;
        let text = text?;

        Ok(Job {
            gone: self.written.keys().filter(|name| !names.contains(*name)).cloned().collect(),
            wallets,
            settings: (self.settings_written.as_ref() != Some(&text)).then_some(text),
        })
    }

    /// Waits for the save under way, and takes what came of it. Whether nothing has failed.
    pub fn wait(&mut self) -> bool {
        if let Some(saving) = self.saving.take() {
            match saving.join() {
                Ok(Ok(done)) => {
                    for name in done.gone {
                        self.written.remove(&name);
                    }

                    self.written.extend(done.wallets);

                    if done.settings.is_some() {
                        self.settings_written = done.settings;
                    }

                    self.failed = None;
                }
                Ok(Err(why)) => self.failed = Some(why),
                Err(_) => self.failed = Some("saving stopped short".to_string()),
            }
        }

        self.failed.is_none()
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        self.wait();
    }
}

/// Does a save: files gone first, then wallets, and settings naming them last, stopping at the
/// first failure, so settings never name a wallet not written.
fn run(dir: &Path, job: Job) -> Result<Done, String> {
    let first = |name: &str| dir.join(format!("{name}.ron"));
    let copy = |name: &str| dir.join(format!("{name}.copy.ron"));

    for name in &job.gone {
        for path in [copy(name), first(name)] {
            match fs::remove_file(&path) {
                Ok(()) => {}
                Err(why) if why.kind() == std::io::ErrorKind::NotFound => {}
                Err(why) => return Err(format!("could not remove {}: {why}", path.display())),
            }
        }

        flush_folder(dir);
    }

    for (name, text) in &job.wallets {
        written_whole(&first(name), text)?;
        written_whole(&copy(name), text)?;
    }

    if let Some(text) = &job.settings {
        written_whole(&first(SETTINGS), text)?;
        written_whole(&copy(SETTINGS), text)?;
    }

    Ok(Done { gone: job.gone, wallets: job.wallets, settings: job.settings })
}

/// Puts this text at this path whole, or leaves what is there: written beside it, flushed, read
/// back, and only then renamed over it.
fn written_whole(path: &Path, text: &str) -> Result<(), String> {
    let beside = path.with_extension(format!("ron.{WRITING}"));
    let fail = |why: std::io::Error| format!("could not write {}: {why}", path.display());
    let result = (|| {
        let mut file = create_private(&beside).map_err(fail)?;
        file.write_all(text.as_bytes()).map_err(fail)?;
        file.sync_all().map_err(fail)?;
        drop(file);

        if fs::read_to_string(&beside).map_err(fail)? != text {
            return Err(format!("{} did not read back as written", path.display()));
        }

        fs::rename(&beside, path).map_err(fail)
    })();

    if result.is_err() {
        fs::remove_file(&beside).ok();
    } else if let Some(folder) = path.parent() {
        flush_folder(folder);
    }

    result
}

/// A new file only this user can read, where that can be asked for. Sealed vaults are in here.
fn create_private(path: &Path) -> std::io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);

    options.open(path)
}

#[cfg(unix)]
fn private_folder(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).ok();
}

#[cfg(not(unix))]
fn private_folder(_: &Path) {}

/// Makes a rename last through losing power, where a folder can be flushed.
fn flush_folder(dir: &Path) {
    #[cfg(unix)]
    if let Ok(folder) = fs::File::open(dir) {
        folder.sync_all().ok();
    }
    #[cfg(not(unix))]
    let _ = dir;
}

fn fresh_name() -> Result<String, String> {
    let mut bytes = [0; 16];
    getrandom::fill(&mut bytes).map_err(|err| format!("no randomness for a file's name: {err}"))?;

    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// What a start works from, given what the folder held and what eframe's file held, which is where
/// every wallet was before this, and where an older build run since will have put any it made: a
/// wallet only there is added, one with the same sealed vault is the same wallet.
pub fn combined(loaded: Loaded, eframes: Option<Settings>) -> Settings {
    let (mut settings, older_wallets) = match (loaded.settings, eframes) {
        (Some(settings), older) => (settings, older.map(|older| older.wallets).unwrap_or_default()),
        (None, Some(mut older)) => {
            let wallets = std::mem::take(&mut older.wallets);

            (older, wallets)
        }
        (None, None) => (Settings::default(), Vec::new()),
    };

    settings.wallets = loaded.wallets;

    for wallet in older_wallets {
        if !settings.wallets.iter().any(|entry| entry.vault == wallet.vault) {
            settings.wallets.push(wallet);
        }
    }

    settings
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::vault::{Book, Vault},
    };

    /// A folder for one test, empty, gone when it ends.
    struct Folder(PathBuf);

    impl Folder {
        fn new(test: &str) -> Folder {
            let path = std::env::temp_dir().join(format!("chainv4-store-{}-{test}", std::process::id()));
            fs::remove_dir_all(&path).ok();
            fs::create_dir_all(&path).unwrap();

            Folder(path)
        }

        fn store(&self) -> Store {
            Store::new(&self.0).unwrap()
        }

        fn path(&self, file: &str) -> PathBuf {
            self.0.join(FOLDER).join(file)
        }

        /// Everything in the folder, as text, to look through.
        fn everything(&self) -> String {
            let mut all = String::new();

            for entry in fs::read_dir(self.0.join(FOLDER)).unwrap().flatten() {
                all.push_str(&fs::read_to_string(entry.path()).unwrap_or_default());
            }

            all
        }

        fn names(&self) -> Vec<String> {
            let mut names: Vec<String> = fs::read_dir(self.0.join(FOLDER)).unwrap().flatten().map(|entry| entry.file_name().to_string_lossy().into_owned()).collect();
            names.sort();

            names
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).ok();
        }
    }

    /// A wallet of this name, sealed with no stretching worth the wait being asked of a test more
    /// than once: each is its own vault.
    fn wallet(name: &str) -> WalletEntry {
        let vault = Vault::create("").unwrap();

        WalletEntry { id: String::new(), name: name.to_string(), vault: vault.seal(&Book::default()).unwrap(), current: String::new(), biometric: None, unprotected: true }
    }

    fn settings(names: &[&str]) -> Settings {
        let mut settings = Settings::default();
        settings.wallets = names.iter().map(|name| wallet(name)).collect();
        settings.node = "http://here".to_string();

        settings
    }

    fn saved(store: &mut Store, settings: &mut Settings) {
        store.save(settings);
        assert!(store.wait(), "{:?}", store.failed);
    }

    fn listed(loaded: &Loaded) -> Vec<&str> {
        loaded.wallets.iter().map(|entry| entry.name.as_str()).collect()
    }

    fn id_of(settings: &Settings, name: &str) -> String {
        settings.wallets.iter().find(|entry| entry.name == name).unwrap().id.clone()
    }

    #[test]
    fn each_wallet_gets_a_file_and_a_copy_and_all_comes_back_in_its_order() {
        let folder = Folder::new("round");
        let mut kept = settings(&["Zed", "Abe", "Mid"]);
        saved(&mut folder.store(), &mut kept);

        // Two files a wallet, and two for settings.
        assert_eq!(folder.names().len(), 8, "{:?}", folder.names());
        assert!(kept.wallets.iter().all(|entry| entry.id.len() == 32));

        let loaded = folder.store().load();

        assert_eq!(listed(&loaded), ["Zed", "Abe", "Mid"]);
        assert_eq!(loaded.settings.as_ref().unwrap().node, "http://here");
        assert!(loaded.settings.as_ref().unwrap().wallets.is_empty(), "wallets are in their own files");
        assert!(loaded.trouble.is_empty() && !loaded.heal, "{:?}", loaded.trouble);
        assert_eq!(loaded.wallets, kept.wallets);
    }

    #[test]
    fn a_settings_file_holds_no_vault() {
        let folder = Folder::new("apart");
        let mut kept = settings(&["Mine"]);
        let sealed = kept.wallets[0].vault.sealed.clone();
        saved(&mut folder.store(), &mut kept);

        assert!(!fs::read_to_string(folder.path("settings.ron")).unwrap().contains(&sealed));
        assert!(fs::read_to_string(folder.path(&format!("{}.ron", kept.wallets[0].id))).unwrap().contains(&sealed));
        assert_eq!(kept.wallets.len(), 1, "and the list in memory is left as it was");
    }

    #[test]
    fn a_torn_file_falls_back_on_its_copy_and_the_next_save_mends_it() {
        let folder = Folder::new("torn");
        let mut kept = settings(&["Mine", "Other"]);
        saved(&mut folder.store(), &mut kept);
        let id = id_of(&kept, "Mine");
        let whole = fs::read_to_string(folder.path(&format!("{id}.ron"))).unwrap();

        for torn in [String::new(), whole[..whole.len() / 2].to_string()] {
            fs::write(folder.path(&format!("{id}.ron")), &torn).unwrap();
            let mut store = folder.store();
            let loaded = store.load();

            assert_eq!(listed(&loaded), ["Mine", "Other"]);
            assert!(loaded.trouble.is_empty(), "the copy read, so nothing was lost");
            assert!(loaded.heal);

            let mut again = combined(loaded, None);
            saved(&mut store, &mut again);

            assert_eq!(fs::read_to_string(folder.path(&format!("{id}.ron"))).unwrap(), whole);
        }
    }

    #[test]
    fn a_kill_between_a_file_and_its_copy_reads_the_new_and_mends_the_copy() {
        let folder = Folder::new("between");
        let mut kept = settings(&["Mine"]);
        saved(&mut folder.store(), &mut kept);
        let id = id_of(&kept, "Mine");
        let old = fs::read_to_string(folder.path(&format!("{id}.ron"))).unwrap();

        // The new one in place, and the copy not reached.
        kept.wallets[0].name = "Renamed".to_string();
        let new = ron::to_string(&kept.wallets[0]).unwrap();
        fs::write(folder.path(&format!("{id}.ron")), &new).unwrap();

        let mut store = folder.store();
        let loaded = store.load();

        assert_eq!(listed(&loaded), ["Renamed"]);
        assert!(loaded.heal);

        let mut again = combined(loaded, None);
        saved(&mut store, &mut again);

        assert_eq!(fs::read_to_string(folder.path(&format!("{id}.copy.ron"))).unwrap(), new);
        assert!(!folder.everything().contains(&old), "what it held before is gone from every file");
    }

    #[test]
    fn a_file_killed_while_being_written_is_ignored_and_what_it_was_to_replace_stands() {
        let folder = Folder::new("writing");
        let mut kept = settings(&["Mine"]);
        saved(&mut folder.store(), &mut kept);
        let id = id_of(&kept, "Mine");
        fs::write(folder.path(&format!("{id}.ron.writing")), "half of a wal").unwrap();
        fs::write(folder.path("settings.ron.writing"), "half of sett").unwrap();

        let loaded = folder.store().load();

        assert_eq!(listed(&loaded), ["Mine"]);
        assert!(loaded.trouble.is_empty() && !loaded.heal);
        assert!(!folder.names().iter().any(|name| name.ends_with(".writing")), "and it is gone: {:?}", folder.names());
    }

    #[test]
    fn a_wallet_neither_of_whose_files_reads_is_set_aside_never_written_over_and_the_rest_open() {
        let folder = Folder::new("both");
        let mut kept = settings(&["Mine", "Other"]);
        saved(&mut folder.store(), &mut kept);
        let id = id_of(&kept, "Mine");
        fs::write(folder.path(&format!("{id}.ron")), "not a wallet").unwrap();
        fs::write(folder.path(&format!("{id}.copy.ron")), "not one either").unwrap();

        let mut store = folder.store();
        let loaded = store.load();

        assert_eq!(listed(&loaded), ["Other"]);
        assert_eq!(loaded.trouble.len(), 1, "{:?}", loaded.trouble);
        assert!(loaded.trouble[0].contains("unreadable"), "it says what it was kept as: {}", loaded.trouble[0]);

        let mut again = combined(loaded, None);
        saved(&mut store, &mut again);

        let names = folder.names();
        assert_eq!(names.iter().filter(|name| name.starts_with(&id) && name.contains(".unreadable-")).count(), 2, "{names:?}");
        assert!(folder.everything().contains("not a wallet") && folder.everything().contains("not one either"));
        assert_eq!(listed(&folder.store().load()), ["Other"], "and what was set aside is not read as a wallet");
    }

    #[test]
    fn settings_that_will_not_read_cost_no_wallet() {
        let folder = Folder::new("settings");
        let mut kept = settings(&["Mine", "Other"]);
        saved(&mut folder.store(), &mut kept);
        fs::write(folder.path("settings.ron"), "broken").unwrap();
        fs::write(folder.path("settings.copy.ron"), "broken too").unwrap();

        let loaded = folder.store().load();

        assert!(loaded.settings.is_none());
        assert_eq!(loaded.wallets.len(), 2, "with no order left, both still come back");
        assert_eq!(loaded.trouble.len(), 1);
        assert!(folder.names().iter().any(|name| name.starts_with("settings.ron.unreadable-")));
    }

    #[test]
    fn a_wallet_written_before_a_kill_kept_settings_from_naming_it_is_taken_in() {
        let folder = Folder::new("orphan");
        let mut kept = settings(&["Mine"]);
        saved(&mut folder.store(), &mut kept);

        // Its files are there, and settings are as they were before it.
        let mut newer = wallet("New");
        newer.id = "ab".repeat(16);
        let text = ron::to_string(&newer).unwrap();
        fs::write(folder.path(&format!("{}.ron", newer.id)), &text).unwrap();
        fs::write(folder.path(&format!("{}.copy.ron", newer.id)), &text).unwrap();

        let loaded = folder.store().load();

        assert_eq!(listed(&loaded), ["Mine", "New"]);
        assert!(loaded.heal, "so that settings come to name it");
    }

    #[test]
    fn a_wallet_removed_is_gone_from_every_file_and_does_not_come_back() {
        let folder = Folder::new("removed");
        let mut kept = settings(&["Mine", "Gone"]);
        let mut store = folder.store();
        saved(&mut store, &mut kept);
        let (id, sealed) = (id_of(&kept, "Gone"), kept.wallets[1].vault.sealed.clone());

        kept.wallets.remove(1);
        saved(&mut store, &mut kept);

        assert!(!folder.names().iter().any(|name| name.starts_with(&id)), "{:?}", folder.names());
        assert!(!folder.everything().contains(&sealed));

        let loaded = folder.store().load();

        assert_eq!(listed(&loaded), ["Mine"]);
        assert!(loaded.trouble.is_empty() && !loaded.heal);
    }

    #[test]
    fn a_kill_after_a_removed_wallets_files_went_only_says_so_and_the_list_mends() {
        let folder = Folder::new("removing");
        let mut kept = settings(&["Mine", "Gone"]);
        saved(&mut folder.store(), &mut kept);
        let id = id_of(&kept, "Gone");
        fs::remove_file(folder.path(&format!("{id}.ron"))).unwrap();
        fs::remove_file(folder.path(&format!("{id}.copy.ron"))).unwrap();

        let mut store = folder.store();
        let loaded = store.load();

        assert_eq!(listed(&loaded), ["Mine"]);
        assert_eq!(loaded.trouble.len(), 1);

        let mut again = combined(loaded, None);
        saved(&mut store, &mut again);

        assert!(folder.store().load().trouble.is_empty(), "settings no longer name it");
    }

    #[test]
    fn a_new_seal_leaves_no_copy_of_the_old_one_anywhere() {
        let folder = Folder::new("password");
        let mut kept = settings(&["Mine"]);
        let mut store = folder.store();
        saved(&mut store, &mut kept);
        let old = kept.wallets[0].vault.sealed.clone();
        let new = Vault::create("a new password").unwrap();
        kept.wallets[0].vault = new.seal(&Book::default()).unwrap();
        saved(&mut store, &mut kept);

        assert!(!folder.everything().contains(&old), "an old password opens nothing kept here");
        assert_eq!(folder.everything().matches(&kept.wallets[0].vault.sealed).count(), 2, "its file and its copy");
    }

    #[test]
    fn only_what_changed_is_written() {
        let folder = Folder::new("changed");
        let mut kept = settings(&["Mine", "Other"]);
        let mut store = folder.store();
        saved(&mut store, &mut kept);
        let (mine, other) = (id_of(&kept, "Mine"), id_of(&kept, "Other"));

        // Marks on the files, which a write would take away.
        for name in [format!("{mine}.ron"), format!("{other}.ron"), "settings.ron".to_string()] {
            fs::write(folder.path(&name), "mark").unwrap();
        }

        saved(&mut store, &mut kept);

        for name in [format!("{mine}.ron"), format!("{other}.ron"), "settings.ron".to_string()] {
            assert_eq!(fs::read_to_string(folder.path(&name)).unwrap(), "mark", "{name}");
        }

        // A setting changed touches settings alone.
        kept.node = "http://there".to_string();
        saved(&mut store, &mut kept);

        assert_eq!(fs::read_to_string(folder.path(&format!("{mine}.ron"))).unwrap(), "mark");
        assert!(fs::read_to_string(folder.path("settings.ron")).unwrap().contains("http://there"));

        // A wallet changed touches that wallet, and not the other.
        kept.wallets[1].current = "elsewhere".to_string();
        saved(&mut store, &mut kept);

        assert_eq!(fs::read_to_string(folder.path(&format!("{mine}.ron"))).unwrap(), "mark");
        assert!(fs::read_to_string(folder.path(&format!("{other}.ron"))).unwrap().contains("elsewhere"));
    }

    #[test]
    fn a_save_that_fails_says_why_leaves_the_files_as_they_were_and_is_tried_again() {
        let folder = Folder::new("failed");
        let mut kept = settings(&["Mine"]);
        let mut store = folder.store();
        saved(&mut store, &mut kept);
        let id = id_of(&kept, "Mine");
        let whole = fs::read_to_string(folder.path(&format!("{id}.ron"))).unwrap();

        // A folder where the file being written would go: nothing can be written there.
        fs::create_dir(folder.path(&format!("{id}.ron.writing"))).unwrap();
        kept.wallets[0].name = "Renamed".to_string();
        store.save(&mut kept);

        assert!(!store.wait());
        assert!(store.failed.as_ref().unwrap().contains("could not write"), "{:?}", store.failed);
        assert_eq!(fs::read_to_string(folder.path(&format!("{id}.ron"))).unwrap(), whole);
        assert!(!fs::read_to_string(folder.path("settings.ron")).unwrap().contains("Renamed"));

        fs::remove_dir(folder.path(&format!("{id}.ron.writing"))).unwrap();
        saved(&mut store, &mut kept);

        assert!(fs::read_to_string(folder.path(&format!("{id}.ron"))).unwrap().contains("Renamed"));
        assert!(store.failed.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn its_files_are_this_users_alone() {
        use std::os::unix::fs::PermissionsExt;

        let folder = Folder::new("private");
        let mut kept = settings(&["Mine"]);
        saved(&mut folder.store(), &mut kept);

        assert_eq!(fs::metadata(folder.0.join(FOLDER)).unwrap().permissions().mode() & 0o777, 0o700);

        for name in folder.names() {
            assert_eq!(fs::metadata(folder.path(&name)).unwrap().permissions().mode() & 0o777, 0o600, "{name}");
        }
    }

    #[test]
    fn a_first_start_takes_every_wallet_and_setting_from_eframes_file() {
        let older = settings(&["One", "Two"]);
        let combined = combined(Loaded::default(), Some(older.clone()));

        assert_eq!(combined.wallets, older.wallets);
        assert_eq!(combined.node, "http://here");
    }

    #[test]
    fn a_wallet_an_older_build_made_since_is_added_and_one_already_here_is_not() {
        let folder = Folder::new("older");
        let mut kept = settings(&["Mine"]);
        let mut store = folder.store();
        saved(&mut store, &mut kept);

        let mut older = settings(&["Made by an older build"]);
        let mut same = kept.wallets[0].clone();
        same.id = String::new();
        older.wallets.insert(0, same);
        older.node = "http://older".to_string();

        let combined = combined(folder.store().load(), Some(older));

        assert_eq!(combined.wallets.iter().map(|entry| entry.name.as_str()).collect::<Vec<_>>(), ["Mine", "Made by an older build"]);
        assert_eq!(combined.node, "http://here", "settings here win");

        // It gets a file with the next save.
        let mut combined = combined;
        saved(&mut store, &mut combined);

        assert_eq!(folder.store().load().wallets.len(), 2);
    }

    #[test]
    fn with_settings_lost_eframes_are_used_and_the_wallets_here_still_stand() {
        let loaded = Loaded { wallets: vec![wallet("Here")], ..Loaded::default() };
        let combined = combined(loaded, Some(settings(&["There"])));

        assert_eq!(combined.wallets.len(), 2);
        assert_eq!(combined.wallets[0].name, "Here");
        assert_eq!(combined.node, "http://here");
    }
}
