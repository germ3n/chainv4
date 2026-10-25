#!/bin/sh
# Builds the explorer's Android app into explorer/target/android, as chainv4-explorer-ARCH.apk: see
# ui/build-android.sh, which builds both apps the same way.
#
#   explorer/build-android.sh [--debug] [--arch arm64|x64] [--pem FILE] [--install]
exec "$(dirname "$0")/../ui/build-android.sh" "$(dirname "$0")" "$@"
