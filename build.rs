use std::env;
use std::path::{Path, PathBuf};

fn main() {
    write_wiki();
}

/// Builds target/wiki/index.html from every `#[document]` in src. A broken page fails the build
/// the same way the attribute itself does.
fn write_wiki() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let src = manifest_dir.join("src");
    // Cargo watches a directory recursively, so a file that gains its first doc is picked up too.
    println!("cargo:rerun-if-changed={}", src.display());
    println!("cargo:rerun-if-changed=build.rs");

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let dest = wiki_html(&out_dir, &manifest_dir);

    if let Err(err) = wiki::generate(&src, &dest) {
        panic!("wiki: {err}");
    }
}

fn wiki_html(out_dir: &Path, manifest_dir: &Path) -> PathBuf {
    // OUT_DIR is target/<profile>/build/<crate>/out.
    let target_dir = out_dir
        .ancestors()
        .nth(4)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| manifest_dir.join("target"));

    target_dir.join("wiki/index.html")
}
