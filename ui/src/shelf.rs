//! Where an app keeps an index it can rebuild, by name, apart from everything it must not lose: files
//! in a private folder on a device or a desktop, written whole, and IndexedDB in a browser, whose
//! localStorage holds about 5 MB and has no room. Losing what is on a shelf costs a rescan and nothing
//! else. Nothing here reads what it holds, or knows it is sealed.
//!
//! A browser only answers later, so everything here answers later, everywhere. A `Keeper` puts one
//! thing on a shelf at a time, in order, and of what waits keeps only the newest for each name: an
//! index saved faster than the shelf takes it never piles up.

use std::sync::{Arc, Mutex, PoisonError};

/// What to do with the answer, when there is one.
pub type Done<T> = Box<dyn FnOnce(Result<T, String>) + Send>;

/// A place to keep bytes by name. Names are the app's own, and have no slashes.
pub trait Shelf: Send + Sync {
    /// What is kept under this name, or None for nothing.
    fn read(&self, name: &str, done: Done<Option<Vec<u8>>>);
    /// Keeps these under this name, whole or not at all.
    fn write(&self, name: &str, bytes: Vec<u8>, done: Done<()>);
    /// Moves what is under this name out of the way, under a name of its own that nothing reads, and
    /// says what it went by.
    fn set_aside(&self, name: &str, done: Done<String>);
    /// Removes everything whose name starts with this, what was set aside too.
    fn forget(&self, prefix: &str, done: Done<()>);
}

/// A shelf in memory: what an app has where there is nowhere else, which starts empty on each run.
#[derive(Default)]
pub struct MemoryShelf {
    held: Mutex<Vec<(String, Vec<u8>)>>,
}

impl MemoryShelf {
    /// Everything on it, by name, for a test to look through.
    pub fn held(&self) -> Vec<(String, Vec<u8>)> {
        self.held.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

impl Shelf for MemoryShelf {
    fn read(&self, name: &str, done: Done<Option<Vec<u8>>>) {
        let found = self.held.lock().unwrap_or_else(PoisonError::into_inner).iter().find(|(held, _)| held == name).map(|(_, bytes)| bytes.clone());

        done(Ok(found));
    }

    fn write(&self, name: &str, bytes: Vec<u8>, done: Done<()>) {
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);

        held.retain(|(kept, _)| kept != name);
        held.push((name.to_string(), bytes));
        drop(held);
        done(Ok(()));
    }

    fn set_aside(&self, name: &str, done: Done<String>) {
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        let aside = format!("{name}.unreadable");

        if let Some(found) = held.iter_mut().find(|(kept, _)| kept == name) {
            found.0.clone_from(&aside);
        }

        drop(held);
        done(Ok(aside));
    }

    fn forget(&self, prefix: &str, done: Done<()>) {
        self.held.lock().unwrap_or_else(PoisonError::into_inner).retain(|(kept, _)| !kept.starts_with(prefix));
        done(Ok(()));
    }
}

/// What a keeper is doing.
#[derive(Default)]
struct Keeping {
    writing: bool,
    /// The newest bytes for each name that waits, oldest name first.
    waiting: Vec<(String, Vec<u8>)>,
    /// What asked to read while something was being written, so that it reads what was written.
    reading: Vec<(String, Done<Option<Vec<u8>>>)>,
    failed: Option<String>,
}

/// Puts things on a shelf one at a time. Cheap to clone; clones share what they are doing.
#[derive(Clone)]
pub struct Keeper {
    shelf: Arc<dyn Shelf>,
    keeping: Arc<Mutex<Keeping>>,
}

impl Keeper {
    pub fn new(shelf: Arc<dyn Shelf>) -> Keeper {
        Keeper { shelf, keeping: Arc::default() }
    }

    /// Reads what is under this name, once what is being written is written: an index put away and
    /// opened again at once comes back as it was put away.
    pub fn read(&self, name: &str, done: Done<Option<Vec<u8>>>) {
        let mut keeping = self.keeping.lock().unwrap_or_else(PoisonError::into_inner);

        if keeping.writing {
            keeping.reading.push((name.to_string(), done));

            return;
        }

        drop(keeping);
        self.shelf.read(name, done);
    }

    /// Moves what is under this name out of the way.
    pub fn set_aside(&self, name: &str, done: Done<String>) {
        self.shelf.set_aside(name, done);
    }

    /// Removes everything whose name starts with this, and anything of it that waits.
    pub fn forget(&self, prefix: &str, done: Done<()>) {
        self.keeping.lock().unwrap_or_else(PoisonError::into_inner).waiting.retain(|(name, _)| !name.starts_with(prefix));
        self.shelf.forget(prefix, done);
    }

    /// Keeps these under this name: now, or once what is under way is done, unless newer ones for the
    /// same name come first.
    pub fn save(&self, name: &str, bytes: Vec<u8>) {
        let mut keeping = self.keeping.lock().unwrap_or_else(PoisonError::into_inner);

        if keeping.writing {
            keeping.waiting.retain(|(waiting, _)| waiting != name);
            keeping.waiting.push((name.to_string(), bytes));

            return;
        }

        keeping.writing = true;
        drop(keeping);
        self.write(name.to_string(), bytes);
    }

    fn write(&self, name: String, bytes: Vec<u8>) {
        let keeper = self.clone();

        self.shelf.write(
            &name,
            bytes,
            Box::new(move |result| {
                let mut keeping = keeper.keeping.lock().unwrap_or_else(PoisonError::into_inner);

                keeping.failed = result.err();
                let next = (!keeping.waiting.is_empty()).then(|| keeping.waiting.remove(0));
                let reading = match next {
                    Some(_) => Vec::new(),
                    None => std::mem::take(&mut keeping.reading),
                };

                keeping.writing = next.is_some();
                drop(keeping);

                if let Some((name, bytes)) = next {
                    keeper.write(name, bytes);
                }

                for (name, done) in reading {
                    keeper.shelf.read(&name, done);
                }
            }),
        );
    }

    /// Why the last write failed, if it did. What failed is tried again by the next save.
    pub fn failed(&self) -> Option<String> {
        self.keeping.lock().unwrap_or_else(PoisonError::into_inner).failed.clone()
    }

    /// Whether anything is being written or waits to be.
    pub fn busy(&self) -> bool {
        let keeping = self.keeping.lock().unwrap_or_else(PoisonError::into_inner);

        keeping.writing || !keeping.waiting.is_empty()
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use files::FileShelf;

#[cfg(not(target_arch = "wasm32"))]
mod files {
    use {
        super::{Done, Shelf},
        crate::kept::{flush_folder, private_folder, written_whole},
        std::fs,
        std::path::PathBuf,
    };

    /// What a file on this shelf ends in, which tells it from anything else in the folder.
    const ENDING: &str = "index";

    /// A shelf in a folder: a file for each name, written whole on a thread of its own.
    pub struct FileShelf {
        dir: PathBuf,
    }

    impl FileShelf {
        /// A shelf in this folder, made if it is not there, and this user's alone.
        pub fn new(dir: PathBuf) -> Result<FileShelf, String> {
            fs::create_dir_all(&dir).map_err(|why| format!("could not make {}: {why}", dir.display()))?;
            private_folder(&dir);

            Ok(FileShelf { dir })
        }

        fn path(&self, name: &str) -> PathBuf {
            self.dir.join(format!("{name}.{ENDING}"))
        }
    }

    impl Shelf for FileShelf {
        fn read(&self, name: &str, done: Done<Option<Vec<u8>>>) {
            let path = self.path(name);

            std::thread::spawn(move || {
                done(match fs::read(&path) {
                    Ok(bytes) => Ok(Some(bytes)),
                    Err(why) if why.kind() == std::io::ErrorKind::NotFound => Ok(None),
                    Err(why) => Err(format!("could not read {}: {why}", path.display())),
                });
            });
        }

        fn write(&self, name: &str, bytes: Vec<u8>, done: Done<()>) {
            let path = self.path(name);

            std::thread::spawn(move || done(written_whole(&path, &bytes)));
        }

        fn set_aside(&self, name: &str, done: Done<String>) {
            let (path, dir) = (self.path(name), self.dir.clone());
            let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_millis());
            let aside = format!("{name}.{ENDING}.unreadable-{stamp}");

            std::thread::spawn(move || {
                done(match fs::rename(&path, dir.join(&aside)) {
                    Ok(()) => Ok(aside),
                    Err(why) if why.kind() == std::io::ErrorKind::NotFound => Ok(aside),
                    Err(why) => Err(format!("could not set {} aside: {why}", path.display())),
                });
            });
        }

        fn forget(&self, prefix: &str, done: Done<()>) {
            let (dir, prefix) = (self.dir.clone(), prefix.to_string());

            std::thread::spawn(move || {
                let mut failed = None;

                for entry in fs::read_dir(&dir).into_iter().flatten().flatten() {
                    let name = entry.file_name().to_string_lossy().into_owned();

                    if name.starts_with(&prefix)
                        && name.contains(&format!(".{ENDING}"))
                        && let Err(why) = fs::remove_file(entry.path())
                    {
                        failed = Some(format!("could not remove {name}: {why}"));
                    }
                }

                flush_folder(&dir);
                done(failed.map_or(Ok(()), Err));
            });
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::IndexedDbShelf;

#[cfg(target_arch = "wasm32")]
mod web {
    use {
        super::{Done, Shelf},
        js_sys::{Array, Date, Uint8Array},
        std::sync::{Arc, Mutex, PoisonError},
        wasm_bindgen::{JsCast, JsValue, closure::Closure},
        web_sys::{IdbDatabase, IdbObjectStore, IdbOpenDbRequest, IdbRequest, IdbTransactionMode},
    };

    /// The object store within the database: bytes by name.
    const STORE: &str = "kept";

    /// A shelf in IndexedDB, in a database of this name. A browser that will not open one, as some do
    /// in a private window, answers with why.
    pub struct IndexedDbShelf {
        database: String,
    }

    impl IndexedDbShelf {
        pub fn new(database: &str) -> IndexedDbShelf {
            IndexedDbShelf { database: database.to_string() }
        }

        /// Opens the database, and hands on its store for this kind of use. An error goes to `failed`.
        fn with_store(&self, mode: IdbTransactionMode, failed: impl FnOnce(String) + Send + 'static, then: impl FnOnce(IdbObjectStore, IdbDatabase) + 'static) {
            let factory = web_sys::window().ok_or("no window").and_then(|window| window.indexed_db().ok().flatten().ok_or("this browser keeps no IndexedDB here"));
            let opened = factory.map_err(str::to_string).and_then(|factory| factory.open_with_u32(&self.database, 1).map_err(said));
            let request = match opened {
                Ok(request) => request,
                Err(why) => return failed(why),
            };
            let upgrade = Closure::once_into_js({
                let request: IdbOpenDbRequest = request.clone();

                move || {
                    if let Some(database) = result(&request).and_then(|value| value.dyn_into::<IdbDatabase>().ok())
                        && !database.object_store_names().contains(STORE)
                    {
                        let _ = database.create_object_store(STORE);
                    }
                }
            });
            let failed = once(failed);
            let on_error = {
                let (failed, request) = (failed.clone(), request.clone());

                Closure::once_into_js(move || fire(&failed, error_of(request.unchecked_ref())))
            };
            let on_success = {
                let (failed, request) = (failed.clone(), request.clone());

                Closure::once_into_js(move || {
                    let store = result(&request).and_then(|value| value.dyn_into::<IdbDatabase>().ok()).ok_or_else(|| "IndexedDB opened nothing".to_string()).and_then(|database| {
                        let store = database.transaction_with_str_and_mode(STORE, mode).and_then(|transaction| transaction.object_store(STORE)).map_err(said)?;

                        Ok((store, database))
                    });

                    match store {
                        Ok((store, database)) => then(store, database),
                        Err(why) => fire(&failed, why),
                    }
                })
            };

            request.set_onupgradeneeded(Some(upgrade.unchecked_ref()));
            request.set_onerror(Some(on_error.unchecked_ref()));
            request.set_onsuccess(Some(on_success.unchecked_ref()));
        }
    }

    /// Something to call at most once, from whichever way a request ends.
    type Once<F> = Arc<Mutex<Option<F>>>;

    fn once<F>(call: F) -> Once<F> {
        Arc::new(Mutex::new(Some(call)))
    }

    fn fire<F: FnOnce(String)>(once: &Once<F>, why: String) {
        if let Some(call) = once.lock().unwrap_or_else(PoisonError::into_inner).take() {
            call(why);
        }
    }

    fn result(request: &IdbRequest) -> Option<JsValue> {
        request.result().ok()
    }

    fn said(value: JsValue) -> String {
        value.as_string().or_else(|| value.dyn_ref::<js_sys::Error>().map(|error| String::from(error.message()))).unwrap_or_else(|| "IndexedDB refused".to_string())
    }

    fn error_of(request: &IdbRequest) -> String {
        request.error().ok().flatten().map_or_else(|| "IndexedDB refused".to_string(), |error| error.message())
    }

    /// Calls `done` with the result of a request, once it ends, and closes the database.
    fn finish<T: 'static>(request: IdbRequest, database: IdbDatabase, done: Done<T>, read: impl FnOnce(JsValue) -> Result<T, String> + 'static) {
        let done = once(done);
        let on_success = {
            let (done, request, database) = (done.clone(), request.clone(), database.clone());

            Closure::once_into_js(move || {
                let value = result(&request).unwrap_or(JsValue::UNDEFINED);

                database.close();

                if let Some(done) = done.lock().unwrap_or_else(PoisonError::into_inner).take() {
                    done(read(value));
                }
            })
        };
        let on_error = {
            let (request2, database) = (request.clone(), database);

            Closure::once_into_js(move || {
                database.close();

                if let Some(done) = done.lock().unwrap_or_else(PoisonError::into_inner).take() {
                    done(Err(error_of(&request2)));
                }
            })
        };

        request.set_onsuccess(Some(on_success.unchecked_ref()));
        request.set_onerror(Some(on_error.unchecked_ref()));
    }

    impl Shelf for IndexedDbShelf {
        fn read(&self, name: &str, done: Done<Option<Vec<u8>>>) {
            let name = name.to_string();
            let done = once(done);
            let failed = {
                let done = done.clone();

                move |why| fire_done(&done, Err(why))
            };

            self.with_store(IdbTransactionMode::Readonly, failed, move |store, database| match store.get(&JsValue::from_str(&name)) {
                Ok(request) => {
                    let done = take_done(&done);

                    finish(request, database, done, |value| Ok((!value.is_undefined()).then(|| Uint8Array::new(&value).to_vec())));
                }
                Err(why) => {
                    database.close();
                    fire_done(&done, Err(said(why)));
                }
            });
        }

        fn write(&self, name: &str, bytes: Vec<u8>, done: Done<()>) {
            let name = name.to_string();
            let done = once(done);
            let failed = {
                let done = done.clone();

                move |why| fire_done(&done, Err(why))
            };

            self.with_store(IdbTransactionMode::Readwrite, failed, move |store, database| match store.put_with_key(&Uint8Array::from(bytes.as_slice()), &JsValue::from_str(&name)) {
                Ok(request) => {
                    let done = take_done(&done);

                    finish(request, database, done, |_| Ok(()));
                }
                Err(why) => {
                    database.close();
                    fire_done(&done, Err(said(why)));
                }
            });
        }

        fn set_aside(&self, name: &str, done: Done<String>) {
            let name = name.to_string();
            let aside = format!("{name}.unreadable-{}", Date::now() as u64);
            let done = once(done);
            let failed = {
                let done = done.clone();

                move |why| fire_done(&done, Err(why))
            };

            self.with_store(IdbTransactionMode::Readwrite, failed, move |store, database| {
                let moved = store.get(&JsValue::from_str(&name)).map_err(said);
                let request = match moved {
                    Ok(request) => request,
                    Err(why) => {
                        database.close();
                        fire_done(&done, Err(why));

                        return;
                    }
                };
                let on_found = {
                    let request2 = request.clone();

                    Closure::once_into_js(move || {
                        let value = result(&request2).unwrap_or(JsValue::UNDEFINED);

                        if !value.is_undefined() {
                            let _ = store.put_with_key(&value, &JsValue::from_str(&aside));
                        }

                        match store.delete(&JsValue::from_str(&name)) {
                            Ok(deleted) => {
                                let done = take_done(&done);

                                finish(deleted, database, done, move |_| Ok(aside));
                            }
                            Err(why) => {
                                database.close();
                                fire_done(&done, Err(said(why)));
                            }
                        }
                    })
                };

                request.set_onsuccess(Some(on_found.unchecked_ref()));
            });
        }

        fn forget(&self, prefix: &str, done: Done<()>) {
            let prefix = prefix.to_string();
            let done = once(done);
            let failed = {
                let done = done.clone();

                move |why| fire_done(&done, Err(why))
            };

            self.with_store(IdbTransactionMode::Readwrite, failed, move |store, database| match store.get_all_keys() {
                Ok(request) => {
                    let store2 = store.clone();
                    let done = take_done(&done);

                    finish(request, database, done, move |keys| {
                        for key in Array::from(&keys).iter() {
                            if key.as_string().is_some_and(|name| name.starts_with(&prefix)) {
                                store2.delete(&key).map_err(said)?;
                            }
                        }

                        Ok(())
                    });
                }
                Err(why) => {
                    database.close();
                    fire_done(&done, Err(said(why)));
                }
            });
        }
    }

    /// Calls what waits with this, if it still does.
    fn fire_done<T>(done: &Once<Done<T>>, result: Result<T, String>) {
        if let Some(done) = done.lock().unwrap_or_else(PoisonError::into_inner).take() {
            done(result);
        }
    }

    /// What waits, taken out to hand on, or a call that does nothing if it was called already.
    fn take_done<T: 'static>(done: &Once<Done<T>>) -> Done<T> {
        done.lock().unwrap_or_else(PoisonError::into_inner).take().unwrap_or_else(|| Box::new(|_| {}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A write waiting to be finished.
    type Pending = (String, Vec<u8>, Done<()>);

    /// A shelf whose writes wait to be finished by the test, as a browser's do, and may fail.
    #[derive(Default)]
    struct Slow {
        held: MemoryShelf,
        pending: Mutex<Vec<Pending>>,
        failing: std::sync::atomic::AtomicBool,
    }

    impl Slow {
        fn finish_one(&self) -> Option<String> {
            let mut pending = self.pending.lock().unwrap();

            if pending.is_empty() {
                return None;
            }

            let (name, bytes, done) = pending.remove(0);
            drop(pending);

            if self.failing.load(std::sync::atomic::Ordering::Relaxed) {
                done(Err("the quota ran out".to_string()));
            } else {
                self.held.write(&name, bytes, done);
            }

            Some(name)
        }
    }

    impl Shelf for Slow {
        fn read(&self, name: &str, done: Done<Option<Vec<u8>>>) {
            self.held.read(name, done);
        }

        fn write(&self, name: &str, bytes: Vec<u8>, done: Done<()>) {
            self.pending.lock().unwrap().push((name.to_string(), bytes, done));
        }

        fn set_aside(&self, name: &str, done: Done<String>) {
            self.held.set_aside(name, done);
        }

        fn forget(&self, prefix: &str, done: Done<()>) {
            self.held.forget(prefix, done);
        }
    }

    fn read(keeper: &Keeper, name: &str) -> Option<Vec<u8>> {
        let found = Arc::new(Mutex::new(None));
        let kept = found.clone();

        keeper.read(name, Box::new(move |result| *kept.lock().unwrap() = Some(result.unwrap())));

        let found = found.lock().unwrap().take();

        found.expect("a memory shelf answers at once")
    }

    #[test]
    fn a_keeper_writes_one_at_a_time_and_of_what_waits_keeps_the_newest_for_each_name() {
        let slow = Arc::new(Slow::default());
        let keeper = Keeper::new(slow.clone());

        keeper.save("a", vec![1]);
        keeper.save("a", vec![2]);
        keeper.save("b", vec![3]);
        keeper.save("a", vec![4]);

        assert!(keeper.busy());
        assert_eq!(slow.pending.lock().unwrap().len(), 1, "one at a time");
        assert_eq!(slow.finish_one().as_deref(), Some("a"));
        assert_eq!(slow.held.held(), [("a".to_string(), vec![1])]);
        assert_eq!(slow.finish_one().as_deref(), Some("b"));
        assert_eq!(slow.finish_one().as_deref(), Some("a"));
        assert_eq!(slow.finish_one(), None, "the 2 never went, as the 4 came first");
        assert_eq!((read(&keeper, "a"), read(&keeper, "b")), (Some(vec![4]), Some(vec![3])));
        assert!(!keeper.busy());
    }

    #[test]
    fn a_read_while_something_is_written_reads_what_was_written() {
        let slow = Arc::new(Slow::default());
        let keeper = Keeper::new(slow.clone());
        let found = Arc::new(Mutex::new(None));
        let kept = found.clone();

        keeper.save("a", vec![1]);
        keeper.save("a", vec![2]);
        keeper.read("a", Box::new(move |result| *kept.lock().unwrap() = Some(result.unwrap())));

        slow.finish_one();
        assert_eq!(*found.lock().unwrap(), None, "the newest is still to come");
        slow.finish_one();
        assert_eq!(*found.lock().unwrap(), Some(Some(vec![2])));
    }

    #[test]
    fn a_write_that_fails_says_why_leaves_what_was_there_and_the_next_save_goes_again() {
        let slow = Arc::new(Slow::default());
        let keeper = Keeper::new(slow.clone());

        keeper.save("a", vec![1]);
        slow.finish_one();
        slow.failing.store(true, std::sync::atomic::Ordering::Relaxed);
        keeper.save("a", vec![2]);
        slow.finish_one();

        assert_eq!(keeper.failed().as_deref(), Some("the quota ran out"));
        assert_eq!(read(&keeper, "a"), Some(vec![1]));

        slow.failing.store(false, std::sync::atomic::Ordering::Relaxed);
        keeper.save("a", vec![3]);
        slow.finish_one();

        assert_eq!(keeper.failed(), None);
        assert_eq!(read(&keeper, "a"), Some(vec![3]));
    }

    #[test]
    fn what_is_set_aside_is_not_read_and_forgetting_a_prefix_takes_it_and_what_waits() {
        let slow = Arc::new(Slow::default());
        let keeper = Keeper::new(slow.clone());

        for (name, byte) in [("wallet-one", 1), ("wallet-two", 2), ("other", 3)] {
            keeper.save(name, vec![byte]);
            slow.finish_one();
        }

        let aside = Arc::new(Mutex::new(String::new()));
        let kept = aside.clone();
        keeper.set_aside("wallet-one", Box::new(move |name| *kept.lock().unwrap() = name.unwrap()));
        assert_eq!(read(&keeper, "wallet-one"), None);
        assert_eq!(read(&keeper, &aside.lock().unwrap().clone()), Some(vec![1]));

        keeper.save("wallet-two", vec![9]);
        keeper.save("wallet-three", vec![9]);
        keeper.forget("wallet-", Box::new(|result| result.unwrap()));
        slow.finish_one();

        assert_eq!(slow.finish_one(), None, "nothing of it waits");
        assert_eq!(read(&keeper, "other"), Some(vec![3]));
        assert!(slow.held.held().iter().all(|(name, _)| !name.starts_with("wallet-") || name == "wallet-two"), "only what was already on its way");
    }

    #[test]
    fn files_on_a_shelf_come_back_as_written_and_are_this_users_alone() {
        let dir = std::env::temp_dir().join(format!("chainv4-shelf-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        let shelf = FileShelf::new(dir.clone()).unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let answered = |sender: &std::sync::mpsc::Sender<Result<Option<Vec<u8>>, String>>| -> Done<Option<Vec<u8>>> {
            let sender = sender.clone();

            Box::new(move |result| sender.send(result).unwrap())
        };
        let (wrote, written) = std::sync::mpsc::channel();

        shelf.read("mine", answered(&sender));
        assert_eq!(receiver.recv().unwrap(), Ok(None));

        let sender2 = wrote.clone();
        shelf.write("mine", vec![1, 2, 255], Box::new(move |result| sender2.send(result).unwrap()));
        written.recv().unwrap().unwrap();

        shelf.read("mine", answered(&sender));
        assert_eq!(receiver.recv().unwrap(), Ok(Some(vec![1, 2, 255])));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            assert_eq!(std::fs::metadata(dir.join("mine.index")).unwrap().permissions().mode() & 0o777, 0o600);
            assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        }

        let (aside_sender, aside) = std::sync::mpsc::channel();
        shelf.set_aside("mine", Box::new(move |result| aside_sender.send(result).unwrap()));
        let name = aside.recv().unwrap().unwrap();
        assert!(dir.join(&name).exists());

        shelf.read("mine", answered(&sender));
        assert_eq!(receiver.recv().unwrap(), Ok(None), "set aside, it is not read");

        std::fs::write(dir.join("mine.ron"), "a wallet's file").unwrap();
        let sender2 = wrote.clone();
        shelf.forget("mine", Box::new(move |result| sender2.send(result).unwrap()));
        written.recv().unwrap().unwrap();

        assert!(!dir.join(&name).exists(), "what was set aside goes with it");
        assert!(dir.join("mine.ron").exists(), "only what the shelf put there");
        std::fs::remove_dir_all(&dir).ok();
    }
}
