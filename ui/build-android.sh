#!/bin/sh
# Builds one of chainv4's egui apps for Android with xbuild, into APP/target/android: an APK for
# arm64 phones and emulators, or with --arch x64 for x86-64 emulators. It is signed with xbuild's
# debug key, or with a release key: --pem FILE, or X_PEM set to the PEM's text (a certificate and
# its PKCS#8 RSA key, as docs/wallet.md shows how to make). --install puts it on the device adb sees.
# With a JDK and Android's build-tools, found through ANDROID_HOME, the app can pick files and use
# biometrics, its Java in ui/android, unless --no-picker leaves that out.
#
#   ui/build-android.sh APP [--debug] [--arch arm64|x64] [--pem FILE] [--no-picker] [--install]
#
# APP is the app's folder, wallet or explorer; wallet/build-android.sh and
# explorer/build-android.sh call this with their own.
set -eu

# rust-mobile/xbuild, pinned: there has been no release with what this needs since 0.2.0.
XBUILD_COMMIT=e67b501cbe0e9ca5436c223aa7ec0fe5e27544d1
XBUILD_REV=$(printf %.10s "$XBUILD_COMMIT")

usage() {
    sed -n '2,12p' "$0" | cut -c3-
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

while [ $# -gt 0 ]; do
    case "$1" in
        --debug) profile=--debug ;;
        --arch) arch=$2; shift ;;
        --pem) pem=$2; shift ;;
        --no-picker) no_picker=1 ;;
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

# The app's Java, ui/android: the file picker, an activity, and biometrics, compiled into the APK
# after xbuild builds it, which then is signed again. That takes a JDK and Android's build-tools
# (d8, zipalign and apksigner): without them the app is built without it, and leaves picking files
# and biometrics out.
build_tools() {
    for sdk in "${ANDROID_HOME:-}" "${ANDROID_SDK_ROOT:-}" "$HOME/Library/Android/sdk" "$HOME/Android/Sdk"; do
        if [ -n "$sdk" ] && [ -d "$sdk/build-tools" ]; then
            newest=$(ls "$sdk/build-tools" | sort -t . -k1,1n -k2,2n -k3,3n | tail -n 1)

            if [ -n "$newest" ] && [ -x "$sdk/build-tools/$newest/d8" ]; then
                echo "$sdk/build-tools/$newest"

                return
            fi
        fi
    done
}

tools=$(build_tools)
picker=

if [ -n "$no_picker" ]; then
    echo "building without the file picker, as asked" >&2
elif javac -version > /dev/null 2>&1 && [ -n "$tools" ] && [ -x "$tools/zipalign" ] && [ -x "$tools/apksigner" ]; then
    picker=1
else
    echo "no JDK or no Android build-tools (set ANDROID_HOME): building without the file picker" >&2
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

apk="target/android/$package-$arch$suffix.apk"

# xbuild's debug key, which it signs with when given none. An APK with the picker is signed again
# with it, so it updates one built without, as Android updates an app only with the same key.
debug_pem() {
    cargo_home=${CARGO_HOME:-$HOME/.cargo}

    for db in "$cargo_home"/git/db/xbuild-*; do
        if [ -d "$db" ] && git -C "$db" show "$XBUILD_COMMIT:apk/assets/debug.pem" 2> /dev/null; then
            return
        fi
    done

    curl -fsSL "https://raw.githubusercontent.com/rust-mobile/xbuild/$XBUILD_COMMIT/apk/assets/debug.pem"
}

if [ -n "$picker" ]; then
    # Against the newest android.jar xbuild downloaded, which the SDK may not have.
    jar=$(ls "${XDG_CACHE_HOME:-$HOME/.cache}"/x/Android.sdk/platforms/android-*/android.jar \
        "$HOME"/Library/Caches/x/Android.sdk/platforms/android-*/android.jar 2> /dev/null | sort -V | tail -n 1)

    if [ -z "$jar" ]; then
        echo "xbuild's android.jar is missing" >&2
        exit 1
    fi

    mkdir -p "$work/classes" "$work/dex" "$work/apk"
    javac --release 11 -classpath "$jar" -d "$work/classes" "$ui"/android/*.java
    "$tools/d8" --release --min-api 24 --lib "$jar" --output "$work/dex" "$work"/classes/org/chainv4/ui/*.class

    # Put together again with classes.dex: resources.arsc and the icons stored, as xbuild stores
    # them and Android wants resources.arsc, and the rest compressed.
    unzip -q "$built" -d "$work/apk"
    cp "$work/dex/classes.dex" "$work/apk/"
    (cd "$work/apk" && zip -q -X -D -0 -r ../unaligned.apk resources.arsc res && zip -q -X -D -r ../unaligned.apk . -x resources.arsc 'res/*')
    "$tools/zipalign" -f -p 4 "$work/unaligned.apk" "$work/aligned.apk"

    if [ -n "$pem" ]; then
        cp "$pem" "$work/signing.pem"
    elif [ -n "${X_PEM:-}" ]; then
        printf '%s\n' "$X_PEM" > "$work/signing.pem"
    else
        debug_pem > "$work/signing.pem"
    fi

    openssl pkcs8 -topk8 -nocrypt -in "$work/signing.pem" -outform DER -out "$work/key.pk8"
    openssl x509 -in "$work/signing.pem" -out "$work/cert.pem"
    "$tools/apksigner" sign --key "$work/key.pk8" --cert "$work/cert.pem" --v4-signing-enabled false \
        --out "$apk" "$work/aligned.apk"
else
    cp "$built" "$apk"
fi

echo "built $(pwd)/$apk"

if [ -n "$install" ]; then
    adb install -r "$apk"
fi
