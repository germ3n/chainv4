//! The toolchains are pinned in several places that must agree (docs/development.md, "Toolchains"):
//! Rust's version in rust-toolchain.toml, docker/Dockerfile's RUST_VERSION and every workflow's
//! toolchain, and clang's major version in docker/Dockerfile's CLANG_VERSION and every workflow
//! that installs it. A version changed in one place and not the others fails here.

use std::{fs, path::Path};

fn read(path: &str) -> String {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(path)).unwrap_or_else(|err| panic!("{path}: {err}"))
}

/// What follows a prefix on a line, up to the end of the line, with quotes taken off.
fn after<'a>(text: &'a str, prefix: &str) -> Vec<&'a str> {
    text.lines().filter_map(|line| line.trim().strip_prefix(prefix)).map(|rest| rest.trim().trim_matches('"')).collect()
}

fn workflows() -> Vec<(String, String)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join(".github/workflows");
    let mut found: Vec<_> = fs::read_dir(&dir)
        .expect("the workflows")
        .map(|entry| entry.expect("a workflow").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "yml"))
        .map(|path| (path.display().to_string(), fs::read_to_string(&path).expect("a workflow")))
        .collect();
    found.sort();

    found
}

#[test]
fn rust_is_pinned_alike_everywhere() {
    let file = read("rust-toolchain.toml");
    let channel = after(&file, "channel = ");
    assert_eq!(channel.len(), 1, "one channel in rust-toolchain.toml");
    let rust = channel[0];
    assert!(rust.split('.').count() == 3 && rust.split('.').all(|part| part.parse::<u32>().is_ok()), "an exact version, not {rust}");

    assert_eq!(after(&read("docker/Dockerfile"), "ARG RUST_VERSION="), [rust], "docker/Dockerfile");

    for (path, text) in workflows() {
        assert!(!text.contains("rust-toolchain@stable"), "{path} takes whatever stable is");

        let uses = text.matches("dtolnay/rust-toolchain@").count();
        let pinned = after(&text, "toolchain: ");
        assert_eq!(pinned.len(), uses, "{path}: every toolchain step names its version");
        assert!(pinned.iter().all(|version| *version == rust), "{path}: {pinned:?}, not {rust}");
    }
}

#[test]
fn clang_is_llvm_s_release_by_its_hash_everywhere() {
    let script = read("docker/llvm.sh");
    let version = after(&script, "LLVM_VERSION=");
    assert_eq!(version.len(), 1, "one LLVM_VERSION in docker/llvm.sh");
    assert!(version[0].split('.').count() == 3 && version[0].split('.').all(|part| part.parse::<u32>().is_ok()), "an exact version, not {}", version[0]);

    for arch in ["LLVM_SHA256_X86_64=", "LLVM_SHA256_AARCH64="] {
        let sha = after(&script, arch);
        assert!(sha.len() == 1 && sha[0].len() == 64 && sha[0].chars().all(|c| c.is_ascii_hexdigit()), "{arch} is one SHA-256");
    }

    // The image installs it and compiles with it, and with no distribution's clang.
    let dockerfile = read("docker/Dockerfile");
    assert!(dockerfile.contains("sh /usr/local/lib/llvm.sh /opt/llvm") && dockerfile.contains("COPY docker/llvm.sh"), "the image runs docker/llvm.sh");
    assert!(dockerfile.contains("CC=/opt/llvm/bin/clang") && dockerfile.contains("CXX=/opt/llvm/bin/clang++"), "the image compiles with it");
    assert!(dockerfile.contains("PATH=/opt/llvm/bin:"), "the image puts it first on PATH, where bindgen looks");
    assert!(!dockerfile.contains("install -y --no-install-recommends clang") && !dockerfile.contains("libclang-"), "the image installs no other clang");

    // So does every workflow that names a C compiler.
    let mut builds = 0;

    for (path, text) in workflows() {
        for line in text.lines().filter(|line| line.contains("CC=") || line.contains("CXX=")) {
            assert!(line.contains("/opt/llvm/bin/clang"), "{path}: {} is not docker/llvm.sh's clang", line.trim());
        }

        if text.contains("CC=/opt/llvm/bin/clang") {
            builds += 1;
            assert!(text.contains("sh docker/llvm.sh /opt/llvm"), "{path} compiles with a clang it does not install");
            assert!(text.contains("echo \"/opt/llvm/bin\" >> \"$GITHUB_PATH\""), "{path} leaves it off PATH, where bindgen looks");
        }

        assert!(!text.contains("install -y clang-"), "{path} installs a distribution's clang");
    }

    assert!(builds > 0, "a workflow builds the node with the pinned clang");
}
