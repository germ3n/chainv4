#!/bin/sh
# The clang every Linux build of the node compiles its C and C++ with: Luau's interpreter and its
# fuel and memory metering, and RocksDB. LLVM's own release, not a distribution's package, so the
# Docker image and the CI run the very same compiler, checked against the SHA-256 below. Only clang,
# its headers and libclang (for bindgen) are unpacked.
#
#   sh docker/llvm.sh /opt/llvm      then CC=/opt/llvm/bin/clang CXX=/opt/llvm/bin/clang++
#                                    LIBCLANG_PATH=/opt/llvm/lib, and /opt/llvm/bin first on PATH:
#                                    bindgen asks that clang where clang's own headers are
#
# The hashes were checked on 2026-11-05: x86_64's against LLVM's signed build attestation
# (gh attestation verify --repo llvm/llvm-project), arm64's against its GPG signature by an LLVM
# release signing key from https://releases.llvm.org/release-keys.asc. Changing the version means
# checking the new files the same way. tests/toolchains.rs checks the image and the CI use this.
set -eu

LLVM_VERSION=19.1.7
LLVM_SHA256_X86_64=4a5ec53951a584ed36f80240f6fbf8fdd46b4cf6c7ee87cc2d5018dc37caf679
LLVM_SHA256_AARCH64=a73d9326e5d756e3937df6a9f621664d76403b59119f741901106b387e53a6ae

prefix=${1:?where to put clang, such as /opt/llvm}

case "$(uname -m)" in
    x86_64 | amd64) name=LLVM-$LLVM_VERSION-Linux-X64; sha=$LLVM_SHA256_X86_64 ;;
    aarch64 | arm64) name=clang+llvm-$LLVM_VERSION-aarch64-linux-gnu; sha=$LLVM_SHA256_AARCH64 ;;
    *) echo "no LLVM $LLVM_VERSION release pinned for $(uname -m)" >&2; exit 1 ;;
esac

# Downloaded beside where it unpacks, not to /tmp, which may be memory too small for it.
mkdir -p "$prefix"
file="$prefix/.download.tar.xz"
trap 'rm -f "$file"' EXIT
curl -fsSL --retry 3 -o "$file" "https://github.com/llvm/llvm-project/releases/download/llvmorg-$LLVM_VERSION/$name.tar.xz"
echo "$sha  $file" | sha256sum -c - > /dev/null || { echo "LLVM $LLVM_VERSION's $name.tar.xz is not the file pinned" >&2; exit 1; }

tar -xJf "$file" -C "$prefix" --strip-components=1 --wildcards \
    "$name/bin/clang" "$name/bin/clang++" "$name/bin/clang-${LLVM_VERSION%%.*}" \
    "$name/lib/clang/*" "$name/lib/libclang.so*"

"$prefix/bin/clang" --version | head -n 1
