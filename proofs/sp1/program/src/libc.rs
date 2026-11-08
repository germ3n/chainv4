//! What Luau's C++ asks of a C library beneath it, inside SP1's zkVM, which has none: memory from
//! Rust's allocator, SP1's, as malloc and newlib's reentrant _malloc_r; and newlib's system calls,
//! output dropped and time at zero. Strings, numbers and formatting are newlib's own libc.a, from
//! SP1's C toolchain. Nothing here changes what a block does: a contract's time is the block's,
//! never the clock's, and Luau reads the clock only for metrics this build leaves out.

use std::alloc::{Layout, alloc, dealloc, realloc as grow};
use std::ffi::{c_char, c_int, c_long, c_void};

/// Every allocation carries its size in front, as free and realloc need it.
const HEADER: usize = 16;

unsafe fn allocate(size: usize, zeroed: bool) -> *mut c_void {
    let Ok(layout) = Layout::from_size_align(size + HEADER, HEADER) else {
        return std::ptr::null_mut();
    };
    let base = unsafe { if zeroed { std::alloc::alloc_zeroed(layout) } else { alloc(layout) } };

    if base.is_null() {
        panic!("out of memory: {size} bytes asked of malloc");
    }

    unsafe {
        (base as *mut usize).write(size);
        base.add(HEADER) as *mut c_void
    }
}

unsafe fn release(pointer: *mut c_void) {
    if pointer.is_null() {
        return;
    }

    unsafe {
        let base = (pointer as *mut u8).sub(HEADER);
        let size = (base as *const usize).read();
        dealloc(base, Layout::from_size_align_unchecked(size + HEADER, HEADER));
    }
}

unsafe fn resize(pointer: *mut c_void, size: usize) -> *mut c_void {
    if pointer.is_null() {
        return unsafe { allocate(size, false) };
    }

    unsafe {
        let base = (pointer as *mut u8).sub(HEADER);
        let old = (base as *const usize).read();
        let moved = grow(base, Layout::from_size_align_unchecked(old + HEADER, HEADER), size + HEADER);

        if moved.is_null() {
            return std::ptr::null_mut();
        }

        (moved as *mut usize).write(size);
        moved.add(HEADER) as *mut c_void
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn malloc(size: usize) -> *mut c_void {
    unsafe { allocate(size, false) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn calloc(count: usize, size: usize) -> *mut c_void {
    count.checked_mul(size).map_or(std::ptr::null_mut(), |size| unsafe { allocate(size, true) })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn realloc(pointer: *mut c_void, size: usize) -> *mut c_void {
    unsafe { resize(pointer, size) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn free(pointer: *mut c_void) {
    unsafe { release(pointer) }
}

// newlib's own calls, with its reentrancy structure first, which these need not.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _malloc_r(_reent: *mut c_void, size: usize) -> *mut c_void {
    unsafe { allocate(size, false) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _calloc_r(_reent: *mut c_void, count: usize, size: usize) -> *mut c_void {
    unsafe { calloc(count, size) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _realloc_r(_reent: *mut c_void, pointer: *mut c_void, size: usize) -> *mut c_void {
    unsafe { resize(pointer, size) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _free_r(_reent: *mut c_void, pointer: *mut c_void) {
    unsafe { release(pointer) }
}

#[unsafe(no_mangle)]
pub extern "C" fn abort() -> ! {
    let (called_from, stack): (usize, usize);
    unsafe { std::arch::asm!("mv {}, ra", "mv {}, sp", out(reg) called_from, out(reg) stack) };
    // What looks like a return address on the stack, to find who called with addr2line.
    let found: Vec<String> = (0..4096)
        .map(|at| unsafe { ((stack + at * 8) as *const usize).read() })
        .filter(|word| (0x7800_0000..0x7900_0000).contains(word))
        .map(|word| format!("{word:#x}"))
        .collect();

    panic!("Luau aborted, called from {called_from:#x}; on the stack: {}", found.join(" "))
}

#[unsafe(no_mangle)]
pub extern "C" fn time(out: *mut c_long) -> c_long {
    if !out.is_null() {
        unsafe { out.write(0) };
    }

    0
}

#[unsafe(no_mangle)]
pub extern "C" fn clock() -> c_long {
    0
}

// newlib's system calls: no files, no processes, output dropped.
#[unsafe(no_mangle)]
pub extern "C" fn _write(_fd: c_int, _bytes: *const c_char, len: c_int) -> c_int {
    len
}

#[unsafe(no_mangle)]
pub extern "C" fn _read(_fd: c_int, _bytes: *mut c_char, _len: c_int) -> c_int {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn _close(_fd: c_int) -> c_int {
    -1
}

#[unsafe(no_mangle)]
pub extern "C" fn _lseek(_fd: c_int, _offset: c_long, _whence: c_int) -> c_long {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn _fstat(_fd: c_int, _stat: *mut c_void) -> c_int {
    -1
}

#[unsafe(no_mangle)]
pub extern "C" fn _isatty(_fd: c_int) -> c_int {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn _open(_path: *const c_char, _flags: c_int) -> c_int {
    -1
}

#[unsafe(no_mangle)]
pub extern "C" fn _sbrk(_increment: isize) -> *mut c_void {
    usize::MAX as *mut c_void
}

#[unsafe(no_mangle)]
pub extern "C" fn _getpid() -> c_int {
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn _kill(_pid: c_int, _signal: c_int) -> c_int {
    -1
}

#[unsafe(no_mangle)]
pub extern "C" fn _exit(_code: c_int) -> ! {
    panic!("Luau exited")
}

#[unsafe(no_mangle)]
pub extern "C" fn _gettimeofday(_tv: *mut c_void, _tz: *mut c_void) -> c_int {
    -1
}

/// What the unwinder asks of each Rust frame a C++ exception passes through, as a Lua error raised
/// from a Rust callback does on its way to Luau's catch: carry on, as a program that aborts on a
/// panic has no Rust handlers to run. _URC_CONTINUE_UNWIND.
#[unsafe(no_mangle)]
pub extern "C" fn rust_eh_personality(_version: c_int, _actions: c_int, _class: u64, _exception: *mut c_void, _context: *mut c_void) -> c_int {
    8
}

unsafe extern "C" {
    /// Where .eh_frame starts, as eh_frame.ld names it.
    static __eh_frame_start: u8;
    fn __register_frame_info(begin: *const u8, object: *mut u8);
    /// The static constructors, as the linker lays them out.
    static __init_array_start: extern "C" fn();
    static __init_array_end: extern "C" fn();
}

/// Room for libgcc's record of the registered tables, its struct object, a few words.
static mut FRAMES: [u64; 16] = [0; 16];

/// What a hosted program's startup does before main, which SP1's does not: registers the C++
/// unwinder's tables, so an exception Luau raises unwinds to where it is caught, and runs the
/// static constructors, which Luau's parser and compiler build their tables in.
pub fn start_cxx() {
    unsafe {
        __register_frame_info(&raw const __eh_frame_start, (&raw mut FRAMES).cast());
        let mut at = &raw const __init_array_start;

        while at < &raw const __init_array_end {
            (*at)();
            at = at.add(1);
        }
    }
}
