#!/bin/sh
# Builds the wallet's Android app into wallet/target/android, as chainv4-wallet-ARCH.apk: see
# ui/build-android.sh, which builds both apps the same way.
#
#   wallet/build-android.sh [--debug] [--arch arm64|x64] [--pem FILE] [--no-picker] [--install]
exec "$(dirname "$0")/../ui/build-android.sh" "$(dirname "$0")" "$@"
