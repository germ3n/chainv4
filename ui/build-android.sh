#!/bin/sh
# Builds one of chainv4's egui apps for Android with xbuild, into APP/target/android: an APK for
# arm64 phones and emulators, or with --arch x64 for x86-64 emulators. It is signed with xbuild's
# debug key, or with a release key: --pem FILE, or X_PEM set to the PEM's text (a certificate and
# its PKCS#8 RSA key, as docs/wallet.md shows how to make). --install puts it on the device adb sees.
#
#   ui/build-android.sh APP [--debug] [--arch arm64|x64] [--pem FILE] [--install]
#
# APP is the app's folder, wallet or explorer; wallet/build-android.sh and
# explorer/build-android.sh call this with their own.
set -eu

# rust-mobile/xbuild, pinned: there has been no release with what this needs since 0.2.0.
XBUILD_REV=e67b501cbe

usage() {
    sed -n '2,10p' "$0" | cut -c3-
}

if [ $# -eq 0 ] || [ "$1" = -h ] || [ "$1" = --help ]; then
    usage
    exit 0
fi

app=$1
shift
cd "$app"
# The package's name, which xbuild names the APK and its library after.
package=$(sed -n 's/^name = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)
profile=--release
arch=arm64
pem=
install=

while [ $# -gt 0 ]; do
    case "$1" in
        --debug) profile=--debug ;;
        --arch) arch=$2; shift ;;
        --pem) pem=$2; shift ;;
        --install) install=1 ;;
        -h | --help) usage; exit 0 ;;
        *) echo "unknown argument $1; see --help" >&2; exit 1 ;;
    esac
    shift
done

case "$arch" in
    arm64) triple=aarch64-linux-android ;;
    x64) triple=x86_64-linux-android ;;
    *) echo "--arch is arm64 or x64" >&2; exit 1 ;;
esac

# xbuild compiles and links with the host's clang and lld, and archives with llvm-ar, against an
# Android sysroot it downloads itself: no NDK needed. Homebrew keeps LLVM off the PATH.
for dir in /opt/homebrew/opt/llvm/bin /usr/local/opt/llvm/bin /opt/homebrew/opt/lld/bin /usr/local/opt/lld/bin; do
    if [ -d "$dir" ]; then
        PATH="$dir:$PATH"
    fi
done
export PATH

for tool in clang lld llvm-ar; do
    if ! command -v "$tool" > /dev/null; then
        echo "$tool is missing: install LLVM and lld (brew install llvm lld, or apt install clang lld llvm)" >&2
        exit 1
    fi
done

if ! cargo install --list | grep -q "xbuild.*rev=$XBUILD_REV"; then
    cargo install xbuild --git https://github.com/rust-mobile/xbuild --rev "$XBUILD_REV" --locked
fi

rustup target add "$triple" > /dev/null

set -- build --platform android --arch "$arch" --format apk "$profile"

if [ -n "$pem" ]; then
    set -- "$@" --pem "$pem"
fi

x "$@"

# Copied out of xbuild's tree, which names it for the package alone, and named for what it is.
suffix=

if [ "$profile" = --debug ]; then
    suffix=-debug
fi

mkdir -p target/android
apk="target/android/$package-$arch$suffix.apk"
cp "target/x/${profile#--}/android/$package.apk" "$apk"
echo "built $(pwd)/$apk"

if [ -n "$install" ]; then
    adb install -r "$apk"
fi
