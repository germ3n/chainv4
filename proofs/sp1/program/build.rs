//! Luau is C++, so the program links SP1's RISC-V C++ runtime, libstdc++ and libgcc, from the
//! toolchain sp1up --c-toolchain installs under $SP1_DIR, ~/.sp1 unless set.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("zkvm") {
        return;
    }

    // .eh_frame named and ended, for main to register: see eh_frame.ld.
    let script = format!("{}/eh_frame.ld", std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets it"));
    println!("cargo:rerun-if-changed={script}");
    println!("cargo:rustc-link-arg=-T{script}");

    let sp1 = std::env::var("SP1_DIR").unwrap_or_else(|_| format!("{}/.sp1", std::env::var("HOME").expect("HOME is set")));
    println!("cargo:rerun-if-env-changed=SP1_DIR");
    println!("cargo:rustc-link-search=native={sp1}/riscv/riscv64-unknown-elf/lib");

    if let Ok(entries) = std::fs::read_dir(format!("{sp1}/riscv/lib/gcc/riscv64-unknown-elf")) {
        for entry in entries.flatten() {
            println!("cargo:rustc-link-search=native={}", entry.path().display());
        }
    }

    // newlib's C library, for what Luau's C++ asks of one; src/libc.rs gives it memory.
    println!("cargo:rustc-link-lib=static=c");
    println!("cargo:rustc-link-lib=static=gcc");
}
