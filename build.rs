use std::env;
use std::path::{Path, PathBuf};

fn main() {
    write_wiki();
    write_release_keys();
}

/// Puts the release keys this build trusts where src/release reads them: the file
/// CHAINV4_RELEASE_KEYS names, or release/keys.json; and names the target, which of a release's
/// binaries is this one.
fn write_release_keys() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let keys = match env::var_os("CHAINV4_RELEASE_KEYS") {
        Some(path) => PathBuf::from(path),
        None => manifest_dir.join("release/keys.json"),
    };
    println!("cargo:rerun-if-env-changed=CHAINV4_RELEASE_KEYS");
    println!("cargo:rerun-if-changed={}", keys.display());
    println!("cargo:rustc-env=CHAINV4_TARGET={}", env::var("TARGET").unwrap());

    let text = std::fs::read_to_string(&keys).unwrap_or_else(|err| panic!("release keys {}: {err}", keys.display()));
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    std::fs::write(out_dir.join("release-keys.json"), text).expect("OUT_DIR is writable");
}

/// Builds target/wiki/index.html from every `#[document]` in src and the guides in docs/ and the
/// README. A broken page, or a link to a guide or heading that is not there, fails the build the
/// same way the attribute itself does.
fn write_wiki() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let src = manifest_dir.join("src");
    // Cargo watches a directory recursively, so a file that gains its first doc is picked up too.
    println!("cargo:rerun-if-changed={}", src.display());
    println!("cargo:rerun-if-changed={}", manifest_dir.join("docs").display());
    println!("cargo:rerun-if-changed={}", manifest_dir.join("README.md").display());
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
