//! A chainv4 explorer: blocks on a map, transactions and the hops their contracts made, the
//! network, the mempool and finality, live. It talks to a node's HTTP API, and runs as a desktop
//! app, in the browser built with trunk, and on Android built with xbuild: the binary starts the
//! first two, and Android loads this library itself.

mod api;
mod app;
mod format;
mod light;
mod model;
mod proof;
mod reading;
mod release;
mod store;
mod theme;
mod views;

/// What each test thread holds on the heap, for measuring what a structure takes in memory.
#[cfg(test)]
pub(crate) mod counting {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;

    thread_local! {
        static LIVE: Cell<i64> = const { Cell::new(0) };
    }

    pub struct Counting;

    // SAFETY: every call goes to the system allocator as it came; only a thread-local count
    // is kept beside it, read with try_with so a thread being torn down is never touched.
    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let _ = LIVE.try_with(|live| live.set(live.get() + layout.size() as i64));
            unsafe { System.alloc(layout) }
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            let _ = LIVE.try_with(|live| live.set(live.get() - layout.size() as i64));
            unsafe { System.dealloc(ptr, layout) }
        }

        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            let _ = LIVE.try_with(|live| live.set(live.get() + new_size as i64 - layout.size() as i64));
            unsafe { System.realloc(ptr, layout, new_size) }
        }
    }

    /// What this thread holds now.
    pub fn live() -> i64 {
        LIVE.with(Cell::get)
    }
}

#[cfg(test)]
#[global_allocator]
static ALLOCATOR: counting::Counting = counting::Counting;

/// The explorer, for eframe to run. It talks to this node, or else to the one it talked to last,
/// or else to the fallback. What it reads of a chain, when told to, is kept under this folder of the
/// Every block freed is wiped first, as it may have held words or a key: what egui laid out to show
/// them, or the text it copied as they were typed (chainv4_ui::wipe). Tests count allocations
/// with their own.
#[cfg(all(feature = "wipe-freed", not(test)))]
#[global_allocator]
static WIPING: chainv4_ui::wipe::Wiping = chainv4_ui::wipe::Wiping::system();

/// app's, or, in a browser, which has none, in IndexedDB.
pub fn explorer(creation: &eframe::CreationContext, node: Option<String>, fallback: &str, folder: Option<std::path::PathBuf>) -> Box<dyn eframe::App> {
    chainv4_ui::wipe::no_dumps();
    Box::new(app::Explorer::new(creation, node, fallback, folder))
}

/// Where Android starts the app, in the NativeActivity's own thread.
#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
fn android_main(app: chainv4_ui::android::AndroidApp) {
    let folder = app.internal_data_path();
    chainv4_ui::android::run(app, "chainv4 explorer", "chainv4-explorer", "explorer.ron", move |creation| explorer(creation, None, chainv4_ui::PUBLIC_NODE, folder));
}
