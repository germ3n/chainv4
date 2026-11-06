#!/bin/sh
# Builds ui/android/classes.dex from the Java beside it, and writes classes.dex.sources, the SHA-256
# of those sources, so the APK build knows the dex is theirs without a JDK. Run it after changing the
# Java, then commit both. Needs a JDK and Android's build-tools (d8), found through ANDROID_HOME, and
# the android.jar xbuild downloads (build an APK once first). The dex comes out the same, byte for
# byte, for the same JDK, d8 and android.jar: classes.dex.sources names the ones it was built with.
#
#   ui/android/dex.sh
set -eu

here=$(cd "$(dirname "$0")" && pwd)
D8_VERSION=34.0.0
PLATFORM=33

sdk=
for candidate in "${ANDROID_HOME:-}" "${ANDROID_SDK_ROOT:-}" "$HOME/Library/Android/sdk" "$HOME/Android/Sdk"; do
    if [ -n "$candidate" ] && [ -x "$candidate/build-tools/$D8_VERSION/d8" ]; then sdk=$candidate; break; fi
done
[ -n "$sdk" ] || { echo "no build-tools $D8_VERSION (d8): sdkmanager 'build-tools;$D8_VERSION', and set ANDROID_HOME" >&2; exit 1; }

jar=
for candidate in "${XDG_CACHE_HOME:-$HOME/.cache}/x/Android.sdk/platforms/android-$PLATFORM/android.jar" "$HOME/Library/Caches/x/Android.sdk/platforms/android-$PLATFORM/android.jar" "$sdk/platforms/android-$PLATFORM/android.jar"; do
    if [ -f "$candidate" ]; then jar=$candidate; break; fi
done
[ -n "$jar" ] || { echo "no android.jar of platform $PLATFORM: build an APK once, so xbuild downloads it, or sdkmanager 'platforms;android-$PLATFORM'" >&2; exit 1; }

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/classes" "$work/dex"
javac --release 11 -nowarn -classpath "$jar" -d "$work/classes" "$here"/*.java
"$sdk/build-tools/$D8_VERSION/d8" --release --min-api 24 --lib "$jar" --output "$work/dex" "$work"/classes/org/chainv4/ui/*.class
cp "$work/dex/classes.dex" "$here/classes.dex"

sources=$(cat "$here"/*.java | shasum -a 256 | cut -d' ' -f1)
# The JDK's major version alone, as a patch release builds the same classes.
jdk=$(javac -version 2>&1 | sed -n 's/^javac \([0-9]*\).*/javac \1/p' | head -n 1)
cat > "$here/classes.dex.sources" <<END
$sources
built with $jdk, d8 $D8_VERSION, android.jar $PLATFORM
END
echo "wrote $here/classes.dex for sources $sources"
