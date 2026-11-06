//! Memory that held a secret, wiped once it is let go. Typed (src/secret.rs) and the keys' own types
//! wipe what they hold, but egui copies text it shows or is typed: into a layout of each run of
//! glyphs, the job that laid it out, accesskit's tree and the key events that brought it. Each copy
//! is freed in its turn, and memory freed is not wiped, so the words stayed in it until something
//! wrote over them. Wiping, the apps' global allocator, zeroes every block as it is freed or moved;
//! let_go has egui free what it laid out for a locked wallet's pages, so it is wiped too.
//!
//! What the system swaps out is beyond reach; no_dumps keeps the process's memory from being
//! written out whole when it crashes.

use std::alloc::{GlobalAlloc, Layout, System};

/// A global allocator that zeroes each block it frees, and the old block of each it moves, before
/// the allocator under it has it back. Its cost is a write over everything freed.
pub struct Wiping<A = System>(pub A);

impl Wiping {
    /// Over the system's allocator.
    pub const fn system() -> Wiping {
        Wiping(System)
    }
}

/// Zeroes len bytes at ptr, writes the compiler may not drop although nothing reads them before
/// the memory is freed.
///
/// # Safety
///
/// ptr must be valid for len bytes of writes.
pub unsafe fn wipe(ptr: *mut u8, len: usize) {
    unsafe { std::ptr::write_bytes(ptr, 0, len) };
    // As far as the compiler knows, this may read the memory, so the zeros must be written first.
    std::hint::black_box(ptr);
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
}

unsafe impl<A: GlobalAlloc> GlobalAlloc for Wiping<A> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        unsafe { self.0.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        unsafe { self.0.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe {
            wipe(ptr, layout.size());
            self.0.dealloc(ptr, layout);
        }
    }

    /// Always a new block, the old one wiped: an allocator's realloc may move the block, even to
    /// shrink it, and would free the old one as it was.
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        unsafe {
            let new_layout = Layout::from_size_align_unchecked(new_size, layout.align());
            let new = self.0.alloc(new_layout);

            if !new.is_null() {
                std::ptr::copy_nonoverlapping(ptr, new, layout.size().min(new_size));
                self.dealloc(ptr, layout);
            }

            new
        }
    }
}

/// Has egui free what it laid out to draw pages just left, a locked wallet's words among them, so
/// Wiping wipes it: the text fields' states and egui's caches go now, and the next few frames are
/// drawn, as egui frees a layout once a frame has gone by without it.
pub fn let_go(ctx: &egui::Context) {
    ctx.data_mut(|data| {
        data.remove_by_type::<egui::text_edit::TextEditState>();
        data.insert_temp(egui::Id::unique(LETTING_GO), FRAMES);
    });
    ctx.memory_mut(|memory| memory.caches = Default::default());
    ctx.request_repaint();
}

/// The frames let_go has drawn, a layout last used in the first freed by the third.
const FRAMES: u8 = 3;
const LETTING_GO: &str = "chainv4-ui letting go";

/// Draws the frames let_go asked for: called each frame, by an app that calls let_go.
pub fn letting_go(ctx: &egui::Context) {
    let left = ctx.data_mut(|data| {
        let left = data.get_temp::<u8>(egui::Id::unique(LETTING_GO)).unwrap_or(0);
        data.insert_temp(egui::Id::unique(LETTING_GO), left.saturating_sub(1));
        left
    });

    if left > 0 {
        ctx.request_repaint();
    }
}

/// Keeps a crash from writing this process's memory out whole, secrets and all: no core file,
/// and on Linux and Android not dumpable at all, which also stops a core helper and another
/// process of the same user reading it. Elsewhere it does nothing. Called as an app starts.
pub fn no_dumps() {
    #[cfg(unix)]
    unsafe {
        let none = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
        if libc::setrlimit(libc::RLIMIT_CORE, &none) != 0 {
            log::warn!("could not refuse core files: {}", std::io::Error::last_os_error());
        }
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    unsafe {
        if libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) != 0 {
            log::warn!("could not make the process undumpable: {}", std::io::Error::last_os_error());
        }
    }
}

#[cfg(test)]
mod tests;
