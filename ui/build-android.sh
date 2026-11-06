#!/bin/sh
# Builds one of chainv4's egui apps for Android with xbuild, into APP/target/android: an APK for
# arm64 phones and emulators, or with --arch x64 for x86-64 emulators. It is signed with xbuild's
# debug key, or with a release key: --pem FILE, or X_PEM set to the PEM's text (a certificate and
# its PKCS#8 RSA key, as docs/wallet.md shows how to make). --install puts it on the device adb sees.
# The app can pick files and use biometrics through its Java in ui/android, compiled once into
# ui/android/classes.dex (ui/android/dex.sh) and added by ui/android/repack, which signs the APK
# again: no JDK and nothing of Android's SDK is needed. --no-picker leaves it out. --store builds what an app
# store takes, as APP/target/android/NAME-ARCH-store.apk: the app's `store` feature alone, for an app
# that has one; an app with none, as the explorer, has nothing a store refuses, and builds as ever.
#
#   ui/build-android.sh APP [--debug] [--arch arm64|x64] [--pem FILE] [--no-picker] [--store] [--install]
#
# APP is the app's folder, wallet or explorer; wallet/build-android.sh and
# explorer/build-android.sh call this with their own.
set -eu

# rust-mobile/xbuild, pinned: there has been no release with what this needs since 0.2.0.
XBUILD_COMMIT=e67b501cbe0e9ca5436c223aa7ec0fe5e27544d1
XBUILD_REV=$(printf %.10s "$XBUILD_COMMIT")

usage() {
    sed -n '2,14p' "$0" | cut -c3-
}

if [ $# -eq 0 ] || [ "$1" = -h ] || [ "$1" = --help ]; then
    usage
    exit 0
fi

ui=$(cd "$(dirname "$0")" && pwd)
app=$1
shift
cd "$app"
# The package's name, which xbuild names the APK and its library after.
package=$(sed -n 's/^name = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)
profile=--release
arch=arm64
pem=
install=
no_picker=
store=

while [ $# -gt 0 ]; do
    case "$1" in
        --debug) profile=--debug ;;
        --arch) arch=$2; shift ;;
        --pem) pem=$2; shift ;;
        --no-picker) no_picker=1 ;;
        --store) store=1 ;;
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

# The app's Java, ui/android: the file picker, an activity, and biometrics, built once into
# ui/android/classes.dex and added to the APK after xbuild builds it, which then is signed again.
# The dex must be the Java's as it is now: classes.dex.sources says which it was built from.
picker=

if [ -n "$no_picker" ]; then
    echo "building without the file picker, as asked" >&2
else
    picker=1
    built_from=$(head -n 1 "$ui/android/classes.dex.sources")
    sources=$(cat "$ui"/android/*.java | shasum -a 256 | cut -d' ' -f1)

    if [ "$built_from" != "$sources" ]; then
        echo "ui/android's Java changed since classes.dex was built: run ui/android/dex.sh (a JDK and build-tools), and commit what it writes" >&2
        exit 1
    fi

    cargo build --release --locked --quiet --manifest-path "$ui/android/repack/Cargo.toml"
fi

work="target/android/work"
rm -rf "$work"
mkdir -p "$work"

# However the script ends: the manifest as it was, and no copy of a signing key left behind.
finish() {
    if [ -f "$work/manifest.yaml" ]; then
        cp "$work/manifest.yaml" manifest.yaml
    fi

    rm -rf "$work"
}

trap finish EXIT
trap 'exit 130' INT TERM

# Android refuses an app that declares code it does not hold, so without the picker the manifest's
# lines for it, each marked "# picker", are left out while xbuild reads it, and put back after.
if [ -z "$picker" ]; then
    cp manifest.yaml "$work/manifest.yaml"
    sed '/# picker$/d' "$work/manifest.yaml" > manifest.yaml
fi

set -- build --platform android --arch "$arch" --format apk "$profile"

# An app store's build: the features the app lists for one, and no others.
if [ -n "$store" ]; then
    if grep -q '^store = ' Cargo.toml; then
        set -- "$@" --no-default-features --features store
    else
        echo "$package has no store feature: nothing in it needs leaving out, so it builds as ever" >&2
        store=
    fi
fi

if [ -n "$pem" ]; then
    set -- "$@" --pem "$pem"
fi

x "$@"

built="target/x/${profile#--}/android/$package.apk"

# Copied out of xbuild's tree, which names it for the package alone, and named for what it is.
suffix=

if [ "$profile" = --debug ]; then
    suffix=-debug
fi

if [ -n "$store" ]; then
    suffix=$suffix-store
fi

apk="target/android/$package-$arch$suffix.apk"

# With the picker, the dex goes in and the APK is signed again: with the key given, or xbuild's
# debug key, which it signed with when given none, so it updates one built without.
if [ -n "$picker" ]; then
    set --
    if [ -n "$pem" ]; then set -- --pem "$pem"; fi
    "$ui/android/repack/target/release/chainv4-apk-repack" "$built" "$ui/android/classes.dex" "$apk" "$@" > /dev/null
else
    cp "$built" "$apk"
fi

echo "built $(pwd)/$apk"

if [ -n "$install" ]; then
    adb install -r "$apk"
fi
